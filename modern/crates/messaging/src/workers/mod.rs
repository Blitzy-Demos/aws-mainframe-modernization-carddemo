//! The three `RabbitMQ` workers that replace the MQ-triggered CICS programs (AAP 0.3.4.2):
//! [`cdrd`] for `CODATE01`, [`cdra`] for `COACCT01` and [`cp00`] for `COPAUA0C`.
//!
//! This module owns what the three would otherwise repeat: the [`Step`] trait each worker
//! implements, the poll loop [`drive`] that runs one worker until a stop or a lost connection, and
//! the crate-internal plumbing every flow uses, the check of the configured request queue name,
//! the worker's channel pair, the request span with its settlement event and the put path of the
//! two fixed-queue flows. Each child module owns only its own flow.
//!
//! # Layering
//!
//! - `domain::messaging` parses every request, makes every decision and formats every reply,
//!   error body and record. No body layout lives in this crate.
//! - `persistence` owns every SQL statement; the workers call its repository traits.
//! - `crate::broker` owns the AMQP rules: the two channels, the passive queue checks, the
//!   `basic_get` poll, the confirmed mandatory publish, the reply properties, the
//!   acknowledgement seam and the reconnect backoff.
//! - The workers orchestrate: take a request, call the domain and the store, publish, then
//!   settle the request on the consumer channel.
//!
//! # One request, one task
//!
//! Each request is processed as the first message of a CICS task, the only case the source
//! determines, and no worker keeps state from one request to the next (E14, E15). Output that
//! depends on several messages of one task is HALTED(H1). The 501-message task limit of
//! `COPAUA0C` has no counterpart (E14): a worker takes requests until it is stopped, and between
//! requests only (see [`drive`]), or until its connection is lost.
//!
//! # Settlement
//!
//! A request is acknowledged only after the publication it causes is confirmed (CODATE01-012,
//! COACCT01-026, COPAUA0C-081, COPAUA0C-082). Every [`Step::process_next`] ends in one
//! [`Processed`] outcome, and every request it takes leaves one settlement event in the JSON log,
//! besides any warning or error on its path:
//!
//! | Outcome | Publication | Settlement of the request | Event |
//! |---|---|---|---|
//! | [`Processed::Idle`] | none: the queue was empty | none | none |
//! | [`Processed::Replied`] | the reply, confirmed | acknowledged | `INFO`, `outcome` `replied` |
//! | [`Processed::ErrorRouted`] | the `CARD.DEMO.ERROR` body, confirmed | acknowledged | `INFO`, `outcome` `error-routed` |
//! | [`Processed::DeadLettered`] | none confirmed | rejected, moved to the dead-letter queue | `INFO`, `outcome` `dead-lettered` |
//! | [`Processed::Discarded`] | none confirmed | rejected, dropped by the broker | `INFO`, `outcome` `discarded` |
//! | [`Processed::Requeued`] | any | none: the consumer channel closed, the broker requeues it | `WARN`, `outcome` `requeued`, from [`drive`] |
//!
//! Each worker runs its processing of a request through `in_message_span`, which opens the
//! request's `mq.message` span and emits the `INFO` event of a settled request inside it, with
//! the queue, the outcome, the `redelivered` flag of a request the broker delivered before, and
//! the body length. A request the broker requeues is taken again later and settles with
//! `redelivered` true. [`drive`] emits the `WARN` of a requeued request after the span closes.
//!
//! # What the workers never do
//!
//! - They never change the schema. Started against an unmigrated database they fail at their
//!   first query; `carddemo-migrate` alone applies the migrations.
//! - Authorization decisions are serialised only by the `PostgreSQL` row lock on the account's
//!   summary row (COPAUA0C-026, i10), so two `carddemo-mq` processes and redelivered requests
//!   stay within the credit limit. There is no in-process lock of any kind.
//!
//! # Redaction
//!
//! A CP00 request body holds a full card number. Every request span is the `mq.message` span
//! of `runtime::telemetry::message_span`, which records the queue and the `message_id` and
//! `correlation_id` as `runtime::telemetry::ScreenedText` shows them, one `*` per byte: the
//! requester chooses both and either can be a card number, an SSN or a password, so the span
//! records only whether each is present and its length, never its text. Events here name the
//! queue, the outcome and the [`AmqpError`] or [`PublishFailure`] text, never a body byte or a
//! record. The settlement event adds only the `redelivered` flag and the body length in bytes,
//! and repeats neither ID: every line written inside the span already shows both, screened.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use lapin::{BasicProperties, Connection};
use runtime::telemetry;
use tracing::Instrument;

