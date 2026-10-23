//! CDRA, the port of `COACCT01`, against a real broker and a migrated, fixture-loaded database:
//! the broker-level rows of `modern/audit/messaging/COACCT01.md` (binding g3a).
//!
//! Every test starts its own `rabbitmq:3.13-management` container with `definitions.json`
//! through `test_support::rabbitmq` and its own `postgres:16` container loaded with the nine
//! fixture loads through `test_support::fixtures`, and drives one in-process
//! `messaging::workers::cdra::Worker` on a connection of its own. The test publishes and reads
//! on a second connection, whose channel is in confirm mode, so a request is on its queue
//! before the worker's next `basic_get`. Closing the worker's connection returns every request
//! it left unsettled to its queue, which is how an empty request queue proves the
//! acknowledgement.
//!
//! | Test | Rows |
//! |---|---|
//! | `inqa_known_account_replies_ws_acct_response` | COACCT01-001, COACCT01-004, COACCT01-020, COACCT01-023 to COACCT01-026; the bytes of COACCT01-006 to COACCT01-019 for all 50 fixture accounts |
//! | `inqa_unknown_account_replies_invalid_parameters` | COACCT01-004, COACCT01-021, COACCT01-023 to COACCT01-025 |
//! | `bad_function_or_zero_key_replies_function_text` | COACCT01-005, COACCT01-022, COACCT01-023 |
//! | `account_read_failure_routes_error_body` | COACCT01-028, COACCT01-032, the asserted bytes of COACCT01-029 |
//! | `unconfirmed_reply_routes_mqput_err_to_the_error_queue` | COACCT01-033, COACCT01-032 |
//! | `unconfirmed_error_discards_the_request_with_one_error_event` | COACCT01-034 |
//! | `ack_follows_confirmed_reply` | COACCT01-026; the bytes of COACCT01-006 to COACCT01-019 for one fixture account |
//! | `refused_reply_confirmation_acks_only_after_the_confirmed_mqput_err` | COACCT01-026, COACCT01-033, COACCT01-032, the asserted bytes of COACCT01-029 |
//! | `blocked_h4_codes`, `blocked_delivery_parity` | reported `BLOCKED` |
//!
//! # Confirmation before acknowledgement
//!
//! A reply on its queue is not a confirmed one: the broker queues a message before its
//! confirmation reaches the publisher. The two acknowledgement tests therefore connect the
//! worker through a `test_support::rabbitmq::ConfirmGate`, which withholds each confirmation
//! the broker sends for [`HOLD`] while the test reads the queues and a recording acknowledger,
//! then releases it or refuses it, handing the worker a negative confirmation. While a
//! confirmation is held, its message is on its queue and the request is neither acknowledged
//! nor rejected; the acknowledgement comes only once the gate has handed on the confirmation of
//! the reply, or, after a refused reply, of its `MQPUT ERR` message.
//!
//! # Expected bytes
//!
//! Labels, masks and error texts are the literals the audit rows state. The account values of a
//! found reply are the bytes of the account's own `acctdata.txt` line at its `CVACT01Y` offsets,
//! read in place: the audit builds the expected reply from the account row and never quotes a
//! fixture value, and the raw line is independent of the codecs and of the reply builder under
//! test. The insta snapshot of account `00000000001` fixes the resulting bytes for review.
//!
//! # Halts
//!
//! The condition and reason codes of the error body (bytes 54-55 and 58-62) are halt H4: no
//! value is asserted for them, and the read-failure test prints `BLOCKED` for COACCT01-030 and
//! COACCT01-031. Halt candidate X-COACCT01-H4 blocks the parity of the delivery of a reply that
//! was put successfully; the acknowledgement after a confirmed publication is asserted as the
//! request's override, never as a parity result. Parity tests assume single delivery.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use domain::messaging::coacct01::{ACCT_RESPONSE_LEN, ERROR_QUEUE, REPLY_QUEUE, REQUEST_LEN};
use domain::messaging::codate01::BUFFER_LEN;
use insta::internals::SnapshotContents;
use lapin::message::BasicGetMessage;
use lapin::options::{ConfirmSelectOptions, QueueDeleteOptions};
use lapin::types::ShortString;
use lapin::{BasicProperties, Channel, Connection};
use messaging::broker::{Acknowledger, AmqpError, PUBLISH_DEADLINE};
use messaging::workers::cdra::Worker;
use messaging::workers::{Processed, Step, StepError};
use persistence::PgStore;
use runtime::config::Config;
use test_support::fixtures::{self, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use test_support::rabbitmq::{self, ConfirmGate, RabbitMqGuard, start_rabbitmq};
use test_support::trace_capture;

type BoxError = Box<dyn Error + Send + Sync>;
type TestResult = Result<(), BoxError>;

/// The request queue, the default of `CARDDEMO_QUEUE_REQUEST_ACCT`, which the read-failure
/// error body names in `MQ-APPL-QUEUE-NAME` (COACCT01-029).
const REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

/// Another flow's reply queue, named as the `reply_to` of a CDRA request that must not reach it
/// (COACCT01-023).
const OTHER_REPLY_QUEUE: &str = "CARD.DEMO.REPLY.DATE";

/// Bound of the two container starts, which carry their own deadlines below it.
const START_WAIT: Duration = Duration::from_secs(300);

/// Bound of every other broker, database or worker operation of a test.
const WAIT: Duration = Duration::from_secs(30);

/// How long a reply or error message may take to appear on its queue.
const MESSAGE_WAIT: Duration = Duration::from_secs(5);

/// How long a queue that must stay empty is watched: long enough for the broker to requeue a
/// delivery left unsettled by a closed connection.
const EMPTY_WAIT: Duration = Duration::from_millis(500);

/// Pause between two `process_next` calls that found the request queue empty.
const IDLE_PAUSE: Duration = Duration::from_millis(50);

/// How long the confirmation gate withholds a confirmation that has reached it before the test
/// reads the queues and the acknowledger. A quarter of the worker's [`PUBLISH_DEADLINE`] at
/// most, so that the publication and its released confirmation still finish within it.
const HOLD: Duration = Duration::from_secs(1);
const _: () = assert!(
    HOLD.as_millis() * 4 <= PUBLISH_DEADLINE.as_millis(),
    "HOLD must stay well within PUBLISH_DEADLINE"
);

/// AMQP `reply-success`, the code of a normal connection close.
const REPLY_SUCCESS: u16 = 200;

/// Bytes of an `acctdata.txt` record, the 300-byte `CVACT01Y` layout.
const ACCOUNT_RECORD_LEN: usize = 300;

/// Bytes of `ACCT-ID PIC 9(11)` at offset 0 of the record, and of `WS-KEY` in the request.
const KEY_LEN: usize = 11;

/// Accounts in `acctdata.txt` (FIXFRM-001).
const FIXTURE_ACCOUNTS: usize = 50;

/// `WS-ACCT-RESPONSE` (`COACCT01.cbl:130-169`): each label literal followed by the `CVACT01Y`
/// field the found read moves after it, as (label, record offset, length) (COACCT01-006 to
/// COACCT01-016). `ACCT-ADDR-ZIP` at offset 102 and the `FILLER` are not moved (COACCT01-019).
const RESPONSE_FIELDS: [(&str, usize, usize); 11] = [
    ("ACCOUNT ID : ", 0, 11),
    ("ACCOUNT STATUS : ", 11, 1),
    ("BALANCE : ", 12, 12),
    ("CREDIT LIMIT : ", 24, 12),
    ("CASH LIMIT : ", 36, 12),
    ("OPEN DATE : ", 48, 10),
    ("EXPR DATE : ", 58, 10),
    ("REIS DATE : ", 68, 10),
    ("CREDIT BAL : ", 78, 12),
    ("DEBIT BAL : ", 90, 12),
    ("GROUP ID : ", 112, 10),
];

/// The text the not-found and bad-parameter replies open with, offsets 0-36 (COACCT01-021,
/// COACCT01-022).
const INVALID_PARAMETERS: &[u8] = b"INVALID REQUEST PARAMETERS ACCT ID : ";

/// Offsets 37-47 of the not-found and bad-parameter replies. Cardholder Data Redaction masks
/// the key the COBOL echoes there, because eleven requester digits can hold an SSN or a
/// password (refinement override X-COACCT01-9): replacement behaviour, never a parity result.
const MASKED_KEY: &[u8] = b"***********";

/// The label after the masked key of the bad-parameter reply, offsets 48-58 (COACCT01-022).
const FUNCTION_LABEL: &[u8] = b"FUNCTION : ";

/// `MQ-APPL-RETURN-MESSAGE` after a failed account read: the 28-character literal
/// `ERROR WHILE READING ACCTFILE` cut to `PIC X(25)` (E18; COACCT01-028).
const READ_FAILURE_MESSAGE: &[u8] = b"ERROR WHILE READING ACCTF";

/// `MQ-APPL-RETURN-MESSAGE` after a failed put (COACCT01-033, COACCT01-034).
const MQPUT_ERR: &[u8] = b"MQPUT ERR";

/// One test's containers, its configuration and its two broker connections: the worker's and
/// the test's own, whose channel `test` is in confirm mode.
struct Harness {
    test: Channel,
    /// The connection that carries `test`, kept open with it.
    _test_connection: Connection,
    worker_connection: Connection,
    config: Config,
    pg: PostgresGuard,
    rabbit: RabbitMqGuard,
}

impl Harness {
    /// Starts both containers, then opens the two connections.
    async fn start() -> Result<Self, BoxError> {
        // Boxed, as every large future here, so that no test future grows past
        // `clippy::large_futures`.
        let (rabbit, pg) = bounded(
            "start the broker and the loaded database",
            START_WAIT,
            Box::pin(try_join(start_rabbitmq(), start_loaded_postgres())),
        )
        .await?;
        let config = Config {
            amqp_url: rabbit.amqp_url().to_owned(),
            database_url: pg.database_url().to_owned(),
            ..Config::default()
        };
        assert_eq!(config.queue_request_acct, REQUEST_QUEUE);
        let test_connection = bounded("connect the test", WAIT, rabbit.connect()).await?;
        let test = bounded(
            "open the test channel",
            WAIT,
            test_connection.create_channel(),
        )
        .await?;
        bounded(
            "put the test channel in confirm mode",
            WAIT,
            test.confirm_select(ConfirmSelectOptions::default()),
        )
        .await?;
        let worker_connection = bounded("connect the worker", WAIT, rabbit.connect()).await?;
        Ok(Self {
            test,
            _test_connection: test_connection,
            worker_connection,
            config,
            pg,
            rabbit,
        })
    }

    /// The store of the fixture-loaded database.
    fn store(&self) -> PgStore {
        PgStore::new(self.pg.pool().clone())
    }

    /// Opens the CDRA worker on its own connection with `store`.
    async fn open_worker(&self, store: PgStore) -> Result<Worker, BoxError> {
        bounded(
            "open the CDRA worker",
            WAIT,
            Worker::open(&self.worker_connection, &self.config, store),
        )
        .await
    }

    /// A confirmation gate in front of the broker, a connection of the worker's own through it,
    /// and a CDRA worker on that connection over the fixture-loaded store that settles through a
    /// [`RecordingAcknowledger`] filling `record`.
    async fn gated_worker(
        &self,
        record: &Arc<Record>,
    ) -> Result<(ConfirmGate, Connection, Worker<RecordingAcknowledger>), BoxError> {
        let gate = self.rabbit.confirm_gate().await?;
        let connection =
            bounded("connect the worker through the gate", WAIT, gate.connect()).await?;
        let worker = bounded(
            "open the CDRA worker through the gate",
            WAIT,
            Worker::open(&connection, &self.config, self.store()),
        )
        .await?;
        let acknowledger = RecordingAcknowledger {
            gate: gate.clone(),
            record: Arc::clone(record),
        };
        Ok((gate, connection, worker.with_acknowledger(acknowledger)))
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
    ) -> Result<Held, BoxError> {
        let held = async {
            gate.wait_for_arrivals(count, MESSAGE_WAIT).await?;
            tokio::time::sleep(HOLD).await;
            let queued = rabbitmq::message_count(&self.test, queue).await?;
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

    /// Publishes the request `body` to the request queue, confirmed.
    async fn send(&self, body: &[u8], properties: BasicProperties) -> TestResult {
        let queue = self.config.queue_request_acct.as_str();
        bounded(
            "publish the request",
            WAIT,
            rabbitmq::publish(&self.test, queue, body, properties),
        )
        .await
    }

    /// Takes the next message from `queue`; none within [`MESSAGE_WAIT`] is an error.
    async fn take(&self, queue: &str) -> Result<BasicGetMessage, BoxError> {
        bounded(
            "take a message",
            WAIT,
            rabbitmq::get(&self.test, queue, MESSAGE_WAIT),
        )
        .await?
        .ok_or_else(|| format!("no message on {queue}").into())
    }

    /// Asserts that `queue` stays empty for [`EMPTY_WAIT`].
    async fn assert_empty(&self, queue: &str) -> TestResult {
        let message = bounded(
            "watch an empty queue",
            WAIT,
            rabbitmq::get(&self.test, queue, EMPTY_WAIT),
        )
        .await?;
        assert!(message.is_none(), "{queue} holds a message");
        Ok(())
    }

    /// Deletes `queue`, so that a mandatory publication to it is returned unrouted.
    async fn delete_queue(&self, queue: &str) -> TestResult {
        bounded(
            "delete a queue",
            WAIT,
            self.test.queue_delete(queue, QueueDeleteOptions::default()),
        )
        .await?;
        Ok(())
    }

    /// Closes the worker's connection; the broker requeues every request it left unsettled.
    async fn close_worker_connection(&self) -> TestResult {
        bounded(
            "close the worker connection",
            WAIT,
            self.worker_connection
                .close(REPLY_SUCCESS, "the CDRA test is done"),
        )
        .await
    }
}

/// Awaits `operation` for at most `limit`, naming `what` when it does not finish.
async fn bounded<T, E>(
    what: &str,
    limit: Duration,
    operation: impl Future<Output = Result<T, E>>,
) -> Result<T, BoxError>
where
    E: Into<BoxError>,
{
    match tokio::time::timeout(limit, operation).await {
        Ok(outcome) => outcome.map_err(Into::into),
        Err(_) => Err(format!("{what} did not finish within {} ms", limit.as_millis()).into()),
    }
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

/// Drives `first` and `second` concurrently on the calling task, as [`join`] does, until both
/// have succeeded, and returns both values. The first error is returned at once, and the other
/// future is dropped unfinished.
async fn try_join<T, U, E>(
    first: impl Future<Output = Result<T, E>>,
    second: impl Future<Output = Result<U, E>>,
) -> Result<(T, U), E> {
    let mut first = Box::pin(first);
    let mut second = Box::pin(second);
    let mut first_value = None;
    let mut second_value = None;
    std::future::poll_fn(|context| {
        if first_value.is_none() {
            match first.as_mut().poll(context) {
                Poll::Ready(Ok(value)) => first_value = Some(value),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {}
            }
        }
        if second_value.is_none() {
            match second.as_mut().poll(context) {
                Poll::Ready(Ok(value)) => second_value = Some(value),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {}
            }
        }
        match (first_value.take(), second_value.take()) {
            (Some(first_done), Some(second_done)) => Poll::Ready(Ok((first_done, second_done))),
            (first_left, second_left) => {
                first_value = first_left;
                second_value = second_left;
                Poll::Pending
            }
        }
    })
    .await
}

/// Runs `worker` until it has taken and settled one request.
async fn settle_one<W: Step>(
    worker: &mut W,
    connection: &Connection,
) -> Result<Processed, BoxError> {
    bounded(
        "process one request",
        WAIT,
        Box::pin(next_request(worker, connection)),
    )
    .await
}

/// Calls `process_next` until it finds a request.
async fn next_request<W: Step>(
    worker: &mut W,
    connection: &Connection,
) -> Result<Processed, StepError> {
    loop {
        match worker.process_next(connection).await? {
            Processed::Idle => tokio::time::sleep(IDLE_PAUSE).await,
            processed => return Ok(processed),
        }
    }
}

/// The `REQUEST-MSG-COPY` bytes the program reads: `WS-FUNC` then `WS-KEY` (COACCT01-001).
fn request(function: &[u8], key: &[u8]) -> Vec<u8> {
    let body = [function, key].concat();
    assert_eq!(body.len(), REQUEST_LEN, "a request is WS-FUNC and WS-KEY");
    body
}

/// Properties carrying both IDs, which every reply and error message must copy (COACCT01-024,
/// COACCT01-032).
fn with_ids(message_id: &str, correlation_id: &str) -> BasicProperties {
    BasicProperties::default()
        .with_message_id(ShortString::from(message_id))
        .with_correlation_id(ShortString::from(correlation_id))
}

/// The 50 `acctdata.txt` records, read in place, each its 300 bytes without the terminator.
fn fixture_accounts() -> Result<Vec<Vec<u8>>, BoxError> {
    let text = std::fs::read(fixtures::fixture_path("acctdata.txt"))?;
    let records: Vec<Vec<u8>> = text
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    assert_eq!(records.len(), FIXTURE_ACCOUNTS, "acctdata.txt accounts");
    for record in &records {
        assert_eq!(
            record.len(),
            ACCOUNT_RECORD_LEN,
            "acctdata.txt record length"
        );
    }
    Ok(records)
}

/// `ACCT-ID`, the first 11 bytes of an account record.
fn account_key(record: &[u8]) -> Result<&[u8], BoxError> {
    record
        .get(..KEY_LEN)
        .ok_or_else(|| "an account record is shorter than ACCT-ID".into())
}

/// The 1,000-byte found reply for the account `record`: the labels of [`RESPONSE_FIELDS`], each
/// followed by the record's bytes of its field, then 748 spaces (COACCT01-006 to COACCT01-020).
fn expected_found_reply(record: &[u8]) -> Result<Vec<u8>, BoxError> {
    let mut reply = Vec::with_capacity(BUFFER_LEN);
    for (label, offset, len) in RESPONSE_FIELDS {
        let value = record
            .get(offset..offset + len)
            .ok_or("an account record is shorter than CVACT01Y")?;
        reply.extend_from_slice(label.as_bytes());
        reply.extend_from_slice(value);
    }
    assert_eq!(reply.len(), ACCT_RESPONSE_LEN, "WS-ACCT-RESPONSE length");
    reply.resize(BUFFER_LEN, b' ');
    Ok(reply)
}

/// The 1,000-byte not-found reply (`function` `None`, COACCT01-021) or bad-parameter reply
/// (COACCT01-022): the opening text, the masked key, for a bad parameter `FUNCTION : ` and the
/// 4 function bytes as received, then spaces.
fn expected_invalid_parameters(function: Option<&[u8]>) -> Vec<u8> {
    let mut reply = [INVALID_PARAMETERS, MASKED_KEY].concat();
    if let Some(function) = function {
        reply.extend_from_slice(FUNCTION_LABEL);
        reply.extend_from_slice(function);
    }
    reply.resize(BUFFER_LEN, b' ');
    reply
}

/// `text` space-padded on the right to `width` bytes.
fn padded(text: &[u8], width: usize) -> Vec<u8> {
    let mut field = text.to_vec();
    field.resize(width, b' ');
    field
}

/// Asserts that `actual` equals `expected` byte for byte, showing both as text when they
/// differ. CDRA bodies are ASCII and carry no protected value (COACCT01-035).
fn assert_body(actual: &[u8], expected: &[u8], context: &str) {
    assert!(
        actual == expected,
        "{context}:\n  got  {:?}\n  want {:?}",
        String::from_utf8_lossy(actual),
        String::from_utf8_lossy(expected)
    );
}

/// Asserts every byte of the 1,000-byte `MQ-ERR-DISPLAY` error `body` outside its two code
/// fields, which halt H4 leaves unasserted (COACCT01-029): `MQ-ERROR-PARA` and the four
/// `FILLER`s spaces, the return message and the queue name space-padded to 25 and 48 bytes,
/// then 887 spaces.
fn assert_error_body(body: &[u8], return_message: &[u8], queue_name: &str) {
    assert_eq!(body.len(), BUFFER_LEN, "error body length");
    let fields: [(&str, usize, Vec<u8>); 8] = [
        ("MQ-ERROR-PARA", 0, padded(b"", 25)),
        ("FILLER after MQ-ERROR-PARA", 25, padded(b"", 2)),
        ("MQ-APPL-RETURN-MESSAGE", 27, padded(return_message, 25)),
        ("FILLER after MQ-APPL-RETURN-MESSAGE", 52, padded(b"", 2)),
        ("FILLER after MQ-APPL-CONDITION-CODE", 56, padded(b"", 2)),
        ("FILLER after MQ-APPL-REASON-CODE", 63, padded(b"", 2)),
        ("MQ-APPL-QUEUE-NAME", 65, padded(queue_name.as_bytes(), 48)),
        (
            "padding of ERROR-MESSAGE",
            113,
            padded(b"", BUFFER_LEN - 113),
        ),
    ];
    for (field, offset, expected) in fields {
        let actual = body
            .get(offset..offset + expected.len())
            .unwrap_or_default();
        assert_body(actual, &expected, field);
    }
}

/// A property as text, for comparison with the request's.
fn text(value: Option<&ShortString>) -> Option<&str> {
    value.map(ShortString::as_str)
}

/// Asserts the properties of a CDRA reply or error message: the request's `message_id` and
/// `correlation_id` copied, an absent one absent; persistent, no `expiration`; and none of the
/// other properties COACCT01-025 names (COACCT01-024, COACCT01-025, COACCT01-032).
fn assert_copied_properties(
    properties: &BasicProperties,
    message_id: Option<&str>,
    correlation_id: Option<&str>,
    context: &str,
) {
    assert_eq!(
        text(properties.message_id().as_ref()),
        message_id,
        "{context}: message_id"
    );
    assert_eq!(
        text(properties.correlation_id().as_ref()),
        correlation_id,
        "{context}: correlation_id"
    );
    assert_eq!(
        *properties.delivery_mode(),
        Some(2),
        "{context}: delivery_mode"
    );
    assert_eq!(
        text(properties.expiration().as_ref()),
        None,
        "{context}: expiration"
    );
    let others = [
        ("reply_to", properties.reply_to().is_some()),
        ("type", properties.kind().is_some()),
        ("content_type", properties.content_type().is_some()),
        ("content_encoding", properties.content_encoding().is_some()),
        ("user_id", properties.user_id().is_some()),
        ("app_id", properties.app_id().is_some()),
        ("timestamp", properties.timestamp().is_some()),
    ];
    for (property, present) in others {
        assert!(!present, "{context}: {property} is set");
    }
}

/// The snapshot text of a found reply: its length, then its 252 response bytes between
/// brackets so that trailing spaces stay visible.
fn snapshot_text(reply: &[u8]) -> Result<String, BoxError> {
    let response = reply
        .get(..ACCT_RESPONSE_LEN)
        .ok_or("the reply is shorter than WS-ACCT-RESPONSE")?;
    let len = reply.len();
    Ok(format!(
        "len={len}\nresponse=[{}]",
        std::str::from_utf8(response)?
    ))
}

/// Fails unless `rendered` matches this file's committed snapshot `name`,
/// `tests/snapshots/cdra__<name>.snap`, as insta matches a named file snapshot: both texts with
/// their leading line breaks and trailing whitespace trimmed and every CRLF read as LF. The
/// snapshot is only read, never written or refreshed. A mismatch names the snapshot, both lengths
/// and the first differing byte offset, never the text.
fn compare_snapshot(name: &str, rendered: &str) -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("cdra__{name}.snap"));
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

// audit: COACCT01-001
// audit: COACCT01-004
// audit: COACCT01-020
// audit: COACCT01-023
// audit: COACCT01-024
// audit: COACCT01-025
// audit: COACCT01-026
#[test]
fn inqa_known_account_replies_ws_acct_response() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let mut worker = harness.open_worker(harness.store()).await?;
        for (index, record) in fixture_accounts()?.iter().enumerate() {
            let key = account_key(record)?;
            let mut body = request(b"INQA", key);
            if index == 0 {
                // Bytes 15-999 are never read, whatever they hold (COACCT01-001).
                body.resize(BUFFER_LEN, b'Z');
            }
            let message_id = format!("CDRA-FOUND-{index:02}");
            let correlation_id = format!("CDRA-CORREL-{index:02}");
            harness
                .send(&body, with_ids(&message_id, &correlation_id))
                .await?;

            let processed = settle_one(&mut worker, &harness.worker_connection).await?;
            assert_eq!(processed, Processed::Replied, "account {index}");
            let reply = harness.take(REPLY_QUEUE).await?;
            let data = reply.delivery.data.as_slice();
            assert_eq!(data.len(), BUFFER_LEN, "account {index}: published length");
            let expected = expected_found_reply(record)?;
            assert_body(data, &expected, &format!("account {index}: found reply"));
            let padding = data.get(ACCT_RESPONSE_LEN..).unwrap_or_default();
            assert!(
                padding.iter().all(|&byte| byte == b' '),
                "account {index}: bytes 252-999 are spaces"
            );
            assert_copied_properties(
                &reply.delivery.properties,
                Some(&message_id),
                Some(&correlation_id),
                &format!("account {index}"),
            );
            if index == 0 {
                compare_snapshot("inqa_account_00000000001_reply", &snapshot_text(data)?)?;
            }
        }

        harness.close_worker_connection().await?;
        harness.assert_empty(REQUEST_QUEUE).await
    })
}

