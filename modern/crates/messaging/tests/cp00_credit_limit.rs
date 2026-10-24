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
//! - Both processes compete at that lock, which the test proves before it accepts the result.
//!   Before they start, the test takes the lock itself, a gate: on a transaction of its own it
//!   makes the worker's own `lock_or_create` call, whose summary row stays uncommitted, so every
//!   decision for the account waits on it. It then waits, within a bound, until the request
//!   queue's ready count shows one delivery held by each process, the two copies of the first
//!   request, checks that no reply got past the gate and rolls the gate back, so the account
//!   still has no summary for its first authorization to create. Each reply's `message_id`
//!   names the process that sent it, and the two copies of the first request must be answered
//!   one by each process.
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
use domain::messaging::copaua0c::{self, REPLY_WRITTEN_LEN};
use domain::records::{AccountRecord, CardXrefRecord, ErrorLogRecord, PendingAuthDetailRecord};
use lapin::message::BasicGetMessage;
use lapin::options::ConfirmSelectOptions;
use lapin::types::ShortString;
use lapin::{BasicProperties, Channel, Connection};
use persistence::{
    AccountRepository, CardXrefRepository, CsslErrorLogRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PgStore, PgTx,
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
/// Bound on every process starting and taking a delivery to the gate's summary lock, and the
/// interval between reads of the request queue's ready count while it is awaited.
const READY_WAIT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(100);
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
    /// status is the only trace of a start-up failure, which would otherwise surface only as a
    /// gate no delivery reaches within [`READY_WAIT`] or as replies missing at [`REPLY_WAIT`].
    fn ensure_running(&mut self) -> TestResult {
        for child in &mut self.0 {
            if let Some(status) = child.try_wait()? {
                return Err(format!("a carddemo-mq process ended early: {status}").into());
            }
        }
        Ok(())
    }

    /// The operating-system process IDs of the workers, in spawn order: the `<process>` part of
    /// the `message_id` of every reply each one sends (`messaging::ids`).
    fn ids(&self) -> Vec<u32> {
        self.0.iter().map(Child::id).collect()
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

/// What the test reads of one reply: its correlation ID, its response code, taken from the 63
/// written bytes only (`HALTED(H6)`), and the process its `message_id` names as its sender.
struct Reply {
    correlation_id: Option<String>,
    response_code: Option<Vec<u8>>,
    process: Option<u32>,
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
            process: message
                .properties
                .message_id()
                .as_ref()
                .and_then(|id| sender_process(id.as_str())),
        }
    }

    fn is_approved(&self) -> bool {
        self.response_code.as_deref() == Some(APPROVED)
    }
}