use crate::Stop;
use crate::broker::{
    self, Acknowledger, AmqpError, Backoff, Consumer, PublishFailure, Publisher, QueueCheck,
    Request,
};
use crate::error_body::{self, Flow, Site};

/// CDRD, the port of `CODATE01`: the system date and time reply on `CARD.DEMO.REPLY.DATE`.
pub mod cdrd;

/// CDRA, the port of `COACCT01`: the account inquiry reply on `CARD.DEMO.REPLY.ACCT`.
pub mod cdra;

/// CP00, the port of `COPAUA0C`: the card authorization decision, replied to the request's
/// `reply_to` and then recorded.
pub mod cp00;

/// What one [`Step::process_next`] did with the next request of its queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Processed {
    /// The request queue was empty; [`drive`] waits the poll interval before the next get.
    Idle,
    /// A reply was published and confirmed, then the request was acknowledged: a CDRD or CDRA
    /// reply, or a CP00 approval or decline (CODATE01-012, COACCT01-026, COPAUA0C-081,
    /// COPAUA0C-082).
    Replied,
    /// A CDRD or CDRA error body was published to `CARD.DEMO.ERROR` and confirmed, then the
    /// request was acknowledged (CODATE01-014, COACCT01-028, COACCT01-033).
    ErrorRouted,
    /// CP00 rejected the request with `requeue: false`, and the dead-letter arguments of its
    /// request queue moved it to `CARD.DEMO.DEADLETTER.PAUTH`, where it is retained: an
    /// unparseable card number, an unconfirmed reply (`M004`) or a failed store read
    /// (COPAUA0C-006, COPAUA0C-117).
    DeadLettered,
    /// Neither the CDRD or CDRA reply nor its error message was confirmed, so the request was
    /// rejected with `requeue: false`. Those request queues carry no dead-letter arguments, so
    /// the broker drops it: the COBOL task end, which commits the get with nothing sent
    /// (CODATE01-019, COACCT01-034).
    Discarded,
    /// The worker could neither acknowledge nor reject the request, so it closed its consumer
    /// channel and the broker returns the request to its queue, not dead-lettered
    /// (COPAUA0C-118). [`drive`] reopens the channels before the next get.
    Requeued,
}

/// Why a worker cannot take its next request. Neither variant carries body text: the
/// [`AmqpError`] names only an operation, a queue and a broker or client condition.
#[derive(Debug, Clone)]
pub enum StepError {
    /// A worker channel is unusable. Reopening the channels on the same connection can cure it,
    /// and [`drive`] does so with backoff.
    Channel(AmqpError),
    /// The AMQP connection is gone. [`drive`] returns it, and the caller connects again.
    ConnectionLost(AmqpError),
}

impl From<AmqpError> for StepError {
    /// [`StepError::ConnectionLost`] when [`AmqpError::is_connection_lost`] holds,
    /// [`StepError::Channel`] otherwise.
    fn from(error: AmqpError) -> Self {
        if error.is_connection_lost() {
            Self::ConnectionLost(error)
        } else {
            Self::Channel(error)
        }
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Channel(error) => write!(f, "a worker channel is unusable: {error}"),
            Self::ConnectionLost(error) => write!(f, "the AMQP connection is lost: {error}"),
        }
    }
}

impl Error for StepError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Channel(error) | Self::ConnectionLost(error) => Some(error),
        }
    }
}

/// One worker, as [`drive`] runs it.
///
/// A worker never stores the connection, because `lapin::Connection` cannot be cloned: every
/// method that may open a channel receives it.
///
/// The trait spells out `impl Future + Send` so that [`drive`] can run on any executor thread;
/// an implementation writes the methods as `async fn` (`clippy::manual_async_fn` rejects the
/// spelled-out form there), and their futures must be `Send`.
pub trait Step: Send {
    /// The request queue the worker consumes, the configured `CARDDEMO_QUEUE_REQUEST_*` name.
    fn queue(&self) -> &str;

