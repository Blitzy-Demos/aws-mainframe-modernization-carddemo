//! CDRD, the port of `CODATE01`, against a real broker: a `rabbitmq:3.13-management`
//! testcontainer booted from `modern/rabbitmq/definitions.json` through `test_support::rabbitmq`,
//! with the CDRD worker running in process (g3a; `modern/audit/messaging/CODATE01.md`). No
//! database is involved.
//!
//! Each test starts its own broker, because some delete a queue to make a publication fail, and
//! gives the worker a connection of its own, apart from the one the test publishes and reads on.
//! Every broker wait is bounded.
//!
//! What is asserted:
//!
//! - The reply's bytes, destination and properties as published (CODATE01-002 to CODATE01-007,
//!   CODATE01-010, CODATE01-011), the date and the time by format only, never by value.
//! - Reply bytes that do not depend on the request body, and IDs copied from each reply's own
//!   request (CODATE01-001, CODATE01-013).
//! - The acknowledgement only after the reply is confirmed (CODATE01-012, the request override).
//!   A reply on its queue is not a confirmed one, as the broker queues a message before its
//!   confirmation reaches the publisher, so the worker connects through a
//!   `test_support::rabbitmq::ConfirmGate` that withholds the broker's confirmation for
//!   [`HOLD`]. While it is held, the reply is on `CARD.DEMO.REPLY.DATE` and the request is
//!   neither acknowledged nor rejected; the acknowledgement comes once the gate has handed the
//!   confirmation on. A confirmation the gate refuses, a negative one, is a failed put: the
//!   request is acknowledged only after its `MQPUT ERR` message is confirmed (CODATE01-014).
//! - The failure paths and how their request is settled (CODATE01-014, CODATE01-015,
//!   CODATE01-018, CODATE01-019).
//!
//! What is reported `BLOCKED` by an ignored test and asserted nowhere:
//!
//! - The MQ condition and reason codes of the error body, HALTED(H4) (CODATE01-016,
//!   CODATE01-017).
//! - Whether a confirmed reply is delivered and its request removed, which the halt candidate
//!   X-CODATE01-H2 blocks for CODATE01-001, CODATE01-012 and CODATE01-013 until the reviewer folds
//!   it in or rejects it. No test here therefore checks that the request queue is empty after a
//!   reply; the failure paths, which the candidate leaves assertable, are checked that way.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use chrono::NaiveDate;
use domain::messaging::codate01::{
    BUFFER_LEN, ERROR_QUEUE, MQ_ERR_DISPLAY_LEN, REPLY_QUEUE, REPLY_TEXT_LEN,
};
use domain::{Clock, FixedClock};
use insta::internals::SnapshotContents;
use lapin::message::Delivery;
use lapin::options::{ConfirmSelectOptions, QueueDeclareOptions, QueueDeleteOptions};
use lapin::types::{FieldTable, ShortString};
use lapin::{BasicProperties, Channel, Connection};
use messaging::broker::{Acknowledger, AmqpError, PUBLISH_DEADLINE};
use messaging::workers::cdrd::Worker;
use messaging::workers::{Processed, Step};
use runtime::config::Config;
use test_support::rabbitmq::{self, ConfirmGate, RabbitMqGuard};
use test_support::trace_capture;
use tokio::time::{Instant, sleep, timeout};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

/// Bound on one worker operation: opening the worker, one `process_next`, one reopen. The worker
/// bounds each broker call itself, so this only keeps a stuck test from hanging the run.
const STEP_BOUND: Duration = Duration::from_secs(30);

/// How long a reply or error message may take to appear on its queue.
const GET_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a queue count may take to reach its expected value.
const COUNT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a request queue must stay empty once the worker's consumer channel has closed. The
/// broker returns an unsettled request within milliseconds of that close, as
/// [`ack_follows_confirmed_reply`] observes.
const SETTLE_WINDOW: Duration = Duration::from_secs(2);

/// Interval between queue counts.
const POLL: Duration = Duration::from_millis(100);

/// How long the confirmation gate withholds a confirmation that has reached it before the test
/// reads the queues and the acknowledger. A quarter of the worker's [`PUBLISH_DEADLINE`] at
/// most, so that the publication and its released confirmation still finish within it.
const HOLD: Duration = Duration::from_secs(1);
const _: () = assert!(
    HOLD.as_millis() * 4 <= PUBLISH_DEADLINE.as_millis(),
    "HOLD must stay well within PUBLISH_DEADLINE"
);

