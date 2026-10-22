//! Invariant i11 of CP00 (AAP 0.3.4.2, E31): every request removed from the authorization
//! request queue gets exactly one reply, except a request whose card number cannot be read, which
//! gets no reply, one `CP00_CARD_UNPARSEABLE` alert and a place on `CARD.DEMO.DEADLETTER.PAUTH`.
//!
//! This is not a parity test. Messages are delivered at least once, so the mixed batch publishes
//! every request twice and counts replies per removal. The CP00 worker runs in process, on the
//! test's thread, against a `rabbitmq:3.13-management` container booted from
//! `modern/rabbitmq/definitions.json` and a migrated, fixture-loaded `postgres:16` container,
//! both started through `test_support`; nothing here migrates.
//!
//! Every reply is read through its 63 written bytes only: the body's length and its 64th byte are
//! `HALTED(H6)`, so no test here asserts either. Each reply is taken as soon as the step that
//! published it returns, because a CP00 reply expires five seconds after its publication (E16).
//!
//! Fixture card numbers are used only to build request bodies and as search needles; no
//! assertion message formats one (Cardholder Data Redaction).

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use copybook::{PicS, PicU};
use domain::messaging::copaua0c::{ALERT_CARD_UNPARSEABLE, GET_BUFFER_LEN, REPLY_WRITTEN_LEN};
use domain::{CardNumber, Clock, SystemClock};
use lapin::message::BasicGetMessage;
use lapin::options::ConfirmSelectOptions;
use lapin::types::ShortString;
use lapin::{BasicProperties, Channel, Connection};
use messaging::broker::DEAD_LETTER_QUEUE;
use messaging::workers::cp00::Worker;
use messaging::workers::{Processed, Step};
use persistence::{
    AccountRepository, CardXrefRepository, CsslErrorLogRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PgStore,
};
use runtime::config::Config;
use runtime::telemetry::ALERT_TARGET;
use rust_decimal::Decimal;
use test_support::fixtures::{fixture_card_numbers, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use test_support::rabbitmq::{self, RabbitMqGuard};
use test_support::trace_capture;
use tokio::time;
use tracing::instrument::WithSubscriber;

type TestError = Box<dyn Error + Send + Sync>;
type TestResult = Result<(), TestError>;

/// Bound on one `process_next` of the worker, a broker round trip and a few statements.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on the whole mixed batch.
const BATCH_TIMEOUT: Duration = Duration::from_secs(240);
/// How long a replied step's reply is awaited; it is on the queue once the step returns.
const REPLY_WAIT: Duration = Duration::from_secs(5);
/// How long the reply queue is watched for a reply that must never come.
const NO_REPLY_WAIT: Duration = Duration::from_secs(3);
/// Bound on the broker moving rejected requests to the dead-letter queue.
const QUEUE_SETTLE: Duration = Duration::from_secs(10);
/// How long a queue's length must hold once it is expected to be final.
const STAY_WINDOW: Duration = Duration::from_secs(1);
/// Interval between two readings of a queue's length.
const POLL: Duration = Duration::from_millis(100);
/// Bound on the close of the worker's connection and on the client channel's confirm mode.
const BROKER_OP_TIMEOUT: Duration = Duration::from_secs(10);
/// AMQP 0-9-1 `reply-success`, sent with the close of the worker's connection.
const CLOSE_NORMAL: u16 = 200;

/// Field 9 of a request the decision approves, and the approved amount its reply echoes.
const APPROVED_AMOUNT: &str = "100.00";
/// The amount field of an approval of [`APPROVED_AMOUNT`]: `-zzzzzzzzz9.99` gives eight spaces
/// then `100.00`.
const APPROVED_AMOUNT_FIELD: &str = "        100.00";
/// The amount field of every decline: `-zzzzzzzzz9.99` gives ten spaces then `0.00`.
const ZERO_AMOUNT_FIELD: &str = "          0.00";
/// Field 9 that is no `NUMVAL` argument.
const NON_NUMERIC_AMOUNT: &str = "ABC";
/// A card number of 16 ASCII digits that no `card_xref` row holds.
const CARD_NOT_IN_XREF: &str = "0000000000000017";
/// Field 3 values that are not exactly 16 ASCII digits: letters, 15 digits, 17 digits.
const UNPARSEABLE_CARDS: [&str; 3] = ["ABC", "000000000000015", "00000000000000017"];

/// Distinctive request fields, which the capture of the unparseable request must never hold.
const MERCHANT_ID: &str = "LIVENESSMERCH01";
const MERCHANT_NAME: &str = "LIVENESS PROBE MART";
const MERCHANT_CITY: &str = "PROBEVILLE";
/// Field 18. Its space keeps every run below eight non-space bytes, so the reply echoes it.
const TRANSACTION_ID: &str = "LIV 0001";

/// The fixture accounts whose cards are approved twice for [`APPROVED_AMOUNT`].
const APPROVAL_ACCOUNTS: [u64; 3] = [1, 2, 3];
/// The fixture accounts whose cards ask for one cent more than their credit headroom allows.
const DECLINE_ACCOUNTS: [u64; 2] = [4, 5];
/// The fixture account whose card carries every field error.
const FIELD_ERROR_ACCOUNT: u64 = 6;
/// The fixture account whose card carries the field error of the single-request test.
const SINGLE_ACCOUNT: u64 = 1;

/// The 0-based positions of fields 8 (`PA-RQ-PROCESSING-CODE 9(06)`) and 15
/// (`PA-RQ-MERCHANT-CITY X(13)`) in [`request_fields`].
const PROCESSING_CODE_FIELD: usize = 7;
const MERCHANT_CITY_FIELD: usize = 14;

/// The 18 comma-delimited `CCPAURQY` fields of a request, in `UNSTRING` order
/// (`COPAUA0C.cbl:354-378`), with `card` as field 3 and `amount` as field 9.
fn request_fields(card: &str, amount: &str) -> [String; 18] {
    [
        "220718",
        "102030",
        card,
        "0100",
        "1227",
        "AUTHRQ",
        "POS001",
        "000000",
        amount,
        "5411",
        "840",
        "01",
        MERCHANT_ID,
        MERCHANT_NAME,
        MERCHANT_CITY,
        "TX",
        "75024",
        TRANSACTION_ID,
    ]
    .map(str::to_owned)
}

/// The body of `fields`, joined with the `UNSTRING` delimiter.
fn body_of(fields: &[String]) -> Vec<u8> {
    fields.join(",").into_bytes()
}

/// One body per field error of a parseable `card`, each with a single fault: fewer than 18
/// fields, an amount that is no `NUMVAL` argument, a non-digit in a `9(n)` field, a body longer
/// than the 500-byte get buffer, and a byte outside printable ASCII.
fn field_error_bodies(card: &str) -> [Vec<u8>; 5] {
    let complete = request_fields(card, APPROVED_AMOUNT);
    let short = body_of(&complete[..17]);
    let non_numeric = body_of(&request_fields(card, NON_NUMERIC_AMOUNT));
    let mut non_digit = complete.clone();
    non_digit[PROCESSING_CODE_FIELD] = String::from("12A456");
    let mut too_long = complete.to_vec();
    too_long.push("Z".repeat(GET_BUFFER_LEN));
    let mut non_printable = complete;
    non_printable[MERCHANT_CITY_FIELD] = String::from("PROBE\tVILLE");
    [
        short,
        non_numeric,
        body_of(&non_digit),
        body_of(&too_long),
        body_of(&non_printable),
    ]
}

/// The `message_id` and `correlation_id` a request carries; CP00 replies copy the latter.
struct Ids {
    message_id: String,
    correlation_id: String,
}

impl Ids {
    fn new(tag: &str) -> Self {
        Self {
            message_id: format!("MSG-{tag}"),
            correlation_id: format!("CORR-{tag}"),
        }
    }
}

/// The text the worker's events show for an ID: one `*` per byte, never the ID itself.
fn screened(id: &str) -> String {
    "*".repeat(id.len())
}

/// What a mixed-batch request is, and so what each of its replies must carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Approval,
    InsufficientFunds,
    CardNotFound,
    FieldError,
    UnparseableCard,
}

