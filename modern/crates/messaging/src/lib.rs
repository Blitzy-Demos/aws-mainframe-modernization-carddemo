//! The `RabbitMQ` workers that replace the three MQ-triggered CICS programs of `CardDemo` (AAP
//! 0.3.4.2), and [`run`], the entry point of the `carddemo-mq` binary and of the tests that run
//! the workers in process.
//!
//! | Flow | COBOL program | Request queue (configuration) | Reply queue | Error destination |
//! |---|---|---|---|---|
//! | CDRD | `CODATE01` | `CARDDEMO_QUEUE_REQUEST_DATE` | `CARD.DEMO.REPLY.DATE` | `CARD.DEMO.ERROR` |
//! | CDRA | `COACCT01` | `CARDDEMO_QUEUE_REQUEST_ACCT` | `CARD.DEMO.REPLY.ACCT` | `CARD.DEMO.ERROR` |
//! | CP00 | `COPAUA0C` | `CARDDEMO_QUEUE_REQUEST_PAUTH` | the request's `reply_to` | the `M004` row of `cssl_error_log`; `CARD.DEMO.DEADLETTER.PAUTH` |
//!
//! Request queues come only from `runtime::config::Config`, because the COBOL takes them from
//! its trigger message. Reply and error queue names are the COBOL literals
//! (`CODATE01.cbl:147,243`, `COACCT01.cbl:198,294`). The README queue names (E01) appear in no
//! source and are not used.
//!
//! # Modules
//!
//! - [`broker`]: the AMQP rules the three workers share: the connection, the consumer and
//!   publisher channels, the passive queue checks, the `basic_get` poll, the confirmed mandatory
//!   publish, the reply properties, acknowledgement and rejection, and the reconnect backoff.
//! - [`workers`]: the poll loop and one module per flow, [`workers::cdrd`], [`workers::cdra`]
//!   and [`workers::cp00`].
//! - [`error_body`]: the `MQ-ERR-DISPLAY` body that CDRD and CDRA publish to `CARD.DEMO.ERROR`.
//! - [`ids`]: the new `message_id` of every CP00 reply.
//! - `EventSite`, `event_metadata`, `event_metadata_with_target` and `emit`, crate-private at
//!   the crate root: the events of every module, each written out as a static callsite and
//!   emitted without a `tracing` event macro, whose expansion holds a lint suppression.
//!
//! # Layering
//!
//! This crate does broker I/O, transactions and orchestration only.
//!
//! - Every reply body, error body, decision, summary and detail record and log record comes
//!   from `domain::messaging::{codate01, coacct01, copaua0c}` and `domain::records`.
//! - Every SQL statement runs through the `persistence` repository traits on `PgStore` or
//!   `PgTx`. The crate depends on no database driver and never builds a query.
//! - Configuration, telemetry and the database pool come from `runtime`.
//!
//! # One request, one task
//!
//! Each request is processed as the first message of a CICS task, the only case the source
//! determines, and no worker carries state from one request to the next (E14, E15;
//! COPAUA0C-130, CODATE01-013, COACCT01-027). Output that depends on several messages of one
//! task is `HALTED(H1)` (COPAUA0C-131). The 501-message task limit of `COPAUA0C` has no
//! counterpart: a worker takes requests until it is stopped, and a lost connection only pauses
//! it until [`run`] reconnects.
//!
//! # Stopping
//!
//! [`run`] stops when its [`Stop`] is requested, which the `carddemo-mq` binary does on SIGTERM
//! or SIGINT. A worker sees the request only between requests: before its next `basic_get`,
//! during an idle poll and during a backoff wait. A request it already holds is finished
//! through its own path, reply, acknowledgement and writes included, so a stop never cuts a CP00
//! authorization between its acknowledgement and its commit. Every broker wait on that path is
//! bounded (see [`broker`]), so the stop completes in bounded time. `run` then closes the
//! connection and returns `Ok`. A process killed without a stop keeps the E29 outcome: the open
//! transaction is discarded with its connection, and the unacknowledged request is redelivered.
//!
//! # Serialisation of authorization decisions
//!
//! CP00 decisions for one account are serialised only by the `PostgreSQL` row lock on that
//! account's `pending_auth_summaries` row, held from before the decision to the commit (i10).
//! No in-process lock guards authorization state, because a second `carddemo-mq` process would
//! not see it: two processes and redelivered requests all decide against committed state.
//!
//! # Schema
//!
//! The workers never change the schema. Started against an unmigrated database they fail at
//! their first query; only the `carddemo-migrate` binary applies the migrations (Single Schema
//! Executor).
//!
//! # Redaction
//!
//! No event, span or error of this crate holds a message body, a card number, the AMQP URL or
//! the database URL. The URLs carry the local passwords and are only ever handed to
//! [`broker::connect`] and `runtime::db::connect`.