/// The process ID that a CP00 reply's `message_id`, `<timestamp>-<process>-<sequence>`, names
/// (`messaging::ids`), or `None` when the ID does not have those three parts or its process
/// part is not a process ID. The ID is split from the right, as only the timestamp can hold a
/// `-`.
fn sender_process(message_id: &str) -> Option<u32> {
    let mut parts = message_id.rsplitn(3, '-');
    let (_sequence, process, _timestamp) = (parts.next()?, parts.next()?, parts.next()?);
    process.parse().ok()
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
#[test]
fn two_processes_and_redelivery_never_commit_past_the_credit_limit() -> TestResult {
    block_on(async {
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
        // TRUNCATION: checked, never lossy. `floor` leaves a whole number, which the
        // precondition above keeps below 50 and `usize` holds; a negative quotient, which
        // `to_usize` refuses, fails the test through `?`, never wraps.
        let k = (headroom / amount)
            .floor()
            .to_usize()
            .ok_or("k = ⌊(L − B) / A⌋ is not a count")?;

        let (_connection, channel) = open_channel(&rabbit).await?;
        let reply_queue = rabbitmq::declare_reply_queue(&channel).await?;
        let correlation_ids =
            publish_requests(&channel, &reply_queue, &xref, &amount.to_string()).await?;
        let first_request = correlation_ids.first().ok_or("no request was published")?;

        // Declared before the workers, so that a failure while it is held kills the processes first
        // and then drops the gate, which rolls it back.
        let gate = close_gate(&store, &xref).await?;
        let mut workers = spawn_workers(pg.database_url(), rabbit.amqp_url())?;
        await_gated_deliveries(&channel, &reply_queue, &mut workers).await?;
        gate.rollback().await?;
        let processes = workers.ids();
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

        assert_participation(&replies, &processes, first_request);
        assert_replies(&replies, &correlation_ids, k);
        assert_committed(&mut store, &account, k, amount).await?;
        assert_details(&settled, k);
        Ok(())
    })
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
/// at once. The queue hands deliveries out in publication order, so the two that the processes
/// hold at the gate are the copies of the first request. Returns the correlation IDs, in
/// publication order; every reply goes to `reply_queue`.
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

/// The gate: the account's summary lock, taken by the test on a transaction of its own through
/// the call and template the CP00 worker uses before it decides (`lock_or_create` with
/// `summary_template`). The account has no summary, so the call creates the row, and the row
/// stays uncommitted: every worker's insert of the same key waits on the gate until it ends,
/// as a concurrent first authorization waits on another's (COPAUA0C-026).
async fn close_gate(store: &PgStore, xref: &CardXrefRecord) -> TestResult<PgTx<'static>> {
    let mut gate = store.begin().await?;
    let (_, created) =
        PendingAuthSummaryRepository::lock_or_create(&mut gate, &copaua0c::summary_template(xref))
            .await?;
    assert!(created, "the gate found a summary instead of creating it");
    Ok(gate)
}

/// Starts [`WORKER_PROCESSES`] `carddemo-mq` processes on the test's database and broker, from
/// the `modern/` directory. Each runs one CP00 worker, which holds one delivery at a time. The
/// request-queue variables are removed so that every process consumes the default queues this
/// test publishes to, and span export is off.
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

/// Waits, within [`READY_WAIT`], until every worker process holds one delivery at the gate,
/// failing as soon as a worker process ends. A delivery is unacknowledged until its decision is
/// replied, each process holds at most one, and every request here is for a found card, whose
/// decision waits on the summary lock. So a ready count of [`DELIVERIES`] less one per process
/// shows every process started, holding a delivery and undecided; a lower count shows a
/// delivery got past the gate. Once the count is reached, the reply queue must be empty and the
/// count must still stand.
async fn await_gated_deliveries(
    channel: &Channel,
    reply_queue: &str,
    workers: &mut ProcessGuard,
) -> TestResult {
    // TRUNCATION: checked, never lossy. DELIVERIES is 100 and the gated count 98, one fewer for
    // each of the two processes, and `u32` holds both; a refused conversion fails the test
    // through `?`, never wraps.
    let published = u32::try_from(DELIVERIES)?;
    let gated = u32::try_from(DELIVERIES - WORKER_PROCESSES)?;
    let deadline = time::Instant::now() + READY_WAIT;
    loop {
        workers.ensure_running()?;
        let ready = rabbitmq::message_count(channel, REQUEST_QUEUE).await?;
        let taken = published.saturating_sub(ready);
        if ready < gated {
            return Err(format!(
                "{taken} deliveries were taken while the gate held the summary lock, more than \
                 one per carddemo-mq process"
            )
            .into());
        }
        if ready == gated {
            break;
        }
        if time::Instant::now() >= deadline {
            return Err(format!(
                "{taken} of {WORKER_PROCESSES} carddemo-mq processes took a delivery to the \
                 gate's summary lock within {} s",
                READY_WAIT.as_secs()
            )
            .into());
        }
        time::sleep(READY_POLL).await;
    }
    let replied = rabbitmq::message_count(channel, reply_queue).await?;
    assert_eq!(
        replied, 0,
        "a reply was sent while the gate held the summary lock"
    );
    let ready = rabbitmq::message_count(channel, REQUEST_QUEUE).await?;
    assert_eq!(ready, gated, "a delivery got past the gate's summary lock");
    Ok(())
}

/// Takes replies from `reply_queue` until one per delivery has arrived, within [`REPLY_WAIT`],
/// failing as soon as a worker process ends. Called once the gate is open; each reply keeps the
/// process that sent it.
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

/// Both processes took part: every reply names one of `processes` as its sender, and the
/// copies of `first_request`, the deliveries held at the gate, were answered one by each
/// process, so both decided at the summary lock after the gate opened.
fn assert_participation(replies: &[Reply], processes: &[u32], first_request: &str) {
    let sent: Vec<String> = processes
        .iter()
        .map(|&process| {
            let count = replies
                .iter()
                .filter(|reply| reply.process == Some(process))
                .count();
            format!("process {process} sent {count}")
        })
        .collect();
    println!(
        "i10 participation: {} of {} replies",
        sent.join(", "),
        replies.len()
    );
    for (index, reply) in replies.iter().enumerate() {
        assert!(
            reply
                .process
                .is_some_and(|process| processes.contains(&process)),
            "reply {index} names {:?} as its sender, not a carddemo-mq process of this test",
            reply.process
        );
    }
    let mut gated: Vec<u32> = replies
        .iter()
        .filter(|reply| reply.correlation_id.as_deref() == Some(first_request))
        .filter_map(|reply| reply.process)
        .collect();
    gated.sort_unstable();
    let mut expected = processes.to_vec();
    expected.sort_unstable();
    assert_eq!(
        gated, expected,
        "the deliveries held at the gate, the copies of {first_request}, were not answered one by \
         each carddemo-mq process"
    );
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
    // TRUNCATION: checked, never lossy. The test's precondition 50 · A > L − B makes k at most
    // 49, so the approved count k and the declined count 100 − k both lie within 0 to 100, which
    // `i16` holds; a refused conversion fails the test through `?`, never wraps.
    let approved_count = i16::try_from(k)?;
    let declined_count = i16::try_from(DELIVERIES - k)?;
    assert_eq!(
        summary.approved_auth_cnt.get(),
        approved_count,
        "approved count"
    );
    assert_eq!(
        summary.declined_auth_cnt.get(),
        declined_count,
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
