//! `tranrept`: batch job TRANREPT, the transaction detail report of `CBTRN03C` (`TRANREPT.jcl`,
//! `TRANREPT.prc`; AAP 0.3.4.3). Its expected results are the `CBTRN03C-` rows of
//! `modern/audit/batch/TRANREPT.md` and, over the POSTED scenario, the `FIXRPT-` rows of
//! `modern/audit/fixtures.md`.
//!
//! # The steps it replaces
//!
//! | JCL step | What it does | Replacement |
//! |---|---|---|
//! | `STEP05R` (`REPROC`) | IDCAMS `REPRO` of the transaction KSDS into `TRANSACT.BKUP(+1)` | none: `transactions` is read in place and no backup is written (CBTRN03C-003, X-TRANREPT-2) |
//! | `STEP05R` (`SORT`) | `INCLUDE` on `TRAN-PROC-DT` inside the window, `SORT FIELDS=(TRAN-CARD-NUM,A)`, into `TRANSACT.DALY(+1)` | `TransactionRepository::stream_processed_between` over the typed window: one statement, in card-number order, whose rows are read one at a time (CBTRN03C-003 to CBTRN03C-005) |
//! | `STEP10R` | `CBTRN03C` over the sorted file, `CARDXREF`, `TRANTYPE`, `TRANCATG` and `DATEPARM`, writing `TRANREPT(+1)` | `domain::batch::cbtrn03c::Tranrept` over the selected rows, writing `TRANREPT.G<nnnn>` under `Config::reports_dir` (CBTRN03C-032) |
//!
//! # Flow
//!
//! 1. `--start-date` and `--end-date` become the DATEPARM window, two typed dates (CBTRN03C-001,
//!    CBTRN03C-002; Typed Dates and Timestamps). The one window stands for both the SORT
//!    constants and the DATEPARM record (X-TRANREPT-1). Once it is accepted the start line is
//!    shown, before any table is read (CBTRN03C-006).
//! 2. The inputs are opened in the program's order (CBTRN03C-007): the selection of
//!    `transactions` (`TRANFILE`), the generation file (`REPTFILE`), then the reads of
//!    `card_xref` (`CARDXREF`), `transaction_types` (`TRANTYPE`) and `transaction_categories`
//!    (`TRANCATG`). Every read is read-only; the job writes no table (CBTRN03C-003). The
//!    selection opens with the read of its first row, so a statement that fails ends the job
//!    before the generation file exists. Its rows hold one pooled connection until the loop
//!    ends, so the lookup tables are read on other connections of the pool. The DATEPARM record
//!    is built in memory, so its read always succeeds and shows the `Reporting from` line
//!    (CBTRN03C-008, CBTRN03C-009).
//! 3. Each selected row is read when the loop reaches it, as `1000-TRANFILE-GET-NEXT` reads the
//!    next record (CBTRN03C-010), and goes to `Tranrept::process` in the order the repository
//!    returns it; the job neither filters nor sorts again. Rows of one card come in
//!    transaction-ID order, the proposed resolution of halt candidate X-TRANREPT-H1, which
//!    claims no parity. The loop ends at the first `Flow::Stop`, as `NEXT SENTENCE` leaves the
//!    COBOL loop, and reads no further row (CBTRN03C-011, X-TRANREPT-9); the window being the
//!    selection's own, no selected row reaches it.
//! 4. The records each pass gives are taken with `Tranrept::take_records` and written at once,
//!    and those `Tranrept::finish` gives after the loop are written last, each as its 133 bytes
//!    and LF (CBTRN03C-032). Neither the selection nor the report is ever held whole
//!    (W011-F06): the selection is the repository's one statement over the window, whose rows
//!    are decoded one at a time as they are read. The last amount counted twice at end of file
//!    and the missing account total of the last card are the domain's, reproduced as audited
//!    and never corrected here (CBTRN03C-030, CBTRN03C-031, X-TRANREPT-7; Behaviour
//!    constraint).
//! 5. A normal end shows the end line once the generation is complete and returns code 0 and the
//!    generation path, which `main` prints after it (CBTRN03C-032, CBTRN03C-034).
//!
//! # Abends
//!
//! Every failure path ends as `9999-ABEND-PROGRAM` does: its job-log lines, then
//! `ABENDING PROGRAM`, whose event carries the abend code 999 the program passes to `CEE3ABD`,
//! then the job's [`JobError`] (CBTRN03C-035). The unfinished generation file is then removed,
//! as `DISP=(NEW,CATLG,DELETE)` deletes it at abend, so no path and no end line are shown and the
//! next run takes the same generation number. No table is written, so nothing is rolled back
//! (CBTRN03C-003).
//!
//! | Failure | Lines before `ABENDING PROGRAM` | Rule |
//! |---|---|---|
//! | the selection's statement or its first row cannot be read | `ERROR OPENING TRANFILE`, status | CBTRN03C-007 |
//! | the generation cannot be allocated or created | `ERROR OPENING REPTFILE`, status | CBTRN03C-007 |
//! | `card_xref` cannot be read | `ERROR OPENING CROSS REF FILE`, status | CBTRN03C-007 |
//! | `transaction_types` cannot be read | `ERROR OPENING TRANSACTION TYPE FILE`, status | CBTRN03C-007 |
//! | `transaction_categories` cannot be read | `ERROR OPENING TRANSACTION CATG FILE`, status | CBTRN03C-007 |
//! | a later row of the selection cannot be fetched or decoded | `ERROR READING TRANSACTION FILE`, status | CBTRN03C-010 |
//! | a report record cannot be written | `ERROR WRITING REPTFILE`, status | CBTRN03C-033 |
//! | the generation cannot be flushed or synced | `ERROR CLOSING REPORT FILE`, status | CBTRN03C-034 |
//! | a card, type or category key missing from its table | the domain's `INVALID ...` line and `FILE STATUS IS: NNNN0023` | CBTRN03C-014, CBTRN03C-016, CBTRN03C-018 |
//! | a card number that is not 16 digits | none (X-TRANREPT-H4) | CBTRN03C-035 |
//!
//! - A failed database or file call first adds one event naming the file operation it replaces
//!   and the error: an SQLSTATE, a constraint or a column, or a static step and an I/O error
//!   kind, never a value or a path. A failed generation open also names the setting of the
//!   directory, `reports under CARDDEMO_VAR_DIR`.
//! - The status line is `FILE STATUS IS: NNNN` and four characters (CBTRN03C-036).
//!   `HALTED(X-TRANREPT-H3)`: a `PostgreSQL` or file-system failure has no file status, so the
//!   job shows `0030`, the permanent-error status, as a stand-in that is no parity result.
//! - Those paths end with [`JobError::Abend`], whose reason is the failure line; the lookup
//!   abends end with [`JobError::Report`]. Neither carries a return code, as `CEE3ABD` sets none,
//!   so the exit status is 1, a value X-TRANREPT-4 leaves to the reviewer.
//! - The lookup tables are read whole when they are opened, so a failed read of one is that
//!   open's failure, before the first record. During the loop a lookup can only miss its key.
//!   The COBOL's fall-through for a lookup read that fails otherwise (CBTRN03C-040 to
//!   CBTRN03C-042, X-TRANREPT-10) is `HALTED(X-TRANREPT-H5)` and has no counterpart here.
//!
//! # Data versions (E23)
//!
//! The type and category descriptions come only from the VSAM data version, `transaction_types`
//! and `transaction_categories`, which the `trantype` and `trancatg` loads replace. No repository
//! of schema `db2` is used here, so a `mnttrdb2` change never reaches a report line
//! (CBTRN03C-038).
//!
//! # Job log and cardholder data
//!
//! Each `DISPLAY` the program performs is one tracing event whose message is the line's text
//! exactly, in program order: `INFO` for the lines of a run and `ERROR` for the lines a failure
//! path adds (see Abends). The events reach standard output as the JSON stream of
//! `runtime::telemetry`, so the stdout lines that are not JSON stay the paths `main` prints from
//! [`JobOutcome::written`] (AAP 0.3.4.3, 0.3.4.7). A run shows:
//!
//! 1. `START OF EXECUTION OF PROGRAM CBTRN03C`, before any table is read; a refused window shows
//!    nothing (CBTRN03C-002, CBTRN03C-006).
//! 2. `Reporting from <start> to <end>`, once the inputs are open (CBTRN03C-008).
//! 3. For each row the window selects, its `TRAN-RECORD` line, before the row is reported; a row
//!    outside the window shows none (CBTRN03C-011, CBTRN03C-012).
//! 4. At end of file after a record, the `TRAN-AMT ` and `WS-PAGE-TOTAL` lines, before the
//!    final totals are written (CBTRN03C-029).
//! 5. `END OF EXECUTION OF PROGRAM CBTRN03C`, once the generation is complete, the last line of a
//!    normal end (CBTRN03C-034).
//!
//! Two lines carry record data, both built by the domain: the `TRAN-RECORD` line, whose card
//! number is redacted and whose six free-text fields show as `*` whatever they hold, and the
//! `INVALID CARD NUMBER : ` line, whose card number is redacted (CBTRN03C-012, CBTRN03C-014;
//! X-TRANREPT-3). The rest of the `TRAN-RECORD` line is the row's encoding, except that the job
//! shows any 16 bytes of it that equal a `card_xref` card number or the row's own, such as a
//! transaction ID that is one, in the redacted form too; the line keeps its 350 bytes. Every
//! other line is a fixed literal, the window's dates or an amount, and a failure's event carries
//! an SQLSTATE, a constraint or an I/O error kind only. Report records have no card field
//! (CBTRN03C-023), and the generation is a local file, not egress. The job's errors are the
//! fixed texts of [`JobError`] and its counts are plain numbers, so nothing the job emits carries
//! a full card number, an SSN or a password (CBTRN03C-039; Cardholder Data Redaction).