    /// Takes the next request from [`Step::queue`], processes it inside its `mq.message` span and
    /// settles it, returning what was done. A settled request leaves one `INFO` event inside the
    /// span, with the queue, the outcome, the redelivery flag and the body length, never a body
    /// byte or an ID; every implementation gets the span and the event from the crate's one
    /// helper, `in_message_span`.
    ///
    /// # Errors
    ///
    /// A [`StepError`] when no request could be taken: the get failed on the consumer channel,
    /// or the publisher channel failed to reopen after an earlier publication failure.
    fn process_next(
        &mut self,
        connection: &Connection,
    ) -> impl Future<Output = Result<Processed, StepError>> + Send;

    /// Reopens the consumer and publisher channels on the same `connection`. Closing the old
    /// consumer channel returns every request it had not settled to its queue.
    ///
    /// # Errors
    ///
    /// The [`AmqpError`] of the channel that could not be opened; the worker keeps its closed
    /// channels until a later reopen succeeds.
    fn reopen(
        &mut self,
        connection: &Connection,
    ) -> impl Future<Output = Result<(), AmqpError>> + Send;
}

/// Runs `worker` until `stop` is requested or its connection is lost.
///
/// - `stop` is checked before every [`Step::process_next`] and ends every idle poll and backoff
///   wait early; `drive` then returns `Ok`. It never interrupts [`Step::process_next`]: a request
///   the worker has taken is always finished through its own path, so a CP00 authorization is
///   never cut between its acknowledgement and its commit.
/// - [`Processed::Idle`]: waits `poll_interval` (`CARDDEMO_MQ_POLL_MS`), then polls again.
/// - Any other processed request resets `backoff`. A settled one has already left its `INFO`
///   event inside its span (`in_message_span`), so `drive` adds none.
/// - [`Processed::Requeued`] or [`StepError::Channel`]: emits a `WARN`, waits the next `backoff`
///   delay and reopens the worker's channels, repeating while the connection stays up, then
///   polls again.
///
/// It never returns while the connection is healthy and no stop is requested. Events name the
/// queue and the outcome, never a body.
///
/// # Errors
///
/// [`StepError::ConnectionLost`] for a lost connection, a channel error while the connection is
/// down, or a reopen failure that shows the connection is gone, so that the caller reconnects. A
/// missed broker deadline counts as a lost connection ([`AmqpError::is_connection_lost`]).
pub async fn drive<W: Step>(
    worker: &mut W,
    connection: &Connection,
    poll_interval: Duration,
    backoff: &mut Backoff,
    stop: &mut Stop,
) -> Result<(), StepError> {
    loop {
        if stop.is_requested() {
            return Ok(());
        }
        match worker.process_next(connection).await {
            Ok(Processed::Idle) => {
                if stop
                    .unless_requested(broker::idle(poll_interval))
                    .await
                    .is_none()
                {
                    return Ok(());
                }
                continue;
            }
            Ok(
                Processed::Replied
                | Processed::ErrorRouted
                | Processed::DeadLettered
                | Processed::Discarded,
            ) => {
                backoff.reset();
                continue;
            }
            Ok(Processed::Requeued) => tracing::warn!(
                queue = worker.queue(),
                outcome = "requeued",
                "the worker closed its consumer channel; the broker requeues the request"
            ),
            // A lost connection fails every channel first, and no reopen on it can succeed.
            Err(StepError::Channel(error))
                if error.is_connection_lost() || !connection.status().connected() =>
            {
                return Err(stopped(worker.queue(), StepError::ConnectionLost(error)));
            }
            Err(StepError::Channel(error)) => tracing::warn!(
                queue = worker.queue(),
                outcome = "channel unusable",
                error = %error,
                "a worker channel failed; reopening the channels"
            ),
            Err(lost @ StepError::ConnectionLost(_)) => {
                return Err(stopped(worker.queue(), lost));
            }
        }
        if let Err(lost) = recover(worker, connection, backoff, stop).await {
            return Err(stopped(worker.queue(), lost));
        }
    }
}

/// Emits the `ERROR` event of a worker on `queue` that stops because its connection is lost,
/// and returns `lost` for [`drive`] to return.
fn stopped(queue: &str, lost: StepError) -> StepError {
    tracing::error!(
        queue,
        outcome = "connection lost",
        error = %lost,
        "the worker stops until the connection is restored"
    );
    lost
}

