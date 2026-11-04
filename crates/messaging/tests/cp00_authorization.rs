//! CP00 (`COPAUA0C`) parity: the section D authorization requests of
//! `docs/parity/parity-oracle-audit.md`, sent through `RabbitMQ` to the running
//! CP00 consumer, their replies, and the pending-authorization rows the
//! consumer writes once it has acknowledged each request.
//!
//! One [`Harness`] runs every case in sequence, its clock pinned to
//! `2022-07-01T12:00:00Z` so the detail key of section F is fixed
//! (`P-COPAUA0C-030`). Each section D row derives its result from the seeded
//! state, where no pending authorization is stored, and every row names the
//! card of the first cross-reference record. So the summary an approval writes
//! is deleted through the `AuthorizationStore` port, its details with it,
//! before the next approval: with the clock pinned, a second detail under the
//! account would repeat its `PAUT9CTS` key. The rows are sent verbatim rather
//! than spread over several accounts, because the audit derived them for this
//! account. The consumer serves every case on one channel, so each reply also
//! proves that every message gets the first-message reply length
//! (`P-COPAUA0C-027`, E-13).
//!
//! Every expected value comes from the audit: each row's request text, reply
//! bytes, reply length, response code, reason and approved amount, the record
//! texts of section B and the formats of section F. The account values a
//! summary copies come from the fixture record the rows cite, cross-checked
//! against each row's available amount.
//!
//! Halt cases (AAP 0.9.6) are asserted only as their disposition, never as a
//! result:
//!
//! - A card the cross-reference lacks (`P-COPAUA0C-026`): its decision on the
//!   first message tests a flag no statement set (`P-COPAUA0C-028`, E-41), so
//!   the row's `05`/`3100` reply is a proposal. The test asserts what is
//!   pinned, the `A001` record and that nothing is written, and that the
//!   consumer sends no reply and rejects the request.
//! - A declined found card (`P-COPAUA0C-023`): its reply is pinned and
//!   asserted, but its summary update adds a detail field before `8500` sets
//!   it (`P-COPAUA0C-028`, E-41), so the test asserts that nothing is written.
//! - The reply's byte 64 and the summary's `PA-AUTH-STATUS` and
//!   `PA-ACCOUNT-STATUS`, which no statement sets (E-25), are excluded from
//!   comparison.
//!
//! A second test, on a harness of its own, sends the `P-COPAUA0C-021` request
//! while a purge unit of work holds the account's summary: it has swept it
//! with its locking `GN` and deleted it, uncommitted. IMS program isolation
//! orders `COPAUA0C` after that purge, so CP00 waits on the summary's row
//! lock, as `pg_stat_activity` shows, and once the purge commits it answers
//! and writes what the seeded state gives after the purge: the row's audited
//! approval, a new summary of one approval and exactly the new detail
//! (`P-COPAUA0C-008`, `P-COPAUA0C-013`, `P-CBPAUP0C-006`).
//!
//! Not proved here: the per-task queue open, get loop, task end, PSB
//! schedule, terminate and close (`P-COPAUA0C-001`, `P-COPAUA0C-003`,
//! `P-COPAUA0C-032`, `P-COPAUA0C-033`, `P-COPAUA0C-036`, `P-COPAUA0C-037`),
//! which the long-running consumer does not reproduce (E-22); the available
//! amount of an existing summary (`P-COPAUA0C-017`), for which section D has
//! no row; and the failed reply put of `P-COPAUA0C-012` (`M004`), which
//! `ack_after_confirm.rs` proves.

pub mod common;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use carddemo_domain::entities::pending_auth_detail::{
    FraudFlag, MatchStatus, NewPendingAuthDetail, PendingAuthDetail,
};
use carddemo_domain::entities::pending_auth_summary::{
    PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary,
};
use carddemo_domain::numeric::ids::{AccountId, CustomerId};
use carddemo_domain::numeric::{Alnum, AuthCount, CardNumber, Money, NumericCode};
use carddemo_domain::ports::{AuthorizationStore, Browse, Direction, PortError};
use carddemo_persistence::UnitOfWork;
use carddemo_testkit::block_on;
use common::{Harness, audit_row, audit_texts};
use lapin::message::Delivery;
use lapin::types::ShortString;

/// The `CARDDEMO_CLOCK_OVERRIDE` of the run, the instant section F pins the
/// detail key at (`P-COPAUA0C-030`).
const PINNED: &str = "2022-07-01T12:00:00Z";

/// `PA-AUTH-DATE-9C` at [`PINNED`]: `99999 - 22182`, day 182 of 2022
/// (`P-COPAUA0C-030`).
const DATE_9C: i32 = 77_817;

/// `PA-AUTH-TIME-9C` at [`PINNED`]: `999999999 - 120000000`, 12:00:00.000
/// (`P-COPAUA0C-030`).
const TIME_9C: i32 = 879_999_999;

/// `ERR-DATE` (`YYMMDD`) and `ERR-TIME` (`HHMMSS`) at [`PINNED`]
/// (`P-COPAUA0C-031`).
const ERR_STAMPS: [(&str, &str); 2] = [("err_date", "220701"), ("err_time", "120000")];

/// The section F formats this file checks, as the audit writes them: the
/// reply `message_id` (`P-COPAUA0C-029`), the detail key parts
/// (`P-COPAUA0C-030`) and the error-record stamps (`P-COPAUA0C-031`).
const MESSAGE_ID_FORMAT: &str = "^[0-9a-f]{48}$";
const DETAIL_KEY_FORMAT: &str = "^[0-9]{5}$ and ^[0-9]{9}$";
const ERR_STAMP_FORMAT: &str = "^[0-9]{6}$";

/// The characters of a reply `message_id`: the `{48}` of
/// [`MESSAGE_ID_FORMAT`].
const MESSAGE_ID_LEN: usize = 48;

/// The highest row number of `COPAUA0C` in the audit, every one of which this
/// file cites.
const LAST_ROW: u16 = 37;

/// The receivers of the request `UNSTRING`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L354-374].
const REQUEST_FIELDS: usize = 18;

/// `XREF-ACCT-ID` and `XREF-CUST-ID` of the first cross-reference record,
/// whose card every section D row of a found card names
/// [app/data/ASCII/cardxref.txt:L1].
const FIXTURE_ACCOUNT: u64 = 50;
const FIXTURE_CUSTOMER: u32 = 50;

