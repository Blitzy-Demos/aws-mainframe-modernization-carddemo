//! The broker rules the three `RabbitMQ` workers share (AAP 0.3.4.2), implemented once: the
//! connection, the two channels each worker owns, the passive queue checks, `basic_get` polling,
//! the confirmed mandatory publish, the reply properties, the acknowledgement seam and the
//! reconnect backoff. The workers (`workers::{cdrd, cdra, cp00}`) reach the broker only through
//! this module, and no COBOL body layout lives here.
//!
//! | Item | MQ counterpart | Rules |
//! |---|---|---|
//! | [`connect`] | the queue-manager connection of the CICS region | — |
//! | [`declare_passive`] | `MQOPEN` of each queue | the queues of `rabbitmq/definitions.json` |
//! | [`Consumer::get`] | `MQGET` with the descriptor IDs reset before every get | CODATE01-001, COACCT01-001, COPAUA0C-001 |
//! | [`Publisher::publish`] | `MQPUT` / `MQPUT1` | CODATE01-002, CODATE01-012, CODATE01-014, COACCT01-023, COACCT01-026, COACCT01-033, COPAUA0C-066, COPAUA0C-117 |
//! | [`copied_properties`] | `MOVE SAVE-MSGID`, `MOVE SAVE-CORELID` (`CODATE01.cbl:373-374`) | CODATE01-010, CODATE01-011, CODATE01-018, COACCT01-024, COACCT01-025, COACCT01-032 |
//! | [`cp00_properties`] | the reply descriptor of `7100-SEND-RESPONSE` (`COPAUA0C.cbl:745-750`) | COPAUA0C-067 |
//! | [`Acknowledger`] | the syncpoint that commits a get with its put; the task end that commits a get with nothing sent | CODATE01-012, CODATE01-019, COACCT01-026, COACCT01-034, COPAUA0C-006, COPAUA0C-081, COPAUA0C-082, COPAUA0C-117 |
//! | [`Consumer::close`], [`Backoff`] | the task end and the next trigger | COPAUA0C-118 |
//! | [`idle`] | `MQGMO-WAIT` with its wait interval | the `basic_get` poll of `CARDDEMO_MQ_POLL_MS` |
//!
//! # Rules shared by the workers
//!
//! - **Two channels per worker.** A [`Consumer`] channel holds the delivery tags for
//!   `basic_get`, acknowledgement and rejection; a separate [`Publisher`] channel publishes in
//!   confirm mode. A failed publication may close the publisher channel, and never the channel
//!   that holds the tag of the request it answers.
//! - **Durable queues, persistent messages.** The seven queues are declared durable in
//!   `rabbitmq/definitions.json`, the dead-letter arguments of `CARD.DEMO.REQUEST.PAUTH`
//!   included. Workers only check them with [`declare_passive`] and never declare a queue. Every
//!   publish is persistent (`delivery_mode` 2), the request's override of the MQ persistence.
//! - **Confirmed publish, then acknowledgement.** A request is acknowledged only after the
//!   publication it causes is confirmed: [`Publisher::publish`] succeeds only on
//!   `Confirmation::Ack` with no returned message.
//! - **Default exchange.** MQ point-to-point maps to the default exchange, with the queue name
//!   as routing key and `mandatory: true`, so a message no queue accepts comes back as
//!   [`PublishFailure::Returned`] instead of being dropped.
//! - **Queue names.** Request queues come from `runtime::config::Config`. Reply and error queues
//!   are the COBOL literals, owned by `domain::messaging::codate01` and
//!   `domain::messaging::coacct01`; CP00 replies go to the request's `reply_to`. The README
//!   queue names (E01) appear in no source and are not used.
//! - **No task boundary.** Each request is processed as the first message of a CICS task, and
//!   nothing here carries state from one request to the next (E14, E15).
//!
//! # Redaction
//!
//! CP00 request bodies hold full card numbers. No `Debug`, `Display`, error text or tracing
//! event of this module shows a body byte: [`Request`]'s `Debug` gives the body length and the
//! three properties as their [`ScreenedText`] shows them, one `*` per byte, and
//! [`AmqpError`] and [`PublishFailure`] name queues and broker conditions only. The AMQP URL
//! carries the local password, so it is never logged and no error repeats it.
//!
//! A request's `message_id`, `correlation_id` and `reply_to` are the requester's choice, and any
//! of them can be a card number, an SSN or a password; a `CardDemo` password is any one to eight
//! printable bytes, spaces included, so nothing in a property tells it safe. [`Request`] keeps
//! them as received, for the reply properties and the reply destination, and every diagnostic
//! takes them through [`Request::screened_message_id`], [`Request::screened_correlation_id`] and
//! [`Request::screened_reply_to`], which show only whether each is present and its length in
//! bytes, never its text.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use lapin::message::{BasicGetMessage, Delivery};
use lapin::options::{
    BasicAckOptions, BasicGetOptions, BasicPublishOptions, BasicRejectOptions,
    ConfirmSelectOptions, QueueDeclareOptions,
};
use lapin::protocol::AMQPErrorKind;
use lapin::publisher_confirm::Confirmation;
use lapin::types::{FieldTable, ShortString};
use lapin::uri::AMQPUri;
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties, ConnectionState};
use runtime::telemetry::ScreenedText;

