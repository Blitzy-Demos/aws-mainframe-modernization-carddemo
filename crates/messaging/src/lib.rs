//! `carddemo-messaging`: the `RabbitMQ` adapter that replaces the three IBM MQ
//! trigger-started CICS transactions of `CardDemo`.
//!
//! The CSD binds each transaction to its program
//! [app/app-vsam-mq/csd/CRDDEMOM.csd:L17-36]
//! [app/app-authorization-ims-db2-mq/csd/CRDDEMO2.csd:L59-60]. Here each one
//! is a long-running consumer:
//!
//! | Transaction | Program | Flow | Request queue | Reply queue |
//! |---|---|---|---|---|
//! | CDRD | `CODATE01` | [`flows::cdrd`] | `CARDDEMO_QUEUE_DATE_REQUEST` | [`flows::cdrd::REPLY_QUEUE`] |
//! | CDRA | `COACCT01` | [`flows::cdra`] | `CARDDEMO_QUEUE_ACCT_REQUEST` | [`flows::cdra::REPLY_QUEUE`] |
//! | CP00 | `COPAUA0C` | [`flows::cp00`] | `CARDDEMO_QUEUE_PAUTH_REQUEST` | the request's `reply_to` |
//!
//! Every business result comes from the `carddemo_domain` rules. This crate
//! only moves bytes between the queues, those rules and `carddemo-persistence`,
//! the one path to `PostgreSQL`: the CDRA account read, and the CP00
//! cross-reference, account, customer and summary reads and its
//! pending-authorization writes.
//!
//! # Queue ownership
//!
//! - The request queues are deployment settings, read from
//!   `carddemo_runtime::config::Settings`, as the COBOL programs take their
//!   input queue from the trigger message (`MQTM-QNAME`)
//!   [app/app-vsam-mq/cbl/CODATE01.cbl:L146].
//! - `CARD.DEMO.REPLY.DATE`, `CARD.DEMO.REPLY.ACCT` and `CARD.DEMO.ERROR` are
//!   COBOL literals, so they are constants: [`flows::cdrd::REPLY_QUEUE`],
//!   [`flows::cdra::REPLY_QUEUE`] and [`error_queue::ERROR_QUEUE`].
//! - CP00 replies to each request's `reply_to`, the MQ `REPLYTOQ`
//!   [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L413].
//!
//! No queue is declared here: `infra/rabbitmq/definitions.json` declares every
//! durable queue.
//!
//! # Acknowledge after confirm
//!
//! Each consumer has its own channel in publisher-confirm mode with a prefetch
//! of 1. A request is acknowledged only after the broker confirms its reply
//! with `Ack(None)`. Without a confirmed reply, CDRD and CDRA publish their
//! `MQ-ERR-DISPLAY` record to `CARD.DEMO.ERROR` and CP00 logs its `CCPAUERY`
//! record; then the request is rejected without requeue. [`consumer`] holds
//! the rule and its COBOL grounds.
//!
//! # Lifecycle
//!
//! [`start`] connects, opens one channel per flow and spawns one task per
//! consumer, returning [`Running`]. [`Running::wait`] returns when the first
//! consumer stops, and [`Running::shutdown`] stops them all. [`run`] is
//! [`start`] then [`Running::wait`], as the `mq` binary uses it. The spawned
//! tasks carry the caller's tracing dispatcher, so a subscriber installed by
//! the caller, global or scoped, receives every message span.
//!
//! No identity is checked: the requests carry no credential, and their
//! `message_id`, `correlation_id` and `reply_to` serve routing and correlation
//! only.

pub mod connection;
pub mod consumer;
pub mod error_queue;
pub mod ids;

/// The three request/reply flows, one module per COBOL program. Each
/// implements [`consumer::Flow`] and decides only what to send; publishing
/// and settling belong to [`consumer::run`].
pub mod flows {
    pub mod cdra;
    pub mod cdrd;
    pub mod cp00;
}

use carddemo_domain::clock::{AnyClock, ClockError};
use carddemo_persistence::{Db, PersistenceError};
use carddemo_runtime::config::Settings;
use lapin::protocol::constants::REPLY_SUCCESS;
use lapin::{Channel, Connection};
use tokio::task::JoinSet;
use tracing::instrument::WithSubscriber;