/// AMQP 0-9-1 `reply-success`, sent when the test closes the worker's connection.
const REPLY_SUCCESS: u16 = 200;

/// `delivery_mode` of a persistent message, the request's override for every CDRD publication.
const PERSISTENT: u8 = 2;

/// The 46 reply characters of CODATE01-003 to CODATE01-006, `N` standing for one ASCII digit:
/// `SYSTEM DATE : `, the date as `MM-DD-YYYY`, `SYSTEM TIME : ` and the time as `HH:MM:SS`.
const REPLY_TEXT_MASK: &[u8; REPLY_TEXT_LEN] = b"SYSTEM DATE : NN-NN-NNNNSYSTEM TIME : NN:NN:NN";

/// One test's broker: the test's connection and its channel in confirm mode, so that a request
/// is on its queue before the worker's next get, the worker's own connection, the worker
/// configuration and, dropped last, the container guard.
struct Harness {
    channel: Channel,
    /// The connection that carries `channel`, kept open with it.
    _connection: Connection,
    worker_connection: Connection,
    config: Config,
    guard: RabbitMqGuard,
}

impl Harness {
    async fn start() -> TestResult<Self> {
        let guard = broker().await?;
        let connection = guard.connect().await?;
        let channel = bounded("open the test channel", connection.create_channel()).await?;
        bounded(
            "put the test channel in confirm mode",
            channel.confirm_select(ConfirmSelectOptions::default()),
        )
        .await?;
        let worker_connection = guard.connect().await?;
        let config = Config {
            amqp_url: guard.amqp_url().to_owned(),
            ..Config::default()
        };
        Ok(Self {
            channel,
            _connection: connection,
            worker_connection,
            config,
            guard,
        })
    }

    /// The configured request queue, `CARD.DEMO.REQUEST.DATE` by default.
    fn request_queue(&self) -> &str {
        &self.config.queue_request_date
    }

    /// A CDRD worker on the worker's connection, reading the fixed clock.
    async fn worker(&self) -> TestResult<Worker> {
        let open = Worker::open(&self.worker_connection, &self.config, fixed_clock()?);
        bounded("open the CDRD worker", open).await
    }

    /// A confirmation gate in front of the broker, a connection of the worker's own through it,
    /// and a CDRD worker on that connection that reads the fixed clock and settles through a
    /// [`RecordingAcknowledger`] filling `record`.
    async fn gated_worker(
        &self,
        record: &Arc<Record>,
    ) -> TestResult<(ConfirmGate, Connection, Worker<RecordingAcknowledger>)> {
        let gate = self.guard.confirm_gate().await?;
        let connection = gate.connect().await?;
        let open = Worker::open(&connection, &self.config, fixed_clock()?);
        let worker = bounded("open the CDRD worker through the gate", open).await?;
        let double = RecordingAcknowledger {
            gate: gate.clone(),
            record: Arc::clone(record),
        };
        Ok((gate, connection, worker.with_acknowledger(double)))
    }

    /// Waits until `count` confirmations have reached `gate`, keeps the last of them withheld
    /// for [`HOLD`], and reads at its end what `queue` holds and what `record` has seen. The
    /// held confirmation is then settled with `settle` whatever the reads returned, so that the
    /// worker is never left waiting for it.
    async fn hold(
        &self,
        gate: &ConfirmGate,
        record: &Record,
        count: usize,
        queue: &str,
        settle: fn(&ConfirmGate),
    ) -> TestResult<Held> {
        let held = async {
            gate.wait_for_arrivals(count, GET_TIMEOUT).await?;
            sleep(HOLD).await;
            let queued = rabbitmq::message_count(&self.channel, queue).await?;
            Ok(Held {
                queued,
                passed: gate.passed(),
                acks: record.acks.load(Ordering::SeqCst),
                rejected: record.rejected.load(Ordering::SeqCst),
            })
        }
        .await;
        settle(gate);
        held
    }