// audit: COACCT01-004
// audit: COACCT01-021
// audit: COACCT01-023
// audit: COACCT01-024
// audit: COACCT01-025
#[test]
fn inqa_unknown_account_replies_invalid_parameters() -> TestResult {
    block_on(async {
        let unknown: &[u8] = b"99999999999";
        let accounts = fixture_accounts()?;
        for record in &accounts {
            assert_ne!(
                account_key(record)?,
                unknown,
                "the key must be absent from acctdata.txt"
            );
        }
        let harness = Harness::start().await?;
        let mut worker = harness.open_worker(harness.store()).await?;
        // No IDs, and a reply_to naming another queue: both are ignored (COACCT01-023,
        // COACCT01-024).
        let properties =
            BasicProperties::default().with_reply_to(ShortString::from(OTHER_REPLY_QUEUE));
        harness.send(&request(b"INQA", unknown), properties).await?;

        let processed = settle_one(&mut worker, &harness.worker_connection).await?;
        assert_eq!(processed, Processed::Replied);
        let reply = harness.take(REPLY_QUEUE).await?;
        assert_body(
            &reply.delivery.data,
            &expected_invalid_parameters(None),
            "not-found reply",
        );
        assert_copied_properties(&reply.delivery.properties, None, None, "not-found reply");
        harness.assert_empty(OTHER_REPLY_QUEUE).await?;

        harness.close_worker_connection().await?;
        harness.assert_empty(REQUEST_QUEUE).await
    })
}

