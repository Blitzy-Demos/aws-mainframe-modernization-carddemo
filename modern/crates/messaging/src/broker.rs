//! The broker rules the three `RabbitMQ` workers share (AAP 0.3.4.2), implemented once: the
//! connection, the two channels each worker owns, the passive queue checks, `basic_get` polling,
//! the confirmed mandatory publish, the reply properties, the acknowledgement seam and the
//! reconnect backoff. The workers (`workers::{cdrd, cdra, cp00}`) reach the broker only through
//! this module, and no COBOL body layout lives here.
//!
//! | Item | MQ counterpart | Rules |
//! |---|---|---|
//! | [`connect`], [`Session`] | the queue-manager connection of the CICS region | — |
//! | [`declare_passive`] | `MQOPEN` of each queue | the queues of `rabbitmq/definitions.json` |
//! | [`Consumer::get`] | `MQGET` with the descriptor IDs reset before every get | CODATE01-001, COACCT01-001, COPAUA0C-001 |
//! | [`Publisher::publish`] | `MQPUT` / `MQPUT1` | CODATE01-002, CODATE01-012, CODATE01-014, COACCT01-023, COACCT01-026, COACCT01-033, COPAUA0C-066, COPAUA0C-117 |
//! | [`copied_properties`] | `MOVE SAVE-MSGID`, `MOVE SAVE-CORELID` (`CODATE01.cbl:373-374`) | CODATE01-010, CODATE01-011, CODATE01-018, COACCT01-024, COACCT01-025, COACCT01-032 |
//! | [`cp00_properties`] | the reply descriptor of `7100-SEND-RESPONSE` (`COPAUA0C.cbl:745-750`) | COPAUA0C-067 |
//! | [`is_direct_reply_to`] | none: a `REPLYTOQ` always names a real queue | COPAUA0C-066, COPAUA0C-117 |
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
//! - **No Direct Reply-to.** A destination that names `RabbitMQ`'s Direct Reply-to pseudo-queue
//!   ([`is_direct_reply_to`]) is refused before any publication: `RabbitMQ` 3.13 both delivers
//!   and returns a mandatory message sent there, so its publication can never be confirmed
//!   without having been delivered.
//! - **Queue names.** Request queues come from `runtime::config::Config`. Reply and error queues
//!   are the COBOL literals, owned by `domain::messaging::codate01` and
//!   `domain::messaging::coacct01`; CP00 replies go to the request's `reply_to`. The README
//!   queue names (E01) appear in no source and are not used.
//! - **No task boundary.** Each request is processed as the first message of a CICS task, and
//!   nothing here carries state from one request to the next (E14, E15).
//!
//! # Deadlines
//!
//! Every broker wait of this module ends by a deadline, cleanup included: the connection and its
//! handshake within [`CONNECT_DEADLINE`], a publication and its confirmation within
//! [`PUBLISH_DEADLINE`], and every other operation, channel and connection closes included,
//! within [`OPERATION_DEADLINE`]. Heartbeats prove only that the socket is alive: a broker that
//! keeps heartbeating while it withholds a reply would otherwise hold a worker, and a CP00
//! worker's authorization transaction and summary lock, for good. A missed deadline is
//! [`Fault::TimedOut`], which leaves the channel and the connection in an unknown state and
//! therefore counts as a lost connection ([`Fault::is_connection_lost`]): the worker stops, the
//! [`Session`] is closed with [`Session::close`] and dropped, and a new connection replaces it,
//! so a late result of the abandoned operation can settle nothing. A timed-out publication is a
//! [`PublishFailure`], never a confirmation.
//!
//! A deadline that passes must also end the TCP connection, because only then does the broker
//! drop the session and requeue every delivery it left unsettled. The client offers no abort: a
//! connection dropped while its handshake or its `connection.close` is unanswered keeps its
//! socket open for as long as the broker sends heartbeats. [`connect`] therefore opens the
//! socket itself and keeps a second handle to it, with which the socket is shut down when the
//! connection attempt fails, misses its deadline or is abandoned, when a close
//! [`Session::close`] makes fails or goes unanswered, and when the [`Session`] is dropped
//! without a close the broker answered.
//!
//! # Redaction
//!
//! CP00 request bodies hold full card numbers. No `Debug`, `Display`, error text or tracing
//! event of this module shows a body byte: [`Request`]'s `Debug` gives the body length and the
//! three properties as their [`ScreenedText`] shows them, one `*` per byte. [`AmqpError`] and
//! [`PublishFailure`] name an operation and a value-free [`Fault`], and a queue only by its
//! [`QueueLabel`]: a configured request queue by the environment variable that holds its name,
//! any other queue by the source constant that names it, never a name read at run time, which
//! may hold any value an operator typed. They keep no client error, because its text repeats the
//! broker's close `reply_text`. The AMQP URL carries the local password, so it is never logged
//! and no error repeats it.
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
use std::io;
use std::net::{Shutdown, TcpStream as Socket, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use lapin::message::{BasicGetMessage, Delivery};
use lapin::options::{
    BasicAckOptions, BasicGetOptions, BasicPublishOptions, BasicRejectOptions,
    ConfirmSelectOptions, QueueDeclareOptions,
};
use lapin::protocol::AMQPErrorKind;
use lapin::publisher_confirm::Confirmation;
use lapin::tcp::{HandshakeResult, TLSConfig, TcpStream};
use lapin::types::{FieldTable, ShortString};
use lapin::uri::{AMQPScheme, AMQPUri};
use lapin::{
    BasicProperties, Channel, ChannelState, Connection, ConnectionProperties, ConnectionState,
};
use runtime::telemetry::ScreenedText;

/// How long [`connect`] may take from the TCP connect to the broker's `connection.open-ok`: the
/// socket, the protocol header and the `start`, `tune` and `open` exchanges together. The
/// default connection properties set no deadline, so a broker that accepts the socket and never
/// finishes the handshake would hold the first connection, or a reconnect, for good. Ten seconds
/// is `RabbitMQ`'s own default `handshake_timeout`, which a merely slow broker still meets.
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(10);

/// How long [`Publisher::publish`] may take from handing the message to the publisher channel
/// to the broker's confirmation, both waits together. A persistent message on a durable queue is
/// normally confirmed within milliseconds, after the broker's next disk sync. A CP00 worker holds
/// its authorization transaction and the account's summary lock across this wait and the
/// acknowledgement after it, so the two deadlines bound how long every other decision for that
/// account can be kept waiting by a broker that withholds a confirmation: at most this plus
/// [`OPERATION_DEADLINE`], beyond the worker's own database work. Five seconds is the life the
/// COBOL gives the CP00 reply itself (`MQMD-EXPIRY` 50 tenths, [`CP00_REPLY_EXPIRATION`]).
pub const PUBLISH_DEADLINE: Duration = Duration::from_secs(5);

/// How long every other broker operation may take: a method the broker answers
/// (`channel.open`, `confirm.select`, the passive `queue.declare`, `basic.get`, `channel.close`
/// and `connection.close`), or a frame the client must hand to the socket (`basic.ack`,
/// `basic.reject`). A healthy broker answers each in milliseconds, so five seconds is ample
/// margin for a loaded one. It bounds the acknowledgement half of a CP00 worker's lock hold (see
/// [`PUBLISH_DEADLINE`]), each idle poll of a worker and every step of a reconnect.
pub const OPERATION_DEADLINE: Duration = Duration::from_secs(5);

/// The queue to which the broker moves a `CARD.DEMO.REQUEST.PAUTH` request that a worker
/// rejects with `requeue: false`, through the request queue's `x-dead-letter-exchange` `""` and
/// `x-dead-letter-routing-key` arguments in `rabbitmq/definitions.json`. CP00 rejects a request
/// whose card number cannot be parsed (i11; COPAUA0C-006) and one whose reply could not be
/// published (`M004`; COPAUA0C-117).
pub const DEAD_LETTER_QUEUE: &str = "CARD.DEMO.DEADLETTER.PAUTH";

/// The name of `RabbitMQ`'s Direct Reply-to pseudo-queue. A requester that consumes from it
/// sends it as its request's `reply_to`, and the broker hands the request on with the `reply_to`
/// rewritten to this name, a `.` and a suffix that addresses the requester's channel. MQ has no
/// counterpart: a `REPLYTOQ` always names a real queue.
pub const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// Whether `queue` names the Direct Reply-to pseudo-queue: exactly [`DIRECT_REPLY_TO`], or it
/// followed by `.` and a suffix. The comparison is exact and case-sensitive, as `RabbitMQ`'s
/// queue names are, so `amq.rabbitmq.reply-tox` is an ordinary queue name.
///
/// No publication to such a queue can meet the confirmation rule without delivering the message
/// first: `RabbitMQ` 3.13 delivers a `mandatory: true` publication to the waiting requester and
/// also returns it as unroutable, so [`Publisher::publish`] would report
/// [`PublishFailure::Returned`] for a message the requester already holds. A worker therefore
/// refuses such a destination before it publishes anything (COPAUA0C-117, `X-COPAUA0C-12`),
/// whatever the broker version does with the return, and never relies on the return itself.
#[must_use]
pub fn is_direct_reply_to(queue: &str) -> bool {
    queue
        .strip_prefix(DIRECT_REPLY_TO)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('.'))
}

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