use std::collections::HashSet;

use copybook::PicX;
use domain::batch::cbtrn03c::{
    ABEND_LINE, CLOSE_REPTFILE_ERROR, DateParm, DateParmError, END_LINE, FILE_STATUS_LABEL, Flow,
    OPEN_CARDXREF_ERROR, OPEN_REPTFILE_ERROR, OPEN_TRANCATG_ERROR, OPEN_TRANFILE_ERROR,
    OPEN_TRANTYPE_ERROR, READ_TRANFILE_ERROR, ReportError, ReportRecord, START_LINE, Tranrept,
    WRITE_REPTFILE_ERROR, record_display, selects,
};
use domain::records::{
    CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
};
use domain::sensitive::CardNumber;
use persistence::{
    CardXrefRepository, PersistenceError, TransactionCategoryRepository, TransactionRepository,
    TransactionRows, TransactionTypeRepository,
};
use runtime::telemetry::JobCounter;

use super::{JobContext, JobError, JobOutcome, OutputFile, REPORTS_DIR_SETTING, count};
use crate::cli::TranreptArgs;

/// The generation base of the report, `AWS.M2.CARDDEMO.TRANREPT(+1)` of `TRANREPT.jcl:80`
/// (CBTRN03C-032).
const REPORT_BASE: &str = "TRANREPT";