impl Kind {
    /// The response code, reason and amount field of every reply of this kind, or `None` for
    /// the kind that gets no reply.
    const fn reply(self) -> Option<(&'static str, &'static str, &'static str)> {
        match self {
            Self::Approval => Some(("00", "0000", APPROVED_AMOUNT_FIELD)),
            Self::InsufficientFunds => Some(("05", "4100", ZERO_AMOUNT_FIELD)),
            Self::CardNotFound => Some(("05", "3100", ZERO_AMOUNT_FIELD)),
            Self::FieldError => Some(("05", "9000", ZERO_AMOUNT_FIELD)),
            Self::UnparseableCard => None,
        }
    }
}

/// One request of the mixed batch: its kind, the card field it sends, its body and its IDs.
struct Probe {
    kind: Kind,
    card: String,
    body: Vec<u8>,
    ids: Ids,
}

/// A reply, read through its 63 written bytes: six fields, each followed by `,`. The fields
/// after the card are the transaction ID, the auth-ID code, then fields 4 to 6, the response
/// code, the reason and the amount. It derives no `Debug`, so a failing assertion cannot print
/// a card field that was not redacted.
struct Reply {
    correlation_id: Option<String>,
    card: String,
    response: String,
    reason: String,
    amount: String,
}

impl Reply {
    /// Reads `message` through its first [`REPLY_WRITTEN_LEN`] bytes, never its length or the
    /// byte after them (`HALTED(H6)`).
    fn read(message: &BasicGetMessage) -> Result<Self, TestError> {
        let written = message
            .delivery
            .data
            .get(..REPLY_WRITTEN_LEN)
            .ok_or("a reply is shorter than its 63 written bytes")?;
        let fields: Vec<&str> = std::str::from_utf8(written)?.split(',').collect();
        let [
            card,
            _transaction_id,
            _auth_id_code,
            response,
            reason,
            amount,
            end,
        ] = fields.as_slice()
        else {
            return Err(format!(
                "a reply holds {} comma-delimited parts, not 7",
                fields.len()
            )
            .into());
        };
        if !end.is_empty() {
            return Err("a reply's written bytes do not end with a comma".into());
        }
        Ok(Self {
            correlation_id: message
                .delivery
                .properties
                .correlation_id()
                .as_ref()
                .map(|id| id.as_str().to_owned()),
            card: (*card).to_owned(),
            response: (*response).to_owned(),
            reason: (*reason).to_owned(),
            amount: (*amount).to_owned(),
        })
    }
}

