//! CP00 (`COPAUA0C`) against a real broker and a fixture-loaded database (g3a; AAP 0.3.4.2,
//! `modern/audit/messaging/COPAUA0C.md`).
//!
//! Every test starts its own `rabbitmq:3.13-management` container, booted from
//! `rabbitmq/definitions.json`, and its own migrated, fixture-loaded `postgres:16` container
//! through `test_support`, and runs the CP00 worker in process on a connection of its own. The
//! test publishes requests and reads replies on a second connection, whose channel is in confirm
//! mode, so a published request is on its queue before the worker polls.
//!
//! # What is asserted
//!
//! Confirmations, settlement, reply properties and database effects, never a reply body: the
//! 64th byte of every CP00 reply comes from a buffer the COBOL never writes, `HALTED(H6)`
//! (COPAUA0C-065), so no body is compared or snapshotted here. The redaction of the reply's card
//! field is `redaction.rs`'s, and response codes are read here only from the detail rows.
//!
//! Every reply's `message_id` is newly generated (COPAUA0C-067): each test that takes several
//! replies keeps their IDs and asserts that no two are equal and that none is a request's, so a
//! constant reply ID fails.
//!
//! Settlement is read from the broker: closing the worker's connection returns every request it
//! left unsettled to its queue, so an empty request queue after the close proves an
//! acknowledgement, and a request back on it proves that none was made.
//!
//! The rows are those the audit's "How the rows are proved" assigns to this file, the
//! injected-store cases included: the order of the store calls around the acknowledgement
//! (COPAUA0C-080, COPAUA0C-081), the failed commit (COPAUA0C-121), the store-read failures
//! (COPAUA0C-122 to COPAUA0C-126) and the not-found warnings (COPAUA0C-115, COPAUA0C-116). A
//! store-read failure is asserted with its critical row after a server error and with no row
//! attempted after an unavailable database, and in both cases with the interim `05`/`9000`
//! decline of `X-COPAUA0C-1`: one reply, acknowledged, nothing written. The `HALTED` rows get
//! no parity assertion; the ignored `blocked_*` tests at the end report them `BLOCKED` (AAP
//! 0.8.5).
//!
//! # Startup through `messaging::run`
//!
//! The `run_*` tests start the three workers as the `carddemo-mq` binary does, through
//! `messaging::run` with a 20 ms poll interval, under `test_support::trace_capture`, and request
//! its stop through a `StopHandle`. They prove the startup resilience of the process, which no
//! audit row states, so they carry no audit tag:
//!
//! - a broker that refuses the first connection is retried with the backoff, one `ERROR` event
//!   per failed attempt, until the stop, and no event shows the AMQP URL or its password;
//! - a queue missing at startup is retried, one `ERROR` event per session whose workers cannot
//!   open, until it is declared, after which the workers open and CP00 answers a request;
//! - a configuration error, an AMQP URL that does not parse or a refused request queue name,
//!   ends the run at once with its `StartupError`, without a retry.
//!
//! # Doubles
//!
//! [`InjectedStore`] and [`InjectedTx`] wrap the real `PgStore` and `PgTx` and delegate every
//! call, except the one [`StoreFault`] names, which fails with the store's [`Failure`];
//! [`StandInAcknowledger`] stands in for the consumer channel's settlement. Both record the
//! order of their calls in a [`CallOrder`] of atomics, the store's `cssl_error_log` writes
//! included. They implement only the worker's seams (`AuthStore`, `AuthTx`, `Acknowledger`),
//! never a `persistence` repository trait.
//!
//! A reply on its queue is not a confirmed one: the broker queues a message before its
//! confirmation reaches the publisher. The confirmation-order tests therefore connect the
//! worker through a `test_support::rabbitmq::ConfirmGate` ([`Harness::open_worker_through`]),
//! which withholds each confirmation the broker sends while the test reads the call order, the
//! database and the queues for [`HOLD`], then releases it, which the test records as the
//! [`CallOrder`]'s `confirm` slot, or refuses it, handing the worker a negative confirmation.
//! While the reply's confirmation is held, the reply is on its queue, the locking reads are
//! done, and nothing is acknowledged, rejected, written, logged, committed or rolled back
//! (COPAUA0C-081); a refused confirmation takes the `M004` path (COPAUA0C-117).
//!
//! # Redaction
//!
//! A request body holds a full card number, taken from the fixtures through
//! `CardNumber::expose` only to build that body. No assertion message shows a body, a full card
//! number or an event key that is checked for one; card numbers compare through `CardNumber`,
//! whose `Debug` shows only the redacted form.