// audit: COACCT01-005
// audit: COACCT01-022
// audit: COACCT01-023
#[test]
fn bad_function_or_zero_key_replies_function_text() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let mut worker = harness.open_worker(harness.store()).await?;
        // A function other than INQA with a stored key, INQA with a zero key, and INQA in lower case:
        // `WS-FUNC = 'INQA' AND WS-KEY > ZEROES` fails for each, so no account is read.
        let cases: [(&[u8], &[u8]); 3] = [
            (b"XXXX", b"00000000001"),
            (b"INQA", b"00000000000"),
            (b"inqa", b"00000000001"),
        ];
        for (index, (function, key)) in cases.into_iter().enumerate() {
            let message_id = format!("CDRA-BAD-{index}");
            let correlation_id = format!("CDRA-BAD-CORREL-{index}");
            harness
                .send(
                    &request(function, key),
                    with_ids(&message_id, &correlation_id),
                )
                .await?;

            let processed = settle_one(&mut worker, &harness.worker_connection).await?;
            assert_eq!(processed, Processed::Replied, "case {index}");
            let reply = harness.take(REPLY_QUEUE).await?;
            assert_body(
                &reply.delivery.data,
                &expected_invalid_parameters(Some(function)),
                &format!("case {index}: bad-parameter reply"),
            );
            assert_copied_properties(
                &reply.delivery.properties,
                Some(&message_id),
                Some(&correlation_id),
                &format!("case {index}"),
            );
        }

        harness.close_worker_connection().await?;
        harness.assert_empty(REQUEST_QUEUE).await
    })
}

