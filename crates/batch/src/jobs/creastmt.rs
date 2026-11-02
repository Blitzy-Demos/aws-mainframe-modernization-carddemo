//! `batch creastmt`: the CREASTMT job [app/jcl/CREASTMT.JCL], which runs `CBSTM03A` with its
//! file-access subroutine `CBSTM03B` and writes the account statements, plus the PDF that
//! `TXT2PDF1.JCL` makes of the text statement [app/jcl/TXT2PDF1.JCL:L24-40].
//!
//! | JCL step | Here |
//! |---|---|
//! | `DELDEF01` IDCAMS DELETE/DEFINE of the `TRXFL` work KSDS (P-CREASTMT-001) | none: the sorted transactions are held in memory |
//! | `STEP010` SORT of `TRANSACT` by card number then TRAN-ID (P-CREASTMT-002) | [`step010_sort`] |
//! | `STEP020` IDCAMS REPRO into `TRXFL` (P-CREASTMT-003) | none, as `DELDEF01` |
//! | `STEP030` IEFBR14 delete of `STATEMNT.PS` and `STATEMNT.HTML` (P-CREASTMT-004) | [`step030_delete`] |
//! | `STEP040` `CBSTM03A` (P-CREASTMT-005) | [`step040_cbstm03a`], then [`prepare`] and [`write_files`] |
//!
//! Every step after `DELDEF01` runs with `COND=(0,NE)`, so the first step that fails ends the
//! job and the later ones never run. The statement lines come from
//! `carddemo_domain::rules::statement`; this module reads the files the program reads, in the
//! order `CBSTM03B` reads them, and writes the three files under `CARDDEMO_STATEMENT_DIR`
//! (`Settings::statement_dir`, default `./var/statements/`): `STATEMNT.PS` with the 80-byte
//! `STMTFILE` records, `STATEMNT.HTML` with the 100-byte `HTMLFILE` records, each record followed
//! by `LF`, and `STATEMNT.PS.PDF` with one text line per `STMTFILE` record.
//!
//! # Exit
//!
//! - 0: the statements were written; `CBSTM03A` ends through `9999-GOBACK` without setting
//!   `RETURN-CODE` (P-CBSTM03A-1264).
//! - 16, abend: a `CBSTM03B` read failed or the transaction file is empty, and `9999-ABEND-PROGRAM`
//!   calls `CEE3ABD` with no code (P-CBSTM03A-003, P-CBSTM03A-1257 to P-CBSTM03A-1259,
//!   P-CBSTM03A-011). stderr carries the `DISPLAY` lines of the path.
//! - 16, capacity stop: the transactions need more than the 51 card slots or the 10 transaction
//!   slots per card of the `CBSTM03A` tables, where the COBOL result is undefined
//!   (P-CBSTM03A-1256, halt escalation E-33).
//! - 16, setup failure: a failure with no COBOL counterpart, such as the database transaction or a
//!   statement file that cannot be deleted, rendered or written.
//!
//! `CBSTM03A` opens `STMTFILE` and `HTMLFILE` before it reads anything (P-CBSTM03A-001), and both
//! DDs are `DISP=(NEW,CATLG,DELETE)` [app/jcl/CREASTMT.JCL:L87-96]: an abend deletes what the
//! step wrote. After `STEP030` has run, every failure therefore leaves no statement file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use carddemo_domain::entities::{CardXref, Transaction};
use carddemo_domain::ports::{PortError, TransactionReader, XrefReader};
use carddemo_domain::rules::statement::{self, StatementError, StatementOutput};
use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;

use super::{JobContext, setup_failure};
use crate::exit::ExitStatus;
use crate::generations::{encode_fixed_lines, write_replacing};
use crate::pdf::render_statement;

/// `STMTFILE` `DSN=AWS.M2.CARDDEMO.STATEMNT.PS` [app/jcl/CREASTMT.JCL:L87-91].
const STMT_FILE: &str = "STATEMNT.PS";

/// `HTMLFILE` `DSN=AWS.M2.CARDDEMO.STATEMNT.HTML` [app/jcl/CREASTMT.JCL:L92-96].
const HTML_FILE: &str = "STATEMNT.HTML";

/// The `OUT` dataset of the `%TXT2PDF` conversion of `STATEMNT.PS` [app/jcl/TXT2PDF1.JCL:L38-39].
const PDF_FILE: &str = "STATEMNT.PS.PDF";