/// The queue to which the broker moves a `CARD.DEMO.REQUEST.PAUTH` request that a worker
/// rejects with `requeue: false`, through the request queue's `x-dead-letter-exchange` `""` and
/// `x-dead-letter-routing-key` arguments in `rabbitmq/definitions.json`. CP00 rejects a request
/// whose card number cannot be parsed (i11; COPAUA0C-006) and one whose reply could not be
/// published (`M004`; COPAUA0C-117).
pub const DEAD_LETTER_QUEUE: &str = "CARD.DEMO.DEADLETTER.PAUTH";

/// The AMQP `expiration` of every CP00 reply, in milliseconds: the audited `MQMD-EXPIRY` of 50
/// tenths of a second (`COPAUA0C.cbl:750`; COPAUA0C-067, E16). CDRD and CDRA replies carry none.
pub const CP00_REPLY_EXPIRATION: &str = "5000";

/// The longest delay [`Backoff`] waits between two reconnection attempts.
pub const BACKOFF_CAP: Duration = Duration::from_secs(5);

/// The shortest delay [`Backoff`] starts from: a zero delay would never grow, and the reconnect
/// loop would spin.
const BACKOFF_FLOOR: Duration = Duration::from_millis(1);

/// AMQP `delivery_mode` of a persistent message.
const PERSISTENT: u8 = 2;

/// The default exchange, which routes a message to the queue its routing key names.
const DEFAULT_EXCHANGE: &str = "";

/// AMQP reply code of a normal channel close (`REPLY_SUCCESS`).
const CLOSE_NORMAL: u16 = 200;

const CONSUMER_CLOSE_TEXT: &str = "carddemo-mq consumer reset";
const PUBLISHER_CLOSE_TEXT: &str = "carddemo-mq publisher reset";

/// A broker operation that failed. The text names the operation, the queue where one applies
/// and the broker's or client's condition, never a message body or the AMQP URL.
#[derive(Debug, Clone)]
pub enum AmqpError {
    /// `CARDDEMO_AMQP_URL` is not a valid AMQP URL. The parser's own message is dropped,
    /// because it can repeat the URL, password included.
    InvalidUrl,
    /// The queue name configured in `variable` cannot be carried whole by the COBOL field that
    /// reports it: it is longer than `max_len` bytes or holds a byte outside printable ASCII.
    /// The CDRA worker refuses such a `CARDDEMO_QUEUE_REQUEST_ACCT` at start, because its
    /// account-read error body names the request queue in `MQ-APPL-QUEUE-NAME PIC X(48)`
    /// (COACCT01-029, X-COACCT01-8). The text names the variable, never the rejected value.
    QueueName {
        /// The environment variable that holds the rejected name.
        variable: &'static str,
        /// The width of the field the name must fit.
        max_len: usize,
    },
    /// The connection to the broker could not be established.
    Connect(lapin::Error),
    /// A channel could not be opened or put in confirm mode.
    Channel(lapin::Error),
    /// The passive declare of `queue` failed: the queue does not exist, or the channel or the
    /// connection failed first.
    Declare {
        /// The queue that was checked.
        queue: String,
        /// The broker's or the client's error.
        source: lapin::Error,
    },
    /// `basic_get` failed on the consumer channel.
    Get(lapin::Error),
    /// `basic_ack` failed on the consumer channel.
    Ack(lapin::Error),
    /// `basic_reject` failed on the consumer channel.
    Reject(lapin::Error),
}

impl AmqpError {
    /// Whether the AMQP connection behind the failed operation is gone, so that reopening
    /// channels on it cannot help and only a new [`connect`] can.
    ///
    /// The heuristic, over the client error the variant carries:
    ///
    /// - lost: [`AmqpError::Connect`]; an IO error; a missed heartbeat; a frame the client could
    ///   not parse or serialise, or a protocol version it rejected, each of which closes the
    ///   connection; a connection state other than connected; a hard AMQP error such as
    ///   `CONNECTION_FORCED`, with which the broker closes the connection.
    /// - not lost: a soft AMQP error such as `NOT_FOUND` or `PRECONDITION_FAILED`, which closes
    ///   only the channel, an invalid channel or channel state, the channel limit, and
    ///   [`AmqpError::InvalidUrl`] and [`AmqpError::QueueName`], configuration errors no
    ///   reconnect cures.
    ///
    /// After a connection loss the client marks its channels failed, so the next operation on a
    /// channel reports a channel state and counts as not lost. The worker then reopens its
    /// channels, `create_channel` reports the connection state, and that error counts as lost.
    #[must_use]
    pub fn is_connection_lost(&self) -> bool {
        match self {
            Self::InvalidUrl | Self::QueueName { .. } => false,
            Self::Connect(_) => true,
            Self::Channel(error)
            | Self::Declare { source: error, .. }
            | Self::Get(error)
            | Self::Ack(error)
            | Self::Reject(error) => connection_lost(error),
        }
    }
}

/// [`AmqpError::is_connection_lost`] for one client error.
fn connection_lost(error: &lapin::Error) -> bool {
    match error {
        lapin::Error::IOError(_)
        | lapin::Error::MissingHeartbeatError
        | lapin::Error::ParsingError(_)
        | lapin::Error::SerialisationError(_)
        | lapin::Error::InvalidProtocolVersion(_) => true,
        lapin::Error::InvalidConnectionState(state) => !matches!(state, ConnectionState::Connected),
        lapin::Error::ProtocolError(protocol) => {
            matches!(protocol.kind(), AMQPErrorKind::Hard(_))
        }
        // Channel-level conditions (an invalid channel or channel state, the channel limit) and
        // any variant a later client release adds.
        _ => false,
    }
}