/// `MOVE 999 TO ABCODE` before `CALL 'CEE3ABD'` (`CBTRN03C.cbl:629-630`; CBTRN03C-035): the
/// abend code the `ABENDING PROGRAM` event carries, where X-TRANREPT-4 proposes to record it.
const ABEND_CODE: i32 = 999;

/// The four status characters of the status line after the failure of a database or file call:
/// `00` and `30`, a permanent error. `HALTED(X-TRANREPT-H3)`: such a failure has no file status,
/// so this is a stand-in, never a parity result (CBTRN03C-036).
const STAND_IN_STATUS: &str = "0030";

/// Bytes of a card number, `PIC X(16)` (PIC-024).
const CARD_LEN: usize = 16;

/// The leading bytes of a card number that its redacted form shows as `*`: all but the last four
/// (PIC-024; Cardholder Data Redaction).
const CARD_MASKED_LEN: usize = 12;

/// The character the redacted form of a card number shows for each masked byte.
const CARD_MASK: u8 = b'*';

// The file operation each database or file call replaces, named in the event that reports its
// failure.

/// `0000-TRANFILE-OPEN` and the read of the selection's first row, done at once.
const OPEN_TRANFILE_OP: &str = "OPEN TRANFILE";
/// `0100-REPTFILE-OPEN`.
const OPEN_REPTFILE_OP: &str = "OPEN REPTFILE";
/// `0200-CARDXREF-OPEN` and every keyed read of `1500-A-LOOKUP-XREF`, done at once.
const OPEN_CARDXREF_OP: &str = "OPEN CARDXREF";
/// `0300-TRANTYPE-OPEN` and every keyed read of `1500-B-LOOKUP-TRANTYPE`, done at once.
const OPEN_TRANTYPE_OP: &str = "OPEN TRANTYPE";
/// `0400-TRANCATG-OPEN` and every keyed read of `1500-C-LOOKUP-TRANCATG`, done at once.
const OPEN_TRANCATG_OP: &str = "OPEN TRANCATG";
/// The `READ` of `1000-TRANFILE-GET-NEXT` after the first row.
const READ_TRANFILE_OP: &str = "READ TRANFILE";
/// The `WRITE` of `1111-WRITE-REPORT-REC`.
const WRITE_REPTFILE_OP: &str = "WRITE REPTFILE";
/// `9100-REPTFILE-CLOSE`.
const CLOSE_REPTFILE_OP: &str = "CLOSE REPTFILE";

