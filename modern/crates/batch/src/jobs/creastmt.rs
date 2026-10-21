//! `carddemo-batch creastmt`: job CREASTMT, whose `STEP040` runs `CBSTM03A` with its file
//! subroutine `CBSTM03B` to write the account statements, followed by the PDF conversion the
//! separate job `TXT2PDF1` performs (`CREASTMT.JCL:22-96`, `TXT2PDF1.JCL:24-39`; AAP 0.3.4.3,
//! 0.3.4.5). Every statement byte is `domain::batch::cbstm03a`'s and every PDF byte
//! `crate::pdf`'s; this module reads, writes and fails in the program's order, after the rows of
//! `modern/audit/batch/CREASTMT.md`. Over the `POSTED` scenario the expected files are
//! FIXSTMT-001 to FIXSTMT-904 of `modern/audit/fixtures.md`.
//!
//! # The steps it replaces
//!
//! | JCL step | What it does | Replacement |
//! |---|---|---|
//! | `DELDEF01` | `IDCAMS` deletes and defines the work KSDS `TRXFL` | none: no work file (CBSTM03A-001) |
//! | `STEP010` | `SORT FIELDS=(263,16,CH,A,1,16,CH,A)` of `TRANSACT`: card number, then transaction ID | `TransactionRepository::find_all_in_card_order`, then `TransactionTable::build`, which puts the rows in EBCDIC order itself (CBSTM03A-001, X-CREASTMT-3) |
//! | `STEP020` | `REPRO` of the sorted records into `TRXFL` | none: the rows stay in memory (CBSTM03A-001, CBSTM03B-002) |
//! | `STEP030` | `IEFBR14` deletes the previous `STATEMNT.PS` and `STATEMNT.HTML` | `OutputFile::replace` of the three fixed names, `STATEMNT.PDF` included, before the first read (CBSTM03A-002) |
//! | `STEP040` | `CBSTM03A` writes `STMTFILE` and `HTMLFILE` | [`run`]: `STATEMNT.PS` of 80-byte records and `STATEMNT.HTML` of 100-byte records, the program's FD rather than the `LRECL=80` of `STEP030` (CBSTM03A-031, CBSTM03A-047, E20) |
//! | `TXT2PDF1` `TXT2PDF` | `%TXT2PDF` converts `STATEMNT.PS` into `STATEMNT.PS.PDF` | `crate::pdf::render` over the text records, written as `STATEMNT.PDF`, the name AAP 0.3.4.5 gives it (CBSTM03A-051, X-CREASTMT-7) |
//!
//! # Flow
//!
//! 1. The three files are replaced first, so the previous run's are gone before any table is
//!    read, as `STEP030` deletes them and `CBSTM03A` opens its outputs before its first read
//!    (CBSTM03A-002, CBSTM03A-004). The names are fixed, not `(+1)` generations.
//! 2. Every `transactions` row is read and loaded into the `TransactionTable`, before the first
//!    cross-reference record, as 8100-TRNXFILE-OPEN and 8500-READTRNX-READ load `WS-TRNX-TABLE`
//!    (CBSTM03A-006 to CBSTM03A-008).
//! 3. Every `card_xref` row is read and put in key order, `xref_sort_key`, the EBCDIC key order
//!    of the KSDS `CBSTM03B` reads sequentially (CBSTM03A-009, CBSTM03B-003). Each one gets one
//!    statement: its customer by `cust_id` and its account by `acct_id` (CBSTM03A-010,
//!    CBSTM03A-011, CBSTM03B-004, CBSTM03B-005), then `cbstm03a::statement`, whose text records
//!    go to `STATEMNT.PS` and HTML records to `STATEMNT.HTML`, each followed by LF.
//! 4. After the last statement the PDF is rendered from all the text records and written; then
//!    `STATEMNT.PS`, `STATEMNT.HTML` and `STATEMNT.PDF` are completed in that order and their
//!    paths returned in that order, which `main` prints; return code 0 (CBSTM03A-050,
//!    CBSTM03A-051).
//!
//! Every read is read-only and outside any transaction: the job writes no table.
//!
//! # Abends
//!
//! The four `ERROR READING` paths of `CBSTM03A` end the job through 9999-ABEND-PROGRAM
//! (CBSTM03A-049): a transaction set with no record, status `10` (CBSTM03A-007); a missing
//! customer or account, status `23` (CBSTM03A-010, CBSTM03A-011); and a failed read of any of
//! the four tables, for which the replacement shows status `30` (CBSTM03A-012, CBSTM03B-006,
//! halt candidate X-CREASTMT-H3). The program's two lines and `ABENDING PROGRAM` are shown, and
//! the job returns [`JobError::Abend`] with return code 16 (X-CREASTMT-2). Every file the run
//! created is then removed, as `DISP=(NEW,CATLG,DELETE)` deletes `STMTFILE` and `HTMLFILE` at
//! abend, so after any failure none of the three files remains and no path is printed; a
//! failure while completing them removes the ones already completed, so a run leaves all three
//! or none (CBSTM03A-002). The empty-table outcome itself is halt candidate X-CREASTMT-H1: the
//! replacement takes the CBSTM03A-007 path, which claims no parity for that case.
//!
//! A transaction set beyond the 51 cards of 10 transactions of `WS-TRNX-TABLE` is printed in
//! full, as the domain keeps every card and transaction, and reported with a warning event,
//! because its parity is blocked (CBSTM03A-008, halt candidate X-CREASTMT-H2).
//!
//! # Rules
//!
//! - Cardholder Data Redaction: no statement record carries a card number, an SSN, a CVV or a
//!   password (X-CREASTMT-5); the records hold customer names, addresses and balances, and stay
//!   local files that no endpoint serves (AAP 0.3.4.4). Nothing here formats a record or a key:
//!   the `DISPLAY` lines are fixed texts, a failed read adds the persistence error's text, an
//!   SQLSTATE or a constraint name only, and every [`JobError`] reason is static.
//! - Local-Only Runtime: files are written only under `Config::statement_dir` (default
//!   `./var/statements/`), with no object storage; the only network use is the `PostgreSQL`
//!   pool.
//! - Single Schema Executor: no schema work here; against a database whose schema has not been
//!   applied, the first read fails and the job abends as CBSTM03A-012 states.
//! - R1: this module handles only record bytes; the page coordinates are `crate::pdf`'s.

