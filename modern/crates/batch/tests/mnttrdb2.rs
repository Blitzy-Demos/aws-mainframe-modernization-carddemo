//! MNTTRDB2 (COBTUPDT) through the real `carddemo-batch mnttrdb2` binary, one subprocess per
//! run, against a testcontainers `PostgreSQL` (g3b). The audit is
//! `modern/audit/batch/MNTTRDB2.md`; every expected value below is copied from its rows and from
//! the seed members they cite (`DB2LTTYP.ctl:15-23`, `DB2LTCAT.ctl:15-37`), never read from the
//! code under test.
//!
//! # Input
//!
//! The JCL's `INPFILE` dataset is not in the repository (`MNTTRDB2.jcl:27`), so every input file
//! holds records of the set the audit derives, COBTUPDT-101 to COBTUPDT-113: 53 bytes each,
//! `INPUT-REC-TYPE X(1)`, `INPUT-REC-NUMBER X(2)` and `INPUT-REC-DESC X(50)`
//! (`COBTUPDT.cbl:71-77`), followed by LF.
//!
//! # What is observed
//!
//! - The exit status, COBTUPDT's `RETURN-CODE` (COBTUPDT-023).
//! - The rows of `db2.transaction_type`, read through `Db2TransactionTypeRepository`. A
//!   description comes back with its stored length, so a 50-byte value shows the untrimmed bind
//!   of COBTUPDT-010 and COBTUPDT-012 and a seed value its own length.
//! - The categories of `db2.transaction_type_category` and the VSAM data version in
//!   `transaction_types` and `transaction_categories`, compared with a snapshot taken before the
//!   run (E23).
//! - The job log: each `DISPLAY` line of COBTUPDT is one JSON event of the job's module,
//!   `carddemo_batch::jobs::mnttrdb2`, whose `fields` hold the line's text as `message` alone
//!   (`crates/batch/src/jobs/mnttrdb2.rs`). [`Run::job_log`] reads those messages from standard
//!   output, in program order, and the tests compare them line by line with the lines the rows
//!   write, every trailing space included.
//! - The standard-output lines that do not begin with `{`, the paths the job wrote: none, as
//!   MNTTRDB2 writes no file (AAP 0.3.4.3).
//! - The JSON `ERROR` event the job emits for each statement that failed, which names the
//!   paragraph and the cause and comes between the record's action line and its result line, so
//!   a run shows that it reached its last record after earlier failures. Its further fields keep
//!   it out of the job log.
//!
//! The five `SQLCODE` positions of the error text, positions 50-54 of its 80 bytes, are blocked
//! by the halt candidate `X-HALTS-2`, proposed to join H3, and an unreadable input's COBOL
//! continuation by `X-MNTTRDB2-H1`; both are reported `BLOCKED` and never asserted (AAP 0.8.5).
//! Every other position of the error text is.
//!
//! # Environment
//!
//! Each run gets `DATABASE_URL` from its own container, `CARDDEMO_VAR_DIR` and the statement and
//! job-deck directories inside a scratch directory under Cargo's target directory,
//! `CARDDEMO_FIXTURE_DIR` from `test_support`, `RUST_LOG=error,carddemo_batch::jobs::mnttrdb2=info`
//! (the job's events at `INFO`, among them each job-log line, and every other target at `ERROR`)
//! and no span export. Every other variable is inherited, never cleared, so coverage
//! instrumentation reaches the binary. The schema and the Db2 seed come from the migrations
//! `test_support` applies when it starts the container; nothing here touches the schema.
//!
//! # Rules
//!
//! Cardholder Data Redaction: the protected values the redaction test writes into descriptions
//! are read from `cardxref.txt` or assembled from parts at run time, so no source line holds one,
//! and no assertion message prints one.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::atomic::{AtomicUsize, Ordering};

use copybook::PicX;
use domain::records::{
    Db2TransactionCategory, Db2TransactionType, TransactionCategoryRecord, TransactionTypeRecord,
};
use persistence::{
    Db2TransactionTypeCategoryRepository, Db2TransactionTypeRepository, PgStore, PgTx,
    TransactionCategoryRepository, TransactionTypeRepository,
};
use test_support::fixtures::{fixture_dir, fixture_path, start_loaded_postgres};
use test_support::postgres::start_postgres;

type TestResult = Result<(), Box<dyn Error>>;

/// Runs `test` to completion on a multi-thread runtime with every driver enabled and the default
/// worker count, the one Tokio's test attribute builds for its `multi_thread` flavour, without the
/// lint attributes, `expect` and unchecked pin that attribute's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

/// The width of `WS-INPUT-REC`: action, code and description (COBTUPDT-002).
const RECORD_LEN: usize = 53;

/// The width of `INPUT-REC-NUMBER PIC X(2)`.
const CODE_LEN: usize = 2;

/// The width of `INPUT-REC-DESC PIC X(50)`.
const DESC_LEN: usize = 50;

/// One derived input record: its action byte, its type code and its description text, which
/// [`input_record`] space-pads to 2 and 50 bytes.
type Record = (u8, &'static str, &'static str);

/// COBTUPDT-101: a comment line, code 2 spaces.
const COMMENT_101: Record = (b'*', "", "AUDIT TEST INPUT FOR MNTTRDB2");
/// COBTUPDT-102: insert of the absent code `08`.
const ADD_08_102: Record = (b'A', "08", "AUDIT TEST TYPE EIGHT");
/// COBTUPDT-103: update of the seeded code `01`.
const UPDATE_01_103: Record = (b'U', "01", "AUDIT TEST PURCHASE RENAMED");
/// COBTUPDT-104: update of the absent code `99` (`+100`).
const UPDATE_99_104: Record = (b'U', "99", "AUDIT TEST ABSENT TYPE");
/// COBTUPDT-105: delete of `08`, which record 102 added and which has no category.
const DELETE_08_105: Record = (b'D', "08", "");
/// COBTUPDT-106: delete of the absent code `99` (`+100`).
const DELETE_99_106: Record = (b'D', "99", "");
/// COBTUPDT-107: the invalid action byte `X`.
const INVALID_01_107: Record = (b'X', "01", "AUDIT TEST NOT APPLIED");
/// COBTUPDT-108: update of the absent code `98` (`+100`).
const UPDATE_98_108: Record = (b'U', "98", "AUDIT TEST ABSENT TYPE TWO");
/// COBTUPDT-109: insert of the present code `01`, a duplicate key.
const ADD_01_109: Record = (b'A', "01", "AUDIT TEST DUPLICATE KEY");
/// COBTUPDT-110: delete of `02`, which still has categories that the foreign key
/// `trc_type_code` restricts (`TRNTYCAT.ddl:6-7`).
const DELETE_02_110: Record = (b'D', "02", "");

