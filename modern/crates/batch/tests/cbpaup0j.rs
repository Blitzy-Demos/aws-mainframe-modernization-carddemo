//! `carddemo-batch cbpaup0j`: job CBPAUP0J, whose program CBPAUP0C purges the expired pending
//! authorizations, run as a subprocess against a `postgres:16` testcontainer (g3b; the rows of
//! `modern/audit/batch/CBPAUP0J.md`).
//!
//! No ASCII fixture holds authorizations (AAP 0.2.2), so each test seeds summaries and details
//! through the repositories, keyed as CP00 keys them, runs the real binary and asserts what the
//! audit rows state: the tables, the SYSOUT lines and the exit status. Expected values are
//! written here from the rows, never computed by `domain::batch::cbpaup0c`.
//!
//! # Today
//!
//! The binary reads today from the system clock, once, as `YYDDD` (CBPAUP0C-001), and the tests
//! build their keys from the same clock through `domain::clock::yyddd`. A detail dated 1000 below
//! today has a day difference of +1000 and one dated 1000 above it −1000: both stay inside the
//! four digits of `WS-DAY-DIFF` (so halt candidate `X-CBPAUP0J-H1` never applies) and far from
//! every expiry of 0 to 99 days, whatever the day, a midnight or a New Year during the run
//! included. Only a detail dated today, difference 0, tells expiry 0 from expiry 5. A later day
//! only raises that difference, so under expiry 0 the detail is deleted whatever day the binary
//! read, and its deletion is asserted on every run. Under expiry 5 a New Year during the run
//! would expire it, so its staying with its summary, the survivor check that includes that
//! summary and every SYSOUT comparison that includes the `TODAYS DATE` line are asserted only
//! when today did not change during the run.
//!
//! # Standard output
//!
//! Each SYSOUT line is one JSON event of the job whose only field is the line's text
//! (`jobs::cbpaup0j`); the job's diagnostics carry further fields, and lines that do not begin
//! with `{` are written paths, of which this job has none. The runs keep the job's own events at
//! `INFO` and every other target at `ERROR`, so standard output holds the SYSOUT lines.
//!
//! # Summary deletes
//!
//! The summary deletes asserted here are the replacement's behaviour that AAP 0.3.4.3 specifies
//! ("empty summaries deleted") and CBPAUP0C-016 states. Halt candidate `X-CBPAUP0J-H2`, not yet
//! folded into `HALTS.md`, questions whether IMS accepts that `DLET` after the `GNP` commands of
//! the same PCB; if the reviewer folds it in, those assertions become blocked parity cases.
//!
//! # Rules
//!
//! - Cardholder Data Redaction: the seeded card number is read from `cardxref.txt` line 1 at run
//!   time and lives only in memory and in the test database; `CardNumber` shows only its redacted
//!   form, so no assertion message can print it.
//! - Checked Integer Narrowing: keys come from checked arithmetic and `PicS::complement_from`.
//! - Local-Only Runtime and Single Schema Executor: the database is `test_support`'s
//!   testcontainer, migrated by the harness; the binary never migrates.
//! - R4: every statement is a repository call of `persistence`.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::{fs, io};

use copybook::{Count, PicS, PicU, PicX};
use domain::clock::{Clock, SystemClock, yyddd};
use domain::money::{Decimal, Money, MoneyError};
use domain::records::{PendingAuthDetailRecord, PendingAuthSummaryRecord};
use domain::sensitive::CardNumber;
use persistence::{PendingAuthDetailRepository, PendingAuthSummaryRepository, PgStore};
use test_support::fixtures::{fixture_dir, fixture_path};
use test_support::postgres::start_postgres;

type Outcome<T> = Result<T, Box<dyn Error>>;
type TestResult = Outcome<()>;

/// Runs `test` to completion on a multi-thread runtime with every driver enabled and the default
/// worker count, the one Tokio's test attribute builds for its `multi_thread` flavour, without the
/// lint attributes, `expect` and unchecked pin that attribute's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