pub mod broker;
pub mod error_body;
pub mod ids;
pub mod workers;

use std::error::Error;
use std::fmt;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use domain::Clock;
use lapin::Connection;
use persistence::PgStore;
use runtime::config::Config;
use runtime::telemetry::event_enabled;
use tokio::sync::watch;
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{self, FieldSet, Value};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use crate::broker::{AmqpError, Backoff, Session};
use crate::workers::{Step, cdra, cdrd, cp00};

/// The reply text of the connection close with which the first worker to stop ends a session.
const SESSION_CLOSE_TEXT: &str = "carddemo-mq worker stopped; reconnecting";

/// The reply text of the connection close with which [`run`] ends, on a stop or a configuration
/// error.
const STOP_CLOSE_TEXT: &str = "carddemo-mq stopped";

/// The reply text of the connection close of a session whose workers could not all be opened.
const OPEN_FAILED_CLOSE_TEXT: &str = "carddemo-mq workers could not be opened";

/// The sending side of a cooperative stop of [`run`]: [`StopHandle::request`] asks every
/// worker to stop at its next loop boundary. Dropping every handle requests the stop as well, so
/// that no worker is left running with nothing able to stop it.
#[derive(Debug)]
pub struct StopHandle {
    sender: watch::Sender<bool>,
}

impl StopHandle {
    /// A handle whose stop is not yet requested.
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }

    /// A [`Stop`] that observes this handle, for [`run`].
    #[must_use]
    pub fn subscribe(&self) -> Stop {
        Stop {
            receiver: self.sender.subscribe(),
        }
    }

    /// Requests the stop. Requesting it again changes nothing.
    pub fn request(&self) {
        self.sender.send_replace(true);
    }
}

impl Default for StopHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// The receiving side of a cooperative stop: [`run`] and each worker loop hold one. A stop is
/// observed only where waiting can be abandoned without harm, never inside the processing of a
/// request.
#[derive(Debug, Clone)]
pub struct Stop {
    receiver: watch::Receiver<bool>,
}

impl Stop {
    /// Whether the stop is requested: [`StopHandle::request`] was called, or every
    /// [`StopHandle`] is dropped.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        *self.receiver.borrow() || self.receiver.has_changed().is_err()
    }

    /// Waits until the stop is requested.
    pub async fn requested(&mut self) {
        // `Ok` when the stop is sent, `Err` when every handle is dropped: both request it.
        let _requested = self.receiver.wait_for(|requested| *requested).await;
    }

    /// Awaits `wait` unless the stop is requested first: `Some` with its output, or `None` when
    /// the stop came first and `wait` was dropped unfinished. Only waits that may be abandoned go
    /// through it, such as a sleep or a connection attempt; the processing of a request never
    /// does, so a stop never cancels one.
    pub async fn unless_requested<F: Future>(&mut self, wait: F) -> Option<F::Output> {
        if self.is_requested() {
            return None;
        }
        // Both futures are pinned on the heap, the pin that needs no pinning macro, and the stop
        // is polled first at every wake-up, so a stop and an output that are ready together
        // end the wait as the stop.
        let mut requested = Box::pin(self.requested());
        let mut wait = Box::pin(wait);
        poll_fn(|cx| {
            if requested.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            wait.as_mut().poll(cx).map(Some)
        })
        .await
    }
}

/// Why [`run`] stopped. Neither variant's text, `Debug` or source chain holds the AMQP URL, a
/// credential, a message body, a configured queue name or the broker's close text: [`AmqpError`]
/// names only an operation, a variable, a queue by its value-free label (the environment
/// variable that configures a request queue, or the source constant that names any other
/// queue), and the value-free class of the broker or client condition.
///
/// Only a configuration error ([`AmqpError::is_configuration`]) is returned, because no retry
/// cures it. A broker that is down or still booting, refuses the login or misses its handshake
/// deadline, a missing request, reply, error or dead-letter queue, and a refused channel are
/// each logged and retried with the backoff, at startup and after a reconnect alike, since the
/// broker or its operator can put each right while the process waits.
#[derive(Debug, Clone)]
pub enum StartupError {
    /// `CARDDEMO_AMQP_URL` is not a valid AMQP URL ([`AmqpError::InvalidUrl`]), so no
    /// connection can be attempted.
    Connect(AmqpError),
    /// A configured request queue name is refused before its worker opens a channel
    /// ([`AmqpError::QueueName`]): CDRD's or CP00's is longer than the 255-byte AMQP short
    /// string, CDRA's does not fit the 48-byte COBOL field that reports it, or a name holds a
    /// byte outside printable ASCII.
    Declare(AmqpError),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(error) => write!(f, "the first AMQP connection failed: {error}"),
            Self::Declare(error) => {
                write!(
                    f,
                    "the workers cannot be opened on the AMQP connection: {error}"
                )
            }
        }
    }
}