/// The input of COBTUPDT-111: records 1 to 10, in file order.
const DERIVED_RUN: [Record; 10] = [
    COMMENT_101,
    ADD_08_102,
    UPDATE_01_103,
    UPDATE_99_104,
    DELETE_08_105,
    DELETE_99_106,
    INVALID_01_107,
    UPDATE_98_108,
    ADD_01_109,
    DELETE_02_110,
];

/// The input of COBTUPDT-112: the records of COBTUPDT-101, -102, -103 and -105, in that order.
const VALID_RUN: [Record; 4] = [COMMENT_101, ADD_08_102, UPDATE_01_103, DELETE_08_105];

/// The width of `WS-RETURN-MSG PIC X(80)`, every line `9999-ABEND` displays (COBTUPDT-022).
const RETURN_MSG_LEN: usize = 80;

/// Positions 1-49 of the error text of COBTUPDT-019: `Error accessing:` (16 characters) and
/// ` TRANSACTION_TYPE table. SQLCODE:` (33 characters, beginning with a space).
const SQL_ERROR_HEAD: &str = "Error accessing: TRANSACTION_TYPE table. SQLCODE:";

/// Position 55, the first after the five `SQLCODE` characters of COBTUPDT-020, as an offset.
const SQL_ERROR_TAIL_AT: usize = 54;