use std::collections::BTreeSet;
use std::error::Error;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::NaiveDate;
use copybook::{PicS, PicU, PicX};
use domain::messaging::codate01::ERROR_QUEUE;
use domain::records::{
    AccountRecord, CardXrefRecord, CustomerRecord, ErrorLogRecord, PendingAuthDetailRecord,
    PendingAuthSummaryRecord,
};
use domain::{CardNumber, Clock, FixedClock};
use lapin::message::BasicGetMessage;
use lapin::options::{ConfirmSelectOptions, QueueDeclareOptions, QueueDeleteOptions};
use lapin::types::{FieldTable, ShortString};
use lapin::{BasicProperties, Channel, Connection};
use messaging::broker::{self, Acknowledger, AmqpError, Fault, Session};
use messaging::workers::cp00::{AuthStore, AuthTx, Worker};
use messaging::workers::{Processed, Step};
use messaging::{StartupError, StopHandle, run};
use persistence::{
    CardXrefRepository, CsslErrorLogRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PersistenceError, PgStore, PgTx,
};
use runtime::config::Config;
use rust_decimal::Decimal;
use test_support::fixtures::{fixture_card_numbers, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use test_support::rabbitmq::{self, ConfirmGate, RabbitMqGuard, start_rabbitmq};
use test_support::trace_capture::{self, TraceCapture};
use tokio::time::{Instant, sleep, timeout};
use tracing::instrument::WithSubscriber;

type BoxError = Box<dyn Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

/// Bound on every broker wait of this file, and on every poll until a count is reached.
const WAIT: Duration = Duration::from_secs(10);
/// How long a queue count must hold once reached: the broker requeues the deliveries of a closed
/// connection asynchronously, so a count read at once could miss a late requeue.
const SETTLE: Duration = Duration::from_millis(500);
/// Interval between the polls of a count.
const POLL: Duration = Duration::from_millis(50);
/// How long a drain of the reply queue waits for one more reply.
const DRAIN: Duration = Duration::from_millis(500);
/// AMQP `reply-success`, the code of the test connection's close.
const REPLY_SUCCESS: u16 = 200;
/// The reason text of every close this file makes.
const CLOSE_REASON: &str = "cp00 test done";
/// How long the confirmation gate withholds a reply's confirmation before the test reads the
/// call order, the database and the queues. A quarter of the worker's
/// [`broker::PUBLISH_DEADLINE`] at most, so that the publication and its released confirmation
/// still finish within it.
const HOLD: Duration = Duration::from_secs(1);
const _: () = assert!(
    HOLD.as_millis() * 4 <= broker::PUBLISH_DEADLINE.as_millis(),
    "HOLD must stay well within PUBLISH_DEADLINE"
);
/// The calls a found card's request makes before its reply is published: the cross-reference
/// read, the transaction, the summary lock, and the account and customer reads (COPAUA0C-081).
const LOCKING_READS: [&str; 5] = [
    "card_xref read",
    "begin",
    "summary lock",
    "account read",
    "customer read",
];

/// A `reply_to` naming no queue, so the worker's `mandatory` publication comes back returned.
const NO_SUCH_REPLY_QUEUE: &str = "CARD.DEMO.NO.SUCH.REPLY";
/// A well-formed card number that `cardxref.txt` does not hold; each test that uses it checks
/// its absence first.
const UNKNOWN_CARD: &str = "1000000000000001";

/// `CARDDEMO_MQ_POLL_MS` of the `messaging::run` tests, and so the first backoff base: short,
/// so that several retries fit in a fraction of a second.
const RUN_POLL_MS: u64 = 20;
/// Bound on a whole `messaging::run` of this file, from its start to its return.
const RUN_BOUND: Duration = Duration::from_secs(60);
/// How soon `messaging::run` must return once its stop is requested, or, on a configuration
/// error, once it starts: well under one backoff cap, so a run that went on retrying fails.
const RUN_RETURN_BOUND: Duration = Duration::from_secs(3);
/// The message of the event of every failed connection attempt: the first attempt's
/// ("connecting to ...") and each reconnect's ("reconnecting to ...").
const CONNECT_RETRIED: &str = "connecting to the AMQP broker failed; retrying after backoff";
/// The message of the event of a session whose workers could not be opened on a live
/// connection, for example over a missing queue.
const OPEN_RETRIED: &str =
    "opening the workers failed on the AMQP connection; reconnecting after backoff";
/// The message of the event of a session whose three workers have opened.
const WORKERS_CONSUME: &str = "the CDRD, CDRA and CP00 workers consume their request queues";

/// Whether a [`StartupError`] is the one a case expects.
type IsExpected = fn(&StartupError) -> bool;

/// The request fields other than the card number, the amount and the transaction ID, in
/// `CCPAURQY` order (`COPAUA0C.cbl:354-373`): fields 1, 2 and 4 to 8, then 10 to 17.
const AUTH_DATE: &str = "220718";
const AUTH_TIME: &str = "101112";
const AUTH_TYPE: &str = "0100";
const CARD_EXPIRY: &str = "1225";
const MESSAGE_TYPE: &str = "AUTHRQ";
const MESSAGE_SOURCE: &str = "POSTRM";
const PROCESSING_CODE: &str = "000000";
const MERCHANT_CATEGORY: &str = "5411";
const ACQUIRER_COUNTRY: &str = "840";
const POS_ENTRY_MODE: &str = "05";
const MERCHANT_ID: &str = "MERCH0000000001";
const MERCHANT_NAME: &str = "CARDDEMO GROCERY";
const MERCHANT_CITY: &str = "SEATTLE";
const MERCHANT_STATE: &str = "WA";
const MERCHANT_ZIP: &str = "98101";

/// An approval of 100.00 as the detail row records it (COPAUA0C-044, COPAUA0C-101).
const APPROVED: Decided = Decided {
    resp_code: "00",
    reason: "0000",
    match_status: "P",
    approved_cents: 10_000,
};

/// A CP00 request body: the 18 comma-delimited fields of `CCPAURQY` in order
/// (`COPAUA0C.cbl:354-378`), with `card` as field 3, `amount` as field 9 and `tran_id` as field
/// 18.
fn auth_request(card: &str, amount: &str, tran_id: &str) -> Vec<u8> {
    [
        AUTH_DATE,
        AUTH_TIME,
        card,
        AUTH_TYPE,
        CARD_EXPIRY,
        MESSAGE_TYPE,
        MESSAGE_SOURCE,
        PROCESSING_CODE,
        amount,
        MERCHANT_CATEGORY,
        ACQUIRER_COUNTRY,
        POS_ENTRY_MODE,
        MERCHANT_ID,
        MERCHANT_NAME,
        MERCHANT_CITY,
        MERCHANT_STATE,
        MERCHANT_ZIP,
        tran_id,
    ]
    .join(",")
    .into_bytes()
}

/// The 16 digits of `card`, only for a request body; never format or print the result.
fn card_text(card: &CardNumber) -> Result<&str, BoxError> {
    Ok(std::str::from_utf8(card.expose().as_bytes())?)
}

/// The IDs a request was published with.
struct Sent {
    message_id: String,
    correlation_id: String,
}

/// One test's broker, database, worker configuration and clock, and the test's own connection
/// with its confirm-mode channel and reply queue.
///
/// Fields drop in declaration order: the test's channel and connection first, the containers
/// last.
struct Harness {
    channel: Channel,
    connection: Connection,
    reply_queue: String,
    config: Config,
    clock: Arc<dyn Clock>,
    postgres: PostgresGuard,
    broker: RabbitMqGuard,
}

impl Harness {
    /// Starts both containers, opens the test's connection with a confirm-mode channel and a
    /// reply queue, and fixes the clock at 2022-07-18 10:11:12.345, one instant to the
    /// millisecond for every reading.
    async fn start() -> Result<Self, BoxError> {
        let (broker, postgres) = tokio::try_join!(start_rabbitmq(), start_loaded_postgres())?;
        let connection = broker.connect().await?;
        let channel = timeout(WAIT, connection.create_channel()).await??;
        timeout(
            WAIT,
            channel.confirm_select(ConfirmSelectOptions::default()),
        )
        .await??;
        let reply_queue = rabbitmq::declare_reply_queue(&channel).await?;
        let config = Config {
            amqp_url: broker.amqp_url().to_owned(),
            database_url: postgres.database_url().to_owned(),
            ..Config::default()
        };
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_milli_opt(10, 11, 12, 345))
            .ok_or("the test instant is not a valid date and time")?;
        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(at));
        Ok(Self {
            channel,
            connection,
            reply_queue,
            config,
            clock,
            postgres,
            broker,
        })
    }

    /// A store over the fixture-loaded database's pool.
    fn store(&self) -> PgStore {
        PgStore::new(self.postgres.pool().clone())
    }

    /// The configured request queue, the default `CARD.DEMO.REQUEST.PAUTH`, which dead-letters
    /// to `CARD.DEMO.DEADLETTER.PAUTH` through `definitions.json`.
    fn request_queue(&self) -> &str {
        &self.config.queue_request_pauth
    }

    /// A CP00 worker with the production seams, on a new connection of its own.
    async fn open_worker(&self) -> Result<(Session, Worker), BoxError> {
        let session = broker::connect(self.broker.amqp_url()).await?;
        let worker = Worker::open(
            session.connection(),
            &self.config,
            self.store(),
            Arc::clone(&self.clock),
        )
        .await?;
        Ok((session, worker))
    }

    /// A CP00 worker with the production seams on a new connection of its own through `gate`,
    /// which withholds every confirmation the broker sends it until the test settles it.
    async fn open_worker_through(&self, gate: &ConfirmGate) -> Result<(Session, Worker), BoxError> {
        let session = broker::connect(gate.amqp_url()).await?;
        let worker = Worker::open(
            session.connection(),
            &self.config,
            self.store(),
            Arc::clone(&self.clock),
        )
        .await?;
        Ok((session, worker))
    }

    /// Waits until the reply's confirmation has reached `gate`, keeps it withheld for [`HOLD`],
    /// and reads at its end the queues, whether `acct_id` has a committed summary, the committed
    /// log rows and, last, the calls `calls` recorded. Then marks `calls.confirm` and settles the
    /// confirmation with `settle`, whatever the reads returned, so that the worker is never left
    /// waiting for it.
    async fn hold_reply(
        &self,
        gate: &ConfirmGate,
        calls: &CallOrder,
        acct_id: PicU<11>,
        settle: fn(&ConfirmGate),
    ) -> Result<Held, BoxError> {
        let held = async {
            gate.wait_for_arrivals(1, WAIT).await?;
            sleep(HOLD).await;
            let replies = rabbitmq::message_count(&self.channel, &self.reply_queue).await?;
            let requests = rabbitmq::message_count(&self.channel, self.request_queue()).await?;
            let dead_letters =
                rabbitmq::message_count(&self.channel, broker::DEAD_LETTER_QUEUE).await?;
            let summary_committed = self.summary(acct_id).await?.is_some();
            let log_rows = self.log_rows().await?.len();
            Ok(Held {
                replies,
                requests,
                dead_letters,
                summary_committed,
                log_rows,
                made: calls.made(),
            })
        }
        .await;
        calls.mark(&calls.confirm);
        settle(gate);
        held
    }

    /// A store that fails as `fault` names, with a server error ([`Failure::Server`]), and
    /// records its calls in `calls`.
    fn injected(&self, fault: StoreFault, calls: &Arc<CallOrder>) -> InjectedStore {
        self.injected_failing(fault, Failure::Server, calls)
    }

    /// A store that fails as `fault` names, with the error of `failure`, and records its calls in
    /// `calls`.
    fn injected_failing(
        &self,
        fault: StoreFault,
        failure: Failure,
        calls: &Arc<CallOrder>,
    ) -> InjectedStore {
        InjectedStore {
            inner: self.store(),
            fault,
            failure,
            calls: Arc::clone(calls),
        }
    }

    /// A CP00 worker with the production seams and `clock` instead of the harness clock, on a
    /// new connection of its own: a clock that reads another instant gives the worker's details
    /// another key (COPAUA0C-099).
    async fn open_worker_at(&self, clock: Arc<dyn Clock>) -> Result<(Session, Worker), BoxError> {
        let session = broker::connect(self.broker.amqp_url()).await?;
        let worker = Worker::open(session.connection(), &self.config, self.store(), clock).await?;
        Ok((session, worker))
    }

    /// Publishes `body` to the request queue with `reply_to` and the distinct IDs `tag` names,
    /// and returns once the broker has confirmed it.
    async fn send(&self, body: &[u8], reply_to: &str, tag: &str) -> Result<Sent, BoxError> {
        let sent = Sent {
            message_id: format!("CP00-TEST-MSG-{tag}"),
            correlation_id: format!("CP00-TEST-CORR-{tag}"),
        };
        let properties = BasicProperties::default()
            .with_message_id(ShortString::from(sent.message_id.as_str()))
            .with_correlation_id(ShortString::from(sent.correlation_id.as_str()))
            .with_reply_to(ShortString::from(reply_to));
        rabbitmq::publish(&self.channel, self.request_queue(), body, properties).await?;
        Ok(sent)
    }

    /// The next reply on the test's reply queue, waiting at most [`WAIT`].
    async fn next_reply(&self) -> Result<BasicGetMessage, BoxError> {
        rabbitmq::get(&self.channel, &self.reply_queue, WAIT)
            .await?
            .ok_or_else(|| BoxError::from("no CP00 reply arrived on the reply queue"))
    }

    /// Every reply left on the reply queue, each taken within [`DRAIN`] of the one before.
    async fn remaining_replies(&self) -> Result<Vec<BasicGetMessage>, BoxError> {
        let mut replies = Vec::new();
        while let Some(reply) = rabbitmq::get(&self.channel, &self.reply_queue, DRAIN).await? {
            replies.push(reply);
        }
        Ok(replies)
    }

    /// The ready messages of `queue` once the count has reached `expected` and held it for
    /// [`SETTLE`], or after [`WAIT`] when it never does.
    async fn settled_count(&self, queue: &str, expected: u32) -> Result<u32, BoxError> {
        let deadline = Instant::now() + WAIT;
        let mut count = rabbitmq::message_count(&self.channel, queue).await?;
        while count != expected && Instant::now() < deadline {
            sleep(POLL).await;
            count = rabbitmq::message_count(&self.channel, queue).await?;
        }
        sleep(SETTLE).await;
        Ok(rabbitmq::message_count(&self.channel, queue).await?)
    }

    /// The `card_xref` row of the fixture account `acct_id`.
    async fn xref(&self, acct_id: u64) -> Result<CardXrefRecord, BoxError> {
        let mut store = self.store();
        CardXrefRepository::find_by_acct_id(&mut store, PicU::new(acct_id)?)
            .await?
            .ok_or_else(|| BoxError::from("a fixture account has no card_xref row"))
    }

    /// The summary of `acct_id`, if any.
    async fn summary(
        &self,
        acct_id: PicU<11>,
    ) -> Result<Option<PendingAuthSummaryRecord>, BoxError> {
        let mut store = self.store();
        Ok(PendingAuthSummaryRepository::find_by_id(&mut store, PicS::from(acct_id)).await?)
    }

    /// Every summary of every account.
    async fn summaries(&self) -> Result<Vec<PendingAuthSummaryRecord>, BoxError> {
        let mut store = self.store();
        Ok(PendingAuthSummaryRepository::find_all(&mut store).await?)
    }

    /// The details of `acct_id`, in key order.
    async fn details(&self, acct_id: PicU<11>) -> Result<Vec<PendingAuthDetailRecord>, BoxError> {
        let mut store = self.store();
        let acct = PicS::from(acct_id);
        Ok(PendingAuthDetailRepository::find_all_by_acct_id(&mut store, acct).await?)
    }

    /// Every `cssl_error_log` row, in write order.
    async fn log_rows(&self) -> Result<Vec<ErrorLogRecord>, BoxError> {
        let mut store = self.store();
        let rows = CsslErrorLogRepository::find_all(&mut store).await?;
        Ok(rows.into_iter().map(|(_, record)| record).collect())
    }

    /// The harness configuration with a [`RUN_POLL_MS`] poll interval, for `messaging::run`.
    fn run_config(&self) -> Config {
        Config {
            mq_poll_ms: RUN_POLL_MS,
            ..self.config.clone()
        }
    }

    /// Deletes `queue` from the broker, as an operator who has not loaded every definition
    /// leaves it missing.
    async fn delete_queue(&self, queue: &str) -> TestResult {
        let delete = self
            .channel
            .queue_delete(queue, QueueDeleteOptions::default());
        timeout(WAIT, delete).await??;
        Ok(())
    }

    /// Declares `queue` durable with no arguments, as `rabbitmq/definitions.json` declares
    /// `CARD.DEMO.ERROR`.
    async fn declare_durable(&self, queue: &str) -> TestResult {
        let options = QueueDeclareOptions {
            durable: true,
            ..QueueDeclareOptions::default()
        };
        let declare = self
            .channel
            .queue_declare(queue, options, FieldTable::default());
        timeout(WAIT, declare).await??;
        Ok(())
    }

    /// Runs `messaging::run` with `config` under `capture` beside `driver`, requests the stop
    /// once `driver` has ended, and returns the run's outcome, how long the run took to return
    /// after the stop request, and what `driver` returned. The whole run is bounded by
    /// [`RUN_BOUND`].
    async fn run_beside<T>(
        &self,
        config: &Config,
        capture: &TraceCapture,
        driver: impl Future<Output = Result<T, BoxError>>,
    ) -> Result<(Result<(), StartupError>, Duration, T), BoxError> {
        let handle = StopHandle::new();
        // Boxed: the three workers' futures exceed the stack budget of `clippy::large_futures`.
        let running = Box::pin(
            run(
                config,
                self.store(),
                Arc::clone(&self.clock),
                handle.subscribe(),
            )
            .with_subscriber(capture.dispatch()),
        );
        let returned = async {
            let outcome = timeout(RUN_BOUND, running).await;
            (outcome, Instant::now())
        };
        let driving = async {
            let finished = driver.await;
            handle.request();
            (finished, Instant::now())
        };
        let ((outcome, returned_at), (finished, stopped_at)) = tokio::join!(returned, driving);
        let outcome = outcome?;
        let finished = finished?;
        Ok((
            outcome,
            returned_at.saturating_duration_since(stopped_at),
            finished,
        ))
    }

    /// Closes the test's connection; the containers go with the harness.
    async fn finish(self) -> TestResult {
        timeout(WAIT, self.connection.close(REPLY_SUCCESS, CLOSE_REASON)).await??;
        Ok(())
    }
}