/// `ACCT-CREDIT-LIMIT`, `ACCT-CASH-CREDIT-LIMIT` and `ACCT-CURR-BAL` of that
/// account (`P-CVACT01Y-004`, `P-CVACT01Y-005`, `P-CVACT01Y-003`)
/// [app/data/ASCII/acctdata.txt:L50].
const CREDIT_LIMIT: &str = "6169.00";
const CASH_LIMIT: &str = "4587.00";
const CURR_BAL: &str = "492.00";

/// AMQP `delivery-mode` 2, persistent: the mandated replacement of the
/// COBOL's non-persistent reply (`P-COPAUA0C-012`).
const PERSISTENT: u8 = 2;

/// The message-span `location` of a request whose reply and writes completed.
const NO_LOCATION: &str = "none";

/// The span `location` and log `paragraph` of a request whose decision is
/// unproven: no reply is sent for it (`P-COPAUA0C-028`).
const DECISION_PARAGRAPH: &str = "6000-MAKE-DECISION";

/// The span `location` and log `paragraph` of a reply whose writes are
/// unproven: nothing is written for it (`P-COPAUA0C-028`).
const WRITE_PARAGRAPH: &str = "8000-WRITE-AUTH-TO-DB";

/// How long a reply, a log line or a written row may take to appear.
const REPLY_WAIT: Duration = Duration::from_secs(30);
const LOG_WAIT: Duration = Duration::from_secs(30);
const DATA_WAIT: Duration = Duration::from_secs(30);

/// How long a reply queue is watched for a second reply, which also leaves
/// time for any late write to show.
const SETTLE: Duration = Duration::from_secs(3);

/// Interval between two reads of the pending-authorization rows.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The most summaries or details one port read returns here: far more than
/// the run writes.
const ROW_LIMIT: u16 = 100;

/// The sessions of the test database whose locking summary read waits on a
/// lock: CP00's `5500-READ-AUTH-SUMMRY`, `SELECT … FOR UPDATE` of
/// `pending_auth_summaries`, behind the row lock of a purge sweep. The
/// session asking is left out, since its own statement names the same words.
const SUMMARY_LOCK_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() AND pid <> pg_backend_pid() \
       AND wait_event_type = 'Lock' \
       AND query LIKE '%FROM pending_auth_summaries%FOR UPDATE%'";

/// How long CP00 may take to reach its wait on the sweep's lock: less than
/// the 30-second database deadline of its read, so the read still waits when
/// the sweep commits.
const LOCK_WAIT: Duration = Duration::from_secs(20);

/// `PA-AUTH-DATE-9C` of the approval the purged summary holds: 30 days
/// before [`PINNED`], a key the new detail does not repeat.
const PURGED_DATE_9C: i32 = DATE_9C + 30;

/// The placeholder of bytes the COBOL never sets, `{UNDEF:n}`.
const UNDEF_OPEN: &str = "{UNDEF:";

/// One section D row of `COPAUA0C`, sent with the request the audit records.
struct Case {
    /// The row ID, for example `P-COPAUA0C-021`.
    id: &'static str,
    /// The `CCPAURQY` request text, the row's `request=` exact text.
    request: String,
    /// The request's `message_id` (`MQMD-MSGID`).
    message_id: String,
    /// The request's `correlation_id` (`MQMD-CORRELID`).
    correlation_id: String,
}

impl Case {
    /// The case of audit row `id`.
    fn new(id: &'static str) -> Self {
        Self {
            id,
            request: row_exact_text(id, "request"),
            message_id: format!("{id}-request"),
            correlation_id: format!("{id}-correlation"),
        }
    }

    /// The request's comma-delimited fields, one per `UNSTRING` receiver in
    /// `CCPAURQY` order (`P-COPAUA0C-004`).
    fn fields(&self) -> Vec<&str> {
        let fields: Vec<&str> = self.request.split(',').collect();
        assert_eq!(
            fields.len(),
            REQUEST_FIELDS,
            "{}: the request does not fill the {REQUEST_FIELDS} UNSTRING receivers",
            self.id
        );
        fields
    }

    /// `PA-RQ-CARD-NUM`, the third field.
    fn card(&self) -> &str {
        self.fields()[2]
    }
}

/// The double-quoted string of the exact text that follows `key=` in audit
/// row `id`.
fn row_exact_text(id: &str, key: &str) -> String {
    let line = audit_row(id);
    let marker = format!("{key}=`\"");
    let start = line
        .find(&marker)
        .unwrap_or_else(|| panic!("audit row {id} has no {key}= exact text"))
        + marker.len();
    let length = line[start..]
        .find("\"`")
        .unwrap_or_else(|| panic!("audit row {id}: the {key}= exact text is unterminated"));
    let text = &line[start..start + length];
    assert!(
        !text.contains('\\'),
        "audit row {id}: the {key}= exact text holds an escape, which no request needs"
    );
    text.to_owned()
}

/// The bare token of ` key=` in audit row `id`: the characters after `=` up
/// to the next `;`, space or `|`.
fn row_token(id: &str, key: &str) -> String {
    let line = audit_row(id);
    let marker = format!(" {key}=");
    let start = line
        .find(&marker)
        .unwrap_or_else(|| panic!("audit row {id} has no {key}= value"))
        + marker.len();
    line[start..]
        .split(|ch: char| ch == ';' || ch == '|' || ch.is_whitespace())
        .next()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| panic!("audit row {id}: {key}= has no value"))
        .to_owned()
}

/// The cents of `amount`, a decimal string with two fraction digits as the
/// audit and `Money`'s `Display` write amounts (`100.00`, `-50.00`).
fn cents_of(amount: &str) -> i64 {
    let (whole, fraction) = amount
        .split_once('.')
        .unwrap_or_else(|| panic!("{amount:?} has no decimal point"));
    assert!(
        fraction.len() == 2 && fraction.bytes().all(|byte| byte.is_ascii_digit()),
        "{amount:?} does not have two fraction digits"
    );
    format!("{whole}{fraction}")
        .parse()
        .unwrap_or_else(|error| panic!("{amount:?} is not an amount: {error}"))
}

/// The bytes of the COBOL alphanumeric `MOVE` of `text` into an `X(width)`
/// item: left-justified, padded with spaces, cut on the right.
fn moved(text: &str, width: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = text.bytes().take(width).collect();
    bytes.resize(width, b' ');
    bytes
}

/// The value of an unsigned `9(n)` request field.
fn digits(text: &str) -> u32 {
    assert!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "{text:?} is not a 9(n) value"
    );
    text.parse()
        .unwrap_or_else(|error| panic!("{text:?} is not a 9(n) value: {error}"))
}