/// What a row says one record's job log shows after its `PROCESSING` line.
#[derive(Clone, Copy)]
enum Shown {
    /// A line of exactly this text.
    Line(&'static str),
    /// The 80 bytes of `WS-RETURN-MSG`: this text, then spaces (COBTUPDT-021, COBTUPDT-022).
    Message(&'static str),
    /// The error text of COBTUPDT-019: [`SQL_ERROR_HEAD`], the five `SQLCODE` characters of
    /// COBTUPDT-020 (X-HALTS-2) and 26 spaces.
    SqlError,
}

/// One record's job log as its row writes it: the action byte and two code bytes its
/// `PROCESSING` line shows before the 50 `*`, then the lines after that line.
type RecordLog = (&'static str, &'static [Shown]);

/// COBTUPDT-101, lines 2-3 of COBTUPDT-111.
const LOG_101: RecordLog = ("*  ", &[Shown::Line("IGNORING COMMENTED LINE")]);
/// COBTUPDT-102, lines 4-6.
const LOG_102: RecordLog = (
    "A08",
    &[
        Shown::Line("ADDING RECORD"),
        Shown::Line("RECORD INSERTED SUCCESSFULLY"),
    ],
);
/// COBTUPDT-103, lines 7-9.
const LOG_103: RecordLog = (
    "U01",
    &[
        Shown::Line("UPDATING RECORD"),
        Shown::Line("RECORD UPDATED SUCCESSFULLY"),
    ],
);
/// COBTUPDT-104, lines 10-12: the first message of the run.
const LOG_104: RecordLog = (
    "U99",
    &[
        Shown::Line("UPDATING RECORD"),
        Shown::Message("No records found."),
    ],
);
/// COBTUPDT-105, lines 13-15.
const LOG_105: RecordLog = (
    "D08",
    &[
        Shown::Line("DELETING RECORD"),
        Shown::Line("RECORD DELETED SUCCESSFULLY"),
    ],
);
/// COBTUPDT-106, lines 16-18: the same 17 characters written over themselves.
const LOG_106: RecordLog = (
    "D99",
    &[
        Shown::Line("DELETING RECORD"),
        Shown::Message("No records found."),
    ],
);
/// COBTUPDT-107, lines 19-20: no action line.
const LOG_107: RecordLog = ("X01", &[Shown::Message("ERROR: TYPE NOT VALID")]);
/// COBTUPDT-108, lines 21-23: positions 18-21 of COBTUPDT-107's message remain.
const LOG_108: RecordLog = (
    "U98",
    &[
        Shown::Line("UPDATING RECORD"),
        Shown::Message("No records found.ALID"),
    ],
);
/// COBTUPDT-109, lines 24-26: the duplicate key.
const LOG_109: RecordLog = ("A01", &[Shown::Line("ADDING RECORD"), Shown::SqlError]);
/// COBTUPDT-110, lines 27-29: the restricted delete.
const LOG_110: RecordLog = ("D02", &[Shown::Line("DELETING RECORD"), Shown::SqlError]);
/// Record 103 when its update fails (COBTUPDT-014): the error text of COBTUPDT-019 replaces the
/// success line.
const LOG_103_FAILED: RecordLog = ("U01", &[Shown::Line("UPDATING RECORD"), Shown::SqlError]);

/// The job log of COBTUPDT-111 after `OPEN FILE OK`: the lines of COBTUPDT-101 to COBTUPDT-110,
/// in that order.
const DERIVED_LOG: [RecordLog; 10] = [
    LOG_101, LOG_102, LOG_103, LOG_104, LOG_105, LOG_106, LOG_107, LOG_108, LOG_109, LOG_110,
];

/// The job log of COBTUPDT-112 after `OPEN FILE OK`: the lines of COBTUPDT-101, -102, -103 and
/// -105.
const VALID_LOG: [RecordLog; 4] = [LOG_101, LOG_102, LOG_103, LOG_105];

/// The seeded `db2.transaction_type` rows, unpadded (`DB2LTTYP.ctl:17-23`, COBTUPDT-111).
const SEED_TYPES: [(&str, &str); 7] = [
    ("01", "PURCHASE"),
    ("02", "PAYMENT"),
    ("03", "CREDIT"),
    ("04", "AUTHORIZATION"),
    ("05", "REFUND"),
    ("06", "REVERAL"),
    ("07", "ADJUSTMENT"),
];

/// How many categories the seed holds, at least one for each seeded type (`DB2LTCAT.ctl:20-37`).
const SEED_CATEGORY_COUNT: usize = 18;

/// The three seeded categories of type `02` (`DB2LTCAT.ctl:25-27`, COBTUPDT-110).
const PAYMENT_CATEGORIES: [(&str, &str); 3] = [
    ("0001", "CASH PAYMENT"),
    ("0002", "ELECTRONIC PAYMENT"),
    ("0003", "CHECK PAYMENT"),
];

/// More rows than `db2.transaction_type` can hold in any of these runs: the seven seeded codes
/// and the one code a record inserts.
const ALL_ROWS: i64 = 100;

/// `RUST_LOG` of every run: the job's events, among them each job-log line, at `INFO`, and every
/// other target at `ERROR`.
const LOG_FILTER: &str = "error,carddemo_batch::jobs::mnttrdb2=info";

/// The `target` member of the job's JSON events.
const JOB_TARGET: &str = r#""target":"carddemo_batch::jobs::mnttrdb2""#;

/// The start of an event's `fields` object whose first member is the message.
const MESSAGE_FIELD: &str = r#""fields":{"message":""#;

/// The message of the job's event for a failed `A`, `U` or `D` statement.
const FAILED_STATEMENT: &str = r#""message":"COBTUPDT database call failed""#;

/// The paragraph of the insert, as the failed-statement event names it.
const INSERT_DB: &str = "10031-INSERT-DB";

/// The paragraph of the update, as the failed-statement event names it.
const UPDATE_DB: &str = "10032-UPDATE-DB";

/// The paragraph of the delete, as the failed-statement event names it.
const DELETE_DB: &str = "10033-DELETE-DB";

/// The cause a lock wait cut off by `lock_timeout` gives: `lock_not_available`, which the
/// audit's mapping reads as Db2 `SQLCODE` -911.
const LOCK_NOT_AVAILABLE: &str = "database error (SQLSTATE 55P03)";

/// The flag of a failed-statement event after which the job rolled back its unit of work.
const UNIT_OF_WORK_ROLLED_BACK: &str = r#""unit_of_work_rolled_back":true"#;

/// The cause a duplicate key gives in the failed-statement event: the primary key `xtran_type`.
const DUPLICATE_KEY: &str = "duplicate key (constraint xtran_type)";

/// The cause a restricted delete gives: the foreign key `trc_type_code` of `TRNTYCAT.ddl:6-7`.
const RESTRICTED: &str = "restricted by foreign key (constraint trc_type_code)";

/// A database URL nothing listens on: a run that reached the database would fail to connect.
const UNREACHABLE_DATABASE: &str = "postgres://127.0.0.1:1/unreachable";

/// Text every derived description starts with; no job output may show it (COBTUPDT-004,
/// COBTUPDT-006).
const DESCRIPTION_MARK: &str = "AUDIT TEST";

/// A scratch directory of one test under Cargo's target directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    /// A new, empty directory for the test `name`, unique within the process.
    fn new(name: &str) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "mnttrdb2-{name}-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // A directory of the same name can only be left by an earlier process with this ID.
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `records`, each an action byte, a code and a description built by
    /// [`input_record`], to `<scratch>/mnttrdb2.input` and returns its absolute path.
    fn write_input(&self, records: &[(u8, &str, &str)]) -> io::Result<PathBuf> {
        let path = self.0.join("mnttrdb2.input");
        let bytes: Vec<u8> = records
            .iter()
            .flat_map(|&(kind, code, desc)| input_record(kind, code, desc))
            .collect();
        fs::write(&path, bytes)?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a directory left behind is inside the target directory and harmless.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One input record: the action byte, `code` space-padded to 2 bytes and `desc` space-padded to
/// 50 bytes, 53 bytes in all (COBTUPDT-002), then LF.
///
/// # Panics
///
/// When `code` or `desc` is longer than its field, or either is not ASCII.
fn input_record(kind: u8, code: &str, desc: &str) -> Vec<u8> {
    assert!(
        code.is_ascii() && desc.is_ascii() && code.len() <= CODE_LEN && desc.len() <= DESC_LEN,
        "a derived record field does not fit its picture"
    );
    let mut record = Vec::with_capacity(RECORD_LEN + 1);
    record.push(kind);
    record.extend_from_slice(format!("{code:<CODE_LEN$}{desc:<DESC_LEN$}").as_bytes());
    assert_eq!(record.len(), RECORD_LEN, "record width");
    record.push(b'\n');
    record
}

/// `text` space-padded to the 50 bytes COBTUPDT binds as `INPUT-REC-DESC` (COBTUPDT-010,
/// COBTUPDT-012).
fn padded(text: &str) -> String {
    format!("{text:<DESC_LEN$}")
}

/// The type code `text` as `DCL-TR-TYPE`.
fn code_of(text: &str) -> Result<PicX<2>, Box<dyn Error>> {
    Ok(PicX::from_text(text)?)
}

/// Reports a case a halt candidate blocks; nothing is asserted for it (AAP 0.8.5).
fn blocked(rule: &str, halt: &str) {
    println!("BLOCKED {rule} ({halt})");
}

/// How one `carddemo-batch` process ended.
struct Run {
    /// The exit status; `None` when a signal ended the process.
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The written paths: the standard-output lines that are no JSON tracing event.
    fn written_paths(&self) -> Vec<&str> {
        self.stdout
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('{'))
            .collect()
    }

    /// The job's events for failed statements, in emission order.
    fn failed_statements(&self) -> Vec<&str> {
        self.stdout
            .lines()
            .filter(|line| line.starts_with('{') && line.contains(FAILED_STATEMENT))
            .collect()
    }

    /// The job's JSON events on standard output, in emission order.
    fn job_events(&self) -> Vec<&str> {
        self.stdout
            .lines()
            .filter(|line| line.starts_with('{') && line.contains(JOB_TARGET))
            .collect()
    }

    /// The job log, in program order: the messages of the job's events whose `fields` hold the
    /// message alone. The failed-statement events carry further fields and `main`'s events
    /// another target, so neither is part of it.
    fn job_log(&self) -> Vec<String> {
        self.job_events()
            .into_iter()
            .filter_map(sole_message)
            .collect()
    }

    /// For each failed-statement event, the job-log lines of the events right before and right
    /// after it, which place the report in program order.
    fn around_failed_statements(&self) -> Vec<(Option<String>, Option<String>)> {
        let events = self.job_events();
        events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.contains(FAILED_STATEMENT))
            .map(|(at, _)| {
                let before = at
                    .checked_sub(1)
                    .and_then(|previous| events.get(previous))
                    .and_then(|event| sole_message(event));
                let after = events.get(at + 1).and_then(|event| sole_message(event));
                (before, after)
            })
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

/// The `PROCESSING` line of a record whose action byte and two code bytes are `head`, as the rows
/// write it: `PROCESSING   ` (13 characters), `head` and 50 `*`, 66 characters (COBTUPDT-006;
/// X-MNTTRDB2-10).
fn processing_line(head: &str) -> String {
    format!("PROCESSING   {head}{}", "*".repeat(DESC_LEN))
}

/// One line of an expected job log.
enum LogLine {
    /// A line compared byte for byte, trailing spaces included.
    Exact(String),
    /// The error text of COBTUPDT-019, compared in its 80-byte length, positions 1-49 and
    /// positions 55-80; positions 50-54 are the `SQLCODE` of COBTUPDT-020, never compared
    /// (X-HALTS-2).
    SqlError,
}

/// The job log of a run whose input was read and decoded and whose records show `records`:
/// `OPEN FILE OK` (COBTUPDT-004), then each record's `PROCESSING` line (COBTUPDT-006) and the
/// lines its row writes after it, in file order, and nothing after the last (COBTUPDT-025).
fn job_log_of(records: &[RecordLog]) -> Vec<LogLine> {
    let mut log = vec![LogLine::Exact("OPEN FILE OK".to_owned())];
    for &(head, shown) in records {
        log.push(LogLine::Exact(processing_line(head)));
        log.extend(shown.iter().map(|&line| match line {
            Shown::Line(text) => LogLine::Exact(text.to_owned()),
            Shown::Message(text) => LogLine::Exact(format!("{text:<RETURN_MSG_LEN$}")),
            Shown::SqlError => LogLine::SqlError,
        }));
    }
    log
}

/// Asserts that `run`'s job log is `expected`, line by line, with every trailing space.
fn assert_job_log(run: &Run, expected: &[LogLine], rule: &str) {
    let log = run.job_log();
    assert_eq!(log.len(), expected.len(), "{rule}: job log {log:?}");
    for (n, (line, wanted)) in (1..).zip(log.iter().zip(expected)) {
        match wanted {
            LogLine::Exact(text) => assert_eq!(line, text, "{rule}: job-log line {n}"),
            LogLine::SqlError => {
                let bytes = line.as_bytes();
                assert_eq!(
                    bytes.len(),
                    RETURN_MSG_LEN,
                    "{rule}: job-log line {n} {line:?}"
                );
                assert_eq!(
                    bytes.get(..SQL_ERROR_HEAD.len()),
                    Some(SQL_ERROR_HEAD.as_bytes()),
                    "{rule}: job-log line {n} {line:?}"
                );
                assert_eq!(
                    bytes.get(SQL_ERROR_TAIL_AT..),
                    Some([b' '; RETURN_MSG_LEN - SQL_ERROR_TAIL_AT].as_slice()),
                    "{rule}: job-log line {n} {line:?}"
                );
            }
        }
    }
}

/// Asserts that each failed-statement event of `run`, in order, comes right after the action
/// line `action_lines` names and right before the error text of COBTUPDT-019, the record's
/// result line, so the report keeps the program order of the job log.
fn assert_failures_in_program_order(run: &Run, action_lines: &[&str], rule: &str) {
    let around = run.around_failed_statements();
    assert_eq!(around.len(), action_lines.len(), "{rule}: {around:?}");
    for ((before, after), action) in around.iter().zip(action_lines) {
        assert_eq!(
            before.as_deref(),
            Some(*action),
            "{rule}: line before a failure"
        );
        assert!(
            after
                .as_deref()
                .is_some_and(|line| line.starts_with(SQL_ERROR_HEAD)),
            "{rule}: line after a failure {after:?}"
        );
    }
}

/// Runs `carddemo-batch` with `args` from `modern/`, against `database_url`, with every output
/// directory inside `scratch`, through `test_support::run_child`, which kills a run still going
/// at `test_support::CHILD_DEADLINE`.
async fn run_batch(
    database_url: &str,
    scratch: &Scratch,
    args: Vec<OsString>,
) -> Result<Run, Box<dyn Error>> {
    let var_dir = scratch.path().join("var");
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .args(args)
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", database_url)
        .env("CARDDEMO_STATEMENT_DIR", var_dir.join("statements"))
        .env("CARDDEMO_INTRDR_DIR", var_dir.join("jobs"))
        .env("CARDDEMO_VAR_DIR", var_dir)
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

/// Runs `carddemo-batch mnttrdb2 --input <input>` against `database_url`.
async fn run_mnttrdb2(
    database_url: &str,
    scratch: &Scratch,
    input: &Path,
) -> Result<Run, Box<dyn Error>> {
    let args = vec!["mnttrdb2".into(), "--input".into(), input.into()];
    run_batch(database_url, scratch, args).await
}

/// Asserts that `run` printed no path and showed no description byte anywhere.
fn assert_quiet(run: &Run, rule: &str) {
    assert!(
        run.written_paths().is_empty(),
        "{rule}: path lines {:?}",
        run.written_paths()
    );
    assert!(
        !run.stdout.contains(DESCRIPTION_MARK) && !run.stderr.contains(DESCRIPTION_MARK),
        "{rule}: a description reached the job's output"
    );
}

/// Asserts that `run` reported exactly the failed statements `expected`, in order, each as its
/// paragraph and its cause.
fn assert_failed_statements(run: &Run, expected: &[(&str, &str)], rule: &str) {
    let events = run.failed_statements();
    assert_eq!(
        events.len(),
        expected.len(),
        "{rule}: failed-statement events {events:?}"
    );
    for (event, (operation, cause)) in events.iter().zip(expected) {
        assert!(
            event.contains(&format!(r#""operation":"{operation}""#)) && event.contains(cause),
            "{rule}: expected {operation} failing with {cause}, got {event}"
        );
    }
}

/// The two data versions of the transaction types and categories, as one run may leave them.
#[derive(Debug, PartialEq)]
struct Snapshot {
    /// Every row of `db2.transaction_type`, in code order.
    db2_types: Vec<Db2TransactionType>,
    /// The categories of every seeded type, in key order. No run adds a category, so no other
    /// type can have one.
    db2_categories: Vec<Db2TransactionCategory>,
    /// `transaction_types`, the VSAM data version.
    vsam_types: Vec<TransactionTypeRecord>,
    /// `transaction_categories`, the VSAM data version.
    vsam_categories: Vec<TransactionCategoryRecord>,
}

impl Snapshot {
    async fn read(store: &mut PgStore) -> Result<Self, Box<dyn Error>> {
        let db2_types =
            Db2TransactionTypeRepository::page_after(store, &PicX::spaces(), None, None, ALL_ROWS)
                .await?;
        let mut db2_categories = Vec::new();
        for (code, _) in SEED_TYPES {
            let code = code_of(code)?;
            db2_categories.extend(
                Db2TransactionTypeCategoryRepository::find_all_by_type(store, &code).await?,
            );
        }
        Ok(Self {
            db2_types,
            db2_categories,
            vsam_types: TransactionTypeRepository::find_all(store).await?,
            vsam_categories: TransactionCategoryRepository::find_all(store).await?,
        })
    }

    /// The VSAM data version, which no maintenance run may change (E23).
    fn vsam(&self) -> (&[TransactionTypeRecord], &[TransactionCategoryRecord]) {
        (&self.vsam_types, &self.vsam_categories)
    }
}

/// The seeded `db2.transaction_type` rows with `description_01` as the description of `01`.
fn seeded_types_with(description_01: &str) -> Result<Vec<Db2TransactionType>, Box<dyn Error>> {
    SEED_TYPES
        .iter()
        .map(|&(code, seeded)| {
            let description = if code == "01" { description_01 } else { seeded };
            Ok(Db2TransactionType::new(
                code_of(code)?,
                description.to_owned(),
            )?)
        })
        .collect()
}

/// Asserts that the Db2 data version holds exactly migration 0006's seed, the state every
/// derived run starts from.
fn assert_seeded(before: &Snapshot) -> TestResult {
    assert_eq!(
        before.db2_types,
        seeded_types_with("PURCHASE")?,
        "Db2 seed types"
    );
    assert_eq!(
        before.db2_categories.len(),
        SEED_CATEGORY_COUNT,
        "Db2 seed categories"
    );
    Ok(())
}

/// Asserts that the fixture loads filled the VSAM data version, so that comparing it after a run
/// compares real rows: the seven `trantype.txt` types, `01` described `Purchase`
/// (`trantype.txt:1`), and the 18 `trancatg.txt` categories.
fn assert_vsam_loaded(before: &Snapshot) {
    let first = before.vsam_types.first();
    assert_eq!(before.vsam_types.len(), 7, "VSAM types loaded");
    assert_eq!(
        first.map(|row| (row.tran_type.to_text(), row.desc.to_text())),
        Some(("01".to_owned(), "Purchase".to_owned())),
        "VSAM type 01"
    );
    assert_eq!(before.vsam_categories.len(), 18, "VSAM categories loaded");
}

/// Asserts that row `code` exists and holds exactly `description`, its stored length included.
async fn assert_row(store: &mut PgStore, code: &str, description: &str, rule: &str) -> TestResult {
    let key = code_of(code)?;
    let expected = Db2TransactionType::new(key.clone(), description.to_owned())?;
    let row = Db2TransactionTypeRepository::find_by_id(store, &key).await?;
    assert_eq!(row, Some(expected), "{rule}: row {code:?}");
    Ok(())
}

/// Asserts that no row has `code`.
async fn assert_no_row(store: &mut PgStore, code: &str, rule: &str) -> TestResult {
    let row = Db2TransactionTypeRepository::find_by_id(store, &code_of(code)?).await?;
    assert_eq!(row, None, "{rule}: row {code:?}");
    Ok(())
}

/// Asserts that a restricted delete left type `code` with `description` and exactly the
/// categories `categories`, as category code and text.
async fn assert_restricted(
    store: &mut PgStore,
    (code, description): (&str, &str),
    categories: &[(&str, &str)],
    rule: &str,
) -> TestResult {
    assert_row(store, code, description, rule).await?;
    let kept: Vec<(String, String)> =
        Db2TransactionTypeCategoryRepository::find_all_by_type(store, &code_of(code)?)
            .await?
            .iter()
            .map(|category| {
                (
                    category.type_category().to_text(),
                    category.cat_data().to_owned(),
                )
            })
            .collect();
    let expected: Vec<(String, String)> = categories
        .iter()
        .map(|&(category, text)| (category.to_owned(), text.to_owned()))
        .collect();
    assert_eq!(kept, expected, "{rule}: categories of {code:?}");
    Ok(())
}

/// Asserts what each record of the derived run left in `db2.transaction_type`, in file order.
async fn assert_derived_records(store: &mut PgStore) -> TestResult {
    let renamed = padded(UPDATE_01_103.2);
    // audit: COBTUPDT-101
    // audit: COBTUPDT-008
    assert_no_row(store, COMMENT_101.1, "COBTUPDT-101").await?;
    // Record 102's row is deleted again by record 105; `all_valid_records_exit_zero` observes
    // the row the insert writes.
    // audit: COBTUPDT-103
    // audit: COBTUPDT-012
    assert_row(store, UPDATE_01_103.1, &renamed, "COBTUPDT-103").await?;
    // audit: COBTUPDT-104
    // audit: COBTUPDT-013
    assert_no_row(store, UPDATE_99_104.1, "COBTUPDT-104").await?;
    // audit: COBTUPDT-105
    // audit: COBTUPDT-015
    assert_no_row(store, DELETE_08_105.1, "COBTUPDT-105").await?;
    // audit: COBTUPDT-106
    // audit: COBTUPDT-016
    assert_no_row(store, DELETE_99_106.1, "COBTUPDT-106").await?;
    // audit: COBTUPDT-107
    // audit: COBTUPDT-009
    assert_row(store, INVALID_01_107.1, &renamed, "COBTUPDT-107").await?;
    // audit: COBTUPDT-108
    assert_no_row(store, UPDATE_98_108.1, "COBTUPDT-108").await?;
    // audit: COBTUPDT-109
    // audit: COBTUPDT-011
    assert_row(store, ADD_01_109.1, &renamed, "COBTUPDT-109").await?;
    // audit: COBTUPDT-110
    // audit: COBTUPDT-017
    assert_restricted(
        store,
        (DELETE_02_110.1, "PAYMENT"),
        &PAYMENT_CATEGORIES,
        "COBTUPDT-110",
    )
    .await
}

/// COBTUPDT-101 to COBTUPDT-111: the ten derived records in one run, on a database holding the
/// Db2 seed and the loaded VSAM data version.
#[test]
fn derived_record_set_applies_in_order() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("derived")?;
        let before = Snapshot::read(&mut store).await?;
        assert_seeded(&before)?;
        assert_vsam_loaded(&before);

        let input = scratch.write_input(&DERIVED_RUN)?;
        let run = run_mnttrdb2(guard.database_url(), &scratch, &input).await?;

        // audit: COBTUPDT-111
        // audit: COBTUPDT-022
        // audit: COBTUPDT-023
        assert_eq!(run.status, Some(4), "COBTUPDT-111: stderr {}", run.stderr);
        // audit: COBTUPDT-025
        // audit: COBTUPDT-006
        assert_quiet(&run, "COBTUPDT-111");
        // Only records 109 and 110 fail a statement; record 110 is the last, so its event shows
        // that the run went on after the failures of records 104 and 106 to 109.
        // audit: COBTUPDT-019
        assert_failed_statements(
            &run,
            &[(INSERT_DB, DUPLICATE_KEY), (DELETE_DB, RESTRICTED)],
            "COBTUPDT-111",
        );
        assert_failures_in_program_order(
            &run,
            &["ADDING RECORD", "DELETING RECORD"],
            "COBTUPDT-111",
        );
        // The job log: `OPEN FILE OK`, then the lines of COBTUPDT-101 to COBTUPDT-110 in file
        // order, the message overlay of records 104 and 106 to 110 included, and nothing after
        // line 29. Lines 26 and 29 are compared but for their five `SQLCODE` positions.
        // audit: COBTUPDT-111
        // audit: COBTUPDT-004
        // audit: COBTUPDT-006
        // audit: COBTUPDT-007
        // audit: COBTUPDT-008
        // audit: COBTUPDT-009
        // audit: COBTUPDT-010
        // audit: COBTUPDT-011
        // audit: COBTUPDT-012
        // audit: COBTUPDT-013
        // audit: COBTUPDT-015
        // audit: COBTUPDT-016
        // audit: COBTUPDT-017
        // audit: COBTUPDT-019
        // audit: COBTUPDT-021
        // audit: COBTUPDT-022
        // audit: COBTUPDT-025
        // audit: COBTUPDT-101
        // audit: COBTUPDT-102
        // audit: COBTUPDT-103
        // audit: COBTUPDT-104
        // audit: COBTUPDT-105
        // audit: COBTUPDT-106
        // audit: COBTUPDT-107
        // audit: COBTUPDT-108
        // audit: COBTUPDT-109
        // audit: COBTUPDT-110
        assert_eq!(run.job_log().len(), 29, "COBTUPDT-111: 29 job-log lines");
        assert_job_log(&run, &job_log_of(&DERIVED_LOG), "COBTUPDT-111");
        // audit: COBTUPDT-020
        blocked("COBTUPDT-020", "X-HALTS-2");

        assert_derived_records(&mut store).await?;
        let after = Snapshot::read(&mut store).await?;
        // audit: COBTUPDT-024
        assert_eq!(
            after.db2_types,
            seeded_types_with(&padded(UPDATE_01_103.2))?,
            "COBTUPDT-111: db2.transaction_type"
        );
        assert_eq!(
            after.db2_categories, before.db2_categories,
            "COBTUPDT-111: db2.transaction_type_category"
        );
        // audit: COBTUPDT-026
        assert_eq!(
            after.vsam(),
            before.vsam(),
            "COBTUPDT-026: VSAM data version"
        );
        Ok(())
    })
}

/// COBTUPDT-112: the derived records that reach no `9999-ABEND`, on a fresh database, then
/// record 102 alone on the same database, where row `08` is absent again, which shows the row
/// the insert writes.
#[test]
fn all_valid_records_exit_zero() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("valid")?;
        let before = Snapshot::read(&mut store).await?;
        assert_seeded(&before)?;
        assert_vsam_loaded(&before);

        let input = scratch.write_input(&VALID_RUN)?;
        let run = run_mnttrdb2(guard.database_url(), &scratch, &input).await?;
        // Exit 0 also shows that record 105 found the row record 102 inserted (COBTUPDT-016).
        // audit: COBTUPDT-112
        // audit: COBTUPDT-023
        assert_eq!(run.status, Some(0), "COBTUPDT-112: stderr {}", run.stderr);
        assert_quiet(&run, "COBTUPDT-112");
        assert_failed_statements(&run, &[], "COBTUPDT-112");
        // `OPEN FILE OK` and the 2, 3, 3 and 3 lines of the four records; `WS-RETURN-MSG` is
        // never displayed.
        // audit: COBTUPDT-112
        // audit: COBTUPDT-004
        // audit: COBTUPDT-006
        // audit: COBTUPDT-025
        assert_eq!(run.job_log().len(), 12, "COBTUPDT-112: 12 job-log lines");
        assert_job_log(&run, &job_log_of(&VALID_LOG), "COBTUPDT-112");
        let after = Snapshot::read(&mut store).await?;
        // audit: COBTUPDT-112
        assert_eq!(
            after.db2_types,
            seeded_types_with(&padded(UPDATE_01_103.2))?,
            "COBTUPDT-112: db2.transaction_type"
        );
        assert_eq!(
            after.db2_categories, before.db2_categories,
            "COBTUPDT-112: db2.transaction_type_category"
        );
        // audit: COBTUPDT-026
        assert_eq!(
            after.vsam(),
            before.vsam(),
            "COBTUPDT-026: VSAM data version"
        );

        let input = scratch.write_input(&[ADD_08_102])?;
        let run = run_mnttrdb2(guard.database_url(), &scratch, &input).await?;
        assert_eq!(run.status, Some(0), "COBTUPDT-102: stderr {}", run.stderr);
        assert_quiet(&run, "COBTUPDT-102");
        // audit: COBTUPDT-102
        // audit: COBTUPDT-007
        assert_eq!(run.job_log().len(), 4, "COBTUPDT-102: 4 job-log lines");
        assert_job_log(&run, &job_log_of(&[LOG_102]), "COBTUPDT-102");
        // audit: COBTUPDT-102
        // audit: COBTUPDT-010
        assert_row(
            &mut store,
            ADD_08_102.1,
            &padded(ADD_08_102.2),
            "COBTUPDT-102",
        )
        .await?;
        let categories =
            Db2TransactionTypeCategoryRepository::find_all_by_type(&mut store, &code_of("08")?)
                .await?;
        assert!(categories.is_empty(), "COBTUPDT-102: categories of 08");
        Ok(())
    })
}