/// Runs `worker` until it takes a request, polling an empty queue for at most [`WAIT`], and
/// returns what it did with the request (COPAUA0C-001).
async fn process<W: Step>(worker: &mut W, session: &Session) -> Result<Processed, BoxError> {
    let deadline = Instant::now() + WAIT;
    loop {
        let processed = timeout(WAIT, worker.process_next(session.connection())).await??;
        if processed != Processed::Idle || Instant::now() >= deadline {
            return Ok(processed);
        }
        sleep(POLL).await;
    }
}

/// A local TCP port on which nothing listens: the operating system hands it to a listener that
/// is closed again at once, so a connection to it is refused.
fn closed_port() -> Result<u16, BoxError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// The lines of `lines` that contain `needle`.
fn matching<'a>(lines: &'a [String], needle: &str) -> Vec<&'a String> {
    lines.iter().filter(|line| line.contains(needle)).collect()
}

/// Waits until `capture` holds at least `expected` lines containing `needle`, polling every
/// [`POLL`] for at most [`WAIT`], and returns how many it holds then.
async fn captured(
    capture: &TraceCapture,
    needle: &str,
    expected: usize,
) -> Result<usize, BoxError> {
    let deadline = Instant::now() + WAIT;
    loop {
        let found = matching(&capture.lines(), needle).len();
        if found >= expected {
            return Ok(found);
        }
        if Instant::now() >= deadline {
            return Err(
                format!("{found} of {expected} \"{needle}\" events within {WAIT:?}").into(),
            );
        }
        sleep(POLL).await;
    }
}

/// Closes the worker's connection while the worker still holds its channels, then drops both:
/// the broker returns every request the worker left unsettled to its queue.
async fn close_worker<W>(session: Session, worker: W) {
    session.close(CLOSE_REASON).await;
    drop(worker);
    drop(session);
}

/// The one element of `rows`, or an error naming `what` and how many there were.
fn single<T>(rows: Vec<T>, what: &str) -> Result<T, BoxError> {
    let count = rows.len();
    let [row]: [T; 1] = rows
        .try_into()
        .map_err(|_| format!("expected exactly one {what}, found {count}"))?;
    Ok(row)
}

/// The properties every CP00 reply carries (COPAUA0C-067, E16): the request's `correlation_id`;
/// a non-empty `message_id` of `messaging::ids`, never the request's (`MQMI-NONE`); the
/// audited five-second expiry as `expiration` `"5000"`; `delivery_mode` 2; no `reply_to`. That
/// each reply's `message_id` is new, never another reply's, is [`assert_fresh_reply_ids`]'s.
fn assert_cp00_properties(
    reply: &BasicGetMessage,
    request_correlation_id: &str,
    request_message_id: &str,
) {
    let properties = &reply.delivery.properties;
    assert_eq!(
        text(properties.correlation_id().as_ref()),
        Some(request_correlation_id),
        "the reply carries the request's correlation_id"
    );
    let message_id = text(properties.message_id().as_ref());
    assert!(
        message_id.is_some_and(|id| !id.is_empty() && id != request_message_id),
        "the reply carries a new message_id, not the request's: {message_id:?}"
    );
    assert_eq!(
        text(properties.expiration().as_ref()),
        Some("5000"),
        "expiration, broker::CP00_REPLY_EXPIRATION"
    );
    assert_eq!(*properties.delivery_mode(), Some(2), "delivery_mode");
    assert!(
        properties.reply_to().is_none(),
        "the reply names no reply_to"
    );
}

/// The `message_id` of `reply`, kept for [`assert_fresh_reply_ids`]; an error when the reply
/// carries none.
fn reply_message_id(reply: &BasicGetMessage) -> Result<String, BoxError> {
    text(reply.delivery.properties.message_id().as_ref())
        .map(str::to_owned)
        .ok_or_else(|| BoxError::from("a CP00 reply carries no message_id"))
}

/// The `message_id`s of one test's replies, `reply_ids`, each newly generated for its reply
/// (COPAUA0C-067): no two are equal, so no ID served a second reply, and none is the
/// `message_id` of any of the test's `requests`.
fn assert_fresh_reply_ids<'a>(reply_ids: &[String], requests: impl IntoIterator<Item = &'a Sent>) {
    let request_ids: BTreeSet<&str> = requests
        .into_iter()
        .map(|sent| sent.message_id.as_str())
        .collect();
    let mut seen = BTreeSet::new();
    for id in reply_ids {
        assert!(
            seen.insert(id.as_str()),
            "two replies carry the message_id {id:?}"
        );
        assert!(
            !request_ids.contains(id.as_str()),
            "a reply carries a request's message_id {id:?}"
        );
    }
}

/// The text of a short-string property, `None` when absent.
fn text(property: Option<&ShortString>) -> Option<&str> {
    property.map(ShortString::as_str)
}

/// Whether `stored`, a `PIC X(n)` column, holds `sent` as the alphanumeric `MOVE` leaves it:
/// left-aligned and padded with spaces (COPAUA0C-003).
fn holds_moved(stored: &[u8], sent: &str) -> bool {
    stored
        .strip_prefix(sent.as_bytes())
        .is_some_and(|rest| rest.iter().all(|byte| *byte == b' '))
}

/// The request fields of a detail row (COPAUA0C-100), the amount excepted: each as its request
/// field moved it, the card number in full.
fn assert_request_echo(detail: &PendingAuthDetailRecord, card: &CardNumber, tran_id: &str) {
    let columns: [(&str, &[u8], &str); 15] = [
        ("auth_orig_date", detail.orig_date.as_bytes(), AUTH_DATE),
        ("auth_orig_time", detail.orig_time.as_bytes(), AUTH_TIME),
        ("auth_type", detail.auth_type.as_bytes(), AUTH_TYPE),
        (
            "card_expiry_date",
            detail.card_expiry_date.as_bytes(),
            CARD_EXPIRY,
        ),
        ("message_type", detail.message_type.as_bytes(), MESSAGE_TYPE),
        (
            "message_source",
            detail.message_source.as_bytes(),
            MESSAGE_SOURCE,
        ),
        (
            "merchant_catagory_code",
            detail.merchant_catagory_code.as_bytes(),
            MERCHANT_CATEGORY,
        ),
        (
            "acqr_country_code",
            detail.acqr_country_code.as_bytes(),
            ACQUIRER_COUNTRY,
        ),
        ("merchant_id", detail.merchant_id.as_bytes(), MERCHANT_ID),
        (
            "merchant_name",
            detail.merchant_name.as_bytes(),
            MERCHANT_NAME,
        ),
        (
            "merchant_city",
            detail.merchant_city.as_bytes(),
            MERCHANT_CITY,
        ),
        (
            "merchant_state",
            detail.merchant_state.as_bytes(),
            MERCHANT_STATE,
        ),
        ("merchant_zip", detail.merchant_zip.as_bytes(), MERCHANT_ZIP),
        ("transaction_id", detail.transaction_id.as_bytes(), tran_id),
        ("auth_id_code", detail.auth_id_code.as_bytes(), AUTH_TIME),
    ];
    for (column, stored, sent) in columns {
        assert!(
            holds_moved(stored, sent),
            "{column} holds its request field as moved"
        );
    }
    assert_eq!(detail.card_num, *card, "card_num is the request's card");
    assert_eq!(detail.processing_code.get(), 0, "processing_code");
    assert_eq!(detail.pos_entry_mode.get(), 5, "pos_entry_mode");
}

/// The decision a detail row records (COPAUA0C-044, COPAUA0C-101).
struct Decided {
    resp_code: &'static str,
    reason: &'static str,
    match_status: &'static str,
    approved_cents: i64,
}

/// The decision columns of a detail row: response code, reason, approved amount and match
/// status as `decided` gives them, `auth_fraud` one space and `fraud_rpt_date` eight spaces
/// (COPAUA0C-101).
fn assert_decided(detail: &PendingAuthDetailRecord, decided: &Decided) {
    assert!(
        holds_moved(detail.auth_resp_code.as_bytes(), decided.resp_code),
        "auth_resp_code {}",
        decided.resp_code
    );
    assert!(
        holds_moved(detail.auth_resp_reason.as_bytes(), decided.reason),
        "auth_resp_reason {}",
        decided.reason
    );
    assert!(
        holds_moved(detail.match_status.as_bytes(), decided.match_status),
        "match_status {}",
        decided.match_status
    );
    assert_eq!(
        detail.approved_amt.as_decimal(),
        Decimal::new(decided.approved_cents, 2),
        "approved_amt"
    );
    assert!(detail.auth_fraud.is_spaces(), "auth_fraud is one space");
    assert!(
        detail.fraud_rpt_date.is_spaces(),
        "fraud_rpt_date is eight spaces"
    );
}

/// Each named amount equals its value in cents.
fn assert_amounts(amounts: &[(&str, Decimal, i64)]) {
    for &(field, actual, cents) in amounts {
        assert_eq!(actual, Decimal::new(cents, 2), "{field}");
    }
}

/// What one `cssl_error_log` row must hold (COPAUA0C-110); `event_key` is `None` where the
/// audit fixes no value for the key.
struct LogExpectation<'a> {
    location: &'a str,
    level: &'a str,
    subsystem: &'a str,
    message: &'a str,
    event_key: Option<String>,
}

/// `row` holds `expected`, each text padded with spaces to its width: `err_application` `CP00`,
/// `err_program` `COPAUA0C`, and `err_date` and `err_time` six digits each, compared by format
/// only. The codes are HALTED(H4) and never compared.
fn assert_log_row(row: &ErrorLogRecord, expected: &LogExpectation<'_>) {
    let location = expected.location;
    let texts: [(&str, &[u8], &str); 6] = [
        ("err_location", row.location.as_bytes(), location),
        ("err_level", row.level.as_bytes(), expected.level),
        (
            "err_subsystem",
            row.subsystem.as_bytes(),
            expected.subsystem,
        ),
        ("err_message", row.message.as_bytes(), expected.message),
        ("err_application", row.application.as_bytes(), "CP00"),
        ("err_program", row.program.as_bytes(), "COPAUA0C"),
    ];
    for (column, stored, value) in texts {
        assert!(
            holds_moved(stored, value),
            "{location}: {column} is {value:?}"
        );
    }
    for (column, stored) in [
        ("err_date", row.date.as_bytes()),
        ("err_time", row.time.as_bytes()),
    ] {
        assert!(
            stored.iter().all(u8::is_ascii_digit),
            "{location}: {column} is six digits"
        );
    }
    if let Some(key) = &expected.event_key {
        assert_eq!(
            row.event_key,
            PicX::moved_from(key.as_bytes()),
            "{location}: event key"
        );
    }
}

/// The event key of a card-bearing row the audit fixes: twelve `*`, the card's last four digits,
/// then four spaces (E32).
fn redacted_key(card: &CardNumber) -> String {
    format!("{:<20}", card.redacted())
}

/// No event key of `rows` holds any of `cards` in full. The failure message never shows the key,
/// which would print the number it found.
fn assert_keys_redacted(rows: &[ErrorLogRecord], cards: &[String]) {
    for row in rows {
        let key = row.event_key.to_text();
        assert!(
            !cards.iter().any(|card| key.contains(card.as_str())),
            "an err_event_key holds a full card number"
        );
    }
}