    /// Publishes a request with `body` and `properties` and waits for the broker's confirmation.
    async fn publish_request(&self, body: &[u8], properties: BasicProperties) -> TestResult {
        rabbitmq::publish(&self.channel, self.request_queue(), body, properties).await?;
        Ok(())
    }

    /// The next message on `queue`, taken with `no_ack`, so the test leaves no delivery unsettled.
    async fn take(&self, queue: &str) -> TestResult<Delivery> {
        let message = rabbitmq::get(&self.channel, queue, GET_TIMEOUT)
            .await?
            .ok_or_else(|| format!("no message on {queue}"))?;
        Ok(message.delivery)
    }

    /// Waits until `queue` holds `expected` ready messages.
    async fn wait_for_count(&self, queue: &str, expected: u32) -> TestResult {
        let deadline = Instant::now() + COUNT_TIMEOUT;
        loop {
            let count = rabbitmq::message_count(&self.channel, queue).await?;
            if count == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("{queue} holds {count} messages, not {expected}").into());
            }
            sleep(POLL).await;
        }
    }

    /// Checks that the request queue holds no message throughout [`SETTLE_WINDOW`]: a request
    /// the worker left unsettled would be back on it by then.
    async fn assert_request_queue_stays_empty(&self) -> TestResult {
        let queue = self.request_queue();
        let deadline = Instant::now() + SETTLE_WINDOW;
        loop {
            let count = rabbitmq::message_count(&self.channel, queue).await?;
            if count != 0 {
                return Err(
                    format!("{queue} holds {count} messages: a request was requeued").into(),
                );
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
            sleep(POLL).await;
        }
    }

    /// Deletes `queue`, one of the queues the worker checked when it opened, so that the
    /// worker's `mandatory` publication to it is returned.
    async fn delete_queue(&self, queue: &str) -> TestResult {
        let delete = self
            .channel
            .queue_delete(queue, QueueDeleteOptions::default());
        bounded("delete a queue", delete).await?;
        Ok(())
    }
}

/// The broker container, removed when the returned guard drops at the end of the test.
async fn broker() -> TestResult<RabbitMqGuard> {
    Ok(rabbitmq::start_rabbitmq().await?)
}

/// The worker's clock, stopped at 2022-07-18 10:11:12.345, so that every reply of a test is
/// built from the same instant.
fn fixed_clock() -> TestResult<Arc<dyn Clock>> {
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|day| day.and_hms_milli_opt(10, 11, 12, 345))
        .ok_or("2022-07-18 10:11:12.345 is not a valid instant")?;
    Ok(Arc::new(FixedClock::new(at)))
}

/// `operation`, failed when it does not finish within [`STEP_BOUND`].
async fn bounded<T, E>(what: &str, operation: impl Future<Output = Result<T, E>>) -> TestResult<T>
where
    E: Into<Box<dyn Error + Send + Sync>>,
{
    let Ok(outcome) = timeout(STEP_BOUND, operation).await else {
        let seconds = STEP_BOUND.as_secs();
        return Err(format!("{what} did not finish within {seconds} s").into());
    };
    outcome.map_err(Into::into)
}

/// Runs `test` to completion on a current-thread runtime with the I/O and time drivers, the
/// runtime Tokio's test attribute builds, without the lint attributes, `expect` and unchecked
/// pinning that the attribute's expansion carries. A runtime that cannot be built fails the test.
/// The test future is boxed, so that it never sits on the stack.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(Box::pin(test))
}

/// Drives `first` and `second` concurrently on the calling task until both have finished, and
/// returns both outputs. Every poll polls each future that has not finished yet, and a finished
/// one is never polled again. Both are boxed, so that they never move once polled.
async fn join<A: Future, B: Future>(first: A, second: B) -> (A::Output, B::Output) {
    let mut first = Box::pin(first);
    let mut second = Box::pin(second);
    let mut first_output = None;
    let mut second_output = None;
    std::future::poll_fn(|context| {
        if first_output.is_none() {
            if let Poll::Ready(output) = first.as_mut().poll(context) {
                first_output = Some(output);
            }
        }
        if second_output.is_none() {
            if let Poll::Ready(output) = second.as_mut().poll(context) {
                second_output = Some(output);
            }
        }
        match (first_output.take(), second_output.take()) {
            (Some(first_done), Some(second_done)) => Poll::Ready((first_done, second_done)),
            (first_left, second_left) => {
                first_output = first_left;
                second_output = second_left;
                Poll::Pending
            }
        }
    })
    .await
}