/// Second lines that are no COBTUPDT record, each after the valid record 102: a body of 52 or
/// 54 bytes, a CR inside the body, a control byte and a byte above `0x7E` (COBTUPDT-003). Each
/// is built from record 103, so a record that slipped through would change row `01`.
fn malformed_lines() -> Vec<(&'static str, Vec<u8>)> {
    let (kind, code, desc) = UPDATE_01_103;
    let body = || {
        let mut record = input_record(kind, code, desc);
        record.pop();
        record
    };
    let replaced = |byte: u8| {
        let mut record = body();
        if let Some(slot) = record.get_mut(20) {
            *slot = byte;
        }
        record
    };
    let mut short = body();
    short.pop();
    let mut long = body();
    long.push(b'X');
    let cases = [
        ("short", short),
        ("long", long),
        ("carriage-return", replaced(b'\r')),
        ("control-byte", replaced(b'\t')),
        ("above-0x7e", replaced(0xE9)),
    ];
    cases
        .into_iter()
        .map(|(name, mut line)| {
            line.push(b'\n');
            (name, line)
        })
        .collect()
}

/// COBTUPDT-003, COBTUPDT-004 and COBTUPDT-113: an input that cannot be read, or holds a line
/// that is no record, fails the run before any record is processed, with a status other than 0
/// and 4; an empty input changes nothing and ends with 0.
#[test]
fn refused_or_empty_input_changes_nothing() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("refused")?;
        let before = Snapshot::read(&mut store).await?;
        assert_seeded(&before)?;

        let mut cases = vec![("absent", scratch.path().join("absent.input"))];
        for (name, line) in malformed_lines() {
            let path = scratch.path().join(format!("{name}.input"));
            let mut bytes = input_record(ADD_08_102.0, ADD_08_102.1, ADD_08_102.2);
            bytes.extend(line);
            fs::write(&path, bytes)?;
            cases.push((name, path));
        }
        for (case, input) in cases {
            let run = run_mnttrdb2(guard.database_url(), &scratch, &input).await?;
            // audit: COBTUPDT-004
            assert!(
                run.status.is_some_and(|status| status != 0 && status != 4),
                "COBTUPDT-004 ({case}): exit status {:?}",
                run.status
            );
            assert!(
                run.stderr.starts_with("carddemo-batch mnttrdb2: "),
                "COBTUPDT-004 ({case}): stderr {}",
                run.stderr
            );
            assert_quiet(&run, "COBTUPDT-004");
            // The job log is `OPEN FILE NOT OK` and nothing else.
            // audit: COBTUPDT-004
            assert_eq!(
                run.job_log(),
                ["OPEN FILE NOT OK"],
                "COBTUPDT-004 ({case}): job log"
            );
            // audit: COBTUPDT-003
            assert_eq!(
                Snapshot::read(&mut store).await?,
                before,
                "COBTUPDT-003 ({case}): no record may be processed"
            );
        }
        // audit: COBTUPDT-005
        blocked("COBTUPDT-005", "X-MNTTRDB2-H1");

        let input = scratch.write_input(&[])?;
        let run = run_mnttrdb2(guard.database_url(), &scratch, &input).await?;
        // audit: COBTUPDT-113
        // audit: COBTUPDT-025
        assert_eq!(run.status, Some(0), "COBTUPDT-113: stderr {}", run.stderr);
        assert_quiet(&run, "COBTUPDT-113");
        // audit: COBTUPDT-113
        // audit: COBTUPDT-004
        // audit: COBTUPDT-025
        assert_eq!(run.job_log(), ["OPEN FILE OK"], "COBTUPDT-113: job log");
        assert_eq!(Snapshot::read(&mut store).await?, before, "COBTUPDT-113");
        Ok(())
    })
}

