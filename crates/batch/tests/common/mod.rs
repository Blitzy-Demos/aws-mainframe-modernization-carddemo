//! Shared harness of the `carddemo-batch` integration tests.
//!
//! Every test file of `crates/batch/tests` declares this module with `pub mod common;`. Cargo
//! builds no test target from a sub-folder of `tests/`, so each test binary compiles its own
//! copy, and the public items a binary leaves unused raise no `dead_code` warning.
//!
//! `carddemo-batch` is a binary-only package: the tests run the real `batch` binary
//! (`CARGO_BIN_EXE_batch`) as a child process, against a migrated and seeded `PostgreSQL`
//! 16.15 container from `carddemo_testkit::TestStack`, and read back the generation files it
//! writes. Only [`Harness::new`] needs Docker. The free functions [`parity`],
//! [`fixture_lines`] and [`split_records`] read repository files only, so
//! `parity_audit_coverage.rs` starts no container.
//!
//! The CT00, CT01, CT02 and CTTU helpers drive the same domain rules and persistence adapters
//! the `carddemo-api` handlers drive, because `carddemo-batch` cannot depend on that crate.
//! Each runs in one `UnitOfWork`, so a test can interleave online changes with batch runs.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use carddemo_audit::{CodeSpan, Document, KeyValue, PARITY_COMMON_COLUMNS, Repo, Row, Table};
use carddemo_domain::entities::Transaction;
use carddemo_domain::error::DomainError;
use carddemo_domain::numeric::TranId;
use carddemo_domain::ports::{Direction, TransactionWriter};
use carddemo_domain::rewards::{AccrualInput, RewardEngine};
use carddemo_domain::rules::Confirm;
use carddemo_domain::rules::tran_type_update::{
    self, TranTypeEdit, TranTypeSaved, TranTypeUpsertInput,
};
use carddemo_domain::rules::transaction_add::{self, TransactionAddInput};
use carddemo_domain::rules::transaction_list::{self, TransactionListInput, TransactionListPage};
use carddemo_domain::rules::transaction_view::{self, TransactionViewInput};
use carddemo_persistence::{Db, UnitOfWork};
use carddemo_runtime::config::keys;
use carddemo_testkit::{TestStack, fixtures};
use chrono::{DateTime, Utc};

pub use carddemo_testkit::block_on;

/// The processing clock the TRANREPT expectations are derived under (AAP 0.9.3): POSTTRAN
/// stamps every posted `TRAN-PROC-TS` with it, so each record falls inside the default
/// report range `2022-01-01` to `2022-07-06` [app/jcl/TRANREPT.jcl:L43-44]. Pass it as the
/// `clock` of [`Harness::run`], which hands it to the binary as `CARDDEMO_CLOCK_OVERRIDE`.
pub const TRANREPT_CLOCK: &str = "2022-07-01T12:00:00Z";

/// The columns of each row [`Harness::live_master_rows`] returns, in order: the view
/// `transaction_master` of migration `0003`, the row identity followed by
/// the 13 `CVTRA05Y` fields.
pub const LIVE_MASTER_COLUMNS: [&str; 14] = [
    "transaction_id",
    "tran_id",
    "tran_type_cd",
    "tran_cat_cd",
    "tran_source",
    "tran_desc",
    "tran_amt",
    "tran_merchant_id",
    "tran_merchant_name",
    "tran_merchant_city",
    "tran_merchant_zip",
    "tran_card_num",
    "tran_orig_ts",
    "tran_proc_ts",
];

/// The live transaction master in KSDS key order. `COLLATE "C"` is byte order, the order of
/// the VSAM key and of the TRAN-ID allocation in `crates/persistence/src/ids.rs`. Legacy reads
/// name only the view (E-09), and the select list is [`LIVE_MASTER_COLUMNS`].
const LIVE_MASTER_SQL: &str = "SELECT transaction_id, tran_id, tran_type_cd, tran_cat_cd, \
     tran_source, tran_desc, tran_amt, tran_merchant_id, tran_merchant_name, \
     tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts, tran_proc_ts \
     FROM transaction_master ORDER BY tran_id COLLATE \"C\"";