impl fmt::Display for AmqpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => f.write_str("the AMQP URL (CARDDEMO_AMQP_URL) is not valid"),
            Self::QueueName { variable, max_len } => write!(
                f,
                "{variable} must name a queue of at most {max_len} bytes of printable ASCII"
            ),
            Self::Connect(error) => write!(f, "cannot connect to the AMQP broker: {error}"),
            Self::Channel(error) => write!(f, "cannot open an AMQP channel: {error}"),
            Self::Declare { queue, source } => {
                write!(f, "passive declare of queue {queue} failed: {source}")
            }
            Self::Get(error) => write!(f, "basic.get failed: {error}"),
            Self::Ack(error) => write!(f, "basic.ack failed: {error}"),
            Self::Reject(error) => write!(f, "basic.reject failed: {error}"),
        }
    }
}

impl Error for AmqpError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUrl | Self::QueueName { .. } => None,
            Self::Connect(error)
            | Self::Channel(error)
            | Self::Declare { source: error, .. }
            | Self::Get(error)
            | Self::Ack(error)
            | Self::Reject(error) => Some(error),
        }
    }
}

/// Why a publication was not confirmed, so the request it answers must not be acknowledged
/// (CODATE01-014, COACCT01-033, COPAUA0C-117).
#[derive(Debug, Clone)]
pub enum PublishFailure {
    /// The broker confirmed the message but first returned it under `mandatory: true`: no queue
    /// matched the routing key, for example a CP00 `reply_to` naming no queue
    /// (`Confirmation::Ack` with a returned message).
    Returned,
    /// The broker negatively acknowledged the message (`Confirmation::Nack`).
    Nacked,
    /// The publisher channel is not in confirm mode, so no confirmation exists
    /// (`Confirmation::NotRequested`).
    NotConfirmed,
    /// `basic_publish` or the wait for its confirmation failed on the publisher channel.
    Channel(lapin::Error),
}

impl fmt::Display for PublishFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Returned => f.write_str("the broker returned the message: no queue matched"),
            Self::Nacked => f.write_str("the broker did not accept the message (nack)"),
            Self::NotConfirmed => f.write_str("the publisher channel is not in confirm mode"),
            Self::Channel(error) => {
                write!(f, "publication failed on the publisher channel: {error}")
            }
        }
    }
}

impl Error for PublishFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Channel(error) => Some(error),
            Self::Returned | Self::Nacked | Self::NotConfirmed => None,
        }
    }
}

/// Opens the AMQP connection the workers share. `amqp_url` is `Config::amqp_url`
/// (`CARDDEMO_AMQP_URL`), the only source of the broker address and credentials; it is never
/// logged.
///
/// # Errors
///
/// [`AmqpError::InvalidUrl`] when `amqp_url` does not parse as an AMQP URL, and
/// [`AmqpError::Connect`] when the broker cannot be reached or refuses the login.
pub async fn connect(amqp_url: &str) -> Result<Connection, AmqpError> {
    let uri = parse_url(amqp_url)?;
    Connection::connect_uri(uri, ConnectionProperties::default())
        .await
        .map_err(AmqpError::Connect)
}

/// Parses the AMQP URL, dropping the parser's message, which can repeat the URL.
fn parse_url(amqp_url: &str) -> Result<AMQPUri, AmqpError> {
    amqp_url
        .parse::<AMQPUri>()
        .map_err(|_| AmqpError::InvalidUrl)
}

/// Checks that each of `queues` exists, with a passive declare that creates nothing.
///
/// A missing queue makes the broker close `channel` (AMQP `NOT_FOUND`, 404), so the first
/// failure ends the check. At startup it is fatal: the queue set of
/// `rabbitmq/definitions.json` is missing or a request queue name in the configuration is wrong.
///
/// # Errors
///
/// [`AmqpError::Declare`] naming the first queue whose check failed.
pub async fn declare_passive(channel: &Channel, queues: &[&str]) -> Result<(), AmqpError> {
    for queue in queues {
        channel
            .queue_declare(
                queue,
                QueueDeclareOptions {
                    passive: true,
                    durable: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|source| AmqpError::Declare {
                queue: (*queue).to_owned(),
                source,
            })?;
    }
    Ok(())
}

/// Closes `channel` with a normal reply code if it is still open. A failure is only traced:
/// the channel is being discarded either way.
async fn close_quietly(channel: &Channel, reason: &str) {
    if !channel.status().connected() {
        return;
    }
    if let Err(error) = channel.close(CLOSE_NORMAL, reason).await {
        tracing::warn!(reason, error = %error, "closing an AMQP channel failed");
    }
}

/// The consumer channel of one worker: it takes requests with `basic_get` and holds their
/// delivery tags until they are acknowledged or rejected (COPAUA0C-001).
#[derive(Clone, Debug)]
pub struct Consumer {
    channel: Channel,
}

impl Consumer {
    /// Opens a consumer channel on `connection`.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Channel`] when the channel cannot be opened, for example because the
    /// connection is gone.
    pub async fn open(connection: &Connection) -> Result<Self, AmqpError> {
        let channel = connection
            .create_channel()
            .await
            .map_err(AmqpError::Channel)?;
        Ok(Self { channel })
    }

    /// The channel itself, for [`declare_passive`].
    #[must_use]
    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// Takes the next request from `queue` in delivery order, or `None` when the queue is empty
    /// (CODATE01-001, COACCT01-001, COPAUA0C-001).
    ///
    /// The get uses `no_ack: false`, so the request stays with the broker until it is
    /// acknowledged after a confirmed publish or rejected through [`Acknowledger`]. If the
    /// channel closes first, the broker returns it to its queue.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Get`] when the get fails, for example because the channel or the connection
    /// is gone.
    pub async fn get(&self, queue: &str) -> Result<Option<Request>, AmqpError> {
        let message = self
            .channel
            .basic_get(queue, BasicGetOptions { no_ack: false })
            .await
            .map_err(AmqpError::Get)?;
        Ok(message.map(|BasicGetMessage { delivery, .. }| Request::from_delivery(delivery)))
    }

    /// Closes the channel if it is still open; a failure is only traced.
    ///
    /// Closing returns every delivery the channel has not acknowledged or rejected to its queue.
    /// That is how a worker leaves a request to be redelivered when it can neither acknowledge
    /// nor reject it, and a requeued delivery is not dead-lettered (COPAUA0C-118).
    pub async fn close(&self) {
        close_quietly(&self.channel, CONSUMER_CLOSE_TEXT).await;
    }

    /// Whether the channel is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.channel.status().connected()
    }
}