use std::fs;
use std::path::Path;

use domain::batch::cbstm03a::{self, STMT_RECORD_LEN, Statement, StatementError, TransactionTable};
use domain::records::{CardXrefRecord, TransactionRecord};
use persistence::{
    AccountRepository, CardXrefRepository, CustomerRepository, PersistenceError,
    TransactionRepository,
};

use super::{JobContext, JobError, JobOutcome, OutputFile, count};

/// The text statement, `STMTFILE` `DSN=AWS.M2.CARDDEMO.STATEMNT.PS` (CBSTM03A-002).
const TEXT_FILE: &str = "STATEMNT.PS";

/// The HTML statement, `HTMLFILE` `DSN=AWS.M2.CARDDEMO.STATEMNT.HTML` (CBSTM03A-002).
const HTML_FILE: &str = "STATEMNT.HTML";

/// The statement PDF, which `TXT2PDF1` names `STATEMNT.PS.PDF` (CBSTM03A-051, X-CREASTMT-7).
const PDF_FILE: &str = "STATEMNT.PDF";

/// The `DISPLAY` of 9999-ABEND-PROGRAM, after the two lines of the failed read (CBSTM03A-049).
const ABENDING_PROGRAM: &str = "ABENDING PROGRAM";

/// The exit status of an abend. `CALL 'CEE3ABD'` passes no abend code, so the source defines
/// none: 16 is the replacement's value, and only a non-zero status is parity (CBSTM03A-049,
/// X-CREASTMT-2).
const ABEND_RETURN_CODE: u8 = 16;

