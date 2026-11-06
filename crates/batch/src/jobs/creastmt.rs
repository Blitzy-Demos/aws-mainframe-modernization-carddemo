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
//! order `CBSTM03B` reads them, the customers and the accounts of all cross-references in one
//! statement per file that serves the rule's keyed reads ([`StatementMasters`]), and writes the
//! three files under `CARDDEMO_STATEMENT_DIR`
//! (`Settings::statement_dir`, default `./var/statements/`): `STATEMNT.PS` with the 80-byte
//! `STMTFILE` records, `STATEMNT.HTML` with the 100-byte `HTMLFILE` records, each record followed
//! by `LF`, and `STATEMNT.PS.PDF` with one text line per `STMTFILE` record, dated with the job
//! clock (`CARDDEMO_CLOCK_OVERRIDE` when set), so a rerun over the same records at the same
//! instant writes all three files with the same bytes. The deletion of the
//! previous files, the record encoding, the PDF rendering and the writes run on Tokio's blocking
//! pool ([`blocking`]), inside the job span; only the database reads run on the job's task.
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
//! - 16, setup failure: a failure with no COBOL counterpart, such as the statement lock held by
//!   another job, a statement directory shared with other users (the "Permissions" section), the
//!   database transaction or a statement file that cannot be deleted, rendered or written.
//!
//! `CBSTM03A` opens `STMTFILE` and `HTMLFILE` before it reads anything (P-CBSTM03A-001), and both
//! DDs are `DISP=(NEW,CATLG,DELETE)` [app/jcl/CREASTMT.JCL:L87-96]: an abend deletes what the
//! step wrote. After `STEP030` has run, every failure therefore leaves no statement file.
//!
//! # Statement lock
//!
//! `STEP030` allocates `STATEMNT.PS` and `STATEMNT.HTML` `DISP=(MOD,DELETE,DELETE)` and `STEP040`
//! `DISP=(NEW,CATLG,DELETE)` [app/jcl/CREASTMT.JCL:L66-96], so JES enqueues both datasets
//! exclusively for the whole job and a second CREASTMT never runs beside the first. Here the job
//! takes that enqueue as the lock file `.STATEMNT.lock` in `CARDDEMO_STATEMENT_DIR`, created
//! exclusively and holding the process id (`crate::generations::LockFile`), before `STEP010`,
//! and holds it to the end of the job, the cleanup of a failed write included. Two runs sharing
//! the directory therefore never delete each other's files, publish a mixed `STATEMNT.PS`,
//! `STATEMNT.HTML` and `STATEMNT.PS.PDF` set or clean up each other's output. Where JES would
//! hold the second job until the first ends, a run that finds the lock fails at once with a
//! setup failure (exit 16) naming the lock file: it deletes and writes nothing, and never takes
//! the lock over. A job killed before it could release the lock (`SIGKILL`, a power loss) leaves
//! the file behind, and every later run is refused until an operator removes it, which is safe
//! only while no batch job is running.
//!
//! # Permissions
//!
//! The statements hold customer and account data, so `CARDDEMO_STATEMENT_DIR` and the three
//! files are owner-only, by the policy of `crate::generations` (its "Permissions" section): the
//! directory is created with mode `0700`, or an existing one loses its group and other
//! permission bits, before the lock is taken, and the lock file and the three statement files
//! are created with mode `0600`. A shared directory, writable by others or sticky as `/tmp` is,
//! or one whose permissions cannot be changed, ends the job with a setup failure (exit 16) that
//! names the directory and its mode; nothing is deleted or written.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use carddemo_domain::clock::Clock;
use carddemo_domain::rules::statement::{
    self, Account, AccountId, AccountReader, CardXref, Customer, CustomerId, CustomerReader,
    StatementError, StatementOutput, Transaction, TransactionReader, XrefReader,
};
use carddemo_domain::rules::{PortError, PortResult};
use carddemo_persistence::tx::{Db, PgStore, UnitOfWork};
use chrono::{DateTime, Utc};
use printpdf::lopdf;
use tracing::Span;

use super::{JobContext, JobCounts, count_of, setup_failure, span_count};
use crate::exit::ExitStatus;
use crate::generations::{
    LockFile, blocking, create_private_dir, encode_fixed_lines, write_replacing,
};
use crate::pdf::render_statement;

/// `STMTFILE` `DSN=AWS.M2.CARDDEMO.STATEMNT.PS` [app/jcl/CREASTMT.JCL:L87-91].
const STMT_FILE: &str = "STATEMNT.PS";

/// `HTMLFILE` `DSN=AWS.M2.CARDDEMO.STATEMNT.HTML` [app/jcl/CREASTMT.JCL:L92-96].
const HTML_FILE: &str = "STATEMNT.HTML";

/// The `OUT` dataset of the `%TXT2PDF` conversion of `STATEMNT.PS` [app/jcl/TXT2PDF1.JCL:L38-39].
const PDF_FILE: &str = "STATEMNT.PS.PDF";

/// The statement lock: the JES dataset enqueue on `STATEMNT.PS` and `STATEMNT.HTML`, which
/// CREASTMT holds for the whole job [app/jcl/CREASTMT.JCL:L66-96]. See the module's "Statement
/// lock" section.
const LOCK_FILE: &str = ".STATEMNT.lock";

/// `STMTFILE` `DCB=(LRECL=80,…,RECFM=FB)` [app/jcl/CREASTMT.JCL:L89]. The job holds the rule's
/// records to the DD's record length, so a record of any other width fails the step instead of
/// shifting the file.
const STMT_LRECL: usize = 80;

/// `HTMLFILE` `DCB=(LRECL=100,…,RECFM=FB)` [app/jcl/CREASTMT.JCL:L94].
const HTML_LRECL: usize = 100;

/// The cross-reference records one `XrefReader::xrefs_after` page reads.
const XREF_PAGE: u16 = 500;

/// The PDF date string (ISO 32000-1, 7.9.4) of a UTC instant, in the form printpdf writes:
/// `D:YYYYMMDDHHmmSS+00'00'`.
const PDF_DATE_FORMAT: &str = "D:%Y%m%d%H%M%S+00'00'";