/// Runs TRANREPT over the window of `args`: writes the next `TRANREPT` generation under
/// `Config::reports_dir` and returns its path, return code 0, and the counts `records` (rows
/// read) and `report_lines` (records written).
///
/// # Errors
///
/// - [`JobError::Abend`] for a window `DateParm::new` refuses, before any table is read and
///   before the start line (CBTRN03C-002); a command line `clap` parsed never gives one.
/// - [`JobError::Abend`] whose reason is the failure line, after the failure's event, that line,
///   the status line and `ABENDING PROGRAM` (module documentation, Abends): when the selection's
///   statement or its first row, the generation file or a lookup table cannot be read or created
///   (CBTRN03C-007), a later row of the selection cannot be fetched or decoded once the records
///   of the rows before it are written (CBTRN03C-010), a report record cannot be written
///   (CBTRN03C-033), or the generation file cannot be flushed or synced (CBTRN03C-034).
/// - [`JobError::Report`] for the lookup abends of `CBTRN03C`, after the domain's job-log lines
///   and `ABENDING PROGRAM`: a card, type or category key missing from its table, or a card
///   number that is not 16 digits (CBTRN03C-014, CBTRN03C-016, CBTRN03C-018, CBTRN03C-035).
///
/// After any error the generation file is removed and no end line is shown (CBTRN03C-035).
pub(crate) async fn run(ctx: &JobContext<'_>, args: &TranreptArgs) -> Result<JobOutcome, JobError> {
    let parm = args.date_parm().map_err(window_error)?;
    tracing::info!("{START_LINE}");
    // Each store is a handle on the shared pool, cloned for the `&mut self` repository calls.
    // The selection's rows borrow theirs, and hold one pooled connection, until the loop ends,
    // so the lookup tables are read through a second handle, on other connections.
    let mut selection = ctx.store.clone();
    let mut rows = selection
        .stream_processed_between(parm.start(), parm.end())
        .await
        .map_err(|error| database_failure(OPEN_TRANFILE_OP, OPEN_TRANFILE_ERROR, &error))?;
    // `TRANFILE` opens with the selection's first row, before `REPTFILE`: a statement that fails
    // ends the job before any generation file exists (CBTRN03C-007).
    let first = rows
        .next()
        .await
        .map_err(|error| database_failure(OPEN_TRANFILE_OP, OPEN_TRANFILE_ERROR, &error))?;
    // The event names the setting of the directory, never its path: the job returns its abend,
    // so `main` cannot name it in the error.
    let mut output = OutputFile::next_generation(&ctx.config.reports_dir(), REPORT_BASE)
        .await
        .map_err(|error| {
            let error = error.naming_output_dir(REPORTS_DIR_SETTING);
            file_failure(OPEN_REPTFILE_OP, OPEN_REPTFILE_ERROR, &error)
        })?;
    let mut store = ctx.store.clone();
    let xrefs = CardXrefRepository::find_all(&mut store)
        .await
        .map_err(|error| database_failure(OPEN_CARDXREF_OP, OPEN_CARDXREF_ERROR, &error))?;
    let types = TransactionTypeRepository::find_all(&mut store)
        .await
        .map_err(|error| database_failure(OPEN_TRANTYPE_OP, OPEN_TRANTYPE_ERROR, &error))?;
    let categories = TransactionCategoryRepository::find_all(&mut store)
        .await
        .map_err(|error| database_failure(OPEN_TRANCATG_OP, OPEN_TRANCATG_ERROR, &error))?;
    // `0550-DATEPARM-READ`: the record built from the window always exists, so the read ends
    // with status `00` and shows the window (CBTRN03C-008, CBTRN03C-009).
    tracing::info!("{}", parm.reporting_line());
    let (read, report_lines) = report(
        parm,
        first,
        &mut rows,
        xrefs,
        types,
        categories,
        &mut output,
    )
    .await?;
    // A loop left at `Flow::Stop` reads no further row: the read ends here and its connection
    // returns to the pool before the generation is finished.
    drop(rows);
    let path = output
        .finish()
        .await
        .map_err(|error| file_failure(CLOSE_REPTFILE_OP, CLOSE_REPTFILE_ERROR, &error))?;
    tracing::info!("{END_LINE}");
    Ok(JobOutcome {
        written: vec![path],
        return_code: 0,
        counts: vec![
            (JobCounter::Records, count(read)),
            (JobCounter::ReportLines, count(report_lines)),
        ],
    })
}

/// The read loop of `CBTRN03C` over the selection, in its order (`CBTRN03C.cbl:170-206`):
/// `first`, the row read when the selection opened, then each row `rows` reads when the loop
/// reaches it (CBTRN03C-010), is passed to `Tranrept::process` until the first `Flow::Stop`,
/// which reads no further row (CBTRN03C-011), and the records each pass gives are written to
/// `output` at once, as the program writes them; then the records `Tranrept::finish` gives are
/// written (CBTRN03C-032). No more of the selection than one row, and of the report than one
/// pass's records, is held (W011-F06). Returns the number of rows passed, the stopping one
/// included, and the number of records written.
///
/// A row the window selects shows its `TRAN-RECORD` line before it is reported, as the program
/// displays it after the window test and before the lookups (CBTRN03C-012); a row outside the
/// window shows none (CBTRN03C-011). At end of file the two lines of CBTRN03C-029 are shown
/// before the final totals are written.
///
/// # Errors
///
/// [`JobError::Report`] for the `ReportError` that abended the run, after its job-log lines and
/// `ABENDING PROGRAM` (CBTRN03C-035), and [`JobError::Abend`] after the failure's lines when the
/// next row cannot be fetched or decoded (CBTRN03C-010) or a record cannot be written
/// (CBTRN03C-033).
async fn report(
    parm: DateParm,
    first: Option<TransactionRecord>,
    rows: &mut TransactionRows<'_>,
    xrefs: Vec<CardXrefRecord>,
    types: Vec<TransactionTypeRecord>,
    categories: Vec<TransactionCategoryRecord>,
    output: &mut OutputFile,
) -> Result<(usize, usize), JobError> {
    // The card numbers every `TRAN-RECORD` line redacts wherever they appear, taken from the
    // cross-reference once, before the first line is shown.
    let held: HashSet<CardNumber> = xrefs.iter().map(|xref| xref.card_num.clone()).collect();
    let mut run = Tranrept::new(parm, xrefs, types, categories);
    let mut read = 0;
    let mut written = 0;
    let mut next = first;
    while let Some(row) = next {
        read += 1;
        if selects(&row, &parm) {
            tracing::info!("{}", record_line(&row, &held));
        }
        let flow = run.process(&row).map_err(lookup_abend)?;
        written += write_records(output, run.take_records()).await?;
        if flow == Flow::Stop {
            break;
        }
        next = rows
            .next()
            .await
            .map_err(|error| database_failure(READ_TRANFILE_OP, READ_TRANFILE_ERROR, &error))?;
    }
    if let Some(lines) = run.end_of_file_display() {
        for line in lines {
            tracing::info!("{line}");
        }
    }
    written += write_records(output, run.finish().map_err(lookup_abend)?).await?;
    Ok((read, written))
}