/// The authorization tables and the error log, counted to show that a request wrote nothing.
#[derive(Debug, PartialEq, Eq)]
struct Counts {
    summaries: usize,
    details: usize,
    log_rows: usize,
}

/// A fixture card with its account's credit limit and current balance.
struct FixtureCard {
    card: String,
    credit_limit: Decimal,
    curr_bal: Decimal,
}

/// What the worker did with the requests of one drain: the replies taken after each replied
/// step and the number of dead-lettered removals.
#[derive(Default)]
struct Drained {
    replies: Vec<Reply>,
    dead_lettered: usize,
}

/// One test's broker, database and in-process CP00 worker, with a client channel in confirm
/// mode that publishes the requests and reads the reply, request and dead-letter queues.
///
/// Fields drop in declaration order: the worker and both connections before the containers.
struct Harness {
    worker: Worker,
    worker_connection: Connection,
    client: Channel,
    _client_connection: Connection,
    reply_queue: String,
    request_queue: String,
    store: PgStore,
    _postgres: PostgresGuard,
    _rabbit: RabbitMqGuard,
}

impl Harness {
    /// Starts both containers, loads the fixtures, opens the CP00 worker on its own connection
    /// with the system clock, and declares the test's reply queue.
    async fn start() -> Result<Self, TestError> {
        let (rabbit, postgres) =
            tokio::try_join!(rabbitmq::start_rabbitmq(), start_loaded_postgres())?;
        let config = Config {
            amqp_url: rabbit.amqp_url().to_owned(),
            database_url: postgres.database_url().to_owned(),
            ..Config::default()
        };
        let store = PgStore::new(postgres.pool().clone());
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let worker_connection = rabbit.connect().await?;
        let worker = Worker::open(&worker_connection, &config, store.clone(), clock).await?;
        let client_connection = rabbit.connect().await?;
        let client = time::timeout(BROKER_OP_TIMEOUT, client_connection.create_channel()).await??;
        time::timeout(
            BROKER_OP_TIMEOUT,
            client.confirm_select(ConfirmSelectOptions::default()),
        )
        .await??;
        let reply_queue = rabbitmq::declare_reply_queue(&client).await?;
        Ok(Self {
            worker,
            worker_connection,
            client,
            _client_connection: client_connection,
            reply_queue,
            request_queue: config.queue_request_pauth,
            store,
            _postgres: postgres,
            _rabbit: rabbit,
        })
    }