// audit: COACCT01-028
// audit: COACCT01-032
#[test]
fn account_read_failure_routes_error_body() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        // A pool closed before the worker reads through it: every account read is a store error,
        // the replacement's `WHEN OTHER` of the CICS READ (COACCT01-028).
        let closed = bounded(
            "open a second pool",
            WAIT,
            runtime::db::connect(&harness.config),
        )
        .await?;
        bounded("close the second pool", WAIT, async {
            closed.close().await;
            Ok::<(), BoxError>(())
        })
        .await?;
        let mut worker = harness.open_worker(PgStore::new(closed)).await?;
        harness
            .send(
                &request(b"INQA", b"00000000001"),
                with_ids("CDRA-READ-FAILURE", "CDRA-READ-FAILURE-CORREL"),
            )
            .await?;

        let processed = settle_one(&mut worker, &harness.worker_connection).await?;
        assert_eq!(processed, Processed::ErrorRouted);
        let error = harness.take(ERROR_QUEUE).await?;
        assert_error_body(&error.delivery.data, READ_FAILURE_MESSAGE, REQUEST_QUEUE);
        // Bytes 54-55 and 58-62 hold the CICS RESP and RESP2 of the failed READ.
        println!("BLOCKED COACCT01-030 (H4)");
        println!("BLOCKED COACCT01-031 (H4)");
        assert_copied_properties(
            &error.delivery.properties,
            Some("CDRA-READ-FAILURE"),
            Some("CDRA-READ-FAILURE-CORREL"),
            "read-failure error message",
        );
        harness.assert_empty(REPLY_QUEUE).await?;

        harness.close_worker_connection().await?;
        harness.assert_empty(REQUEST_QUEUE).await
    })
}