/// One `process_next` of `worker` on `connection`. Its future is boxed because it holds the
/// whole put path, too large to keep inline in every test's future (`clippy::large_futures`).
async fn step<W: Step>(worker: &mut W, connection: &Connection) -> TestResult<Processed> {
    bounded(
        "process the next CDRD request",
        Box::pin(Step::process_next(worker, connection)),
    )
    .await
}

/// Request properties carrying `message_id` and `correlation_id`.
fn request_ids(message_id: &str, correlation_id: &str) -> BasicProperties {
    BasicProperties::default()
        .with_message_id(ShortString::from(message_id))
        .with_correlation_id(ShortString::from(correlation_id))
}

/// The `len` bytes of `bytes` from `start`.
fn segment(bytes: &[u8], start: usize, len: usize) -> TestResult<&[u8]> {
    let end = start.checked_add(len).ok_or("segment end overflows")?;
    bytes
        .get(start..end)
        .ok_or_else(|| format!("no {len} bytes at offset {start}").into())
}

fn all_spaces(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == b' ')
}

/// `text` followed by spaces to `width` bytes.
fn padded(text: &str, width: usize) -> Vec<u8> {
    let mut field = text.as_bytes().to_vec();
    field.resize(width, b' ');
    field
}

/// Whether `text` matches `mask` byte for byte, where `N` in the mask stands for one ASCII digit.
fn conforms(text: &[u8], mask: &[u8]) -> bool {
    text.len() == mask.len()
        && text.iter().zip(mask).all(|(&byte, &expected)| {
            if expected == b'N' {
                byte.is_ascii_digit()
            } else {
                byte == expected
            }
        })
}

/// Asserts that `body` is a CDRD reply: exactly 1,000 bytes, the 46 characters of
/// [`REPLY_TEXT_MASK`] with the date and the time checked by format only, then 954 spaces
/// (CODATE01-003 to CODATE01-007).
fn assert_reply_body(body: &[u8]) -> TestResult {
    assert_eq!(body.len(), BUFFER_LEN, "the published reply length");
    let text = segment(body, 0, REPLY_TEXT_LEN)?;
    assert!(
        conforms(text, REPLY_TEXT_MASK),
        "reply text {:?}",
        String::from_utf8_lossy(text)
    );
    let padding = segment(body, REPLY_TEXT_LEN, BUFFER_LEN - REPLY_TEXT_LEN)?;
    assert!(all_spaces(padding), "the reply is not space-padded");
    Ok(())
}

/// The snapshot of a reply: its 46 written characters with every digit shown as `N`, since the
/// date and the time are compared by format only, then its length and trailing spaces.
fn reply_rendering(body: &[u8]) -> TestResult<String> {
    let masked: String = segment(body, 0, REPLY_TEXT_LEN)?
        .iter()
        .map(|&byte| {
            if byte.is_ascii_digit() {
                'N'
            } else {
                char::from(byte)
            }
        })
        .collect();
    let len = body.len();
    let trailing_spaces = body
        .iter()
        .skip(REPLY_TEXT_LEN)
        .filter(|&&byte| byte == b' ')
        .count();
    Ok(format!(
        "{masked}\nlen={len} trailing_spaces={trailing_spaces}"
    ))
}

/// Fails unless `rendered` matches this file's committed snapshot `name`,
/// `tests/snapshots/cdrd__<name>.snap`, as insta matches a named file snapshot: both texts with
/// their leading line breaks and trailing whitespace trimmed and every CRLF read as LF. The
/// snapshot is only read, never written or refreshed. A mismatch names the snapshot, both lengths
/// and the first differing byte offset, never the text.
fn compare_snapshot(name: &str, rendered: &str) -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("cdrd__{name}.snap"));
    let snapshot = insta::Snapshot::from_file(&path)
        .map_err(|error| format!("the committed snapshot {name} cannot be read: {error}"))?;
    let SnapshotContents::Text(committed) = snapshot.contents() else {
        return Err(format!("the committed snapshot {name} is not a text snapshot").into());
    };
    let expected = committed.to_string();
    let actual = rendered
        .trim_start_matches(['\r', '\n'])
        .trim_end()
        .replace("\r\n", "\n");
    if actual == expected {
        return Ok(());
    }
    let differs_at = actual
        .bytes()
        .zip(expected.bytes())
        .position(|(got, want)| got != want)
        .unwrap_or_else(|| actual.len().min(expected.len()));
    Err(format!(
        "the rendering differs from the committed snapshot {name} at byte {differs_at}: {} \
         bytes rendered, {} committed",
        actual.len(),
        expected.len()
    )
    .into())
}