/// `STMTFILE` `DCB=(LRECL=80,…,RECFM=FB)` [app/jcl/CREASTMT.JCL:L89]. The job holds the rule's
/// records to the DD's record length, so a record of any other width fails the step instead of
/// shifting the file.
const STMT_LRECL: usize = 80;

/// `HTMLFILE` `DCB=(LRECL=100,…,RECFM=FB)` [app/jcl/CREASTMT.JCL:L94].
const HTML_LRECL: usize = 100;

/// The cross-reference records one `XrefReader::xrefs_after` page reads.
const XREF_PAGE: u16 = 500;

/// The three statement files under `CARDDEMO_STATEMENT_DIR`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatementFiles {
    /// `STATEMNT.PS`, the `STMTFILE` records.
    stmt: PathBuf,
    /// `STATEMNT.HTML`, the `HTMLFILE` records.
    html: PathBuf,
    /// `STATEMNT.PS.PDF`, the `STMTFILE` records as PDF text.
    pdf: PathBuf,
}

impl StatementFiles {
    /// The statement files in the directory `dir`.
    fn under(dir: &Path) -> Self {
        Self {
            stmt: dir.join(STMT_FILE),
            html: dir.join(HTML_FILE),
            pdf: dir.join(PDF_FILE),
        }
    }

    /// Every statement file, in write order.
    fn all(&self) -> [&Path; 3] {
        [&self.stmt, &self.html, &self.pdf]
    }
}

/// The bytes of the three statement files, built in full before any of them is written.
#[derive(Debug)]
struct PreparedFiles {
    /// `STATEMNT.PS`.
    stmt: Vec<u8>,
    /// `STATEMNT.HTML`.
    html: Vec<u8>,
    /// `STATEMNT.PS.PDF`.
    pdf: Vec<u8>,
}

/// Runs CREASTMT: `STEP010`, `STEP030` and `STEP040` in JCL order, then writes the statement
/// files. All reads run in one unit of work, which is rolled back because the job writes nothing
/// to the database.
///
/// Records `read` (the live transactions read) and `written` (the `STMTFILE` records) on the job
/// span, and ends with one event naming the statement count and the three files.
pub(super) async fn run(ctx: &JobContext, db: &Db) -> ExitStatus {
    match create_statement_files(ctx, db).await {
        Ok(()) => ExitStatus::Success,
        Err(status) => status,
    }
}

/// The steps of [`run`]; the first failing one ends the job with its status.
async fn create_statement_files(ctx: &JobContext, db: &Db) -> Result<(), ExitStatus> {
    let files = StatementFiles::under(&ctx.settings.statement_dir);
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure("statement read unit of work could not begin", err))?;
    let transactions = step010_sort(&mut uow).await?;
    step030_delete(&files)?;
    let (output, cards) = step040_cbstm03a(&mut uow, transactions).await?;
    uow.rollback()
        .await
        .map_err(|err| setup_failure("statement read unit of work could not end", err))?;
    let prepared = prepare(&output)?;
    write_files(&files, &prepared)?;
    Span::current().record("written", output.stmt_lines.len());
    tracing::info!(
        cards,
        stmt_records = output.stmt_lines.len(),
        html_records = output.html_lines.len(),
        stmt_file = %files.stmt.display(),
        html_file = %files.html.display(),
        pdf_file = %files.pdf.display(),
        "statements written"
    );
    Ok(())
}

/// CREASTMT STEP010 — P-CREASTMT-002 [app/jcl/CREASTMT.JCL:L44-55], read as CBSTM03B
/// 1000-TRNXFILE-PROC — P-CBSTM03B-002 reads the sorted file: every live transaction by card
/// number then TRAN-ID, `SORT FIELDS=(263,16,CH,A,1,16,CH,A)`, in the order the transaction
/// port returns it. The count is recorded in `read` on the job span.
///
/// A failed read is reported as the `ERROR READING TRNXFILE` abend the statement rule defines for
/// it (P-CBSTM03A-003). It ends the job before `STEP030`, as a failed `STEP010` bypasses the
/// later steps under `COND=(0,NE)`, so the statement files of the previous run stay.
async fn step010_sort(uow: &mut UnitOfWork) -> Result<Vec<Transaction>, ExitStatus> {
    let transactions = uow
        .store()
        .transactions_by_card()
        .await
        .map_err(|err| statement_failure(&StatementError::trnxfile_read_failed(&err)))?;
    Span::current().record("read", transactions.len());
    tracing::info!(
        records = transactions.len(),
        "STEP010 live transactions read by card number then TRAN-ID"
    );
    Ok(transactions)
}

