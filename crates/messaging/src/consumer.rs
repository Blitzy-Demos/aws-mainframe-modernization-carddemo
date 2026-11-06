//! The consumer loop shared by the CDRD, CDRA and CP00 flows: prefetch 1,
//! acknowledge a request only after its reply is confirmed, otherwise reject
//! it without requeue.
//!
//! IBM MQ starts `CODATE01`, `COACCT01` and `COPAUA0C` by trigger. Each one
//! gets its request, answers it, and ends the task with `EXEC CICS RETURN`,
//! which commits the get and every put
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L442-454]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L538-550]. Here one long-lived consumer
//! per flow reads its request queue, and [`run`] settles every delivery with the
//! broker before it reads the next. The flows implement [`Flow`] and decide
//! only what to send. Publishing, confirming, acknowledging and rejecting all
//! happen in this module.
//!
//! # Settlement
//!
//! - **Single delivery.** The channel's prefetch is 1 and each delivery is
//!   settled before the next is read, so a flow handles one request at a time.
//! - **Persistent, confirmed publishes.** Replies and error records go to the
//!   default exchange with the queue name as routing key, `mandatory` set,
//!   `delivery_mode` 2 (persistent), `content_type` `text/plain`,
//!   `content_encoding` `utf-8` and no expiry. This is a mandated change
//!   from CP00's `MQPER-NOT-PERSISTENT` reply with `MQMD-EXPIRY` 50
//!   [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L749-750]. The channel
//!   is in confirm mode (`crate::connection::open_channel`), so every publish
//!   resolves to a broker confirmation, and only `Ack(None)` counts as
//!   delivered ([`classify`]). A publish whose confirmation has not arrived
//!   within [`connection::BROKER_TIMEOUT`] counts as not delivered
//!   ([`PublishFailure::TimedOut`]) and takes the same failure path. The
//!   channel stays open, because closing it would return the request to its
//!   queue for redelivery.
//! - **Ack after confirm.** A request is acknowledged only after its reply's
//!   `Ack(None)`. Then [`Flow::after_ack`] runs: CP00 writes its pending
//!   authorization there, after the reply, as `COPAUA0C` writes after
//!   `7100-SEND-RESPONSE`.
//! - **Reject without requeue on failure.** When there is no confirmed reply,
//!   the request is rejected with `requeue = false`:
//!   - after its error record is published to `CARD.DEMO.ERROR` and confirmed,
//!     as `9000-ERROR` puts `MQ-ERR-DISPLAY` before the task returns, and
//!     logged as an `ERROR` event that carries the record's `MQ-ERR-DISPLAY`
//!     group;
//!   - after the record is logged as the `DISPLAY MQ-ERR-DISPLAY` line when
//!     that publish fails too [app/app-vsam-mq/cbl/CODATE01.cbl:L433-439]
//!     [app/app-vsam-mq/cbl/COACCT01.cbl:L529-535];
//!   - directly when the flow has already logged the failure (CP00, which has
//!     no error queue).
//!
//!   Rejection removes the request, the outcome the COBOL commits. No path
//!   requeues. A request left unsettled because the channel failed is
//!   redelivered by the broker, which single delivery assumes never happens.
//!
//!   The acknowledgement and the rejection have no deadline. While the broker
//!   blocks the connection, lapin holds every outgoing frame, so the
//!   settlement waits until the broker unblocks it; giving up would stop the
//!   consumer and return the request to its queue for redelivery.
//!
//! # Message span
//!
//! Each delivery runs inside one `message` span with the fields `flow`,
//! `message_id`, `correlation_id`, `outcome` (`replied` or `rejected`),
//! `error_record` (`published`, `logged` or `none`) and `location`.
//!
//! - `outcome` and `error_record` are the disposition decided after the
//!   reply's or error record's confirm, recorded before the request is
//!   acknowledged or rejected.
//! - `location` is the `CCPAUERY` location or paragraph of a CP00 failure:
//!   `M004`, `C001` to `C003`, `I002`, `M003`, `3100-READ-REQUEST-MQ`,
//!   `2100-EXTRACT-REQUEST-MSG` or `6000-MAKE-DECISION` before a rejection.
//!   Every other path records `none`; a replied request
//!   records it before its acknowledgement, and CP00's [`Flow::after_ack`]
//!   then replaces it through [`record_location`] as it logs each
//!   post-acknowledgement failure: `I003`, `I004` or `8000-WRITE-AUTH-TO-DB`.
//! - A request rejected with an error record is an `ERROR` event in the span,
//!   whichever way the record went: `request rejected: MQ-ERR-DISPLAY
//!   published to the error queue` with the `MQ-ERR-DISPLAY` group published
//!   (`mq_err_display`), or `MQ-ERR-DISPLAY` with the text logged in its
//!   place. Neither carries the request body.
//! - An acknowledgement or rejection that fails is an `ERROR` event in the
//!   span: the request stays unsettled, and the broker redelivers it once the
//!   channel closes.
//! - The span is `info`, so a filter such as `RUST_LOG=warn` drops it. Every
//!   warning and error this module logs for a delivery therefore names the
//!   request itself by its `message_id` and `correlation_id`, empty when it
//!   has none: a redelivery, an unconfirmed reply, a rejection with an error
//!   record and a failed settlement.
//!
//! The binary's or test's subscriber from `carddemo_runtime::telemetry` writes
//! the span and the events as JSON lines.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::{AbortHandle, AbortRegistration, Abortable};
use lapin::acker::Acker;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicPublishOptions, BasicQosOptions, BasicRejectOptions,
};
use lapin::protocol::constants::REPLY_SUCCESS;
use lapin::publisher_confirm::Confirmation;
use lapin::types::{FieldTable, ShortString};
use lapin::{BasicProperties, Channel};
use tracing::{Instrument, Span};