/// Reopens `worker`'s channels after the next `backoff` delay, until a reopen succeeds, the
/// connection is gone or `stop` is requested during a delay. After a stop it returns `Ok` with
/// the channels as they are, and [`drive`] returns before it takes another request.
///
/// # Errors
///
/// [`StepError::ConnectionLost`] with the reopen error that shows the connection is gone.
async fn recover<W: Step>(
    worker: &mut W,
    connection: &Connection,
    backoff: &mut Backoff,
    stop: &mut Stop,
) -> Result<(), StepError> {
    loop {
        if stop.unless_requested(backoff.wait()).await.is_none() {
            return Ok(());
        }
        match worker.reopen(connection).await {
            Ok(()) => {
                backoff.reset();
                return Ok(());
            }
            Err(error) if error.is_connection_lost() || !connection.status().connected() => {
                return Err(StepError::ConnectionLost(error));
            }
            Err(error) => tracing::warn!(
                queue = worker.queue(),
                outcome = "reopen failed",
                error = %error,
                "reopening the worker channels failed; retrying"
            ),
        }
    }
}

/// The longest queue name AMQP 0-9-1 can carry: `queue.declare` and `basic.get` send it as a
/// short string, whose one-byte length prefix counts at most 255 bytes. A longer name goes out
/// with a wrapped length, and the broker closes the whole connection with `FRAME_ERROR` over a
/// frame it cannot decode.
pub(crate) const AMQP_QUEUE_NAME_LEN: usize = 255;

/// Refuses the request queue `name` configured in `variable` unless it is at most `max_len`
/// bytes of printable ASCII (`' '` to `'~'`). Every worker calls it before it opens a channel:
/// the broker answers a name longer than the frame carries with `FRAME_ERROR`, and a non-ASCII
/// one with `INTERNAL_ERROR`, by closing the whole connection, which
/// [`AmqpError::is_connection_lost`] counts as lost, so the session would reconnect forever.
/// Control bytes are refused too, as no MQ queue name holds them. The refusal is a
/// configuration error no reconnect cures, and `run` returns it as `StartupError::Declare`.
///
/// # Errors
///
/// [`AmqpError::QueueName`] with `variable` and `max_len`, never the rejected name.
pub(crate) fn check_request_queue(
    variable: &'static str,
    name: &str,
    max_len: usize,
) -> Result<(), AmqpError> {
    let printable = name.bytes().all(|byte| matches!(byte, b' '..=b'~'));
    if printable && name.len() <= max_len {
        Ok(())
    } else {
        Err(AmqpError::QueueName { variable, max_len })
    }
}

/// Runs `handling`, a worker's processing of `request` taken from `queue`, inside the request's
/// `mq.message` span, and records a settled request there with one `INFO` event.
///
/// - The span is `runtime::telemetry::message_span` with the request's `message_id` and
///   `correlation_id` as [`Request::screened_message_id`] and
///   [`Request::screened_correlation_id`] give them, one `*` per byte, so no field holds the text
///   of either ID.
/// - When `handling` returns [`Processed::Replied`], [`Processed::ErrorRouted`],
///   [`Processed::DeadLettered`] or [`Processed::Discarded`], one `INFO` event inside the span
///   records `queue`, `outcome` (`replied`, `error-routed`, `dead-lettered` or `discarded`),
///   `redelivered`, which shows a request the broker delivered before, and `body_len`, the
///   body's length in bytes. The event repeats neither ID, which the span around it shows
///   screened.
/// - [`Processed::Requeued`] adds no event here: [`drive`] warns of it after the span, as it
///   reopens the channels. [`Processed::Idle`] never reaches it, as no request was taken.
///
/// No field of the span or the event holds a body byte or the `reply_to`. The future is `Send`
/// whenever `handling` is, as [`Step::process_next`] requires.
pub(crate) async fn in_message_span<F>(queue: &str, request: &Request, handling: F) -> Processed
where
    F: Future<Output = Processed>,
{
    let span = telemetry::message_span(
        queue,
        request.screened_message_id(),
        request.screened_correlation_id(),
    );
    async {
        let processed = handling.await;
        if let Some(outcome) = settled_outcome(processed) {
            tracing::info!(
                queue,
                outcome,
                redelivered = request.redelivered,
                body_len = request.body.len(),
                "the request is settled"
            );
        }
        processed
    }
    .instrument(span)
    .await
}