/// The string value of the JSON member `key` in the log `line`.
fn json_string<'l>(line: &'l str, key: &str) -> &'l str {
    let marker = format!("\"{key}\":\"");
    let start = line
        .find(&marker)
        .unwrap_or_else(|| panic!("no string member {key} in {line}"))
        + marker.len();
    let length = line[start..]
        .find('"')
        .unwrap_or_else(|| panic!("member {key} is unterminated in {line}"));
    &line[start..start + length]
}

/// The account of the fixture card.
fn fixture_account() -> AccountId {
    AccountId::new(FIXTURE_ACCOUNT).expect("the fixture account fits 9(11)")
}

/// The customer of the fixture card.
fn fixture_customer() -> CustomerId {
    CustomerId::new(FIXTURE_CUSTOMER).expect("the fixture customer fits 9(09)")
}

/// Checks the audit rows this file reads before any case runs: every
/// `COPAUA0C` row it cites exists, and the section F formats are the ones
/// the checks below implement. Returns the escalation `P-COPAUA0C-028` holds
/// at `halt`, which the consumer logs for each unproven request.
fn audited_rows() -> String {
    for number in 1..=LAST_ROW {
        let id = format!("P-COPAUA0C-{number:03}");
        assert!(audit_row(&id).contains(&id), "audit row {id} not found");
    }
    assert_eq!(audit_texts("P-COPAUA0C-029"), [MESSAGE_ID_FORMAT]);
    assert_eq!(audit_texts("P-COPAUA0C-030"), [DETAIL_KEY_FORMAT]);
    let pin = format!("pinned 2022-07-01T12:00:00.000Z gives {DATE_9C} and {TIME_9C}");
    assert!(
        audit_row("P-COPAUA0C-030").contains(&pin),
        "P-COPAUA0C-030 does not record {pin:?}"
    );
    assert_eq!(audit_texts("P-COPAUA0C-031"), [ERR_STAMP_FORMAT]);
    row_token("P-COPAUA0C-028", "escalation")
}

/// `P-COPAUA0C-011`: compares `actual` with the exact text `expected` byte
/// for byte, skipping the n bytes each `{UNDEF:n}` stands for, which the
/// COBOL never sets and E-25 excludes from comparison until the space fill is
/// confirmed. Every other byte, and the length, must match.
fn assert_exact_bytes(actual: &[u8], expected: &str, id: &str) {
    let mut at = 0;
    let mut rest = expected;
    while !rest.is_empty() {
        let (literal, skipped, tail) = match rest.find(UNDEF_OPEN) {
            Some(start) => {
                let (count, tail) = rest[start + UNDEF_OPEN.len()..]
                    .split_once('}')
                    .unwrap_or_else(|| panic!("audit row {id}: unterminated {{UNDEF:n}}"));
                let skipped: usize = count
                    .parse()
                    .unwrap_or_else(|error| panic!("audit row {id}: {{UNDEF:{count}}}: {error}"));
                (&rest[..start], skipped, tail)
            }
            None => (rest, 0, ""),
        };
        let end = at + literal.len();
        let found = actual.get(at..end);
        assert!(
            found == Some(literal.as_bytes()),
            "audit row {id}: reply bytes {at}..{end} are {:?}, the audit has {literal:?}; \
             reply {:?}",
            found.map(String::from_utf8_lossy),
            String::from_utf8_lossy(actual)
        );
        at = end + skipped;
        rest = tail;
    }
    assert_eq!(
        actual.len(),
        at,
        "audit row {id}: the reply {:?} is not the audited length",
        String::from_utf8_lossy(actual)
    );
}

/// `P-COPAUA0C-002`, `P-COPAUA0C-012`: puts the request of `case` on the CP00
/// request queue through the default exchange, with its `message_id`,
/// `correlation_id` and, as `reply_to`, a reply queue of its own, which is
/// returned.
async fn send(h: &Harness, case: &Case) -> String {
    let reply_queue = h.temp_reply_queue().await;
    h.publish(
        &h.settings.queue_pauth_request,
        case.request.as_bytes(),
        &case.message_id,
        &case.correlation_id,
        Some(&reply_queue),
    )
    .await;
    reply_queue
}

/// `P-COPAUA0C-011`, `P-COPAUA0C-012`, `P-COPAUA0C-020`, `P-COPAUA0C-027`,
/// `P-COPAUA0C-029`: the reply of `case` is the row's exact text (byte 64
/// excepted, E-25) at the row's `reply_length`; it echoes the request's
/// `correlation_id` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L745];
/// its `message_id` is new [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L746],
/// in the section F format and seen on no earlier reply; and it is persistent
/// without expiry, the mandated replacement of the COBOL's non-persistent
/// 5-second reply [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L749-750].
fn assert_reply(reply: &Delivery, case: &Case, message_ids: &mut HashSet<String>) {
    let id = case.id;
    let texts = audit_texts(id);
    assert_eq!(texts.len(), 1, "audit row {id} records one reply text");
    assert_exact_bytes(&reply.data, &texts[0], id);
    let length: usize = row_token(id, "reply_length")
        .parse()
        .unwrap_or_else(|error| panic!("audit row {id}: reply_length: {error}"));
    assert_eq!(reply.data.len(), length, "{id}: reply length");

    let properties = &reply.properties;
    assert_eq!(
        properties
            .correlation_id()
            .as_ref()
            .map(ShortString::as_str),
        Some(case.correlation_id.as_str()),
        "{id}: the reply correlation_id"
    );
    let message_id = properties.message_id().as_ref().map_or_else(
        || panic!("{id}: the reply has no message_id"),
        ShortString::as_str,
    );
    assert!(
        message_id.len() == MESSAGE_ID_LEN
            && message_id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "{id}: the reply message_id {message_id:?} does not match {MESSAGE_ID_FORMAT}"
    );
    assert_ne!(
        message_id, case.message_id,
        "{id}: the request message_id was echoed"
    );
    assert!(
        message_ids.insert(message_id.to_owned()),
        "{id}: the reply message_id {message_id} repeats an earlier reply's"
    );
    assert_eq!(
        *properties.delivery_mode(),
        Some(PERSISTENT),
        "{id}: delivery mode"
    );
    assert!(
        properties.expiration().is_none(),
        "{id}: the reply expires: {:?}",
        properties.expiration()
    );
}

/// The message span line of `case`. The span closes once the consumer is done
/// with the request, after its post-acknowledgement writes, so the line also
/// marks the moment the rows are committed.
async fn message_span(h: &Harness, case: &Case) -> String {
    let message_id = format!("\"message_id\":\"{}\"", case.message_id);
    h.wait_for_log(
        &["\"type\":\"span\"", "\"flow\":\"CP00\"", &message_id],
        LOG_WAIT,
    )
    .await
}