/// Runs CREASTMT: writes `STATEMNT.PS`, `STATEMNT.HTML` and `STATEMNT.PDF` under
/// `Config::statement_dir` in place of the previous run's, and returns their paths in that
/// order, return code 0, and the counts `statements`, `text_records` and `html_records`
/// (CBSTM03A-050).
///
/// # Errors
///
/// - [`JobError::Abend`] with return code 16 when the transaction set is empty (CBSTM03A-007),
///   when a cross-reference record's customer or account is missing (CBSTM03A-010,
///   CBSTM03A-011), or when a read of `transactions`, `card_xref`, `customers` or `accounts`
///   fails (CBSTM03A-012), after the program's `DISPLAY` lines (CBSTM03A-049).
/// - [`JobError::Render`] when a text record cannot be rendered into the PDF (CBSTM03A-051).
/// - [`JobError::Io`] when a file cannot be replaced, written, or completed.
///
/// After any error none of the three files remains (CBSTM03A-002, CBSTM03A-049).
pub(crate) async fn run(ctx: &JobContext<'_>) -> Result<JobOutcome, JobError> {
    let mut files = StatementFiles::replace(&ctx.config.statement_dir)?;
    // The store is a handle on the shared pool, cloned for the `&mut self` repository calls.
    let mut store = ctx.store.clone();
    let table = transaction_table(TransactionRepository::find_all_in_card_order(&mut store).await)?;
    let xrefs = in_key_order(CardXrefRepository::find_all(&mut store).await)?;
    for xref in &xrefs {
        let customer = found(
            CustomerRepository::find_by_id(&mut store, xref.cust_id).await,
            ReadFailure::CustomerNotFound,
            ReadFailure::CustomerUnreadable,
        )?;
        let account = found(
            AccountRepository::find_by_id(&mut store, xref.acct_id).await,
            ReadFailure::AccountNotFound,
            ReadFailure::AccountUnreadable,
        )?;
        files.write(&cbstm03a::statement(xref, &customer, &account, &table))?;
    }
    files.finish()
}

/// The table of every transaction, from the read that stands for `TRNXFILE` (CBSTM03A-001,
/// CBSTM03A-006 to CBSTM03A-008). The table orders the rows in EBCDIC order itself, whatever
/// order the read returns them in.
///
/// # Errors
///
/// [`JobError::Abend`] for a read that failed (CBSTM03A-012) or returned no row, the first read
/// at end of file (CBSTM03A-007).
fn transaction_table(
    read: Result<Vec<TransactionRecord>, PersistenceError>,
) -> Result<TransactionTable, JobError> {
    let rows = read.map_err(|error| abend(ReadFailure::TransactionsUnreadable, Some(&error)))?;
    let table = TransactionTable::build(&rows).map_err(|error| match error {
        StatementError::NoTransactions => abend(ReadFailure::TransactionsEmpty, None),
    })?;
    if !table.within_cobol_bounds() {
        tracing::warn!(
            cards = table.card_count(),
            "the transactions exceed the 51 cards of 10 transactions of the CBSTM03A table: \
             every statement is written in full, and its parity is BLOCKED (X-CREASTMT-H2)"
        );
    }
    Ok(table)
}

/// The cross-reference records, from the read that stands for `XREFFILE`, in the key order of
/// the KSDS: `XREF-CARD-NUM` ascending in EBCDIC order (CBSTM03A-009, CBSTM03B-003,
/// X-CREASTMT-3). The sort is stable and the card number is the table's key, so no two records
/// tie.
///
/// # Errors
///
/// [`JobError::Abend`] for a read that failed (CBSTM03A-012).
fn in_key_order(
    read: Result<Vec<CardXrefRecord>, PersistenceError>,
) -> Result<Vec<CardXrefRecord>, JobError> {
    let mut xrefs = read.map_err(|error| abend(ReadFailure::XrefsUnreadable, Some(&error)))?;
    xrefs.sort_by_cached_key(cbstm03a::xref_sort_key);
    Ok(xrefs)
}