/// The default `--sysin`, the in-stream card of `CBPAUP0J.jcl:36-37`: expiry `00`, checkpoint
/// and display frequency `00001`, debug `Y` (CBPAUP0C-002).
const DEFAULT_CARD: &str = "00,00001,00001,Y";

/// The 17 bytes of `PRM-INFO` the default card gives, with its trailing space, which
/// `PARM RECEIVED` shows as received (CBPAUP0C-003).
const DEFAULT_RECEIVED: &str = "00,00001,00001,Y ";

/// A card whose expiry is `05`, five days, with debug `N` (CBPAUP0C-004, CBPAUP0C-007).
const FIVE_DAY_CARD: &str = "05,00001,00001,N";

/// What `PARM RECEIVED` shows of any card but the default one or a blank one (CBPAUP0C-003).
const MASKED_RECEIVED: &str = "*****************";

/// `*`, 37 hyphens and `*` (CBPAUP0C-003, CBPAUP0C-021).
const DASHES: &str = "*-------------------------------------*";

/// `RUST_LOG` of every run: the job's events, among them each SYSOUT line, at `INFO`, and every
/// other target at `ERROR`.
const LOG_FILTER: &str = "error,carddemo_batch::jobs::cbpaup0j=info";

/// The `target` member of the job's JSON events.
const JOB_TARGET: &str = r#""target":"carddemo_batch::jobs::cbpaup0j""#;

/// The start of an event's `fields` object whose first member is the message.
const MESSAGE_FIELD: &str = r#""fields":{"message":""#;

/// The day offset of the expired and the kept details: a `YYDDD` difference of +1000 or −1000,
/// within the four digits of `WS-DAY-DIFF PIC S9(4) COMP` (CBPAUP0C-012).
const DAY_OFFSET: u32 = 1_000;

/// `HHMMSSmmm` of every seeded detail, 12:00:00.000, inverted into `PA-AUTH-TIME-9C`.
const NOON: u64 = 120_000_000;

/// `PA-AUTH-RESP-CODE` of an approval, `88 PA-AUTH-APPROVED VALUE '00'` (CIPAUDTY-011).
const APPROVED: [u8; 2] = *b"00";

/// `PA-AUTH-RESP-CODE` of a decline, CP00's insufficient-funds response.
const DECLINED: [u8; 2] = *b"05";

/// The amount of every seeded authorization, 100.00, in cents.
const AUTH_CENTS: i64 = 10_000;

/// The customer of every seeded summary, `PA-CUST-ID PIC 9(09)`.
const CUSTOMER: u64 = 50;

/// The exit status of a run that ended with return code 0 (CBPAUP0C-022).
const NORMAL_END: Option<i32> = Some(0);

/// The exit status of a run that ended through `9999-ABEND`, `MOVE 16 TO RETURN-CODE`
/// (CBPAUP0C-028).
const ABEND_END: Option<i32> = Some(16);

/// A directory of one test's own under Cargo's temporary directory for integration tests,
/// removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    /// An empty directory named after `test` and this process.
    fn new(test: &str) -> io::Result<Self> {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("cbpaup0j-{test}-{}", process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A directory left behind holds no record and is removed by the next run of the test.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What one run of `carddemo-batch` produced.
struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The lines of standard output that are not JSON events: the paths the job wrote, one per
    /// line (AAP 0.3.4.3).
    fn written_paths(&self) -> Vec<&str> {
        self.stdout
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('{'))
            .collect()
    }

    /// The SYSOUT lines, in program order: the job's events whose `fields` hold the message
    /// alone.
    fn sysout(&self) -> Vec<String> {
        self.stdout
            .lines()
            .filter(|line| line.contains(JOB_TARGET))
            .filter_map(sole_message)
            .collect()
    }
}

/// The message of the event `line` when its `fields` object holds nothing else.
fn sole_message(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(MESSAGE_FIELD)?;
    let (message, after) = json_string(rest)?;
    after.starts_with('}').then_some(message)
}