// audit: COACCT01-033
// audit: COACCT01-032
#[test]
fn unconfirmed_reply_routes_mqput_err_to_the_error_queue() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let mut worker = harness.open_worker(harness.store()).await?;
        // Deleted after the worker's passive declare: the mandatory reply is returned unrouted.
        harness.delete_queue(REPLY_QUEUE).await?;
        harness
            .send(
                &request(b"INQA", b"00000000001"),
                with_ids("CDRA-UNROUTED", "CDRA-UNROUTED-CORREL"),
            )
            .await?;

        let processed = settle_one(&mut worker, &harness.worker_connection).await?;
        assert_eq!(processed, Processed::ErrorRouted);
        let error = harness.take(ERROR_QUEUE).await?;
        assert_error_body(&error.delivery.data, MQPUT_ERR, REPLY_QUEUE);
        assert_copied_properties(
            &error.delivery.properties,
            Some("CDRA-UNROUTED"),
            Some("CDRA-UNROUTED-CORREL"),
            "MQPUT ERR error message",
        );

        harness.close_worker_connection().await?;
        harness.assert_empty(REQUEST_QUEUE).await
    })
}

// audit: COACCT01-034
#[test]
fn unconfirmed_error_discards_the_request_with_one_error_event() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let mut worker = harness.open_worker(harness.store()).await?;
        // Both destinations deleted after the worker's passive declare: the reply and then its
        // MQPUT ERR message are returned unrouted.
        harness.delete_queue(REPLY_QUEUE).await?;
        harness.delete_queue(ERROR_QUEUE).await?;
        harness
            .send(
                &request(b"INQA", b"00000000001"),
                with_ids("CDRA-DISCARDED", "CDRA-DISCARDED-CORREL"),
            )
            .await?;

        let capture = trace_capture::capture();
        let processed = settle_one(&mut worker, &harness.worker_connection).await?;
        let lines = capture.lines();
        drop(capture);
        assert_eq!(processed, Processed::Discarded);
        let return_message = format!(
            r#""return_message":"{}""#,
            String::from_utf8_lossy(MQPUT_ERR)
        );
        let events: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains(r#""level":"ERROR""#) && line.contains(&return_message))
            .collect();
        let [event] = events.as_slice() else {
            panic!("expected one MQ-ERR-DISPLAY error event, got {events:?}");
        };
        let queue_name = format!(r#""queue_name":"{ERROR_QUEUE}""#);
        assert!(event.contains(&queue_name), "{event}");

        // Rejected without requeue: the request is gone and is not redelivered.
        harness.close_worker_connection().await?;
        harness.assert_empty(REQUEST_QUEUE).await
    })
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
/// the number of confirmations the gate had handed on to the worker when it was made, and a
/// rejection is recorded.
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
    /// A hold at whose end the queue held `queued` messages, the gate had handed on `passed`
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