/// The record a keyed read returned, the status `00` of `CBSTM03B` (CBSTM03B-006).
///
/// # Errors
///
/// [`JobError::Abend`] on the path `missing` names when no row has the key, status `23`
/// (CBSTM03A-010, CBSTM03A-011), and on the path `failed` names when the read failed
/// (CBSTM03A-012).
fn found<T>(
    read: Result<Option<T>, PersistenceError>,
    missing: ReadFailure,
    failed: ReadFailure,
) -> Result<T, JobError> {
    match read {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(abend(missing, None)),
        Err(error) => Err(abend(failed, Some(&error))),
    }
}

/// The read whose failure ends the program: one `ERROR READING` path of `CBSTM03A`, with the
/// file status `CBSTM03B` returns for it (CBSTM03B-006).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadFailure {
    /// The first `TRNXFILE` read is at end of file, status `10`: `transactions` holds no row
    /// (CBSTM03A-007).
    TransactionsEmpty,
    /// `transactions` could not be read (CBSTM03A-012).
    TransactionsUnreadable,
    /// `card_xref` could not be read (CBSTM03A-012).
    XrefsUnreadable,
    /// No customer has the cross-reference record's `XREF-CUST-ID`, status `23`
    /// (CBSTM03A-010).
    CustomerNotFound,
    /// `customers` could not be read (CBSTM03A-012).
    CustomerUnreadable,
    /// No account has the cross-reference record's `XREF-ACCT-ID`, status `23` (CBSTM03A-011).
    AccountNotFound,
    /// `accounts` could not be read (CBSTM03A-012).
    AccountUnreadable,
}

impl ReadFailure {
    /// The first `DISPLAY` of the paragraph whose read failed: `ERROR READING ` and the DD name
    /// of the file the table stands for.
    const fn error_line(self) -> &'static str {
        match self {
            Self::TransactionsEmpty | Self::TransactionsUnreadable => "ERROR READING TRNXFILE",
            Self::XrefsUnreadable => "ERROR READING XREFFILE",
            Self::CustomerNotFound | Self::CustomerUnreadable => "ERROR READING CUSTFILE",
            Self::AccountNotFound | Self::AccountUnreadable => "ERROR READING ACCTFILE",
        }
    }

    /// The second `DISPLAY`, `RETURN CODE: ` and the file status. A database failure has no
    /// file status, so its `30` is the replacement's and not a parity result (CBSTM03A-012,
    /// halt candidate X-CREASTMT-H3).
    const fn return_code_line(self) -> &'static str {
        match self {
            Self::TransactionsEmpty => "RETURN CODE: 10",
            Self::CustomerNotFound | Self::AccountNotFound => "RETURN CODE: 23",
            Self::TransactionsUnreadable
            | Self::XrefsUnreadable
            | Self::CustomerUnreadable
            | Self::AccountUnreadable => "RETURN CODE: 30",
        }
    }

    /// The [`JobError::Abend`] reason: the DD name, what failed and the status, never a key.
    const fn reason(self) -> &'static str {
        match self {
            Self::TransactionsEmpty => {
                "CBSTM03A: ERROR READING TRNXFILE: the transaction set holds no record (status 10)"
            }
            Self::TransactionsUnreadable => {
                "CBSTM03A: ERROR READING TRNXFILE: the transactions could not be read (status 30)"
            }
            Self::XrefsUnreadable => {
                "CBSTM03A: ERROR READING XREFFILE: the card cross-references could not be read \
                 (status 30)"
            }
            Self::CustomerNotFound => {
                "CBSTM03A: ERROR READING CUSTFILE: no customer record for a card cross-reference \
                 (status 23)"
            }
            Self::CustomerUnreadable => {
                "CBSTM03A: ERROR READING CUSTFILE: a customer record could not be read (status 30)"
            }
            Self::AccountNotFound => {
                "CBSTM03A: ERROR READING ACCTFILE: no account record for a card cross-reference \
                 (status 23)"
            }
            Self::AccountUnreadable => {
                "CBSTM03A: ERROR READING ACCTFILE: an account record could not be read (status 30)"
            }
        }
    }
}