/// The message span of `case` records `outcome`, no error record and
/// `location`, under the request's `correlation_id`.
fn assert_span(span: &str, case: &Case, outcome: &str, location: &str) {
    let attributes = [
        format!("\"outcome\":\"{outcome}\""),
        "\"error_record\":\"none\"".to_owned(),
        format!("\"location\":\"{location}\""),
        format!("\"correlation_id\":\"{}\"", case.correlation_id),
    ];
    for attribute in &attributes {
        assert!(
            span.contains(attribute.as_str()),
            "{}: the message span lacks {attribute}: {span}",
            case.id
        );
    }
}

/// `P-COPAUA0C-002`: the get removed the request of `case` from the request
/// queue, replied or not; nothing is requeued.
async fn assert_request_consumed(h: &Harness, case: &Case) {
    let queue = &h.settings.queue_pauth_request;
    let depth = h.queue_depth(queue).await;
    assert_eq!(
        depth, 0,
        "{}: {queue} still holds {depth} request(s)",
        case.id
    );
}

/// Every `ERROR-LOG-RECORD` line the consumer logged while it handled `case`.
fn records_of(h: &Harness, case: &Case) -> Vec<String> {
    h.logs()
        .into_iter()
        .filter(|line| line.contains("ERROR-LOG-RECORD") && line.contains(&case.message_id))
        .collect()
}

/// Every line logging the input class of `case` as unproven.
fn halts_of(h: &Harness, case: &Case) -> Vec<String> {
    h.logs()
        .into_iter()
        .filter(|line| line.contains("\"escalation\":") && line.contains(&case.message_id))
        .collect()
}

/// `P-COPAUA0C-028`: the consumer logged the input class of `case` once as
/// unproven under `escalation`, naming `paragraph`, the output it skipped.
fn assert_halted(h: &Harness, case: &Case, escalation: &str, paragraph: &str) {
    let halts = halts_of(h, case);
    assert_eq!(
        halts.len(),
        1,
        "{}: unproven-input lines {halts:?}",
        case.id
    );
    for member in [
        format!("\"escalation\":\"{escalation}\""),
        format!("\"paragraph\":\"{paragraph}\""),
    ] {
        assert!(
            halts[0].contains(&member),
            "{}: the unproven-input line lacks {member}: {}",
            case.id,
            halts[0]
        );
    }
}

/// Sleeps for `period` on a blocking thread: the crate enables no tokio
/// timer.
async fn pause(period: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(period))
        .await
        .expect("sleep task");
}

/// A browse of every twin of a summary, newest first.
fn every_twin<K>() -> Browse<K> {
    Browse {
        start_key: None,
        direction: Direction::Forward,
        limit: ROW_LIMIT,
    }
}

/// The pending-authorization rows of `account`, read through the port in a
/// unit of work that is rolled back: its summary, if one is stored, and its
/// details in twin order.
async fn rows_of(
    h: &Harness,
    account: AccountId,
) -> (Option<PendingAuthSummary>, Vec<PendingAuthDetail>) {
    let mut uow = h.db.begin().await.expect("open a read unit of work");
    let summary = match uow.store().read_summary(account).await {
        Ok(summary) => Some(summary),
        Err(PortError::NotFound) => None,
        Err(error) => panic!("cannot read the summary of account {account}: {error}"),
    };
    let details = uow
        .store()
        .details_for_account(account, every_twin())
        .await
        .unwrap_or_else(|error| panic!("cannot read the details of account {account}: {error}"));
    uow.rollback()
        .await
        .expect("roll the read unit of work back");
    (summary, details)
}

/// The rows of `account` once `ready` holds for them, read again every
/// `POLL_INTERVAL` for up to `DATA_WAIT`: the writes follow the request's
/// acknowledgement. The last read is returned when time runs out, so the
/// caller's assertions report what was found.
async fn settled(
    h: &Harness,
    account: AccountId,
    ready: impl Fn(Option<&PendingAuthSummary>, &[PendingAuthDetail]) -> bool,
) -> (Option<PendingAuthSummary>, Vec<PendingAuthDetail>) {
    let until = Instant::now() + DATA_WAIT;
    loop {
        let (summary, details) = rows_of(h, account).await;
        if ready(summary.as_ref(), &details) || Instant::now() >= until {
            return (summary, details);
        }
        pause(POLL_INTERVAL).await;
    }
}

/// The whole pending-authorization database through the port: every summary
/// in `ACCNTID` order (the `GN` sweep of `summaries_after`), each with its
/// details in twin order. A detail exists only under its summary.
async fn all_rows(h: &Harness) -> Vec<(PendingAuthSummary, Vec<PendingAuthDetail>)> {
    let mut uow = h.db.begin().await.expect("open a read unit of work");
    let summaries = uow
        .store()
        .summaries_after(None, ROW_LIMIT)
        .await
        .unwrap_or_else(|error| panic!("cannot browse the summaries: {error}"));
    assert!(
        summaries.len() < usize::from(ROW_LIMIT),
        "more summaries than one browse returns"
    );
    let mut rows = Vec::with_capacity(summaries.len());
    for summary in summaries {
        let account = summary.pa_acct_id;
        let details = uow
            .store()
            .details_for_account(account, every_twin())
            .await
            .unwrap_or_else(|error| {
                panic!("cannot read the details of account {account}: {error}")
            });
        rows.push((summary, details));
    }
    uow.rollback()
        .await
        .expect("roll the read unit of work back");
    rows
}

/// Restores the seeded pending-authorization state the section D rows start
/// from: deletes the fixture account's summary through the port, which
/// deletes its details with it, and commits.
async fn reset(h: &Harness) {
    let account = fixture_account();
    let mut uow = h.db.begin().await.expect("open the reset unit of work");
    uow.store()
        .delete_summary(account)
        .await
        .unwrap_or_else(|error| panic!("cannot delete the summary of account {account}: {error}"));
    uow.commit().await.expect("commit the reset");
    assert!(
        all_rows(h).await.is_empty(),
        "pending authorizations remain after the reset"
    );
}

/// `P-COPAUA0C-009`, `P-COPAUA0C-018`, `P-COPAUA0C-034`: no summary is
/// stored, so the decision of row `id` compares the amount with
/// `ACCT-CREDIT-LIMIT - ACCT-CURR-BAL` of the fixture account, no profile
/// data taking part; that difference is the row's `available=`.
fn assert_available(id: &str) {
    assert_eq!(
        cents_of(CREDIT_LIMIT) - cents_of(CURR_BAL),
        cents_of(&row_token(id, "available")),
        "{id}: the fixture account does not give the audited available amount"
    );
}