/// `database_url` with every session giving up a lock wait after one second, so that a row the
/// test holds locked fails the job's statement with `lock_not_available` (`55P03`).
fn with_lock_timeout(database_url: &str) -> String {
    format!("{database_url}?options[lock_timeout]=1000")
}

/// Opens a transaction that holds the row lock of the seeded type `code`, by rewriting its seed
/// description unchanged; rolling it back releases the lock.
async fn hold_row(
    store: &PgStore,
    (code, seeded): (&str, &str),
) -> Result<PgTx<'static>, Box<dyn Error>> {
    let mut tx = store.begin().await?;
    let row = Db2TransactionType::new(code_of(code)?, seeded.to_owned())?;
    let locked = Db2TransactionTypeRepository::update(&mut tx, &row).await?;
    assert!(locked, "row {code:?} to lock");
    Ok(tx)
}

/// Runs `records` while the test holds the row lock of type `01`, which record 103 updates, and
/// asserts that the update was the run's one failed statement, a lock timeout after which the
/// job rolled back its unit of work, with return code 4, and that the job log is `log`'s, the
/// update's result line the error text of COBTUPDT-019.
async fn run_with_01_locked(
    store: &PgStore,
    scratch: &Scratch,
    database_url: &str,
    records: &[Record],
    log: &[RecordLog],
) -> TestResult {
    let lock = hold_row(store, SEED_TYPES[0]).await?;
    let input = scratch.write_input(records)?;
    let run = run_mnttrdb2(&with_lock_timeout(database_url), scratch, &input).await;
    lock.rollback().await?;
    let run = run?;
    // audit: COBTUPDT-022
    assert_eq!(run.status, Some(4), "COBTUPDT-014: stderr {}", run.stderr);
    assert_quiet(&run, "COBTUPDT-014");
    assert_failed_statements(&run, &[(UPDATE_DB, LOCK_NOT_AVAILABLE)], "COBTUPDT-024");
    assert!(
        run.failed_statements()
            .iter()
            .all(|event| event.contains(UNIT_OF_WORK_ROLLED_BACK)),
        "COBTUPDT-024: the unit of work must be rolled back"
    );
    assert_failures_in_program_order(&run, &["UPDATING RECORD"], "COBTUPDT-014");
    // audit: COBTUPDT-014
    // audit: COBTUPDT-019
    assert_job_log(&run, &job_log_of(log), "COBTUPDT-014");
    // audit: COBTUPDT-020
    blocked("COBTUPDT-020", "X-HALTS-2");
    Ok(())
}