/// The `outcome` label of the event [`in_message_span`] emits for a request `processed`
/// settled, or `None` for [`Processed::Idle`], which took no request, and for
/// [`Processed::Requeued`], which left the request to the broker.
fn settled_outcome(processed: Processed) -> Option<&'static str> {
    match processed {
        Processed::Replied => Some("replied"),
        Processed::ErrorRouted => Some("error-routed"),
        Processed::DeadLettered => Some("dead-lettered"),
        Processed::Discarded => Some("discarded"),
        Processed::Idle | Processed::Requeued => None,
    }
}

/// The channel pair of one worker: the [`Consumer`] that takes requests and holds their
/// delivery tags, and the [`Publisher`] in confirm mode.
///
/// Requests are settled on the live consumer channel, because delivery tags are scoped to the
/// channel that delivered them and that channel changes with every [`Channels::reopen`]. An
/// acknowledger set through [`Channels::with_acknowledger`] replaces that settlement for the
/// lifetime of the channels; it exists for the failure-injection tests.
pub(crate) struct Channels<A: Acknowledger> {
    consumer: Consumer,
    publisher: Publisher,
    acknowledger: Option<A>,
    /// The error of the last failed publisher reopen. [`Channels::next`] reports it instead of
    /// taking a request, so that [`drive`] reopens both channels before any request meets a
    /// publisher that cannot publish.
    publisher_fault: Option<AmqpError>,
}

impl Channels<Consumer> {
    /// Opens the consumer channel, checks that each of `queues` exists with a passive declare
    /// on it, then opens the publisher channel in confirm mode. Requests are settled on the
    /// consumer channel itself.
    ///
    /// # Errors
    ///
    /// The [`AmqpError`] of the channel that could not be opened, or [`AmqpError::Declare`]
    /// naming the first missing queue by its value-free label. A consumer channel opened before
    /// the failure is closed.
    pub(crate) async fn open(
        connection: &Connection,
        queues: &[QueueCheck<'_>],
    ) -> Result<Self, AmqpError> {
        let consumer = Consumer::open(connection).await?;
        let publisher = match broker::declare_passive(consumer.channel(), queues).await {
            Ok(()) => Publisher::open(connection).await,
            Err(error) => Err(error),
        };
        match publisher {
            Ok(publisher) => Ok(Self {
                consumer,
                publisher,
                acknowledger: None,
                publisher_fault: None,
            }),
            Err(error) => {
                consumer.close().await;
                Err(error)
            }
        }
    }
}

impl<A: Acknowledger> Channels<A> {
    /// These channels with `acknowledger` settling every request in place of the consumer
    /// channel.
    #[must_use]
    pub(crate) fn with_acknowledger<B: Acknowledger>(self, acknowledger: B) -> Channels<B> {
        Channels {
            consumer: self.consumer,
            publisher: self.publisher,
            acknowledger: Some(acknowledger),
            publisher_fault: self.publisher_fault,
        }
    }

    /// Takes the next request from `queue` with `basic_get`, or `None` when the queue is empty
    /// (CODATE01-001, COACCT01-001, COPAUA0C-001).
    ///
    /// # Errors
    ///
    /// The [`StepError`] of a failed get, or of the last failed publisher reopen, reported once
    /// and before any request is taken.
    pub(crate) async fn next(&mut self, queue: &str) -> Result<Option<Request>, StepError> {
        if let Some(fault) = self.publisher_fault.take() {
            return Err(StepError::from(fault));
        }
        self.consumer.get(queue).await.map_err(StepError::from)
    }

    /// Acknowledges `request`, which removes it from its queue for good. Call it only after the
    /// publication the request causes is confirmed.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Ack`] when the acknowledgement fails; the delivery tag is then unusable.
    pub(crate) async fn ack(&self, request: &Request) -> Result<(), AmqpError> {
        match &self.acknowledger {
            Some(acknowledger) => acknowledger.ack(request.delivery_tag).await,
            None => self.consumer.ack(request.delivery_tag).await,
        }
    }

    /// Rejects `request` with `requeue: false`: dead-lettered from `CARD.DEMO.REQUEST.PAUTH`,
    /// dropped from the CDRD and CDRA request queues, which carry no dead-letter arguments.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Reject`] when the rejection fails; the delivery tag is then unusable.
    pub(crate) async fn reject_without_requeue(&self, request: &Request) -> Result<(), AmqpError> {
        match &self.acknowledger {
            Some(acknowledger) => {
                acknowledger
                    .reject_without_requeue(request.delivery_tag)
                    .await
            }
            None => {
                self.consumer
                    .reject_without_requeue(request.delivery_tag)
                    .await
            }
        }
    }