// The `DISPLAY` lines become tracing events whose message is the line's text exactly, as for
// `cbpaup0j`, rather than no output at all: `CREASTMT.md` puts the two lines of the failed read
// and `ABENDING PROGRAM` on standard output in program order (CBSTM03A-049), and the plain
// stdout lines stay the written paths alone, of which an abended run has none.
/// 9999-ABEND-PROGRAM after the failed read `failure`: its two `DISPLAY` lines, then
/// `ABENDING PROGRAM`, one `ERROR` event each, and the [`JobError::Abend`] that ends the job with
/// return code 16 (CBSTM03A-049, X-CREASTMT-2). The first event of a failed read also carries the
/// persistence error's text, an SQLSTATE, a constraint or a column only.
fn abend(failure: ReadFailure, cause: Option<&PersistenceError>) -> JobError {
    let error_line = failure.error_line();
    if let Some(cause) = cause {
        tracing::error!(cause = %cause, "{error_line}");
    } else {
        tracing::error!("{error_line}");
    }
    let return_code_line = failure.return_code_line();
    tracing::error!("{return_code_line}");
    tracing::error!("{ABENDING_PROGRAM}");
    JobError::Abend {
        code: Some(ABEND_RETURN_CODE),
        reason: failure.reason(),
    }
}

/// The three statement files of one run, open from before the first read to the end of the
/// job, and what has been written to them.
///
/// Each file is an [`OutputFile`], so dropping this value before [`StatementFiles::finish`], as
/// any error return does, removes all three, as `DISP=(NEW,CATLG,DELETE)` deletes the outputs of
/// an abended step (CBSTM03A-049).
struct StatementFiles {
    /// `STATEMNT.PS`.
    text: OutputFile,
    /// `STATEMNT.HTML`.
    html: OutputFile,
    /// `STATEMNT.PDF`, written only once every text record is known.
    pdf: OutputFile,
    /// Every text record written so far, in order: the input of the PDF (CBSTM03A-051).
    text_lines: Vec<[u8; STMT_RECORD_LEN]>,
    /// Statements written so far, one per cross-reference record (CBSTM03A-009).
    statements: usize,
    /// HTML records written so far.
    html_records: usize,
}

impl StatementFiles {
    /// Replaces the three files in `dir`, creating `dir` when it is missing: `STEP030` deletes
    /// the previous run's statement files and `CBSTM03A` opens both outputs before its first
    /// read; the PDF of the previous run goes with them (CBSTM03A-002, CBSTM03A-004).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] when the directory cannot be created, a previous file cannot be removed
    /// or a new one cannot be created; the files already created are removed again.
    fn replace(dir: &Path) -> Result<Self, JobError> {
        Ok(Self {
            text: OutputFile::replace(dir, TEXT_FILE)?,
            html: OutputFile::replace(dir, HTML_FILE)?,
            pdf: OutputFile::replace(dir, PDF_FILE)?,
            text_lines: Vec::new(),
            statements: 0,
            html_records: 0,
        })
    }

    /// Writes one statement: its text records to `STATEMNT.PS` and its HTML records to
    /// `STATEMNT.HTML`, each record followed by LF, in the order `CBSTM03A` writes them
    /// (CBSTM03A-031, CBSTM03A-047).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] when a record cannot be written.
    fn write(&mut self, statement: &Statement) -> Result<(), JobError> {
        for record in &statement.text_lines {
            self.text.write_record(record)?;
        }
        for record in &statement.html_records {
            self.html.write_record(record)?;
        }
        self.text_lines.extend_from_slice(&statement.text_lines);
        self.html_records += statement.html_records.len();
        self.statements += 1;
        Ok(())
    }