    /// Publishes `body` to the request queue with `ids` and the test's reply queue as
    /// `reply_to`, and returns once the broker has confirmed it.
    async fn send(&self, body: &[u8], ids: &Ids) -> TestResult {
        let properties = BasicProperties::default()
            .with_message_id(ShortString::from(ids.message_id.as_str()))
            .with_correlation_id(ShortString::from(ids.correlation_id.as_str()))
            .with_reply_to(ShortString::from(self.reply_queue.as_str()));
        rabbitmq::publish(&self.client, &self.request_queue, body, properties).await?;
        Ok(())
    }

    /// One `process_next` of the worker, bounded by [`STEP_TIMEOUT`]. A panic in the worker,
    /// such as a parser that is not total, fails the test here.
    async fn step(&mut self) -> Result<Processed, TestError> {
        let next = Box::pin(self.worker.process_next(&self.worker_connection));
        Ok(time::timeout(STEP_TIMEOUT, next).await??)
    }

    /// The reply the last replied step published, read through its written bytes, after which
    /// no other reply may be waiting: one reply per removal.
    async fn take_reply(&self) -> Result<Reply, TestError> {
        let message = rabbitmq::get(&self.client, &self.reply_queue, REPLY_WAIT)
            .await?
            .ok_or("a replied request left no reply on the reply queue")?;
        let reply = Reply::read(&message)?;
        self.assert_no_reply_waiting().await?;
        Ok(reply)
    }

    /// Fails when a reply waits on the reply queue.
    async fn assert_no_reply_waiting(&self) -> TestResult {
        let waiting = rabbitmq::message_count(&self.client, &self.reply_queue).await?;
        assert_eq!(waiting, 0, "a removal left more than one reply");
        Ok(())
    }

    /// Waits until `queue` holds `expected` messages, then fails if it holds more.
    async fn wait_for_len(&self, queue: &str, expected: u32) -> TestResult {
        let reached = time::timeout(QUEUE_SETTLE, async {
            loop {
                let len = rabbitmq::message_count(&self.client, queue).await?;
                if len >= expected {
                    return Ok::<u32, TestError>(len);
                }
                time::sleep(POLL).await;
            }
        })
        .await??;
        assert_eq!(
            reached, expected,
            "{queue} holds more messages than expected"
        );
        Ok(())
    }

    /// Fails unless `queue` holds `expected` messages at every reading over [`STAY_WINDOW`].
    async fn assert_len_stays(&self, queue: &str, expected: u32) -> TestResult {
        let changed = time::timeout(STAY_WINDOW, async {
            loop {
                let len = rabbitmq::message_count(&self.client, queue).await?;
                if len != expected {
                    return Ok::<u32, TestError>(len);
                }
                time::sleep(POLL).await;
            }
        })
        .await;
        match changed {
            Err(_held) => Ok(()),
            Ok(len) => Err(format!("{queue} holds {} messages, not {expected}", len?).into()),
        }
    }

    /// Closes the worker's connection. A request it took and never settled would return to the
    /// request queue now, so an empty request queue afterwards shows every removal settled.
    async fn close_worker(&self) -> TestResult {
        time::timeout(
            BROKER_OP_TIMEOUT,
            self.worker_connection
                .close(CLOSE_NORMAL, "cp00_liveness done"),
        )
        .await??;
        Ok(())
    }

    /// The card of fixture account `acct`, read through `CardXrefRepository::find_by_acct_id`,
    /// with the account's credit limit and current balance.
    async fn fixture_card(&self, acct: u64) -> Result<FixtureCard, TestError> {
        let acct_id = PicU::<11>::new(acct)?;
        let mut store = self.store.clone();
        let xref = CardXrefRepository::find_by_acct_id(&mut store, acct_id)
            .await?
            .ok_or("a fixture account has no cross-reference")?;
        let account = AccountRepository::find_by_id(&mut store, acct_id)
            .await?
            .ok_or("a fixture account has no account row")?;
        Ok(FixtureCard {
            card: String::from_utf8(xref.card_num.expose().as_bytes().to_vec())?,
            credit_limit: account.credit_limit.as_decimal(),
            curr_bal: account.curr_bal.as_decimal(),
        })
    }