/// COBTUPDT-014 and the `SQLCODE` -911 case of COBTUPDT-024, with records 102 and 103: an update
/// that fails on a lock timeout keeps the row's description, rolls back every earlier change of
/// the run, and the later records form a new unit of work, committed at the end.
///
/// Row COBTUPDT-014 names a repository double for this failure. The job runs here as a separate
/// process, which no double reaches, so the test holds the row's lock instead and the job's
/// `lock_timeout` ends its wait with the same SQLSTATE, `55P03`, on every run.
#[test]
fn a_lock_timeout_rolls_back_the_unit_of_work() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("lock")?;
        let before = Snapshot::read(&mut store).await?;
        assert_seeded(&before)?;
        let (code_01, seeded_01) = SEED_TYPES[0];

        // The insert of record 102 comes before the failed update, in the unit of work it undoes.
        run_with_01_locked(
            &store,
            &scratch,
            guard.database_url(),
            &[ADD_08_102, UPDATE_01_103],
            &[LOG_102, LOG_103_FAILED],
        )
        .await?;
        // audit: COBTUPDT-024
        assert_no_row(&mut store, ADD_08_102.1, "COBTUPDT-024: earlier insert").await?;
        // audit: COBTUPDT-014
        assert_row(&mut store, code_01, seeded_01, "COBTUPDT-014").await?;

        // The same insert after the failed update falls in the new unit of work, committed.
        run_with_01_locked(
            &store,
            &scratch,
            guard.database_url(),
            &[UPDATE_01_103, ADD_08_102],
            &[LOG_103_FAILED, LOG_102],
        )
        .await?;
        // audit: COBTUPDT-024
        assert_row(
            &mut store,
            ADD_08_102.1,
            &padded(ADD_08_102.2),
            "COBTUPDT-024: later insert",
        )
        .await?;
        // audit: COBTUPDT-014
        assert_row(&mut store, code_01, seeded_01, "COBTUPDT-014").await?;
        assert_eq!(
            Snapshot::read(&mut store).await?.db2_categories,
            before.db2_categories,
            "COBTUPDT-014: db2.transaction_type_category"
        );
        Ok(())
    })
}