/// The publisher channel of one worker, in confirm mode from the moment it opens.
#[derive(Debug)]
pub struct Publisher {
    channel: Channel,
}

impl Publisher {
    /// Opens a publisher channel on `connection` and puts it in confirm mode, once.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Channel`] when the channel cannot be opened or confirm mode is refused; a
    /// channel opened before the refusal is closed again.
    pub async fn open(connection: &Connection) -> Result<Self, AmqpError> {
        let channel = connection
            .create_channel()
            .await
            .map_err(AmqpError::Channel)?;
        if let Err(error) = channel
            .confirm_select(ConfirmSelectOptions::default())
            .await
        {
            close_quietly(&channel, PUBLISHER_CLOSE_TEXT).await;
            return Err(AmqpError::Channel(error));
        }
        Ok(Self { channel })
    }

    /// Replaces the channel with a new one in confirm mode. Workers call this after every
    /// [`PublishFailure`], because the failure may have closed the channel. The old channel is
    /// closed first if it is still open, and an error closing it is only traced.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Channel`] as [`Publisher::open`]; the old, closed channel is kept then.
    pub async fn reopen(&mut self, connection: &Connection) -> Result<(), AmqpError> {
        close_quietly(&self.channel, PUBLISHER_CLOSE_TEXT).await;
        *self = Self::open(connection).await?;
        Ok(())
    }

    /// Publishes `body` to `queue` through the default exchange with `mandatory: true`, then
    /// waits for the broker's confirmation (CODATE01-002, CODATE01-012, COACCT01-023,
    /// COACCT01-026, COPAUA0C-066).
    ///
    /// Only `Confirmation::Ack` with no returned message is success, and only then may the
    /// worker acknowledge the request it answers.
    ///
    /// # Errors
    ///
    /// The [`PublishFailure`] that kept the message from being confirmed (CODATE01-014,
    /// COACCT01-033, COPAUA0C-117).
    pub async fn publish(
        &self,
        queue: &str,
        body: &[u8],
        properties: BasicProperties,
    ) -> Result<(), PublishFailure> {
        let confirm = self
            .channel
            .basic_publish(
                DEFAULT_EXCHANGE,
                queue,
                BasicPublishOptions {
                    mandatory: true,
                    ..BasicPublishOptions::default()
                },
                body,
                properties,
            )
            .await
            .map_err(PublishFailure::Channel)?;
        match confirm.await.map_err(PublishFailure::Channel)? {
            Confirmation::Ack(None) => Ok(()),
            Confirmation::Ack(Some(_)) => Err(PublishFailure::Returned),
            Confirmation::Nack(_) => Err(PublishFailure::Nacked),
            Confirmation::NotRequested => Err(PublishFailure::NotConfirmed),
        }
    }
}

/// One request taken from a request queue: its delivery tag, body and the properties that
/// stand for the MQ descriptor fields `MSGID`, `CORRELID` and `REPLYTOQ`.
///
/// The body is untrusted and copied as received. `Debug` shows its length, never its bytes,
/// because a CP00 body holds a full card number; no `Display` or `Serialize` exists.
///
/// The three properties are untrusted too. Their fields keep them as received, because the reply
/// properties copy them and a CP00 reply goes to `reply_to`. Diagnostics, `Debug` included, show
/// them only through [`Request::screened_message_id`], [`Request::screened_correlation_id`] and
/// [`Request::screened_reply_to`], one `*` per byte, since a requester can put a card number, an
/// SSN or a password in any of them.
pub struct Request {
    /// The tag that acknowledges or rejects this delivery on the consumer channel.
    pub delivery_tag: u64,
    /// The message body, exactly as received.
    pub body: Vec<u8>,
    /// The `message_id` property (`MQMD-MSGID`), if present.
    pub message_id: Option<String>,
    /// The `correlation_id` property (`MQMD-CORRELID`), if present.
    pub correlation_id: Option<String>,
    /// The `reply_to` property (`MQMD-REPLYTOQ`), if present; only CP00 replies to it.
    pub reply_to: Option<String>,
    /// Whether the broker delivered this request before, to a channel that closed without
    /// acknowledging or rejecting it.
    pub redelivered: bool,
}