/// Asserts the `MQ-ERR-DISPLAY` body that a failed put of the reply to `queue` publishes, every
/// byte except the HALTED(H4) code fields at offsets 54-55 and 58-62 (CODATE01-015).
fn assert_error_body(body: &[u8], queue: &str) -> TestResult {
    assert_eq!(body.len(), BUFFER_LEN, "the published error body length");
    let expected = [
        // MQ-ERROR-PARA as INITIALIZE left it, and its FILLER.
        (0, padded("", 27)),
        (27, padded("MQPUT ERR", 25)),
        (52, padded("", 2)),
        (56, padded("", 2)),
        (63, padded("", 2)),
        (65, padded(queue, 48)),
        (
            MQ_ERR_DISPLAY_LEN,
            padded("", BUFFER_LEN - MQ_ERR_DISPLAY_LEN),
        ),
    ];
    for (offset, bytes) in &expected {
        assert_eq!(
            segment(body, *offset, bytes.len())?,
            bytes.as_slice(),
            "error body bytes at offset {offset}"
        );
    }
    let asserted: usize = expected.iter().map(|(_, bytes)| bytes.len()).sum();
    assert_eq!(
        asserted + 2 + 5,
        BUFFER_LEN,
        "every byte but the two code fields"
    );
    Ok(())
}

/// Asserts the properties of a CDRD reply or error message: the request's `message_id` and
/// `correlation_id` unchanged, an absent one absent, `delivery_mode` 2, and no other property,
/// `expiration` included (CODATE01-010, CODATE01-011, CODATE01-018).
fn assert_copied_properties(
    properties: &BasicProperties,
    message_id: Option<&str>,
    correlation_id: Option<&str>,
) {
    let message_id_seen = properties.message_id().as_ref().map(ShortString::as_str);
    assert_eq!(message_id_seen, message_id, "message_id");
    let correlation_id_seen = properties
        .correlation_id()
        .as_ref()
        .map(ShortString::as_str);
    assert_eq!(correlation_id_seen, correlation_id, "correlation_id");
    assert_eq!(
        *properties.delivery_mode(),
        Some(PERSISTENT),
        "delivery_mode"
    );
    let absent = [
        ("expiration", properties.expiration().is_none()),
        ("reply_to", properties.reply_to().is_none()),
        ("type", properties.kind().is_none()),
        ("content_type", properties.content_type().is_none()),
        ("content_encoding", properties.content_encoding().is_none()),
        ("headers", properties.headers().is_none()),
        ("priority", properties.priority().is_none()),
        ("timestamp", properties.timestamp().is_none()),
        ("user_id", properties.user_id().is_none()),
        ("app_id", properties.app_id().is_none()),
        ("cluster_id", properties.cluster_id().is_none()),
    ];
    for (name, is_absent) in absent {
        assert!(is_absent, "the {name} property is set");
    }
}

/// What [`RecordingAcknowledger`] saw.
#[derive(Default)]
struct Record {
    /// Acknowledgements made.
    acks: AtomicUsize,
    /// How many confirmations the gate had handed on to the worker at the latest one.
    passed_at_ack: AtomicUsize,
    rejected: AtomicBool,
}

/// An [`Acknowledger`] that settles nothing on the broker. Each acknowledgement is counted with
/// the number of confirmations the gate had handed on to the worker when it was made.
struct RecordingAcknowledger {
    gate: ConfirmGate,
    record: Arc<Record>,
}