/// The unescaped JSON string that `text` starts with, its opening quote already consumed, and
/// the text after its closing quote; `None` when `text` holds no well-formed string.
fn json_string(text: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((value, text.get(at + 1..)?)),
            '\\' => {
                let (_, escape) = chars.next()?;
                value.push(match escape {
                    '"' | '\\' | '/' => escape,
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        let hex: String = chars.by_ref().take(4).map(|(_, digit)| digit).collect();
                        if hex.len() != 4 || !hex.chars().all(|digit| digit.is_ascii_hexdigit()) {
                            return None;
                        }
                        char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                    }
                    _ => return None,
                });
            }
            other => value.push(other),
        }
    }
    None
}

/// Runs `carddemo-batch` with `args` from `modern/`, against `database_url`, with every output
/// directory inside `scratch`, and waits for it through `test_support::run_child`, which kills a
/// run still going at `test_support::CHILD_DEADLINE`.
async fn run_batch(database_url: &str, scratch: &Scratch, args: &[&str]) -> Outcome<Run> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .args(args)
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", database_url)
        .env("CARDDEMO_VAR_DIR", scratch.path().join("var"))
        .env(
            "CARDDEMO_STATEMENT_DIR",
            scratch.path().join("var/statements"),
        )
        .env("CARDDEMO_INTRDR_DIR", scratch.path().join("var/jobs"))
        .env("CARDDEMO_FIXTURE_DIR", fixture_dir())
        .env("RUST_LOG", LOG_FILTER)
        .env_remove("CARDDEMO_OTEL_EXPORT");
    let output = test_support::run_child(command, test_support::CHILD_DEADLINE).await?;
    Ok(Run {
        status: output.status.code(),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

/// Today's `YYDDD` from the system clock the binary reads (CBPAUP0C-001).
fn today() -> u32 {
    yyddd(SystemClock.now())
}

/// Whether today is still `before`, read ahead of a run: the run read today between the two
/// readings, so it read `before` too. When today changed, the outcomes that depend on it are
/// left unasserted, and the test says so.
fn day_held(before: u32) -> bool {
    let held = today() == before;
    if !held {
        eprintln!(
            "today changed during the run: the outcomes that depend on today are not asserted"
        );
    }
    held
}

/// The `PA-AUTH-DATE-9C` keys of the seeded details around today `T`.
#[derive(Clone, Copy)]
struct DateKeys {
    /// Dated `T` − 1000: day difference +1000, expired under every expiry of 0 to 99 days.
    expired: PicS<5>,
    /// Dated `T` + 1000: day difference −1000, kept under every such expiry.
    kept: PicS<5>,
    /// Dated `T`: day difference 0, expired under expiry 0 and kept under expiry 5.
    today: PicS<5>,
}

impl DateKeys {
    fn around(today: u32) -> Outcome<Self> {
        let earlier = today
            .checked_sub(DAY_OFFSET)
            .ok_or("today minus 1000 is negative")?;
        let later = today
            .checked_add(DAY_OFFSET)
            .ok_or("today plus 1000 overflows")?;
        Ok(Self {
            expired: date_key(earlier)?,
            kept: date_key(later)?,
            today: date_key(today)?,
        })
    }
}

/// The key of an authorization dated `yyddd`: 99999 − `yyddd`, as CP00 stores it
/// (`COPAUA0C.cbl:874`) and CBPAUP0C-011 turns it back into the date.
fn date_key(yyddd: u32) -> Outcome<PicS<5>> {
    Ok(PicS::complement_from(u64::from(yyddd))?)
}

/// `cents` hundredths as an amount of picture `S9(INT)V99`.
fn money<const INT: u32>(cents: i64) -> Result<Money<INT>, MoneyError> {
    Money::new(Decimal::new(cents, 2))
}

/// The card number of `cardxref.txt` line 1, its first 16 bytes (`XREF-CARD-NUM`), read in
/// place.
fn card_number() -> Outcome<CardNumber> {
    let file = fs::read(fixture_path("cardxref.txt"))?;
    let bytes = file.get(..16).ok_or("cardxref.txt holds a first record")?;
    Ok(CardNumber::new(PicX::decode(bytes)?))
}

/// The summary of account `acct` as CP00 leaves it after `approved_cnt` approvals totalling
/// `approved_cents` and `declined_cnt` declines totalling `declined_cents` (`CIPAUSMY`): the
/// approvals are in the credit balance too, which CBPAUP0C never names (CBPAUP0C-017).
fn summary(
    acct: i64,
    approved_cnt: i16,
    approved_cents: i64,
    declined_cnt: i16,
    declined_cents: i64,
) -> Outcome<PendingAuthSummaryRecord> {
    let mut record = PendingAuthSummaryRecord::initialized(PicS::new(acct)?, PicU::new(CUSTOMER)?);
    record.credit_limit = money(500_000)?;
    record.cash_limit = money(100_000)?;
    record.credit_balance = money(approved_cents)?;
    record.approved_auth_cnt = Count::new(approved_cnt);
    record.approved_auth_amt = money(approved_cents)?;
    record.declined_auth_cnt = Count::new(declined_cnt);
    record.declined_auth_amt = money(declined_cents)?;
    Ok(record)
}

/// A detail of 100.00 dated by `date_9c`, at noon, on `card` (`CIPAUDTY`), keyed as CP00 keys it
/// (`COPAUA0C.cbl:868-875`): an approval `00`/`0000` with that approved amount and match status
/// `P`, or a decline `05`/`4100` with no approved amount and match status `D`.
fn detail(
    date_9c: PicS<5>,
    resp_code: [u8; 2],
    card: &CardNumber,
) -> Outcome<PendingAuthDetailRecord> {
    let amount = money(AUTH_CENTS)?;
    let (reason, approved_amt, match_status) = if resp_code == APPROVED {
        (*b"0000", amount, *b"P")
    } else {
        (*b"4100", Money::ZERO, *b"D")
    };
    Ok(PendingAuthDetailRecord {
        auth_date_9c: date_9c,
        auth_time_9c: PicS::complement_from(NOON)?,
        orig_date: PicX::spaces(),
        orig_time: PicX::spaces(),
        card_num: card.clone(),
        auth_type: PicX::from_array(*b"PURC"),
        card_expiry_date: PicX::from_array(*b"1230"),
        message_type: PicX::from_array(*b"0100  "),
        message_source: PicX::from_array(*b"POS   "),
        auth_id_code: PicX::from_array(*b"123456"),
        auth_resp_code: PicX::from_array(resp_code),
        auth_resp_reason: PicX::from_array(reason),
        processing_code: PicU::new(0)?,
        transaction_amt: amount,
        approved_amt,
        merchant_catagory_code: PicX::from_array(*b"5411"),
        acqr_country_code: PicX::from_array(*b"840"),
        pos_entry_mode: PicU::new(1)?,
        merchant_id: PicX::from_array(*b"MERCHANT0000001"),
        merchant_name: PicX::from_array(*b"CARDDEMO TEST MERCHANT"),
        merchant_city: PicX::from_array(*b"SEATTLE      "),
        merchant_state: PicX::from_array(*b"WA"),
        merchant_zip: PicX::from_array(*b"98101    "),
        transaction_id: PicX::from_array(*b"TRANSACTION0001"),
        match_status: PicX::from_array(match_status),
        auth_fraud: PicX::spaces(),
        fraud_rpt_date: PicX::spaces(),
        filler: PicX::spaces(),
    })
}

/// One seeded account: its summary and its details in key order.
struct Account {
    summary: PendingAuthSummaryRecord,
    details: Vec<PendingAuthDetailRecord>,
}

impl Account {
    /// Stores the summary, then each detail under it.
    async fn insert(&self, store: &PgStore) -> TestResult {
        let mut store = store.clone();
        PendingAuthSummaryRepository::insert(&mut store, &self.summary).await?;
        for detail in &self.details {
            PendingAuthDetailRepository::insert(&mut store, self.summary.acct_id, detail).await?;
        }
        Ok(())
    }
}

/// The accounts every run is seeded with, in ascending `acct_id`, the order in which the purge
/// reads them (CBPAUP0C-009).
struct Seeded {
    /// Account 11: approved count 1, its one approval expired.
    lone_approval: Account,
    /// Account 22: approved count 2, an approval dated ahead and, after it in key order, an
    /// expired one.
    partly_expired: Account,
    /// Account 33: approved count 1 and declined count 1, its one detail an expired decline.
    expired_decline: Account,
    /// Account 44: approved count 0 and no detail.
    empty: Account,
    /// Account 55: approved count 1, its one approval dated today.
    todays_approval: Account,
    /// Account 66: approved count 0 and declined count 1, its one decline dated ahead.
    declines_only: Account,
}

impl Seeded {
    fn accounts(&self) -> [&Account; 6] {
        [
            &self.lone_approval,
            &self.partly_expired,
            &self.expired_decline,
            &self.empty,
            &self.todays_approval,
            &self.declines_only,
        ]
    }
}

/// Seeds the six accounts of [`Seeded`], their details dated by `keys`.
async fn seed(store: &PgStore, keys: DateKeys) -> Outcome<Seeded> {
    let card = card_number()?;
    let approval = |date_9c| detail(date_9c, APPROVED, &card);
    let decline = |date_9c| detail(date_9c, DECLINED, &card);
    let account = |summary, details| Account { summary, details };
    let seeded = Seeded {
        lone_approval: account(summary(11, 1, 10_000, 0, 0)?, vec![approval(keys.expired)?]),
        partly_expired: account(
            summary(22, 2, 20_000, 0, 0)?,
            vec![approval(keys.kept)?, approval(keys.expired)?],
        ),
        expired_decline: account(
            summary(33, 1, 10_000, 1, 10_000)?,
            vec![decline(keys.expired)?],
        ),
        empty: account(summary(44, 0, 0, 0, 0)?, Vec::new()),
        todays_approval: account(summary(55, 1, 10_000, 0, 0)?, vec![approval(keys.today)?]),
        declines_only: account(summary(66, 0, 0, 1, 10_000)?, vec![decline(keys.kept)?]),
    };
    for account in seeded.accounts() {
        account.insert(store).await?;
    }
    Ok(seeded)
}

/// What a run leaves of a seeded account.
enum Left<'k> {
    /// Nothing: the summary is deleted and, through `ON DELETE CASCADE`, every detail still
    /// under it (CBPAUP0C-016).
    Nothing,
    /// The summary exactly as seeded, no count or amount written back (CBPAUP0C-017; E30), with
    /// the details dated by these keys; every other detail is deleted (CBPAUP0C-015).
    Summary(&'k [PicS<5>]),
}

/// Asserts that the run left `left` of `account`, read back by key.
async fn assert_left(store: &PgStore, account: &Account, left: Left<'_>) -> TestResult {
    let mut store = store.clone();
    let acct_id = account.summary.acct_id;
    let acct = acct_id.get();
    let summary = PendingAuthSummaryRepository::find_by_id(&mut store, acct_id).await?;
    let details = PendingAuthDetailRepository::find_all_by_acct_id(&mut store, acct_id).await?;
    match left {
        Left::Nothing => {
            assert_eq!(summary, None, "account {acct}: the summary is deleted");
            assert!(details.is_empty(), "account {acct}: no detail is left");
        }
        Left::Summary(kept) => {
            assert_eq!(
                summary.as_ref(),
                Some(&account.summary),
                "account {acct}: the summary is stored as seeded"
            );
            let expected: Vec<_> = account
                .details
                .iter()
                .filter(|detail| kept.contains(&detail.auth_date_9c))
                .cloned()
                .collect();
            assert_eq!(details, expected, "account {acct}: the details left");
        }
    }
    Ok(())
}

/// Asserts that the summaries of `survivors`, as seeded, are all that is left, in ascending
/// `acct_id`.
async fn assert_survivors(store: &PgStore, survivors: &[&Account]) -> TestResult {
    let mut store = store.clone();
    let left = PendingAuthSummaryRepository::find_all(&mut store).await?;
    let expected: Vec<_> = survivors
        .iter()
        .map(|account| account.summary.clone())
        .collect();
    assert_eq!(left, expected, "the summaries left");
    Ok(())
}

/// The five opening lines, with `received` shown for `PRM-INFO` and today's `YYDDD`
/// (CBPAUP0C-003).
fn opening(received: &str, today: u32) -> Vec<String> {
    vec![
        "STARTING PROGRAM CBPAUP0C::".to_owned(),
        DASHES.to_owned(),
        format!("CBPAUP0C PARM RECEIVED :{received}"),
        format!("TODAYS DATE            :{today:05}"),
        " ".to_owned(),
    ]
}

/// The eight end lines with the program's four totals, 8 digits each (CBPAUP0C-021).
fn totals(
    summaries_read: u32,
    summaries_deleted: u32,
    details_read: u32,
    details_deleted: u32,
) -> Vec<String> {
    vec![
        " ".to_owned(),
        DASHES.to_owned(),
        format!("# TOTAL SUMMARY READ  :{summaries_read:08}"),
        format!("# SUMMARY REC DELETED :{summaries_deleted:08}"),
        format!("# TOTAL DETAILS READ  :{details_read:08}"),
        format!("# DETAILS REC DELETED :{details_deleted:08}"),
        DASHES.to_owned(),
        " ".to_owned(),
    ]
}

/// The debug lines of the default card's run over [`Seeded`] (CBPAUP0C-008): before every
/// summary read and every detail read, the one that finds no further row included, the count of
/// rows read so far, and before every delete the summary's `PA-ACCT-ID`.
const DEFAULT_RUN_DEBUG: [&str; 27] = [
    // Account 11: its expired approval, then the summary.
    "DEBUG: AUTH SMRY READ : 00000000",
    "DEBUG: AUTH DTL READ : 00000000",
    "DEBUG: AUTH DTL DLET : 00000000011",
    "DEBUG: AUTH DTL READ : 00000001",
    "DEBUG: AUTH SMRY DLET : 00000000011",
    // Account 22: the approval dated ahead first in key order and kept, then the expired one.
    "DEBUG: AUTH SMRY READ : 00000001",
    "DEBUG: AUTH DTL READ : 00000001",
    "DEBUG: AUTH DTL READ : 00000002",
    "DEBUG: AUTH DTL DLET : 00000000022",
    "DEBUG: AUTH DTL READ : 00000003",
    // Account 33: its expired decline; the approved count 1 keeps the summary.
    "DEBUG: AUTH SMRY READ : 00000002",
    "DEBUG: AUTH DTL READ : 00000003",
    "DEBUG: AUTH DTL DLET : 00000000033",
    "DEBUG: AUTH DTL READ : 00000004",
    // Account 44: no detail; the approved count 0 deletes the summary.
    "DEBUG: AUTH SMRY READ : 00000003",
    "DEBUG: AUTH DTL READ : 00000004",
    "DEBUG: AUTH SMRY DLET : 00000000044",
    // Account 55: today's approval, then the summary.
    "DEBUG: AUTH SMRY READ : 00000004",
    "DEBUG: AUTH DTL READ : 00000004",
    "DEBUG: AUTH DTL DLET : 00000000055",
    "DEBUG: AUTH DTL READ : 00000005",
    "DEBUG: AUTH SMRY DLET : 00000000055",
    // Account 66: its decline dated ahead is kept, and the summary deleted with it.
    "DEBUG: AUTH SMRY READ : 00000005",
    "DEBUG: AUTH DTL READ : 00000005",
    "DEBUG: AUTH DTL READ : 00000006",
    "DEBUG: AUTH SMRY DLET : 00000000066",
    // The read that reaches the end of the database.
    "DEBUG: AUTH SMRY READ : 00000006",
];

/// The default card: expiry 0 days, debug on. Every detail dated today or earlier expires; a
/// summary whose approved count, less its expired approvals, is 0 or less is deleted with its
/// details, and every other summary keeps each stored count and amount. The run shows the debug
/// lines and the totals, writes no file and returns 0.
#[test]
fn default_sysin_purges_expired_and_keeps_amounts() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("default")?;
        let before = today();
        let keys = DateKeys::around(before)?;
        let seeded = seed(&store, keys).await?;

        let run = run_batch(guard.database_url(), &scratch, &["cbpaup0j"]).await?;
        let held = day_held(before);

        // audit: CBPAUP0C-022
        assert_eq!(run.status, NORMAL_END, "{}", run.stderr);
        assert!(run.written_paths().is_empty(), "cbpaup0j writes no file");
        // The approved count 1 less its expired approval is 0: the detail and the summary go.
        // audit: CBPAUP0C-011
        // audit: CBPAUP0C-012
        // audit: CBPAUP0C-013
        // audit: CBPAUP0C-015
        // audit: CBPAUP0C-016
        assert_left(&store, &seeded.lone_approval, Left::Nothing).await?;
        // The approved count 2 less one expired approval is 1: the summary stays exactly as seeded,
        // with the approval dated ahead (E30).
        // audit: CBPAUP0C-017
        assert_left(&store, &seeded.partly_expired, Left::Summary(&[keys.kept])).await?;
        // An expired decline moves only the declined pair in memory: the approved count 1 keeps the
        // summary, unchanged, while the decline is deleted.
        // audit: CBPAUP0C-014
        assert_left(&store, &seeded.expired_decline, Left::Summary(&[])).await?;
        // A stored approved count of 0 deletes the summary on every run, with no detail or with a
        // decline dated ahead, which goes with it (X-CBPAUP0J-2).
        assert_left(&store, &seeded.empty, Left::Nothing).await?;
        assert_left(&store, &seeded.declines_only, Left::Nothing).await?;
        // Under expiry 0 a detail dated today or earlier qualifies, so today's approval goes, and
        // its summary with it, whether or not the day changed during the run.
        // audit: CBPAUP0C-004
        assert_left(&store, &seeded.todays_approval, Left::Nothing).await?;
        // The final checkpoint committed every delete: the test's own connections see them.
        // audit: CBPAUP0C-019
        assert_survivors(&store, &[&seeded.partly_expired, &seeded.expired_decline]).await?;

        if held {
            let mut expected = opening(DEFAULT_RECEIVED, before);
            expected.extend(DEFAULT_RUN_DEBUG.map(String::from));
            expected.extend(totals(6, 4, 6, 4));
            // The summaries come in ascending account order and each one's details in key order;
            // six summaries are far below the 1000 at which the count first compares above `00001`
            // as text, so no checkpoint is taken in the loop and no CHKP SUCCESS line is shown. The
            // decline removed with account 66 is read, not counted as deleted.
            // audit: CBPAUP0C-001
            // audit: CBPAUP0C-002
            // audit: CBPAUP0C-003
            // audit: CBPAUP0C-007
            // audit: CBPAUP0C-008
            // audit: CBPAUP0C-009
            // audit: CBPAUP0C-010
            // audit: CBPAUP0C-018
            // audit: CBPAUP0C-020
            // audit: CBPAUP0C-021
            assert_eq!(run.sysout(), expected);
        }
        Ok(())
    })
}