impl Request {
    /// The request view of a delivery taken with `basic_get`.
    fn from_delivery(delivery: Delivery) -> Self {
        let Delivery {
            delivery_tag,
            redelivered,
            properties,
            data,
            ..
        } = delivery;
        Self::from_parts(delivery_tag, redelivered, &properties, data)
    }

    /// The request view of a delivery's parts. Properties are copied, never indexed or
    /// unwrapped, so any value the broker accepted maps.
    fn from_parts(
        delivery_tag: u64,
        redelivered: bool,
        properties: &BasicProperties,
        body: Vec<u8>,
    ) -> Self {
        Self {
            delivery_tag,
            body,
            message_id: owned(properties.message_id().as_ref()),
            correlation_id: owned(properties.correlation_id().as_ref()),
            reply_to: owned(properties.reply_to().as_ref()),
            redelivered,
        }
    }

    /// The `message_id` property in the only form a span, an event or `Debug` may show it: a
    /// [`ScreenedText`], one `*` per byte, so a diagnostic learns whether it is present and its
    /// length, never its text.
    #[must_use]
    pub fn screened_message_id(&self) -> Option<ScreenedText<'_>> {
        self.message_id.as_deref().map(ScreenedText::new)
    }

    /// The `correlation_id` property in the only form a span, an event or `Debug` may show it: a
    /// [`ScreenedText`], one `*` per byte, so a diagnostic learns whether it is present and its
    /// length, never its text.
    #[must_use]
    pub fn screened_correlation_id(&self) -> Option<ScreenedText<'_>> {
        self.correlation_id.as_deref().map(ScreenedText::new)
    }

    /// The `reply_to` property in the only form a span, an event or `Debug` may show it: a
    /// [`ScreenedText`], one `*` per byte, so a diagnostic learns whether it is present and its
    /// length, never its text.
    #[must_use]
    pub fn screened_reply_to(&self) -> Option<ScreenedText<'_>> {
        self.reply_to.as_deref().map(ScreenedText::new)
    }
}

/// An owned copy of an optional short-string property.
fn owned(value: Option<&ShortString>) -> Option<String> {
    value.map(|text| text.as_str().to_owned())
}

impl fmt::Debug for Request {
    /// The delivery tag, the three properties as their [`ScreenedText`] shows them (one `*` per
    /// byte, or `None` when absent), the redelivery flag and the body's length; never a body byte
    /// or a byte of a property.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("delivery_tag", &self.delivery_tag)
            .field("message_id", &self.screened_message_id())
            .field("correlation_id", &self.screened_correlation_id())
            .field("reply_to", &self.screened_reply_to())
            .field("redelivered", &self.redelivered)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// The properties of a CDRD or CDRA reply and of their `CARD.DEMO.ERROR` messages
/// (CODATE01-010, CODATE01-011, CODATE01-018, COACCT01-024, COACCT01-025, COACCT01-032).
///
/// `4100-PUT-REPLY` moves the saved request IDs back into the descriptor before the put
/// (`CODATE01.cbl:373-374`, `COACCT01.cbl:469-470`), so the request's `message_id` and
/// `correlation_id` are copied unchanged, and an absent one stays absent. The message is
/// persistent (`delivery_mode` 2), the request's override, and carries no `expiration`, no
/// `reply_to` and no other property.
#[must_use]
pub fn copied_properties(request: &Request) -> BasicProperties {
    let mut properties = BasicProperties::default().with_delivery_mode(PERSISTENT);
    if let Some(message_id) = &request.message_id {
        properties = properties.with_message_id(ShortString::from(message_id.as_str()));
    }
    if let Some(correlation_id) = &request.correlation_id {
        properties = properties.with_correlation_id(ShortString::from(correlation_id.as_str()));
    }
    properties
}

/// The properties of a CP00 reply (COPAUA0C-067).
///
/// - `message_id`: `message_id`, the new ID the caller draws from `crate::ids::next_message_id`.
///   `7100-SEND-RESPONSE` moves `MQMI-NONE` to the reply's `MQMD-MSGID`, so the queue manager
///   generates one and none is the request's (`COPAUA0C.cbl:746`).
/// - `correlation_id`: the request's, unchanged (`COPAUA0C.cbl:745`); absent when the request
///   has none.
/// - `delivery_mode` 2: persistent, the request's override of `MQPER-NOT-PERSISTENT`
///   (`COPAUA0C.cbl:749`, E16).
/// - `expiration` [`CP00_REPLY_EXPIRATION`], the audited `MQMD-EXPIRY` of 50 tenths of a second
///   (`COPAUA0C.cbl:750`, E16).
/// - No `reply_to` (`MQMD-REPLYTOQ` is set to spaces) and no other property.
#[must_use]
pub fn cp00_properties(request: &Request, message_id: String) -> BasicProperties {
    let mut properties = BasicProperties::default()
        .with_delivery_mode(PERSISTENT)
        .with_message_id(ShortString::from(message_id))
        .with_expiration(ShortString::from(CP00_REPLY_EXPIRATION));
    if let Some(correlation_id) = &request.correlation_id {
        properties = properties.with_correlation_id(ShortString::from(correlation_id.as_str()));
    }
    properties
}

/// How a worker settles a request on the consumer channel that delivered it. Tests substitute
/// an implementation that fails, through the worker constructors.
///
/// A request is acknowledged only after the publication it causes is confirmed by
/// [`Publisher::publish`] (CODATE01-012, COACCT01-026, COPAUA0C-081, COPAUA0C-082).
pub trait Acknowledger: Send + Sync {
    /// Acknowledges the delivery, which removes the request from its queue for good.
    fn ack(&self, delivery_tag: u64) -> impl Future<Output = Result<(), AmqpError>> + Send;