/// Values Cardholder Data Redaction keeps out of every output: the card number of `cardxref.txt`
/// line 1, its first 16 bytes (`XREF-CARD-NUM`), read in place, then an SSN with and without
/// dashes and a password, assembled from parts at run time, so that no source line holds one.
/// Joined by single spaces they fill the 50 bytes of `INPUT-REC-DESC`.
fn protected_values() -> Result<[String; 4], Box<dyn Error>> {
    let file = fs::read(fixture_path("cardxref.txt"))?;
    let card = file.get(..16).ok_or("cardxref.txt holds a first record")?;
    let ssn = ["987", "65", "4321"];
    Ok([
        String::from_utf8(card.to_vec())?,
        ssn.join("-"),
        ssn.concat(),
        ["Tr0ub4", "dor&3"].concat(),
    ])
}

/// The records of the redaction run, each with the protected description: an `A`, a `U`, a
/// `D`, a `*` comment and an invalid action byte, and the row that says the record's statement
/// binds all 50 description bytes, for the two that bind them.
const REDACTION_RECORDS: [(u8, &str, Option<&str>); 5] = [
    (b'A', "08", Some("COBTUPDT-010")),
    (b'U', "01", Some("COBTUPDT-012")),
    (b'D', "02", None),
    (b'*', "", None),
    (b'X', "01", None),
];