/// The 50 fixture card numbers and `extra`, as needles for [`assert_keys_redacted`] only.
fn protected_cards(extra: &[&str]) -> Result<Vec<String>, BoxError> {
    let mut cards = fixture_card_numbers()?;
    cards.extend(extra.iter().map(|card| (*card).to_owned()));
    Ok(cards)
}

/// The order of the doubles' calls: each slot holds the 1-based position of the latest call it
/// names, 0 when none was made. `confirm` is the test's own: [`Harness::hold_reply`] marks it
/// when it settles the reply's withheld confirmation, just before the worker can learn of it.
#[derive(Debug, Default)]
struct CallOrder {
    next: AtomicUsize,
    xref: AtomicUsize,
    begin: AtomicUsize,
    lock: AtomicUsize,
    account: AtomicUsize,
    customer: AtomicUsize,
    confirm: AtomicUsize,
    ack: AtomicUsize,
    update: AtomicUsize,
    insert: AtomicUsize,
    commit: AtomicUsize,
    rollback: AtomicUsize,
    reject: AtomicUsize,
    log: AtomicUsize,
}

impl CallOrder {
    /// Records a call in `slot` at the next position.
    fn mark(&self, slot: &AtomicUsize) {
        let position = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        slot.store(position, Ordering::SeqCst);
    }

    /// The position `slot` recorded.
    fn at(slot: &AtomicUsize) -> usize {
        slot.load(Ordering::SeqCst)
    }

    /// The names of the slots recorded so far, in the order of their positions.
    fn made(&self) -> Vec<&'static str> {
        let mut made: Vec<(usize, &'static str)> = [
            (&self.xref, "card_xref read"),
            (&self.begin, "begin"),
            (&self.lock, "summary lock"),
            (&self.account, "account read"),
            (&self.customer, "customer read"),
            (&self.confirm, "confirmation"),
            (&self.ack, "acknowledgement"),
            (&self.update, "summary update"),
            (&self.insert, "detail insert"),
            (&self.commit, "commit"),
            (&self.rollback, "rollback"),
            (&self.reject, "rejection"),
            (&self.log, "cssl_error_log write"),
        ]
        .into_iter()
        .map(|(slot, name)| (Self::at(slot), name))
        .filter(|(position, _)| *position > 0)
        .collect();
        made.sort_unstable();
        made.into_iter().map(|(_, name)| name).collect()
    }
}

/// What [`Harness::hold_reply`] read at the end of the hold of a reply's confirmation.
#[derive(Debug, PartialEq, Eq)]
struct Held {
    /// Ready messages on the test's reply queue.
    replies: u32,
    /// Ready messages on the request queue.
    requests: u32,
    /// Ready messages on `CARD.DEMO.DEADLETTER.PAUTH`.
    dead_letters: u32,
    /// Whether the account had a committed summary.
    summary_committed: bool,
    /// Committed `cssl_error_log` rows.
    log_rows: usize,
    /// The calls the doubles had recorded, in order ([`CallOrder::made`]).
    made: Vec<&'static str>,
}

impl Held {
    /// The state while a found card's reply is unconfirmed: the reply on its queue, the request
    /// neither requeued nor dead-lettered, nothing committed or logged, and no call made after
    /// the locking reads (COPAUA0C-081).
    fn unconfirmed() -> Self {
        Self {
            replies: 1,
            requests: 0,
            dead_letters: 0,
            summary_committed: false,
            log_rows: 0,
            made: LOCKING_READS.to_vec(),
        }
    }
}

/// The one store call an [`InjectedStore`] fails, or [`StoreFault::Healthy`] for none. A failing
/// call returns the error of the store's [`Failure`]; a `*Missing` read finds no row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreFault {
    Healthy,
    XrefRead,
    Begin,
    SummaryLock,
    AccountRead,
    CustomerRead,
    AccountMissing,
    CustomerMissing,
    SummaryUpdate,
    Commit,
}

/// The error a [`StoreFault`] call fails with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// The database answered with an error, [`PersistenceError::Database`] with SQLSTATE
    /// `XX000` (`internal_error`), so the worker still writes the call's log row. The codes it
    /// leaves in the row are HALTED(H4) and never compared.
    Server,
    /// [`PersistenceError::Unavailable`]: the database cannot be reached, as when the pool's
    /// acquire times out.
    Unavailable,
}

impl Failure {
    /// The error a failing call returns.
    fn error(self) -> PersistenceError {
        match self {
            Self::Server => PersistenceError::Database {
                sqlstate: Some("XX000".to_owned()),
                constraint: None,
            },
            Self::Unavailable => PersistenceError::Unavailable,
        }
    }
}

/// The real store with one injected fault: every other call goes to `inner`.
struct InjectedStore {
    inner: PgStore,
    fault: StoreFault,
    failure: Failure,
    calls: Arc<CallOrder>,
}

impl AuthStore for InjectedStore {
    type Tx = InjectedTx;

    async fn find_xref(
        &self,
        card: &CardNumber,
    ) -> Result<Option<CardXrefRecord>, PersistenceError> {
        self.calls.mark(&self.calls.xref);
        if self.fault == StoreFault::XrefRead {
            return Err(self.failure.error());
        }
        AuthStore::find_xref(&self.inner, card).await
    }

    async fn begin(&self) -> Result<InjectedTx, PersistenceError> {
        self.calls.mark(&self.calls.begin);
        if self.fault == StoreFault::Begin {
            return Err(self.failure.error());
        }
        let inner = AuthStore::begin(&self.inner).await?;
        Ok(InjectedTx {
            inner,
            fault: self.fault,
            failure: self.failure,
            calls: Arc::clone(&self.calls),
        })
    }

    async fn log(&self, record: &ErrorLogRecord) -> Result<i64, PersistenceError> {
        self.calls.mark(&self.calls.log);
        AuthStore::log(&self.inner, record).await
    }
}

/// The real authorization transaction with the fault and failure of the [`InjectedStore`] that
/// opened it.
struct InjectedTx {
    inner: PgTx<'static>,
    fault: StoreFault,
    failure: Failure,
    calls: Arc<CallOrder>,
}

impl AuthTx for InjectedTx {
    async fn lock_or_create_summary(
        &mut self,
        template: &PendingAuthSummaryRecord,
    ) -> Result<(PendingAuthSummaryRecord, bool), PersistenceError> {
        self.calls.mark(&self.calls.lock);
        if self.fault == StoreFault::SummaryLock {
            return Err(self.failure.error());
        }
        AuthTx::lock_or_create_summary(&mut self.inner, template).await
    }

    async fn find_account(
        &mut self,
        id: PicU<11>,
    ) -> Result<Option<AccountRecord>, PersistenceError> {
        self.calls.mark(&self.calls.account);
        match self.fault {
            StoreFault::AccountRead => Err(self.failure.error()),
            StoreFault::AccountMissing => Ok(None),
            _ => AuthTx::find_account(&mut self.inner, id).await,
        }
    }

    async fn find_customer(
        &mut self,
        id: PicU<9>,
    ) -> Result<Option<CustomerRecord>, PersistenceError> {
        self.calls.mark(&self.calls.customer);
        match self.fault {
            StoreFault::CustomerRead => Err(self.failure.error()),
            StoreFault::CustomerMissing => Ok(None),
            _ => AuthTx::find_customer(&mut self.inner, id).await,
        }
    }

    async fn update_summary(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> Result<bool, PersistenceError> {
        self.calls.mark(&self.calls.update);
        if self.fault == StoreFault::SummaryUpdate {
            return Err(self.failure.error());
        }
        AuthTx::update_summary(&mut self.inner, record).await
    }

    async fn insert_detail(
        &mut self,
        acct_id: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> Result<(), PersistenceError> {
        self.calls.mark(&self.calls.insert);
        AuthTx::insert_detail(&mut self.inner, acct_id, record).await
    }

    async fn commit(self) -> Result<(), PersistenceError> {
        self.calls.mark(&self.calls.commit);
        if self.fault == StoreFault::Commit {
            // A refused commit ends the transaction rolled back, so nothing it wrote stands.
            AuthTx::rollback(self.inner).await?;
            return Err(self.failure.error());
        }
        AuthTx::commit(self.inner).await
    }

    async fn rollback(self) -> Result<(), PersistenceError> {
        self.calls.mark(&self.calls.rollback);
        AuthTx::rollback(self.inner).await
    }
}

/// Stands in for the consumer channel's settlement and settles nothing: the broker returns the
/// request to its queue when the worker's connection closes. `ack` fails when `ack_fails`, as an
/// acknowledgement on a dead delivery tag does (COPAUA0C-118).
struct StandInAcknowledger {
    ack_fails: bool,
    calls: Arc<CallOrder>,
}

impl StandInAcknowledger {
    fn new(ack_fails: bool, calls: &Arc<CallOrder>) -> Self {
        Self {
            ack_fails,
            calls: Arc::clone(calls),
        }
    }
}

impl Acknowledger for StandInAcknowledger {
    async fn ack(&self, _delivery_tag: u64) -> Result<(), AmqpError> {
        self.calls.mark(&self.calls.ack);
        if self.ack_fails {
            Err(AmqpError::Ack(Fault::Unclassified))
        } else {
            Ok(())
        }
    }

    async fn reject_without_requeue(&self, _delivery_tag: u64) -> Result<(), AmqpError> {
        self.calls.mark(&self.calls.reject);
        Ok(())
    }
}

// audit: COPAUA0C-001
// audit: COPAUA0C-020
// audit: COPAUA0C-022
// audit: COPAUA0C-023
// audit: COPAUA0C-024
// audit: COPAUA0C-025
// audit: COPAUA0C-027
// audit: COPAUA0C-042
// audit: COPAUA0C-044
// audit: COPAUA0C-046
// audit: COPAUA0C-066
// audit: COPAUA0C-067
// audit: COPAUA0C-090
// audit: COPAUA0C-092
// audit: COPAUA0C-094
// audit: COPAUA0C-097
// audit: COPAUA0C-100
// audit: COPAUA0C-101
// audit: COPAUA0C-103
#[tokio::test]
async fn approval_creates_summary_and_pending_detail() -> TestResult {
    let harness = Harness::start().await?;
    // Account 1: balance 194.00, credit limit 2020.00, cash limit 1020.00, no summary, so
    // 100.00 is within 2020.00 − 194.00 and the guard's 194.00 + 0.00 + 100.00.
    let xref = harness.xref(1).await?;
    let tran_id = "CP00T01 0000001";
    let body = auth_request(card_text(&xref.card_num)?, "100.00", tran_id);
    let sent = harness
        .send(&body, &harness.reply_queue, "approval")
        .await?;

    let (session, mut worker) = harness.open_worker().await?;
    assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);

    let summary = harness
        .summary(xref.acct_id)
        .await?
        .ok_or("the approval left no summary")?;
    assert_eq!(summary.acct_id, PicS::from(xref.acct_id), "acct_id");
    assert_eq!(summary.cust_id, xref.cust_id, "cust_id");
    assert_eq!(summary.approved_auth_cnt.get(), 1, "approved_auth_cnt");
    assert_eq!(summary.declined_auth_cnt.get(), 0, "declined_auth_cnt");
    assert_amounts(&[
        ("credit_limit", summary.credit_limit.as_decimal(), 202_000),
        ("cash_limit", summary.cash_limit.as_decimal(), 102_000),
        (
            "credit_balance",
            summary.credit_balance.as_decimal(),
            10_000,
        ),
        ("cash_balance", summary.cash_balance.as_decimal(), 0),
        (
            "approved_auth_amt",
            summary.approved_auth_amt.as_decimal(),
            10_000,
        ),
        (
            "declined_auth_amt",
            summary.declined_auth_amt.as_decimal(),
            0,
        ),
    ]);

    let detail = single(harness.details(xref.acct_id).await?, "detail row")?;
    assert_decided(&detail, &APPROVED);
    assert_request_echo(&detail, &xref.card_num, tran_id);
    assert_amounts(&[(
        "transaction_amt",
        detail.transaction_amt.as_decimal(),
        10_000,
    )]);
    assert!(
        harness.log_rows().await?.is_empty(),
        "an approval logs no cssl_error_log row"
    );

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the approved request was acknowledged");
    harness.finish().await
}

// audit: COPAUA0C-042
// audit: COPAUA0C-044
// audit: COPAUA0C-045
// audit: COPAUA0C-067
// audit: COPAUA0C-090
// audit: COPAUA0C-095
// audit: COPAUA0C-101
// audit: COPAUA0C-103
#[tokio::test]
async fn insufficient_funds_decline_counts_declined() -> TestResult {
    let harness = Harness::start().await?;
    // Account 1's available amount is 2020.00 − 194.00 = 1826.00, so 5000.00 is declined for
    // insufficient funds with the account and the customer found: `05`/`4100`.
    let xref = harness.xref(1).await?;
    let tran_id = "CP00T02 0000001";
    let body = auth_request(card_text(&xref.card_num)?, "5000.00", tran_id);
    let sent = harness.send(&body, &harness.reply_queue, "decline").await?;

    let (session, mut worker) = harness.open_worker().await?;
    assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);