/// A broker operation that failed. Each variant names the operation; the [`Fault`] it carries
/// names the client's or the broker's condition by its class and AMQP reply code only.
///
/// No variant keeps a `lapin::Error`: [`Fault::from`] keeps only its class, because the client
/// error's text repeats what the broker sent, the `reply_text` of a channel or connection close
/// included, and the broker writes queue names and other caller-controlled text there. The
/// `Display`, `Debug` and [`Error::source`] of this type therefore never show broker, parser or
/// operating-system text, a message body, the AMQP URL or a configured queue name. `Debug` is
/// derived because every field is value-free by its type: a [`Fault`], a [`QueueLabel`], which
/// names a queue only by its environment variable or its source constant, a variable name and a
/// length.
#[derive(Debug, Clone)]
pub enum AmqpError {
    /// `CARDDEMO_AMQP_URL` is not a valid AMQP URL. The parser's own message is dropped,
    /// because it can repeat the URL, password included.
    InvalidUrl,
    /// The request queue name configured in `variable` is refused before its worker opens a
    /// channel: it is longer than `max_len` bytes or holds a byte outside printable ASCII. CDRD
    /// and CP00 refuse a `CARDDEMO_QUEUE_REQUEST_DATE` or `CARDDEMO_QUEUE_REQUEST_PAUTH` longer
    /// than the 255 bytes an AMQP short string carries, over which the broker would close the
    /// whole connection. CDRA refuses a `CARDDEMO_QUEUE_REQUEST_ACCT` longer than 48 bytes,
    /// because its account-read error body names the request queue in
    /// `MQ-APPL-QUEUE-NAME PIC X(48)` (COACCT01-029, X-COACCT01-8). The text names the variable,
    /// never the rejected value.
    QueueName {
        /// The environment variable that holds the rejected name.
        variable: &'static str,
        /// The longest name, in bytes, the worker accepts.
        max_len: usize,
    },
    /// The connection to the broker could not be established within [`CONNECT_DEADLINE`].
    Connect(Fault),
    /// A channel could not be opened or put in confirm mode.
    Channel(Fault),
    /// The passive declare of `queue` failed: the queue does not exist (AMQP 404), or the channel
    /// or the connection failed first.
    Declare {
        /// The queue that was checked, by its value-free label: the environment variable that
        /// configures a request queue, or the source constant that names any other queue.
        queue: QueueLabel,
        /// The class of the broker's or the client's error.
        source: Fault,
    },
    /// `basic_get` failed on the consumer channel.
    Get(Fault),
    /// `basic_ack` failed on the consumer channel.
    Ack(Fault),
    /// `basic_reject` failed on the consumer channel.
    Reject(Fault),
}

impl AmqpError {
    /// Whether the AMQP connection behind the failed operation is gone, or in a state no longer
    /// known, so that reopening channels on it cannot help and only a new [`connect`] can.
    ///
    /// - lost: [`AmqpError::Connect`], and every variant whose [`Fault`] is lost by
    ///   [`Fault::is_connection_lost`], a missed deadline included.
    /// - not lost: [`AmqpError::InvalidUrl`] and [`AmqpError::QueueName`], configuration errors
    ///   no reconnect cures, and every variant whose fault concerns the channel only.
    ///
    /// After a connection loss the client marks its channels failed, so the next operation on a
    /// channel reports a channel state and counts as not lost. The worker then reopens its
    /// channels, `create_channel` reports the connection state, and that error counts as lost.
    #[must_use]
    pub fn is_connection_lost(&self) -> bool {
        match self {
            Self::InvalidUrl | Self::QueueName { .. } => false,
            Self::Connect(_) => true,
            Self::Channel(fault)
            | Self::Declare { source: fault, .. }
            | Self::Get(fault)
            | Self::Ack(fault)
            | Self::Reject(fault) => fault.is_connection_lost(),
        }
    }
}

/// The value-free class of a client or broker failure: which condition occurred, never the text
/// that came with it.
///
/// [`Fault::from`] keeps the AMQP reply code of a protocol error and drops its `reply_text`,
/// keeps the [`io::ErrorKind`] of an I/O error and drops the operating system's message, keeps
/// the state name of a channel or connection state error, and keeps only the class of a parse or
/// serialisation error, whose text can quote frame bytes. No variant holds a string.
#[derive(Debug, Clone, PartialEq)]
pub enum Fault {
    /// The operation did not finish within its deadline, `after`: [`CONNECT_DEADLINE`],
    /// [`PUBLISH_DEADLINE`] or [`OPERATION_DEADLINE`]. The operation was abandoned, and the
    /// broker may still act on it, so the channel and the connection are in an unknown state.
    TimedOut {
        /// The deadline that passed.
        after: Duration,
    },
    /// An AMQP error with reply `code`, for example 404 `NOT_FOUND` when a passive declare finds
    /// no queue (a soft error, which closes the channel) or 320 `CONNECTION_FORCED` (a hard
    /// error, which closes the connection).
    Protocol {
        /// The AMQP reply code.
        code: u16,
    },
    /// An I/O error of the socket, by its kind.
    Io(io::ErrorKind),
    /// No heartbeat came from the broker for too long.
    MissingHeartbeat,
    /// A frame from the broker could not be parsed.
    Parsing,
    /// A frame could not be serialised.
    Serialisation,
    /// The broker does not speak AMQP 0-9-1.
    ProtocolVersion,
    /// The channel is not open: it is closing, closed or failed.
    ChannelState(ChannelState),
    /// The connection is not in the state the operation needs.
    ConnectionState(ConnectionState),
    /// The channel the operation named does not exist.
    InvalidChannel,
    /// The connection has as many channels as the broker allows.
    ChannelsLimit,
    /// A client error of a kind this crate does not classify, from a later client release.
    Unclassified,
}