    /// The summary rows, the detail rows under every fixture account and the error-log rows.
    async fn counts(&self) -> Result<Counts, TestError> {
        let mut store = self.store.clone();
        let summaries = PendingAuthSummaryRepository::find_all(&mut store)
            .await?
            .len();
        let mut details = 0;
        for xref in CardXrefRepository::find_all(&mut store).await? {
            let acct = PicS::from(xref.acct_id);
            details += PendingAuthDetailRepository::find_all_by_acct_id(&mut store, acct)
                .await?
                .len();
        }
        let log_rows = CsslErrorLogRepository::find_all(&mut store).await?.len();
        Ok(Counts {
            summaries,
            details,
            log_rows,
        })
    }
}

/// The requests of the mixed batch: approvals on distinct accounts, insufficient-funds declines
/// on two others, a well-formed card with no cross-reference, every field error on a sixth
/// account's card, and the three unparseable cards.
async fn mixed_probes(harness: &Harness) -> Result<Vec<Probe>, TestError> {
    // Two approvals of 100.00 per account, the request and its second delivery.
    let twice_approved = Decimal::new(20_000, 2);
    let mut probes = Vec::new();
    for acct in APPROVAL_ACCOUNTS {
        let fixture = harness.fixture_card(acct).await?;
        assert!(
            fixture.curr_bal >= Decimal::ZERO
                && fixture.credit_limit - fixture.curr_bal >= twice_approved,
            "fixture account {acct} cannot approve 100.00 twice"
        );
        let body = body_of(&request_fields(&fixture.card, APPROVED_AMOUNT));
        push_probe(&mut probes, Kind::Approval, &fixture.card, body);
    }
    for acct in DECLINE_ACCOUNTS {
        let fixture = harness.fixture_card(acct).await?;
        let headroom = fixture.credit_limit - fixture.curr_bal;
        assert!(
            headroom > Decimal::ZERO,
            "fixture account {acct} has no credit headroom"
        );
        // One cent above the account's limit minus its balance.
        let amount = (headroom + Decimal::new(1, 2)).to_string();
        let body = body_of(&request_fields(&fixture.card, &amount));
        push_probe(&mut probes, Kind::InsufficientFunds, &fixture.card, body);
    }
    let unknown = CardNumber::parse(CARD_NOT_IN_XREF)?;
    let mut store = harness.store.clone();
    assert!(
        CardXrefRepository::find_by_id(&mut store, &unknown)
            .await?
            .is_none(),
        "the card-not-found probe has a cross-reference"
    );
    let body = body_of(&request_fields(CARD_NOT_IN_XREF, APPROVED_AMOUNT));
    push_probe(&mut probes, Kind::CardNotFound, CARD_NOT_IN_XREF, body);
    let fixture = harness.fixture_card(FIELD_ERROR_ACCOUNT).await?;
    for body in field_error_bodies(&fixture.card) {
        push_probe(&mut probes, Kind::FieldError, &fixture.card, body);
    }
    for card in UNPARSEABLE_CARDS {
        let body = body_of(&request_fields(card, APPROVED_AMOUNT));
        push_probe(&mut probes, Kind::UnparseableCard, card, body);
    }
    Ok(probes)
}

/// Appends a probe of `kind` with IDs unique within the batch.
fn push_probe(probes: &mut Vec<Probe>, kind: Kind, card: &str, body: Vec<u8>) {
    let ids = Ids::new(&format!("MIX-{:02}", probes.len()));
    probes.push(Probe {
        kind,
        card: card.to_owned(),
        body,
        ids,
    });
}

/// Steps the worker until it finds the request queue empty, at most `max_steps` times, taking
/// each reply as soon as its step returns. Every removal must be replied or dead-lettered.
async fn drain(harness: &mut Harness, max_steps: usize) -> Result<Drained, TestError> {
    let mut drained = Drained::default();
    for _ in 0..max_steps {
        match harness.step().await? {
            Processed::Idle => return Ok(drained),
            Processed::Replied => drained.replies.push(harness.take_reply().await?),
            Processed::DeadLettered => {
                drained.dead_lettered += 1;
                harness.assert_no_reply_waiting().await?;
            }
            other => {
                return Err(
                    format!("a removal ended {other:?}, not replied or dead-lettered").into(),
                );
            }
        }
    }
    Err(format!("the worker did not find the request queue empty within {max_steps} steps").into())
}