impl Error for StartupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connect(error) | Self::Declare(error) => Some(error),
        }
    }
}

/// Runs the CDRD, CDRA and CP00 workers on one AMQP connection until `stop` is requested or a
/// configuration error, and connects again whenever that connection fails, is lost or cannot
/// open the workers.
///
/// The caller reads `config` with `Config::from_env`, installs `runtime::telemetry`, opens
/// `store` over the pool of `runtime::db::connect`, passes the clock, `SystemClock` in
/// production, and holds the [`StopHandle`] of `stop`; `run` itself touches neither the
/// environment nor the schema. `store` serves CDRA's account reads and every CP00 read, write and
/// log row; `clock` supplies the CDRD date and time and the CP00 keys, log dates and message IDs.
///
/// The first connection is attempted at once. Each session opens the three workers, which check
/// their queues with passive declares and never create one, then drives them concurrently until
/// the connection is lost or the stop is requested. Every worker finishes the request it holds
/// through its own path; none is cancelled mid-message, for example between a CP00
/// acknowledgement and its commit. The broker requeues every request left unsettled.
///
/// A failed connection attempt, the first included, is logged with its [`AmqpError`] text and
/// retried. So is a session whose workers cannot be opened, whether its connection was lost or
/// it is still up while a queue is missing or a channel is refused, at startup or after a
/// reconnect. Such a session, like one whose connection is lost while the workers run, is closed
/// within [`broker::OPERATION_DEADLINE`] and, when the broker does not answer the close in time,
/// has its socket shut down, so that the broker ends the old connection before a new one
/// replaces it ([`Session::close`]). `run` then connects again after the next delay of a
/// [`Backoff`] whose base starts at `config.mq_poll_interval()` and doubles up to
/// [`broker::BACKOFF_CAP`] over consecutive failures, and opens the workers again
/// (COPAUA0C-118). Each delay is drawn between half its base and the whole base, so processes
/// that lost the broker together spread their attempts. The base returns to the poll interval
/// only once the workers have opened.
///
/// The stop is observed during every connection attempt, during every backoff delay and, by
/// each worker, between requests. Once the three workers have ended, the connection is closed
/// within [`broker::OPERATION_DEADLINE`], its socket shut down if the broker does not answer,
/// and `run` returns `Ok`. A stop that comes while no session is open returns `Ok` at once.
///
/// # Errors
///
/// Only configuration errors, which no retry cures:
///
/// - [`StartupError::Connect`] when `CARDDEMO_AMQP_URL` is not a valid AMQP URL.
/// - [`StartupError::Declare`] when a configured request queue name is refused before its worker
///   opens a channel ([`AmqpError::QueueName`]).
///
/// Every other failure, of the broker, its queues or the connection, is logged and retried.
pub async fn run(
    config: &Config,
    store: PgStore,
    clock: Arc<dyn Clock>,
    mut stop: Stop,
) -> Result<(), StartupError> {
    let poll_interval = config.mq_poll_interval();
    let mut backoff = Backoff::new(poll_interval);
    let Some(mut session) = connect_first(&config.amqp_url, &mut backoff, &mut stop).await? else {
        return Ok(());
    };
    loop {
        match Workers::open(&session, config, &store, &clock).await {
            Ok(mut workers) => {
                // The reconnect delay grows only over consecutive failed attempts: a session
                // whose workers opened starts the next outage from the poll interval again.
                backoff.reset();
                workers_consuming(config);
                workers.serve(&session, poll_interval, &stop).await;
                let reason = if stop.is_requested() {
                    STOP_CLOSE_TEXT
                } else {
                    workers_connection_lost();
                    SESSION_CLOSE_TEXT
                };
                // Closed while the workers still hold their channels: the client closes a
                // channel dropped on a live connection in the background, and that close would
                // race this one and fail it.
                session.close(reason).await;
            }
            // A refused request queue name comes from the configuration, and no session cures it.
            Err(OpenFailure { error, .. }) if error.is_configuration() => {
                session.close(STOP_CLOSE_TEXT).await;
                return Err(StartupError::Declare(error));
            }
            Err(OpenFailure {
                error,
                connection_lost: true,
            }) => {
                open_connection_lost(&error);
            }
            // A missing queue (404) or a refused channel on a live connection: the operator can
            // load the definitions or the broker free a channel, so the session is replaced
            // after the backoff, which keeps growing until the workers open. No queue is ever
            // declared here: the queues come only from `rabbitmq/definitions.json`.
            Err(OpenFailure { error, .. }) => {
                open_failed(&error);
            }
        }
        if stop.is_requested() {
            session.close(STOP_CLOSE_TEXT).await;
            return Ok(());
        }
        // Opening the workers can fail on a connection that still reports connected, for example
        // after a missed deadline: it is closed before it is dropped, and its socket shut down
        // if the broker does not answer, so that the broker ends it and requeues its deliveries
        // before a new connection replaces it.
        session.close(SESSION_CLOSE_TEXT).await;
        drop(session);
        let Some(reconnected) = reconnect(&config.amqp_url, &mut backoff, &mut stop).await? else {
            return Ok(());
        };
        session = reconnected;
    }
}