/// `P-COPAUA0C-008`, `P-COPAUA0C-013`, `P-COPAUA0C-019`: the summary `8400`
/// inserts when `5500` found none: account and customer of the
/// cross-reference, the account's limits moved into `S9(09)V99`, one approval
/// of the row's approved amount added to the approved total and the credit
/// balance, the cash balance zero, and the declined count and amount left at
/// the zeros `INITIALIZE` gave them
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L801-816].
/// `PA-AUTH-STATUS` and `PA-ACCOUNT-STATUS` are never set (E-25) and are not
/// compared.
fn assert_approved_summary(summary: &PendingAuthSummary, case: &Case) {
    let id = case.id;
    let approved = row_token(id, "approved_amt");
    assert_eq!(summary.pa_acct_id, fixture_account(), "{id}: PA-ACCT-ID");
    assert_eq!(summary.pa_cust_id, fixture_customer(), "{id}: PA-CUST-ID");
    let amounts: [(&str, String, &str); 6] = [
        (
            "PA-CREDIT-LIMIT",
            summary.pa_credit_limit.to_string(),
            CREDIT_LIMIT,
        ),
        (
            "PA-CASH-LIMIT",
            summary.pa_cash_limit.to_string(),
            CASH_LIMIT,
        ),
        (
            "PA-CREDIT-BALANCE",
            summary.pa_credit_balance.to_string(),
            &approved,
        ),
        (
            "PA-CASH-BALANCE",
            summary.pa_cash_balance.to_string(),
            "0.00",
        ),
        (
            "PA-APPROVED-AUTH-AMT",
            summary.pa_approved_auth_amt.to_string(),
            &approved,
        ),
        (
            "PA-DECLINED-AUTH-AMT",
            summary.pa_declined_auth_amt.to_string(),
            "0.00",
        ),
    ];
    for (field, actual, expected) in amounts {
        assert_eq!(actual, expected, "{id}: {field}");
    }
    assert_eq!(
        summary.pa_approved_auth_cnt.get(),
        1,
        "{id}: PA-APPROVED-AUTH-CNT"
    );
    assert_eq!(
        summary.pa_declined_auth_cnt.get(),
        0,
        "{id}: PA-DECLINED-AUTH-CNT"
    );
}

/// The `CIPAUDTY` receivers `8500` moves request fields into
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L877-899], with the
/// index of the request field moved and the receiver's `X(n)` width
/// (`P-CIPAUDTY-003` to `P-CIPAUDTY-024`). `PA-AUTH-ID-CODE` is the
/// request's auth time, through `PA-RL-AUTH-ID-CODE`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L662].
const REQUEST_MOVES: [(&str, usize, usize); 16] = [
    ("PA-AUTH-ORIG-DATE", 0, 6),
    ("PA-AUTH-ORIG-TIME", 1, 6),
    ("PA-CARD-NUM", 2, 16),
    ("PA-AUTH-TYPE", 3, 4),
    ("PA-CARD-EXPIRY-DATE", 4, 4),
    ("PA-MESSAGE-TYPE", 5, 6),
    ("PA-MESSAGE-SOURCE", 6, 6),
    ("PA-MERCHANT-CATAGORY-CODE", 9, 4),
    ("PA-ACQR-COUNTRY-CODE", 10, 3),
    ("PA-MERCHANT-ID", 12, 15),
    ("PA-MERCHANT-NAME", 13, 22),
    ("PA-MERCHANT-CITY", 14, 13),
    ("PA-MERCHANT-STATE", 15, 2),
    ("PA-MERCHANT-ZIP", 16, 9),
    ("PA-TRANSACTION-ID", 17, 15),
    ("PA-AUTH-ID-CODE", 1, 6),
];

/// The stored bytes of the `X(n)` detail field `field`, by its copybook name.
fn detail_text<'d>(detail: &'d PendingAuthDetail, field: &str) -> &'d [u8] {
    match field {
        "PA-AUTH-ORIG-DATE" => detail.pa_auth_orig_date.as_bytes(),
        "PA-AUTH-ORIG-TIME" => detail.pa_auth_orig_time.as_bytes(),
        "PA-CARD-NUM" => detail.pa_card_num.as_bytes(),
        "PA-AUTH-TYPE" => detail.pa_auth_type.as_bytes(),
        "PA-CARD-EXPIRY-DATE" => detail.pa_card_expiry_date.as_bytes(),
        "PA-MESSAGE-TYPE" => detail.pa_message_type.as_bytes(),
        "PA-MESSAGE-SOURCE" => detail.pa_message_source.as_bytes(),
        "PA-AUTH-ID-CODE" => detail.pa_auth_id_code.as_bytes(),
        "PA-AUTH-RESP-CODE" => detail.pa_auth_resp_code.as_bytes(),
        "PA-AUTH-RESP-REASON" => detail.pa_auth_resp_reason.as_bytes(),
        "PA-MERCHANT-CATAGORY-CODE" => detail.pa_merchant_catagory_code.as_bytes(),
        "PA-ACQR-COUNTRY-CODE" => detail.pa_acqr_country_code.as_bytes(),
        "PA-MERCHANT-ID" => detail.pa_merchant_id.as_bytes(),
        "PA-MERCHANT-NAME" => detail.pa_merchant_name.as_bytes(),
        "PA-MERCHANT-CITY" => detail.pa_merchant_city.as_bytes(),
        "PA-MERCHANT-STATE" => detail.pa_merchant_state.as_bytes(),
        "PA-MERCHANT-ZIP" => detail.pa_merchant_zip.as_bytes(),
        "PA-TRANSACTION-ID" => detail.pa_transaction_id.as_bytes(),
        "PA-FRAUD-RPT-DATE" => detail.pa_fraud_rpt_date.as_bytes(),
        _ => panic!("{field} is no X(n) field of PENDING-AUTH-DETAILS"),
    }
}

/// The detail field `field` of row `id` holds the `MOVE` of `text` into its
/// `X(width)`.
fn assert_moved(id: &str, detail: &PendingAuthDetail, field: &str, text: &str, width: usize) {
    let actual = detail_text(detail, field);
    let expected = moved(text, width);
    assert!(
        actual == expected.as_slice(),
        "{id}: {field} holds {:?}, the MOVE of {text:?} gives {:?}",
        String::from_utf8_lossy(actual),
        String::from_utf8_lossy(&expected)
    );
}