/// The entries of the PDF document information dictionary that hold a date.
const PDF_DATE_ENTRIES: [&str; 2] = ["CreationDate", "ModDate"];

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
/// Sets `read` (the live transactions read) in the job's counts, whose `posted` and `rejected`
/// stay 0, records `written` (the `STMTFILE` records) on the job span, and ends with one event
/// naming the statement count and the three files.
///
/// Normal completion is CBSTM03A 9999-GOBACK — P-CBSTM03A-1264 [app/cbl/CBSTM03A.CBL:L341-342]:
/// neither CBSTM03A nor CBSTM03B sets `RETURN-CODE`, so the step condition code is 0 and the job
/// exits 0 ([`ExitStatus::Success`]).
pub(super) async fn run(ctx: &JobContext, db: &Db) -> ExitStatus {
    match create_statement_files(ctx, db).await {
        Ok(()) => ExitStatus::Success,
        Err(status) => status,
    }
}

/// The steps of [`run`]; the first failing one ends the job with its status.
///
/// The statement lock is taken first, before `STEP010` and so before CREASTMT STEP030 —
/// P-CREASTMT-004 deletes anything. It is held through `STEP010`, `STEP030`, CREASTMT STEP040 —
/// P-CREASTMT-005, the rollback, [`prepare`], [`write_files`] and its [`discard_written`]
/// cleanup, and released when [`write_statement_files`] has returned, on any path. A lock that
/// cannot be taken ends the job before any statement file is deleted or written.
///
/// Taking and releasing the lock, `STEP030` and the preparation and writing of the statement
/// files run on Tokio's blocking pool, inside the job span. A runtime that shuts down before one
/// of them starts ends the job with a setup failure (exit 16), with nothing of that phase
/// deleted or written; a lock whose release cannot run on the pool is released where the
/// runtime drops it.
async fn create_statement_files(ctx: &JobContext, db: &Db) -> Result<(), ExitStatus> {
    let dir = ctx.settings.statement_dir.clone();
    let lock = blocking(move || lock_statement_dir(&dir))
        .await
        .unwrap_or_else(|cancelled| {
            Err(setup_failure(
                "statement lock acquisition did not complete",
                cancelled,
            ))
        })?;
    let written = write_statement_files(ctx, db).await;
    release_statement_lock(lock).await;
    written
}

/// The steps of [`create_statement_files`] that run while it holds the statement lock: `STEP010`,
/// `STEP030`, `STEP040`, the rollback, [`prepare`] and [`write_files`].
///
/// `STEP030` and the preparation and writing of the statement files run on Tokio's blocking
/// pool, inside the job span. The job clock is read once, before the preparation, for the dates
/// of `STATEMNT.PS.PDF` ([`prepare`]).
async fn write_statement_files(ctx: &JobContext, db: &Db) -> Result<(), ExitStatus> {
    let files = StatementFiles::under(&ctx.settings.statement_dir);
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure("statement read unit of work could not begin", err))?;
    let transactions = step010_sort(&mut uow, &ctx.counts).await?;
    let previous = files.clone();
    blocking(move || step030_delete(&previous))
        .await
        .unwrap_or_else(|cancelled| {
            Err(setup_failure(
                "STEP030 statement file delete did not complete",
                cancelled,
            ))
        })?;
    let (output, cards) = step040_cbstm03a(&mut uow, transactions).await?;
    uow.rollback()
        .await
        .map_err(|err| setup_failure("statement read unit of work could not end", err))?;
    let stmt_records = output.stmt_lines.len();
    let html_records = output.html_lines.len();
    let targets = files.clone();
    let created = ctx.clock.now();
    blocking(move || {
        prepare(&output, created).and_then(|prepared| write_files(&targets, &prepared))
    })
    .await
    .unwrap_or_else(|cancelled| {
        Err(setup_failure(
            "statement file preparation and write did not complete",
            cancelled,
        ))
    })?;
    Span::current().record("written", span_count(stmt_records));
    tracing::info!(
        cards,
        stmt_records,
        html_records,
        stmt_file = %files.stmt.display(),
        html_file = %files.html.display(),
        pdf_file = %files.pdf.display(),
        "statements written"
    );
    Ok(())
}

/// CREASTMT STEP010 — P-CREASTMT-002 [app/jcl/CREASTMT.JCL:L44-55], read as CBSTM03B
/// 1000-TRNXFILE-PROC — P-CBSTM03B-002 reads the sorted file: the live transactions by card
/// number then TRAN-ID, `SORT FIELDS=(263,16,CH,A,1,16,CH,A)`, in the order the transaction
/// port returns them.
///
/// At most [`statement::TRNXFILE_READ_LIMIT`] records are read, one past the slots of the
/// CBSTM03A 8500-READTRNX-READ tables (P-CBSTM03A-004): the load over them is the load over the
/// whole file, and a file of more records ends in the capacity stop (P-CBSTM03A-1256, E-33), so
/// no more of the master is held in memory. The records read are set in `read` in `counts`,
/// whose `posted` and `rejected` stay 0: CBSTM03A writes no file record back and turns no
/// transaction away.
///
/// A failed read is reported as the `ERROR READING TRNXFILE` abend the statement rule defines for
/// it (P-CBSTM03A-003), with `read` left 0. It ends the job before `STEP030`, as a failed
/// `STEP010` bypasses the later steps under `COND=(0,NE)`, so the statement files of the previous
/// run stay.
async fn step010_sort(
    uow: &mut UnitOfWork,
    counts: &JobCounts,
) -> Result<Vec<Transaction>, ExitStatus> {
    let transactions = uow
        .store()
        .transactions_by_card(statement::TRNXFILE_READ_LIMIT)
        .await
        .map_err(|err| statement_failure(&StatementError::trnxfile_read_failed(&err)))?;
    counts.set_read(count_of(transactions.len()));
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
///    customer and the account read by key (P-CBSTM03B-004, P-CBSTM03B-005). The keyed reads
///    are served from [`read_masters`], which reads every customer and every account the
///    cross-references name in one statement per file.
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
    let mut masters = read_masters(&mut store, &xrefs).await;
    let output = statement::create_statements(&mut masters, xrefs, &table)
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

/// The outcome of the keyed reads of one master file that a batch read stands for: the
/// outcome of each key, or the failure of the batch read as a whole.
type BatchRead<K, T> = PortResult<BTreeMap<K, PortResult<T>>>;

/// CBSTM03A 2000-CUSTFILE-GET — P-CBSTM03A-1258 through CBSTM03B 3000-CUSTFILE-PROC —
/// P-CBSTM03B-004, and CBSTM03A 3000-ACCTFILE-GET — P-CBSTM03A-1259 through CBSTM03B
/// 4000-ACCTFILE-PROC — P-CBSTM03B-005: the keyed reads of every customer and every account
/// `xrefs` name, each distinct key once, the customers in one statement and then the accounts in
/// another. No statement runs for an empty cross-reference file.
async fn read_masters<'s, 't, 'c>(
    store: &'s mut PgStore<'t, 'c>,
    xrefs: &[CardXref],
) -> StatementMasters<'s, PgStore<'t, 'c>> {
    let customer_ids = xrefs.iter().map(|xref| xref.xref_cust_id).collect();
    let account_ids = xrefs.iter().map(|xref| xref.xref_acct_id).collect();
    let customers = store.read_customers(customer_ids).await;
    let accounts = store.read_accounts(account_ids).await;
    StatementMasters {
        customers,
        accounts,
        store,
    }
}