use crate::MessagingError;
use crate::connection;
use crate::error_queue;

/// Deliveries the broker may hand a consumer before it settles the previous
/// one: one, so each flow handles a single request at a time.
const PREFETCH: u16 = 1;

/// AMQP 0-9-1 `delivery-mode` 2: the broker writes the message to disk on a
/// durable queue, so a reply or error record survives a broker restart.
const PERSISTENT: u8 = 2;

/// AMQP `content-type` of every reply and error record: each body is text,
/// the reply a flow's domain rule builds or the `MQ-ERR-DISPLAY` record of
/// `error_queue::record_bytes`.
const CONTENT_TYPE: &str = "text/plain";

/// AMQP `content-encoding` of every reply and error record. Each body is the
/// bytes of a Rust `String`, so UTF-8, and ASCII for every audited body; the
/// bytes are published unchanged.
const CONTENT_ENCODING: &str = "utf-8";

/// The AMQP default exchange, which routes a message to the queue its routing
/// key names, as an MQ put names its target queue.
const DEFAULT_EXCHANGE: &str = "";

/// The message span's `location` on a path that has no `CCPAUERY` location:
/// every CDRD and CDRA request, and a CP00 request whose reply and writes
/// completed.
const NO_LOCATION: &str = "none";

/// CODATE01 / COACCT01 3000-GET-REQUEST, COPAUA0C 3100-READ-REQUEST-MQ —
/// P-CODATE01-012, P-COACCT01-011, P-COPAUA0C-002.
///
/// A request as a flow sees it: the message body and the three message
/// descriptor fields the programs save after `MQGET`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The message body (`MQ-BUFFER`, `W01-GET-BUFFER`).
    pub body: Vec<u8>,
    /// AMQP `message_id`, the MQ `MSGID` (`SAVE-MSGID`).
    pub message_id: Option<String>,
    /// AMQP `correlation_id`, the MQ `CORRELID` (`SAVE-CORELID`,
    /// `WS-SAVE-CORRELID`).
    pub correlation_id: Option<String>,
    /// AMQP `reply_to`, the MQ `REPLYTOQ` (`SAVE-REPLY2Q`, `WS-REPLY-QNAME`).
    pub reply_to: Option<String>,
}

impl Request {
    /// CODATE01 3000-GET-REQUEST — P-CODATE01-012; COACCT01 3000-GET-REQUEST —
    /// P-COACCT01-011; COPAUA0C 3100-READ-REQUEST-MQ — P-COPAUA0C-002.
    ///
    /// Builds the request from a delivery's body and properties:
    /// `message_id` → `MQMD-MSGID`, `correlation_id` → `MQMD-CORRELID` and
    /// `reply_to` → `MQMD-REPLYTOQ`, as `CODATE01` saves them
    /// [app/app-vsam-mq/cbl/CODATE01.cbl:L312-322] and `COPAUA0C` saves the
    /// correlation ID and reply queue
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L410-414]. An absent
    /// property stays `None`.
    #[must_use]
    pub fn from_parts(body: Vec<u8>, properties: &BasicProperties) -> Self {
        Self {
            body,
            message_id: owned_text(properties.message_id().as_ref()),
            correlation_id: owned_text(properties.correlation_id().as_ref()),
            reply_to: owned_text(properties.reply_to().as_ref()),
        }
    }
}

/// The text of an AMQP short-string property.
fn owned_text(value: Option<&ShortString>) -> Option<String> {
    value.map(|text| text.as_str().to_owned())
}

/// CODATE01 / COACCT01 4100-PUT-REPLY, COPAUA0C 7100-SEND-RESPONSE —
/// P-CODATE01-014, P-COACCT01-017, P-COPAUA0C-012.
///
/// A reply to publish: its target queue, body and the message descriptor IDs
/// it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The queue the reply goes to: the fixed reply queue of CDRD and CDRA, or
    /// the request's `reply_to` for CP00.
    pub queue: String,
    /// The reply body, exactly as the flow's domain rule built it.
    pub body: Vec<u8>,
    /// The reply's `message_id`: the request's for CDRD and CDRA, a new one
    /// for CP00.
    pub message_id: Option<String>,
    /// The reply's `correlation_id`: the request's in every flow.
    pub correlation_id: Option<String>,
}

/// What a flow decided for one request.
///
/// `A` is the work the flow runs after the request is acknowledged
/// ([`Flow::AfterAck`]).
#[derive(Debug)]
pub enum Disposition<A> {
    /// Publish `reply`; once it is confirmed, acknowledge the request, then run
    /// [`Flow::after_ack`] with `after_ack`.
    Reply {
        /// The reply to publish.
        reply: Reply,
        /// The work to run once the request is acknowledged.
        after_ack: A,
    },
    /// No reply exists: publish `record` to `CARD.DEMO.ERROR`, then reject the
    /// request without requeue (CDRA's account read error, COACCT01
    /// `9000-ERROR` — P-COACCT01-015).
    ErrorRecord {
        /// The error message body (`error_queue::record_bytes`).
        record: Vec<u8>,
    },
    /// No reply exists and the flow has already logged why: reject the request
    /// without requeue (a CP00 critical path). `location` is recorded in the
    /// message span.
    Reject {
        /// The `CCPAUERY` location (`ERR-LOCATION`) or paragraph of the failure.
        location: &'static str,
    },
}