/// CREASTMT STEP030 — P-CREASTMT-004 [app/jcl/CREASTMT.JCL:L66-75]: `IEFBR14` with
/// `DISP=(MOD,DELETE,DELETE)` on `STATEMNT.HTML` and `STATEMNT.PS`, which deletes each file when
/// it exists and does nothing when it does not.
///
/// `STATEMNT.PS.PDF` is deleted with them, although no CREASTMT DD names it: the PDF is the
/// `TXT2PDF1` conversion of `STATEMNT.PS`, so a run that ends without a text statement must not
/// leave the PDF of an earlier one behind.
///
/// # Errors
///
/// A setup failure when a file exists and cannot be deleted.
fn step030_delete(files: &StatementFiles) -> Result<(), ExitStatus> {
    for path in files.all() {
        if remove_if_present(path).map_err(|err| {
            setup_failure(
                "STEP030 statement file delete failed",
                format!("{}: {err}", path.display()),
            )
        })? {
            tracing::info!(path = %path.display(), "STEP030 previous statement file deleted");
        }
    }
    Ok(())
}

/// CREASTMT STEP040 — P-CREASTMT-005 [app/jcl/CREASTMT.JCL:L79-96]: `CBSTM03A`, in the order its
/// `0000-START` dispatch runs the files (P-CBSTM03A-002).
///
/// 1. CBSTM03A 8100-TRNXFILE-OPEN and 8500-READTRNX-READ — P-CBSTM03A-003, P-CBSTM03A-004: the
///    transactions are loaded into the card and transaction tables.
/// 2. CBSTM03B 2000-XREFFILE-PROC — P-CBSTM03B-003: the whole cross-reference file is read in
///    card-number order.
/// 3. CBSTM03A 1000-MAINLINE — P-CBSTM03A-005: one statement per cross-reference, with the
///    customer and the account read by key (P-CBSTM03B-004, P-CBSTM03B-005).
///
/// Returns the statement records and the number of statements, one per cross-reference.
async fn step040_cbstm03a(
    uow: &mut UnitOfWork,
    transactions: Vec<Transaction>,
) -> Result<(StatementOutput, usize), ExitStatus> {
    let table = statement::load_table(transactions).map_err(|err| statement_failure(&err))?;
    let mut store = uow.store();
    let xrefs = read_xref_file(&mut store)
        .await
        .map_err(|err| statement_failure(&StatementError::xreffile_read_failed(&err)))?;
    let cards = xrefs.len();
    let output = statement::create_statements(&mut store, xrefs, &table)
        .await
        .map_err(|err| statement_failure(&err))?;
    Ok((output, cards))
}

/// CBSTM03B 2000-XREFFILE-PROC — P-CBSTM03B-003 [app/cbl/CBSTM03B.CBL:L157-179], driving CBSTM03A
/// 1000-XREFFILE-GET-NEXT — P-CBSTM03A-1257: the sequential `READ` of `CCXREF` from its first
/// record to its end, in card-number order, [`XREF_PAGE`] records at a time.
///
/// # Errors
///
/// The port failure of the first page that cannot be read.
async fn read_xref_file<S: XrefReader>(store: &mut S) -> Result<Vec<CardXref>, PortError> {
    let mut xrefs = Vec::new();
    let mut after = None;
    loop {
        let page = store.xrefs_after(after, XREF_PAGE).await?;
        let full = page.len() == usize::from(XREF_PAGE);
        after = page.last().map(|xref| xref.xref_card_num);
        xrefs.extend(page);
        if !full {
            return Ok(xrefs);
        }
    }
}