/// Closes `connection`, a worker's connection through the gate; the broker requeues every
/// request it left unsettled.
async fn close_gated(connection: &Connection) -> TestResult {
    bounded(
        "close the worker connection",
        WAIT,
        connection.close(REPLY_SUCCESS, "the CDRA test is done"),
    )
    .await
}

// audit: COACCT01-026
#[test]
fn ack_follows_confirmed_reply() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let record = Arc::new(Record::default());
        let (gate, gated, mut worker) = harness.gated_worker(&record).await?;
        let accounts = fixture_accounts()?;
        let account = accounts.first().ok_or("acctdata.txt holds no account")?;
        harness
            .send(
                &request(b"INQA", account_key(account)?),
                with_ids("CDRA-ACK", "CDRA-ACK-CORREL"),
            )
            .await?;

        // The worker answers while the test holds the reply's confirmation, then releases it.
        let held = harness.hold(&gate, &record, 1, REPLY_QUEUE, ConfirmGate::release);
        let (processed, held) = join(settle_one(&mut worker, &gated), held).await;
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
        let reply = harness.take(REPLY_QUEUE).await?;
        assert_body(
            &reply.delivery.data,
            &expected_found_reply(account)?,
            "found reply",
        );

        // The double settled nothing on the broker, so closing the worker's connection returns
        // the request: the acknowledgement went through the double alone.
        close_gated(&gated).await?;
        let returned = harness.take(REQUEST_QUEUE).await?;
        assert!(
            returned.delivery.redelivered,
            "the request comes back redelivered"
        );
        Ok(())
    })
}

