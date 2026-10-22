//! i10, the credit limit (AAP 0.3.4.2, E30; COPAUA0C-050 of `modern/audit/messaging/COPAUA0C.md`):
//! after every committed CP00 authorization, `acct_curr_bal + approved_auth_amt ≤
//! acct_credit_limit` for the account, with two `carddemo-mq` processes competing for the
//! authorization queue and every request delivered twice.
//!
//! - One broker and one fixture-loaded database come from `test_support`, whose harness applied
//!   the migrations; neither this test nor the spawned binary changes the schema.
//! - Fixture account `00000000001` has a card in `card_xref` and no summary, and 50 requests of
//!   100.00 ask more than its limit leaves above its balance. Only k = ⌊(L − B) / A⌋ of the 100
//!   deliveries can be approved: the i10 guard declines the rest, where COPAUA0C's own rule
//!   alone would approve more (COPAUA0C-046, E30).
//! - Only the `SELECT … FOR UPDATE` lock on the summary row, which the account's first
//!   authorization creates (COPAUA0C-025), serialises the two processes and the redelivered
//!   duplicates (COPAUA0C-026); no duplicate is suppressed, so each consumes limit as a request
//!   of its own.
//! - Not a parity test. A reply is read only within the 63 bytes the reply `STRING` writes, for
//!   the response code: the 64th byte and the body length are `HALTED(H6)`. The summary's
//!   declined amount (`HALTED(H2)`) is not read.
//!
//! The only messaging test that spawns the binary; the others run the workers in process.