/// The customer and account masters of CBSTM03A 1000-MAINLINE — P-CBSTM03A-005, read before the
/// statements by [`read_masters`] and served to [`statement::create_statements`] in place of its
/// keyed reads.
///
/// Each plain read returns what the keyed read of CBSTM03A 2000-CUSTFILE-GET — P-CBSTM03A-1258
/// or 3000-ACCTFILE-GET — P-CBSTM03A-1259 returns for the key:
///
/// - the key's entry of its file's batch read: the record, `NotFound` (return code 23) when no
///   record has the key, or the failure of a stored row that does not fit its PIC clause (return
///   code 30), at the cross-reference that names the key;
/// - the failure of the file's batch read, on every read of that file, when that read failed as a
///   whole, so the first read of the file abends with it (return code 30);
/// - `NotFound` for a key no cross-reference names, which the batch read was not asked for.
///
/// The rule still reads the customer before the account of each cross-reference, in
/// cross-reference order, so the first failing read and its file and return code are those of
/// the keyed reads. A locking read is never served from a batch read: it goes to the wrapped
/// store.
struct StatementMasters<'s, S> {
    /// The `CUSTFILE` read of every customer the cross-references name.
    customers: BatchRead<CustomerId, Customer>,
    /// The `ACCTFILE` read of every account the cross-references name.
    accounts: BatchRead<AccountId, Account>,
    /// The store the locking reads go to.
    store: &'s mut S,
}

/// What the keyed read of `key` returns from the batch read `batch`.
fn batch_entry<K: Ord, T: Clone>(batch: &BatchRead<K, T>, key: &K) -> PortResult<T> {
    let entries = batch.as_ref().map_err(PortError::clone)?;
    entries
        .get(key)
        .cloned()
        .unwrap_or(Err(PortError::NotFound))
}

impl<S: CustomerReader + Send> CustomerReader for StatementMasters<'_, S> {
    /// CBSTM03A 2000-CUSTFILE-GET — P-CBSTM03A-1258 through CBSTM03B 3000-CUSTFILE-PROC —
    /// P-CBSTM03B-004: the customer's outcome in the `CUSTFILE` batch read.
    async fn read_customer(&mut self, id: CustomerId) -> PortResult<Customer> {
        batch_entry(&self.customers, &id)
    }

    /// The locking read of the wrapped store; no batch read stands for it.
    async fn read_customer_for_update(&mut self, id: CustomerId) -> PortResult<Customer> {
        self.store.read_customer_for_update(id).await
    }
}

impl<S: AccountReader + Send> AccountReader for StatementMasters<'_, S> {
    /// CBSTM03A 3000-ACCTFILE-GET — P-CBSTM03A-1259 through CBSTM03B 4000-ACCTFILE-PROC —
    /// P-CBSTM03B-005: the account's outcome in the `ACCTFILE` batch read.
    async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
        batch_entry(&self.accounts, &id)
    }

    /// The locking read of the wrapped store; no batch read stands for it.
    async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
        self.store.read_account_for_update(id).await
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
/// records, which `crate::pdf` writes one text object per record with every character kept,
/// dated `created` ([`date_pdf`]).
///
/// Nothing is written until all three are built, so a failure here leaves no statement file.
///
/// # Errors
///
/// A setup failure when a record does not have its file's record length or holds a line break,
/// or when the PDF cannot be rendered or dated. `CBSTM03A` tests no status on either file
/// (P-CBSTM03A-1263), so the audit records no COBOL outcome for these.
fn prepare(output: &StatementOutput, created: DateTime<Utc>) -> Result<PreparedFiles, ExitStatus> {
    let stmt = encode_fixed_lines(&output.stmt_lines, STMT_LRECL)
        .map_err(|err| setup_failure("STMTFILE record does not fit LRECL=80", err))?;
    let html = encode_fixed_lines(&output.html_lines, HTML_LRECL)
        .map_err(|err| setup_failure("HTMLFILE record does not fit LRECL=100", err))?;
    let rendered = render_statement(&output.stmt_lines)
        .map_err(|err| setup_failure("STATEMNT.PS.PDF could not be rendered", err))?;
    let pdf = date_pdf(&rendered, created)
        .map_err(|err| setup_failure("STATEMNT.PS.PDF could not be dated", err))?;
    Ok(PreparedFiles { stmt, html, pdf })
}