/// Asserts that each probe got the replies its kind gets: two identical outcomes per parseable
/// probe, one per delivery, and none for an unparseable card.
fn assert_replies_per_probe(probes: &[Probe], replies: &[Reply]) -> TestResult {
    let mut by_correlation: BTreeMap<&str, Vec<&Reply>> = BTreeMap::new();
    for reply in replies {
        let id = reply
            .correlation_id
            .as_deref()
            .ok_or("a reply carries no correlation_id")?;
        by_correlation.entry(id).or_default().push(reply);
    }
    for probe in probes {
        let got = by_correlation
            .remove(probe.ids.correlation_id.as_str())
            .unwrap_or_default();
        let id = &probe.ids.correlation_id;
        let Some((response, reason, amount)) = probe.kind.reply() else {
            assert!(got.is_empty(), "{id} ({:?}) got a reply", probe.kind);
            continue;
        };
        assert_eq!(
            got.len(),
            2,
            "{id} ({:?}): one reply per delivery",
            probe.kind
        );
        let redacted = CardNumber::parse(&probe.card)?.redacted();
        for reply in got {
            assert_eq!(
                reply.response, response,
                "{id} ({:?}) response code",
                probe.kind
            );
            assert_eq!(reply.reason, reason, "{id} ({:?}) reason", probe.kind);
            assert_eq!(reply.amount, amount, "{id} ({:?}) amount field", probe.kind);
            assert!(
                reply.card == redacted,
                "{id}: the reply's card field is not redacted"
            );
        }
    }
    assert!(
        by_correlation.is_empty(),
        "replies to no published request: {:?}",
        by_correlation.keys().collect::<Vec<_>>()
    );
    Ok(())
}

// audit: COPAUA0C-008
// audit: COPAUA0C-012
// audit: COPAUA0C-132
#[tokio::test]
async fn valid_card_non_numeric_amount_gets_one_9000_decline_and_no_writes() -> TestResult {
    let mut harness = Harness::start().await?;
    let fixture = harness.fixture_card(SINGLE_ACCOUNT).await?;
    let before = harness.counts().await?;
    let ids = Ids::new("NUMVAL");
    let body = body_of(&request_fields(&fixture.card, NON_NUMERIC_AMOUNT));
    harness.send(&body, &ids).await?;

    assert_eq!(harness.step().await?, Processed::Replied);
    let reply = harness.take_reply().await?;
    assert_eq!(
        reply.correlation_id.as_deref(),
        Some(ids.correlation_id.as_str())
    );
    assert_eq!(reply.response, "05");
    assert_eq!(reply.reason, "9000");
    assert_eq!(reply.amount, ZERO_AMOUNT_FIELD);
    assert!(
        reply.card == CardNumber::parse(&fixture.card)?.redacted(),
        "the reply's card field is not the redacted card number"
    );
    assert_eq!(
        harness.counts().await?,
        before,
        "a field error takes no lock and writes no row"
    );

    // Acknowledged after the confirmed publish: nothing returns to the request queue when the
    // worker's connection closes, and nothing was dead-lettered.
    harness.close_worker().await?;
    harness.assert_len_stays(&harness.request_queue, 0).await?;
    harness.assert_len_stays(DEAD_LETTER_QUEUE, 0).await?;
    Ok(())
}