use std::collections::HashMap;
use std::error::Error;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use copybook::{PicS, PicU};
use domain::messaging::copaua0c::REPLY_WRITTEN_LEN;
use domain::records::{AccountRecord, CardXrefRecord, ErrorLogRecord, PendingAuthDetailRecord};
use lapin::message::BasicGetMessage;
use lapin::options::ConfirmSelectOptions;
use lapin::types::ShortString;
use lapin::{BasicProperties, Channel, Connection};
use persistence::{
    AccountRepository, CardXrefRepository, CsslErrorLogRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PgStore,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use test_support::fixtures::start_loaded_postgres;
use test_support::rabbitmq::{self, RabbitMqGuard, start_rabbitmq};
use tokio::time;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// Fixture account `00000000001` (`acctdata.txt:1`): balance 194.00, limit 2,020.00, the card of
/// `cardxref.txt:49` and no summary, so k = 18 where COPAUA0C's own rule would approve 20.
const ACCOUNT: u64 = 1;

/// The distinct requests, and how often each is published.
const REQUESTS: usize = 50;
const COPIES: usize = 2;
const DELIVERIES: usize = REQUESTS * COPIES;

/// A = 100.00 in cents, the amount of every request.
const AMOUNT_CENTS: i64 = 10_000;

/// The default `CARDDEMO_QUEUE_REQUEST_PAUTH`, which carries the dead-letter arguments of
/// `rabbitmq/definitions.json`, and its dead-letter queue.
const REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.PAUTH";
const DEAD_LETTER_QUEUE: &str = "CARD.DEMO.DEADLETTER.PAUTH";

const WORKER_PROCESSES: usize = 2;

/// Bound on opening the test's channel in confirm mode.
const CHANNEL_WAIT: Duration = Duration::from_secs(10);
/// Bound on collecting the 100 replies.
const REPLY_WAIT: Duration = Duration::from_secs(180);
/// One `get` poll of the reply queue while replies are expected. CP00 replies expire after
/// 5 seconds (E16), so the queue is polled without pause.
const REPLY_POLL: Duration = Duration::from_millis(500);
/// How long a 101st reply is waited for.
const QUIET_WAIT: Duration = Duration::from_secs(3);
/// Bound on the writes that follow the acknowledgement (E29), and the interval between reads.
const SETTLE_WAIT: Duration = Duration::from_secs(60);
const SETTLE_POLL: Duration = Duration::from_millis(200);

/// The response code is the fourth comma-delimited reply field (CCPAURLY-004).
const RESPONSE_CODE_FIELD: usize = 3;
const APPROVED: &[u8] = b"00";
const DECLINED: &[u8] = b"05";

/// The spawned `carddemo-mq` processes, killed and reaped when the guard drops, so that a failed
/// assertion leaves no process behind.
struct ProcessGuard(Vec<Child>);

impl ProcessGuard {
    /// Fails when a worker process has already ended: its output is discarded, so the exit
    /// status is the only trace of a start-up failure, which would otherwise surface only as
    /// replies missing at [`REPLY_WAIT`].
    fn ensure_running(&mut self) -> TestResult {
        for child in &mut self.0 {
            if let Some(status) = child.try_wait()? {
                return Err(format!("a carddemo-mq process ended early: {status}").into());
            }
        }
        Ok(())
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        for child in &mut self.0 {
            // Both fail only when the process has already ended and been reaped.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// What the test reads of one reply: its correlation ID and its response code, taken from the
/// 63 written bytes only (`HALTED(H6)`).
struct Reply {
    correlation_id: Option<String>,
    response_code: Option<Vec<u8>>,
}

impl Reply {
    fn read(message: &BasicGetMessage) -> Self {
        Self {
            correlation_id: message
                .properties
                .correlation_id()
                .as_ref()
                .map(|id| id.as_str().to_owned()),
            response_code: message
                .data
                .get(..REPLY_WRITTEN_LEN)
                .and_then(|written| written.split(|&byte| byte == b',').nth(RESPONSE_CODE_FIELD))
                .map(<[u8]>::to_vec),
        }
    }

    fn is_approved(&self) -> bool {
        self.response_code.as_deref() == Some(APPROVED)
    }
}

/// The account's detail rows and the whole error log, once every delivery is recorded.
struct Settled {
    details: Vec<PendingAuthDetailRecord>,
    log: Vec<ErrorLogRecord>,
}

impl Settled {
    /// The number of log rows written at `location`, such as `I004`.
    fn logged(&self, location: [u8; 4]) -> usize {
        self.log
            .iter()
            .filter(|record| *record.location.as_bytes() == location)
            .count()
    }
}

// The rows COPAUA0C.md binds to this test, and the summary-lock row COPAUA0C-025 it exercises:
// the account has no summary before the run and exactly one, counted from zero, after it.
// audit: COPAUA0C-025
// audit: COPAUA0C-026
// audit: COPAUA0C-046
// audit: COPAUA0C-049
// audit: COPAUA0C-050
// audit: COPAUA0C-132
#[tokio::test]
async fn two_processes_and_redelivery_never_commit_past_the_credit_limit() -> TestResult {
    let rabbit = start_rabbitmq().await?;
    let pg = start_loaded_postgres().await?;
    let mut store = PgStore::new(pg.pool().clone());
    let (account, xref) = account_under_test(&mut store).await?;
    let amount = Decimal::new(AMOUNT_CENTS, 2);
    let headroom = account.credit_limit.as_decimal() - account.curr_bal.as_decimal();
    assert!(
        Decimal::from(REQUESTS) * amount > headroom,
        "precondition: 50 · A > L − B"
    );
    let k = (headroom / amount)
        .floor()
        .to_usize()
        .ok_or("k = ⌊(L − B) / A⌋ is not a count")?;

    let (_connection, channel) = open_channel(&rabbit).await?;
    let reply_queue = rabbitmq::declare_reply_queue(&channel).await?;
    let correlation_ids =
        publish_requests(&channel, &reply_queue, &xref, &amount.to_string()).await?;

    let mut workers = spawn_workers(pg.database_url(), rabbit.amqp_url())?;
    let replies = collect_replies(&channel, &reply_queue, &mut workers).await?;
    assert!(
        rabbitmq::get(&channel, &reply_queue, QUIET_WAIT)
            .await?
            .is_none(),
        "a reply beyond one per delivery arrived (COPAUA0C-132)"
    );
    let settled = settle(&mut store, PicS::from(account.id)).await?;
    drop(workers);
    for queue in [REQUEST_QUEUE, DEAD_LETTER_QUEUE] {
        let left = rabbitmq::message_count(&channel, queue).await?;
        assert_eq!(left, 0, "{queue} still holds messages");
    }

    assert_replies(&replies, &correlation_ids, k);
    assert_committed(&mut store, &account, k, amount).await?;
    assert_details(&settled, k);
    Ok(())
}

/// The fixture account and its cross-reference, read from the loaded database, after checking
/// that the account has no summary yet: its first authorization creates the summary and every
/// later one finds it (COPAUA0C-025).
async fn account_under_test(store: &mut PgStore) -> TestResult<(AccountRecord, CardXrefRecord)> {
    let id = PicU::<11>::new(ACCOUNT)?;
    let account = AccountRepository::find_by_id(&mut *store, id)
        .await?
        .ok_or("fixture account 00000000001 is not loaded")?;
    let xref = CardXrefRepository::find_by_acct_id(&mut *store, id)
        .await?
        .ok_or("fixture account 00000000001 has no card")?;
    let summary = PendingAuthSummaryRepository::find_by_id(&mut *store, PicS::from(id)).await?;
    assert!(
        summary.is_none(),
        "precondition: the account has no summary"
    );
    Ok((account, xref))
}

/// A connection and a channel on it in confirm mode, so that every request is in its queue
/// before the workers start. The connection is returned to keep it open while the channel is in
/// use.
async fn open_channel(rabbit: &RabbitMqGuard) -> TestResult<(Connection, Channel)> {
    let connection = rabbit.connect().await?;
    let opened = time::timeout(CHANNEL_WAIT, async {
        let channel = connection.create_channel().await?;
        channel
            .confirm_select(ConfirmSelectOptions::default())
            .await?;
        Ok::<_, lapin::Error>(channel)
    })
    .await
    .map_err(|_| "the broker did not open the test's channel in time")?;
    Ok((connection, opened?))
}

/// Request `index` as its 18 comma-delimited `CCPAURQY` fields, asking `amount` for `card`. Its
/// transaction ID (field 18) is its own and short enough that no reply screen withholds it
/// (COPAUA0C-062).
fn request_body(card: &[u8], index: usize, amount: &str) -> Vec<u8> {
    let transaction_id = format!("TXN {index:04}");
    let fields: [&[u8]; 18] = [
        b"220618",
        b"143012",
        card,
        b"0100",
        b"0927",
        b"0200",
        b"POS",
        b"000000",
        amount.as_bytes(),
        b"5411",
        b"840",
        b"05",
        b"MERCHANT0000001",
        b"CORNER GROCERY",
        b"SEATTLE",
        b"WA",
        b"98101",
        transaction_id.as_bytes(),
    ];
    fields.join(b",".as_slice())
}

/// Publishes [`REQUESTS`] distinct requests for the card of `xref`, each [`COPIES`] times with
/// the same body and properties, the copies back to back so that both processes can hold them
/// at once. Returns the correlation IDs; every reply goes to `reply_queue`.
async fn publish_requests(
    channel: &Channel,
    reply_queue: &str,
    xref: &CardXrefRecord,
    amount: &str,
) -> TestResult<Vec<String>> {
    let card = xref.card_num.expose().as_bytes();
    let mut correlation_ids = Vec::with_capacity(REQUESTS);
    for index in 1..=REQUESTS {
        let correlation_id = format!("CP00-I10-{index:02}");
        let body = request_body(card, index, amount);
        let properties = BasicProperties::default()
            .with_correlation_id(ShortString::from(correlation_id.as_str()))
            .with_reply_to(ShortString::from(reply_queue));
        for _ in 0..COPIES {
            rabbitmq::publish(channel, REQUEST_QUEUE, &body, properties.clone()).await?;
        }
        correlation_ids.push(correlation_id);
    }
    Ok(correlation_ids)
}

/// Starts [`WORKER_PROCESSES`] `carddemo-mq` processes on the test's database and broker, from
/// the `modern/` directory. The request-queue variables are removed so that every process
/// consumes the default queues this test publishes to, and span export is off.
fn spawn_workers(database_url: &str, amqp_url: &str) -> TestResult<ProcessGuard> {
    let mut workers = ProcessGuard(Vec::with_capacity(WORKER_PROCESSES));
    for _ in 0..WORKER_PROCESSES {
        let child = Command::new(env!("CARGO_BIN_EXE_carddemo-mq"))
            .current_dir(test_support::modern_dir())
            .env("DATABASE_URL", database_url)
            .env("CARDDEMO_AMQP_URL", amqp_url)
            .env("CARDDEMO_MQ_POLL_MS", "20")
            .env("RUST_LOG", "warn")
            // Removals rather than `env_clear`: the inherited environment carries what a runner
            // sets for child processes, such as cargo-llvm-cov's `LLVM_PROFILE_FILE`.
            .env_remove("CARDDEMO_QUEUE_REQUEST_DATE")
            .env_remove("CARDDEMO_QUEUE_REQUEST_ACCT")
            .env_remove("CARDDEMO_QUEUE_REQUEST_PAUTH")
            .env_remove("CARDDEMO_OTEL_EXPORT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        workers.0.push(child);
    }
    Ok(workers)
}

/// Takes replies from `reply_queue` until one per delivery has arrived, within [`REPLY_WAIT`],
/// failing as soon as a worker process ends.
async fn collect_replies(
    channel: &Channel,
    reply_queue: &str,
    workers: &mut ProcessGuard,
) -> TestResult<Vec<Reply>> {
    let mut replies = Vec::with_capacity(DELIVERIES);
    let collected = time::timeout(REPLY_WAIT, async {
        while replies.len() < DELIVERIES {
            workers.ensure_running()?;
            if let Some(message) = rabbitmq::get(channel, reply_queue, REPLY_POLL).await? {
                replies.push(Reply::read(&message));
            }
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await;
    match collected {
        Ok(outcome) => outcome.map(|()| replies),
        Err(_) => Err(format!(
            "{} of {DELIVERIES} replies arrived within {} s",
            replies.len(),
            REPLY_WAIT.as_secs()
        )
        .into()),
    }
}

/// Waits, within [`SETTLE_WAIT`], until every delivery of account `acct` is recorded. CP00
/// writes only after it has acknowledged the request (E29), so a reply can precede its rows.
/// Each delivery leaves one detail row, or one `I004` row when its detail key collided with
/// another's (COPAUA0C-099, COPAUA0C-120).
async fn settle(store: &mut PgStore, acct: PicS<11>) -> TestResult<Settled> {
    time::timeout(SETTLE_WAIT, async {
        loop {
            let details =
                PendingAuthDetailRepository::find_all_by_acct_id(&mut *store, acct).await?;
            let log = CsslErrorLogRepository::find_all(&mut *store)
                .await?
                .into_iter()
                .map(|(_, record)| record)
                .collect();
            let settled = Settled { details, log };
            if settled.details.len() + settled.logged(*b"I004") == DELIVERIES {
                return Ok::<_, Box<dyn Error>>(settled);
            }
            time::sleep(SETTLE_POLL).await;
        }
    })
    .await
    .map_err(|_| {
        format!(
            "the authorization writes did not settle within {} s",
            SETTLE_WAIT.as_secs()
        )
    })?
}

/// One reply per delivery, both copies of every request answered, and exactly k approvals among
/// them (COPAUA0C-132, COPAUA0C-046).
fn assert_replies(replies: &[Reply], correlation_ids: &[String], k: usize) {
    assert_eq!(
        replies.len(),
        DELIVERIES,
        "one reply per delivery (COPAUA0C-132)"
    );
    let mut answered: HashMap<&str, usize> = HashMap::new();
    for reply in replies {
        *answered
            .entry(reply.correlation_id.as_deref().unwrap_or_default())
            .or_default() += 1;
    }
    let expected: HashMap<&str, usize> = correlation_ids
        .iter()
        .map(|id| (id.as_str(), COPIES))
        .collect();
    assert_eq!(
        answered, expected,
        "every copy of every request is answered once (COPAUA0C-132)"
    );
    assert!(
        replies
            .iter()
            .all(|reply| reply.is_approved() || reply.response_code.as_deref() == Some(DECLINED)),
        "every reply approves (00) or declines (05)"
    );
    let approvals = replies.iter().filter(|reply| reply.is_approved()).count();
    assert_eq!(approvals, k, "k = ⌊(L − B) / A⌋ approvals (COPAUA0C-046)");
}

/// The committed state after the run: the summary holds k approvals of A and the 100 − k
/// declines, the account's balance and limit are those read before (CP00 never writes them),
/// and i10 holds over the stored values (COPAUA0C-049, COPAUA0C-050).
async fn assert_committed(
    store: &mut PgStore,
    before: &AccountRecord,
    k: usize,
    amount: Decimal,
) -> TestResult {
    let summary = PendingAuthSummaryRepository::find_by_id(&mut *store, PicS::from(before.id))
        .await?
        .ok_or("no summary was committed")?;
    let account = AccountRepository::find_by_id(&mut *store, before.id)
        .await?
        .ok_or("the fixture account is gone")?;
    assert_eq!(
        summary.approved_auth_cnt.get(),
        i16::try_from(k)?,
        "approved count"
    );
    assert_eq!(
        summary.declined_auth_cnt.get(),
        i16::try_from(DELIVERIES - k)?,
        "declined count"
    );
    let approved_amt = summary.approved_auth_amt.as_decimal();
    assert_eq!(
        approved_amt,
        Decimal::from(k) * amount,
        "approved amount k · A"
    );
    let (balance, limit) = (
        account.curr_bal.as_decimal(),
        account.credit_limit.as_decimal(),
    );
    assert_eq!(
        (balance, limit),
        (
            before.curr_bal.as_decimal(),
            before.credit_limit.as_decimal()
        ),
        "CP00 changed the account's balance or limit"
    );
    assert!(
        balance + approved_amt <= limit,
        "i10: B + approved_auth_amt = {} exceeds L = {limit}",
        balance + approved_amt
    );
    Ok(())
}

/// The detail rows and the error log after the run (COPAUA0C-050, X-COPAUA0C-10).
fn assert_details(settled: &Settled, k: usize) {
    let total = settled.details.len();
    let approved = settled
        .details
        .iter()
        .filter(|detail| detail.is_approved())
        .count();
    let collided = settled.logged(*b"I004");
    let bound = approved + collided;
    // The AAP's "detail rows plus I004 rows equal k", read over the approved authorizations.
    // COPAUA0C inserts a detail for every decision on a found card, declines included (match
    // status `D`, `COPAUA0C.cbl:463-465`), so declines add details of their own, and every
    // delivery leaves a detail or an `I004` row. Two processes reading the system clock can give
    // two decisions the same millisecond key, whose second insert fails as `I004` with the
    // summary committed alone (COPAUA0C-120); an `I004` row does not say whether its request was
    // approved, so k lies between the approved details and those plus every `I004` row.
    println!(
        "i10 detail reading: {total} detail rows + {collided} I004 rows = {DELIVERIES} \
         deliveries; {approved} approved details <= k = {k} <= {bound}"
    );
    assert_eq!(
        total + collided,
        DELIVERIES,
        "every delivery leaves a detail row or an I004 row"
    );
    assert!(
        approved <= k && k <= bound,
        "approved details {approved} and I004 rows {collided} do not bound k = {k}"
    );
    for location in [*b"I003", *b"M004"] {
        assert_eq!(settled.logged(location), 0, "a CP00 write or reply failed");
    }
}