    /// Renders and writes `STATEMNT.PDF` from every text record, then completes `STATEMNT.PS`,
    /// `STATEMNT.HTML` and `STATEMNT.PDF` in that order, and returns the outcome of a normal end
    /// (CBSTM03A-050, CBSTM03A-051). With no statement the text and HTML files are empty and the
    /// PDF has one empty page.
    ///
    /// # Errors
    ///
    /// [`JobError::Render`] when the PDF cannot be rendered, and [`JobError::Io`] when a file
    /// cannot be written or completed. None of the three files remains then: the unfinished
    /// ones are removed by their guards and the completed ones here, so that a run leaves all
    /// three files or none (CBSTM03A-002).
    fn finish(self) -> Result<JobOutcome, JobError> {
        let Self {
            text,
            html,
            mut pdf,
            text_lines,
            statements,
            html_records,
        } = self;
        let bytes = crate::pdf::render(&text_lines)?;
        pdf.write_bytes(&bytes)?;
        let text_path = text.finish()?;
        let html_path = html
            .finish()
            .inspect_err(|_| discard(&[text_path.as_path()]))?;
        let pdf_path = pdf
            .finish()
            .inspect_err(|_| discard(&[text_path.as_path(), html_path.as_path()]))?;
        Ok(JobOutcome {
            written: vec![text_path, html_path, pdf_path],
            return_code: 0,
            counts: vec![
                ("statements", count(statements)),
                ("text_records", count(text_lines.len())),
                ("html_records", count(html_records)),
            ],
        })
    }
}