/// A five-day card, debug off: the expired details and the summaries they empty go as under
/// the default card, while today's approval, a day difference of 0, is below the expiry and
/// keeps its summary. `PARM RECEIVED` masks a card other than the default one, and only the
/// opening and end lines are shown.
#[test]
fn explicit_sysin_expiry_keeps_recent() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("five-days")?;
        let before = today();
        let keys = DateKeys::around(before)?;
        let seeded = seed(&store, keys).await?;

        let args = ["cbpaup0j", "--sysin", FIVE_DAY_CARD];
        let run = run_batch(guard.database_url(), &scratch, &args).await?;
        let held = day_held(before);

        // audit: CBPAUP0C-022
        assert_eq!(run.status, NORMAL_END, "{}", run.stderr);
        assert!(run.written_paths().is_empty(), "cbpaup0j writes no file");
        // A day difference of +1000 is above five days and −1000 below it.
        // audit: CBPAUP0C-012
        // audit: CBPAUP0C-015
        // audit: CBPAUP0C-016
        // audit: CBPAUP0C-017
        assert_left(&store, &seeded.lone_approval, Left::Nothing).await?;
        assert_left(&store, &seeded.partly_expired, Left::Summary(&[keys.kept])).await?;
        assert_left(&store, &seeded.expired_decline, Left::Summary(&[])).await?;
        assert_left(&store, &seeded.empty, Left::Nothing).await?;
        assert_left(&store, &seeded.declines_only, Left::Nothing).await?;
        if !held {
            return Ok(());
        }
        // Expiry `05` is five days: today's approval, a difference of 0, is kept, and the approved
        // count 1 keeps its summary unchanged.
        // audit: CBPAUP0C-004
        assert_left(
            &store,
            &seeded.todays_approval,
            Left::Summary(&[keys.today]),
        )
        .await?;
        let survivors = [
            &seeded.partly_expired,
            &seeded.expired_decline,
            &seeded.todays_approval,
        ];
        assert_survivors(&store, &survivors).await?;
        let mut expected = opening(MASKED_RECEIVED, before);
        expected.extend(totals(6, 3, 6, 3));
        // audit: CBPAUP0C-002
        // audit: CBPAUP0C-003
        // audit: CBPAUP0C-007
        // audit: CBPAUP0C-021
        assert_eq!(run.sysout(), expected);
        Ok(())
    })
}