/// The exit status of a statement failure, logged with its lines.
///
/// - CBSTM03A 8500-READTRNX-READ beyond its tables — P-CBSTM03A-1256: a capacity stop naming
///   the `halt` escalation [`statement::CAPACITY_ESCALATION`] (E-33).
/// - CBSTM03A 9999-ABEND-PROGRAM — P-CBSTM03A-011 [app/cbl/CBSTM03A.CBL:L921-923]: an abend
///   whose lines are the `DISPLAY` lines of the failed read, with no `ABCODE` because the
///   program calls `CEE3ABD` without parameters. The port failure behind the read goes to the
///   log only.
fn statement_failure(err: &StatementError) -> ExitStatus {
    match err {
        StatementError::CapacityExceeded { .. } => {
            tracing::error!(
                error = %err,
                escalation = statement::CAPACITY_ESCALATION,
                "STEP040 transactions exceed the CBSTM03A tables; no statement is written"
            );
            ExitStatus::capacity_stop(err, statement::CAPACITY_ESCALATION)
        }
        StatementError::ReadFailed { .. } => {
            let cause = std::error::Error::source(err).map(ToString::to_string);
            tracing::error!(error = %err, cause = cause.as_deref(), "STEP040 CBSTM03A abended");
            ExitStatus::abend_from(err, None)
        }
    }
}

/// Builds the bytes of the three statement files: the `STMTFILE` records at `LRECL=80` and the
/// `HTMLFILE` records at `LRECL=100`, each followed by `LF`, and the PDF of the `STMTFILE`
/// records, which `crate::pdf` writes one text object per record with every character kept.
///
/// Nothing is written until all three are built, so a failure here leaves no statement file.
///
/// # Errors
///
/// A setup failure when a record does not have its file's record length or holds a line break,
/// or when the PDF cannot be rendered. `CBSTM03A` tests no status on either file
/// (P-CBSTM03A-1263), so the audit records no COBOL outcome for these.
fn prepare(output: &StatementOutput) -> Result<PreparedFiles, ExitStatus> {
    let stmt = encode_fixed_lines(&output.stmt_lines, STMT_LRECL)
        .map_err(|err| setup_failure("STMTFILE record does not fit LRECL=80", err))?;
    let html = encode_fixed_lines(&output.html_lines, HTML_LRECL)
        .map_err(|err| setup_failure("HTMLFILE record does not fit LRECL=100", err))?;
    let pdf = render_statement(&output.stmt_lines)
        .map_err(|err| setup_failure("STATEMNT.PS.PDF could not be rendered", err))?;
    Ok(PreparedFiles { stmt, html, pdf })
}

/// Writes the three statement files, each replaced as a whole.
///
/// When one write fails, the files this run already wrote are deleted again, as the abnormal
/// end of `STEP040` deletes its `DISP=(NEW,CATLG,DELETE)` datasets: after `STEP030` every
/// statement file present was written by this run.
///
/// # Errors
///
/// A setup failure naming the file that could not be written.
fn write_files(files: &StatementFiles, prepared: &PreparedFiles) -> Result<(), ExitStatus> {
    let writes = [
        (files.stmt.as_path(), prepared.stmt.as_slice()),
        (files.html.as_path(), prepared.html.as_slice()),
        (files.pdf.as_path(), prepared.pdf.as_slice()),
    ];
    for (path, bytes) in writes {
        if let Err(err) = write_replacing(path, bytes) {
            discard_written(files);
            return Err(setup_failure("statement file write failed", err));
        }
    }
    Ok(())
}

/// Deletes every statement file still present after a failed write. A file that cannot be
/// deleted is logged and left for the next run's `STEP030`.
fn discard_written(files: &StatementFiles) {
    for path in files.all() {
        if let Err(err) = remove_if_present(path) {
            tracing::error!(
                path = %path.display(),
                error = %err,
                "statement file of the failed run could not be deleted"
            );
        }
    }
}