// audit: COPAUA0C-006
#[tokio::test(flavor = "current_thread")]
async fn unparseable_card_is_dead_lettered_with_alert_and_no_reply() -> TestResult {
    let mut harness = Harness::start().await?;
    let before = harness.counts().await?;
    let ids = Ids::new("UNPARSEABLE");
    let body = body_of(&request_fields("ABC", APPROVED_AMOUNT));
    harness.send(&body, &ids).await?;

    // The capture is the default of this thread, which runs every task of the current-thread
    // runtime, and the step carries its dispatcher as well.
    let capture = trace_capture::capture();
    let processed = harness.step().with_subscriber(capture.dispatch()).await?;
    assert_eq!(processed, Processed::DeadLettered);

    let alerts = capture.lines_with_target(ALERT_TARGET);
    let [alert] = alerts.as_slice() else {
        return Err(format!("{} alert lines, not exactly one", alerts.len()).into());
    };
    for member in [
        r#""level":"ERROR""#.to_owned(),
        format!(r#""alert":"{ALERT_CARD_UNPARSEABLE}""#),
        format!(r#""message_id":"{}""#, screened(&ids.message_id)),
        format!(r#""correlation_id":"{}""#, screened(&ids.correlation_id)),
        format!(r#""body_len":{}"#, body.len()),
    ] {
        assert!(alert.contains(&member), "{member} missing from {alert}");
    }
    let text = capture.text();
    assert_eq!(text.matches(ALERT_CARD_UNPARSEABLE).count(), 1, "{text}");
    let body_text = String::from_utf8(body.clone())?;
    for needle in [
        body_text.as_str(),
        "ABC,",
        ",ABC",
        MERCHANT_ID,
        MERCHANT_NAME,
        MERCHANT_CITY,
        ",102030,",
        ids.message_id.as_str(),
        ids.correlation_id.as_str(),
    ] {
        assert!(!text.contains(needle), "{needle:?} captured in {text}");
    }
    for card in fixture_card_numbers()? {
        assert!(!text.contains(&card), "a fixture card number was captured");
    }
    drop(capture);

    // The broker moved the rejected request, body unchanged, and nothing answered it.
    harness.wait_for_len(DEAD_LETTER_QUEUE, 1).await?;
    let dead = rabbitmq::get(&harness.client, DEAD_LETTER_QUEUE, REPLY_WAIT)
        .await?
        .ok_or("the dead-letter queue is empty")?;
    assert!(dead.delivery.data == body, "the dead-lettered body differs");
    let reply = rabbitmq::get(&harness.client, &harness.reply_queue, NO_REPLY_WAIT).await?;
    assert!(reply.is_none(), "an unparseable card was answered");
    assert_eq!(
        harness.counts().await?,
        before,
        "an unparseable card wrote a row"
    );

    harness.close_worker().await?;
    harness.assert_len_stays(&harness.request_queue, 0).await?;
    Ok(())
}

// audit: COPAUA0C-006
// audit: COPAUA0C-007
// audit: COPAUA0C-008
// audit: COPAUA0C-009
// audit: COPAUA0C-010
// audit: COPAUA0C-011
// audit: COPAUA0C-012
// audit: COPAUA0C-132
#[tokio::test]
async fn mixed_batch_yields_one_reply_per_removal() -> TestResult {
    let mut harness = Harness::start().await?;
    let probes = mixed_probes(&harness).await?;
    // At-least-once delivery: every request reaches the queue twice.
    for _delivery in 0..2 {
        for probe in &probes {
            harness.send(&probe.body, &probe.ids).await?;
        }
    }
    let deliveries = 2 * probes.len();
    let drained = time::timeout(BATCH_TIMEOUT, drain(&mut harness, deliveries + 1)).await??;

    let unparseable = probes
        .iter()
        .filter(|probe| probe.kind == Kind::UnparseableCard)
        .count();
    let parseable = probes.len() - unparseable;
    assert_eq!(
        drained.replies.len() + drained.dead_lettered,
        deliveries,
        "every delivery is removed once"
    );
    assert_eq!(
        drained.replies.len(),
        2 * parseable,
        "one reply per parseable removal"
    );
    assert_eq!(
        drained.dead_lettered,
        2 * unparseable,
        "unparseable removals are dead-lettered"
    );
    harness
        .wait_for_len(DEAD_LETTER_QUEUE, u32::try_from(2 * unparseable)?)
        .await?;
    assert_replies_per_probe(&probes, &drained.replies)?;

    harness.close_worker().await?;
    harness.assert_len_stays(&harness.request_queue, 0).await?;
    harness.assert_len_stays(&harness.reply_queue, 0).await?;
    Ok(())
}

// The body's length and its byte at offset 63 have no single COBOL value; this file compares
// replies through their 63 written bytes only and asserts neither.
#[test]
#[ignore = "BLOCKED: HALTED(H6) CP00 reply length/64th byte; no parity assertion (AAP 0.8.5)"]
fn blocked_h6_reply_length() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: CP00 reply length and 64th byte (H6): the STRING pointer ends one past the 63 \
         written bytes and becomes the reply length, so the byte at offset 63 comes from \
         W02-PUT-BUFFER, which has no VALUE and is never written, and no single COBOL value \
         exists for it"
            .into(),
    )
}