/// The rendered statement PDF `pdf` with the `/CreationDate` and `/ModDate` of its document
/// information dictionary set to `created`, the job clock's instant, as a PDF date in UTC
/// ([`PDF_DATE_FORMAT`]).
///
/// printpdf dates a document with the host clock when it builds it, and `crate::pdf` receives
/// the text lines alone, so the job sets the dates here from its clock, which
/// `CARDDEMO_CLOCK_OVERRIDE` fixes: a rerun at the same instant over the same records writes the
/// same bytes. printpdf writes no XMP metadata stream for the document `crate::pdf` builds, so
/// these two entries are its only dates. Only the dictionary changes; the pages, and with them
/// the extracted text, stay as rendered.
///
/// # Errors
///
/// The lopdf error when `pdf` cannot be parsed, holds no information dictionary, or cannot be
/// serialized again.
fn date_pdf(pdf: &[u8], created: DateTime<Utc>) -> Result<Vec<u8>, lopdf::Error> {
    let mut doc = lopdf::Document::load_mem(pdf)?;
    let info = doc.trailer.get(b"Info")?.as_reference()?;
    let date = created.format(PDF_DATE_FORMAT).to_string();
    let entries = doc.get_dictionary_mut(info)?;
    for entry in PDF_DATE_ENTRIES {
        entries.set(entry, lopdf::Object::string_literal(date.as_str()));
    }
    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok(out)
}

/// CREASTMT STEP040 — P-CREASTMT-005: writes the three statement files, each replaced as a
/// whole. The caller holds the statement lock (see the module's "Statement lock" section).
///
/// When one write fails, the files this run already wrote are deleted again, as the abnormal
/// end of `STEP040` deletes its `DISP=(NEW,CATLG,DELETE)` datasets: the lock keeps every other
/// job out from before `STEP030` to the end of the cleanup, so every statement file present
/// after `STEP030` was written by this run.
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

/// CREASTMT STEP040 — P-CREASTMT-005, abnormal end: deletes every statement file still present
/// after a failed write, as `DISP=(NEW,CATLG,DELETE)` deletes the step's datasets. It runs under
/// the statement lock, so the files it deletes are this run's and never another job's. A file
/// that cannot be deleted is logged and left for the next run's `STEP030`.
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

/// Takes the statement lock [`LOCK_FILE`] in the statement directory `dir`, first creating the
/// directory owner-only, or restricting an existing one to its owner, with
/// [`create_private_dir`]: the JES enqueue on `STATEMNT.PS` and `STATEMNT.HTML` that CREASTMT
/// STEP030 — P-CREASTMT-004 (`DISP=(MOD,DELETE,DELETE)`) and STEP040 — P-CREASTMT-005
/// (`DISP=(NEW,CATLG,DELETE)`) allocate [app/jcl/CREASTMT.JCL:L66-96]. The lock is released when
/// the returned guard is dropped.
///
/// # Errors
///
/// A setup failure when the directory cannot be created, when it is shared with other users or
/// cannot be restricted to its owner (the text names the directory and its mode), when another
/// batch job holds the lock or a killed job left it behind (the text names the lock file), or
/// when the lock file cannot be created or written. No statement file is deleted or written.
fn lock_statement_dir(dir: &Path) -> Result<LockFile, ExitStatus> {
    create_private_dir(dir).map_err(|err| {
        setup_failure(
            "statement directory could not be created or restricted",
            err,
        )
    })?;
    LockFile::acquire(dir.join(LOCK_FILE), "statement lock")
        .map_err(|err| setup_failure("statement lock not acquired", err))
}