/// Makes the first connection attempt at once: `Some` with its session, or `None` once `stop`
/// is requested during it. A broker that is down, still booting or refusing the login does not
/// end the process: the failure is logged with the [`AmqpError`] text, never the URL, and the
/// connection is retried through [`reconnect`] with `backoff`.
///
/// # Errors
///
/// [`StartupError::Connect`] when the attempt fails on the configuration
/// ([`AmqpError::is_configuration`]): `CARDDEMO_AMQP_URL` is not a valid AMQP URL.
async fn connect_first(
    amqp_url: &str,
    backoff: &mut Backoff,
    stop: &mut Stop,
) -> Result<Option<Session>, StartupError> {
    match stop.unless_requested(broker::connect(amqp_url)).await {
        None => Ok(None),
        Some(Ok(session)) => Ok(Some(session)),
        Some(Err(error)) if error.is_configuration() => Err(StartupError::Connect(error)),
        Some(Err(error)) => {
            connect_failed(&error);
            reconnect(amqp_url, backoff, stop).await
        }
    }
}

/// Connects again after the next `backoff` delay, retrying until the broker accepts: `Some`
/// with the new session, or `None` once `stop` is requested during a delay or an attempt. Each
/// failure is logged with the [`AmqpError`] text, never the URL. A failed or abandoned attempt
/// has its socket shut down before the next one starts (see [`broker::connect`]).
///
/// # Errors
///
/// [`StartupError::Connect`] when an attempt fails on the configuration
/// ([`AmqpError::is_configuration`]), which no further attempt can cure.
async fn reconnect(
    amqp_url: &str,
    backoff: &mut Backoff,
    stop: &mut Stop,
) -> Result<Option<Session>, StartupError> {
    loop {
        if stop.unless_requested(backoff.wait()).await.is_none() {
            return Ok(None);
        }
        match stop.unless_requested(broker::connect(amqp_url)).await {
            None => return Ok(None),
            Some(Ok(session)) => {
                reconnected();
                return Ok(Some(session));
            }
            Some(Err(error)) if error.is_configuration() => {
                return Err(StartupError::Connect(error));
            }
            Some(Err(error)) => reconnect_failed(&error),
        }
    }
}

/// The three workers of one session, opened on one connection with the production seams: each
/// settles requests on its own consumer channel, CDRA and CP00 use the shared [`PgStore`], and
/// CDRD and CP00 the shared clock.
struct Workers {
    cdrd: cdrd::Worker,
    cdra: cdra::Worker,
    cp00: cp00::Worker,
}

impl Workers {
    /// Opens CDRD, CDRA and CP00 in that order on the connection of `session`, each with its
    /// channel pair and the passive declares of its queues.
    ///
    /// # Errors
    ///
    /// The [`OpenFailure`] of the first worker that could not be opened, classified on the
    /// connection as it stood then. When a worker opened before it holds channels, the
    /// session is closed next with [`Session::close`], and only then are those workers dropped:
    /// the client closes a channel dropped on a live connection in the background, and that
    /// close would race the connection close that [`run`] makes next and fail it.
    async fn open(
        session: &Session,
        config: &Config,
        store: &PgStore,
        clock: &Arc<dyn Clock>,
    ) -> Result<Self, OpenFailure> {
        let connection = session.connection();
        let date = cdrd::Worker::open(connection, config, Arc::clone(clock))
            .await
            .map_err(|error| OpenFailure::new(connection, error))?;
        let account = match cdra::Worker::open(connection, config, store.clone()).await {
            Ok(worker) => worker,
            Err(error) => return Err(abandon(session, error, date).await),
        };
        match cp00::Worker::open(connection, config, store.clone(), Arc::clone(clock)).await {
            Ok(authorization) => Ok(Self {
                cdrd: date,
                cdra: account,
                cp00: authorization,
            }),
            Err(error) => Err(abandon(session, error, (date, account)).await),
        }
    }