/// Why a publish did not count as delivered.
#[derive(Debug)]
pub enum PublishFailure {
    /// The broker returned the mandatory publish as unroutable: the target
    /// queue does not exist.
    Returned,
    /// The broker nacked the publish.
    Nacked,
    /// The channel is not in confirm mode, so the publish was never confirmed.
    NotRequested,
    /// The channel failed during the publish or while awaiting its
    /// confirmation.
    Channel(lapin::Error),
    /// The publish and its confirmation did not complete within
    /// [`connection::BROKER_TIMEOUT`], for example because the broker blocked
    /// the connection or withheld the confirm, so the message counts as not
    /// delivered. The broker may still route it later.
    ///
    /// The channel is left open: closing it would return the unsettled
    /// request to its queue, and the broker would redeliver it.
    TimedOut,
}

impl PublishFailure {
    /// CODATE01 / COACCT01 4100-PUT-REPLY `WHEN OTHER` — P-CODATE01-015,
    /// P-CODATE01-019, P-COACCT01-017.
    ///
    /// The `(MQ-APPL-CONDITION-CODE, MQ-APPL-REASON-CODE)` pair that stands
    /// for this failure in an `MQPUT ERR` record or display, from the
    /// constants of [`error_queue`].
    #[must_use]
    pub fn mq_codes(&self) -> (u32, u32) {
        let reason = match self {
            Self::Returned => error_queue::REASON_RETURNED,
            Self::Nacked => error_queue::REASON_NACKED,
            Self::NotRequested | Self::TimedOut => error_queue::REASON_NOT_CONFIRMED,
            Self::Channel(_) => error_queue::REASON_CHANNEL_ERROR,
        };
        (error_queue::MQPUT_FAILED_CONDITION_CODE, reason)
    }
}

impl fmt::Display for PublishFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Returned => f.write_str("returned unroutable"),
            Self::Nacked => f.write_str("nacked"),
            Self::NotRequested => f.write_str("not confirmed"),
            Self::Channel(error) => write!(f, "channel error: {error}"),
            Self::TimedOut => write!(f, "not confirmed within {:?}", connection::BROKER_TIMEOUT),
        }
    }
}

/// What a flow does when its reply was not confirmed. Either way the request
/// is then rejected without requeue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureAction {
    /// Publish this `MQPUT ERR` record to `CARD.DEMO.ERROR` first (CDRD and
    /// CDRA, 4100-PUT-REPLY `WHEN OTHER` — P-CODATE01-015, P-COACCT01-017).
    ErrorRecord(Vec<u8>),
    /// The flow has logged the failure itself (CP00 `M004` —
    /// P-COPAUA0C-012). `location` is recorded in the message span.
    Logged {
        /// The `CCPAUERY` location of the logged failure.
        location: &'static str,
    },
}

/// One request/reply flow served by [`run`].
///
/// Each method returns a `Send` future rather than being an `async fn`, so the
/// consumer can run on tokio's multi-threaded runtime. Implementations may
/// still write `async fn` in their `impl`; the compiler checks the `Send`
/// bound there.
pub trait Flow: Send + Sync + 'static {
    /// The transaction ID: `"CDRD"`, `"CDRA"` or `"CP00"`. It names the
    /// consumer tag and the `flow` field of the message span.
    const NAME: &'static str;

    /// The work [`Flow::after_ack`] receives with a confirmed, acknowledged
    /// reply.
    type AfterAck: Send;

    /// Decides what to send for `req`: a reply, an error record or a logged
    /// rejection. Publishing and settling are left to the consumer.
    fn handle<'a>(
        &'a self,
        req: &'a Request,
    ) -> impl Future<Output = Disposition<Self::AfterAck>> + Send + 'a;

    /// Called when the reply of [`Disposition::Reply`] was not confirmed.
    /// Returns the error record to publish, or the location the flow logged.
    fn on_reply_failure(&self, req: &Request, failure: &PublishFailure) -> FailureAction;

    /// Runs `work` once the reply is confirmed and the request acknowledged.
    /// Nothing it does reaches the broker: the request is already settled.
    ///
    /// It runs inside the request's message span, whose `location` is already
    /// `none`. A flow reports the location of a failure it logs here with
    /// [`record_location`], at the moment it logs it.
    fn after_ack<'a>(
        &'a self,
        req: &'a Request,
        work: Self::AfterAck,
    ) -> impl Future<Output = ()> + Send + 'a;
}

/// For a flow's [`Flow::after_ack`], which runs inside the request's message
/// span: records `location`, the `CCPAUERY` location or paragraph of a failure,
/// as the span's `location` as soon as the failure is logged, replacing
/// `none`.
///
/// A later call replaces an earlier one, so the span keeps the last failure
/// logged, and cancelling the remaining work cannot lose it.
pub fn record_location(location: &'static str) {
    Span::current().record("location", location);
}

/// Maps a publisher confirmation to the publish's outcome.
///
/// Only `Ack(None)` is a delivered message. `Ack(Some(_))` carries the
/// mandatory publish the broker returned because no queue matched.
///
/// # Errors
///
/// Returns [`PublishFailure::Returned`] for `Ack(Some(_))`,
/// [`PublishFailure::Nacked`] for `Nack(_)` and
/// [`PublishFailure::NotRequested`] for `NotRequested`.
pub fn classify(confirmation: Confirmation) -> Result<(), PublishFailure> {
    match confirmation {
        Confirmation::Ack(None) => Ok(()),
        // A named binding, not `_`, moves the returned message out, so the
        // by-value parameter is consumed and clippy's `needless_pass_by_value`
        // holds without a suppression.
        Confirmation::Ack(Some(_returned)) => Err(PublishFailure::Returned),
        Confirmation::Nack(_) => Err(PublishFailure::Nacked),
        Confirmation::NotRequested => Err(PublishFailure::NotRequested),
    }
}