/// Deletes the file `path` and returns whether it existed: the `DISP=(MOD,DELETE,DELETE)`
/// disposition, where a dataset that does not exist is no error.
///
/// # Errors
///
/// The operating-system error of any outcome other than the file being deleted or absent.
fn remove_if_present(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use carddemo_domain::entities::CardXref;
    use carddemo_domain::messages::{
        ABENDING_PROGRAM, ERROR_READING_TRNXFILE, ERROR_READING_XREFFILE, RETURN_CODE,
    };
    use carddemo_domain::numeric::{AccountId, CardNumber, CustomerId};
    use carddemo_domain::ports::{PortError, PortResult, XrefReader};
    use carddemo_domain::rules::statement::{CAPACITY_ESCALATION, StatementError, StatementOutput};

    use super::{
        StatementFiles, XREF_PAGE, prepare, read_xref_file, remove_if_present, statement_failure,
        step030_delete, write_files,
    };
    use crate::exit::ExitStatus;

    /// A fresh directory of its own for the test `name`.
    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("carddemo-creastmt-{name}-{}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("clear {}: {e}", dir.display()));
        }
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        dir
    }

    fn remove_dir(dir: &Path) {
        fs::remove_dir_all(dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
    }

    fn present(files: &StatementFiles) -> Vec<bool> {
        files.all().iter().map(|path| path.exists()).collect()
    }

    /// A statement output of `stmt` 80-byte and `html` 100-byte records.
    fn output(stmt: usize, html: usize) -> StatementOutput {
        StatementOutput {
            stmt_lines: (0..stmt).map(|n| format!("{n:<80}")).collect(),
            html_lines: (0..html).map(|n| format!("{n:<100}")).collect(),
        }
    }

    /// The cross-reference file of `count` cards, numbered from 1 in key order.
    fn xref_file(count: u32) -> Vec<CardXref> {
        (1..=count)
            .map(|n| CardXref {
                xref_card_num: CardNumber::move_from(&format!("{n:016}")),
                xref_cust_id: CustomerId::new(n).expect("a 9-digit customer number"),
                xref_acct_id: AccountId::new(u64::from(n)).expect("an 11-digit account number"),
            })
            .collect()
    }

    /// `CCXREF` in memory, counting the pages read and failing from page `fail_at`.
    struct PagedXrefs {
        records: Vec<CardXref>,
        pages: usize,
        fail_at: Option<usize>,
    }

    impl XrefReader for PagedXrefs {
        async fn read_xref_by_card(&mut self, _card: CardNumber) -> PortResult<CardXref> {
            Err(PortError::NotFound)
        }

        async fn read_xref_by_account(&mut self, _id: AccountId) -> PortResult<CardXref> {
            Err(PortError::NotFound)
        }

        async fn xrefs_after(
            &mut self,
            after: Option<CardNumber>,
            limit: u16,
        ) -> PortResult<Vec<CardXref>> {
            self.pages += 1;
            if self.fail_at == Some(self.pages) {
                return Err(PortError::Unavailable("connection reset".to_owned()));
            }
            Ok(self
                .records
                .iter()
                .filter(|xref| after.is_none_or(|card| xref.xref_card_num > card))
                .take(usize::from(limit))
                .cloned()
                .collect())
        }
    }

    #[test]
    fn statement_files_are_the_cataloged_dataset_names() {
        let files = StatementFiles::under(Path::new("var/statements"));
        assert_eq!(
            files.all(),
            [
                Path::new("var/statements/STATEMNT.PS"),
                Path::new("var/statements/STATEMNT.HTML"),
                Path::new("var/statements/STATEMNT.PS.PDF"),
            ]
        );
    }

    #[test]
    fn step030_deletes_present_files_and_ignores_absent_ones() {
        let dir = scratch_dir("step030");
        let files = StatementFiles::under(&dir);
        fs::write(&files.stmt, b"old statement\n").expect("write STATEMNT.PS");
        fs::write(&files.pdf, b"old pdf").expect("write STATEMNT.PS.PDF");

        let first = step030_delete(&files);
        let after_first = present(&files);
        let second = step030_delete(&files);
        let absent = remove_if_present(&files.html).expect("an absent file is no error");
        remove_dir(&dir);

        assert_eq!(first, Ok(()));
        assert_eq!(after_first, [false, false, false]);
        assert_eq!(second, Ok(()));
        assert!(!absent);
    }

    #[test]
    fn step030_failure_is_a_setup_failure() {
        let dir = scratch_dir("step030-failure");
        let files = StatementFiles::under(&dir);
        fs::create_dir_all(files.html.join("member")).expect("a directory in the file's place");

        let status = step030_delete(&files);
        remove_dir(&dir);

        let status = status.expect_err("a directory cannot be deleted as a file");
        assert_eq!(status.code(), 16);
        assert!(
            matches!(status, ExitStatus::SetupFailure(ref text) if text.contains(super::HTML_FILE))
        );
    }

    #[test]
    fn capacity_exceeded_stops_with_the_halt_escalation() {
        let err = StatementError::CapacityExceeded {
            cards: 52,
            max_per_card: 3,
        };
        assert_eq!(CAPACITY_ESCALATION, "E-33");
        assert_eq!(
            statement_failure(&err),
            ExitStatus::CapacityStop {
                lines: vec![err.to_string()],
                escalation_id: "E-33".to_owned(),
            }
        );
        assert_eq!(statement_failure(&err).code(), 16);
    }

    #[test]
    fn read_failures_abend_with_their_display_lines_and_no_abcode() {
        let unavailable = PortError::Unavailable("connection reset".to_owned());
        let cases = [
            (
                StatementError::trnxfile_read_failed(&unavailable),
                ERROR_READING_TRNXFILE,
            ),
            (
                StatementError::xreffile_read_failed(&unavailable),
                ERROR_READING_XREFFILE,
            ),
        ];
        for (err, file) in cases {
            let status = statement_failure(&err);
            assert_eq!(status.code(), 16);
            assert_eq!(
                status,
                ExitStatus::Abend {
                    lines: vec![
                        file.to_owned(),
                        format!("{RETURN_CODE}30"),
                        ABENDING_PROGRAM.to_owned()
                    ],
                    abcode: None,
                },
                "{file}"
            );
        }
    }

    #[test]
    fn prepare_encodes_both_record_files_and_the_pdf() {
        let out = output(2, 3);
        let prepared = prepare(&out).expect("records of the right widths");
        assert_eq!(prepared.stmt.len(), 2 * 81);
        assert_eq!(prepared.html.len(), 3 * 101);
        assert_eq!(&prepared.stmt[..81], format!("{:<80}\n", 0).as_bytes());
        assert_eq!(
            &prepared.html[101..],
            format!("{:<100}\n{:<100}\n", 1, 2).as_bytes()
        );
        assert!(prepared.pdf.starts_with(b"%PDF-"));
    }

    #[test]
    fn prepare_rejects_a_record_off_its_record_length() {
        let cases = [
            StatementOutput {
                stmt_lines: vec![format!("{:<79}", "short")],
                html_lines: Vec::new(),
            },
            StatementOutput {
                stmt_lines: Vec::new(),
                html_lines: vec![format!("{:<101}", "long")],
            },
        ];
        for out in cases {
            let status = prepare(&out).expect_err("a record off its LRECL");
            assert!(matches!(status, ExitStatus::SetupFailure(_)), "{status:?}");
        }
    }

    #[test]
    fn write_files_writes_the_three_files() {
        let dir = scratch_dir("write");
        let files = StatementFiles::under(&dir.join("statements"));
        let prepared = prepare(&output(1, 1)).expect("records of the right widths");

        let written = write_files(&files, &prepared);
        let contents: Vec<Vec<u8>> = files
            .all()
            .iter()
            .map(|path| fs::read(path).unwrap_or_default())
            .collect();
        remove_dir(&dir);

        assert_eq!(written, Ok(()));
        assert_eq!(contents, [prepared.stmt, prepared.html, prepared.pdf]);
    }

    #[test]
    fn a_failed_write_leaves_no_statement_file() {
        let dir = scratch_dir("write-failure");
        let mut files = StatementFiles::under(&dir);
        let blocker = dir.join("not-a-directory");
        fs::write(&blocker, b"").expect("write the blocking file");
        files.html = blocker.join(super::HTML_FILE);
        let prepared = prepare(&output(1, 1)).expect("records of the right widths");

        let written = write_files(&files, &prepared);
        let left = present(&files);
        remove_dir(&dir);

        let status = written.expect_err("STATEMNT.HTML cannot be created under a file");
        assert!(matches!(status, ExitStatus::SetupFailure(_)), "{status:?}");
        assert_eq!(left, [false, false, false]);
    }

    #[tokio::test]
    async fn the_cross_reference_file_is_read_to_its_end_in_key_order() {
        let page = u32::from(XREF_PAGE);
        for count in [0, 1, page - 1, page, 2 * page + 3] {
            let mut store = PagedXrefs {
                records: xref_file(count),
                pages: 0,
                fail_at: None,
            };
            let read = read_xref_file(&mut store).await;
            let pages = usize::try_from(count / page + 1).expect("a small page count");
            assert_eq!(read.as_ref(), Ok(&store.records), "{count} records");
            assert_eq!(store.pages, pages, "{count} records");
        }
    }

    #[tokio::test]
    async fn a_failed_cross_reference_page_is_the_port_failure() {
        let mut store = PagedXrefs {
            records: xref_file(u32::from(XREF_PAGE) + 1),
            pages: 0,
            fail_at: Some(2),
        };
        assert_eq!(
            read_xref_file(&mut store).await,
            Err(PortError::Unavailable("connection reset".to_owned()))
        );
    }
}