/// `P-COPAUA0C-004`, `P-COPAUA0C-014`, `P-COPAUA0C-016`, `P-COPAUA0C-030`: the
/// detail `8500` inserts for the approved request of `case`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L857-911]. Its key is
/// complemented from the pinned clock; the request fields are moved into
/// their receivers ([`REQUEST_MOVES`]) and the reply's code and reason into
/// theirs; an approval moves the request amount, after `NUMVAL`, into both
/// amounts, which the row's `approved_amt` records; the match status is
/// pending and both fraud fields are spaces.
fn assert_detail(detail: &PendingAuthDetail, case: &Case) {
    let id = case.id;
    let fields = case.fields();
    assert_eq!(detail.pa_acct_id, fixture_account(), "{id}: parent ACCNTID");
    assert_eq!(
        (detail.pa_auth_date_9c, detail.pa_auth_time_9c),
        (DATE_9C, TIME_9C),
        "{id}: PA-AUTH-DATE-9C and PA-AUTH-TIME-9C"
    );
    for (field, index, width) in REQUEST_MOVES {
        assert_moved(id, detail, field, fields[index], width);
    }
    let resp_code = row_token(id, "auth_resp_code");
    assert_moved(id, detail, "PA-AUTH-RESP-CODE", &resp_code, 2);
    let reason = row_token(id, "auth_resp_reason");
    assert_moved(id, detail, "PA-AUTH-RESP-REASON", &reason, 4);
    assert_moved(id, detail, "PA-FRAUD-RPT-DATE", "", 8);
    let numbers = [
        ("PA-PROCESSING-CODE", detail.pa_processing_code.get(), 7),
        ("PA-POS-ENTRY-MODE", detail.pa_pos_entry_mode.get(), 11),
    ];
    for (field, actual, index) in numbers {
        assert_eq!(actual, digits(fields[index]), "{id}: {field}");
    }
    let approved = row_token(id, "approved_amt");
    let amounts = [
        ("PA-TRANSACTION-AMT", detail.pa_transaction_amt.to_string()),
        ("PA-APPROVED-AMT", detail.pa_approved_amt.to_string()),
    ];
    for (field, actual) in amounts {
        assert_eq!(actual, approved, "{id}: {field}");
    }
    let status = detail.pa_match_status;
    assert_eq!(status, MatchStatus::Pending, "{id}: PA-MATCH-STATUS");
    let fraud = detail.pa_auth_fraud;
    assert_eq!(fraud, FraudFlag::Blank, "{id}: PA-AUTH-FRAUD");
}

/// `P-COPAUA0C-006`, `P-COPAUA0C-015`, `P-COPAUA0C-031`: the one record of the
/// unknown card of `case` is the `A001` warning of the application subsystem
/// with the audited text, logged with the `CCPAUERY` fields
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L490-500] and stamped
/// from the pinned clock in the section F format. Its event key is the card,
/// which no log line shows in full.
async fn assert_a001(h: &Harness, case: &Case) {
    let id = case.id;
    let message = audit_texts("P-COPAUA0C-006")
        .into_iter()
        .next()
        .expect("P-COPAUA0C-006 records the A001 text first");
    let line = h
        .wait_for_log(&["\"err_location\":\"A001\"", &case.message_id], LOG_WAIT)
        .await;
    let members = [
        format!("\"err_message\":\"{message}\""),
        "\"err_level\":\"W\"".to_owned(),
        "\"err_subsystem\":\"A\"".to_owned(),
        "\"err_application\":\"CP00\"".to_owned(),
        "\"err_program\":\"COPAUA0C\"".to_owned(),
    ];
    for member in &members {
        assert!(
            line.contains(member.as_str()),
            "{id}: the A001 record lacks {member}: {line}"
        );
    }
    for (key, pinned) in ERR_STAMPS {
        let stamp = json_string(&line, key);
        assert!(
            stamp.len() == pinned.len() && stamp.bytes().all(|byte| byte.is_ascii_digit()),
            "{id}: {key} {stamp:?} does not match {ERR_STAMP_FORMAT}"
        );
        assert_eq!(stamp, pinned, "{id}: {key} at the pinned clock");
    }
    let card = case.card();
    let last_four = &card[card.len() - 4..];
    assert!(
        json_string(&line, "err_event_key").ends_with(last_four) && !line.contains(card),
        "{id}: the A001 event key is not the masked card: {line}"
    );
    let records = records_of(h, case);
    assert_eq!(records.len(), 1, "{id}: error records {records:?}");
}

/// `P-COPAUA0C-021`, `P-COPAUA0C-022`, `P-COPAUA0C-024`, `P-COPAUA0C-025`: an
/// approval of the fixture card from the seeded state, code `00`, reason
/// `0000` and the request amount approved (`P-COPAUA0C-010`). Row 022
/// approves exactly the available amount (`P-COPAUA0C-018`), row 024 a
/// negative amount (`P-COPAUA0C-020`), row 025 a third decimal rounded to the
/// cent (`P-COPAUA0C-016`).
///
/// The reply carries the row's text; once the request is acknowledged the
/// summary and the detail are written (`P-COPAUA0C-005`, `P-COPAUA0C-035`).
/// No record is logged: the card, its account and its customer are found
/// (`P-COPAUA0C-006`, `P-COPAUA0C-007`) and the summary read finds none,
/// which is no error (`P-COPAUA0C-008`).
async fn approved(h: &Harness, id: &'static str, message_ids: &mut HashSet<String>) {
    let case = Case::new(id);
    assert_eq!(
        row_token(id, "auth_resp_code"),
        "00",
        "audit row {id} approves"
    );
    assert_available(id);
    let reply_queue = send(h, &case).await;
    let reply = h.next_message(&reply_queue, REPLY_WAIT).await;
    assert_reply(&reply, &case, message_ids);
    let span = message_span(h, &case).await;
    assert_span(&span, &case, "replied", NO_LOCATION);
    assert_request_consumed(h, &case).await;

    let (summary, details) = settled(h, fixture_account(), |summary, details| {
        summary.is_some() && details.len() == 1
    })
    .await;
    let summary = summary.unwrap_or_else(|| panic!("{id}: no summary within {DATA_WAIT:?}"));
    assert_approved_summary(&summary, &case);
    assert_eq!(details.len(), 1, "{id}: details under the summary");
    assert_detail(&details[0], &case);
    let records = records_of(h, &case);
    assert!(records.is_empty(), "{id}: error records {records:?}");
    let halts = halts_of(h, &case);
    assert!(halts.is_empty(), "{id}: unproven-input lines {halts:?}");
    h.assert_no_message(&reply_queue, SETTLE).await;
}