    /// Closes the consumer channel, a failure only traced, for a request that can be neither
    /// acknowledged nor rejected. The broker returns every request the channel had not settled
    /// to its queue, and a requeued delivery is not dead-lettered (COPAUA0C-118).
    pub(crate) async fn abandon(&self) -> Processed {
        self.consumer.close().await;
        Processed::Requeued
    }

    /// Replaces both channels with new ones on `connection`: the consumer first, whose old
    /// channel is closed so that its unsettled requests return to their queues, then the
    /// publisher.
    ///
    /// # Errors
    ///
    /// The [`AmqpError`] of the channel that could not be opened.
    pub(crate) async fn reopen(&mut self, connection: &Connection) -> Result<(), AmqpError> {
        self.consumer.close().await;
        self.consumer = Consumer::open(connection).await?;
        self.reopen_publisher(connection).await
    }

    /// Publishes `body` to `queue` and waits for its confirmation, through
    /// [`Publisher::publish`]: the default exchange, `mandatory: true`, success only on
    /// `Confirmation::Ack` with no returned message.
    ///
    /// # Errors
    ///
    /// The [`PublishFailure`] that kept the message from being confirmed. The publisher channel
    /// may be closed then, so the caller calls [`Channels::reopen_publisher`].
    pub(crate) async fn publish(
        &self,
        queue: &str,
        body: &[u8],
        properties: BasicProperties,
    ) -> Result<(), PublishFailure> {
        self.publisher.publish(queue, body, properties).await
    }