/// The `Expected result` column of every parity audit data table.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The origination and processing date [`ct02_input`] enters: `YYYY-MM-DD`, the form
/// COTRN02C checks [app/cbl/COTRN02C.cbl:L354-373], and a calendar date `CSUTLDTC` accepts
/// (P-CSUTLDTC-002). It is the date of the daily-transaction fixture records.
const CT02_DATE: &str = "2022-06-10";

/// A migrated, seeded `PostgreSQL` test database with private output directories for the
/// `batch` binary.
///
/// Dropping the harness removes the database container. The output directories are removed
/// too, unless the thread is panicking, so a failed test leaves its generation and statement
/// files behind for inspection.
pub struct Harness {
    stack: TestStack,
    batch_dir: PathBuf,
    statement_dir: PathBuf,
}

impl Harness {
    /// Starts a `PostgreSQL` 16.15 container with every workspace migration applied, loads the
    /// seeded state `batch seed` produces on an empty database (the transaction master and the
    /// reward accruals stay empty), and creates fresh directories for `CARDDEMO_BATCH_DIR` and
    /// `CARDDEMO_STATEMENT_DIR`.
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable, the container does not start or migrate, the seed
    /// fails, or an output directory cannot be created.
    #[must_use]
    pub fn new() -> Self {
        let stack = block_on(TestStack::postgres());
        if let Err(error) = block_on(stack.seed()) {
            panic!("cannot seed the batch test database: {error}");
        }
        Self {
            stack,
            batch_dir: fixtures::temp_dir("batch"),
            statement_dir: fixtures::temp_dir("statements"),
        }
    }

    /// The test stack: its pool, its database URL and its text-typed SQL helpers.
    #[must_use]
    pub fn stack(&self) -> &TestStack {
        &self.stack
    }

    /// The directory the binary writes its GDG generation files to (`CARDDEMO_BATCH_DIR`).
    #[must_use]
    pub fn batch_dir(&self) -> &Path {
        &self.batch_dir
    }

    /// The directory `batch creastmt` writes the statement files to
    /// (`CARDDEMO_STATEMENT_DIR`).
    #[must_use]
    pub fn statement_dir(&self) -> &Path {
        &self.statement_dir
    }

    /// The persistence handle on the test database, for the CT00, CT01, CT02 and CTTU
    /// helpers.
    #[must_use]
    pub fn db(&self) -> Db {
        Db::from_pool(self.stack.pool().clone())
    }

    /// Runs `batch <args>` to completion and returns its exit status and output.
    ///
    /// The child runs in the repository root, so the JCL-relative defaults such as
    /// `posttran --input app/data/ASCII/dailytran.txt` resolve. Its environment is the test
    /// process's, with `CARDDEMO_DATABASE_URL` naming the test database, `CARDDEMO_BATCH_DIR`
    /// and `CARDDEMO_STATEMENT_DIR` the harness directories, and `CARDDEMO_FIXTURE_DIR` the
    /// ASCII fixture corpus. `clock` becomes `CARDDEMO_CLOCK_OVERRIDE`, an RFC 3339 instant
    /// that pins every time-dependent output; `None` removes the variable, so the child reads
    /// the system clock.
    ///
    /// # Panics
    ///
    /// Panics if the binary cannot be spawned.
    #[must_use]
    pub fn run(&self, args: &[&str], clock: Option<&str>) -> Output {
        let program = env!("CARGO_BIN_EXE_batch");
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(fixtures::repo_root())
            .env(keys::DATABASE_URL, self.stack.database_url())
            .env(keys::BATCH_DIR, &self.batch_dir)
            .env(keys::STATEMENT_DIR, &self.statement_dir)
            .env(keys::FIXTURE_DIR, fixtures::ascii_dir());
        if let Some(instant) = clock {
            command.env(keys::CLOCK_OVERRIDE, instant);
        } else {
            command.env_remove(keys::CLOCK_OVERRIDE);
        }
        command
            .output()
            .unwrap_or_else(|error| panic!("cannot run {program} {args:?}: {error}"))
    }