/// `P-COPAUA0C-023`: the fixture card declined from the seeded state for one
/// cent above the available amount, the strict test of `P-COPAUA0C-018`:
/// code `05`, reason `4100`, nothing approved (`P-COPAUA0C-010`).
///
/// The reply is pinned and asserted. The declined-amount summary update is
/// not (`P-COPAUA0C-028`, E-41 at `halt`), so the consumer logs the
/// escalation at `8000-WRITE-AUTH-TO-DB` and writes nothing, rather than a
/// guessed summary and detail.
async fn declined(
    h: &Harness,
    id: &'static str,
    escalation: &str,
    message_ids: &mut HashSet<String>,
) {
    let case = Case::new(id);
    assert_eq!(
        row_token(id, "auth_resp_code"),
        "05",
        "audit row {id} declines"
    );
    assert_available(id);
    let reply_queue = send(h, &case).await;
    let reply = h.next_message(&reply_queue, REPLY_WAIT).await;
    assert_reply(&reply, &case, message_ids);
    let span = message_span(h, &case).await;
    assert_span(&span, &case, "replied", WRITE_PARAGRAPH);
    assert_halted(h, &case, escalation, WRITE_PARAGRAPH);
    let records = records_of(h, &case);
    assert!(records.is_empty(), "{id}: error records {records:?}");
    assert_request_consumed(h, &case).await;
    h.assert_no_message(&reply_queue, SETTLE).await;
    let rows = all_rows(h).await;
    assert!(
        rows.is_empty(),
        "{id}: the unproven decline was written: {rows:?}"
    );
}

/// `P-COPAUA0C-026`, `P-COPAUA0C-028`, `P-COPAUA0C-035`: a card the
/// cross-reference lacks.
///
/// What is pinned is asserted: the `A001` warning (`P-COPAUA0C-006`) and that
/// nothing is written, since `8000` runs only for a found card
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L463-465], the row's
/// `database_writes=none`. Its decision on the first message is not pinned
/// (E-41 at `halt`), so the row's `05`/`3100` reply is a proposal: the
/// consumer logs the escalation at `6000-MAKE-DECISION`, sends no reply and
/// rejects the request, which leaves the request queue as the COBOL's
/// no-syncpoint get does (`P-COPAUA0C-002`).
async fn unknown_card(h: &Harness, id: &'static str, escalation: &str) {
    let case = Case::new(id);
    assert_eq!(
        row_token(id, "database_writes"),
        "none",
        "audit row {id} writes"
    );
    let before = all_rows(h).await;
    let reply_queue = send(h, &case).await;
    let span = message_span(h, &case).await;
    assert_span(&span, &case, "rejected", DECISION_PARAGRAPH);
    assert_a001(h, &case).await;
    assert_halted(h, &case, escalation, DECISION_PARAGRAPH);
    assert_request_consumed(h, &case).await;
    h.assert_no_message(&reply_queue, SETTLE).await;
    assert_eq!(
        all_rows(h).await,
        before,
        "{id}: pending authorizations changed"
    );
}

/// The database after every case: the summary of the last approval
/// (`P-COPAUA0C-025`) with its one detail, and nothing else, so neither the
/// decline nor the unknown card left a row.
async fn assert_final_state(h: &Harness) {
    let rows = all_rows(h).await;
    let accounts: Vec<AccountId> = rows.iter().map(|(summary, _)| summary.pa_acct_id).collect();
    assert_eq!(
        accounts,
        [fixture_account()],
        "summaries at the end of the run"
    );
    let last = Case::new("P-COPAUA0C-025");
    assert_eq!(rows[0].1.len(), 1, "details at the end of the run");
    assert_detail(&rows[0].1[0], &last);
}

/// `P-COPAUA0C-013`, `P-COPAUA0C-014`, `P-COPAUA0C-035`: no write failed in
/// the run: no line holds the `I003` or `I004` location or the record text
/// of either.
fn assert_no_write_failures(h: &Harness) {
    let texts: Vec<String> = ["P-COPAUA0C-013", "P-COPAUA0C-014"]
        .into_iter()
        .flat_map(audit_texts)
        .collect();
    for line in h.logs() {
        let needles = ["\"I003\"", "\"I004\""]
            .into_iter()
            .chain(texts.iter().map(String::as_str));
        for needle in needles {
            assert!(
                !line.contains(needle),
                "a pending-authorization write failed: {line}"
            );
        }
    }
}

/// Every section D row of `COPAUA0C` against one running consumer, in an
/// order that keeps each row's seeded starting state: the summary of each
/// approval is deleted before the next one, and the decline and the unknown
/// card write nothing. The consumer handles the requests one after another,
/// and each reply is the first-message reply the audit records
/// (`P-COPAUA0C-027`, E-13).
#[test]
fn section_d_requests_get_their_audited_replies_and_writes() {
    block_on(async {
        let escalation = audited_rows();
        let h = Harness::start(Some(PINNED)).await;
        assert!(
            all_rows(&h).await.is_empty(),
            "the seed holds no pending authorization"
        );
        let mut message_ids = HashSet::new();
        approved(&h, "P-COPAUA0C-021", &mut message_ids).await;
        reset(&h).await;
        approved(&h, "P-COPAUA0C-022", &mut message_ids).await;
        reset(&h).await;
        declined(&h, "P-COPAUA0C-023", &escalation, &mut message_ids).await;
        approved(&h, "P-COPAUA0C-024", &mut message_ids).await;
        reset(&h).await;
        approved(&h, "P-COPAUA0C-025", &mut message_ids).await;
        unknown_card(&h, "P-COPAUA0C-026", &escalation).await;
        assert_final_state(&h).await;
        assert_no_write_failures(&h);
        h.shutdown().await;
    });
}

/// An amount of the fixture account as `Money`, from its audit text.
fn money<const D: u8>(amount: &str) -> Money<D> {
    Money::from_cents(cents_of(amount)).expect("an amount within the field")
}

/// The summary a purge sweep holds when the request of
/// [`a_request_during_a_purge_waits_for_it_and_writes_on_what_it_left`]
/// arrives: the fixture account with three approvals and 69.00 of its credit
/// limit left, so a decision on it would decline the row's 100.00
/// (`P-COPAUA0C-017`).
fn purged_summary() -> PendingAuthSummary {
    PendingAuthSummary {
        pa_acct_id: fixture_account(),
        pa_cust_id: fixture_customer(),
        pa_auth_status: Alnum::spaces(),
        pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
        pa_credit_limit: money(CREDIT_LIMIT),
        pa_cash_limit: money(CASH_LIMIT),
        pa_credit_balance: money("6100.00"),
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: AuthCount::new(3).expect("a count of at most four digits"),
        pa_declined_auth_cnt: AuthCount::ZERO,
        pa_approved_auth_amt: money("6100.00"),
        pa_declined_auth_amt: Money::ZERO,
    }
}