/// Releases the statement lock `lock` on Tokio's blocking pool, inside the job span (see
/// [`blocking`]), by dropping it there, which deletes the lock file. When the runtime cancels
/// the release because it is shutting down, a warning is logged; the runtime still drops the
/// lock, with the same effect.
async fn release_statement_lock(lock: LockFile) {
    if let Err(cancelled) = blocking(move || drop(lock)).await {
        tracing::warn!(
            error = %cancelled,
            "statement lock release cancelled by runtime shutdown"
        );
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
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Barrier;
    use std::thread;

    use carddemo_domain::entities::CardXref;
    use carddemo_domain::messages::{
        ABENDING_PROGRAM, ERROR_READING_TRNXFILE, ERROR_READING_XREFFILE, RETURN_CODE,
    };
    use carddemo_domain::numeric::{AccountId, CardNumber, CustomerId};
    use carddemo_domain::ports::{PortError, PortResult, XrefReader};
    use carddemo_domain::rules::statement::{CAPACITY_ESCALATION, StatementError, StatementOutput};
    use chrono::{DateTime, Utc};

    use super::{
        LOCK_FILE, StatementFiles, XREF_PAGE, date_pdf, lock_statement_dir, prepare,
        read_xref_file, remove_if_present, statement_failure, step030_delete, write_files,
    };
    use crate::exit::ExitStatus;
    use crate::generations::tests::{dir_mode_of, mode_of};
    use crate::pdf::render_statement;

    /// The `CARDDEMO_CLOCK_OVERRIDE` instant of the statement tests.
    const CLOCK: &str = "2022-07-01T12:00:00Z";

    /// The RFC 3339 instant `text`.
    fn at(text: &str) -> DateTime<Utc> {
        text.parse()
            .unwrap_or_else(|e| panic!("{text} is an RFC 3339 instant: {e}"))
    }

    /// The `/CreationDate` and `/ModDate` of the information dictionary of `pdf`, the catalog's
    /// `/Metadata` entry if it has one, and the text lines `lopdf` extracts from every page.
    fn pdf_dates_and_text(pdf: &[u8]) -> ([Vec<u8>; 2], bool, Vec<String>) {
        let doc = lopdf::Document::load_mem(pdf).expect("the statement PDF parses");
        let info = doc
            .trailer
            .get(b"Info")
            .and_then(lopdf::Object::as_reference)
            .and_then(|id| doc.get_dictionary(id))
            .expect("the statement PDF has an information dictionary");
        let date = |entry: &[u8]| {
            info.get(entry).and_then(lopdf::Object::as_str).map_or_else(
                |e| panic!("{}: {e}", String::from_utf8_lossy(entry)),
                <[u8]>::to_vec,
            )
        };
        let metadata = doc
            .catalog()
            .expect("the statement PDF has a catalog")
            .has(b"Metadata");
        let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
        let text = doc
            .extract_text(&pages)
            .expect("the statement PDF text extracts");
        (
            [date(b"CreationDate"), date(b"ModDate")],
            metadata,
            text.lines().map(str::to_owned).collect(),
        )
    }

    /// `pdf` without the trailer `/ID`, saved again.
    fn without_id(pdf: &[u8]) -> Vec<u8> {
        let mut doc = lopdf::Document::load_mem(pdf).expect("the statement PDF parses");
        doc.trailer.remove(b"ID");
        let mut out = Vec::new();
        doc.save_to(&mut out).expect("the statement PDF saves");
        out
    }

    /// `bytes` with every `from` replaced by `to`.
    fn replaced(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut rest = bytes;
        while let Some(at) = rest.windows(from.len()).position(|window| window == from) {
            out.extend_from_slice(&rest[..at]);
            out.extend_from_slice(to);
            rest = &rest[at + from.len()..];
        }
        out.extend_from_slice(rest);
        out
    }

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
        let prepared = prepare(&out, at(CLOCK)).expect("records of the right widths");
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
            let status = prepare(&out, at(CLOCK)).expect_err("a record off its LRECL");
            assert!(matches!(status, ExitStatus::SetupFailure(_)), "{status:?}");
        }
    }

    #[test]
    fn the_statement_pdf_is_dated_with_the_job_clock() {
        let out = output(3, 1);
        let cases = [
            (CLOCK, "D:20220701120000+00'00'"),
            ("1999-12-31T23:59:59.999999Z", "D:19991231235959+00'00'"),
        ];
        for (instant, expected) in cases {
            let prepared = prepare(&out, at(instant)).expect("records of the right widths");

            let (dates, metadata, text) = pdf_dates_and_text(&prepared.pdf);

            let expected = expected.as_bytes().to_vec();
            assert_eq!(dates, [expected.clone(), expected], "{instant}");
            assert!(
                !metadata,
                "{instant}: no XMP metadata stream holds other dates"
            );
            assert_eq!(text, out.stmt_lines, "{instant}: the text is as rendered");
            let (_, _, rendered) = pdf_dates_and_text(
                &render_statement(&out.stmt_lines).expect("the statement PDF renders"),
            );
            assert_eq!(text, rendered, "{instant}");
        }
    }

    #[test]
    fn the_statement_pdf_bytes_follow_the_records_and_the_instant_only() {
        let out = output(2, 0);
        let rendered = render_statement(&out.stmt_lines).expect("the statement PDF renders");
        let first = date_pdf(&rendered, at(CLOCK)).expect("the statement PDF is dated");
        let again = date_pdf(&rendered, at(CLOCK)).expect("the statement PDF is dated");
        let later = date_pdf(&rendered, at("2022-07-01T12:00:01Z")).expect("is dated");

        assert_eq!(first, again, "the same instant gives the same bytes");
        assert_ne!(first, later, "another instant gives other dates");
        assert_eq!(
            replaced(&first, b"D:20220701120000", b"D:20220701120001"),
            later,
            "the dates are the only bytes the instant changes"
        );
        // printpdf numbers the trailer /ID of each document a process builds from a
        // process-wide counter, so two renders in this test process differ there; one
        // `batch creastmt` run renders one document, so its reruns write the same /ID too.
        let one = prepare(&out, at(CLOCK)).expect("records of the right widths");
        let other = prepare(&out, at(CLOCK)).expect("records of the right widths");
        assert_eq!(
            without_id(&one.pdf),
            without_id(&other.pdf),
            "no host-clock value is left in the document"
        );
    }

    #[test]
    fn write_files_writes_the_three_files() {
        let dir = scratch_dir("write");
        let files = StatementFiles::under(&dir.join("statements"));
        let prepared = prepare(&output(1, 1), at(CLOCK)).expect("records of the right widths");

        let written = write_files(&files, &prepared);
        let contents: Vec<Vec<u8>> = files
            .all()
            .iter()
            .map(|path| fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())))
            .collect();
        let modes: Vec<u32> = files.all().iter().map(|path| mode_of(path)).collect();
        let dir_mode = dir_mode_of(&dir.join("statements"));
        remove_dir(&dir);

        assert_eq!(written, Ok(()));
        assert_eq!(contents, [prepared.stmt, prepared.html, prepared.pdf]);
        assert_eq!(modes, [0o600; 3], "the statement files are owner-only");
        assert_eq!(dir_mode, 0o700, "the statement directory is owner-only");
    }

    #[test]
    fn a_failed_write_leaves_no_statement_file() {
        let dir = scratch_dir("write-failure");
        let mut files = StatementFiles::under(&dir);
        let blocker = dir.join("not-a-directory");
        fs::write(&blocker, b"").expect("write the blocking file");
        files.html = blocker.join(super::HTML_FILE);
        let prepared = prepare(&output(1, 1), at(CLOCK)).expect("records of the right widths");

        let written = write_files(&files, &prepared);
        let left = present(&files);
        remove_dir(&dir);

        let status = written.expect_err("STATEMNT.HTML cannot be created under a file");
        assert!(matches!(status, ExitStatus::SetupFailure(_)), "{status:?}");
        assert_eq!(left, [false, false, false]);
    }

    #[test]
    fn the_statement_lock_holds_the_process_id_until_dropped() {
        let dir = scratch_dir("lock");
        let statements = dir.join("statements");
        let lock_path = statements.join(LOCK_FILE);

        let lock = lock_statement_dir(&statements).expect("a free lock in a new directory");
        let held = fs::read_to_string(&lock_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));
        let modes = (dir_mode_of(&statements), mode_of(&lock_path));
        drop(lock);
        let released = !lock_path.exists();
        let again = lock_statement_dir(&statements).map(drop);
        let left = lock_path.exists();
        remove_dir(&dir);

        assert_eq!(held, format!("{}\n", std::process::id()));
        assert_eq!(
            modes,
            (0o700, 0o600),
            "the directory and the lock file are owner-only"
        );
        assert!(released, "dropping the guard deletes the lock file");
        assert_eq!(again, Ok(()), "a released lock is taken again");
        assert!(!left, "the second guard releases the lock too");
    }

    #[test]
    fn a_held_statement_lock_refuses_the_job_and_leaves_the_files() {
        // The lock is held by a running job of this process, or left by a killed job.
        for (case, stale) in [("live", false), ("stale", true)] {
            let dir = scratch_dir(&format!("lock-{case}"));
            let files = StatementFiles::under(&dir);
            let lock_path = dir.join(LOCK_FILE);
            let previous: Vec<Vec<u8>> = files
                .all()
                .iter()
                .map(|path| format!("previous {}\n", path.display()).into_bytes())
                .collect();
            for (path, bytes) in files.all().iter().zip(&previous) {
                fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            }
            let holder = if stale {
                fs::write(&lock_path, b"4242\n")
                    .unwrap_or_else(|e| panic!("write {}: {e}", lock_path.display()));
                None
            } else {
                Some(lock_statement_dir(&dir).expect("the first job takes the lock"))
            };
            let held = fs::read(&lock_path)
                .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));

            let refused = lock_statement_dir(&dir);
            let kept = fs::read(&lock_path)
                .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));
            let contents: Vec<Vec<u8>> = files
                .all()
                .iter()
                .map(|path| {
                    fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
                })
                .collect();
            drop(holder);
            remove_dir(&dir);

            let status = refused.expect_err("the lock is held");
            assert_eq!(status.code(), 16, "{case}");
            assert_eq!(
                status,
                ExitStatus::SetupFailure(format!(
                    "another batch job holds the statement lock {}; remove that file only when no \
                     batch job is running, because a job killed before releasing the lock leaves \
                     it behind",
                    lock_path.display()
                )),
                "{case}"
            );
            assert_eq!(kept, held, "{case}: the lock is never taken over");
            assert_eq!(
                contents, previous,
                "{case}: no statement file is deleted or written"
            );
        }
    }

    #[test]
    fn a_statement_directory_that_cannot_be_created_is_a_setup_failure() {
        let dir = scratch_dir("lock-no-dir");
        let blocker = dir.join("not-a-directory");
        fs::write(&blocker, b"").expect("write the blocking file");
        let statements = blocker.join("statements");

        let locked = lock_statement_dir(&statements);
        remove_dir(&dir);

        let status = locked.expect_err("a directory cannot be created under a file");
        assert_eq!(status.code(), 16);
        assert!(
            matches!(
                status,
                ExitStatus::SetupFailure(ref text)
                    if text.contains(&statements.display().to_string())
            ),
            "{status:?}"
        );
    }

    /// A statement directory shared with other users is refused before the lock is taken: the
    /// job ends with a setup failure naming the directory and its mode, creates nothing in it
    /// and leaves its mode as it is.
    #[test]
    fn a_shared_statement_directory_is_a_setup_failure() {
        let dir = scratch_dir("lock-shared");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o1777))
            .unwrap_or_else(|e| panic!("chmod {}: {e}", dir.display()));

        let locked = lock_statement_dir(&dir).map(drop);
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("list {}: {e}", dir.display()))
            .count();
        let mode = mode_of(&dir);
        remove_dir(&dir);

        let status = locked.expect_err("a shared directory is refused");
        assert_eq!(status.code(), 16);
        let refusal = format!("output directory {} has mode 1777, ", dir.display());
        assert!(
            matches!(status, ExitStatus::SetupFailure(ref text) if text.starts_with(&refusal)),
            "{status:?}"
        );
        assert_eq!(entries, 0, "no lock or statement file is created");
        assert_eq!(mode, 0o1777, "the shared directory is left as it is");
    }

    #[test]
    fn racing_jobs_take_the_statement_lock_one_at_a_time() {
        const JOBS: usize = 8;
        let dir = scratch_dir("lock-race");
        let start = Barrier::new(JOBS);
        let tried = Barrier::new(JOBS);

        let outcomes: Vec<Result<(), ExitStatus>> = thread::scope(|scope| {
            let (dir, start, tried) = (&dir, &start, &tried);
            let jobs: Vec<_> = (0..JOBS)
                .map(|_| {
                    scope.spawn(move || {
                        start.wait();
                        let lock = lock_statement_dir(dir);
                        // Every job tries while the winner still holds the lock.
                        tried.wait();
                        lock.map(drop)
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().expect("the job thread ends"))
                .collect()
        });
        let left = dir.join(LOCK_FILE).exists();
        remove_dir(&dir);

        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "{outcomes:?}"
        );
        for status in outcomes.iter().filter_map(|outcome| outcome.as_ref().err()) {
            assert!(
                matches!(status, ExitStatus::SetupFailure(text) if text.contains(LOCK_FILE)),
                "{status:?}"
            );
        }
        assert!(!left, "the winner releases the lock");
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

/// [`StatementMasters`] against the keyed `CUSTFILE` and `ACCTFILE` reads it stands for.
#[cfg(test)]
mod master_tests {
    use std::collections::BTreeMap;
    use std::future::Future;

    use carddemo_domain::entities::{Account, CardXref, Customer, NewTransaction, Transaction};
    use carddemo_domain::messages::{ERROR_READING_ACCTFILE, ERROR_READING_CUSTFILE};
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustSsn, CustomerId, FicoScore, MerchantId,
        Money, TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
    };
    use carddemo_domain::ports::{AccountReader, CustomerReader, PortError, PortResult};
    use carddemo_domain::rules::statement::{
        self, StatementError, StatementOutput, TransactionTable,
    };

    use super::{BatchRead, StatementMasters};

    /// The failure of a whole read, as a store reports it.
    fn unavailable() -> PortError {
        PortError::Unavailable("connection reset".to_owned())
    }

    /// The failure of a stored row that does not fit its PIC clause, as the repositories report
    /// it for that row's key.
    fn corrupt(column: &str) -> PortError {
        PortError::Unavailable(format!(
            "stored {column} value does not fit its domain type: value is outside the range of \
             the field"
        ))
    }

    fn card(n: u32) -> CardNumber {
        CardNumber::move_from(&format!("{n:016}"))
    }

    fn customer_id(n: u32) -> CustomerId {
        CustomerId::new(n).expect("a 9-digit customer number")
    }

    fn account_id(n: u32) -> AccountId {
        AccountId::new(u64::from(n)).expect("an 11-digit account number")
    }

    /// Customer `n`, whose name, address and score are its own.
    fn customer(n: u32) -> Customer {
        Customer {
            cust_id: customer_id(n),
            cust_first_name: Alnum::move_from(&format!("First{n}")),
            cust_middle_name: Alnum::move_from("Alba"),
            cust_last_name: Alnum::move_from(&format!("Last{n}")),
            cust_addr_line_1: Alnum::move_from(&format!("{n} Nienow Cape")),
            cust_addr_line_2: Alnum::move_from("Suite 187"),
            cust_addr_line_3: Alnum::move_from("New Aricchester"),
            cust_addr_state_cd: Alnum::move_from("OR"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("04257"),
            cust_phone_num_1: Alnum::move_from("(325)301-0827"),
            cust_phone_num_2: Alnum::move_from("(493)985-9283"),
            cust_ssn: CustSsn::new(931_248_469).expect("a 9-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000303878241"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1960-12-01"),
            cust_eft_account_id: Alnum::move_from("0000000000"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(
                600 + u16::try_from(n).expect("a small customer number"),
            )
            .expect("a 3-digit score"),
        }
    }

    /// Account `n`, whose balance is its own.
    fn account(n: u32) -> Account {
        Account {
            acct_id: account_id(n),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::from_cents(194_500 + i64::from(n))
                .expect("an amount within S9(10)V99"),
            acct_credit_limit: Money::from_cents(616_900).expect("an amount within S9(10)V99"),
            acct_cash_credit_limit: Money::from_cents(458_700).expect("an amount within S9(10)V99"),
            acct_open_date: Alnum::move_from("2011-04-22"),
            acct_expiraion_date: Alnum::move_from("2023-03-09"),
            acct_reissue_date: Alnum::move_from("2023-03-09"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn tran(card_n: u32, tran_id: &str, amount: i64) -> Transaction {
        Transaction::from_new(
            TransactionRowId::from(1),
            NewTransaction {
                tran_id: TranId::move_from(tran_id),
                tran_type_cd: TranTypeCode::move_from("01"),
                tran_cat_cd: TranCategoryCode::new(1).expect("a 4-digit category"),
                tran_source: Alnum::move_from("POS TERM"),
                tran_desc: Alnum::move_from("Purchase at Blick-Rippin"),
                tran_amt: Money::from_cents(amount).expect("an amount within S9(09)V99"),
                tran_merchant_id: MerchantId::new(800_000_000).expect("a 9-digit merchant"),
                tran_merchant_name: Alnum::move_from("Blick-Rippin"),
                tran_merchant_city: Alnum::move_from("North Enoch"),
                tran_merchant_zip: Alnum::move_from("56398"),
                tran_card_num: card(card_n),
                tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
                tran_proc_ts: Alnum::move_from("2022-07-18 00:00:00.000000"),
            },
        )
    }

    /// Transactions on cards 1 and 3, so the statements of the cards differ in their lines too.
    fn table() -> TransactionTable {
        statement::load_table(vec![
            tran(1, "0000000000000001", 18_388),
            tran(1, "0000000000000002", -4_788),
            tran(3, "0000000000000003", 2_500),
        ])
        .expect("a table within the CBSTM03A capacity")
    }

    /// The cross-references `(card, customer, account)`, in card order.
    fn xrefs(rows: &[(u32, u32, u32)]) -> Vec<CardXref> {
        rows.iter()
            .map(|&(card_n, cust, acct)| CardXref {
                xref_card_num: card(card_n),
                xref_cust_id: customer_id(cust),
                xref_acct_id: account_id(acct),
            })
            .collect()
    }

    /// `CUSTFILE` and `ACCTFILE` read one key at a time, the CBSTM03B keyed reads, with each call
    /// recorded by name. A key's entry is the outcome of its read, and a key with none is not
    /// found; a file whose failure is set fails every read of it, as a store that is down does.
    #[derive(Clone)]
    struct KeyedReads {
        customers: BTreeMap<CustomerId, PortResult<Customer>>,
        accounts: BTreeMap<AccountId, PortResult<Account>>,
        customer_failure: Option<PortError>,
        account_failure: Option<PortError>,
        calls: Vec<&'static str>,
    }

    /// The outcome of the keyed read of `key`.
    fn keyed<K: Ord, T: Clone>(
        failure: Option<&PortError>,
        entries: &BTreeMap<K, PortResult<T>>,
        key: &K,
    ) -> PortResult<T> {
        if let Some(err) = failure {
            return Err(err.clone());
        }
        entries
            .get(key)
            .cloned()
            .unwrap_or(Err(PortError::NotFound))
    }

    impl CustomerReader for KeyedReads {
        async fn read_customer(&mut self, id: CustomerId) -> PortResult<Customer> {
            self.calls.push("read_customer");
            keyed(self.customer_failure.as_ref(), &self.customers, &id)
        }

        async fn read_customer_for_update(&mut self, id: CustomerId) -> PortResult<Customer> {
            self.calls.push("read_customer_for_update");
            keyed(self.customer_failure.as_ref(), &self.customers, &id)
        }
    }

    impl AccountReader for KeyedReads {
        async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
            self.calls.push("read_account");
            keyed(self.account_failure.as_ref(), &self.accounts, &id)
        }

        async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
            self.calls.push("read_account_for_update");
            keyed(self.account_failure.as_ref(), &self.accounts, &id)
        }
    }

    /// Customers and accounts 1 to 4, every one readable.
    fn masters() -> KeyedReads {
        KeyedReads {
            customers: (1..=4).map(|n| (customer_id(n), Ok(customer(n)))).collect(),
            accounts: (1..=4).map(|n| (account_id(n), Ok(account(n)))).collect(),
            customer_failure: None,
            account_failure: None,
            calls: Vec::new(),
        }
    }

    /// The batch read of the keys `ids` names over the keyed reads `entries` and `failure`, as
    /// `PgStore::read_customers` and `PgStore::read_accounts` return it: each distinct key with
    /// its outcome, or the failure of the whole statement.
    fn batch<K: Ord + Copy, T: Clone>(
        ids: impl IntoIterator<Item = K>,
        entries: &BTreeMap<K, PortResult<T>>,
        failure: Option<&PortError>,
    ) -> BatchRead<K, T> {
        match failure {
            Some(err) => Err(err.clone()),
            None => Ok(ids
                .into_iter()
                .map(|id| (id, keyed(None, entries, &id)))
                .collect()),
        }
    }

    /// Fails to compile unless `future` is `Send`, as the multi-threaded batch runtime needs.
    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    async fn statements<S: CustomerReader + AccountReader + Send>(
        store: &mut S,
        xrefs: &[CardXref],
        table: &TransactionTable,
    ) -> Result<StatementOutput, StatementError> {
        require_send(statement::create_statements(store, xrefs.to_vec(), table)).await
    }

    /// The number of statements written, or the file and return code of the abend.
    fn outcome(result: &Result<StatementOutput, StatementError>) -> Result<usize, (&str, &str)> {
        match result {
            Ok(output) => Ok(output
                .stmt_lines
                .iter()
                .filter(|line| line.contains("START OF STATEMENT"))
                .count()),
            Err(StatementError::ReadFailed { file, rc, .. }) => Err((file, rc.as_str())),
            Err(other) => panic!("a read rule ends in a read outcome, got {other}"),
        }
    }

    /// One master set, its cross-references and the outcome its statements must have.
    struct Case {
        name: &'static str,
        xrefs: &'static [(u32, u32, u32)],
        edit: fn(&mut KeyedReads),
        expected: Result<usize, (&'static str, &'static str)>,
    }

    const THREE_CARDS: &[(u32, u32, u32)] = &[(1, 1, 1), (2, 2, 2), (3, 3, 3)];

    /// The master sets the batch reads are held to: every record found, records missing,
    /// corrupt and failed in either file and at either position, shared keys and no
    /// cross-reference.
    fn cases() -> [Case; 10] {
        [
            Case {
                name: "every record found",
                xrefs: THREE_CARDS,
                edit: |_| {},
                expected: Ok(3),
            },
            Case {
                name: "a missing customer",
                xrefs: THREE_CARDS,
                edit: |m| {
                    m.customers.remove(&customer_id(2));
                },
                expected: Err((ERROR_READING_CUSTFILE, "23")),
            },
            Case {
                name: "a missing account before a later missing customer",
                xrefs: THREE_CARDS,
                edit: |m| {
                    m.accounts.remove(&account_id(1));
                    m.customers.remove(&customer_id(2));
                },
                expected: Err((ERROR_READING_ACCTFILE, "23")),
            },
            Case {
                name: "a corrupt customer row",
                xrefs: THREE_CARDS,
                edit: |m| {
                    m.customers
                        .insert(customer_id(2), Err(corrupt("customers.cust_ssn")));
                },
                expected: Err((ERROR_READING_CUSTFILE, "30")),
            },
            Case {
                name: "a corrupt account row at a later cross-reference",
                xrefs: THREE_CARDS,
                edit: |m| {
                    m.accounts
                        .insert(account_id(3), Err(corrupt("accounts.acct_curr_bal")));
                },
                expected: Err((ERROR_READING_ACCTFILE, "30")),
            },
            Case {
                name: "a failed customer read with a missing account",
                xrefs: THREE_CARDS,
                edit: |m| {
                    m.customer_failure = Some(unavailable());
                    m.accounts.remove(&account_id(1));
                },
                expected: Err((ERROR_READING_CUSTFILE, "30")),
            },
            Case {
                name: "a failed account read",
                xrefs: THREE_CARDS,
                edit: |m| m.account_failure = Some(unavailable()),
                expected: Err((ERROR_READING_ACCTFILE, "30")),
            },
            Case {
                name: "keys named by several cross-references",
                xrefs: &[(1, 1, 1), (2, 1, 1), (3, 2, 1), (4, 2, 4)],
                edit: |_| {},
                expected: Ok(4),
            },
            Case {
                name: "a missing key named twice",
                xrefs: &[(1, 1, 1), (2, 4, 4), (3, 4, 4)],
                edit: |m| {
                    m.accounts.remove(&account_id(4));
                },
                expected: Err((ERROR_READING_ACCTFILE, "23")),
            },
            Case {
                name: "no cross-reference",
                xrefs: &[],
                edit: |_| {},
                expected: Ok(0),
            },
        ]
    }

    #[tokio::test]
    async fn batch_reads_give_the_statements_and_the_abend_of_the_keyed_reads() {
        let table = table();
        for case in cases() {
            let mut keyed_reads = masters();
            (case.edit)(&mut keyed_reads);
            let xrefs = xrefs(case.xrefs);
            let mut wrapped = keyed_reads.clone();
            let expected = statements(&mut keyed_reads, &xrefs, &table).await;

            let customers = batch(
                xrefs.iter().map(|xref| xref.xref_cust_id),
                &wrapped.customers,
                wrapped.customer_failure.as_ref(),
            );
            let accounts = batch(
                xrefs.iter().map(|xref| xref.xref_acct_id),
                &wrapped.accounts,
                wrapped.account_failure.as_ref(),
            );
            let mut served = StatementMasters {
                customers,
                accounts,
                store: &mut wrapped,
            };
            let actual = statements(&mut served, &xrefs, &table).await;

            assert_eq!(actual, expected, "{}", case.name);
            assert_eq!(outcome(&actual), case.expected, "{}", case.name);
            assert_eq!(
                wrapped.calls,
                Vec::<&str>::new(),
                "{}: no read reaches the wrapped store",
                case.name
            );
        }
    }

    #[tokio::test]
    async fn plain_reads_answer_from_the_batch_and_locking_reads_go_to_the_store() {
        let mut wrapped = masters();
        let mut served = StatementMasters {
            customers: Err(unavailable()),
            accounts: Ok(BTreeMap::from([(account_id(1), Ok(account(1)))])),
            store: &mut wrapped,
        };

        let failed_customer = served.read_customer(customer_id(1)).await;
        let batched_account = served.read_account(account_id(1)).await;
        let unbatched_account = served.read_account(account_id(2)).await;
        let locked_customer = served.read_customer_for_update(customer_id(1)).await;
        let locked_account = served.read_account_for_update(account_id(2)).await;

        assert_eq!(failed_customer, Err(unavailable()));
        assert_eq!(batched_account, Ok(account(1)));
        assert_eq!(unbatched_account, Err(PortError::NotFound));
        assert_eq!(locked_customer, Ok(customer(1)));
        assert_eq!(locked_account, Ok(account(2)));
        assert_eq!(
            wrapped.calls,
            ["read_customer_for_update", "read_account_for_update"]
        );
    }
}