impl Acknowledger for RecordingAcknowledger {
    async fn ack(&self, _delivery_tag: u64) -> Result<(), AmqpError> {
        self.record
            .passed_at_ack
            .store(self.gate.passed(), Ordering::SeqCst);
        self.record.acks.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn reject_without_requeue(&self, _delivery_tag: u64) -> Result<(), AmqpError> {
        self.record.rejected.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// What [`Harness::hold`] read at the end of one hold of a confirmation.
#[derive(Debug, PartialEq, Eq)]
struct Held {
    /// The ready messages of the queue the held confirmation's message went to.
    queued: u32,
    /// Confirmations the gate had handed on to the worker.
    passed: usize,
    /// Acknowledgements the worker had made.
    acks: usize,
    /// Whether the worker had rejected the request.
    rejected: bool,
}

impl Held {
    /// A hold at whose end `queue` held `queued` messages, the gate had handed on `passed`
    /// confirmations and the request was not settled.
    fn unsettled(queued: u32, passed: usize) -> Self {
        Self {
            queued,
            passed,
            acks: 0,
            rejected: false,
        }
    }
}

// audit: CODATE01-002
// audit: CODATE01-003
// audit: CODATE01-004
// audit: CODATE01-005
// audit: CODATE01-006
// audit: CODATE01-007
// audit: CODATE01-010
// audit: CODATE01-011
#[test]
fn reply_on_fixed_queue_with_copied_properties() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let reply_to = rabbitmq::declare_reply_queue(&harness.channel).await?;
        let properties = request_ids("cdrd-msg-0001", "cdrd-corr-0001")
            .with_reply_to(ShortString::from(reply_to.as_str()));
        harness.publish_request(b"DATE", properties).await?;
        let mut worker = harness.worker().await?;

        let processed = step(&mut worker, &harness.worker_connection).await?;
        assert_eq!(processed, Processed::Replied);

        let reply = harness.take(REPLY_QUEUE).await?;
        assert_reply_body(&reply.data)?;
        assert_copied_properties(
            &reply.properties,
            Some("cdrd-msg-0001"),
            Some("cdrd-corr-0001"),
        );
        // The request's reply_to is ignored: the reply goes only to CARD.DEMO.REPLY.DATE.
        let misrouted = rabbitmq::get(&harness.channel, &reply_to, Duration::ZERO).await?;
        assert!(
            misrouted.is_none(),
            "a reply went to the request's reply_to"
        );
        compare_snapshot("fixed_clock_reply", &reply_rendering(&reply.data)?)?;
        Ok(())
    })
}

// audit: CODATE01-001
// audit: CODATE01-010
// audit: CODATE01-013
#[test]
fn replies_ignore_the_request_body_and_copy_only_their_own_ids() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        // An empty body, then 1,000 bytes of every byte value; the second request has no
        // correlation_id.
        let any_bytes: Vec<u8> = (0..=u8::MAX).cycle().take(BUFFER_LEN).collect();
        harness
            .publish_request(b"", request_ids("cdrd-msg-0002", "cdrd-corr-0002"))
            .await?;
        let second = BasicProperties::default().with_message_id(ShortString::from("cdrd-msg-0003"));
        harness.publish_request(&any_bytes, second).await?;
        let mut worker = harness.worker().await?;

        for _ in 0..2 {
            let processed = step(&mut worker, &harness.worker_connection).await?;
            assert_eq!(processed, Processed::Replied);
        }

        let first = harness.take(REPLY_QUEUE).await?;
        let second = harness.take(REPLY_QUEUE).await?;
        assert_reply_body(&first.data)?;
        assert_copied_properties(
            &first.properties,
            Some("cdrd-msg-0002"),
            Some("cdrd-corr-0002"),
        );
        assert_copied_properties(&second.properties, Some("cdrd-msg-0003"), None);
        // One clock instant, two different request bodies: the same reply bytes.
        assert_eq!(
            first.data, second.data,
            "the reply depends on the request body"
        );
        Ok(())
    })
}