    // `declined_auth_amt` is HALTED(H2) (COPAUA0C-096) and is not read.
    let summary = harness
        .summary(xref.acct_id)
        .await?
        .ok_or("the decline left no summary")?;
    assert_eq!(summary.declined_auth_cnt.get(), 1, "declined_auth_cnt");
    assert_eq!(summary.approved_auth_cnt.get(), 0, "approved_auth_cnt");
    assert_amounts(&[
        ("credit_limit", summary.credit_limit.as_decimal(), 202_000),
        ("cash_limit", summary.cash_limit.as_decimal(), 102_000),
        ("credit_balance", summary.credit_balance.as_decimal(), 0),
        ("cash_balance", summary.cash_balance.as_decimal(), 0),
        (
            "approved_auth_amt",
            summary.approved_auth_amt.as_decimal(),
            0,
        ),
    ]);

    let declined = Decided {
        resp_code: "05",
        reason: "4100",
        match_status: "D",
        approved_cents: 0,
    };
    let detail = single(harness.details(xref.acct_id).await?, "detail row")?;
    assert_decided(&detail, &declined);
    assert_request_echo(&detail, &xref.card_num, tran_id);
    assert!(
        harness.log_rows().await?.is_empty(),
        "a decline logs no cssl_error_log row"
    );

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the declined request was acknowledged");
    harness.finish().await
}

// audit: COPAUA0C-021
// audit: COPAUA0C-067
// audit: COPAUA0C-082
// audit: COPAUA0C-110
// audit: COPAUA0C-114
#[tokio::test]
async fn card_not_in_xref_logs_a001_without_writes() -> TestResult {
    let harness = Harness::start().await?;
    let card = CardNumber::parse(UNKNOWN_CARD)?;
    let mut store = harness.store();
    let found = CardXrefRepository::find_by_id(&mut store, &card).await?;
    assert!(found.is_none(), "the unknown card is absent from card_xref");
    let body = auth_request(UNKNOWN_CARD, "100.00", "CP00T03 0000001");
    let sent = harness.send(&body, &harness.reply_queue, "no-xref").await?;

    let (session, mut worker) = harness.open_worker().await?;
    assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);

    // No summary at all, and so no detail, which the foreign key keeps under a summary.
    assert!(
        harness.summaries().await?.is_empty(),
        "a card with no card_xref row writes no summary and no detail"
    );
    let rows = harness.log_rows().await?;
    assert_keys_redacted(&rows, &protected_cards(&[UNKNOWN_CARD])?);
    let row = single(rows, "cssl_error_log row")?;
    let a001 = LogExpectation {
        location: "A001",
        level: "W",
        subsystem: "A",
        message: "CARD NOT FOUND IN XREF",
        event_key: Some(redacted_key(&card)),
    };
    assert_log_row(&row, &a001);

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the answered request was acknowledged");
    harness.finish().await
}

// audit: COPAUA0C-066
// audit: COPAUA0C-110
// audit: COPAUA0C-117
#[tokio::test]
async fn m004_unroutable_reply_dead_letters_request() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T04 0000001");
    harness.send(&body, NO_SUCH_REPLY_QUEUE, "m004").await?;

    let (session, mut worker) = harness.open_worker().await?;
    assert_eq!(
        process(&mut worker, &session).await?,
        Processed::DeadLettered
    );

    // The rollback removed the summary `lock_or_create` had created, so neither table changed.
    assert!(
        harness.summary(xref.acct_id).await?.is_none(),
        "no summary after M004"
    );
    assert!(
        harness.details(xref.acct_id).await?.is_empty(),
        "no detail after M004"
    );
    // The event key is COPAUA0C-113's interim value and the codes HALTED(H4): neither is
    // compared, and the key is only checked for a full card number.
    let rows = harness.log_rows().await?;
    assert_keys_redacted(&rows, &protected_cards(&[])?);
    let row = single(rows, "cssl_error_log row")?;
    let m004 = LogExpectation {
        location: "M004",
        level: "C",
        subsystem: "M",
        message: "FAILED TO PUT ON REPLY MQ",
        event_key: None,
    };
    assert_log_row(&row, &m004);

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the request was rejected, not requeued");
    let dead = harness.settled_count(broker::DEAD_LETTER_QUEUE, 1).await?;
    assert_eq!(
        dead, 1,
        "the rejected request is retained on the dead-letter queue"
    );
    harness.finish().await
}

// audit: COPAUA0C-001
// audit: COPAUA0C-067
// audit: COPAUA0C-081
// audit: COPAUA0C-118
#[tokio::test]
async fn failed_first_ack_writes_nothing_and_redelivery_is_processed_once() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T05 0000001");
    let sent = harness
        .send(&body, &harness.reply_queue, "ack-fails")
        .await?;

    // First delivery: the reply is confirmed, then the acknowledgement fails, so the worker
    // rolls back and closes its consumer channel, and the broker requeues the request.
    let calls = Arc::new(CallOrder::default());
    let (first_session, first) = harness.open_worker().await?;
    let mut first = first.with_acknowledger(StandInAcknowledger::new(true, &calls));
    assert_eq!(
        process(&mut first, &first_session).await?,
        Processed::Requeued
    );
    // The reply confirmed before the failed acknowledgement, taken at once, inside its
    // five-second expiry.
    let first_reply = harness.next_reply().await?;
    assert_cp00_properties(&first_reply, &sent.correlation_id, &sent.message_id);
    assert!(
        CallOrder::at(&calls.ack) > 0,
        "the acknowledgement was tried"
    );
    assert_eq!(
        CallOrder::at(&calls.reject),
        0,
        "the request was not rejected"
    );
    assert!(
        harness.summary(xref.acct_id).await?.is_none(),
        "no summary before a successful acknowledgement"
    );
    assert!(
        harness.details(xref.acct_id).await?.is_empty(),
        "no detail before a successful acknowledgement"
    );
    assert!(
        harness.log_rows().await?.is_empty(),
        "a confirmed reply logs no M004"
    );
    close_worker(first_session, first).await;
    let requeued = harness.settled_count(harness.request_queue(), 1).await?;
    assert_eq!(
        requeued, 1,
        "the broker requeued the unacknowledged request"
    );

    // Redelivery: processed once, as a new request. Each delivery was answered once, the
    // first by the reply confirmed before its acknowledgement failed (AAP 0.3.4.2), and each
    // reply carries a message_id of its own.
    let (second_session, mut second) = harness.open_worker().await?;
    assert_eq!(
        process(&mut second, &second_session).await?,
        Processed::Replied
    );
    let second_reply = harness.next_reply().await?;
    assert_cp00_properties(&second_reply, &sent.correlation_id, &sent.message_id);
    let reply_ids = [
        reply_message_id(&first_reply)?,
        reply_message_id(&second_reply)?,
    ];
    assert_fresh_reply_ids(&reply_ids, [&sent]);
    assert!(
        harness.remaining_replies().await?.is_empty(),
        "one reply per delivery: the redelivery was processed once"
    );
    let summary = harness
        .summary(xref.acct_id)
        .await?
        .ok_or("the redelivered approval left no summary")?;
    assert_eq!(summary.approved_auth_cnt.get(), 1, "approved_auth_cnt");
    assert_amounts(&[(
        "approved_auth_amt",
        summary.approved_auth_amt.as_decimal(),
        10_000,
    )]);
    let detail = single(harness.details(xref.acct_id).await?, "detail row")?;
    assert_decided(&detail, &APPROVED);
    close_worker(second_session, second).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the redelivered request was acknowledged");
    harness.finish().await
}

// audit: COPAUA0C-041
// audit: COPAUA0C-046
// audit: COPAUA0C-049
// audit: COPAUA0C-067
// audit: COPAUA0C-094
// audit: COPAUA0C-095
// audit: COPAUA0C-099
// audit: COPAUA0C-110
// audit: COPAUA0C-120
#[tokio::test]
async fn two_approvals_same_instant_commit_summary_and_log_i004() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let card = card_text(&xref.card_num)?;
    let first_tran_id = "CP00T06 0000001";
    let first_sent = harness
        .send(
            &auth_request(card, "100.00", first_tran_id),
            &harness.reply_queue,
            "same-instant-1",
        )
        .await?;
    let second_sent = harness
        .send(
            &auth_request(card, "100.00", "CP00T06 0000002"),
            &harness.reply_queue,
            "same-instant-2",
        )
        .await?;

    // The second decision takes the found-summary path: 2020.00 − 100.00 available, and the
    // guard's 194.00 + 100.00 + 100.00 is within the limit. Under one clock instant both
    // details get the same key, so the second insert fails. The summary then counts 200.00
    // and the details 100.00, and the third decision, `summary_guard_declines_past_the_details`
    // after these assertions, tells the two apart: only a guard over the summary declines it.
    let (session, mut worker) = harness.open_worker().await?;
    let mut reply_ids = Vec::new();
    for sent in [&first_sent, &second_sent] {
        assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
        let reply = harness.next_reply().await?;
        assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);
        reply_ids.push(reply_message_id(&reply)?);
    }

    let summary = harness
        .summary(xref.acct_id)
        .await?
        .ok_or("the approvals left no summary")?;
    assert_eq!(summary.approved_auth_cnt.get(), 2, "approved_auth_cnt");
    assert_amounts(&[
        (
            "approved_auth_amt",
            summary.approved_auth_amt.as_decimal(),
            20_000,
        ),
        (
            "credit_balance",
            summary.credit_balance.as_decimal(),
            20_000,
        ),
    ]);
    let detail = single(harness.details(xref.acct_id).await?, "detail row")?;
    assert_decided(&detail, &APPROVED);
    let row = single(harness.log_rows().await?, "cssl_error_log row")?;
    let i004 = LogExpectation {
        location: "I004",
        level: "C",
        subsystem: "I",
        message: "IMS INSERT DETL FAILED",
        event_key: Some(redacted_key(&xref.card_num)),
    };
    assert_log_row(&row, &i004);

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "both requests were acknowledged and none requeued");

    let (third_sent, third_reply_id) =
        summary_guard_declines_past_the_details(&harness, &xref, first_tran_id, &i004).await?;
    // Two replies under one clock instant and a third under another: three message_ids.
    reply_ids.push(third_reply_id);
    assert_fresh_reply_ids(&reply_ids, [&first_sent, &second_sent, &third_sent]);
    harness.finish().await
}