    /// Runs `batch <args>` as [`Harness::run`] does and checks that it exits with `code`, the
    /// condition code of the job (0, 4, 8, 16, or a REPRO code).
    ///
    /// # Panics
    ///
    /// Panics if the binary cannot be spawned, or if it exits with another code or by a
    /// signal. The message shows the arguments, both codes, and the child's stderr and stdout.
    #[must_use]
    pub fn run_expect(&self, args: &[&str], clock: Option<&str>, code: i32) -> Output {
        let output = self.run(args, clock);
        assert_eq!(
            output.status.code(),
            Some(code),
            "batch {args:?} exited with {:?}, expected {code}\n--- stderr ---\n{}\n--- stdout ---\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
        output
    }

    /// The cataloged generations of `dataset` under the batch directory, oldest first: the
    /// files named exactly `<dataset>.G<nnnn>V00` with `nnnn` from `0001` to `9999`
    /// (AAP 0.5.3.5). Only the newest 5 are kept, as `LIMIT(5) SCRATCH` keeps them
    /// [app/jcl/DALYREJS.jcl:L26-27].
    ///
    /// The match is exact, so `TRANSACT.BKUP.G0001V00` is no generation of `TRANSACT`, and the
    /// hidden temporary files the binary writes a generation under are ignored. A missing
    /// directory holds no generations. Datasets: `DALYREJS`, `SYSTRAN`, `TRANSACT.BKUP`,
    /// `TRANSACT.COMBINED`, `TRANSACT.DALY` and `TRANREPT`.
    ///
    /// # Panics
    ///
    /// Panics if the directory or one of its entries cannot be read for a reason other than
    /// the directory not existing.
    #[must_use]
    pub fn generations(&self, dataset: &str) -> Vec<PathBuf> {
        let entries = match fs::read_dir(&self.batch_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("cannot list {}: {error}", self.batch_dir.display()),
        };
        let mut found: Vec<(u16, PathBuf)> = entries
            .filter_map(|entry| {
                let entry = entry.unwrap_or_else(|error| {
                    panic!(
                        "cannot read an entry of {}: {error}",
                        self.batch_dir.display()
                    )
                });
                let number = generation_number(dataset, entry.file_name().to_str()?)?;
                Some((number, entry.path()))
            })
            .collect();
        found.sort_unstable_by_key(|(number, _)| *number);
        found.into_iter().map(|(_, path)| path).collect()
    }

    /// The bytes of the `(0)` generation of `dataset`, the newest of [`Harness::generations`].
    ///
    /// # Panics
    ///
    /// Panics, naming the dataset and the directory, if `dataset` has no generation, or if the
    /// file cannot be read.
    #[must_use]
    pub fn newest(&self, dataset: &str) -> Vec<u8> {
        let path = self.generations(dataset).pop().unwrap_or_else(|| {
            panic!(
                "{dataset} has no generation under {}",
                self.batch_dir.display()
            )
        });
        fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    /// Every row of the live transaction master, the view `transaction_master`, in TRAN-ID
    /// byte order. Each row holds the [`LIVE_MASTER_COLUMNS`] in that order, as their
    /// `PostgreSQL` text forms: the identity and the integer columns as decimal digits,
    /// `tran_amt` as `NUMERIC(11,2)` text such as `50.47`, the texts right-trimmed as stored.
    ///
    /// # Panics
    ///
    /// Panics if the read fails or a value is not UTF-8 text.
    #[must_use]
    pub fn live_master_rows(&self) -> Vec<Vec<Option<String>>> {
        block_on(self.stack.fetch_text_rows(LIVE_MASTER_SQL))
    }
}

impl Default for Harness {
    /// [`Harness::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Harness {
    /// Removes the output directories after a passing test, and keeps them after a panic.
    /// Removal is best effort: a directory that is already gone, or one that cannot be
    /// removed, changes no test outcome. The container goes with the stack afterwards.
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        for dir in [&self.batch_dir, &self.statement_dir] {
            if let Err(error) = fs::remove_dir_all(dir) {
                if error.kind() != ErrorKind::NotFound {
                    eprintln!("cannot remove test directory {}: {error}", dir.display());
                }
            }
        }
    }
}

/// The absolute generation number of `file_name` when it is exactly
/// `<dataset>.G<nnnn>V00` with `nnnn` four ASCII digits from `0001` to `9999`, the naming of
/// `crates/batch/src/generations.rs`, which the binary keeps crate-private.
fn generation_number(dataset: &str, file_name: &str) -> Option<u16> {
    let digits = file_name
        .strip_prefix(dataset)?
        .strip_prefix(".G")?
        .strip_suffix("V00")?;
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number: u16 = digits.parse().ok()?;
    (number >= 1).then_some(number)
}

/// The parity oracle audit `docs/parity/parity-oracle-audit.md`, parsed by `carddemo_audit`.
///
/// A free function that reads one repository file, so it starts no container.
///
/// # Panics
///
/// Panics if the audit cannot be found or read.
#[must_use]
pub fn parity() -> Document {
    Repo::new(fixtures::repo_root())
        .parity_audit()
        .unwrap_or_else(|error| panic!("cannot load the parity audit: {error}"))
}

/// The data table row whose `ID` is `id`, with its table.
///
/// # Panics
///
/// Panics, naming the ID, if no data table of `doc` has such a row.
#[must_use]
pub fn parity_row<'d>(doc: &'d Document, id: &str) -> (&'d Table, &'d Row) {
    doc.find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no row {id}"))
}

