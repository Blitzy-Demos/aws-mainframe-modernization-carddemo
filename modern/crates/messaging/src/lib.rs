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
//! (`CODATE01.cbl:147,243`, `COACCT01.cbl:198,294`); the README's `CARDDEMO.REQUEST.QUEUE` and
//! `AWS.M2.CARDDEMO.PAUTH.*` appear in no source and are not used (E01).
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
//! counterpart: a worker takes requests until its connection is lost.
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

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use domain::Clock;
use lapin::Connection;
use lapin::protocol::constants::REPLY_SUCCESS;
use persistence::PgStore;
use runtime::config::Config;

use crate::broker::{AmqpError, Backoff};
use crate::workers::{Step, cdra, cdrd, cp00};

/// The reply text of the connection close with which the first worker to stop ends a session.
const SESSION_CLOSE_TEXT: &str = "carddemo-mq worker stopped; reconnecting";

/// Why [`run`] stopped. Neither variant's text holds the AMQP URL, a credential or a message
/// body: [`AmqpError`] names only an operation, a queue or variable, and a broker or client
/// condition.
#[derive(Debug, Clone)]
pub enum StartupError {
    /// The first connection to the broker failed: `CARDDEMO_AMQP_URL` is not a valid AMQP URL
    /// ([`AmqpError::InvalidUrl`]), or the broker cannot be reached or refuses the login
    /// ([`AmqpError::Connect`]). A connection lost later is retried and never returned.
    Connect(AmqpError),
    /// The workers could not be opened on a live connection: a request, reply, error or
    /// dead-letter queue is missing from the broker ([`AmqpError::Declare`]), the configured CDRA
    /// request queue does not fit the COBOL field that reports it ([`AmqpError::QueueName`]), or
    /// the broker refused a channel. Each is a misconfiguration that no reconnect cures.
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

/// Runs the CDRD, CDRA and CP00 workers on one AMQP connection until a fatal error, and
/// reconnects whenever that connection is lost.
///
/// The caller reads `config` with `Config::from_env`, installs `runtime::telemetry`, opens
/// `store` over the pool of `runtime::db::connect` and passes the clock, `SystemClock` in
/// production; `run` itself touches neither the environment nor the schema. `store` serves
/// CDRA's account reads and every CP00 read, write and log row; `clock` supplies the CDRD date
/// and time and the CP00 keys, log dates and message IDs.
///
/// Each session opens the three workers, which check their queues with passive declares, then
/// drives them concurrently until the connection is lost. Every worker finishes the request it
/// holds through its own failure path; none is cancelled mid-message, for example between a CP00
/// acknowledgement and its commit. The broker requeues every request left unsettled. `run` then
/// reconnects with a backoff that starts at `config.mq_poll_interval()` and doubles up to
/// [`broker::BACKOFF_CAP`] over consecutive failed attempts, and opens the workers again
/// (COPAUA0C-118).
///
/// # Errors
///
/// - [`StartupError::Connect`] when the first connection fails.
/// - [`StartupError::Declare`] when the workers cannot be opened on a live connection, at
///   startup or after a reconnect.
///
/// A connection lost after startup, while opening the workers or while they run, is never
/// returned: it is logged and retried.
pub async fn run(
    config: &Config,
    store: PgStore,
    clock: Arc<dyn Clock>,
) -> Result<Infallible, StartupError> {
    let poll_interval = config.mq_poll_interval();
    let mut backoff = Backoff::new(poll_interval);
    let mut connection = broker::connect(&config.amqp_url)
        .await
        .map_err(StartupError::Connect)?;
    loop {
        match Workers::open(&connection, config, &store, &clock).await {
            Ok(workers) => {
                // The reconnect delay grows only over consecutive failed attempts: a session
                // whose workers opened starts the next outage from the poll interval again.
                backoff.reset();
                tracing::info!(
                    cdrd = %config.queue_request_date,
                    cdra = %config.queue_request_acct,
                    cp00 = %config.queue_request_pauth,
                    "the CDRD, CDRA and CP00 workers consume their request queues"
                );
                // Boxed: the three workers' futures together exceed the stack budget that
                // `clippy::large_futures` allows, and would make every caller of `run` hold them.
                Box::pin(workers.serve(&connection, poll_interval)).await;
                tracing::error!(
                    outcome = "connection lost",
                    "the workers stopped because the AMQP connection is lost; reconnecting"
                );
            }
            Err(error) if error.is_connection_lost() || !connection.status().connected() => {
                tracing::error!(
                    outcome = "connection lost",
                    error = %error,
                    "opening the workers failed because the AMQP connection is lost; reconnecting"
                );
            }
            Err(error) => return Err(StartupError::Declare(error)),
        }
        drop(connection);
        connection = reconnect(&config.amqp_url, &mut backoff).await;
    }
}

/// Connects again after the next `backoff` delay, retrying until the broker accepts. Each
/// failure is logged with the [`AmqpError`] text, never the URL.
async fn reconnect(amqp_url: &str, backoff: &mut Backoff) -> Connection {
    loop {
        backoff.wait().await;
        match broker::connect(amqp_url).await {
            Ok(connection) => {
                tracing::info!("reconnected to the AMQP broker");
                return connection;
            }
            Err(error) => tracing::error!(
                outcome = "reconnect failed",
                error = %error,
                "reconnecting to the AMQP broker failed; retrying after backoff"
            ),
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
    /// Opens CDRD, CDRA and CP00 in that order on `connection`, each with its channel pair and
    /// the passive declares of its queues.
    ///
    /// # Errors
    ///
    /// The [`AmqpError`] of the first worker that could not be opened. Channels of the workers
    /// opened before it close when they are dropped.
    async fn open(
        connection: &Connection,
        config: &Config,
        store: &PgStore,
        clock: &Arc<dyn Clock>,
    ) -> Result<Self, AmqpError> {
        Ok(Self {
            cdrd: cdrd::Worker::open(connection, config, Arc::clone(clock)).await?,
            cdra: cdra::Worker::open(connection, config, store.clone()).await?,
            cp00: cp00::Worker::open(connection, config, store.clone(), Arc::clone(clock)).await?,
        })
    }

    /// Drives the three workers concurrently until all of them have stopped.
    ///
    /// `tokio::join!` polls every worker to its own end: a worker is never dropped while it
    /// holds a request, as racing them would drop the losers wherever they stood.
    async fn serve(self, connection: &Connection, poll_interval: Duration) {
        let Self { cdrd, cdra, cp00 } = self;
        tokio::join!(
            serve_until_stopped(cdrd, connection, poll_interval),
            serve_until_stopped(cdra, connection, poll_interval),
            serve_until_stopped(cp00, connection, poll_interval),
        );
    }
}

/// Drives `worker` with [`workers::drive`] and its own [`Backoff`] until it stops, which happens
/// only when it finds the connection lost; `drive` has already logged why.
///
/// If `connection` still reports connected then, it is closed, so that the other two workers
/// stop at their next broker operation and the three restart together on a new connection: no
/// request queue is left unserved by a session that goes on without one of its workers.
async fn serve_until_stopped<W: Step>(
    mut worker: W,
    connection: &Connection,
    poll_interval: Duration,
) {
    let mut backoff = Backoff::new(poll_interval);
    workers::drive(&mut worker, connection, poll_interval, &mut backoff).await;
    if connection.status().connected() {
        if let Err(error) = connection.close(REPLY_SUCCESS, SESSION_CLOSE_TEXT).await {
            tracing::warn!(
                queue = worker.queue(),
                error = %error,
                "closing the AMQP connection after a worker stopped failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use domain::Clock;
    use lapin::ChannelState;
    use persistence::PgStore;
    use runtime::config::Config;

    use super::{StartupError, run};
    use crate::broker::AmqpError;

    /// Accepts only a value whose type is `Send`.
    fn assert_send<T: Send>(_: T) {}

    #[test]
    fn run_can_be_spawned_on_a_multi_threaded_runtime() {
        // Type-checks that the future `run` returns is `Send`, without building a store.
        let _ = |config: &Config, store: PgStore, clock: Arc<dyn Clock>| {
            assert_send(run(config, store, clock));
        };
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

        let declare = StartupError::Declare(AmqpError::Declare {
            queue: "CARD.DEMO.REQUEST.PAUTH".to_owned(),
            source: lapin::Error::InvalidChannelState(ChannelState::Closed),
        });
        let text = declare.to_string();
        assert!(
            text.starts_with(
                "the workers cannot be opened on the AMQP connection: passive declare of queue \
                 CARD.DEMO.REQUEST.PAUTH failed: "
            ),
            "{text}"
        );
        assert!(declare.source().is_some());
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
}