/// Writes `records` to `output`, each as its 133 bytes and LF (CBTRN03C-032), and returns how
/// many were written.
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR WRITING REPTFILE`, after the failure's lines, when a record
/// cannot be written (CBTRN03C-033).
async fn write_records(
    output: &mut OutputFile,
    records: Vec<ReportRecord>,
) -> Result<usize, JobError> {
    for record in &records {
        output
            .write_record(record)
            .await
            .map_err(|error| file_failure(WRITE_REPTFILE_OP, WRITE_REPTFILE_ERROR, &error))?;
    }
    Ok(records.len())
}

/// The `TRAN-RECORD` job-log line of `row`: the domain's 350 bytes, its card number redacted and
/// its free-text fields masked, and any other 16 bytes equal to a card number of `held` or to the
/// row's own redacted too ([`redact_held_cards`]), one character per byte (CBTRN03C-012,
/// CBTRN03C-039).
fn record_line(row: &TransactionRecord, held: &HashSet<CardNumber>) -> String {
    let mut line = record_display(row);
    redact_held_cards(&mut line, held, &row.card_num);
    line.into_iter().map(char::from).collect()
}

/// Shows in the redacted form, twelve `*` and the last four digits, every 16 bytes of `line` that
/// equal `own` or a card number of `held`. The matches are all found on `line` as given before
/// any byte is masked, so two that overlap leave no more than the last four digits of either.
/// The line keeps its length.
///
/// Cardholder Data Redaction: CBTRN03C-012 shows bytes 1-22, 133-152 and 279-330 of the line as
/// the row's encoding, but nothing keeps a card number out of them (a transaction ID can be one)
/// and the rule allows no full card number in a job-log event. Only such 16 bytes depart from
/// the row's encoding; every other byte is the domain's.
fn redact_held_cards(line: &mut [u8], held: &HashSet<CardNumber>, own: &CardNumber) {
    let starts: Vec<usize> = line
        .windows(CARD_LEN)
        .enumerate()
        .filter(|(_, window)| {
            <[u8; CARD_LEN]>::try_from(*window)
                .map(|bytes| CardNumber::new(PicX::from_array(bytes)))
                .is_ok_and(|card| card == *own || held.contains(&card))
        })
        .map(|(start, _)| start)
        .collect();
    for start in starts {
        if let Some(masked) = line.get_mut(start..start + CARD_MASKED_LEN) {
            masked.fill(CARD_MASK);
        }
    }
}

/// The failure of repository call `operation` on the path that displays `line`: the event
/// carrying the error, whose text is an SQLSTATE, a constraint or a column only (Cardholder Data
/// Redaction), then the path's lines and the abend.
fn database_failure(
    operation: &'static str,
    line: &'static str,
    error: &PersistenceError,
) -> JobError {
    tracing::error!(operation, error = %error, "CBTRN03C database call failed");
    abend(line)
}

/// The failure of local-file call `operation` on the path that displays `line`: the event
/// carrying the error, whose text is a static step and the I/O error kind, or the generation
/// limit and the `TRANREPT` base, never the path; then the path's lines and the abend.
fn file_failure(operation: &'static str, line: &'static str, error: &JobError) -> JobError {
    tracing::error!(operation, error = %error, "CBTRN03C file operation failed");
    abend(line)
}

/// `9999-ABEND-PROGRAM` after the failure line `line`: `line`, the status line of
/// `9910-DISPLAY-IO-STATUS` with the stand-in status (CBTRN03C-036, X-TRANREPT-H3), then
/// `ABENDING PROGRAM` and the abend, with no return code (CBTRN03C-035, X-TRANREPT-4). The caller
/// returns the error at once, which drops the unfinished generation and so removes it.
fn abend(line: &'static str) -> JobError {
    tracing::error!("{line}");
    tracing::error!("{FILE_STATUS_LABEL}{STAND_IN_STATUS}");
    show_abend();
    JobError::Abend {
        code: None,
        reason: line,
    }
}

/// The abend of a lookup that misses its key, or of a card number that is not 16 digits: the
/// domain's `INVALID ...` and status lines, which show the card number redacted
/// (CBTRN03C-014, CBTRN03C-016, CBTRN03C-018), none for `HALTED(X-TRANREPT-H4)`, then
/// `ABENDING PROGRAM` and [`JobError::Report`], whose text names the failed lookup only
/// (CBTRN03C-035).
fn lookup_abend(error: ReportError) -> JobError {
    if let Some(lines) = error.job_log_lines() {
        for line in lines {
            tracing::error!("{line}");
        }
    }
    show_abend();
    JobError::Report(error)
}

/// `DISPLAY 'ABENDING PROGRAM'`, the last job-log line of every failure path, carrying the
/// abend code `CEE3ABD` is called with (CBTRN03C-035, X-TRANREPT-4).
fn show_abend() {
    tracing::error!(abend_code = ABEND_CODE, "{ABEND_LINE}");
}

/// The [`JobError`] of a window the domain refuses (CBTRN03C-002). `JobError` has no variant of
/// its own for it, because the date options' parser already refuses every such date, so the
/// unreachable case ends the job on the abend path with a fixed text naming the option, never
/// its value.
fn window_error(error: DateParmError) -> JobError {
    JobError::Abend {
        code: None,
        reason: match error {
            DateParmError::Start => "CBTRN03C: the start date is not a valid YYYY-MM-DD date",
            DateParmError::End => "CBTRN03C: the end date is not a valid YYYY-MM-DD date",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use copybook::{FixedRecord, PicTimestamp, PicU, PicX};
    use domain::batch::cbtrn03c::{DateParm, DateParmError, REPORT_RECORD_LEN, record_display};
    use domain::money::{Decimal, Money};
    use domain::records::{
        CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
    };
    use domain::sensitive::CardNumber;
    use persistence::{PersistenceError, TransactionRows};

    use super::{OutputFile, REPORT_BASE, record_line, report, window_error};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Tells apart the scratch directories of the tests of one process.
    static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

    /// A directory of its own under the system temporary directory, removed with its content
    /// when dropped: the reports directory of one run.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Result<Self, Box<dyn Error>> {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "carddemo-batch-tranrept-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The report records of a finished generation, without their LF, after checking that
    /// each is 133 bytes followed by LF (CBTRN03C-032).
    fn report_records(bytes: &[u8]) -> Result<Vec<&[u8]>, Box<dyn Error>> {
        let lines = bytes.chunks_exact(REPORT_RECORD_LEN + 1);
        if !lines.remainder().is_empty() {
            return Err("the report ends in a partial record".into());
        }
        lines
            .map(|line| match line.split_at(REPORT_RECORD_LEN) {
                (record, b"\n") => Ok(record),
                _ => Err("a report record is not followed by LF".into()),
            })
            .collect()
    }

    /// A synthetic card number, in no fixture file.
    const CARD: [u8; 16] = *b"4000000000000001";

    /// A processing timestamp inside the default window, layout `D` (PIC-019).
    const INSIDE: &[u8; 26] = b"2022-07-01-10.15.42.370000";

    /// A processing timestamp the day after the default window ends.
    const OUTSIDE: &[u8; 26] = b"2022-07-07-00.00.00.000000";

    /// A purchase of 183.88 on [`CARD`] with transaction ID `n`, processed at `proc_ts`.
    fn purchase(n: u64, proc_ts: &[u8; 26]) -> Result<TransactionRecord, Box<dyn Error>> {
        Ok(TransactionRecord {
            id: PicX::from_text(&format!("{n:016}"))?,
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            source: PicX::from_text("POS TERM")?,
            desc: PicX::from_text("Purchase at Abshire-Lowe")?,
            amt: Money::new(Decimal::new(18_388, 2))?,
            merchant_id: PicU::new(800_000_000)?,
            merchant_name: PicX::from_text("Abshire-Lowe")?,
            merchant_city: PicX::from_text("North Enoshaven")?,
            merchant_zip: PicX::from_text("72112")?,
            card_num: CardNumber::new(PicX::from_array(CARD)),
            orig_ts: PicTimestamp::decode(b"2022-06-10 19:27:53.000000")?,
            proc_ts: PicTimestamp::decode(proc_ts)?,
            filler: PicX::spaces(),
        })
    }

    /// The rows of the three lookup tables a run is built with.
    struct Lookups {
        xrefs: Vec<CardXrefRecord>,
        types: Vec<TransactionTypeRecord>,
        categories: Vec<TransactionCategoryRecord>,
    }

    /// One run of [`report`]: the processing timestamps of its rows, which are numbered from 1,
    /// the rows it reads, the records it returns and how the last record begins.
    struct Case {
        stamps: &'static [&'static [u8; 26]],
        read: usize,
        records: usize,
        last_prefix: &'static [u8],
    }

    /// The lookup rows of [`CARD`] (account 50) and of type `01`, category `0001`, as
    /// `cardxref.txt`, `trantype.txt:1` and `trancatg.txt:1` describe them.
    fn lookups() -> Result<Lookups, Box<dyn Error>> {
        let xref = CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(50)?,
            acct_id: PicU::new(50)?,
            filler: PicX::spaces(),
        };
        let tran_type = TransactionTypeRecord {
            tran_type: PicX::from_array(*b"01"),
            desc: PicX::from_text("Purchase")?,
            filler: PicX::from_array(*b"00000000"),
        };
        let category = TransactionCategoryRecord {
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            type_desc: PicX::from_text("Regular Sales Draft")?,
            filler: PicX::from_array(*b"0000"),
        };
        Ok(Lookups {
            xrefs: vec![xref],
            types: vec![tran_type],
            categories: vec![category],
        })
    }

    /// Rows read and records written, counted and in the generation file: a run over two rows
    /// ends with the end-of-file totals, while a row outside the window ends the loop, so the
    /// row after it is never read and no total is written. The records are the header group
    /// (4), the details, then the page total, its dashes and the grand total.
    // audit: CBTRN03C-011
    // audit: CBTRN03C-030
    // audit: CBTRN03C-032
    #[tokio::test]
    async fn rows_are_read_until_the_loop_is_left() -> TestResult {
        let cases = [
            Case {
                stamps: &[INSIDE, INSIDE],
                read: 2,
                records: 9,
                last_prefix: b"Grand Total",
            },
            Case {
                stamps: &[INSIDE, OUTSIDE, INSIDE],
                read: 2,
                records: 5,
                last_prefix: b"0000000000000001 00000000050 01-Purchase",
            },
        ];
        for case in cases {
            let stamps = case.stamps.len();
            let mut rows = TransactionRows::from_records(
                case.stamps
                    .iter()
                    .zip(1..)
                    .map(|(stamp, n)| purchase(n, stamp))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            let first = rows.next().await?;
            let Lookups {
                xrefs,
                types,
                categories,
            } = lookups()?;
            let scratch = Scratch::new()?;
            let mut output = OutputFile::next_generation(scratch.path(), REPORT_BASE).await?;
            let (read, written) = report(
                DateParm::DEFAULT,
                first,
                &mut rows,
                xrefs,
                types,
                categories,
                &mut output,
            )
            .await?;
            let bytes = fs::read(output.finish().await?)?;
            let records = report_records(&bytes)?;
            assert_eq!(read, case.read, "{stamps} rows");
            assert_eq!(written, case.records, "{stamps} rows");
            assert_eq!(records.len(), case.records, "{stamps} rows");
            let last = records.last().ok_or("no report record")?;
            assert!(last.starts_with(case.last_prefix), "{stamps} rows");
        }
        Ok(())
    }

    /// A missing cross-reference ends the run with the domain's error, whose job text names the
    /// failed lookup only, and the generation is removed once the run drops it.
    // audit: CBTRN03C-014
    // audit: CBTRN03C-035
    #[tokio::test]
    async fn a_missing_cross_reference_abends_the_run() -> TestResult {
        let mut rows = TransactionRows::from_records(Vec::new());
        let Lookups {
            types, categories, ..
        } = lookups()?;
        let scratch = Scratch::new()?;
        let mut output = OutputFile::next_generation(scratch.path(), REPORT_BASE).await?;
        let error = report(
            DateParm::DEFAULT,
            Some(purchase(1, INSIDE)?),
            &mut rows,
            Vec::new(),
            types,
            categories,
            &mut output,
        )
        .await
        .err()
        .ok_or("a card with no cross-reference was reported")?;
        assert_eq!(
            error.to_string(),
            "CBTRN03C: card number not in the card cross-reference"
        );
        drop(output);
        assert_eq!(
            fs::read_dir(scratch.path())?.count(),
            0,
            "the generation remains"
        );
        Ok(())
    }

    /// A row that cannot be fetched after the first ends the run on the abend path of
    /// `1000-TRANFILE-GET-NEXT` once the first row's records are written, with no return code;
    /// the row after the failure is never read, and the generation is removed once the run drops
    /// it.
    // audit: CBTRN03C-010
    // audit: CBTRN03C-035
    #[tokio::test]
    async fn a_row_that_cannot_be_fetched_abends_the_run() -> TestResult {
        let mut rows = TransactionRows::from_results(vec![
            Ok(purchase(1, INSIDE)?),
            Err(PersistenceError::Unavailable),
            Ok(purchase(3, INSIDE)?),
        ]);
        let first = rows.next().await?;
        let Lookups {
            xrefs,
            types,
            categories,
        } = lookups()?;
        let scratch = Scratch::new()?;
        let mut output = OutputFile::next_generation(scratch.path(), REPORT_BASE).await?;
        let error = report(
            DateParm::DEFAULT,
            first,
            &mut rows,
            xrefs,
            types,
            categories,
            &mut output,
        )
        .await
        .err()
        .ok_or("a failed fetch was reported")?;
        assert_eq!(error.to_string(), "ABEND: ERROR READING TRANSACTION FILE");
        assert_eq!(error.exit_code(), None);
        assert!(
            rows.next().await?.is_none(),
            "a row after the failure was read"
        );
        drop(output);
        assert_eq!(
            fs::read_dir(scratch.path())?.count(),
            0,
            "the generation remains"
        );
        Ok(())
    }

    /// A refused window ends the job on the abend path with no return code of its own and a
    /// text that names the option only.
    // audit: CBTRN03C-002
    #[test]
    fn refused_windows_name_the_option_only() {
        let cases = [
            (
                DateParmError::Start,
                "ABEND: CBTRN03C: the start date is not a valid YYYY-MM-DD date",
            ),
            (
                DateParmError::End,
                "ABEND: CBTRN03C: the end date is not a valid YYYY-MM-DD date",
            ),
        ];
        for (error, text) in cases {
            let job_error = window_error(error);
            assert_eq!(job_error.to_string(), text);
            assert_eq!(job_error.exit_code(), None);
        }
    }

    /// Bytes 1-22 of the rows of [`held_card_numbers_are_redacted_anywhere_in_the_line`],
    /// `TRAN-ID`, `TRAN-TYPE-CD` and `TRAN-CAT-CD`: the digits `0` to `9` in turn, so that each
    /// 16 of them are a synthetic card number in no fixture file.
    fn leading_digits() -> [u8; 22] {
        let mut digits = [b'0'; 22];
        for (digit, n) in digits.iter_mut().zip(0_u8..) {
            *digit = b'0' + n % 10;
        }
        digits
    }

    /// A card number of a case: [`CARD`] for `None`, else the 16 of [`leading_digits`] from
    /// offset `at`.
    fn case_card(at: Option<usize>) -> Result<CardNumber, Box<dyn Error>> {
        let bytes = match at {
            None => CARD,
            Some(start) => <[u8; 16]>::try_from(
                leading_digits()
                    .get(start..start + 16)
                    .ok_or("a card number past byte 22")?,
            )?,
        };
        Ok(CardNumber::new(PicX::from_array(bytes)))
    }

    /// One row of [`held_card_numbers_are_redacted_anywhere_in_the_line`], whose bytes 1-22 are
    /// [`leading_digits`]: its own card number and the ones `card_xref` holds, as [`case_card`]
    /// names them, and the 0-based bytes of the domain's line that show as `*` besides.
    struct RedactionCase {
        own: Option<usize>,
        held: &'static [Option<usize>],
        masked: Option<(usize, usize)>,
    }

    /// The `TRAN-RECORD` line is the domain's but that every 16 bytes equal to a card number the
    /// run holds show twelve `*` and its last four digits: one that spans `TRAN-ID`, the type
    /// and the category; the row's own, which `card_xref` lacks; two that overlap, neither then
    /// showing more than its last four digits. A transaction ID that is no such number shows as
    /// stored.
    // audit: CBTRN03C-012
    // audit: CBTRN03C-039
    #[test]
    fn held_card_numbers_are_redacted_anywhere_in_the_line() -> TestResult {
        let cases = [
            RedactionCase {
                own: None,
                held: &[None, Some(6)],
                masked: Some((6, 18)),
            },
            RedactionCase {
                own: Some(0),
                held: &[],
                masked: Some((0, 12)),
            },
            RedactionCase {
                own: None,
                held: &[None, Some(0), Some(6)],
                masked: Some((0, 18)),
            },
            RedactionCase {
                own: None,
                held: &[None],
                masked: None,
            },
        ];
        for (case, number) in cases.iter().zip(1..) {
            let mut bytes = purchase(1, INSIDE)?.encode();
            bytes
                .get_mut(..22)
                .ok_or("a record shorter than 22 bytes")?
                .copy_from_slice(&leading_digits());
            let mut row = TransactionRecord::decode(&bytes)?;
            row.card_num = case_card(case.own)?;
            let held = case
                .held
                .iter()
                .map(|&at| case_card(at))
                .collect::<Result<HashSet<_>, _>>()?;
            let mut expected = record_display(&row);
            if let Some((start, end)) = case.masked {
                expected
                    .get_mut(start..end)
                    .ok_or("a line shorter than its masked bytes")?
                    .fill(b'*');
            }
            assert_eq!(
                record_line(&row, &held).into_bytes(),
                expected,
                "case {number}"
            );
        }
        Ok(())
    }
}