    /// Rejects the delivery with `requeue: false`. The broker moves a
    /// `CARD.DEMO.REQUEST.PAUTH` request to [`DEAD_LETTER_QUEUE`] through that queue's
    /// dead-letter arguments (COPAUA0C-006, COPAUA0C-117). The CDRD and CDRA request queues
    /// carry no dead-letter arguments, so there the broker discards the request, as the COBOL
    /// task end does when neither its reply nor its error message could be put (CODATE01-019,
    /// COACCT01-034).
    fn reject_without_requeue(
        &self,
        delivery_tag: u64,
    ) -> impl Future<Output = Result<(), AmqpError>> + Send;
}

impl Acknowledger for Consumer {
    /// `basic_ack` on the consumer channel.
    async fn ack(&self, delivery_tag: u64) -> Result<(), AmqpError> {
        self.channel
            .basic_ack(delivery_tag, BasicAckOptions::default())
            .await
            .map_err(AmqpError::Ack)
    }

    /// `basic_reject` with `requeue: false` on the consumer channel.
    async fn reject_without_requeue(&self, delivery_tag: u64) -> Result<(), AmqpError> {
        self.channel
            .basic_reject(delivery_tag, BasicRejectOptions { requeue: false })
            .await
            .map_err(AmqpError::Reject)
    }
}

/// The delay before each attempt to reopen channels or reconnect: it starts at the poll
/// interval (`Config::mq_poll_interval`, `CARDDEMO_MQ_POLL_MS`) and doubles after every attempt
/// up to [`BACKOFF_CAP`], until [`Backoff::reset`] (COPAUA0C-118).
#[derive(Debug, Clone)]
pub struct Backoff {
    initial: Duration,
    current: Duration,
}

impl Backoff {
    /// A backoff starting at `initial`, held between 1 ms and [`BACKOFF_CAP`].
    #[must_use]
    pub fn new(initial: Duration) -> Self {
        let initial = initial.clamp(BACKOFF_FLOOR, BACKOFF_CAP);
        Self {
            initial,
            current: initial,
        }
    }

    /// The delay to wait now; the next call returns twice as much, up to [`BACKOFF_CAP`].
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = self.current.saturating_mul(2).min(BACKOFF_CAP);
        delay
    }

    /// Starts again from the initial delay, after a successfully processed message.
    pub fn reset(&mut self) {
        self.current = self.initial;
    }

    /// Sleeps for [`Backoff::next_delay`].
    pub async fn wait(&mut self) {
        tokio::time::sleep(self.next_delay()).await;
    }
}