/// CODATE01 / COACCT01 4100-PUT-REPLY and 9000-ERROR, COPAUA0C
/// 7100-SEND-RESPONSE — P-CODATE01-014, P-CODATE01-016, P-COACCT01-017,
/// P-COACCT01-018, P-COPAUA0C-012.
///
/// Publishes `body` to `queue` through the default exchange, mandatory, with
/// the [`published_properties`], and waits for the broker's confirmation.
///
/// The publish and the confirmation together are held to
/// [`connection::BROKER_TIMEOUT`], so a broker that stays connected but
/// blocks the connection or withholds the confirm cannot hold the
/// prefetch-1 consumer: the message then counts as not delivered,
/// [`PublishFailure::TimedOut`].
async fn publish_confirmed(
    channel: &Channel,
    queue: &str,
    body: &[u8],
    message_id: Option<&str>,
    correlation_id: Option<&str>,
) -> Result<(), PublishFailure> {
    let properties = published_properties(message_id, correlation_id);
    let options = BasicPublishOptions {
        mandatory: true,
        ..BasicPublishOptions::default()
    };
    confirmed_within(connection::BROKER_TIMEOUT, async {
        let confirm = channel
            .basic_publish(DEFAULT_EXCHANGE, queue, options, body, properties)
            .await
            .map_err(PublishFailure::Channel)?;
        let confirmation = confirm.await.map_err(PublishFailure::Channel)?;
        classify(confirmation)
    })
    .await
}

/// The properties of every reply and error record: persistent, labelled
/// [`CONTENT_TYPE`] in [`CONTENT_ENCODING`], with no expiry, carrying
/// `message_id` and `correlation_id` when they are given.
fn published_properties(message_id: Option<&str>, correlation_id: Option<&str>) -> BasicProperties {
    let mut properties = BasicProperties::default()
        .with_delivery_mode(PERSISTENT)
        .with_content_type(CONTENT_TYPE.into())
        .with_content_encoding(CONTENT_ENCODING.into());
    if let Some(id) = message_id {
        properties = properties.with_message_id(id.into());
    }
    if let Some(id) = correlation_id {
        properties = properties.with_correlation_id(id.into());
    }
    properties
}

/// Waits for `delivery`, a publish and its confirmation, for at most `limit`.
///
/// The timer is tokio's, as in `crate::connection::bounded`. The abandoned
/// publish is dropped; lapin keeps its pending confirmation, and the channel
/// stays open.
///
/// # Errors
///
/// Returns the failure `delivery` resolves to, and
/// [`PublishFailure::TimedOut`] when it has not resolved within `limit`.
async fn confirmed_within(
    limit: Duration,
    delivery: impl Future<Output = Result<(), PublishFailure>>,
) -> Result<(), PublishFailure> {
    match tokio::time::timeout(limit, delivery).await {
        Ok(outcome) => outcome,
        Err(_elapsed) => Err(PublishFailure::TimedOut),
    }
}

/// Consumes `queue` on `channel` with `flow` until the stream ends or fails.
///
/// This is [`run_until`] with a stop that nothing can signal, so it returns
/// only an error.
///
/// # Errors
///
/// Returns the errors of [`run_until`].
pub async fn run<F: Flow>(channel: Channel, queue: &str, flow: F) -> Result<(), MessagingError> {
    // The handle is dropped at once, so the stop is never signalled.
    let (_, never) = AbortHandle::new_pair();
    run_until(channel, queue, flow, never).await
}

/// Consumes `queue` on `channel` with `flow` until `stop` is signalled or the
/// stream ends or fails.
///
/// Sets the channel's prefetch to 1, starts a manually acknowledged consumer
/// tagged `carddemo-<flow>` (for example `carddemo-cdrd`) and settles each
/// delivery before reading the next, as the [module](self) describes.
/// `channel` must be in confirm mode, as `crate::connection::open_channel`
/// leaves it; otherwise every publish fails as
/// [`PublishFailure::NotRequested`] and every request is rejected.
///
/// `stop` is checked only between deliveries. Once its [`AbortHandle`] is
/// aborted, the delivery being settled still finishes, its publishes, its
/// acknowledgement or rejection and [`Flow::after_ack`] included, and no other
/// delivery is handled: one the broker has already sent stays unsettled, and
/// the broker redelivers it once the channel closes. Then `run_until` closes
/// the channel and returns `Ok(())`.
///
/// A request whose settlement fails is left unsettled; the broker redelivers it
/// once the channel closes.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when `basic.qos`, `basic.consume`, an
/// acknowledgement, a rejection, the delivery stream or the closing
/// `channel.close` fails, [`MessagingError::BrokerTimeout`] when the broker
/// does not answer `basic.qos`, `basic.consume` or that `channel.close` within
/// [`connection::BROKER_TIMEOUT`], and
/// [`MessagingError::ConsumerStopped`] when the broker ends the delivery stream
/// before `stop` is signalled, for example because the queue was deleted.
pub async fn run_until<F: Flow>(
    channel: Channel,
    queue: &str,
    flow: F,
    stop: AbortRegistration,
) -> Result<(), MessagingError> {
    let session = Session {
        channel,
        queue: queue.to_owned(),
        flow,
    };
    session.consume(stop).await
}

/// The channel, queue and flow of one running consumer.
struct Session<F> {
    channel: Channel,
    queue: String,
    flow: F,
}

