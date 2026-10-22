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
//! - The standard-output lines that do not begin with `{`, the paths the job wrote: none, as
//!   MNTTRDB2 writes no file (AAP 0.3.4.3).
//! - The JSON `ERROR` event the job emits for each statement that failed, which names the
//!   paragraph and the cause, so a run shows that it reached its last record after earlier
//!   failures.
//!
//! The job log is not observed: `batch::jobs::mnttrdb2` discards COBTUPDT's `DISPLAY` lines,
//! because the batch standard-output contract carries only written paths, and the domain tests
//! prove those lines. The five `SQLCODE` positions of the error text are blocked by the halt
//! candidate `X-HALTS-2`, proposed to join H3, and an unreadable input's COBOL continuation by
//! `X-MNTTRDB2-H1`; both are reported `BLOCKED` and never asserted (AAP 0.8.5).
//!
//! # Environment
//!
//! Each run gets `DATABASE_URL` from its own container, `CARDDEMO_VAR_DIR` and the statement and
//! job-deck directories inside a scratch directory under Cargo's target directory,
//! `CARDDEMO_FIXTURE_DIR` from `test_support`, `RUST_LOG=error` and no span export. Every other
//! variable is inherited, never cleared, so coverage instrumentation reaches the binary. The
//! schema and the Db2 seed come from the migrations `test_support` applies when it starts the
//! container; nothing here touches the schema.

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
use test_support::fixtures::{fixture_dir, start_loaded_postgres};
use test_support::postgres::start_postgres;

type TestResult = Result<(), Box<dyn Error>>;

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

    /// Writes `records`, each built by [`input_record`], to `<scratch>/mnttrdb2.input` and
    /// returns its absolute path.
    fn write_input(&self, records: &[Record]) -> io::Result<PathBuf> {
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
}

/// Runs `carddemo-batch` with `args` from `modern/`, against `database_url`, with every output
/// directory inside `scratch`, on the blocking pool.
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
        .env("RUST_LOG", "error")
        .env_remove("CARDDEMO_OTEL_EXPORT");
    let output = tokio::task::spawn_blocking(move || command.output()).await??;
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
#[tokio::test(flavor = "multi_thread")]
async fn derived_record_set_applies_in_order() -> TestResult {
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
    // Only records 109 and 110 fail a statement; record 110 is the last, so its event shows that
    // the run went on after the failures of records 104 and 106 to 109.
    // audit: COBTUPDT-019
    assert_failed_statements(
        &run,
        &[(INSERT_DB, DUPLICATE_KEY), (DELETE_DB, RESTRICTED)],
        "COBTUPDT-111",
    );
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
}

/// COBTUPDT-112: the derived records that reach no `9999-ABEND`, on a fresh database, then
/// record 102 alone on the same database, where row `08` is absent again, which shows the row
/// the insert writes.
#[tokio::test(flavor = "multi_thread")]
async fn all_valid_records_exit_zero() -> TestResult {
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
    // audit: COBTUPDT-010
    assert_row(
        &mut store,
        ADD_08_102.1,
        &padded(ADD_08_102.2),
        "COBTUPDT-102",
    )
    .await?;
    let categories =
        Db2TransactionTypeCategoryRepository::find_all_by_type(&mut store, &code_of("08")?).await?;
    assert!(categories.is_empty(), "COBTUPDT-102: categories of 08");
    Ok(())
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
#[tokio::test(flavor = "multi_thread")]
async fn refused_or_empty_input_changes_nothing() -> TestResult {
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
    assert_eq!(Snapshot::read(&mut store).await?, before, "COBTUPDT-113");
    Ok(())
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
/// job rolled back its unit of work, with return code 4.
async fn run_with_01_locked(
    store: &PgStore,
    scratch: &Scratch,
    database_url: &str,
    records: &[Record],
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
    Ok(())
}

/// COBTUPDT-014 and the `SQLCODE` -911 case of COBTUPDT-024, with records 102 and 103: an update
/// that fails on a lock timeout keeps the row's description, rolls back every earlier change of
/// the run, and the later records form a new unit of work, committed at the end.
///
/// The audit provokes these failures through a repository double. The job runs here as a
/// separate process, which no double reaches, so the test holds the row's lock instead and the
/// job's `lock_timeout` ends its wait with the same SQLSTATE, `55P03`, on every run.
#[tokio::test(flavor = "multi_thread")]
async fn a_lock_timeout_rolls_back_the_unit_of_work() -> TestResult {
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
}

/// COBTUPDT-001: `--input` has no default, so `mnttrdb2` without it is a usage error, reported
/// before the environment or the database is reached: the database URL here leads nowhere, and
/// a connection attempt would end with status 1, not 2.
#[tokio::test(flavor = "multi_thread")]
async fn mnttrdb2_without_input_is_a_usage_error() -> TestResult {
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
}