    /// Drives the three workers concurrently on the connection of `session` until all of them
    /// have stopped, each with its own copy of `stop`. The workers, and with them their
    /// channels, stay with the caller, which closes the session before it drops them.
    ///
    /// Each worker's future is pinned on the heap with `Box::pin`, the pin that needs no pinning
    /// macro, and one hand-written future polls every worker that has not yet ended at each
    /// wake-up, all three before it decides, and ends only once all three have. A worker is
    /// therefore polled to its own end and never dropped while it holds a request, as racing
    /// them would drop the losers wherever they stood.
    async fn serve(&mut self, session: &Session, poll_interval: Duration, stop: &Stop) {
        let Self { cdrd, cdra, cp00 } = self;
        let mut date = Some(Box::pin(serve_until_stopped(
            cdrd,
            session,
            poll_interval,
            stop.clone(),
        )));
        let mut account = Some(Box::pin(serve_until_stopped(
            cdra,
            session,
            poll_interval,
            stop.clone(),
        )));
        let mut authorization = Some(Box::pin(serve_until_stopped(
            cp00,
            session,
            poll_interval,
            stop.clone(),
        )));
        poll_fn(|cx| {
            // Every result is computed before they are combined, so no worker misses a poll
            // because another one is still running.
            let date_ended = poll_to_end(&mut date, cx);
            let account_ended = poll_to_end(&mut account, cx);
            let authorization_ended = poll_to_end(&mut authorization, cx);
            if date_ended && account_ended && authorization_ended {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

/// Polls the future in `slot`, unless it has already ended, and empties the slot once it ends,
/// so that an ended future is never polled again. Returns whether it has ended.
fn poll_to_end<F: Future<Output = ()>>(
    slot: &mut Option<Pin<Box<F>>>,
    cx: &mut Context<'_>,
) -> bool {
    let Some(future) = slot.as_mut() else {
        return true;
    };
    if future.as_mut().poll(cx).is_ready() {
        *slot = None;
        true
    } else {
        false
    }
}

/// Why the workers of a session could not be opened: the [`AmqpError`] of the first that failed,
/// and whether it showed the connection lost, decided before any close of the connection.
struct OpenFailure {
    error: AmqpError,
    /// [`AmqpError::is_connection_lost`], or the connection already down. [`run`] reconnects
    /// either way, unless the error is a configuration error ([`AmqpError::is_configuration`]),
    /// which it returns; this flag only tells a lost connection from a missing queue or a
    /// refused channel in the event `run` logs.
    connection_lost: bool,
}

impl OpenFailure {
    /// `error` classified on `connection` as it stands now.
    fn new(connection: &Connection, error: AmqpError) -> Self {
        let connection_lost = error.is_connection_lost() || !connection.status().connected();
        Self {
            error,
            connection_lost,
        }
    }
}

/// Classifies `error`, then closes `session` with [`Session::close`] while `opened`, the
/// workers of a session that could not open all three, still hold their channels, and only then
/// drops them.
async fn abandon<T>(session: &Session, error: AmqpError, opened: T) -> OpenFailure {
    let failure = OpenFailure::new(session.connection(), error);
    session.close(OPEN_FAILED_CLOSE_TEXT).await;
    drop(opened);
    failure
}

/// Drives `worker` with [`workers::drive`] on the connection of `session`, with its own
/// [`Backoff`], until it stops: at a loop boundary once `stop` is requested, or when it finds
/// the connection lost, which `drive` has already logged.
///
/// After a lost connection, `session` is closed with [`Session::close`], which waits at most
/// [`broker::OPERATION_DEADLINE`] for the broker and shuts the socket down if it does not
/// answer, so that the other two workers stop at their next broker operation and the three
/// restart together on a new connection: no request queue is left unserved by a session that
/// goes on without one of its workers. The close never cancels another worker: each finishes the request it holds through
/// its own failure path. After a stop, the session is left to [`run`], which closes it once
/// every worker has ended.
async fn serve_until_stopped<W: Step>(
    worker: &mut W,
    session: &Session,
    poll_interval: Duration,
    mut stop: Stop,
) {
    let mut backoff = Backoff::new(poll_interval);
    let connection = session.connection();
    let driven = workers::drive(worker, connection, poll_interval, &mut backoff, &mut stop).await;
    if driven.is_err() {
        let reason = if stop.is_requested() {
            STOP_CLOSE_TEXT
        } else {
            SESSION_CLOSE_TEXT
        };
        session.close(reason).await;
    }
}

/// `INFO` once the workers of a session have opened, with the request queue each of them
/// consumes, as `config` names it.
fn workers_consuming(config: &Config) {
    static SITE: EventSite<4> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "workers_consuming",
        module_path!(),
        Level::INFO,
        &["message", "cdrd", "cdra", "cp00"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the CDRD, CDRA and CP00 workers consume their request queues"),
            Some(&field::display(&config.queue_request_date)),
            Some(&field::display(&config.queue_request_acct)),
            Some(&field::display(&config.queue_request_pauth)),
        ],
    );
}

/// `ERROR` when the workers of a session stopped because its AMQP connection is lost.
fn workers_connection_lost() {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "workers_connection_lost",
        module_path!(),
        Level::ERROR,
        &["message", "outcome"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the workers stopped because the AMQP connection is lost; reconnecting"),
            Some(&"connection lost"),
        ],
    );
}

/// `ERROR` when the workers cannot be opened because the AMQP connection is lost, with the
/// `error`.
fn open_connection_lost(error: &AmqpError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "open_connection_lost",
        module_path!(),
        Level::ERROR,
        &["message", "outcome", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"opening the workers failed because the AMQP connection is lost; reconnecting"),
            Some(&"connection lost"),
            Some(&field::display(error)),
        ],
    );
}

/// `ERROR` when the workers cannot be opened on a live AMQP connection, with the `error`.
fn open_failed(error: &AmqpError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "open_failed",
        module_path!(),
        Level::ERROR,
        &["message", "outcome", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"opening the workers failed on the AMQP connection; reconnecting after backoff"),
            Some(&"open failed"),
            Some(&field::display(error)),
        ],
    );
}