// audit: CODATE01-012
#[test]
fn ack_follows_confirmed_reply() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        harness
            .publish_request(b"DATE", request_ids("cdrd-msg-0004", "cdrd-corr-0004"))
            .await?;
        let record = Arc::new(Record::default());
        let (gate, gated, mut worker) = harness.gated_worker(&record).await?;

        // The worker answers while the test holds the reply's confirmation, then releases it.
        let held = harness.hold(&gate, &record, 1, REPLY_QUEUE, ConfirmGate::release);
        let (processed, held) = join(step(&mut worker, &gated), held).await;
        // Queue presence is not confirmation: the reply was on its queue throughout the hold.
        assert_eq!(
            held?,
            Held::unsettled(1, 0),
            "while the reply's confirmation was held: the reply queued, the request unsettled"
        );
        assert_eq!(processed?, Processed::Replied);
        assert_eq!(
            record.acks.load(Ordering::SeqCst),
            1,
            "the request was acknowledged once"
        );
        assert_eq!(
            record.passed_at_ack.load(Ordering::SeqCst),
            1,
            "the request was acknowledged before the reply's confirmation reached the worker"
        );
        assert!(
            !record.rejected.load(Ordering::SeqCst),
            "the request was rejected"
        );

        // The double, not the consumer channel, settled the request: closing the worker's
        // connection returns it to its queue.
        let close = gated.close(REPLY_SUCCESS, "test done");
        bounded("close the worker connection", close).await?;
        harness.wait_for_count(harness.request_queue(), 1).await
    })
}

// audit: CODATE01-012
// audit: CODATE01-014
// audit: CODATE01-015
// audit: CODATE01-018
#[test]
fn refused_reply_confirmation_acks_only_after_the_confirmed_mqput_err() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        harness
            .publish_request(b"DATE", request_ids("cdrd-msg-0008", "cdrd-corr-0008"))
            .await?;
        let record = Arc::new(Record::default());
        let (gate, gated, mut worker) = harness.gated_worker(&record).await?;

        // The reply's confirmation is held, then refused; the MQPUT ERR message's is held, then
        // released.
        let holds = async {
            let reply = harness
                .hold(&gate, &record, 1, REPLY_QUEUE, ConfirmGate::refuse)
                .await?;
            let error = harness
                .hold(&gate, &record, 2, ERROR_QUEUE, ConfirmGate::release)
                .await?;
            Ok::<_, Box<dyn Error + Send + Sync>>((reply, error))
        };
        let (processed, holds) = join(step(&mut worker, &gated), holds).await;
        let (reply_hold, error_hold) = holds?;
        // The broker queued the reply and only the worker was told Nack, so the reply is on
        // CARD.DEMO.REPLY.DATE: a refused confirmation, not an unroutable reply.
        assert_eq!(
            reply_hold,
            Held::unsettled(1, 0),
            "while the reply's confirmation was held"
        );
        assert_eq!(
            error_hold,
            Held::unsettled(1, 1),
            "while the error message's confirmation was held"
        );
        // Only Ack(None) is a confirmed put: the refused reply took the MQPUT ERR path.
        assert_eq!(processed?, Processed::ErrorRouted);
        assert_eq!(
            record.acks.load(Ordering::SeqCst),
            1,
            "the request was acknowledged once"
        );
        assert_eq!(
            record.passed_at_ack.load(Ordering::SeqCst),
            2,
            "the request was acknowledged before the error message's confirmation reached the worker"
        );
        assert!(
            !record.rejected.load(Ordering::SeqCst),
            "the request was rejected"
        );
        let error = harness.take(ERROR_QUEUE).await?;
        assert_error_body(&error.data, REPLY_QUEUE)?;
        assert_copied_properties(
            &error.properties,
            Some("cdrd-msg-0008"),
            Some("cdrd-corr-0008"),
        );

        // The double, not the consumer channel, settled the request: closing the worker's
        // connection returns it to its queue.
        let close = gated.close(REPLY_SUCCESS, "test done");
        bounded("close the worker connection", close).await?;
        harness.wait_for_count(harness.request_queue(), 1).await
    })
}