/// Removes the files of this run already completed when a later one could not be, so that the
/// run leaves none of the three (CBSTM03A-002, CBSTM03A-049).
fn discard(completed: &[&Path]) {
    for path in completed {
        // Best effort: the job fails with the completion error whether or not this succeeds.
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use copybook::{PicU, PicX};
    use domain::batch::cbstm03a::{HTML_RECORD_LEN, STMT_RECORD_LEN, Statement};
    use domain::records::CardXrefRecord;
    use domain::sensitive::CardNumber;
    use persistence::PersistenceError;

    use super::{
        HTML_FILE, JobError, PDF_FILE, ReadFailure, StatementFiles, TEXT_FILE, abend, found,
        in_key_order, transaction_table,
    };
    use crate::pdf::RenderError;

    type TestResult = Result<(), Box<dyn Error>>;

    /// Tells apart the scratch directories of the tests of one process.
    static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

    /// A directory of its own under the system temporary directory, removed with its content
    /// when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Result<Self, Box<dyn Error>> {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "carddemo-creastmt-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }

        /// The statement directory inside the scratch directory, not created yet.
        fn statements(&self) -> PathBuf {
            self.0.join("statements")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The file names in `dir`, sorted.
    fn names(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
        let mut names = fs::read_dir(dir)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        names.sort();
        Ok(names)
    }

    /// `dir` holding a file of each statement name, as a previous run leaves it.
    fn previous_run(dir: &Path) -> TestResult {
        fs::create_dir_all(dir)?;
        for name in [TEXT_FILE, HTML_FILE, PDF_FILE] {
            fs::write(dir.join(name), b"previous run\n")?;
        }
        Ok(())
    }

    /// A statement of `text.len()` text records and `html.len()` HTML records, each record its
    /// byte repeated.
    fn statement(text: &[u8], html: &[u8]) -> Statement {
        Statement {
            text_lines: text.iter().map(|&byte| [byte; STMT_RECORD_LEN]).collect(),
            html_records: html.iter().map(|&byte| [byte; HTML_RECORD_LEN]).collect(),
        }
    }

    /// The bytes of fixed-length records each followed by LF.
    fn lf_records<const N: usize>(records: &[[u8; N]]) -> Vec<u8> {
        records
            .iter()
            .flat_map(|record| record.iter().copied().chain([b'\n']))
            .collect()
    }

    /// A cross-reference record of a synthetic card number, in no fixture file; `cust_id`
    /// identifies it in the assertions, so no card number is compared or shown.
    fn xref(card: [u8; 16], cust_id: u64) -> Result<CardXrefRecord, Box<dyn Error>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(card)),
            cust_id: PicU::new(cust_id)?,
            acct_id: PicU::new(cust_id)?,
            filler: PicX::spaces(),
        })
    }

    /// Each failed read shows its two `DISPLAY` lines and ends the job with return code 16 and
    /// a static reason that names the file and the status.
    // audit: CBSTM03A-007
    // audit: CBSTM03A-010
    // audit: CBSTM03A-011
    // audit: CBSTM03A-012
    // audit: CBSTM03A-049
    #[test]
    fn every_read_failure_abends_with_its_lines_and_return_code_16() {
        let cases = [
            (ReadFailure::TransactionsEmpty, "TRNXFILE", "10"),
            (ReadFailure::TransactionsUnreadable, "TRNXFILE", "30"),
            (ReadFailure::XrefsUnreadable, "XREFFILE", "30"),
            (ReadFailure::CustomerNotFound, "CUSTFILE", "23"),
            (ReadFailure::CustomerUnreadable, "CUSTFILE", "30"),
            (ReadFailure::AccountNotFound, "ACCTFILE", "23"),
            (ReadFailure::AccountUnreadable, "ACCTFILE", "30"),
        ];
        for (failure, dd, status) in cases {
            assert_eq!(failure.error_line(), format!("ERROR READING {dd}"));
            assert_eq!(failure.return_code_line(), format!("RETURN CODE: {status}"));
            let reason = failure.reason();
            assert!(reason.starts_with(&format!("CBSTM03A: ERROR READING {dd}: ")));
            assert!(reason.ends_with(&format!("(status {status})")), "{reason}");
            let error = abend(failure, Some(&PersistenceError::Unavailable));
            assert_eq!(error.exit_code(), Some(16), "{reason}");
            assert_eq!(
                error.to_string(),
                format!("ABEND: {reason} (return code 16)")
            );
        }
    }

    /// A keyed read gives its record, or the abend of a missing key or of a failed read.
    // audit: CBSTM03A-010
    // audit: CBSTM03A-011
    // audit: CBSTM03B-006
    #[test]
    fn keyed_reads_give_the_record_or_their_abend() -> TestResult {
        let paths = [
            (
                ReadFailure::CustomerNotFound,
                ReadFailure::CustomerUnreadable,
            ),
            (ReadFailure::AccountNotFound, ReadFailure::AccountUnreadable),
        ];
        for (missing, failed) in paths {
            assert_eq!(found(Ok(Some(7_u8)), missing, failed).ok(), Some(7));
            let cases = [
                (Ok(None), missing),
                (Err(PersistenceError::Unavailable), failed),
            ];
            for (read, path) in cases {
                let error = found::<u8>(read, missing, failed)
                    .err()
                    .ok_or("a read with no record gave one")?;
                assert_eq!(
                    error.to_string(),
                    format!("ABEND: {} (return code 16)", path.reason())
                );
            }
        }
        Ok(())
    }

    /// An empty transaction set and a failed transaction read both abend at `TRNXFILE`, before
    /// any statement.
    // audit: CBSTM03A-007
    // audit: CBSTM03A-012
    #[test]
    fn no_transaction_table_without_transactions() -> TestResult {
        let cases = [
            (Ok(Vec::new()), ReadFailure::TransactionsEmpty),
            (
                Err(PersistenceError::Unavailable),
                ReadFailure::TransactionsUnreadable,
            ),
        ];
        for (read, path) in cases {
            let error = transaction_table(read)
                .err()
                .ok_or("a table was built from no transaction")?;
            assert_eq!(error.exit_code(), Some(16));
            assert_eq!(
                error.to_string(),
                format!("ABEND: {} (return code 16)", path.reason())
            );
        }
        Ok(())
    }

    /// Cross-reference records are taken in EBCDIC key order, whatever order the read returns:
    /// a letter sorts below a digit there, unlike in ASCII byte order; a failed read abends at
    /// `XREFFILE`.
    // audit: CBSTM03A-009
    // audit: CBSTM03B-003
    #[test]
    fn cross_references_are_taken_in_ebcdic_key_order() -> TestResult {
        let read = vec![
            xref(*b"4000000000000003", 3)?,
            xref(*b"4000000000000001", 1)?,
            xref(*b"A000000000000000", 0)?,
            xref(*b"4000000000000002", 2)?,
        ];
        let ordered = in_key_order(Ok(read))?;
        let order: Vec<u64> = ordered.iter().map(|xref| xref.cust_id.get()).collect();
        assert_eq!(order, [0, 1, 2, 3]);
        let error = in_key_order(Err(PersistenceError::Unavailable))
            .err()
            .ok_or("a failed read gave records")?;
        assert_eq!(
            error.to_string(),
            format!(
                "ABEND: {} (return code 16)",
                ReadFailure::XrefsUnreadable.reason()
            )
        );
        Ok(())
    }

    /// A run replaces the previous run's three files and leaves exactly its own: the records of
    /// each statement in order, each followed by LF, and a PDF; the paths come back in the
    /// order `STATEMNT.PS`, `STATEMNT.HTML`, `STATEMNT.PDF`, with the counts.
    // audit: CBSTM03A-002
    // audit: CBSTM03A-031
    // audit: CBSTM03A-047
    // audit: CBSTM03A-050
    // audit: CBSTM03A-051
    #[test]
    fn a_run_replaces_the_three_files_with_its_own() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.statements();
        previous_run(&dir)?;
        let mut files = StatementFiles::replace(&dir)?;
        assert_eq!(
            names(&dir)?,
            [HTML_FILE, PDF_FILE, TEXT_FILE].map(String::from)
        );
        for name in [TEXT_FILE, HTML_FILE, PDF_FILE] {
            assert!(
                fs::read(dir.join(name))?.is_empty(),
                "{name} kept old bytes"
            );
        }
        files.write(&statement(b"AB", b"<"))?;
        files.write(&statement(b"C", b">"))?;
        let outcome = files.finish()?;
        assert_eq!(
            outcome.written,
            [TEXT_FILE, HTML_FILE, PDF_FILE].map(|name| dir.join(name))
        );
        assert_eq!(outcome.return_code, 0);
        assert_eq!(
            outcome.counts,
            [("statements", 2), ("text_records", 3), ("html_records", 2)]
        );
        let text = [b'A', b'B', b'C'].map(|byte| [byte; STMT_RECORD_LEN]);
        assert_eq!(fs::read(dir.join(TEXT_FILE))?, lf_records(&text));
        let html = [b'<', b'>'].map(|byte| [byte; HTML_RECORD_LEN]);
        assert_eq!(fs::read(dir.join(HTML_FILE))?, lf_records(&html));
        assert!(fs::read(dir.join(PDF_FILE))?.starts_with(b"%PDF-"));
        assert_eq!(
            names(&dir)?,
            [HTML_FILE, PDF_FILE, TEXT_FILE].map(String::from)
        );
        Ok(())
    }

    /// With no cross-reference record the run ends normally with two empty statement files and
    /// a PDF of one empty page.
    // audit: CBSTM03A-009
    // audit: CBSTM03A-051
    #[test]
    fn no_cross_reference_gives_empty_statement_files() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.statements();
        let outcome = StatementFiles::replace(&dir)?.finish()?;
        assert_eq!(outcome.written.len(), 3);
        assert!(fs::read(dir.join(TEXT_FILE))?.is_empty());
        assert!(fs::read(dir.join(HTML_FILE))?.is_empty());
        assert!(fs::read(dir.join(PDF_FILE))?.starts_with(b"%PDF-"));
        assert_eq!(
            outcome.counts,
            [("statements", 0), ("text_records", 0), ("html_records", 0)]
        );
        Ok(())
    }

    /// A run that ends early, as every abend returns, and a run whose PDF cannot be rendered
    /// both leave none of the three files, the previous run's included.
    // audit: CBSTM03A-002
    // audit: CBSTM03A-049
    #[test]
    fn a_failed_run_leaves_no_statement_file() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.statements();
        previous_run(&dir)?;
        let mut files = StatementFiles::replace(&dir)?;
        files.write(&statement(b"A", b"<"))?;
        drop(files);
        assert!(names(&dir)?.is_empty(), "an abended run left a file");

        previous_run(&dir)?;
        let mut files = StatementFiles::replace(&dir)?;
        files.write(&statement(b"AB", b"<"))?;
        files.write(&statement(&[0x01], b">"))?;
        let error = files
            .finish()
            .err()
            .ok_or("a record of no text was rendered")?;
        assert!(
            matches!(error, JobError::Render(RenderError::NonText { line: 3 })),
            "{error}"
        );
        assert!(names(&dir)?.is_empty(), "a failed render left a file");
        Ok(())
    }
}