/// The third decision of [`two_approvals_same_instant_commit_summary_and_log_i004`], once its
/// `I004` has left the summary counting 200.00 and the details 100.00: 1700.00 for `xref`'s
/// card under a clock one second later, so that its detail takes a key of its own
/// (COPAUA0C-099). COPAUA0C's rule approves it, 2020.00 − 200.00 = 1820.00 being available
/// (COPAUA0C-041), and so would a guard over the details, 194.00 + 100.00 + 1700.00 = 1994.00;
/// the guard over the summary, which still counts the approval whose detail failed, declines it
/// `05`/`4100`, as 194.00 + 200.00 + 1700.00 = 2094.00 exceeds 2020.00 (COPAUA0C-046,
/// COPAUA0C-049).
///
/// Asserts the reply's properties; the summary's approved count, approved amount and credit
/// balance unchanged and one decline counted (COPAUA0C-095); the decline's detail beside the
/// detail of `approved_tran_id`, still approved; `i004` still the only log row; and the request
/// acknowledged. Returns the request and its reply's `message_id`.
async fn summary_guard_declines_past_the_details(
    harness: &Harness,
    xref: &CardXrefRecord,
    approved_tran_id: &str,
    i004: &LogExpectation<'_>,
) -> Result<(Sent, String), BoxError> {
    let tran_id = "CP00T06 0000003";
    let body = auth_request(card_text(&xref.card_num)?, "1700.00", tran_id);
    let sent = harness
        .send(&body, &harness.reply_queue, "later-instant")
        .await?;
    let later = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 11, 13, 345))
        .ok_or("the later instant is not a valid date and time")?;
    let (session, mut worker) = harness
        .open_worker_at(Arc::new(FixedClock::new(later)))
        .await?;
    // Boxed: the step's future, kept inline beside this helper's own state, would grow the
    // helper's future past `clippy::large_futures`.
    assert_eq!(
        Box::pin(process(&mut worker, &session)).await?,
        Processed::Replied
    );
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);
    let reply_id = reply_message_id(&reply)?;

    // `declined_auth_amt` is HALTED(H2) (COPAUA0C-096) and is not read.
    let summary = harness
        .summary(xref.acct_id)
        .await?
        .ok_or("the decline left no summary")?;
    assert_eq!(
        summary.approved_auth_cnt.get(),
        2,
        "approved_auth_cnt after the guard decline"
    );
    assert_eq!(
        summary.declined_auth_cnt.get(),
        1,
        "declined_auth_cnt after the guard decline"
    );
    assert_amounts(&[
        (
            "approved_auth_amt after the guard decline",
            summary.approved_auth_amt.as_decimal(),
            20_000,
        ),
        (
            "credit_balance after the guard decline",
            summary.credit_balance.as_decimal(),
            20_000,
        ),
    ]);
    let details = harness.details(xref.acct_id).await?;
    assert_eq!(
        details.len(),
        2,
        "the surviving approval's detail and the decline's"
    );
    let detail_of = |tran_id: &str| {
        details
            .iter()
            .find(|detail| holds_moved(detail.transaction_id.as_bytes(), tran_id))
            .ok_or_else(|| BoxError::from(format!("no detail row for transaction {tran_id}")))
    };
    let guard_declined = Decided {
        resp_code: "05",
        reason: "4100",
        match_status: "D",
        approved_cents: 0,
    };
    let declined = detail_of(tran_id)?;
    assert_decided(declined, &guard_declined);
    assert_request_echo(declined, &xref.card_num, tran_id);
    assert_amounts(&[(
        "transaction_amt of the decline",
        declined.transaction_amt.as_decimal(),
        170_000,
    )]);
    assert_decided(detail_of(approved_tran_id)?, &APPROVED);
    let row = single(
        harness.log_rows().await?,
        "cssl_error_log row after the guard decline",
    )?;
    assert_log_row(&row, i004);

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the declined request was acknowledged");
    Ok((sent, reply_id))
}

// audit: COPAUA0C-067
// audit: COPAUA0C-110
// audit: COPAUA0C-119
#[tokio::test]
async fn summary_write_failure_logs_i003_and_writes_nothing() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T07 0000001");
    let sent = harness.send(&body, &harness.reply_queue, "i003").await?;

    let calls = Arc::new(CallOrder::default());
    let (session, worker) = harness.open_worker().await?;
    let mut worker = worker.with_store(harness.injected(StoreFault::SummaryUpdate, &calls));
    assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);

    // The rollback also undid the summary `lock_or_create` inserted, and no detail was tried.
    assert!(
        harness.summary(xref.acct_id).await?.is_none(),
        "no summary after I003"
    );
    assert!(
        harness.details(xref.acct_id).await?.is_empty(),
        "no detail after I003"
    );
    assert_eq!(
        CallOrder::at(&calls.insert),
        0,
        "the detail insert was not attempted"
    );
    // The event key is COPAUA0C-113's interim value: only checked for a full card number.
    let rows = harness.log_rows().await?;
    assert_keys_redacted(&rows, &protected_cards(&[])?);
    let row = single(rows, "cssl_error_log row")?;
    let i003 = LogExpectation {
        location: "I003",
        level: "C",
        subsystem: "I",
        message: "IMS UPDATE SUMRY FAILED",
        event_key: None,
    };
    assert_log_row(&row, &i003);

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the request was acknowledged and not requeued");
    harness.finish().await
}

// audit: COPAUA0C-020
// audit: COPAUA0C-025
// audit: COPAUA0C-067
// audit: COPAUA0C-080
// audit: COPAUA0C-081
#[tokio::test]
async fn writes_follow_the_acknowledgement_inside_the_locking_transaction() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T08 0000001");
    let sent = harness.send(&body, &harness.reply_queue, "order").await?;

    let calls = Arc::new(CallOrder::default());
    let gate = harness.broker.confirm_gate().await?;
    let (session, worker) = harness.open_worker_through(&gate).await?;
    let mut worker = worker
        .with_store(harness.injected(StoreFault::Healthy, &calls))
        .with_acknowledger(StandInAcknowledger::new(false, &calls));
    // The worker answers while the test holds the reply's confirmation, then releases it.
    let held = harness.hold_reply(&gate, &calls, xref.acct_id, ConfirmGate::release);
    let (processed, held) = tokio::join!(process(&mut worker, &session), held);
    // Queue presence is not confirmation: the reply was on its queue throughout the hold.
    assert_eq!(
        held?,
        Held::unconfirmed(),
        "while the reply's confirmation was held: the reply queued after the locking reads, \
         and nothing settled, written, logged or committed"
    );
    assert_eq!(processed?, Processed::Replied);
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);

    // The cross-reference read, the transaction, the summary lock, the account and customer
    // reads, then the reply's confirmation, the acknowledgement of the confirmed reply, and
    // only then the writes and the commit.
    let order = [
        ("card_xref read", CallOrder::at(&calls.xref)),
        ("begin", CallOrder::at(&calls.begin)),
        ("summary lock", CallOrder::at(&calls.lock)),
        ("account read", CallOrder::at(&calls.account)),
        ("customer read", CallOrder::at(&calls.customer)),
        ("confirmation", CallOrder::at(&calls.confirm)),
        ("acknowledgement", CallOrder::at(&calls.ack)),
        ("summary update", CallOrder::at(&calls.update)),
        ("detail insert", CallOrder::at(&calls.insert)),
        ("commit", CallOrder::at(&calls.commit)),
    ];
    for pair in order.windows(2) {
        if let [(earlier, before), (later, after)] = pair {
            assert!(
                *before > 0 && before < after,
                "{earlier} comes before {later}: {order:?}"
            );
        }
    }
    assert_eq!(CallOrder::at(&calls.rollback), 0, "nothing was rolled back");
    assert_eq!(CallOrder::at(&calls.reject), 0, "nothing was rejected");
    let summary = harness
        .summary(xref.acct_id)
        .await?
        .ok_or("the committed approval left no summary")?;
    assert_eq!(summary.approved_auth_cnt.get(), 1, "approved_auth_cnt");
    let detail = single(harness.details(xref.acct_id).await?, "detail row")?;
    assert_decided(&detail, &APPROVED);

    // The stand-in settled nothing, so the request returns to its queue with the connection.
    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 1).await?;
    assert_eq!(
        left, 1,
        "only the stand-in acknowledger settled the request"
    );
    harness.finish().await
}

// audit: COPAUA0C-066
// audit: COPAUA0C-110
// audit: COPAUA0C-117
#[tokio::test]
async fn refused_reply_confirmation_logs_m004_writes_nothing_and_dead_letters() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T10 0000001");
    harness.send(&body, &harness.reply_queue, "refused").await?;

    // The consumer channel settles the request; the healthy store only records its calls.
    let calls = Arc::new(CallOrder::default());
    let gate = harness.broker.confirm_gate().await?;
    let (session, worker) = harness.open_worker_through(&gate).await?;
    let mut worker = worker.with_store(harness.injected(StoreFault::Healthy, &calls));
    // The worker answers while the test holds the reply's confirmation, then refuses it.
    let held = harness.hold_reply(&gate, &calls, xref.acct_id, ConfirmGate::refuse);
    let (processed, held) = tokio::join!(process(&mut worker, &session), held);
    // The broker queued the reply and only the worker was told Nack, so the reply is on the
    // reply queue: a refused confirmation, not an unroutable reply.
    assert_eq!(
        held?,
        Held::unconfirmed(),
        "while the reply's confirmation was held: no row, no write, the request unsettled"
    );
    // Only Ack(None) is a confirmed reply: the refused one takes the M004 path.
    assert_eq!(processed?, Processed::DeadLettered);
    assert!(
        CallOrder::at(&calls.confirm) < CallOrder::at(&calls.log),
        "the M004 row follows the refused confirmation"
    );

    // The rollback removed the summary `lock_or_create` had created, so neither table changed.
    assert!(
        harness.summary(xref.acct_id).await?.is_none(),
        "no summary after a refused confirmation"
    );
    assert!(
        harness.details(xref.acct_id).await?.is_empty(),
        "no detail after a refused confirmation"
    );
    for (call, slot) in [
        ("summary update", &calls.update),
        ("detail insert", &calls.insert),
        ("commit", &calls.commit),
    ] {
        assert_eq!(
            CallOrder::at(slot),
            0,
            "{call} after a refused confirmation"
        );
    }
    assert!(
        CallOrder::at(&calls.rollback) > 0,
        "the authorization transaction was rolled back"
    );
    // The event key is COPAUA0C-113's interim value and the codes HALTED(H4): neither is
    // compared, and the key is only checked for a full card number.
    let rows = harness.log_rows().await?;
    assert_keys_redacted(&rows, &protected_cards(&[])?);
    let row = single(rows, "cssl_error_log row")?;
    let m004 = LogExpectation {
        location: "M004",
        level: "C",
        subsystem: "M",
        message: "FAILED TO PUT ON REPLY MQ",
        event_key: None,
    };
    assert_log_row(&row, &m004);

    // Rejected without requeue on the consumer channel, never acknowledged: dead-lettered.
    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the request was rejected, not requeued");
    let dead = harness.settled_count(broker::DEAD_LETTER_QUEUE, 1).await?;
    assert_eq!(
        dead, 1,
        "the rejected request is retained on the dead-letter queue"
    );
    harness.finish().await
}