    /// Replaces the publisher channel with a new one in confirm mode, after a
    /// [`PublishFailure`]. A failure is kept and reported by the next [`Channels::next`].
    ///
    /// # Errors
    ///
    /// [`AmqpError::Channel`] when the new channel cannot be opened or put in confirm mode.
    pub(crate) async fn reopen_publisher(
        &mut self,
        connection: &Connection,
    ) -> Result<(), AmqpError> {
        match self.publisher.reopen(connection).await {
            Ok(()) => {
                self.publisher_fault = None;
                Ok(())
            }
            Err(error) => {
                self.publisher_fault = Some(error.clone());
                Err(error)
            }
        }
    }
}

/// The CDRD and CDRA put path, `4100-PUT-REPLY` with its `9000-ERROR` (`CODATE01.cbl:366-440`,
/// `COACCT01.cbl:462-537`), for the reply `body` of `flow` to `request`.
///
/// 1. `body` is published to the flow's fixed reply queue, never the request's `reply_to`,
///    with [`broker::copied_properties`]: the request's IDs, persistent, no expiration
///    (CODATE01-002, CODATE01-010, CODATE01-011, COACCT01-023, COACCT01-024, COACCT01-025).
/// 2. Confirmed: the request is acknowledged, [`Processed::Replied`] (CODATE01-012,
///    COACCT01-026).
/// 3. Not confirmed: the publisher channel is reopened, then the `MQPUT ERR` body of
///    `error_body::render` naming the reply queue goes to `CARD.DEMO.ERROR` through
///    [`route_error`] (CODATE01-014, COACCT01-033).
///
/// A request is never acknowledged without a confirmed publication. When it can be neither
/// acknowledged nor rejected, or no publisher channel can be reopened for the error message,
/// the consumer channel is closed and the broker requeues it, [`Processed::Requeued`].
pub(crate) async fn reply_fixed<A: Acknowledger>(
    channels: &mut Channels<A>,
    connection: &Connection,
    request: &Request,
    flow: Flow,
    body: &[u8],
) -> Processed {
    let reply_queue = flow.reply_queue();
    match channels
        .publish(reply_queue, body, broker::copied_properties(request))
        .await
    {
        Ok(()) => acknowledged(channels, request, Processed::Replied).await,
        Err(failure) => {
            tracing::warn!(
                reply_queue,
                failure = %failure,
                "the reply was not confirmed; the MQPUT ERR message goes to the error queue"
            );
            if let Err(error) = channels.reopen_publisher(connection).await {
                tracing::warn!(
                    error = %error,
                    "no publisher channel for the error message; the broker requeues the request"
                );
                return channels.abandon().await;
            }
            route_error(
                channels,
                connection,
                request,
                flow,
                &error_body::render(flow),
            )
            .await
        }
    }
}

/// Publishes the ready-made error `body` of `flow` to `CARD.DEMO.ERROR` for `request`, with the
/// request's IDs through [`broker::copied_properties`] (CODATE01-018, COACCT01-032): the
/// `MQPUT ERR` body of [`reply_fixed`], or the CDRA account-read error (COACCT01-028).
///
/// - Confirmed: the request is acknowledged, [`Processed::ErrorRouted`] (CODATE01-014,
///   COACCT01-028, COACCT01-033).
/// - Not confirmed: one `ERROR` event stands in for the COBOL `DISPLAY MQ-ERR-DISPLAY`, with the
///   return message `MQPUT ERR` and the queue name `CARD.DEMO.ERROR` of
///   `error_body::mq_err_display(flow, Site::ErrorPut)`, its codes HALTED(H4) and not
///   recorded. The request is rejected without requeue, so it is removed and not redelivered,
///   as the COBOL task end commits the get, [`Processed::Discarded`] (CODATE01-019,
///   COACCT01-034, X-CODATE01-5). The publisher channel is then reopened.
///
/// A request that can be neither acknowledged nor rejected is left to the broker,
/// [`Processed::Requeued`].
pub(crate) async fn route_error<A: Acknowledger>(
    channels: &mut Channels<A>,
    connection: &Connection,
    request: &Request,
    flow: Flow,
    body: &[u8],
) -> Processed {
    let error_queue = flow.error_queue();
    let failure = match channels
        .publish(error_queue, body, broker::copied_properties(request))
        .await
    {
        Ok(()) => return acknowledged(channels, request, Processed::ErrorRouted).await,
        Err(failure) => failure,
    };
    let display = error_body::mq_err_display(flow, Site::ErrorPut);
    let return_message = display.return_message.to_text();
    let queue_name = display.queue_name.to_text();
    tracing::error!(
        return_message,
        queue_name,
        failure = %failure,
        "MQ-ERR-DISPLAY: the error message was not confirmed either; the request is discarded"
    );
    if let Err(error) = channels.reject_without_requeue(request).await {
        tracing::warn!(
            error = %error,
            "rejecting the request failed; the broker requeues it"
        );
        return channels.abandon().await;
    }
    if let Err(error) = channels.reopen_publisher(connection).await {
        tracing::warn!(
            error = %error,
            "reopening the publisher channel failed; the next get reopens both channels"
        );
    }
    Processed::Discarded
}

/// Acknowledges `request` after its confirmed publication and returns `outcome`; when the
/// acknowledgement fails, closes the consumer channel so that the broker requeues the request.
async fn acknowledged<A: Acknowledger>(
    channels: &Channels<A>,
    request: &Request,
    outcome: Processed,
) -> Processed {
    match channels.ack(request).await {
        Ok(()) => outcome,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "the acknowledgement after a confirmed publication failed; the broker requeues it"
            );
            channels.abandon().await
        }
    }
}

#[cfg(test)]
mod tests {
    use test_support::trace_capture;

    use super::{AMQP_QUEUE_NAME_LEN, Processed, cdrd, check_request_queue, cp00, in_message_span};
    use crate::broker::{AmqpError, Request};

    /// The request queue of the settlement cases, the default of `CARDDEMO_QUEUE_REQUEST_PAUTH`.
    const QUEUE: &str = "CARD.DEMO.REQUEST.PAUTH";