impl Fault {
    /// Whether the fault ends the connection or leaves its state unknown:
    ///
    /// - lost: a missed deadline ([`Fault::TimedOut`]), after which neither the channel nor the
    ///   connection can be trusted to settle the right delivery; an I/O error; a missed
    ///   heartbeat; a frame the client could not parse or serialise, or a protocol version it
    ///   rejected, each of which closes the connection; a connection state other than
    ///   connected; a hard AMQP error such as `CONNECTION_FORCED`, with which the broker closes
    ///   the connection.
    /// - not lost: a soft AMQP error such as `NOT_FOUND` or `PRECONDITION_FAILED`, which closes
    ///   only the channel; an invalid channel or channel state; the channel limit; an
    ///   unclassified client error.
    #[must_use]
    pub fn is_connection_lost(&self) -> bool {
        match self {
            Self::TimedOut { .. }
            | Self::Io(_)
            | Self::MissingHeartbeat
            | Self::Parsing
            | Self::Serialisation
            | Self::ProtocolVersion => true,
            Self::ConnectionState(state) => !matches!(state, ConnectionState::Connected),
            Self::Protocol { code } => {
                matches!(AMQPErrorKind::from_id(*code), Some(AMQPErrorKind::Hard(_)))
            }
            Self::ChannelState(_)
            | Self::InvalidChannel
            | Self::ChannelsLimit
            | Self::Unclassified => false,
        }
    }
}

impl From<&lapin::Error> for Fault {
    /// The class of `error`, without any of its text.
    fn from(error: &lapin::Error) -> Self {
        match error {
            lapin::Error::ProtocolError(protocol) => Self::Protocol {
                code: protocol.get_id(),
            },
            lapin::Error::IOError(io) => Self::Io(io.kind()),
            lapin::Error::MissingHeartbeatError => Self::MissingHeartbeat,
            lapin::Error::ParsingError(_) => Self::Parsing,
            lapin::Error::SerialisationError(_) => Self::Serialisation,
            lapin::Error::InvalidProtocolVersion(_) => Self::ProtocolVersion,
            lapin::Error::InvalidChannelState(state) => Self::ChannelState(state.clone()),
            lapin::Error::InvalidConnectionState(state) => Self::ConnectionState(state.clone()),
            lapin::Error::InvalidChannel(_) => Self::InvalidChannel,
            lapin::Error::ChannelsLimitReached => Self::ChannelsLimit,
            // The client's error enumeration is non-exhaustive.
            _ => Self::Unclassified,
        }
    }
}

impl From<lapin::Error> for Fault {
    /// The class of `error`, without any of its text; `error` is dropped.
    fn from(error: lapin::Error) -> Self {
        Self::from(&error)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut { after } => write!(f, "no response within the {after:?} deadline"),
            Self::Protocol { code } => match AMQPErrorKind::from_id(*code) {
                Some(AMQPErrorKind::Soft(kind)) => {
                    write!(f, "AMQP soft error {kind} (reply code {code})")
                }
                Some(AMQPErrorKind::Hard(kind)) => {
                    write!(f, "AMQP hard error {kind} (reply code {code})")
                }
                None => write!(f, "AMQP error with reply code {code}"),
            },
            Self::Io(kind) => write!(f, "I/O error: {kind}"),
            Self::MissingHeartbeat => f.write_str("no heartbeat from the broker for too long"),
            Self::Parsing => f.write_str("a frame from the broker could not be parsed"),
            Self::Serialisation => f.write_str("a frame could not be serialised"),
            Self::ProtocolVersion => f.write_str("the broker does not speak AMQP 0-9-1"),
            Self::ChannelState(state) => write!(f, "invalid channel state {state:?}"),
            Self::ConnectionState(state) => write!(f, "invalid connection state {state:?}"),
            Self::InvalidChannel => f.write_str("invalid channel"),
            Self::ChannelsLimit => f.write_str("the connection's channel limit is reached"),
            Self::Unclassified => f.write_str("an unclassified client error"),
        }
    }
}

impl Error for Fault {}

/// A queue as diagnostics name it, value-free by construction: a configured request queue by the
/// environment variable that holds its name, and a queue whose name is a constant of the source
/// by that constant. Neither variant holds a name read at run time, so no `Display`, `Debug` or
/// error text built from a label can repeat what an operator configured, which may be anything,
/// a card number or a password included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueLabel {
    /// A request queue whose name comes from `runtime::config::Config`, shown as the environment
    /// variable that holds it, for example `CARDDEMO_QUEUE_REQUEST_PAUTH`.
    Configured {
        /// The environment variable that holds the queue name.
        variable: &'static str,
    },
    /// A queue whose name is a constant of the source and of `rabbitmq/definitions.json`, shown
    /// as that name: a COBOL reply or error queue literal (`CARD.DEMO.REPLY.DATE`,
    /// `CARD.DEMO.REPLY.ACCT`, `CARD.DEMO.ERROR`) or [`DEAD_LETTER_QUEUE`].
    Literal(&'static str),
}

impl fmt::Display for QueueLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configured { variable } => write!(f, "the queue named by {variable}"),
            Self::Literal(name) => write!(f, "queue {name}"),
        }
    }
}

/// A queue [`declare_passive`] checks: the name sent to the broker, and the [`QueueLabel`] that
/// a failure shows in its place. Its `Debug` shows only the label, because a configured name is
/// not value-free.
#[derive(Clone, Copy)]
pub struct QueueCheck<'a> {
    name: &'a str,
    label: QueueLabel,
}

impl<'a> QueueCheck<'a> {
    /// The request queue `name` configured in the environment variable `variable`; a failure
    /// shows the variable, never `name`.
    #[must_use]
    pub fn configured(variable: &'static str, name: &'a str) -> Self {
        Self {
            name,
            label: QueueLabel::Configured { variable },
        }
    }

    /// The queue named by the source constant `name`; a failure shows `name` itself.
    #[must_use]
    pub fn literal(name: &'static str) -> Self {
        Self {
            name,
            label: QueueLabel::Literal(name),
        }
    }

    /// The label a failure of this check shows.
    #[must_use]
    pub fn label(&self) -> QueueLabel {
        self.label
    }
}

impl fmt::Debug for QueueCheck<'_> {
    /// The label only; never the name sent to the broker.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueCheck")
            .field("label", &self.label)
            .finish_non_exhaustive()
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
            Self::Connect(fault) => write!(f, "cannot connect to the AMQP broker: {fault}"),
            Self::Channel(fault) => write!(f, "cannot open an AMQP channel: {fault}"),
            Self::Declare { queue, source } => {
                write!(f, "passive declare of {queue} failed: {source}")
            }
            Self::Get(fault) => write!(f, "basic.get failed: {fault}"),
            Self::Ack(fault) => write!(f, "basic.ack failed: {fault}"),
            Self::Reject(fault) => write!(f, "basic.reject failed: {fault}"),
        }
    }
}

impl Error for AmqpError {
    /// The value-free [`Fault`], never the client error it was classified from.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUrl | Self::QueueName { .. } => None,
            Self::Connect(fault)
            | Self::Channel(fault)
            | Self::Declare { source: fault, .. }
            | Self::Get(fault)
            | Self::Ack(fault)
            | Self::Reject(fault) => Some(fault),
        }
    }
}

/// Why a publication was not confirmed, so the request it answers must not be acknowledged
/// (CODATE01-014, COACCT01-033, COPAUA0C-117). Like [`AmqpError`], it keeps only a value-free
/// [`Fault`], so its derived `Debug` and its `Display` never show broker text.
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
    /// `basic_publish` or the wait for its confirmation failed on the publisher channel, or the
    /// two together did not finish within [`PUBLISH_DEADLINE`] ([`Fault::TimedOut`]). A
    /// publication that timed out may still reach its queue later, but it is never counted as
    /// confirmed: the request it answers takes its failure path.
    Channel(Fault),
}