// audit: COPAUA0C-067
// audit: COPAUA0C-121
#[tokio::test]
async fn failed_commit_leaves_the_confirmed_reply_unrecorded() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T09 0000001");
    let sent = harness.send(&body, &harness.reply_queue, "commit").await?;

    let calls = Arc::new(CallOrder::default());
    let (session, worker) = harness.open_worker().await?;
    let mut worker = worker.with_store(harness.injected(StoreFault::Commit, &calls));
    assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);

    assert!(CallOrder::at(&calls.commit) > 0, "the commit was tried");
    assert!(
        harness.summary(xref.acct_id).await?.is_none(),
        "no summary after a failed commit"
    );
    assert!(
        harness.details(xref.acct_id).await?.is_empty(),
        "no detail after a failed commit"
    );
    assert!(
        harness.log_rows().await?.is_empty(),
        "a failed commit logs no cssl_error_log row"
    );

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the acknowledged request is not redelivered");
    harness.finish().await
}

/// Where the event key of an asserted log row comes from (COPAUA0C-110 to COPAUA0C-126).
#[derive(Debug, Clone, Copy)]
enum KeySource {
    /// The request's card, redacted (E32).
    Card,
    /// The cross-reference's account ID, its 11 digits.
    Account,
    /// The cross-reference's customer ID, its nine digits.
    Customer,
    /// No value the audit fixes: HALTED(X-COPAUA0C-H1) for `I001`, COPAUA0C-113's interim key
    /// for `I002`.
    Unasserted,
}

impl KeySource {
    /// The key a row of this source holds for a request on `xref`'s card, padded to 20.
    fn expected(self, xref: &CardXrefRecord) -> Option<String> {
        let key = match self {
            Self::Card => return Some(redacted_key(&xref.card_num)),
            Self::Account => format!("{:011}", xref.acct_id.get()),
            Self::Customer => format!("{:09}", xref.cust_id.get()),
            Self::Unasserted => return None,
        };
        Some(format!("{key:<20}"))
    }
}

/// One injected store fault and the `cssl_error_log` row it must leave.
struct FaultRow {
    fault: StoreFault,
    location: &'static str,
    level: &'static str,
    subsystem: &'static str,
    message: &'static str,
    key: KeySource,
}

impl FaultRow {
    fn expectation(&self, xref: &CardXrefRecord) -> LogExpectation<'static> {
        LogExpectation {
            location: self.location,
            level: self.level,
            subsystem: self.subsystem,
            message: self.message,
            event_key: self.key.expected(xref),
        }
    }
}

/// The five store reads whose failure ends the COBOL task at a critical row, each with that row
/// (COPAUA0C-122 to COPAUA0C-126).
fn critical_reads() -> [FaultRow; 5] {
    let critical = |fault, location, subsystem, message, key| FaultRow {
        fault,
        location,
        level: "C",
        subsystem,
        message,
        key,
    };
    [
        critical(
            StoreFault::XrefRead,
            "C001",
            "C",
            "FAILED TO READ XREF FILE",
            KeySource::Card,
        ),
        critical(
            StoreFault::Begin,
            "I001",
            "I",
            "IMS SCHD FAILED",
            KeySource::Unasserted,
        ),
        critical(
            StoreFault::SummaryLock,
            "I002",
            "I",
            "IMS GET SUMMARY FAILED",
            KeySource::Unasserted,
        ),
        critical(
            StoreFault::AccountRead,
            "C002",
            "C",
            "FAILED TO READ ACCT FILE",
            KeySource::Account,
        ),
        critical(
            StoreFault::CustomerRead,
            "C003",
            "C",
            "FAILED TO READ CUST FILE",
            KeySource::Customer,
        ),
    ]
}

/// Sends one request for `xref`'s card, with field 18 `tran_id`, to a worker whose store fails
/// `case`'s read with `failure`, and asserts the interim handling of `X-COPAUA0C-1`, not a
/// parity result: one reply carrying the CP00 properties, the request acknowledged and not
/// dead-lettered, and neither a summary nor a detail for the account. The reply's `05`/`9000`
/// bytes are the domain's unit test's, as no CP00 body is compared here (HALTED(H6)). Returns
/// the store's calls, the request and its reply's `message_id`.
async fn declined_after_failed_read(
    harness: &Harness,
    xref: &CardXrefRecord,
    case: &FaultRow,
    failure: Failure,
    tran_id: &str,
) -> Result<(Arc<CallOrder>, Sent, String), BoxError> {
    let location = case.location;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", tran_id);
    let sent = harness.send(&body, &harness.reply_queue, location).await?;
    let calls = Arc::new(CallOrder::default());
    let (session, worker) = harness.open_worker().await?;
    let mut worker = worker.with_store(harness.injected_failing(case.fault, failure, &calls));
    // Boxed: the step's future, kept inline beside this helper's own state, would grow the
    // helper's future past `clippy::large_futures`.
    assert_eq!(
        Box::pin(process(&mut worker, &session)).await?,
        Processed::Replied,
        "{location}: the failed read is answered"
    );
    let reply = harness.next_reply().await?;
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);
    let reply_id = reply_message_id(&reply)?;
    assert!(
        harness.remaining_replies().await?.is_empty(),
        "{location}: exactly one reply"
    );

    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "{location}: the request was acknowledged");
    let dead = harness.settled_count(broker::DEAD_LETTER_QUEUE, 0).await?;
    assert_eq!(dead, 0, "{location}: the request was not dead-lettered");
    assert!(
        harness.summary(xref.acct_id).await?.is_none(),
        "{location}: no summary after a failed read"
    );
    assert!(
        harness.details(xref.acct_id).await?.is_empty(),
        "{location}: no detail after a failed read"
    );
    Ok((calls, sent, reply_id))
}

// audit: COPAUA0C-067
// audit: COPAUA0C-110
// audit: COPAUA0C-122
// audit: COPAUA0C-123
// audit: COPAUA0C-124
// audit: COPAUA0C-125
// audit: COPAUA0C-126
#[tokio::test]
async fn store_read_failures_log_their_critical_rows_and_decline_without_writes() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let cards = protected_cards(&[])?;
    let mut requests = Vec::new();
    let mut reply_ids = Vec::new();
    for (written, case) in critical_reads().iter().enumerate() {
        let tran_id = format!("CP00T10 000000{written}");
        let (calls, sent, reply_id) =
            declined_after_failed_read(&harness, &xref, case, Failure::Server, &tran_id).await?;
        requests.push(sent);
        reply_ids.push(reply_id);
        assert!(
            CallOrder::at(&calls.log) > 0,
            "{}: the row was written",
            case.location
        );

        let rows = harness.log_rows().await?;
        assert_keys_redacted(&rows, &cards);
        assert_eq!(
            rows.len(),
            written + 1,
            "{}: one row per failure",
            case.location
        );
        let row = rows.last().ok_or("no cssl_error_log row")?;
        assert_log_row(row, &case.expectation(&xref));
    }
    assert_fresh_reply_ids(&reply_ids, &requests);
    harness.finish().await
}

/// The text of the `ERROR` event that stands for a `cssl_error_log` row not written.
const ROW_NOT_WRITTEN: &str =
    "the cssl_error_log row is not written because the database is unavailable";

/// How many captured lines report a `cssl_error_log` row not written.
fn rows_not_written(capture: &TraceCapture) -> usize {
    capture
        .lines()
        .iter()
        .filter(|line| line.contains(ROW_NOT_WRITTEN))
        .count()
}

// audit: COPAUA0C-067
// audit: COPAUA0C-122
// audit: COPAUA0C-123
// audit: COPAUA0C-124
// audit: COPAUA0C-125
// audit: COPAUA0C-126
#[tokio::test]
async fn an_unavailable_database_declines_the_request_and_attempts_no_log_row() -> TestResult {
    let harness = Harness::start().await?;
    let xref = harness.xref(1).await?;
    let capture = trace_capture::capture();
    let mut requests = Vec::new();
    let mut reply_ids = Vec::new();
    for (nth, case) in critical_reads().iter().enumerate() {
        let location = case.location;
        let tran_id = format!("CP00T12 000000{nth}");
        capture.clear();
        let (calls, sent, reply_id) =
            declined_after_failed_read(&harness, &xref, case, Failure::Unavailable, &tran_id)
                .await?;
        requests.push(sent);
        reply_ids.push(reply_id);
        assert_eq!(
            CallOrder::at(&calls.log),
            0,
            "{location}: no cssl_error_log write was attempted"
        );
        assert_eq!(
            rows_not_written(&capture),
            1,
            "{location}: one ERROR event stands for the row"
        );
        assert!(
            !capture.contains("the cssl_error_log row could not be written"),
            "{location}: no write failed"
        );
    }
    assert_fresh_reply_ids(&reply_ids, &requests);
    assert!(
        harness.log_rows().await?.is_empty(),
        "an unavailable database leaves no cssl_error_log row"
    );

    // A decline that cannot be delivered takes the M004 path, and its row is skipped too.
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T12 0000009");
    harness.send(&body, NO_SUCH_REPLY_QUEUE, "m004").await?;
    let calls = Arc::new(CallOrder::default());
    let (session, worker) = harness.open_worker().await?;
    let mut worker = worker.with_store(harness.injected_failing(
        StoreFault::XrefRead,
        Failure::Unavailable,
        &calls,
    ));
    capture.clear();
    assert_eq!(
        process(&mut worker, &session).await?,
        Processed::DeadLettered
    );
    assert_eq!(
        CallOrder::at(&calls.log),
        0,
        "neither the C001 nor the M004 row was attempted"
    );
    assert_eq!(
        rows_not_written(&capture),
        2,
        "one ERROR event stands for each of the C001 and M004 rows"
    );
    close_worker(session, worker).await;
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the request was rejected, not requeued");
    let dead = harness.settled_count(broker::DEAD_LETTER_QUEUE, 1).await?;
    assert_eq!(
        dead, 1,
        "the undelivered decline's request is retained on the dead-letter queue"
    );
    assert!(
        harness.log_rows().await?.is_empty(),
        "no cssl_error_log row after the M004 path either"
    );
    harness.finish().await
}