/// `ERROR` when the first connection attempt failed, with the `error`.
fn connect_failed(error: &AmqpError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "connect_failed",
        module_path!(),
        Level::ERROR,
        &["message", "outcome", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"connecting to the AMQP broker failed; retrying after backoff"),
            Some(&"connect failed"),
            Some(&field::display(error)),
        ],
    );
}

/// `INFO` when a reconnection attempt succeeded.
fn reconnected() {
    static SITE: EventSite<1> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "reconnected",
        module_path!(),
        Level::INFO,
        &["message"],
        &SITE,
    );
    emit(&SITE, [Some(&"reconnected to the AMQP broker")]);
}

/// `ERROR` when a reconnection attempt failed, with the `error`.
fn reconnect_failed(error: &AmqpError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "reconnect_failed",
        module_path!(),
        Level::ERROR,
        &["message", "outcome", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"reconnecting to the AMQP broker failed; retrying after backoff"),
            Some(&"reconnect failed"),
            Some(&field::display(error)),
        ],
    );
}

/// The callsite of one event of this crate, whose metadata names `N` fields, `message` first.
///
/// Every `tracing` event macro expands to an attribute that allows `unused_imports`, and no
/// source of this workspace, generated source included, holds a lint suppression. An event of
/// this crate is therefore written out as two statics, an `EventSite` and the
/// [`event_metadata`] or [`event_metadata_with_target`] it holds, and emitted with [`emit`].
/// All of them take the same `N`, so an event always carries exactly one value, or `None`, per
/// field it declares.
pub(crate) struct EventSite<const N: usize>(DefaultCallsite);

impl<const N: usize> EventSite<N> {
    /// The callsite of the event that `metadata` describes.
    pub(crate) const fn new(metadata: &'static Metadata<'static>) -> Self {
        Self(DefaultCallsite::new(metadata))
    }
}

/// The metadata of the event `name` at `level`, with the field `names`, for the callsite `site`
/// in the module `module_path`.
///
/// The target and the module path are both `module_path`, which a caller passes as
/// `module_path!()` written in the emitting module: the target a `tracing` event macro gives an
/// event, so `RUST_LOG` directives select the event exactly as they would select a macro's.
pub(crate) const fn event_metadata<const N: usize>(
    name: &'static str,
    module_path: &'static str,
    level: Level,
    names: &'static [&'static str; N],
    site: &'static EventSite<N>,
) -> Metadata<'static> {
    event_metadata_with_target(name, module_path, module_path, level, names, site)
}

/// [`event_metadata`] with the explicit `target` in place of the module path, which stays
/// `module_path`: for an event that names its own target, such as an alert with
/// `runtime::telemetry::ALERT_TARGET`.
///
/// No source file or line is given: the JSON output and the span export print neither.
pub(crate) const fn event_metadata_with_target<const N: usize>(
    name: &'static str,
    target: &'static str,
    module_path: &'static str,
    level: Level,
    names: &'static [&'static str; N],
    site: &'static EventSite<N>,
) -> Metadata<'static> {
    Metadata::new(
        name,
        target,
        level,
        None,
        None,
        Some(module_path),
        FieldSet::new(names, Identifier(&site.0)),
        Kind::EVENT,
    )
}

/// Emits one event of `site` in the current span with `values`, one per field the site names
/// and in that order, a `None` leaving its field out, when the current subscriber enables it.
///
/// The decision is `runtime::telemetry::event_enabled`, which takes the steps an event macro
/// takes, so the level filters, the JSON layer and the span export treat the event exactly as
/// they treat a macro's.
pub(crate) fn emit<const N: usize>(site: &'static EventSite<N>, values: [Option<&dyn Value>; N]) {
    let callsite = &site.0;
    if !event_enabled(callsite) {
        return;
    }
    let metadata = callsite.metadata();
    Event::dispatch(metadata, &metadata.fields().value_set_all(&values));
}