impl fmt::Display for PublishFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Returned => f.write_str("the broker returned the message: no queue matched"),
            Self::Nacked => f.write_str("the broker did not accept the message (nack)"),
            Self::NotConfirmed => f.write_str("the publisher channel is not in confirm mode"),
            Self::Channel(fault) => {
                write!(f, "publication failed on the publisher channel: {fault}")
            }
        }
    }
}

impl Error for PublishFailure {
    /// The value-free [`Fault`] of [`PublishFailure::Channel`], never a client error.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Channel(fault) => Some(fault),
            Self::Returned | Self::Nacked | Self::NotConfirmed => None,
        }
    }
}

/// Awaits `operation` for at most `deadline`. A client or broker error becomes its [`Fault`]; a
/// missed deadline becomes [`Fault::TimedOut`], and the unfinished operation is dropped, so a
/// late result, if one ever comes, reaches no caller.
async fn within<T>(
    deadline: Duration,
    operation: impl Future<Output = lapin::Result<T>>,
) -> Result<T, Fault> {
    match tokio::time::timeout(deadline, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(Fault::from(&error)),
        Err(_elapsed) => Err(Fault::TimedOut { after: deadline }),
    }
}

/// Opens the AMQP connection the workers share, as a [`Session`]. `amqp_url` is
/// `Config::amqp_url` (`CARDDEMO_AMQP_URL`), the only source of the broker address and
/// credentials; it is never logged.
///
/// The TCP connect and the whole AMQP handshake must finish within [`CONNECT_DEADLINE`]. The
/// socket is opened here, as the client's own connector would open it, `amqps` included, so
/// that the session keeps a second handle to it. When the attempt fails or misses its
/// deadline, or the returned future is dropped unfinished, as a stop drops it, the socket is
/// shut down through that handle. The client alone would keep it open, its handshake
/// unfinished, for as long as the broker sends heartbeats.
///
/// # Errors
///
/// [`AmqpError::InvalidUrl`] when `amqp_url` does not parse as an AMQP URL, and
/// [`AmqpError::Connect`] when the broker cannot be reached, refuses the login or does not
/// finish the handshake in time ([`Fault::TimedOut`]).
pub async fn connect(amqp_url: &str) -> Result<Session, AmqpError> {
    connect_within(amqp_url, CONNECT_DEADLINE).await
}

/// [`connect`] with `deadline` in place of [`CONNECT_DEADLINE`].
async fn connect_within(amqp_url: &str, deadline: Duration) -> Result<Session, AmqpError> {
    let uri = parse_url(amqp_url)?;
    // Dropped on every failure and when this future is dropped unfinished, which shuts the
    // socket down; kept by the session on success.
    let transport = Transport::new();
    let connector = transport.connector(Instant::now(), deadline);
    let connection = within(
        deadline,
        Connection::connector(uri, connector, ConnectionProperties::default()),
    )
    .await
    .map_err(AmqpError::Connect)?;
    Ok(Session {
        connection,
        transport,
    })
}

/// The AMQP connection the workers share, opened by [`connect`], with a second handle to the
/// socket under it.
///
/// The workers open their channels on [`Session::connection`]. The second handle lets the
/// session end the TCP connection where the client cannot: when a close [`Session::close`]
/// makes fails or goes unanswered, and when the session is dropped without a close the broker
/// answered. The broker then ends its side of the connection and requeues every delivery the
/// connection left unsettled, even while it keeps sending heartbeats and withholds every answer.
pub struct Session {
    connection: Connection,
    /// Declared after `connection`, so that it is dropped after it, and shuts the socket down
    /// when it is, unless a close the broker answered released it.
    transport: Transport,
}

impl Session {
    /// The AMQP connection, on which the workers open their channels.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Closes the connection with a normal reply code and `reason`, waiting at most
    /// [`OPERATION_DEADLINE`] for the broker's `connection.close-ok`. When the close fails or
    /// the deadline passes, the socket is shut down; the failure is only traced, with its
    /// [`Fault`], because the caller drops the session afterwards.
    ///
    /// - connected: the close above. The client marks every channel of the connection closing
    ///   before it sends the close, so no request is taken or settled on the connection
    ///   afterwards. Once the broker answers, the client closes the socket itself, and neither
    ///   this method nor the session's drop shuts it down: that would only race the client's
    ///   last read and make it report a failed connection.
    /// - closing: a close is already in progress, the broker's or one this method makes for
    ///   another worker, which ends by its own deadline, so this call returns at once. The
    ///   session's drop shuts the socket down unless that close was answered.
    /// - closed or failed: the client is done with the socket, which the second handle still
    ///   holds open, so it is shut down at once.
    ///
    /// Once the socket is shut down, or the close answered, the broker has ended the connection
    /// and requeued every delivery it left unsettled.
    pub async fn close(&self, reason: &str) {
        self.close_within(reason, OPERATION_DEADLINE).await;
    }

    /// [`Session::close`] with `deadline` in place of [`OPERATION_DEADLINE`].
    async fn close_within(&self, reason: &str, deadline: Duration) {
        let status = self.connection.status();
        if status.closing() {
            return;
        }
        if status.connected() {
            match within(deadline, self.connection.close(CLOSE_NORMAL, reason)).await {
                Ok(()) => {
                    self.transport.release();
                    return;
                }
                Err(fault) => tracing::warn!(
                    reason,
                    error = %fault,
                    "closing the AMQP connection failed; its socket is shut down"
                ),
            }
        }
        self.transport.shut_down();
    }
}

impl fmt::Debug for Session {
    /// The connection state only: the client's own `Debug` of a connection shows the user name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("state", &self.connection.status().state())
            .finish_non_exhaustive()
    }
}

/// The second handle to the socket of one connection, shared with the connector that opens the
/// socket. Shutting it down, or dropping it, ends the TCP connection, unless it was released
/// after a close the broker answered.
struct Transport {
    /// Set once: `Some` by the connector with its socket's second handle, or `None` by
    /// [`Transport::shut_down`] when the connection was given up before the connector got that
    /// far, which makes the connector shut its socket down and hand the client none.
    socket: Arc<OnceLock<Option<Socket>>>,
    /// Set by [`Transport::release`] once the broker has answered the connection's close: the
    /// client then closes its own handle, and the second handle only goes with the drop.
    released: AtomicBool,
}

impl Transport {
    fn new() -> Self {
        Self {
            socket: Arc::new(OnceLock::new()),
            released: AtomicBool::new(false),
        }
    }

    /// Leaves the socket to the client, which closes it after the close the broker answered:
    /// [`Transport::shut_down`] and the drop no longer shut it down.
    fn release(&self) {
        self.released.store(true, Ordering::Release);
    }

    /// The connector for `Connection::connector`: [`open_socket`] within what is left of
    /// `deadline` since `started`, recording the second handle here.
    fn connector(
        &self,
        started: Instant,
        deadline: Duration,
    ) -> Box<dyn FnOnce(&AMQPUri) -> HandshakeResult + Send + Sync> {
        let slot = Arc::clone(&self.socket);
        Box::new(move |uri: &AMQPUri| open_socket(uri, started, deadline, &slot))
    }