impl<F: Flow> Session<F> {
    /// CODATE01 / COACCT01 4000-MAIN-PROCESS, COPAUA0C 2000-MAIN-PROCESS —
    /// P-CODATE01-024, P-COPAUA0C-003: get and answer requests one at a time,
    /// until `stop` is signalled between two of them.
    async fn consume(&self, stop: AbortRegistration) -> Result<(), MessagingError> {
        connection::bounded(
            "basic.qos",
            connection::BROKER_TIMEOUT,
            self.channel.basic_qos(PREFETCH, BasicQosOptions::default()),
        )
        .await?;
        let consumer_tag = format!("carddemo-{}", F::NAME.to_ascii_lowercase());
        let consumer = connection::bounded(
            "basic.consume",
            connection::BROKER_TIMEOUT,
            self.channel.basic_consume(
                &self.queue,
                &consumer_tag,
                BasicConsumeOptions::default(),
                FieldTable::default(),
            ),
        )
        .await?;
        tracing::info!(
            flow = F::NAME,
            queue = %self.queue,
            consumer_tag = %consumer.tag(),
            "consumer started"
        );
        // The stream is polled only between settlements, so a signalled stop
        // ends it before the next delivery and never inside the current one.
        let mut deliveries = Abortable::new(consumer, stop);
        while let Some(item) = deliveries.next().await {
            let delivery = item.map_err(MessagingError::Amqp)?;
            self.settle(delivery).await?;
        }
        if !deliveries.is_aborted() {
            return Err(MessagingError::ConsumerStopped { flow: F::NAME });
        }
        // Closed here, while `deliveries` still holds the consumer, and not by
        // dropping the handles: lapin runs a dropped channel's close in the
        // background, and one that runs after `Running::shutdown` has begun the
        // connection close fails and turns into a connection error, which fails
        // that close. The closed channel cancels the consumer, so dropping
        // `deliveries` afterwards sends nothing.
        connection::bounded(
            "channel.close",
            connection::BROKER_TIMEOUT,
            self.channel.close(REPLY_SUCCESS, "consumer stopped"),
        )
        .await
    }

    /// Handles one delivery inside its message span and settles it with the
    /// broker.
    async fn settle(&self, delivery: Delivery) -> Result<(), MessagingError> {
        let Delivery {
            data,
            properties,
            acker,
            redelivered,
            ..
        } = delivery;
        let req = Request::from_parts(data, &properties);
        let span = tracing::info_span!(
            "message",
            flow = F::NAME,
            message_id = req.message_id.as_deref().unwrap_or(""),
            correlation_id = req.correlation_id.as_deref().unwrap_or(""),
            outcome = tracing::field::Empty,
            error_record = tracing::field::Empty,
            location = tracing::field::Empty,
        );
        async {
            if redelivered {
                // Single delivery is the parity assumption; a redelivery follows
                // a lost channel and may repeat CP00's post-acknowledgement writes.
                tracing::warn!(
                    flow = F::NAME,
                    message_id = req.message_id.as_deref().unwrap_or(""),
                    correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                    "request redelivered after a lost channel"
                );
            }
            self.dispatch(&acker, &req, &span).await
        }
        .instrument(span.clone())
        .await
    }

    /// Runs the flow and carries out its disposition.
    async fn dispatch(
        &self,
        acker: &Acker,
        req: &Request,
        span: &Span,
    ) -> Result<(), MessagingError> {
        match self.flow.handle(req).await {
            Disposition::Reply { reply, after_ack } => {
                let published = publish_confirmed(
                    &self.channel,
                    &reply.queue,
                    &reply.body,
                    reply.message_id.as_deref(),
                    reply.correlation_id.as_deref(),
                )
                .await;
                match published {
                    Ok(()) => self.acknowledge(acker, req, after_ack, span).await,
                    Err(failure) => {
                        tracing::warn!(
                            flow = F::NAME,
                            message_id = req.message_id.as_deref().unwrap_or(""),
                            correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                            queue = %reply.queue,
                            failure = %failure,
                            "reply not confirmed"
                        );
                        match self.flow.on_reply_failure(req, &failure) {
                            FailureAction::ErrorRecord(record) => {
                                self.reject_with_record(acker, req, &record, span).await
                            }
                            FailureAction::Logged { location } => {
                                reject_logged(acker, F::NAME, req, location, span).await
                            }
                        }
                    }
                }
            }
            Disposition::ErrorRecord { record } => {
                self.reject_with_record(acker, req, &record, span).await
            }
            Disposition::Reject { location } => {
                reject_logged(acker, F::NAME, req, location, span).await
            }
        }
    }

    /// Acknowledges a request whose reply is confirmed, then runs
    /// [`Flow::after_ack`].
    ///
    /// `outcome`, `error_record` and a `location` of `none` are recorded
    /// before the acknowledgement, since the reply is already delivered, so a
    /// span whose remaining work is cancelled still carries all three. The
    /// flow replaces `location` through [`record_location`] as it logs a
    /// failure. An acknowledgement that fails leaves the request to be
    /// redelivered, so the post-acknowledgement work does not run.
    async fn acknowledge(
        &self,
        acker: &Acker,
        req: &Request,
        work: F::AfterAck,
        span: &Span,
    ) -> Result<(), MessagingError> {
        span.record("outcome", "replied");
        span.record("error_record", "none");
        span.record("location", NO_LOCATION);
        if let Err(error) = acker.ack(BasicAckOptions::default()).await {
            return Err(unsettled(F::NAME, req, "acknowledged", error));
        }
        self.flow.after_ack(req, work).await;
        Ok(())
    }