// audit: CODATE01-014
// audit: CODATE01-015
// audit: CODATE01-018
#[test]
fn reply_put_failure_routes_mq_err_display_to_error_queue() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let mut worker = harness.worker().await?;
        harness.delete_queue(REPLY_QUEUE).await?;
        harness
            .publish_request(b"DATE", request_ids("cdrd-msg-0005", "cdrd-corr-0005"))
            .await?;

        let processed = step(&mut worker, &harness.worker_connection).await?;
        assert_eq!(processed, Processed::ErrorRouted);
        let error = harness.take(ERROR_QUEUE).await?;
        assert_error_body(&error.data, REPLY_QUEUE)?;
        assert_copied_properties(
            &error.properties,
            Some("cdrd-msg-0005"),
            Some("cdrd-corr-0005"),
        );

        // The request was acknowledged after the confirmed error message: closing the consumer
        // channel returns nothing to the request queue.
        bounded(
            "reopen the worker channels",
            Step::reopen(&mut worker, &harness.worker_connection),
        )
        .await?;
        harness.assert_request_queue_stays_empty().await?;

        // No task boundary: with its reply queue back, the worker answers the next request.
        let declare = harness.channel.queue_declare(
            REPLY_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        );
        bounded("declare the reply queue again", declare).await?;
        harness
            .publish_request(b"DATE", request_ids("cdrd-msg-0006", "cdrd-corr-0006"))
            .await?;
        let processed = step(&mut worker, &harness.worker_connection).await?;
        assert_eq!(processed, Processed::Replied);
        let reply = harness.take(REPLY_QUEUE).await?;
        assert_reply_body(&reply.data)?;
        assert_copied_properties(
            &reply.properties,
            Some("cdrd-msg-0006"),
            Some("cdrd-corr-0006"),
        );
        Ok(())
    })
}

// audit: CODATE01-019
#[test]
fn error_put_failure_discards_the_request_with_one_error_event() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let mut worker = harness.worker().await?;
        harness.delete_queue(REPLY_QUEUE).await?;
        harness.delete_queue(ERROR_QUEUE).await?;
        harness
            .publish_request(b"DATE", request_ids("cdrd-msg-0007", "cdrd-corr-0007"))
            .await?;

        let capture = trace_capture::capture();
        let processed = step(&mut worker, &harness.worker_connection).await?;
        let lines = capture.lines();
        drop(capture);
        assert_eq!(processed, Processed::Discarded);
        // The event that stands in for DISPLAY MQ-ERR-DISPLAY; its code fields are HALTED(H4).
        let errors: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains(r#""level":"ERROR""#))
            .collect();
        let [event] = errors.as_slice() else {
            return Err(format!("expected one ERROR event, got {errors:?}").into());
        };
        for member in [
            r#""return_message":"MQPUT ERR""#,
            r#""queue_name":"CARD.DEMO.ERROR""#,
        ] {
            assert!(event.contains(member), "{member} missing from {event}");
        }

        // Rejected without requeue, and CARD.DEMO.REQUEST.DATE has no dead-letter arguments: closing
        // the consumer channel returns nothing, so the request is gone and not redelivered.
        bounded(
            "reopen the worker channels",
            Step::reopen(&mut worker, &harness.worker_connection),
        )
        .await?;
        harness.assert_request_queue_stays_empty().await
    })
}

/// Reports the MQ completion and reason codes of the error body as `BLOCKED`: IBM MQ defines
/// them and AMQP has no counterpart, so no test asserts a value for them.
// audit: CODATE01-016
// audit: CODATE01-017
#[test]
#[ignore = "BLOCKED: HALTED(H4) MQ condition/reason codes in MQ-ERR-DISPLAY; no parity assertion (AAP 0.8.5)"]
fn blocked_h4_mq_codes_in_error_body() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: CODATE01-016 and CODATE01-017 (H4): the MQ completion and reason codes of the \
         MQ-ERR-DISPLAY error body are defined by IBM MQ, and AMQP has no counterpart for them"
            .into(),
    )
}

/// Reports as `BLOCKED` whether a confirmed reply is delivered and its request removed from the
/// request queue, which the halt candidate X-CODATE01-H2 of the CODATE01 audit leaves without a
/// single COBOL result until the reviewer folds it in or rejects it.
// audit: CODATE01-001
// audit: CODATE01-012
// audit: CODATE01-013
#[test]
#[ignore = "BLOCKED: X-CODATE01-H2 delivery of a confirmed reply; no parity assertion (AAP 0.8.5)"]
fn blocked_x_codate01_h2_delivery_after_confirmed_reply() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: CODATE01-001, CODATE01-012 and CODATE01-013 (X-CODATE01-H2): whether a \
         confirmed reply is delivered and its request removed depends on the ADD to MQ-MSG-COUNT, \
         which has no VALUE, so the COBOL has no single result"
            .into(),
    )
}