// audit: COPAUA0C-043
// audit: COPAUA0C-067
// audit: COPAUA0C-103
// audit: COPAUA0C-110
// audit: COPAUA0C-115
// audit: COPAUA0C-116
#[tokio::test]
async fn missing_account_or_customer_logs_a_warning_and_processing_goes_on() -> TestResult {
    let harness = Harness::start().await?;
    let warning = |fault, location, message, key| FaultRow {
        fault,
        location,
        level: "W",
        subsystem: "A",
        message,
        key,
    };
    // Account 2 (no summary, account missing): declined with reason 3100. Account 3
    // (balance 147.00, limit 4909.00, customer missing): 100.00 is approved.
    let declined = Decided {
        resp_code: "05",
        reason: "3100",
        match_status: "D",
        approved_cents: 0,
    };
    let cases = [
        (
            2,
            warning(
                StoreFault::AccountMissing,
                "A002",
                "ACCT NOT FOUND IN XREF",
                KeySource::Account,
            ),
            declined,
        ),
        (
            3,
            warning(
                StoreFault::CustomerMissing,
                "A003",
                "CUST NOT FOUND IN XREF",
                KeySource::Customer,
            ),
            APPROVED,
        ),
    ];
    let mut requests = Vec::new();
    let mut reply_ids = Vec::new();
    for (written, (acct_id, case, decided)) in cases.iter().enumerate() {
        let xref = harness.xref(*acct_id).await?;
        let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T11 0000001");
        let sent = harness
            .send(&body, &harness.reply_queue, case.location)
            .await?;
        let calls = Arc::new(CallOrder::default());
        let (session, worker) = harness.open_worker().await?;
        let mut worker = worker.with_store(harness.injected(case.fault, &calls));
        assert_eq!(process(&mut worker, &session).await?, Processed::Replied);
        let reply = harness.next_reply().await?;
        assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);
        reply_ids.push(reply_message_id(&reply)?);
        requests.push(sent);

        let rows = harness.log_rows().await?;
        assert_eq!(
            rows.len(),
            written + 1,
            "{}: one warning row",
            case.location
        );
        let row = rows.last().ok_or("no cssl_error_log row")?;
        assert_log_row(row, &case.expectation(&xref));
        // The warning lets the request go on to its summary update and detail insert.
        assert!(
            harness.summary(xref.acct_id).await?.is_some(),
            "{}: the summary was written",
            case.location
        );
        let detail = single(harness.details(xref.acct_id).await?, "detail row")?;
        assert_decided(&detail, decided);

        close_worker(session, worker).await;
        let left = harness.settled_count(harness.request_queue(), 0).await?;
        assert_eq!(left, 0, "{}: the request was acknowledged", case.location);
    }
    assert_fresh_reply_ids(&reply_ids, &requests);
    harness.finish().await
}

/// `messaging::run` started while no broker listens at `CARDDEMO_AMQP_URL` keeps running
/// instead of ending the process: the first attempt fails at once, and it and every reconnect
/// after it is logged and retried after a backoff delay. A stop then ends the run with `Ok`,
/// although it never connected. No event shows the URL or its password, only the value-free
/// failure.
#[tokio::test]
async fn run_retries_a_refused_first_connection_until_it_is_stopped() -> TestResult {
    let harness = Harness::start().await?;
    // A password assembled at run time, in a URL whose port nothing listens on.
    let password = ["no-broker", "-", "secret"].concat();
    let amqp_url = format!(
        "amqp://carddemo:{password}@127.0.0.1:{}/%2f",
        closed_port()?
    );
    let config = Config {
        amqp_url: amqp_url.clone(),
        ..harness.run_config()
    };
    let capture = trace_capture::capture();
    let driver = captured(&capture, CONNECT_RETRIED, 3);
    let (outcome, stop_took, retried) = harness.run_beside(&config, &capture, driver).await?;
    let lines = capture.lines();
    drop(capture);

    if let Err(error) = outcome {
        return Err(format!("run ended with {error} instead of retrying").into());
    }
    assert!(
        stop_took < RUN_RETURN_BOUND,
        "run returned {stop_took:?} after the stop"
    );
    assert!(retried >= 3, "{retried} failed attempts were logged");
    let first = matching(&lines, r#""outcome":"connect failed""#);
    assert_eq!(first.len(), 1, "the first attempt logs one event");
    let later = matching(&lines, r#""outcome":"reconnect failed""#);
    assert!(later.len() >= 2, "{} reconnect events", later.len());
    for line in first.iter().chain(&later) {
        assert!(line.contains(r#""level":"ERROR""#), "{line}");
        assert!(line.contains("cannot connect to the AMQP broker"), "{line}");
    }
    assert!(
        matching(&lines, WORKERS_CONSUME).is_empty(),
        "no worker opened without a broker"
    );
    for line in &lines {
        assert!(!line.contains(&amqp_url), "an event shows the AMQP URL");
        assert!(
            !line.contains(&password),
            "an event shows the AMQP password"
        );
    }
    harness.finish().await
}

/// A queue missing from the broker when `messaging::run` starts, here `CARD.DEMO.ERROR`, which
/// CDRD checks first, does not end the run: each session whose workers cannot open is logged,
/// closed and replaced after a backoff delay, and no worker declares the queue. Once the
/// operator declares it as `rabbitmq/definitions.json` does, the next session opens the three
/// workers, and CP00 answers a request and acknowledges it.
#[tokio::test]
async fn run_opens_the_workers_once_a_missing_queue_is_declared() -> TestResult {
    let harness = Harness::start().await?;
    // Account 1 approves 100.00 (see `approval_creates_summary_and_pending_detail`).
    let xref = harness.xref(1).await?;
    let body = auth_request(card_text(&xref.card_num)?, "100.00", "CP00T20 0000001");
    harness.delete_queue(ERROR_QUEUE).await?;
    let config = harness.run_config();
    let capture = trace_capture::capture();
    let driver = async {
        let failed = captured(&capture, OPEN_RETRIED, 2).await?;
        harness.declare_durable(ERROR_QUEUE).await?;
        captured(&capture, WORKERS_CONSUME, 1).await?;
        let sent = harness
            .send(&body, &harness.reply_queue, "run-reopened")
            .await?;
        let reply = harness.next_reply().await?;
        Ok::<_, BoxError>((failed, sent, reply))
    };
    let (outcome, stop_took, (failed, sent, reply)) =
        harness.run_beside(&config, &capture, driver).await?;
    let lines = capture.lines();
    drop(capture);

    if let Err(error) = outcome {
        return Err(format!("run ended with {error} instead of retrying").into());
    }
    assert!(
        stop_took < RUN_RETURN_BOUND,
        "run returned {stop_took:?} after the stop"
    );
    assert!(failed >= 2, "{failed} failed sessions were logged");
    for line in matching(&lines, OPEN_RETRIED) {
        assert!(line.contains(r#""level":"ERROR""#), "{line}");
        assert!(line.contains(r#""outcome":"open failed""#), "{line}");
        assert!(
            line.contains(
                "passive declare of queue CARD.DEMO.ERROR failed: AMQP soft error NOT-FOUND \
                 (reply code 404)"
            ),
            "{line}"
        );
    }
    assert_eq!(
        matching(&lines, WORKERS_CONSUME).len(),
        1,
        "the workers opened once, on the first session after the declare"
    );
    assert_cp00_properties(&reply, &sent.correlation_id, &sent.message_id);
    let amqp_url = harness.broker.amqp_url();
    assert!(
        lines.iter().all(|line| !line.contains(amqp_url)),
        "an event shows the AMQP URL"
    );
    let left = harness.settled_count(harness.request_queue(), 0).await?;
    assert_eq!(left, 0, "the answered request was acknowledged");
    harness.finish().await
}

/// A configuration error ends `messaging::run` at once with that error, because no retry cures
/// it: a `CARDDEMO_AMQP_URL` that does not parse, before any connection, and a 49-byte
/// `CARDDEMO_QUEUE_REQUEST_ACCT`, which CDRA refuses on the live broker because its error body
/// names the request queue in 48 bytes. Neither is retried, and neither the error nor any event
/// shows the configured value.
#[tokio::test]
async fn run_returns_a_configuration_error_without_retrying() -> TestResult {
    let harness = Harness::start().await?;
    let invalid_url = "amqp:carddemo-invalid-url".to_owned();
    let long_queue = "Q".repeat(49);
    let cases: [(Config, &str, IsExpected); 2] = [
        (
            Config {
                amqp_url: invalid_url.clone(),
                ..harness.run_config()
            },
            invalid_url.as_str(),
            |error| matches!(error, StartupError::Connect(AmqpError::InvalidUrl)),
        ),
        (
            Config {
                queue_request_acct: long_queue.clone(),
                ..harness.run_config()
            },
            long_queue.as_str(),
            |error| {
                matches!(
                    error,
                    StartupError::Declare(AmqpError::QueueName {
                        variable: "CARDDEMO_QUEUE_REQUEST_ACCT",
                        max_len: 48,
                    })
                )
            },
        ),
    ];
    for (config, value, expected) in cases {
        let capture = trace_capture::capture();
        // Held until `run` returns: dropping every handle would request the stop.
        let handle = StopHandle::new();
        let running = Box::pin(
            run(
                &config,
                harness.store(),
                Arc::clone(&harness.clock),
                handle.subscribe(),
            )
            .with_subscriber(capture.dispatch()),
        );
        let outcome = timeout(RUN_RETURN_BOUND, running).await?;
        drop(handle);
        let lines = capture.lines();
        drop(capture);

        let Err(error) = outcome else {
            return Err(format!("run returned Ok with {value:?} configured").into());
        };
        assert!(
            expected(&error),
            "unexpected error for {value:?}: {error:?}"
        );
        let shown = format!("{error} | {error:?}");
        assert!(!shown.contains(value), "the error shows {value:?}: {shown}");
        for needle in [CONNECT_RETRIED, OPEN_RETRIED, WORKERS_CONSUME] {
            assert!(
                matching(&lines, needle).is_empty(),
                "{value:?}: \"{needle}\" was logged"
            );
        }
        assert!(
            lines.iter().all(|line| !line.contains(value)),
            "an event shows {value:?}"
        );
    }
    harness.finish().await
}

// audit: COPAUA0C-065
#[test]
#[ignore = "BLOCKED: HALTED(H6) every CP00 reply body, its 64-byte length and the never-written \
            byte at offset 63; no parity assertion (AAP 0.8.5)"]
fn blocked_h6_reply_body() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COPAUA0C-065 (H6): every CP00 reply body is 64 bytes long, and its byte at \
         offset 63 comes from W02-PUT-BUFFER, which has no VALUE and is never written, so no \
         reply body has a single COBOL value"
            .into(),
    )
}

// audit: COPAUA0C-040
// audit: COPAUA0C-091
// audit: COPAUA0C-093
// audit: COPAUA0C-096
// audit: COPAUA0C-102
// audit: COPAUA0C-111
// audit: COPAUA0C-113
#[test]
#[ignore = "BLOCKED: HALTED(H2) declined amounts, the detail FILLER and the storage that a \
            task's first message reads before assigning it (the candidates X-HALTS-1, \
            X-COPAUA0C-H1, X-COPAUA0C-H2, X-ARITH-8 and E34 proposed to join H2); no parity \
            assertion (AAP 0.8.5)"]
fn blocked_h2_uninitialised_storage() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COPAUA0C-040, COPAUA0C-091, COPAUA0C-093, COPAUA0C-096, COPAUA0C-102, \
         COPAUA0C-111 and COPAUA0C-113 (H2): the declined amounts, the detail FILLER and the \
         storage a task's first message reads before assigning it depend on runtime storage \
         initialisation, which no repository source sets"
            .into(),
    )
}

// audit: COPAUA0C-131
#[test]
#[ignore = "BLOCKED: HALTED(H1) replies and decisions after the first message of one CICS \
            trigger task; no parity assertion (AAP 0.8.5)"]
fn blocked_h1_later_messages_of_a_task() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COPAUA0C-131 (H1): the bytes, length and decision of every reply after a \
         task's first message depend on how many messages the CICS trigger task processed \
         before it, which RabbitMQ does not expose"
            .into(),
    )
}

// audit: COPAUA0C-112
#[test]
#[ignore = "BLOCKED: HALTED(H4) the CICS, MQ and IMS codes of ERR-CODE-1 and ERR-CODE-2; no \
            parity assertion (AAP 0.8.5)"]
fn blocked_h4_error_codes() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COPAUA0C-112 (H4): CICS, MQ and IMS define the codes of ERR-CODE-1 and \
         ERR-CODE-2, and PostgreSQL and AMQP have no counterpart for them"
            .into(),
    )
}