    /// The members of the `fields` object of the JSON line `line`, the event's own fields without
    /// its span's, or `None` when the line has none. No field value here holds a `}`.
    fn event_fields(line: &str) -> Option<&str> {
        let (_, rest) = line.split_once(r#""fields":{"#)?;
        rest.split_once('}').map(|(fields, _)| fields)
    }

    #[tokio::test]
    async fn a_settled_request_leaves_one_info_event_in_its_span_with_no_body_or_id_text() {
        // A test card number, an SSN and a password, assembled at run time.
        let card = format!("4111{}", "1".repeat(12));
        let ssn = ["123-45", "-6789"].concat();
        let password = ["PASS", "WORD"].concat();
        let body = format!("220718,102030,{card},{ssn},{password},").into_bytes();
        let body_len = body.len();
        let correlation_id = format!("{ssn} {password}");
        let cases = [
            (Processed::Replied, Some("replied"), false),
            (Processed::ErrorRouted, Some("error-routed"), true),
            (Processed::DeadLettered, Some("dead-lettered"), false),
            (Processed::Discarded, Some("discarded"), true),
            (Processed::Requeued, None, true),
        ];
        for (processed, outcome, redelivered) in cases {
            let request = Request {
                delivery_tag: 5,
                body: body.clone(),
                message_id: Some(card.clone()),
                correlation_id: Some(correlation_id.clone()),
                reply_to: Some(password.clone()),
                redelivered,
            };

            let capture = trace_capture::capture();
            let returned = in_message_span(QUEUE, &request, async { processed }).await;
            assert_eq!(returned, processed);

            let lines = capture.lines();
            let events: Vec<&String> = lines
                .iter()
                .filter(|line| line.contains(r#""message":"the request is settled""#))
                .collect();
            if let Some(outcome) = outcome {
                let [event] = events.as_slice() else {
                    panic!("{outcome}: expected one settlement event, got {lines:?}");
                };
                // The span's opening line, the event and the span's closing line.
                assert_eq!(lines.len(), 3, "{outcome}: {lines:?}");
                for member in [
                    r#""level":"INFO""#.to_owned(),
                    r#""name":"mq.message""#.to_owned(),
                    format!(r#""message_id":"{}""#, "*".repeat(card.len())),
                    format!(r#""correlation_id":"{}""#, "*".repeat(correlation_id.len())),
                ] {
                    assert!(event.contains(&member), "{member} missing from {event}");
                }
                let fields = event_fields(event).unwrap_or_default();
                for member in [
                    format!(r#""queue":"{QUEUE}""#),
                    format!(r#""outcome":"{outcome}""#),
                    format!(r#""redelivered":{redelivered}"#),
                    format!(r#""body_len":{body_len}"#),
                ] {
                    assert!(fields.contains(&member), "{member} missing from {event}");
                }
                for id in ["message_id", "correlation_id", "reply_to"] {
                    assert!(!fields.contains(id), "{id} among the fields of {event}");
                }
            } else {
                assert!(events.is_empty(), "{processed:?}: {lines:?}");
                assert!(
                    !capture.contains(r#""outcome""#),
                    "{processed:?}: {lines:?}"
                );
                // The span's opening and closing lines only.
                assert_eq!(lines.len(), 2, "{processed:?}: {lines:?}");
            }

            // The protected values, and the body's fields in the comma-delimited context they
            // have in the body, so that a timestamp's digits cannot match them.
            let text = capture.text();
            for value in [
                card.as_str(),
                ssn.as_str(),
                password.as_str(),
                "220718,",
                ",102030,",
            ] {
                assert!(!text.contains(value), "{value:?} in {text}");
            }
        }
    }

    #[test]
    fn a_cdrd_or_cp00_request_queue_the_amqp_frame_cannot_carry_is_a_startup_refusal() {
        let longest = "Q".repeat(AMQP_QUEUE_NAME_LEN);
        let too_long = "Q".repeat(AMQP_QUEUE_NAME_LEN + 1);
        for variable in [cdrd::REQUEST_QUEUE_VARIABLE, cp00::REQUEST_QUEUE_VARIABLE] {
            for name in ["CARD.DEMO.REQUEST.PAUTH", "QA REQUEST ~", longest.as_str()] {
                assert!(
                    check_request_queue(variable, name, AMQP_QUEUE_NAME_LEN).is_ok(),
                    "{variable}: {name}"
                );
            }
            for name in [
                too_long.as_str(),
                "QA.PAUTH.é✓",
                "QA.PAUTH.\u{1}",
                "QA.PAUTH\t",
                "QA.PAUTH.\u{7f}",
            ] {
                let error = check_request_queue(variable, name, AMQP_QUEUE_NAME_LEN).err();
                assert!(
                    matches!(
                        error,
                        Some(AmqpError::QueueName { variable: refused, max_len: 255 })
                            if refused == variable
                    ),
                    "{variable}: {name:?}: {error:?}"
                );
                assert!(
                    error
                        .as_ref()
                        .is_some_and(|error| !error.is_connection_lost()),
                    "{variable}: {name:?}"
                );
                let text = error
                    .map(|error| format!("{error} {error:?}"))
                    .unwrap_or_default();
                assert!(text.contains(variable), "{text}");
                assert!(!text.contains(name), "{text}");
            }
        }
    }
}