    /// Shuts the socket down, or, when the connector has not recorded one yet, makes it refuse
    /// the socket it opens; nothing once [`Transport::release`] was called. Calling it again
    /// shuts the same socket down again, which is harmless.
    fn shut_down(&self) {
        if self.released.load(Ordering::Acquire) || self.socket.set(None).is_ok() {
            return;
        }
        if let Some(Some(socket)) = self.socket.get() {
            shut_down_socket(socket);
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Opens the TCP connection to the broker as the client's default connector,
/// `lapin::tcp::AMQPUriTcpExt::connect`, does: the URL's `host:port` resolved and connected,
/// `TCP_NODELAY`, TLS with the default configuration for `amqps`, then non-blocking mode. A
/// second handle to the socket is recorded in `slot` before TLS starts, so that a deadline that
/// passes during the TLS or AMQP handshake can still end the connection; when `slot` already
/// holds `None`, the connection was given up, and the socket is shut down instead of handed to
/// the client.
///
/// The connect itself is bounded more tightly than by the default connector, which bounds only
/// the first resolved address, and only by the URL's `connection_timeout`: see
/// [`connect_socket`]. The client runs this on a blocking thread of its own.
fn open_socket(
    uri: &AMQPUri,
    started: Instant,
    deadline: Duration,
    slot: &OnceLock<Option<Socket>>,
) -> HandshakeResult {
    let socket = connect_socket(uri, started, deadline)?;
    socket.set_nodelay(true)?;
    if slot.set(Some(socket.try_clone()?)).is_err() {
        shut_down_socket(&socket);
        return Err(io::Error::from(io::ErrorKind::ConnectionAborted).into());
    }
    let stream = TcpStream::from_std(socket)?;
    let stream = match uri.scheme {
        AMQPScheme::AMQP => stream,
        AMQPScheme::AMQPS => stream.into_tls(&uri.authority.host, TLSConfig::default())?,
    };
    stream.set_nonblocking(true)?;
    Ok(stream)
}

/// Connects to the first address of the URL's `host:port` that accepts within what is left of
/// `deadline` since `started`, each attempt also bounded by the URL's `connection_timeout` in
/// milliseconds when it sets one above 0.
///
/// # Errors
///
/// The error of the last address tried; [`io::ErrorKind::TimedOut`] when no time is left for
/// the next address; [`io::ErrorKind::AddrNotAvailable`] when the name resolves to no address.
fn connect_socket(uri: &AMQPUri, started: Instant, deadline: Duration) -> io::Result<Socket> {
    let address = format!("{}:{}", uri.authority.host, uri.authority.port);
    let mut failure = io::Error::from(io::ErrorKind::AddrNotAvailable);
    for address in address.to_socket_addrs()? {
        let left = deadline.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let wait = uri
            .query
            .connection_timeout
            .filter(|timeout| *timeout > 0)
            .map_or(left, |timeout| left.min(Duration::from_millis(timeout)));
        match Socket::connect_timeout(&address, wait) {
            Ok(socket) => return Ok(socket),
            Err(error) => failure = error,
        }
    }
    Err(failure)
}

/// Shuts `socket` down in both directions, which sends the peer the end of the stream at once,
/// whatever other handle still holds the socket. A socket no longer connected
/// ([`io::ErrorKind::NotConnected`]) needs nothing more; any other failure is traced by its
/// [`io::ErrorKind`] only.
fn shut_down_socket(socket: &Socket) {
    if let Err(error) = socket.shutdown(Shutdown::Both) {
        let kind = error.kind();
        if kind != io::ErrorKind::NotConnected {
            tracing::warn!(?kind, "shutting down the AMQP socket failed");
        }
    }
}

/// Parses the AMQP URL, dropping the parser's message, which can repeat the URL.
fn parse_url(amqp_url: &str) -> Result<AMQPUri, AmqpError> {
    amqp_url
        .parse::<AMQPUri>()
        .map_err(|_| AmqpError::InvalidUrl)
}

/// Checks, in order, that each of `queues` exists, with a passive declare of its name that
/// creates nothing.
///
/// A missing queue makes the broker close `channel` (AMQP `NOT_FOUND`, 404), so the first
/// failure ends the check. At startup it is fatal: the queue set of
/// `rabbitmq/definitions.json` is missing or a request queue name in the configuration is wrong.
/// Each declare must be answered within [`OPERATION_DEADLINE`].
///
/// # Errors
///
/// [`AmqpError::Declare`] naming the first queue whose check failed, by its [`QueueLabel`].
pub async fn declare_passive(
    channel: &Channel,
    queues: &[QueueCheck<'_>],
) -> Result<(), AmqpError> {
    for queue in queues {
        let declare = channel.queue_declare(
            queue.name,
            QueueDeclareOptions {
                passive: true,
                durable: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        );
        within(OPERATION_DEADLINE, declare)
            .await
            .map_err(|source| AmqpError::Declare {
                queue: queue.label(),
                source,
            })?;
    }
    Ok(())
}

/// Closes `channel` with a normal reply code if it is still open, waiting at most
/// [`OPERATION_DEADLINE`] for the broker's `channel.close-ok`. A failure or a missed deadline is
/// only traced, with its [`Fault`]: the channel is being discarded either way, and the client
/// already counts it as closing, so no request is taken or settled on it afterwards.
async fn close_quietly(channel: &Channel, reason: &str) {
    if !channel.status().connected() {
        return;
    }
    if let Err(fault) = within(OPERATION_DEADLINE, channel.close(CLOSE_NORMAL, reason)).await {
        tracing::warn!(reason, error = %fault, "closing an AMQP channel failed");
    }
}

/// The consumer channel of one worker: it takes requests with `basic_get` and holds their
/// delivery tags until they are acknowledged or rejected (COPAUA0C-001).
#[derive(Clone, Debug)]
pub struct Consumer {
    channel: Channel,
}

impl Consumer {
    /// Opens a consumer channel on `connection`, within [`OPERATION_DEADLINE`].
    ///
    /// # Errors
    ///
    /// [`AmqpError::Channel`] when the channel cannot be opened, for example because the
    /// connection is gone or the broker does not answer in time.
    pub async fn open(connection: &Connection) -> Result<Self, AmqpError> {
        let channel = within(OPERATION_DEADLINE, connection.create_channel())
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
    /// The broker must answer within [`OPERATION_DEADLINE`]. A get that times out may still be
    /// answered with a delivery the worker never sees; [`Fault::TimedOut`] counts as a lost
    /// connection, so the session's connection is closed and the broker requeues that delivery.
    ///
    /// # Errors
    ///
    /// [`AmqpError::Get`] when the get fails, for example because the channel or the connection
    /// is gone or the broker does not answer in time.
    pub async fn get(&self, queue: &str) -> Result<Option<Request>, AmqpError> {
        let get = self
            .channel
            .basic_get(queue, BasicGetOptions { no_ack: false });
        let message = within(OPERATION_DEADLINE, get)
            .await
            .map_err(AmqpError::Get)?;
        Ok(message.map(|BasicGetMessage { delivery, .. }| Request::from_delivery(delivery)))
    }

    /// Closes the channel if it is still open, within [`OPERATION_DEADLINE`]; a failure or a
    /// missed deadline is only traced.
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
    /// Opens a publisher channel on `connection` and puts it in confirm mode, once; each step
    /// within [`OPERATION_DEADLINE`].
    ///
    /// # Errors
    ///
    /// [`AmqpError::Channel`] when the channel cannot be opened or confirm mode is refused or not
    /// answered in time; a channel opened before the refusal is closed again.
    pub async fn open(connection: &Connection) -> Result<Self, AmqpError> {
        let channel = within(OPERATION_DEADLINE, connection.create_channel())
            .await
            .map_err(AmqpError::Channel)?;
        let confirm_select = channel.confirm_select(ConfirmSelectOptions::default());
        if let Err(fault) = within(OPERATION_DEADLINE, confirm_select).await {
            close_quietly(&channel, PUBLISHER_CLOSE_TEXT).await;
            return Err(AmqpError::Channel(fault));
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
    /// worker acknowledge the request it answers. The publication and its confirmation must
    /// both finish within [`PUBLISH_DEADLINE`]; past it the wait is abandoned and the
    /// publication counts as failed, never as confirmed.
    ///
    /// # Errors
    ///
    /// The [`PublishFailure`] that kept the message from being confirmed (CODATE01-014,
    /// COACCT01-033, COPAUA0C-117), [`PublishFailure::Channel`] with [`Fault::TimedOut`] when
    /// the deadline passed.
    pub async fn publish(
        &self,
        queue: &str,
        body: &[u8],
        properties: BasicProperties,
    ) -> Result<(), PublishFailure> {
        let publication = async {
            self.channel
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
                .await?
                .await
        };
        match within(PUBLISH_DEADLINE, publication)
            .await
            .map_err(PublishFailure::Channel)?
        {
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
    /// `basic_ack` on the consumer channel, handed to the socket within [`OPERATION_DEADLINE`].
    /// A missed deadline is [`AmqpError::Ack`] with [`Fault::TimedOut`], and the worker closes
    /// the channel so that the broker requeues the request.
    async fn ack(&self, delivery_tag: u64) -> Result<(), AmqpError> {
        let ack = self
            .channel
            .basic_ack(delivery_tag, BasicAckOptions::default());
        within(OPERATION_DEADLINE, ack)
            .await
            .map_err(AmqpError::Ack)
    }

    /// `basic_reject` with `requeue: false` on the consumer channel, handed to the socket within
    /// [`OPERATION_DEADLINE`]. A missed deadline is [`AmqpError::Reject`] with
    /// [`Fault::TimedOut`], and the worker closes the channel so that the broker requeues the
    /// request.
    async fn reject_without_requeue(&self, delivery_tag: u64) -> Result<(), AmqpError> {
        let reject = self
            .channel
            .basic_reject(delivery_tag, BasicRejectOptions { requeue: false });
        within(OPERATION_DEADLINE, reject)
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
/// consumer is used. The interval is at most five seconds, because the configuration refuses a
/// longer one.
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
        AmqpError, BACKOFF_CAP, Backoff, CONNECT_DEADLINE, CP00_REPLY_EXPIRATION,
        DEAD_LETTER_QUEUE, Fault, OPERATION_DEADLINE, PUBLISH_DEADLINE, PublishFailure, QueueCheck,
        QueueLabel, Request, copied_properties, cp00_properties, parse_url, within,
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
        let timed_out = Fault::TimedOut {
            after: OPERATION_DEADLINE,
        };
        let lost = [
            AmqpError::Connect(Fault::from(&io)),
            AmqpError::Connect(Fault::TimedOut {
                after: CONNECT_DEADLINE,
            }),
            AmqpError::Get(Fault::from(io)),
            AmqpError::Get(timed_out.clone()),
            AmqpError::Ack(timed_out.clone()),
            AmqpError::Reject(timed_out.clone()),
            AmqpError::Channel(timed_out.clone()),
            AmqpError::Declare {
                queue: QueueLabel::Literal("CARD.DEMO.ERROR"),
                source: timed_out,
            },
            AmqpError::Ack(Fault::from(lapin::Error::MissingHeartbeatError)),
            AmqpError::Channel(Fault::from(lapin::Error::InvalidConnectionState(
                ConnectionState::Closed,
            ))),
            AmqpError::Reject(Fault::from(forced)),
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
                queue: QueueLabel::Configured {
                    variable: "CARDDEMO_QUEUE_REQUEST_DATE",
                },
                source: Fault::from(not_found),
            },
            AmqpError::Get(Fault::from(lapin::Error::InvalidChannelState(
                ChannelState::Closed,
            ))),
            AmqpError::Ack(Fault::from(lapin::Error::InvalidChannelState(
                ChannelState::Error,
            ))),
            AmqpError::Channel(Fault::from(lapin::Error::InvalidConnectionState(
                ConnectionState::Connected,
            ))),
            AmqpError::Channel(Fault::from(lapin::Error::ChannelsLimitReached)),
        ];
        for error in &channel_only {
            assert!(!error.is_connection_lost(), "{error}");
        }
    }

    #[test]
    fn error_texts_name_the_queue_and_keep_only_the_fault_as_source() {
        let closed = Fault::from(lapin::Error::InvalidChannelState(ChannelState::Closed));
        let not_found = Fault::Protocol { code: 404 };
        let cases = [
            (
                QueueCheck::configured("CARDDEMO_QUEUE_REQUEST_PAUTH", "CARD.DEMO.REQUEST.PAUTH"),
                closed.clone(),
                "passive declare of the queue named by CARDDEMO_QUEUE_REQUEST_PAUTH failed: \
                 invalid channel state Closed",
            ),
            (
                QueueCheck::configured("CARDDEMO_QUEUE_REQUEST_PAUTH", "CARD.DEMO.REQUEST.PAUTH"),
                not_found.clone(),
                "passive declare of the queue named by CARDDEMO_QUEUE_REQUEST_PAUTH failed: \
                 AMQP soft error NOT-FOUND (reply code 404)",
            ),
            (
                QueueCheck::literal(DEAD_LETTER_QUEUE),
                not_found,
                "passive declare of queue CARD.DEMO.DEADLETTER.PAUTH failed: \
                 AMQP soft error NOT-FOUND (reply code 404)",
            ),
            (
                QueueCheck::literal("CARD.DEMO.ERROR"),
                closed,
                "passive declare of queue CARD.DEMO.ERROR failed: invalid channel state Closed",
            ),
        ];
        for (check, source, shown) in cases {
            let declare = AmqpError::Declare {
                queue: check.label(),
                source: source.clone(),
            };
            assert_eq!(declare.to_string(), shown);
            assert!(
                declare
                    .source()
                    .and_then(|fault| fault.downcast_ref::<Fault>())
                    .is_some_and(|fault| *fault == source)
            );
        }
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
        let timed_out = PublishFailure::Channel(Fault::TimedOut {
            after: PUBLISH_DEADLINE,
        });
        assert_eq!(
            timed_out.to_string(),
            "publication failed on the publisher channel: no response within the 5s deadline"
        );
        assert!(
            timed_out
                .source()
                .and_then(|source| source.downcast_ref::<Fault>())
                .is_some_and(Fault::is_connection_lost)
        );
    }

    #[test]
    fn a_protocol_fault_keeps_the_reply_code_and_its_class_only() {
        let cases = [
            (AMQPErrorKind::Soft(AMQPSoftError::NOTFOUND), 404, false),
            (
                AMQPErrorKind::Soft(AMQPSoftError::ACCESSREFUSED),
                403,
                false,
            ),
            (
                AMQPErrorKind::Hard(AMQPHardError::CONNECTIONFORCED),
                320,
                true,
            ),
            (AMQPErrorKind::Hard(AMQPHardError::FRAMEERROR), 501, true),
        ];
        for (kind, code, lost) in cases {
            let fault = Fault::from(lapin::Error::ProtocolError(AMQPError::new(
                kind,
                ShortString::from("reply text"),
            )));
            assert_eq!(fault, Fault::Protocol { code });
            assert_eq!(fault.is_connection_lost(), lost, "{fault}");
            let shown = format!("{fault} {fault:?}");
            assert!(shown.contains(&code.to_string()), "{shown}");
            assert!(!shown.contains("reply text"), "{shown}");
        }
        let unknown = Fault::Protocol { code: 999 };
        assert_eq!(unknown.to_string(), "AMQP error with reply code 999");
        assert!(!unknown.is_connection_lost());
    }

    #[test]
    fn a_configured_queue_shows_as_its_variable_and_never_as_its_name() {
        // Configured names holding protected values, assembled at run time: a password-like
        // word of letters only, a test card number and a test SSN.
        let word = ["ONLY", "LETR"].concat();
        let card = format!("4111{}", "1".repeat(12));
        let ssn = ["123", "45", "6789"].join("-");
        let names = [
            format!("QA.{word}"),
            word.clone(),
            format!("QA.{card}"),
            CARD_NUMBER.to_owned(),
            format!("SSN {ssn} Q"),
            format!("QA.{}", "Q".repeat(300)),
        ];
        for name in &names {
            let check = QueueCheck::configured("CARDDEMO_QUEUE_REQUEST_PAUTH", name);
            assert_eq!(
                check.name,
                name.as_str(),
                "the broker is sent the name itself"
            );
            let label = check.label();
            assert_eq!(
                label,
                QueueLabel::Configured {
                    variable: "CARDDEMO_QUEUE_REQUEST_PAUTH"
                }
            );
            let declare = AmqpError::Declare {
                queue: label,
                source: Fault::Protocol { code: 404 },
            };
            let mut shown = vec![
                format!("{label} | {label:?} | {label:#?}"),
                format!("{check:?} | {check:#?}"),
                format!("{declare} | {declare:?} | {declare:#?}"),
            ];
            for text in &shown {
                assert!(text.contains("CARDDEMO_QUEUE_REQUEST_PAUTH"), "{text}");
            }
            let mut current = declare.source();
            while let Some(error) = current {
                shown.push(format!("{error} | {error:?} | {error:#?}"));
                current = error.source();
            }
            for text in &shown {
                let protected: [&str; 5] = [name, &word, &card, CARD_NUMBER, &ssn];
                for value in protected {
                    assert!(!text.contains(value), "{value} shown in: {text}");
                }
            }
        }
    }

    #[test]
    fn a_literal_queue_shows_as_its_name() {
        for name in [
            DEAD_LETTER_QUEUE,
            "CARD.DEMO.REPLY.DATE",
            "CARD.DEMO.REPLY.ACCT",
            "CARD.DEMO.ERROR",
        ] {
            let check = QueueCheck::literal(name);
            assert_eq!(check.name, name);
            assert_eq!(check.label(), QueueLabel::Literal(name));
            assert_eq!(check.label().to_string(), format!("queue {name}"));
            assert_eq!(
                format!("{check:?}"),
                format!("QueueCheck {{ label: Literal({name:?}), .. }}")
            );
        }
    }

    #[tokio::test]
    async fn a_wait_ends_by_its_deadline_and_keeps_only_the_fault() {
        let deadline = Duration::from_millis(20);
        let pending = within(deadline, std::future::pending::<lapin::Result<()>>()).await;
        assert_eq!(pending, Err(Fault::TimedOut { after: deadline }));

        let ready = within(deadline, async { lapin::Result::Ok(7) }).await;
        assert_eq!(ready, Ok(7));

        let failed = within(deadline, async {
            lapin::Result::<()>::Err(lapin::Error::IOError(Arc::new(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("os text naming {CARD_NUMBER}"),
            ))))
        })
        .await;
        assert_eq!(failed, Err(Fault::Io(io::ErrorKind::ConnectionRefused)));
    }

    #[test]
    fn the_publish_deadline_is_the_cp00_reply_expiry() {
        assert_eq!(
            PUBLISH_DEADLINE.as_millis().to_string(),
            CP00_REPLY_EXPIRATION
        );
    }

    #[test]
    fn the_dead_letter_queue_is_the_one_the_definitions_route_to() {
        let definitions = include_str!("../../../rabbitmq/definitions.json");
        let routing = format!("\"x-dead-letter-routing-key\": \"{DEAD_LETTER_QUEUE}\"");
        assert!(definitions.contains(&routing));
        assert!(definitions.contains(&format!("\"name\": \"{DEAD_LETTER_QUEUE}\"")));
    }

    /// The deadline the session tests give the client in place of the production ones.
    const TEST_DEADLINE: Duration = Duration::from_millis(300);

    /// How soon after the client gives a connection up its scripted broker must see the TCP
    /// connection end.
    const END_WITHIN: Duration = Duration::from_secs(2);

    /// How long a scripted broker waits for the client's side to end before it gives up, so that
    /// a socket left open fails the test instead of hanging it.
    const PEER_PATIENCE: Duration = Duration::from_secs(10);

    /// How long a scripted broker waits for client bytes before it looks again, sending a
    /// heartbeat first once the handshake is complete.
    const PEER_TICK: Duration = Duration::from_millis(100);

    /// The AMQP 0-9-1 protocol header, the first bytes a client sends.
    const PROTOCOL_HEADER: [u8; 8] = *b"AMQP\x00\x00\x09\x01";

    /// A heartbeat frame: type 8, channel 0, an empty payload and the frame end.
    const HEARTBEAT: [u8; 8] = [8, 0, 0, 0, 0, 0, 0, 0xCE];

    /// What a scripted broker does once it accepts the client's socket.
    #[derive(Clone, Copy, Debug)]
    enum Script {
        /// Reads and never answers, not even the protocol header, so the handshake never
        /// finishes.
        Silent,
        /// Completes the handshake (`connection.start`, `connection.tune`,
        /// `connection.open-ok`), then sends heartbeats and never answers `connection.close`.
        WithholdCloseOk,
        /// Completes the handshake as [`Script::WithholdCloseOk`] does, then answers
        /// `connection.close` with `connection.close-ok`.
        AnswerCloseOk,
    }

    /// What a scripted broker saw before the client's side of the TCP connection ended.
    #[derive(Debug)]
    struct PeerReport {
        /// Whether the client sent `connection.close` first.
        close_requested: bool,
        /// When the broker saw the end: the end of the stream, a reset or a broken pipe.
        ended_at: std::time::Instant,
    }

    /// Starts a scripted broker for one connection on a free local port: the AMQP URL that
    /// reaches it, which names no credentials, and the receiver of its [`PeerReport`]. The
    /// receiver disconnects without a report when the client's side stays open for
    /// [`PEER_PATIENCE`].
    fn scripted_broker(
        script: Script,
    ) -> io::Result<(String, std::sync::mpsc::Receiver<PeerReport>)> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("amqp://{}/%2f", listener.local_addr()?);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(report) = play(&listener, script) {
                // The test may have given up already; nothing waits for the report then.
                let _delivered = sender.send(report);
            }
        });
        Ok((url, receiver))
    }

    /// Accepts one connection on `listener` and plays `script` until the client's side ends.
    fn play(listener: &std::net::TcpListener, script: Script) -> io::Result<PeerReport> {
        use std::io::{Read, Write};

        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(PEER_TICK))?;
        let accepted = std::time::Instant::now();
        let ended = |close_requested| PeerReport {
            close_requested,
            ended_at: std::time::Instant::now(),
        };
        let mut received = Vec::new();
        let mut chunk = [0_u8; 4096];
        let mut header_read = false;
        let mut open = false;
        let mut close_requested = false;
        while accepted.elapsed() < PEER_PATIENCE {
            match socket.read(&mut chunk) {
                Ok(0) => return Ok(ended(close_requested)),
                Ok(count) => received.extend_from_slice(chunk.get(..count).unwrap_or_default()),
                Err(error) if is_connection_end(&error) => return Ok(ended(close_requested)),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    if open {
                        match socket.write_all(&HEARTBEAT) {
                            Ok(()) => {}
                            Err(error) if is_connection_end(&error) => {
                                return Ok(ended(close_requested));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    continue;
                }
                Err(error) => return Err(error),
            }
            if matches!(script, Script::Silent) {
                continue;
            }
            if !header_read {
                let Some(header) = received.get(..PROTOCOL_HEADER.len()) else {
                    continue;
                };
                if header != PROTOCOL_HEADER {
                    return Err(io::Error::from(io::ErrorKind::InvalidData));
                }
                received.drain(..PROTOCOL_HEADER.len());
                header_read = true;
                socket.write_all(&connection_start())?;
            }
            while let Some((method, length)) = next_frame(&received) {
                received.drain(..length);
                match method {
                    // connection.start-ok
                    Some((10, 11)) => socket.write_all(&connection_tune())?,
                    // connection.open: answered with connection.open-ok
                    Some((10, 40)) => {
                        socket.write_all(&method_frame(10, 41, &[0]))?;
                        open = true;
                    }
                    // connection.close: answered with connection.close-ok by one script only
                    Some((10, 50)) => {
                        close_requested = true;
                        if matches!(script, Script::AnswerCloseOk) {
                            socket.write_all(&method_frame(10, 51, &[]))?;
                            // Nothing follows a close-ok; the broker waits for the client's end.
                            open = false;
                        }
                    }
                    _ => {}
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::TimedOut))
    }

    /// Whether `error` shows the peer's side of the TCP connection gone.
    fn is_connection_end(error: &io::Error) -> bool {
        matches!(
            error.kind(),
            io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
        )
    }

    /// The first whole frame of `received`: the class and method ids of a method frame, `None`
    /// for any other frame, and the frame's length; `None` until a whole frame has arrived.
    fn next_frame(received: &[u8]) -> Option<(Option<(u16, u16)>, usize)> {
        let header = received.get(..7)?;
        let size = u32::from_be_bytes(header.get(3..7)?.try_into().ok()?);
        // TRUNCATION: checked, never lossy. Every target this workspace builds for has a `usize`
        // of at least 32 bits, so `size` converts exactly; a refused conversion or an overflowing
        // add yields `None`, no whole frame, and never a wrapped length.
        let length = usize::try_from(size).ok()?.checked_add(8)?;
        let frame = received.get(..length)?;
        let method = if header.first() == Some(&1) {
            let class = u16::from_be_bytes(frame.get(7..9)?.try_into().ok()?);
            let method = u16::from_be_bytes(frame.get(9..11)?.try_into().ok()?);
            Some((class, method))
        } else {
            None
        };
        Some((method, length))
    }

    /// A method frame on channel 0: type 1, the payload size, the class and method ids, the
    /// arguments and the frame end.
    fn method_frame(class: u16, method: u16, arguments: &[u8]) -> Vec<u8> {
        // TRUNCATION: checked, never lossy. Callers pass fixed handshake arguments of at most 24
        // bytes, so the size is at most 28 and `u32` holds it exactly; the unreachable fallback
        // saturates, never wraps, announcing a frame far past any frame size the client accepts.
        let size = u32::try_from(arguments.len().saturating_add(4)).unwrap_or(u32::MAX);
        let mut frame = vec![1, 0, 0];
        frame.extend_from_slice(&size.to_be_bytes());
        frame.extend_from_slice(&class.to_be_bytes());
        frame.extend_from_slice(&method.to_be_bytes());
        frame.extend_from_slice(arguments);
        frame.push(0xCE);
        frame
    }

    /// An AMQP long string: the length as a big-endian `u32`, then the bytes.
    fn long_string(text: &str) -> Vec<u8> {
        // TRUNCATION: checked, never lossy. The only texts are the fixed mechanism `PLAIN` and
        // locale `en_US`, five bytes each, so `u32` holds the length exactly; the unreachable
        // fallback saturates and never wraps, so a mis-sized string fails the handshake visibly.
        let length = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let mut encoded = length.to_be_bytes().to_vec();
        encoded.extend_from_slice(text.as_bytes());
        encoded
    }

    /// `connection.start`: version 0-9, no server properties, mechanism `PLAIN`, locale `en_US`.
    fn connection_start() -> Vec<u8> {
        let mut arguments = vec![0, 9];
        arguments.extend_from_slice(&0_u32.to_be_bytes());
        arguments.extend(long_string("PLAIN"));
        arguments.extend(long_string("en_US"));
        method_frame(10, 10, &arguments)
    }

    /// `connection.tune`: 2047 channels, 128 KiB frames and a one-second heartbeat, which the
    /// broker's heartbeats every [`PEER_TICK`] of client silence keep satisfied.
    fn connection_tune() -> Vec<u8> {
        let mut arguments = 2047_u16.to_be_bytes().to_vec();
        arguments.extend_from_slice(&131_072_u32.to_be_bytes());
        arguments.extend_from_slice(&1_u16.to_be_bytes());
        method_frame(10, 30, &arguments)
    }

    /// Asserts that the scripted broker behind `report` saw the TCP connection end within
    /// [`END_WITHIN`] of `given_up`, and returns what it saw.
    fn assert_connection_ended(
        report: &std::sync::mpsc::Receiver<PeerReport>,
        given_up: std::time::Instant,
    ) -> Option<PeerReport> {
        let report = report
            .recv_timeout(END_WITHIN.saturating_add(PEER_TICK))
            .ok();
        assert!(
            report.as_ref().is_some_and(|report| {
                report.ended_at.saturating_duration_since(given_up) <= END_WITHIN
            }),
            "the broker saw the TCP connection end late or never: {report:?}"
        );
        report
    }

    #[tokio::test]
    async fn a_handshake_given_up_ends_its_tcp_connection() -> io::Result<()> {
        use super::connect_within;

        // Given up once by the connect deadline, and once by the caller, which drops the
        // unfinished attempt as a stop does.
        for dropped_by_caller in [false, true] {
            let (url, report) = scripted_broker(Script::Silent)?;
            let started = std::time::Instant::now();
            let outcome = if dropped_by_caller {
                tokio::time::timeout(TEST_DEADLINE, connect_within(&url, CONNECT_DEADLINE))
                    .await
                    .ok()
            } else {
                Some(connect_within(&url, TEST_DEADLINE).await)
            };
            let given_up = std::time::Instant::now();
            assert!(
                match &outcome {
                    None => dropped_by_caller,
                    Some(Err(AmqpError::Connect(Fault::TimedOut { after }))) => {
                        !dropped_by_caller && *after == TEST_DEADLINE
                    }
                    Some(_) => false,
                },
                "dropped by the caller: {dropped_by_caller}, outcome: {outcome:?}"
            );
            assert!(given_up.duration_since(started) < TEST_DEADLINE.saturating_add(END_WITHIN));
            assert_connection_ended(&report, given_up);
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_session_ends_its_tcp_connection_whether_its_close_is_answered_or_not()
    -> io::Result<()> {
        use super::connect_within;

        // (script, closed): a close the broker never answers; a session dropped without a
        // close; and a close the broker answers, after which the session is dropped.
        let cases = [
            (Script::WithholdCloseOk, true),
            (Script::WithholdCloseOk, false),
            (Script::AnswerCloseOk, true),
        ];
        for (script, closed) in cases {
            let (url, report) = scripted_broker(script)?;
            let session = match connect_within(&url, CONNECT_DEADLINE).await {
                Ok(session) => session,
                Err(error) => panic!("{script:?}: the scripted handshake failed: {error}"),
            };
            assert!(session.connection().status().connected(), "{script:?}");
            let started = std::time::Instant::now();
            if closed {
                session
                    .close_within("carddemo-mq session test", TEST_DEADLINE)
                    .await;
                assert!(
                    started.elapsed() < TEST_DEADLINE.saturating_add(END_WITHIN),
                    "{script:?}: the close outlived its deadline"
                );
            }
            // A close never answered must end the connection by itself, so that session is held
            // until the broker has reported; every other one is dropped first.
            let kept = if closed && matches!(script, Script::WithholdCloseOk) {
                Some(session)
            } else {
                drop(session);
                None
            };
            let given_up = std::time::Instant::now();
            let seen = assert_connection_ended(&report, given_up);
            if closed {
                assert!(
                    seen.is_some_and(|seen| seen.close_requested),
                    "{script:?}: the client sent no connection.close before the socket ended"
                );
            }
            drop(kept);
        }
        Ok(())
    }
}