/// A summary read that fails, here against the container's `postgres` database, which holds no
/// schema: the failure lines, `CBPAUP0C ABENDING ...` and return code 16, with no end lines. The
/// default card typed out is shown as received, as when `--sysin` is left out.
#[test]
fn an_unreadable_summary_table_abends_with_return_code_16() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::new("abend")?;
        let unmigrated = guard
            .database_url()
            .strip_suffix("/carddemo")
            .map(|server| format!("{server}/postgres"))
            .ok_or("the harness URL names the database carddemo")?;
        let before = today();

        let args = ["cbpaup0j", "--sysin", DEFAULT_CARD];
        let run = run_batch(&unmigrated, &scratch, &args).await?;
        let held = day_held(before);

        // audit: CBPAUP0C-028
        assert_eq!(run.status, ABEND_END, "{}", run.stderr);
        assert!(
            run.stderr
                .starts_with("carddemo-batch cbpaup0j: ABEND: CBPAUP0C ABENDING ..."),
            "{}",
            run.stderr
        );
        assert!(run.written_paths().is_empty(), "cbpaup0j writes no file");
        let sysout = run.sysout();
        let (shown, after) = sysout
            .split_at_checked(6)
            .ok_or("six lines before the failure")?;
        if held {
            let mut expected = opening(DEFAULT_RECEIVED, before);
            expected.push("DEBUG: AUTH SMRY READ : 00000000".to_owned());
            // audit: CBPAUP0C-001
            // audit: CBPAUP0C-002
            // audit: CBPAUP0C-003
            // audit: CBPAUP0C-008
            assert_eq!(shown, expected);
        }
        let (failure, rest) = after.split_first().ok_or("the failure line")?;
        // The two characters after the label are the DL/I status, BLOCKED by halt H4: only the
        // label is compared.
        // audit: CBPAUP0C-029
        eprintln!(
            "BLOCKED (H4): the status characters of the summary read failure are not asserted"
        );
        // audit: CBPAUP0C-023
        assert!(
            failure.starts_with("AUTH SUMMARY READ FAILED  :"),
            "{failure}"
        );
        assert_eq!(
            rest,
            [
                "SUMMARY READ BEFORE ABEND :00000000",
                "CBPAUP0C ABENDING ..."
            ]
        );
        Ok(())
    })
}