/// The cell of row `id` under the header `column`, for example `"Input condition"`.
///
/// # Panics
///
/// Panics if the row does not exist, or its table has no such column or the row does not
/// reach it.
#[must_use]
pub fn parity_cell<'d>(doc: &'d Document, id: &str, column: &str) -> &'d str {
    let (table, row) = parity_row(doc, id);
    table
        .cell(row, column)
        .unwrap_or_else(|| panic!("parity audit row {id} has no {column:?} cell"))
}

/// The `key=value` pairs of the `Expected result` cell of row `id`, in order.
///
/// # Panics
///
/// As [`parity_cell`].
#[must_use]
pub fn expected_values(doc: &Document, id: &str) -> Vec<KeyValue> {
    carddemo_audit::key_values(parity_cell(doc, id, EXPECTED_RESULT))
}

/// The value of the first `key=` pair of the `Expected result` cell of row `id`, backticks
/// kept, for example `262` for `records_in_range` of P-CBTRN03C-015.
///
/// # Panics
///
/// As [`parity_cell`], and if the cell has no pair named `key`.
#[must_use]
pub fn expected_value(doc: &Document, id: &str, key: &str) -> String {
    expected_values(doc, id)
        .into_iter()
        .find(|pair| pair.key == key)
        .unwrap_or_else(|| panic!("parity audit row {id} expects no {key}="))
        .value
}

/// The exact text of the code span of the first `key=` pair of the `Expected result` cell of
/// row `id`, with its `width=<n>`, for example a byte-exact message or record.
///
/// # Panics
///
/// As [`expected_value`], and if that value holds no code span.
#[must_use]
pub fn expected_span(doc: &Document, id: &str, key: &str) -> CodeSpan {
    let value = expected_value(doc, id, key);
    carddemo_audit::code_span(&value)
        .unwrap_or_else(|| panic!("parity audit row {id}: {key}={value} holds no code span"))
}