    /// CODATE01 / COACCT01 9000-ERROR — P-CODATE01-015, P-CODATE01-016,
    /// P-COACCT01-015, P-COACCT01-017, P-COACCT01-018.
    ///
    /// Publishes `record` to `CARD.DEMO.ERROR` with the request's IDs, which
    /// the COBOL's put keeps because it reuses the request's message
    /// descriptor. When that publish is confirmed, logs at `error` the
    /// rejection with the request's IDs, the error queue and the
    /// `MQ-ERR-DISPLAY` group of `record`, so the reason stays in the log as
    /// well as on the queue. When it is not confirmed, logs the
    /// `DISPLAY MQ-ERR-DISPLAY` text [app/app-vsam-mq/cbl/CODATE01.cbl:L433-439]
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L529-535]. Neither event carries the
    /// request body. Then rejects the request without requeue: the COBOL ends
    /// the task with a normal `RETURN`, which commits the get.
    async fn reject_with_record(
        &self,
        acker: &Acker,
        req: &Request,
        record: &[u8],
        span: &Span,
    ) -> Result<(), MessagingError> {
        let published = publish_confirmed(
            &self.channel,
            error_queue::ERROR_QUEUE,
            record,
            req.message_id.as_deref(),
            req.correlation_id.as_deref(),
        )
        .await;
        match published {
            Ok(()) => {
                tracing::error!(
                    flow = F::NAME,
                    message_id = req.message_id.as_deref().unwrap_or(""),
                    correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                    queue = error_queue::ERROR_QUEUE,
                    mq_err_display = %error_queue::display_text(record),
                    "request rejected: MQ-ERR-DISPLAY published to the error queue"
                );
                span.record("error_record", "published");
            }
            Err(failure) => {
                let (condition_code, reason_code) = failure.mq_codes();
                tracing::error!(
                    flow = F::NAME,
                    message_id = req.message_id.as_deref().unwrap_or(""),
                    correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                    failure = %failure,
                    mq_err_display = %error_queue::error_put_failure_display(
                        record,
                        condition_code,
                        reason_code,
                    ),
                    "MQ-ERR-DISPLAY"
                );
                span.record("error_record", "logged");
            }
        }
        span.record("outcome", "rejected");
        span.record("location", NO_LOCATION);
        reject(acker, F::NAME, req).await
    }
}

/// Rejects the request `req` of `flow` whose failure the flow has already
/// logged, recording the flow's `location` in the message span first.
async fn reject_logged(
    acker: &Acker,
    flow: &'static str,
    req: &Request,
    location: &'static str,
    span: &Span,
) -> Result<(), MessagingError> {
    span.record("outcome", "rejected");
    span.record("error_record", "none");
    span.record("location", location);
    reject(acker, flow, req).await
}

/// Removes the request `req` of `flow` from its queue without a reply:
/// `basic.reject` with `requeue = false`, the only way this module settles a
/// failed request.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when the rejection fails, after logging
/// that the request stays unsettled.
async fn reject(acker: &Acker, flow: &'static str, req: &Request) -> Result<(), MessagingError> {
    acker
        .reject(BasicRejectOptions { requeue: false })
        .await
        .map_err(|error| unsettled(flow, req, "rejected", error))
}

/// Logs, as an `ERROR` event in the message span that names `req` by its
/// IDs, that the request of `flow` was not `settlement` (`acknowledged` or
/// `rejected`) because `basic.ack` or `basic.reject` failed with `error`. The
/// request stays unsettled, and the broker redelivers it once the channel
/// closes. Returns the error the consumer stops with.
fn unsettled(
    flow: &'static str,
    req: &Request,
    settlement: &'static str,
    error: lapin::Error,
) -> MessagingError {
    tracing::error!(
        flow,
        message_id = req.message_id.as_deref().unwrap_or(""),
        correlation_id = req.correlation_id.as_deref().unwrap_or(""),
        error = %error,
        "request not {settlement}: it stays unsettled, and the broker redelivers it once the channel closes"
    );
    MessagingError::Amqp(error)
}

#[cfg(test)]
mod tests {
    use std::future::{self, Future};
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_util::stream::AbortRegistration;
    use lapin::acker::Acker;
    use lapin::message::{BasicReturnMessage, Delivery};
    use lapin::publisher_confirm::Confirmation;
    use lapin::types::ShortString;
    use lapin::{BasicProperties, Channel, ChannelState};

    use super::{
        Disposition, FailureAction, Flow, NO_LOCATION, PublishFailure, Reply, Request, classify,
        confirmed_within, published_properties, record_location, run, run_until, unsettled,
    };
    use crate::MessagingError;
    use crate::error_queue;

    fn assert_send<T: Send>(_: &T) {}

    /// A flow written with `async fn`, as the CDRD, CDRA and CP00 flows may be.
    struct EchoFlow;

    impl Flow for EchoFlow {
        const NAME: &'static str = "ECHO";
        type AfterAck = Vec<u8>;

        async fn handle(&self, req: &Request) -> Disposition<Vec<u8>> {
            let body = future::ready(req.body.clone()).await;
            Disposition::Reply {
                reply: Reply {
                    queue: "ECHO.REPLY".to_owned(),
                    body: body.clone(),
                    message_id: req.message_id.clone(),
                    correlation_id: req.correlation_id.clone(),
                },
                after_ack: body,
            }
        }

        fn on_reply_failure(&self, _req: &Request, failure: &PublishFailure) -> FailureAction {
            let (condition_code, _) = failure.mq_codes();
            FailureAction::ErrorRecord(condition_code.to_string().into_bytes())
        }