/// An approval under [`purged_summary`], dated [`PURGED_DATE_9C`].
fn purged_detail() -> NewPendingAuthDetail {
    NewPendingAuthDetail {
        pa_acct_id: fixture_account(),
        pa_auth_date_9c: PURGED_DATE_9C,
        pa_auth_time_9c: TIME_9C,
        pa_auth_orig_date: Alnum::move_from("220601"),
        pa_auth_orig_time: Alnum::move_from("120000"),
        pa_card_num: CardNumber::move_from("0500024453765740"),
        pa_auth_type: Alnum::move_from("0100"),
        pa_card_expiry_date: Alnum::move_from("1225"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("120000"),
        pa_auth_resp_code: Alnum::move_from("00"),
        pa_auth_resp_reason: Alnum::move_from("0000"),
        pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
        pa_transaction_amt: money("6100.00"),
        pa_approved_amt: money("6100.00"),
        pa_merchant_catagory_code: Alnum::move_from("5411"),
        pa_acqr_country_code: Alnum::move_from("840"),
        pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
        pa_merchant_id: Alnum::move_from("M00000000000001"),
        pa_merchant_name: Alnum::move_from("FIXTURE MERCHANT"),
        pa_merchant_city: Alnum::move_from("SEATTLE"),
        pa_merchant_state: Alnum::move_from("WA"),
        pa_merchant_zip: Alnum::move_from("98101"),
        pa_transaction_id: Alnum::move_from("TXNPURGED000001"),
        pa_match_status: MatchStatus::Pending,
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    }
}

/// Commits [`purged_summary`] and [`purged_detail`] through the port.
async fn commit_purged_state(h: &Harness) {
    let mut uow = h.db.begin().await.expect("open the setup unit of work");
    uow.store()
        .insert_summary(purged_summary())
        .await
        .expect("insert the summary the purge holds");
    uow.store()
        .insert_detail(purged_detail())
        .await
        .expect("insert the detail under it");
    uow.commit().await.expect("commit the setup");
}

/// `CBPAUP0C` as the request meets it (`P-CBPAUP0C-002`, `P-CBPAUP0C-006`,
/// `P-CBPAUP0C-007`): a purge unit of work that has taken the purge
/// exclusion, swept the summaries with its locking `GN` and deleted the
/// fixture account's summary, its detail with it, without committing. It
/// holds the deleted summary's row lock.
async fn holding_sweep(h: &Harness) -> UnitOfWork {
    let mut uow = h.db.begin().await.expect("open the purge unit of work");
    uow.store()
        .exclude_other_purges()
        .await
        .expect("the purge excludes other purges");
    let swept = uow
        .store()
        .summaries_after(None, ROW_LIMIT)
        .await
        .expect("the sweep reads the summaries");
    let accounts: Vec<AccountId> = swept.iter().map(|summary| summary.pa_acct_id).collect();
    assert_eq!(
        accounts,
        [fixture_account()],
        "the summaries the sweep holds"
    );
    uow.store()
        .delete_summary(fixture_account())
        .await
        .expect("the purge deletes the summary");
    uow
}

/// Whether a session of the test database waits in a locking summary read
/// ([`SUMMARY_LOCK_WAITS`]) within [`LOCK_WAIT`].
async fn summary_lock_wait_seen(h: &Harness) -> bool {
    let until = Instant::now() + LOCK_WAIT;
    loop {
        let rows = h.stack.fetch_text_rows(SUMMARY_LOCK_WAITS).await;
        let waiting = rows.first().and_then(|row| row.first()).cloned().flatten();
        if waiting.is_some_and(|sessions| sessions != "0") {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        pause(POLL_INTERVAL).await;
    }
}

/// `P-COPAUA0C-008`, `P-COPAUA0C-013`, `P-COPAUA0C-021`, `P-CBPAUP0C-006`: a
/// request that arrives while a purge holds its account's summary.
///
/// `COPAUA0C` and the `CBPAUP0C` BMP both run under the PSB `PSBPAUTB` with
/// `PROCOPT=AP` [app/app-authorization-ims-db2-mq/ims/PSBPAUTB.psb:L17], and
/// IMS program isolation orders them one after the other. The purge here has
/// deleted the summary, which would decline the row's 100.00, and has not
/// committed. CP00's summary read waits on the purge's row lock, as
/// `pg_stat_activity` shows, so no reply exists while the purge holds it.
/// Once the purge commits, CP00 decides on what it left, no summary, so the
/// fixture account's available amount approves the row (`P-COPAUA0C-018`)
/// with the reply the audit records, and the write inserts a new summary of
/// one approval with exactly the new detail: the result of the purge followed
/// by the request.
#[test]
fn a_request_during_a_purge_waits_for_it_and_writes_on_what_it_left() {
    block_on(async {
        let h = Harness::start(Some(PINNED)).await;
        commit_purged_state(&h).await;
        let holder = holding_sweep(&h).await;
        let case = Case::new("P-COPAUA0C-021");
        let reply_queue = send(&h, &case).await;
        let waited = summary_lock_wait_seen(&h).await;
        // The purge ends before any assertion, so a failed one never leaves the
        // consumer waiting on a lock the test still holds.
        holder.commit().await.expect("the purge commits");
        assert!(
            waited,
            "CP00's summary read never waited on the purge's row lock within {LOCK_WAIT:?}"
        );

        let reply = h.next_message(&reply_queue, REPLY_WAIT).await;
        assert_reply(&reply, &case, &mut HashSet::new());
        let span = message_span(&h, &case).await;
        assert_span(&span, &case, "replied", NO_LOCATION);
        assert_request_consumed(&h, &case).await;

        let (summary, details) = settled(&h, fixture_account(), |summary, details| {
            summary.is_some() && !details.is_empty()
        })
        .await;
        let summary =
            summary.unwrap_or_else(|| panic!("{}: no summary within {DATA_WAIT:?}", case.id));
        assert_approved_summary(&summary, &case);
        assert_eq!(
            details.len(),
            1,
            "{}: details under the new summary",
            case.id
        );
        assert_detail(&details[0], &case);
        let records = records_of(&h, &case);
        assert!(records.is_empty(), "{}: error records {records:?}", case.id);
        assert_no_write_failures(&h);
        h.assert_no_message(&reply_queue, SETTLE).await;
        h.shutdown().await;
    });
}