/// Waits `poll_interval` (`CARDDEMO_MQ_POLL_MS`, default 250 ms) after [`Consumer::get`] found
/// the request queue empty. This poll replaces the COBOL's five-second `MQGMO-WAIT`; no stream
/// consumer is used.
pub async fn idle(poll_interval: Duration) {
    tokio::time::sleep(poll_interval).await;
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;
    use std::sync::Arc;
    use std::time::Duration;

    use lapin::protocol::{AMQPError, AMQPErrorKind, AMQPHardError, AMQPSoftError};
    use lapin::types::ShortString;
    use lapin::uri::AMQPUri;
    use lapin::{BasicProperties, ChannelState, ConnectionState};

    use super::{
        AmqpError, BACKOFF_CAP, Backoff, CP00_REPLY_EXPIRATION, DEAD_LETTER_QUEUE, PublishFailure,
        Request, copied_properties, cp00_properties, parse_url,
    };

    /// The first card number of `app/data/ASCII/cardxref.txt`.
    const CARD_NUMBER: &str = "0500024453765740";

    fn request(message_id: Option<&str>, correlation_id: Option<&str>) -> Request {
        Request {
            delivery_tag: 7,
            body: format!("000001,120000,{CARD_NUMBER},0,1,100.00").into_bytes(),
            message_id: message_id.map(str::to_owned),
            correlation_id: correlation_id.map(str::to_owned),
            reply_to: Some("REQUESTER.REPLY".to_owned()),
            redelivered: false,
        }
    }

    fn text(value: Option<&ShortString>) -> Option<&str> {
        value.map(ShortString::as_str)
    }

    #[test]
    fn backoff_doubles_from_the_poll_interval_to_the_cap_and_resets() {
        let mut backoff = Backoff::new(Duration::from_millis(250));
        let delays: Vec<Duration> = (0..7).map(|_| backoff.next_delay()).collect();
        let expected = [250, 500, 1_000, 2_000, 4_000, 5_000, 5_000].map(Duration::from_millis);
        assert_eq!(delays, expected);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
    }

    #[test]
    fn backoff_starts_between_one_millisecond_and_the_cap() {
        let mut from_zero = Backoff::new(Duration::ZERO);
        assert_eq!(from_zero.next_delay(), Duration::from_millis(1));
        assert_eq!(from_zero.next_delay(), Duration::from_millis(2));
        let mut from_long = Backoff::new(Duration::from_secs(60));
        assert_eq!(from_long.next_delay(), BACKOFF_CAP);
        assert_eq!(from_long.next_delay(), BACKOFF_CAP);
        from_long.reset();
        assert_eq!(from_long.next_delay(), BACKOFF_CAP);
    }

    // audit: CODATE01-010
    // audit: CODATE01-011
    // audit: CODATE01-018
    // audit: COACCT01-024
    // audit: COACCT01-025
    // audit: COACCT01-032
    #[test]
    fn copied_properties_copy_both_ids_into_a_persistent_message_without_expiration() {
        let properties = copied_properties(&request(Some("MSG-0001"), Some("CORR-0001")));
        assert_eq!(text(properties.message_id().as_ref()), Some("MSG-0001"));
        assert_eq!(
            text(properties.correlation_id().as_ref()),
            Some("CORR-0001")
        );
        assert_eq!(*properties.delivery_mode(), Some(2));
        let expected = BasicProperties::default()
            .with_delivery_mode(2)
            .with_message_id(ShortString::from("MSG-0001"))
            .with_correlation_id(ShortString::from("CORR-0001"));
        assert_eq!(
            properties, expected,
            "no expiration, reply_to or other property"
        );
    }

    // audit: CODATE01-010
    // audit: COACCT01-024
    #[test]
    fn copied_properties_leave_an_absent_id_absent() {
        let properties = copied_properties(&request(None, Some("CORR-0002")));
        assert_eq!(*properties.message_id(), None);
        assert_eq!(
            text(properties.correlation_id().as_ref()),
            Some("CORR-0002")
        );
        let properties = copied_properties(&request(Some("MSG-0003"), None));
        assert_eq!(text(properties.message_id().as_ref()), Some("MSG-0003"));
        assert_eq!(*properties.correlation_id(), None);
        assert_eq!(
            copied_properties(&request(None, None)),
            BasicProperties::default().with_delivery_mode(2)
        );
    }

    // audit: COPAUA0C-067
    #[test]
    fn cp00_properties_carry_a_new_id_the_correlation_persistence_and_the_expiry() {
        let properties = cp00_properties(
            &request(Some("REQUEST-MSG"), Some("CORR-PAUTH")),
            "20220718102030456-1-0".to_owned(),
        );
        assert_eq!(
            text(properties.message_id().as_ref()),
            Some("20220718102030456-1-0")
        );
        assert_eq!(
            text(properties.correlation_id().as_ref()),
            Some("CORR-PAUTH")
        );
        assert_eq!(*properties.delivery_mode(), Some(2));
        assert_eq!(text(properties.expiration().as_ref()), Some("5000"));
        assert_eq!(CP00_REPLY_EXPIRATION, "5000");
        let expected = BasicProperties::default()
            .with_delivery_mode(2)
            .with_message_id(ShortString::from("20220718102030456-1-0"))
            .with_expiration(ShortString::from("5000"))
            .with_correlation_id(ShortString::from("CORR-PAUTH"));
        assert_eq!(properties, expected, "no reply_to and no other property");
    }

    // audit: COPAUA0C-067
    #[test]
    fn cp00_properties_leave_an_absent_correlation_absent() {
        let properties = cp00_properties(&request(None, None), "ID-1".to_owned());
        assert_eq!(*properties.correlation_id(), None);
        assert_eq!(text(properties.message_id().as_ref()), Some("ID-1"));
        assert_eq!(text(properties.expiration().as_ref()), Some("5000"));
    }

    #[test]
    fn a_request_copies_the_delivery_and_its_three_properties() {
        let properties = BasicProperties::default()
            .with_message_id(ShortString::from("MSG"))
            .with_correlation_id(ShortString::from("CORR"))
            .with_reply_to(ShortString::from("REPLY.Q"))
            .with_expiration(ShortString::from("60000"));
        let request = Request::from_parts(42, true, &properties, b"body".to_vec());
        assert_eq!(request.delivery_tag, 42);
        assert!(request.redelivered);
        assert_eq!(request.body, b"body");
        assert_eq!(request.message_id.as_deref(), Some("MSG"));
        assert_eq!(request.correlation_id.as_deref(), Some("CORR"));
        assert_eq!(request.reply_to.as_deref(), Some("REPLY.Q"));

        let bare = Request::from_parts(1, false, &BasicProperties::default(), Vec::new());
        assert_eq!(bare.message_id, None);
        assert_eq!(bare.correlation_id, None);
        assert_eq!(bare.reply_to, None);
        assert!(bare.body.is_empty());
    }

    #[test]
    fn request_debug_shows_the_body_length_and_never_a_body_byte() {
        // Property values a requester may choose: a card number, whole and spaced out, an SSN
        // with and without dashes, an 8-byte password, a password holding a space, a short
        // password and a short ID.
        let probes = [
            CARD_NUMBER,
            "123-45-6789",
            "123456789",
            "PASSWORD",
            "PASS WOR",
            "SECRET",
            "M-1",
            "0500 0244 5376 5740",
        ];
        // The body's fields, none of which `Debug` may show.
        let body_fields = ["000001", "120000", CARD_NUMBER, "100.00"];
        for probe in probes {
            let mut protected = request(Some(probe), Some(probe));
            protected.reply_to = Some(probe.to_owned());
            let shown = format!("{protected:?}");
            let stars = "*".repeat(probe.len());
            for field in ["message_id", "correlation_id", "reply_to"] {
                let expected = format!(r#"{field}: Some("{stars}")"#);
                assert!(shown.contains(&expected), "{expected} missing from {shown}");
            }
            assert!(
                shown.contains(&format!("body_len: {}", protected.body.len())),
                "{shown}"
            );
            for value in probes.into_iter().chain(body_fields) {
                assert!(!shown.contains(value), "{value:?} in {shown}");
            }
        }

        let mut absent = request(Some("M-1"), None);
        absent.reply_to = None;
        let shown = format!("{absent:?}");
        for expected in [
            r#"message_id: Some("***")"#,
            "correlation_id: None",
            "reply_to: None",
        ] {
            assert!(shown.contains(expected), "{expected} missing from {shown}");
        }
        assert!(!shown.contains("M-1"), "{shown}");
    }

    #[test]
    fn an_invalid_url_error_never_repeats_the_url() {
        let url = "amqp:carddemo-local-password";
        let parser_message = url.parse::<AMQPUri>().err().unwrap_or_default();
        assert!(
            parser_message.contains("carddemo-local-password"),
            "the parser's own message repeats the URL, which is why it is dropped"
        );
        let error = parse_url(url).err();
        assert!(matches!(error, Some(AmqpError::InvalidUrl)));
        let shown = error
            .map(|error| format!("{error} {error:?}"))
            .unwrap_or_default();
        assert!(!shown.contains("carddemo-local-password"), "{shown}");
        assert!(parse_url("amqp://localhost:5672/%2f").is_ok());
    }

    #[test]
    fn connection_loss_is_told_from_a_channel_failure() {
        let io = lapin::Error::IOError(Arc::new(io::Error::from(io::ErrorKind::ConnectionReset)));
        let forced = lapin::Error::ProtocolError(AMQPError::new(
            AMQPErrorKind::Hard(AMQPHardError::CONNECTIONFORCED),
            ShortString::from("broker shut down"),
        ));
        let not_found = lapin::Error::ProtocolError(AMQPError::new(
            AMQPErrorKind::Soft(AMQPSoftError::NOTFOUND),
            ShortString::from("no queue"),
        ));
        let lost = [
            AmqpError::Connect(io.clone()),
            AmqpError::Get(io),
            AmqpError::Ack(lapin::Error::MissingHeartbeatError),
            AmqpError::Channel(lapin::Error::InvalidConnectionState(
                ConnectionState::Closed,
            )),
            AmqpError::Reject(forced),
        ];
        for error in &lost {
            assert!(error.is_connection_lost(), "{error}");
        }
        let channel_only = [
            AmqpError::InvalidUrl,
            AmqpError::QueueName {
                variable: "CARDDEMO_QUEUE_REQUEST_ACCT",
                max_len: 48,
            },
            AmqpError::Declare {
                queue: "CARD.DEMO.ERROR".to_owned(),
                source: not_found,
            },
            AmqpError::Get(lapin::Error::InvalidChannelState(ChannelState::Closed)),
            AmqpError::Ack(lapin::Error::InvalidChannelState(ChannelState::Error)),
            AmqpError::Channel(lapin::Error::InvalidConnectionState(
                ConnectionState::Connected,
            )),
            AmqpError::Channel(lapin::Error::ChannelsLimitReached),
        ];
        for error in &channel_only {
            assert!(!error.is_connection_lost(), "{error}");
        }
    }

    #[test]
    fn error_texts_name_the_queue_and_keep_the_client_error_as_source() {
        let declare = AmqpError::Declare {
            queue: "CARD.DEMO.REQUEST.PAUTH".to_owned(),
            source: lapin::Error::InvalidChannelState(ChannelState::Closed),
        };
        assert!(
            declare.to_string().contains("CARD.DEMO.REQUEST.PAUTH"),
            "{declare}"
        );
        assert!(declare.source().is_some());
        assert!(AmqpError::InvalidUrl.source().is_none());

        let failures = [
            PublishFailure::Returned,
            PublishFailure::Nacked,
            PublishFailure::NotConfirmed,
        ];
        for failure in &failures {
            assert!(failure.source().is_none(), "{failure}");
            assert!(!failure.to_string().is_empty());
        }
        let channel =
            PublishFailure::Channel(lapin::Error::InvalidChannelState(ChannelState::Closed));
        assert!(channel.source().is_some(), "{channel}");
    }

    #[test]
    fn the_dead_letter_queue_is_the_one_the_definitions_route_to() {
        let definitions = include_str!("../../../rabbitmq/definitions.json");
        let routing = format!("\"x-dead-letter-routing-key\": \"{DEAD_LETTER_QUEUE}\"");
        assert!(definitions.contains(&routing));
        assert!(definitions.contains(&format!("\"name\": \"{DEAD_LETTER_QUEUE}\"")));
    }
}