        fn after_ack<'a>(
            &'a self,
            _req: &'a Request,
            work: Vec<u8>,
        ) -> impl Future<Output = ()> + Send + 'a {
            drop(work);
            future::ready(())
        }
    }

    fn channel_error() -> PublishFailure {
        PublishFailure::Channel(lapin::Error::InvalidChannelState(ChannelState::Closed))
    }

    /// A mandatory publish as the broker returns it when no queue is bound to
    /// its routing key on the default exchange (`basic.return` 312 `NO_ROUTE`).
    fn returned() -> Box<BasicReturnMessage> {
        Box::new(BasicReturnMessage {
            delivery: Delivery {
                delivery_tag: 0,
                exchange: "".into(),
                routing_key: "QA.NOPE".into(),
                redelivered: false,
                properties: BasicProperties::default(),
                data: b"REPLY".to_vec(),
                // `Acker::default`, the only public constructor of `Acker`,
                // builds the same unusable acker lapin gives every returned
                // message, and logs a warning saying so. Nothing acks a
                // returned publish, nor does this test.
                acker: Acker::default(),
            },
            reply_code: 312,
            reply_text: "NO_ROUTE".into(),
        })
    }

    #[test]
    fn only_an_unreturned_ack_is_delivered() {
        /// A confirmation shape, a builder for it and the outcome it must get.
        type Case = (
            &'static str,
            fn() -> Confirmation,
            fn(&Result<(), PublishFailure>) -> bool,
        );

        // Every shape a publisher confirmation takes. `Confirmation` is not
        // `Clone`, so each case builds its own.
        let cases: [Case; 5] = [
            (
                "Ack(None)",
                || Confirmation::Ack(None),
                |outcome| matches!(outcome, Ok(())),
            ),
            (
                "Ack(Some(returned))",
                || Confirmation::Ack(Some(returned())),
                |outcome| matches!(outcome, Err(PublishFailure::Returned)),
            ),
            (
                "Nack(Some(returned))",
                || Confirmation::Nack(Some(returned())),
                |outcome| matches!(outcome, Err(PublishFailure::Nacked)),
            ),
            (
                "Nack(None)",
                || Confirmation::Nack(None),
                |outcome| matches!(outcome, Err(PublishFailure::Nacked)),
            ),
            (
                "NotRequested",
                || Confirmation::NotRequested,
                |outcome| matches!(outcome, Err(PublishFailure::NotRequested)),
            ),
        ];
        let mut delivered = Vec::new();
        for (shape, confirmation, expected) in cases {
            let outcome = classify(confirmation());
            assert!(expected(&outcome), "{shape} classified as {outcome:?}");
            if outcome.is_ok() {
                delivered.push(shape);
            }
        }
        assert_eq!(delivered, ["Ack(None)"]);
    }

    #[test]
    fn request_takes_the_three_descriptor_ids() {
        // P-CODATE01-012, P-COACCT01-011, P-COPAUA0C-002: MSGID, CORRELID and
        // REPLYTOQ saved after the get.
        let properties = BasicProperties::default()
            .with_message_id("MSG-1".into())
            .with_correlation_id("CORR-1".into())
            .with_reply_to("REPLY.Q".into());
        let req = Request::from_parts(b"INQA00000000001".to_vec(), &properties);
        assert_eq!(
            req,
            Request {
                body: b"INQA00000000001".to_vec(),
                message_id: Some("MSG-1".to_owned()),
                correlation_id: Some("CORR-1".to_owned()),
                reply_to: Some("REPLY.Q".to_owned()),
            }
        );

        let bare = Request::from_parts(Vec::new(), &BasicProperties::default());
        assert_eq!(bare.message_id, None);
        assert_eq!(bare.correlation_id, None);
        assert_eq!(bare.reply_to, None);
        assert!(bare.body.is_empty());
    }

    #[test]
    fn publish_failures_map_to_the_error_queue_codes_and_texts() {
        // P-CODATE01-015, P-CODATE01-019, P-COACCT01-017: MQPUT ERR codes.
        let cases = [
            (
                PublishFailure::Returned,
                error_queue::REASON_RETURNED,
                "returned unroutable".to_owned(),
            ),
            (
                PublishFailure::Nacked,
                error_queue::REASON_NACKED,
                "nacked".to_owned(),
            ),
            (
                PublishFailure::NotRequested,
                error_queue::REASON_NOT_CONFIRMED,
                "not confirmed".to_owned(),
            ),
            (
                channel_error(),
                error_queue::REASON_CHANNEL_ERROR,
                format!(
                    "channel error: {}",
                    lapin::Error::InvalidChannelState(ChannelState::Closed)
                ),
            ),
            (
                PublishFailure::TimedOut,
                error_queue::REASON_NOT_CONFIRMED,
                "not confirmed within 10s".to_owned(),
            ),
        ];
        for (failure, reason, text) in cases {
            assert_eq!(
                failure.mq_codes(),
                (error_queue::MQPUT_FAILED_CONDITION_CODE, reason),
                "{failure:?}"
            );
            assert_eq!(failure.to_string(), text);
        }
    }

    #[tokio::test]
    async fn a_publish_and_its_confirm_are_held_to_the_deadline() {
        // A confirm the broker withholds past the limit is a publish not
        // delivered; an outcome within the limit passes through unchanged.
        let cases = [
            (None, "Err(TimedOut)"),
            (Some(Ok(())), "Ok(())"),
            (Some(Err(PublishFailure::Nacked)), "Err(Nacked)"),
        ];
        for (answer, expected) in cases {
            let delivery = async move {
                match answer {
                    Some(outcome) => outcome,
                    None => future::pending().await,
                }
            };
            let outcome = confirmed_within(Duration::from_millis(10), delivery).await;
            assert_eq!(format!("{outcome:?}"), expected);
        }
    }

    #[test]
    fn replies_and_error_records_are_persistent_utf8_text() {
        // P-CODATE01-014, P-COACCT01-017, P-COPAUA0C-012: the IDs a put
        // carries, labelled as the text the domain rules build.
        let cases = [
            (Some("MSG-1"), Some("CORR-1")),
            (None, Some("CORR-1")),
            (Some("MSG-1"), None),
            (None, None),
        ];
        for (message_id, correlation_id) in cases {
            let properties = published_properties(message_id, correlation_id);
            let text = |value: &Option<ShortString>| value.as_ref().map(ShortString::to_string);
            assert_eq!(*properties.delivery_mode(), Some(2));
            assert_eq!(
                text(properties.content_type()).as_deref(),
                Some("text/plain")
            );
            assert_eq!(
                text(properties.content_encoding()).as_deref(),
                Some("utf-8")
            );
            assert_eq!(text(properties.expiration()), None);
            assert_eq!(text(properties.message_id()).as_deref(), message_id);
            assert_eq!(text(properties.correlation_id()).as_deref(), correlation_id);
        }
    }

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            let bytes = self.0.lock().expect("capture buffer").clone();
            String::from_utf8(bytes).expect("JSON lines are UTF-8")
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_settlement_is_one_error_event_and_the_amqp_error() {
        // A request whose ack or reject fails is never reported as settled.
        // Outside any span, the event still names the request by its IDs.
        let properties = BasicProperties::default()
            .with_message_id("MSG-9".into())
            .with_correlation_id("CORR-9".into());
        let req = Request::from_parts(b"INQA00000000001".to_vec(), &properties);
        for settlement in ["acknowledged", "rejected"] {
            let capture = Capture::default();
            let sink = capture.clone();
            let (subscriber, guard) =
                carddemo_runtime::telemetry::subscriber_with_writer("mq", "info", move || {
                    sink.clone()
                })
                .expect("valid directives");
            let closed = lapin::Error::InvalidChannelState(ChannelState::Closed);
            let error = tracing::subscriber::with_default(subscriber, || {
                unsettled("ECHO", &req, settlement, closed)
            });
            guard.shutdown().expect("the tracer provider shuts down");
            assert!(
                matches!(
                    error,
                    MessagingError::Amqp(lapin::Error::InvalidChannelState(ChannelState::Closed))
                ),
                "{error:?}"
            );
            let text = capture.text();
            let mut lines = text.lines();
            let line = lines.next().expect("one JSON line");
            assert_eq!(lines.next(), None, "{text}");
            let message = format!(
                "\"message\":\"request not {settlement}: it stays unsettled, and the broker \
                 redelivers it once the channel closes\""
            );
            for expected in [
                "\"level\":\"ERROR\"",
                "\"flow\":\"ECHO\"",
                "\"message_id\":\"MSG-9\"",
                "\"correlation_id\":\"CORR-9\"",
                &message,
            ] {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
            assert!(!line.contains("INQA00000000001"), "{line}");
        }
    }

    #[test]
    fn a_replied_span_keeps_none_or_the_last_post_ack_location_recorded() {
        // P-COPAUA0C-035: a failed commit's I003 replaces the I004 logged
        // before it; with no failure logged the span keeps `none`.
        let cases: [(&[&'static str], &str); 2] = [(&[], "none"), (&["I004", "I003"], "I003")];
        for (logged, expected) in cases {
            let capture = Capture::default();
            let sink = capture.clone();
            let (subscriber, guard) =
                carddemo_runtime::telemetry::subscriber_with_writer("mq", "info", move || {
                    sink.clone()
                })
                .expect("valid directives");
            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!("message", location = tracing::field::Empty);
                span.record("location", NO_LOCATION);
                let _entered = span.enter();
                for &location in logged {
                    record_location(location);
                }
                tracing::info!("post-acknowledgement work ended");
            });
            guard.shutdown().expect("the tracer provider shuts down");
            let text = capture.text();
            let field = format!("\"location\":\"{expected}\"");
            let event = text
                .lines()
                .find(|line| line.contains("post-acknowledgement work ended"))
                .expect("one event line");
            assert!(event.contains(&field), "{field} missing from {event}");
            let span = text
                .lines()
                .find(|line| line.contains("\"type\":\"span\""))
                .expect("one span line");
            let attributes = span
                .split_once("\"attributes\":")
                .expect("span attributes")
                .1;
            assert!(attributes.contains(&field), "{field} missing from {span}");
            assert_eq!(
                attributes.matches("\"location\"").count(),
                1,
                "one location attribute in {span}"
            );
        }
    }

    #[test]
    fn flow_values_and_the_consumer_future_are_send() {
        // Type-checked for every `F: Flow`, so `run` and `run_until` stay
        // spawnable on tokio's multi-threaded runtime whatever the flow.
        fn run_is_send<F: Flow>(channel: Channel, flow: F) {
            assert_send(&run(channel, "QUEUE", flow));
        }
        fn run_until_is_send<F: Flow>(channel: Channel, flow: F, stop: AbortRegistration) {
            assert_send(&run_until(channel, "QUEUE", flow, stop));
        }

        let req = Request::from_parts(b"x".to_vec(), &BasicProperties::default());
        assert_send(&req);
        let disposition: Disposition<()> = Disposition::Reject { location: "M004" };
        assert_send(&disposition);
        assert_send(&EchoFlow.handle(&req));
        let check: fn(Channel, EchoFlow) = run_is_send::<EchoFlow>;
        assert_send(&check);
        let check_until: fn(Channel, EchoFlow, AbortRegistration) = run_until_is_send::<EchoFlow>;
        assert_send(&check_until);
    }

    #[tokio::test]
    async fn async_fn_flows_satisfy_the_trait() {
        let properties = BasicProperties::default().with_correlation_id("CORR-2".into());
        let req = Request::from_parts(b"ABC".to_vec(), &properties);
        let Disposition::Reply { reply, after_ack } = EchoFlow.handle(&req).await else {
            panic!("EchoFlow replies");
        };
        assert_eq!(reply.correlation_id.as_deref(), Some("CORR-2"));
        assert_eq!(reply.body, b"ABC");
        EchoFlow.after_ack(&req, after_ack).await;
        assert_eq!(
            EchoFlow.on_reply_failure(&req, &PublishFailure::Returned),
            FailureAction::ErrorRecord(
                error_queue::MQPUT_FAILED_CONDITION_CODE
                    .to_string()
                    .into_bytes()
            )
        );
    }
}