use crate::consumer::Flow;
use crate::flows::cdra::CdraFlow;
use crate::flows::cdrd::CdrdFlow;
use crate::flows::cp00::Cp00Flow;

/// Every way starting or running the consumers can fail. It is the only error
/// type of the crate.
#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    /// The broker connection, a channel, a publish confirmation or a
    /// settlement failed.
    #[error("AMQP error: {0}")]
    Amqp(#[from] lapin::Error),

    /// A persistence operation made outside a port call failed, such as
    /// opening the database pool.
    #[error("persistence error: {0}")]
    Persistence(#[from] PersistenceError),

    /// `CARDDEMO_CLOCK_OVERRIDE` is set but is not an RFC 3339 instant.
    #[error("invalid clock override: {0}")]
    Clock(#[from] ClockError),

    /// The broker ended a consumer's delivery stream, for example because its
    /// request queue was deleted.
    #[error("consumer {flow} stopped")]
    ConsumerStopped {
        /// The transaction ID of the consumer: `CDRD`, `CDRA` or `CP00`.
        flow: &'static str,
    },

    /// A consumer task panicked or was cancelled before it returned.
    #[error("consumer task failed: {0}")]
    Join(String),
}

/// The started consumers: the broker connection and one task per flow.
///
/// Dropping it aborts the tasks, and lapin then closes the connection without
/// waiting for the broker. [`Running::wait`] and [`Running::shutdown`] close it
/// and wait for the broker's reply.
#[derive(Debug)]
#[must_use = "dropping `Running` aborts the consumers; call `wait` or `shutdown`"]
pub struct Running {
    /// The connection every consumer channel is opened on.
    connection: Connection,
    /// One task per flow, each running `consumer::run` until it fails.
    tasks: JoinSet<Result<(), MessagingError>>,
}

impl Running {
    /// Waits until the first consumer stops, then stops the others and closes
    /// the broker connection.
    ///
    /// A failure to close the connection is logged and does not replace the
    /// consumer's outcome.
    ///
    /// # Errors
    ///
    /// Returns the stopped consumer's error: [`MessagingError::Amqp`] or
    /// [`MessagingError::ConsumerStopped`]. Returns [`MessagingError::Join`]
    /// when its task panicked or was cancelled.
    pub async fn wait(mut self) -> Result<(), MessagingError> {
        let outcome = match self.tasks.join_next().await {
            // A consumer returns only errors; an `Ok` is passed through as is.
            Some(Ok(result)) => result,
            Some(Err(error)) => Err(MessagingError::Join(error.to_string())),
            None => Ok(()),
        };
        if let Err(error) = self.shutdown().await {
            tracing::warn!(error = %error, "broker connection close failed after a consumer stopped");
        }
        outcome
    }

    /// Aborts every consumer, waits for the tasks to end and closes the broker
    /// connection.
    ///
    /// A delivery being settled when its task is aborted stays unsettled, and
    /// the broker redelivers it once the connection closes.
    ///
    /// # Errors
    ///
    /// Returns [`MessagingError::Amqp`] when the connection is no longer open
    /// or the broker does not confirm the close.
    pub async fn shutdown(mut self) -> Result<(), MessagingError> {
        self.tasks.abort_all();
        while let Some(joined) = self.tasks.join_next().await {
            // Cancellation is the expected end here, and a consumer that had
            // already failed logged its error in its own task.
            if let Err(error) = joined
                && error.is_panic()
            {
                tracing::error!(error = %error, "consumer task panicked");
            }
        }
        self.connection.close(REPLY_SUCCESS, "shutdown").await?;
        Ok(())
    }
}

/// Starts the CDRD, CDRA and CP00 consumers.
///
/// Builds the clock from `settings.clock_override` before anything connects,
/// opens one broker connection on `settings.amqp_url` and one confirm-mode
/// channel per flow, then spawns each consumer on its request queue. Every
/// task inherits the caller's tracing dispatcher.
///
/// # Errors
///
/// Returns [`MessagingError::Clock`] when the clock override is not an RFC
/// 3339 instant, and [`MessagingError::Amqp`] when the broker connection or a
/// channel cannot be opened.
pub async fn start(settings: &Settings, db: Db) -> Result<Running, MessagingError> {
    let clock = AnyClock::from_override(settings.clock_override.as_deref())?;
    let connection = connection::connect(&settings.amqp_url).await?;
    // On failure `connection` is dropped with its last handle, and lapin then
    // closes it.
    let [date_channel, acct_channel, pauth_channel] = open_channels(&connection).await?;

    let mut tasks = JoinSet::new();
    spawn_consumer(
        &mut tasks,
        date_channel,
        settings.queue_date_request.clone(),
        CdrdFlow::new(clock),
    );
    spawn_consumer(
        &mut tasks,
        acct_channel,
        settings.queue_acct_request.clone(),
        CdraFlow::new(db.clone(), settings.queue_acct_request.clone()),
    );
    spawn_consumer(
        &mut tasks,
        pauth_channel,
        settings.queue_pauth_request.clone(),
        Cp00Flow::new(db, clock),
    );
    Ok(Running { connection, tasks })
}

/// Starts the consumers and waits until the first one stops.
///
/// # Errors
///
/// Returns any error of [`start`] or of [`Running::wait`].
pub async fn run(settings: &Settings, db: Db) -> Result<(), MessagingError> {
    start(settings, db).await?.wait().await
}

/// Opens the three consumer channels: CDRD, CDRA and CP00, in that order.
async fn open_channels(connection: &Connection) -> Result<[Channel; 3], MessagingError> {
    Ok([
        connection::open_channel(connection).await?,
        connection::open_channel(connection).await?,
        connection::open_channel(connection).await?,
    ])
}

/// Spawns `flow`'s consumer on `queue` into `tasks`.
///
/// The task logs its own failure with the flow and queue, because the error
/// [`Running::wait`] returns does not always name them.
fn spawn_consumer<F: Flow>(
    tasks: &mut JoinSet<Result<(), MessagingError>>,
    channel: Channel,
    queue: String,
    flow: F,
) {
    tracing::info!(flow = F::NAME, queue = %queue, "consumer started");
    tasks.spawn(
        async move {
            let result = consumer::run(channel, &queue, flow).await;
            if let Err(error) = &result {
                tracing::error!(flow = F::NAME, queue = %queue, error = %error, "consumer stopped");
            }
            result
        }
        .with_current_subscriber(),
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use carddemo_domain::clock::{AnyClock, ClockError};
    use carddemo_persistence::PersistenceError;
    use lapin::ChannelState;

    use super::MessagingError;

    /// `AnyClock::from_override` behind `?`, as `start` calls it.
    fn clock(value: &str) -> Result<AnyClock, MessagingError> {
        Ok(AnyClock::from_override(Some(value))?)
    }

    #[test]
    fn every_variant_displays_its_cause() {
        let cases = [
            (
                MessagingError::ConsumerStopped { flow: "CDRD" },
                "consumer CDRD stopped".to_owned(),
            ),
            (
                MessagingError::Join("task 7 panicked".to_owned()),
                "consumer task failed: task 7 panicked".to_owned(),
            ),
            (
                MessagingError::from(lapin::Error::InvalidChannelState(ChannelState::Closed)),
                format!(
                    "AMQP error: {}",
                    lapin::Error::InvalidChannelState(ChannelState::Closed)
                ),
            ),
            (
                MessagingError::from(PersistenceError::Fixture {
                    path: PathBuf::from("app/data/ASCII/acctdata.txt"),
                    line: 3,
                    reason: "short record".to_owned(),
                }),
                "persistence error: app/data/ASCII/acctdata.txt:3: short record".to_owned(),
            ),
            (
                MessagingError::from(ClockError::InvalidOverride("x".to_owned())),
                "invalid clock override: CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: x"
                    .to_owned(),
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text, "{error:?}");
        }
    }

    #[test]
    fn an_invalid_clock_override_converts_to_the_clock_variant() {
        let error = clock("not-a-date").expect_err("not an RFC 3339 instant");
        assert!(
            matches!(
                &error,
                MessagingError::Clock(ClockError::InvalidOverride(text)) if text == "not-a-date"
            ),
            "{error:?}"
        );
        assert!(clock("2022-07-01T12:00:00Z").is_ok());
    }
}