/// COBTUPDT-006 at the job level, on a database of its own: a description holding a card number,
/// an SSN with and without dashes and a password reaches neither standard output nor standard
/// error for any of [`REDACTION_RECORDS`]; every `PROCESSING` line shows the action byte, the
/// code and 50 `*`; and the rows the insert and the update wrote hold all 50 description bytes.
/// No assertion message shows a value.
#[test]
fn protected_descriptions_reach_the_rows_but_no_output() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new("redaction")?;
        let values = protected_values()?;
        let description = values.join(" ");
        assert_eq!(
            description.len(),
            DESC_LEN,
            "the values fill INPUT-REC-DESC"
        );
        let records: Vec<(u8, &str, &str)> = REDACTION_RECORDS
            .iter()
            .map(|&(kind, code, _)| (kind, code, description.as_str()))
            .collect();
        let input = scratch.write_input(&records)?;
        let run = run_mnttrdb2(guard.database_url(), &scratch, &input).await?;
        // The restricted delete and the invalid action byte reach `9999-ABEND`.
        assert_eq!(run.status, Some(4), "COBTUPDT-023: exit status");

        // audit: COBTUPDT-006
        assert!(
            values.iter().all(|value| {
                !run.stdout.contains(value.as_str()) && !run.stderr.contains(value.as_str())
            }),
            "COBTUPDT-006: a protected value reached standard output or standard error"
        );
        let processing: Vec<String> = run
            .job_log()
            .into_iter()
            .filter(|line| line.starts_with("PROCESSING"))
            .collect();
        assert_eq!(
            processing.len(),
            REDACTION_RECORDS.len(),
            "COBTUPDT-006: one PROCESSING line per record"
        );
        for (n, (line, &(kind, code, _))) in (1..).zip(processing.iter().zip(&REDACTION_RECORDS)) {
            let head = format!("{}{code:<CODE_LEN$}", char::from(kind));
            assert!(
                *line == processing_line(&head),
                "COBTUPDT-006: the PROCESSING line of record {n} must show its head and 50 `*`"
            );
        }
        // audit: COBTUPDT-010
        // audit: COBTUPDT-012
        for (code, rule) in REDACTION_RECORDS
            .iter()
            .filter_map(|&(_, code, rule)| Some((code, rule?)))
        {
            let row = Db2TransactionTypeRepository::find_by_id(&mut store, &code_of(code)?).await?;
            assert!(
                row.is_some_and(|row| row.tr_description() == description),
                "{rule}: row {code} must hold all 50 description bytes"
            );
        }
        Ok(())
    })
}

/// COBTUPDT-001: `--input` has no default, so `mnttrdb2` without it is a usage error, reported
/// before the environment or the database is reached: the database URL here leads nowhere, and
/// a connection attempt would end with status 1, not 2.
#[test]
fn mnttrdb2_without_input_is_a_usage_error() -> TestResult {
    block_on(async {
        let scratch = Scratch::new("usage")?;
        let run = run_batch(UNREACHABLE_DATABASE, &scratch, vec!["mnttrdb2".into()]).await?;
        // audit: COBTUPDT-001
        assert_eq!(run.status, Some(2), "COBTUPDT-001: stderr {}", run.stderr);
        assert!(run.stdout.is_empty(), "COBTUPDT-001: stdout {}", run.stdout);
        assert!(
            run.stderr.contains("--input"),
            "COBTUPDT-001: stderr {}",
            run.stderr
        );
        Ok(())
    })
}