/// The records of the ASCII fixture `name` under `app/data/ASCII`, each exactly the bytes
/// before its LF.
///
/// Nothing is stripped or trimmed, so a CRLF record keeps its CR and a short record its
/// stored length (E-04): `trancatg.txt` is all CRLF, `tcatbal.txt` records 1 to 49 and
/// `trantype.txt` records 1 to 6 are CRLF, and `cardxref.txt` stores 36 of 50 bytes per line.
/// Only the empty segment after a final LF is dropped; a final record without LF is kept.
/// `dailytran.txt` gives 300 records of 350 bytes.
///
/// # Panics
///
/// Panics if the file cannot be read.
#[must_use]
pub fn fixture_lines(name: &str) -> Vec<Vec<u8>> {
    let path = fixtures::ascii_dir().join(name);
    let bytes =
        fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut records: Vec<Vec<u8>> = bytes
        .split(|&byte| byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    if records.last().is_some_and(Vec::is_empty) {
        records.pop();
    }
    records
}

/// The fixed-width records of a generation or statement file written by the binary: each
/// `width` bytes followed by LF, returned without the LF.
///
/// Widths: `DALYREJS` 430 (350 + 80 trailer), `SYSTRAN` and the `TRANSACT` datasets 350,
/// `TRANREPT` 133, `STATEMNT.PS` 80 and `STATEMNT.HTML` 100.
///
/// # Panics
///
/// Panics if the length is not a whole number of `width + 1` byte frames, or if a record,
/// counted from 1 in the message, is not followed by LF or is not UTF-8.
#[must_use]
pub fn split_records(bytes: &[u8], width: usize) -> Vec<String> {
    let frame = width + 1;
    assert_eq!(
        bytes.len() % frame,
        0,
        "{} bytes are not a whole number of {width}-byte records each followed by LF",
        bytes.len()
    );
    bytes
        .chunks_exact(frame)
        .enumerate()
        .map(|(index, chunk)| {
            let number = index + 1;
            let (record, terminator) = chunk.split_at(width);
            assert_eq!(terminator, b"\n", "record {number} is not followed by LF");
            String::from_utf8(record.to_vec())
                .unwrap_or_else(|error| panic!("record {number} is not UTF-8: {error}"))
        })
        .collect()
}

/// The `COTRN2A` screen input of a CT02 add that passes every COTRN02C edit
/// [app/cbl/COTRN02C.cbl:L193-373] and the confirmation [app/cbl/COTRN02C.cbl:L169-188]:
/// the card path of `VALIDATE-INPUT-KEY-FIELDS` (`card_number` entered, account blank),
/// type `01` and category `0001`, source `POS TERM`, both dates `2022-06-10`, merchant ID
/// `800000000`, the merchant name, city and zip of the first daily-transaction fixture
/// record, and confirmation `Y`.
///
/// `card_number` must be 16 digits on file in the cross-reference. `amount` must be in the
/// screen format `-99999999.99`: a sign `+` or `-`, 8 digits, `.` and 2 digits, for example
/// `+00000050.47` [app/cbl/COTRN02C.cbl:L340-345]. `description` must not be blank.
#[must_use]
pub fn ct02_input(card_number: &str, amount: &str, description: &str) -> TransactionAddInput {
    TransactionAddInput {
        account_id: None,
        card_number: Some(card_number.to_owned()),
        type_cd: "01".to_owned(),
        cat_cd: "0001".to_owned(),
        source: "POS TERM".to_owned(),
        description: description.to_owned(),
        amount: amount.to_owned(),
        orig_date: CT02_DATE.to_owned(),
        proc_date: CT02_DATE.to_owned(),
        merchant_id: "800000000".to_owned(),
        merchant_name: "Abshire-Lowe".to_owned(),
        merchant_city: "North Enoshaven".to_owned(),
        merchant_zip: "72112".to_owned(),
        confirm: Confirm::Yes,
    }
}

/// COTRN02C PROCESS-ENTER-KEY, ADD-TRANSACTION, WRITE-TRANSACT-FILE with the CT02 reward
/// accrual, as the CT02 add endpoint performs them, in one unit of work: validate `input`,
/// take the TRAN-ID, write the record, accrue its points through `RewardEngine::accrue` at
/// `at` (an RFC 3339 instant, the processing clock), commit.
///
/// The TRAN-ID is the next one the serialized allocation gives when `tran_id` is `None`,
/// otherwise `tran_id` itself, so a test can reuse a TRAN-ID that `batch tranbkp` took out of
/// the live master. Returns the TRAN-ID written.
///
/// # Panics
///
/// Panics, naming the step and its error, if `at` is not RFC 3339 or if the begin, the
/// validation, the allocation (an empty live master included), the write, the accrual or the
/// commit fails. Nothing is committed then.
#[must_use]
pub fn ct02_add(db: &Db, input: TransactionAddInput, tran_id: Option<TranId>, at: &str) -> TranId {
    let at_utc = DateTime::parse_from_rfc3339(at)
        .unwrap_or_else(|error| panic!("CT02 accrual instant {at:?} is not RFC 3339: {error}"))
        .with_timezone(&Utc);
    block_on(async {
        let mut uow = begin(db, "CT02").await;
        let validated = transaction_add::validate(&mut uow.store(), input)
            .await
            .unwrap_or_else(|error| panic!("CT02 validation failed: {error}"));
        let tran_id = match tran_id {
            Some(given) => given,
            None => uow
                .next_tran_id()
                .await
                .unwrap_or_else(|error| panic!("CT02 TRAN-ID allocation failed: {error}")),
        };
        let record = transaction_add::build(&validated, tran_id);
        let row_id = uow
            .store()
            .write_transaction(record.clone())
            .await
            .unwrap_or_else(|error| panic!("CT02 write of TRAN-ID {tran_id} failed: {error}"));
        let transaction = Transaction::from_new(row_id, record);
        let accrual = AccrualInput::for_transaction(&transaction, validated.account_id());
        if let Err(error) = RewardEngine::accrue(&mut uow.rewards(), accrual, at_utc).await {
            panic!("CT02 reward accrual of TRAN-ID {tran_id} failed: {error}");
        }
        commit(uow, "CT02").await;
        tran_id
    })
}

/// COTRN00C PROCESS-ENTER-KEY with a blank `TRNIDIN`: the first page of the transaction list,
/// as the CT00 list endpoint serves it with no parameters. The browse starts at the lowest key,
/// reads forward, holds at most `SCREEN_ROWS` (10) rows and filters no account. The read-only
/// unit of work is rolled back.
///
/// # Panics
///
/// Panics, with the error, if the unit of work cannot begin or roll back, or if the browse
/// fails.
#[must_use]
pub fn ct00_first_page(db: &Db) -> TransactionListPage {
    let input = TransactionListInput {
        start_key: None,
        direction: Direction::Forward,
        page_size: transaction_list::SCREEN_ROWS,
        account_id: None,
    };
    block_on(async {
        let mut uow = begin(db, "CT00").await;
        let page = transaction_list::list(&mut uow.store(), input).await;
        roll_back(uow, "CT00").await;
        page.unwrap_or_else(|error| panic!("CT00 first page failed: {error}"))
    })
}

/// COTRN01C PROCESS-ENTER-KEY: the transaction `tran_id` names, as
/// the CT01 view endpoint serves it. The read-only unit of work is rolled back.
///
/// # Errors
///
/// The CT01 [`DomainError`]: empty for a blank `tran_id`, not found when no live transaction
/// has the key (for example after `batch tranbkp` empties the live master), unexpected when
/// the read fails.
///
/// # Panics
///
/// Panics, with the error, if the unit of work cannot begin or roll back.
pub fn ct01_view(db: &Db, tran_id: &str) -> Result<Transaction, DomainError> {
    let input = TransactionViewInput {
        tran_id: Some(tran_id.to_owned()),
    };
    block_on(async {
        let mut uow = begin(db, "CT01").await;
        let viewed = transaction_view::view(&mut uow.store(), input).await;
        roll_back(uow, "CT01").await;
        viewed
    })
}

/// COTRTUPC 1200-EDIT-MAP-INPUTS and 9600-WRITE-PROCESSING: writes the type `code` with
/// `description` to the Db2 master `transaction_types` and commits. Only the master changes;
/// the processing copy that `batch tranrept` reads follows after `batch seed --only tranextr`.
///
/// The edits run without a fetched type, so the no-change compare of `1205-COMPARE-OLD-NEW`
/// never applies and the change is always written: an update of an existing code, or an
/// insert when no row has it.
///
/// # Panics
///
/// Panics, naming the step and its error, if an edit rejects `code` or `description`, or if
/// the begin, the write or the commit fails.
#[must_use]
pub fn cttu_upsert(db: &Db, code: &str, description: &str) -> TranTypeSaved {
    let input = TranTypeUpsertInput {
        type_code: code.to_owned(),
        description: description.to_owned(),
    };
    let edited = tran_type_update::edit(&input, None)
        .unwrap_or_else(|error| panic!("CTTU edit of type {code:?} failed: {error}"));
    let TranTypeEdit::Changed(master) = edited else {
        panic!("CTTU edit of type {code:?} reported no change without a fetched type");
    };
    block_on(async {
        let mut uow = begin(db, "CTTU").await;
        let saved = tran_type_update::upsert(&mut uow.store(), master)
            .await
            .unwrap_or_else(|error| panic!("CTTU write of type {code:?} failed: {error}"));
        commit(uow, "CTTU").await;
        saved
    })
}

/// Begins the unit of work of the online transaction `transaction`.
async fn begin(db: &Db, transaction: &str) -> UnitOfWork {
    db.begin()
        .await
        .unwrap_or_else(|error| panic!("{transaction}: cannot begin a unit of work: {error}"))
}

/// Commits the unit of work of the online transaction `transaction`.
async fn commit(uow: UnitOfWork, transaction: &str) {
    if let Err(error) = uow.commit().await {
        panic!("{transaction}: commit failed: {error}");
    }
}

/// Rolls back the read-only unit of work of the online transaction `transaction`.
async fn roll_back(uow: UnitOfWork, transaction: &str) {
    if let Err(error) = uow.rollback().await {
        panic!("{transaction}: rollback failed: {error}");
    }
}