/// A current-thread runtime with every driver enabled, the runtime an async test attribute
/// would build, made here so that no test macro expands into a lint suppression or an
/// unchecked pin. Every async unit test of the crate runs its body on one with `block_on`.
#[cfg(test)]
fn current_thread() -> tokio::runtime::Runtime {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => panic!("a current-thread runtime does not build: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;
    use std::sync::Arc;

    use domain::Clock;
    use lapin::protocol::{AMQPError, AMQPErrorKind, AMQPHardError, AMQPSoftError};
    use lapin::types::ShortString;
    use persistence::PgStore;
    use runtime::config::Config;
    use test_support::trace_capture;

    use super::{StartupError, Stop, StopHandle, reconnect_failed, run};
    use crate::broker::{
        AmqpError, DEAD_LETTER_QUEUE, Fault, PublishFailure, QueueCheck, QueueLabel,
    };
    use crate::current_thread;
    use crate::workers::StepError;

    /// Accepts only a value whose type is `Send`.
    fn assert_send<T: Send>(_: T) {}

    #[test]
    fn run_can_be_spawned_on_a_multi_threaded_runtime() {
        // Type-checks that the future `run` returns is `Send`, without building a store.
        let _ = |config: &Config, store: PgStore, clock: Arc<dyn Clock>, stop: Stop| {
            assert_send(run(config, store, clock, stop));
        };
    }

    #[test]
    fn a_stop_ends_only_the_waits_that_may_be_abandoned() {
        current_thread().block_on(async {
            let handle = StopHandle::new();
            let mut stop = handle.subscribe();
            assert!(!stop.is_requested());
            assert_eq!(stop.unless_requested(async { 7 }).await, Some(7));

            let mut waiting = stop.clone();
            let forever = std::future::pending::<()>();
            let pending = tokio::spawn(async move { waiting.unless_requested(forever).await });
            handle.request();
            let ended = pending.await.ok();
            assert_eq!(ended, Some(None), "a pending wait ends at the stop");
            assert!(stop.is_requested());
            assert_eq!(stop.unless_requested(async { 7 }).await, None);
            handle.request();
            assert!(stop.is_requested(), "requesting again changes nothing");
        });
    }

    #[test]
    fn dropping_every_stop_handle_requests_the_stop() {
        current_thread().block_on(async {
            let handle = StopHandle::default();
            let mut stop = handle.subscribe();
            let copy = stop.clone();
            drop(handle);
            assert!(stop.is_requested());
            assert!(copy.is_requested());
            stop.requested().await;
            assert_eq!(stop.unless_requested(async { 7 }).await, None);
        });
    }

    #[test]
    fn an_emitted_event_has_its_module_as_target_and_its_fields_in_declared_order() {
        let capture = trace_capture::capture();
        reconnect_failed(&AmqpError::InvalidUrl);

        let lines = capture.lines();
        let [line] = lines.as_slice() else {
            panic!("expected exactly one event line, got {lines:?}");
        };
        // The target a `tracing` event macro in this module would have given the event.
        for member in [
            r#""level":"ERROR""#,
            r#""target":"messaging""#,
            concat!(
                r#""fields":{"message":"reconnecting to the AMQP broker failed; retrying after "#,
                r#"backoff","outcome":"reconnect failed","error":"the AMQP URL "#,
                r#"(CARDDEMO_AMQP_URL) is not valid"}"#
            ),
        ] {
            assert!(line.contains(member), "{member} missing from {line}");
        }
    }

    #[test]
    fn startup_errors_name_the_failure_and_keep_the_broker_error_as_source() {
        let connect = StartupError::Connect(AmqpError::InvalidUrl);
        assert_eq!(
            connect.to_string(),
            "the first AMQP connection failed: the AMQP URL (CARDDEMO_AMQP_URL) is not valid"
        );
        assert!(
            connect
                .source()
                .is_some_and(|source| source.to_string() == AmqpError::InvalidUrl.to_string())
        );

        let not_found = Fault::from(lapin::Error::ProtocolError(AMQPError::new(
            AMQPErrorKind::Soft(AMQPSoftError::NOTFOUND),
            ShortString::from("NOT_FOUND - no queue"),
        )));
        let cases = [
            (
                QueueLabel::Configured {
                    variable: "CARDDEMO_QUEUE_REQUEST_PAUTH",
                },
                "the queue named by CARDDEMO_QUEUE_REQUEST_PAUTH",
            ),
            (
                QueueLabel::Literal(DEAD_LETTER_QUEUE),
                "queue CARD.DEMO.DEADLETTER.PAUTH",
            ),
        ];
        for (queue, shown) in cases {
            let declare = StartupError::Declare(AmqpError::Declare {
                queue,
                source: not_found.clone(),
            });
            assert_eq!(
                declare.to_string(),
                format!(
                    "the workers cannot be opened on the AMQP connection: passive declare of \
                     {shown} failed: AMQP soft error NOT-FOUND (reply code 404)"
                )
            );
            assert!(declare.source().is_some_and(|source| {
                source.to_string() == format!("passive declare of {shown} failed: {not_found}")
            }));
        }
    }

    #[test]
    fn startup_errors_never_repeat_the_amqp_url() {
        let config = Config::default();
        let password = "carddemo@";
        assert!(config.amqp_url.contains(password));
        for error in [
            StartupError::Connect(AmqpError::InvalidUrl),
            StartupError::Declare(AmqpError::QueueName {
                variable: "CARDDEMO_QUEUE_REQUEST_ACCT",
                max_len: 48,
            }),
        ] {
            let text = format!("{error} {error:?}");
            assert!(!text.contains(&config.amqp_url), "{text}");
            assert!(!text.contains(password), "{text}");
        }
    }

    /// Fails when the `Display`, `Debug` or alternate `Debug` of `error`, or of any error in its
    /// source chain, holds one of `protected`.
    fn assert_no_protected_text(error: &dyn Error, protected: &[&str]) {
        let mut current = Some(error);
        while let Some(error) = current {
            let shown = format!("{error} | {error:?} | {error:#?}");
            for text in protected {
                assert!(!shown.contains(text), "{text} shown in: {shown}");
            }
            current = error.source();
        }
    }

    #[test]
    fn no_error_text_debug_or_source_repeats_the_broker_close_text() {
        // A test card number and two password-like words, one of letters only, assembled at
        // run time; the configured request queue holds both the card number and the word.
        let card = format!("4111{}", "1".repeat(12));
        let password = ["Tr0ub4dor", "&3"].concat();
        let word = ["ONLY", "LETR"].concat();
        let queue = format!("QA.{word}.{card}");
        let reply_text = format!("NOT_FOUND - no queue '{queue}' in vhost '/' {password}");
        let protected = [
            card.as_str(),
            password.as_str(),
            word.as_str(),
            queue.as_str(),
            "no queue",
            "vhost",
        ];
        let configured = QueueCheck::configured("CARDDEMO_QUEUE_REQUEST_PAUTH", &queue);
        let client_errors = [
            lapin::Error::ProtocolError(AMQPError::new(
                AMQPErrorKind::Soft(AMQPSoftError::NOTFOUND),
                ShortString::from(reply_text.as_str()),
            )),
            lapin::Error::ProtocolError(AMQPError::new(
                AMQPErrorKind::Hard(AMQPHardError::CONNECTIONFORCED),
                ShortString::from(reply_text.as_str()),
            )),
            lapin::Error::IOError(Arc::new(io::Error::new(
                io::ErrorKind::ConnectionReset,
                reply_text.clone(),
            ))),
        ];
        for client_error in &client_errors {
            assert!(
                client_error.to_string().contains(&card),
                "the client's own text repeats the broker's, which is why it is dropped"
            );
            let fault = Fault::from(client_error);
            let operations = [
                AmqpError::Connect(fault.clone()),
                AmqpError::Channel(fault.clone()),
                AmqpError::Declare {
                    queue: configured.label(),
                    source: fault.clone(),
                },
                AmqpError::Declare {
                    queue: QueueCheck::literal(DEAD_LETTER_QUEUE).label(),
                    source: fault.clone(),
                },
                AmqpError::Get(fault.clone()),
                AmqpError::Ack(fault.clone()),
                AmqpError::Reject(fault.clone()),
            ];
            let mut errors: Vec<Box<dyn Error>> =
                vec![Box::new(PublishFailure::Channel(fault.clone()))];
            for operation in operations {
                assert!(operation.to_string().ends_with(&fault.to_string()));
                if let AmqpError::Declare { queue: label, .. } = &operation {
                    let (QueueLabel::Configured { variable: named } | QueueLabel::Literal(named)) =
                        label;
                    for shown in [
                        operation.to_string(),
                        format!("{operation:?}"),
                        StartupError::Declare(operation.clone()).to_string(),
                    ] {
                        assert!(shown.contains(named), "{named} not shown in: {shown}");
                    }
                }
                errors.push(Box::new(StartupError::Connect(operation.clone())));
                errors.push(Box::new(StartupError::Declare(operation.clone())));
                errors.push(Box::new(StepError::Channel(operation.clone())));
                errors.push(Box::new(StepError::ConnectionLost(operation.clone())));
                errors.push(Box::new(operation));
            }
            for error in &errors {
                assert_no_protected_text(error.as_ref(), &protected);
            }
        }
        let shown = format!("{configured:?} | {configured:#?}");
        for text in protected {
            assert!(!shown.contains(text), "{text} shown in: {shown}");
        }
        let not_found = Fault::from(&client_errors[0]).to_string();
        assert_eq!(not_found, "AMQP soft error NOT-FOUND (reply code 404)");
    }
}