// audit: COACCT01-026
// audit: COACCT01-032
// audit: COACCT01-033
#[test]
fn refused_reply_confirmation_acks_only_after_the_confirmed_mqput_err() -> TestResult {
    block_on(async {
        let harness = Harness::start().await?;
        let record = Arc::new(Record::default());
        let (gate, gated, mut worker) = harness.gated_worker(&record).await?;
        harness
            .send(
                &request(b"INQA", b"00000000001"),
                with_ids("CDRA-REFUSED", "CDRA-REFUSED-CORREL"),
            )
            .await?;

        // The reply's confirmation is held, then refused; the MQPUT ERR message's is held, then
        // released.
        let holds = async {
            let reply = harness
                .hold(&gate, &record, 1, REPLY_QUEUE, ConfirmGate::refuse)
                .await?;
            let error = harness
                .hold(&gate, &record, 2, ERROR_QUEUE, ConfirmGate::release)
                .await?;
            Ok::<_, BoxError>((reply, error))
        };
        let (processed, holds) = join(settle_one(&mut worker, &gated), holds).await;
        let (reply_hold, error_hold) = holds?;
        // The broker queued the reply and only the worker was told Nack, so the reply is on
        // CARD.DEMO.REPLY.ACCT: a refused confirmation, not an unroutable reply.
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
        assert_error_body(&error.delivery.data, MQPUT_ERR, REPLY_QUEUE);
        assert_copied_properties(
            &error.delivery.properties,
            Some("CDRA-REFUSED"),
            Some("CDRA-REFUSED-CORREL"),
            "MQPUT ERR error message",
        );

        // The double settled nothing on the broker, so closing the worker's connection returns
        // the request: the acknowledgement went through the double alone.
        close_gated(&gated).await?;
        let returned = harness.take(REQUEST_QUEUE).await?;
        assert!(
            returned.delivery.redelivered,
            "the request comes back redelivered"
        );
        Ok(())
    })
}

// audit: COACCT01-030
// audit: COACCT01-031
#[test]
#[ignore = "BLOCKED: HALTED(H4) CICS/MQ codes in MQ-ERR-DISPLAY; no parity assertion (AAP 0.8.5)"]
fn blocked_h4_codes() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COACCT01-030 and COACCT01-031 (H4): bytes 54-55 and 58-62 of the error body \
         hold CICS and MQ codes that PostgreSQL and AMQP have no counterpart for"
            .into(),
    )
}

// audit: COACCT01-001
// audit: COACCT01-026
// audit: COACCT01-027
#[test]
#[ignore = "BLOCKED: HALTED(X-COACCT01-H4) delivery of a reply put before the ADD to MQ-MSG-COUNT; no parity assertion (AAP 0.8.5)"]
fn blocked_delivery_parity() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COACCT01-001, COACCT01-026 and COACCT01-027 (X-COACCT01-H4): whether the \
         COBOL delivers a reply it put depends on uninitialised storage"
            .into(),
    )
}
