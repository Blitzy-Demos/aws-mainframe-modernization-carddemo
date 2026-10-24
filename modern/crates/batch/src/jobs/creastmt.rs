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
//! | `STEP010` | `SORT FIELDS=(263,16,CH,A,1,16,CH,A)` of `TRANSACT`: card number, then transaction ID | `TransactionRepository::stream_in_card_order`, every row read once and counted by `TableCensus`, none kept; then, for each statement, `TransactionRepository::find_by_card`, whose rows `TransactionTable::of_card` puts in EBCDIC order itself (CBSTM03A-001, CBSTM03A-008, X-CREASTMT-3) |
//! | `STEP020` | `REPRO` of the sorted records into `TRXFL` | none: no work file; each statement reads its card's rows by key, so at most one card's transactions are held (CBSTM03A-001, CBSTM03B-002) |
//! | `STEP030` | `IEFBR14` deletes the previous `STATEMNT.PS` and `STATEMNT.HTML` | `OutputFile::replace` of the three fixed names, `STATEMNT.PDF` included, under the statement directory's `DirectoryLock`, before the first read (CBSTM03A-002) |
//! | `STEP040` | `CBSTM03A` writes `STMTFILE` and `HTMLFILE` | [`run`]: `STATEMNT.PS` of 80-byte records and `STATEMNT.HTML` of 100-byte records, the program's FD rather than the `LRECL=80` of `STEP030` (CBSTM03A-031, CBSTM03A-047, E20) |
//! | `TXT2PDF1` `TXT2PDF` | `%TXT2PDF` converts `STATEMNT.PS` into `STATEMNT.PS.PDF` | `crate::pdf::render` over the text records, written as `STATEMNT.PDF`, the name AAP 0.3.4.5 gives it (CBSTM03A-051, X-CREASTMT-7) |
//!
//! # Flow
//!
//! 1. The run takes the statement directory's `DirectoryLock`, creating the directory
//!    owner-only when it is missing, and releases it only once its files are completed or
//!    removed: another run over the same directory, from any process or container that shares
//!    the directory, waits until then, so the three files are one set no two runs touch at once.
//! 2. The three files are replaced, so the previous run's are gone before any table is read, as
//!    `STEP030` deletes them and `CBSTM03A` opens its outputs before its first read
//!    (CBSTM03A-002, CBSTM03A-004). The names are fixed, not `(+1)` generations.
//! 3. Every `transactions` row is read once, in card order, and counted by `TableCensus`, not
//!    kept, before the first cross-reference record, as 8100-TRNXFILE-OPEN and
//!    8500-READTRNX-READ load `WS-TRNX-TABLE` (CBSTM03A-004, CBSTM03A-006 to CBSTM03A-008). The
//!    count gives the empty-set abend and the bounds warning.
//! 4. Every `card_xref` row is read and put in key order, `xref_sort_key`, the EBCDIC key order
//!    of the KSDS `CBSTM03B` reads sequentially (CBSTM03A-009, CBSTM03B-003). Each one gets one
//!    statement: its customer by `cust_id`, then its account by `acct_id` (CBSTM03A-010,
//!    CBSTM03A-011, CBSTM03B-004, CBSTM03B-005), then its card's transactions, the card's entry
//!    of the table, read by `card_num` and put in EBCDIC transaction-ID order by
//!    `TransactionTable::of_card` (CBSTM03A-013), then `cbstm03a::statement`, whose text
//!    records go to `STATEMNT.PS` and HTML records to `STATEMNT.HTML`, each followed by LF. So at
//!    most one card's transactions are held at a time. The keyed master reads are served from
//!    `customers` and `accounts` each read whole, once, the first time a statement needs one of
//!    their records, so a run makes two master reads rather than two per cross-reference record.
//! 5. After the last statement the PDF is rendered from the text records read back from
//!    `STATEMNT.PS` through the run's own handle, one at a time, and written; then
//!    `STATEMNT.PS`, `STATEMNT.HTML` and `STATEMNT.PDF` are completed in that order and their
//!    paths returned in that order, which `main` prints; return code 0 (CBSTM03A-050,
//!    CBSTM03A-051). No text record is held beyond its statement. The PDF document is the one
//!    part of the run that still grows with it: `printpdf` builds the whole document in memory
//!    before it is written (escalation candidate X-CREASTMT-9).
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
//! or none (CBSTM03A-002); a file that cannot be removed is reported by a cleanup event naming
//! the operation and the error kind only. The run still holds the directory's lock then, and
//! removes each path only while it names the file the run created, so every file it removes is
//! its own. The empty-table outcome itself is halt candidate X-CREASTMT-H1: the replacement
//! takes the CBSTM03A-007 path, which claims no parity for that case.
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
//!   pool. The directory lock is an entry of the statement directory, on Linux a directory
//!   holding a Unix socket that other runs connect to only to learn whether its run still holds
//!   the lock; the entry carries no data.
//! - Single Schema Executor: no schema work here; against a database whose schema has not been
//!   applied, the first read fails and the job abends as CBSTM03A-012 states.
//! - R1: this module handles only record bytes; the page coordinates are `crate::pdf`'s.

use std::collections::BTreeMap;
use std::io::{self, BufRead, ErrorKind};
use std::path::PathBuf;

use copybook::PicU;
use domain::batch::cbstm03a::{
    self, STMT_RECORD_LEN, Statement, StatementError, TableCensus, TransactionTable,
};
use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord};
use persistence::{
    AccountRepository, CardXrefRepository, CustomerRepository, PersistenceError,
    TransactionRepository,
};
use runtime::telemetry::JobCounter;
use tracing::field::Value;
use tracing::{Level, Metadata};

use super::{
    DirectoryLock, EventSite, FileIdentity, JobContext, JobError, JobOutcome, OutputFile, count,
    emit, event_metadata,
};

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

/// The [`JobError::Io`] context of reading `STATEMNT.PS` back for the PDF (CBSTM03A-051).
const READING_STATEMENT_TEXT: &str = "reading statement text";

/// The cleanup operation of a completed statement file removed because a later one could not
/// be completed (CBSTM03A-002).
const REMOVING_COMPLETED_STATEMENT: &str = "removing completed statement file";

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
/// - [`JobError::Io`] when the statement directory cannot be created or locked, or a file
///   cannot be replaced, written, read back or completed.
///
/// After any error none of the three files remains (CBSTM03A-002, CBSTM03A-049).
pub(crate) async fn run(ctx: &JobContext<'_>) -> Result<JobOutcome, JobError> {
    let lock = DirectoryLock::acquire(&ctx.config.statement_dir).await?;
    let outcome = replace_statements(ctx, &lock).await;
    // Released only now, once the files are completed or removed.
    lock.release().await;
    outcome
}

/// The run under the statement directory's `lock`: replaces the three files, counts the
/// transactions, reads the cross-references and writes the statements, then completes the
/// files. Every file it created is completed or removed when it returns.
///
/// # Errors
///
/// As [`run`].
async fn replace_statements(
    ctx: &JobContext<'_>,
    lock: &DirectoryLock,
) -> Result<JobOutcome, JobError> {
    let mut files = StatementFiles::replace(lock).await?;
    // The store is a handle on the shared pool, cloned for the `&mut self` repository calls.
    let mut store = ctx.store.clone();
    transaction_census(&mut store).await?;
    let xrefs = in_key_order(CardXrefRepository::find_all(&mut store).await)?;
    write_statements(&mut store, &xrefs, &mut files).await?;
    files.finish().await
}

/// The main loop of `CBSTM03A` over `xrefs`, in their order (CBSTM03A-009): for each record its
/// customer, then its account, then its card's transactions, then its statement written to
/// `files` (CBSTM03A-010, CBSTM03A-011, CBSTM03A-013). The transactions are the card's entry of
/// the table, read by `card_num` for this statement alone and put in EBCDIC order by
/// `TransactionTable::of_card`, so at most one card's transactions are held. With no record
/// nothing is read.
///
/// # Errors
///
/// [`JobError::Abend`] for the first customer or account that is missing or cannot be read
/// (CBSTM03A-010 to CBSTM03A-012), or the first card whose transactions cannot be read
/// (CBSTM03A-012); [`JobError::Io`] when a record cannot be written.
async fn write_statements<S>(
    store: &mut S,
    xrefs: &[CardXrefRecord],
    files: &mut StatementFiles,
) -> Result<(), JobError>
where
    S: CustomerRepository + AccountRepository + TransactionRepository,
{
    let mut masters = Masters::default();
    for xref in xrefs {
        let (customer, account) = masters.records_of(store, xref).await?;
        let rows = TransactionRepository::find_by_card(store, &xref.card_num)
            .await
            .map_err(|error| abend(ReadFailure::TransactionsUnreadable, Some(&error)))?;
        let table = TransactionTable::of_card(&rows);
        files
            .write(&cbstm03a::statement(xref, customer, account, &table))
            .await?;
    }
    Ok(())
}

/// The two master files a statement reads by key, `CUSTFILE` and `ACCTFILE` (CBSTM03B-004,
/// CBSTM03B-005): `customers` and `accounts`, each read whole the first time a statement needs
/// one of its records and kept by key for the rest of the run, rather than read once per key.
#[derive(Default)]
struct Masters {
    /// `customers` by `CUST-ID`, once read.
    customers: Option<BTreeMap<PicU<9>, CustomerRecord>>,
    /// `accounts` by `ACCT-ID`, once read.
    accounts: Option<BTreeMap<PicU<11>, AccountRecord>>,
}

impl Masters {
    /// The customer, then the account, of `xref`, in the order `CBSTM03A` reads them
    /// (CBSTM03A-010, CBSTM03A-011). `customers` is read the first time a customer is needed,
    /// and `accounts` the first time an account is, after that record's customer was found: a
    /// first record whose customer is missing ends the run before `accounts` is read, as the
    /// program never reaches its account read.
    ///
    /// # Errors
    ///
    /// [`JobError::Abend`] when a table cannot be read (CBSTM03A-012) or holds no row with the
    /// key, status `23` (CBSTM03A-010, CBSTM03A-011).
    async fn records_of<S>(
        &mut self,
        store: &mut S,
        xref: &CardXrefRecord,
    ) -> Result<(&CustomerRecord, &AccountRecord), JobError>
    where
        S: CustomerRepository + AccountRepository,
    {
        let customers = match self.customers.take() {
            Some(rows) => rows,
            None => keyed(
                CustomerRepository::find_all(store).await,
                |customer| customer.id,
                ReadFailure::CustomerUnreadable,
            )?,
        };
        let customer = found(
            self.customers.insert(customers).get(&xref.cust_id),
            ReadFailure::CustomerNotFound,
        )?;
        let accounts = match self.accounts.take() {
            Some(rows) => rows,
            None => keyed(
                AccountRepository::find_all(store).await,
                |account| account.id,
                ReadFailure::AccountUnreadable,
            )?,
        };
        let account = found(
            self.accounts.insert(accounts).get(&xref.acct_id),
            ReadFailure::AccountNotFound,
        )?;
        Ok((customer, account))
    }
}

/// The census of every transaction, from the read in card order that stands for `TRNXFILE`,
/// before the first cross-reference record, as 8100-TRNXFILE-OPEN and 8500-READTRNX-READ load
/// the table (CBSTM03A-001, CBSTM03A-004, CBSTM03A-006 to CBSTM03A-008). Each record is
/// counted as it is read and none is kept: the statements read their card's records again by
/// key. A table beyond the 51 cards of 10 transactions is reported by one warning event
/// (halt candidate X-CREASTMT-H2).
///
/// # Errors
///
/// [`JobError::Abend`] for a read that failed to start or failed at a row (CBSTM03A-012), or
/// that returned no row, the first read at end of file (CBSTM03A-007).
async fn transaction_census<S: TransactionRepository>(
    store: &mut S,
) -> Result<TableCensus, JobError> {
    let unreadable =
        |error: PersistenceError| abend(ReadFailure::TransactionsUnreadable, Some(&error));
    let mut rows = store.stream_in_card_order().await.map_err(unreadable)?;
    let mut census = TableCensus::default();
    while let Some(record) = rows.next().await.map_err(unreadable)? {
        census.count(&record);
    }
    let census = census.finish().map_err(|error| match error {
        StatementError::NoTransactions => abend(ReadFailure::TransactionsEmpty, None),
    })?;
    if !census.within_cobol_bounds() {
        beyond_the_cobol_table(census.card_count());
    }
    Ok(census)
}

/// The message of the warning of a transaction set beyond the CBSTM03A table.
const BEYOND_THE_COBOL_TABLE: &str = "the transactions exceed the 51 cards of 10 transactions of \
                                      the CBSTM03A table: every statement is written in full, \
                                      and its parity is BLOCKED (X-CREASTMT-H2)";

/// Warns that the transaction set exceeds the 51 cards of 10 transactions of the CBSTM03A
/// table: one `WARN` event with the number of cards, `cards`, and no card number (halt
/// candidate X-CREASTMT-H2).
fn beyond_the_cobol_table(cards: usize) {
    /// The callsite of the event.
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    /// The event's metadata: `WARN`, this module's target, the message and `cards`.
    static METADATA: Metadata<'static> = event_metadata(
        BEYOND_THE_COBOL_TABLE,
        module_path!(),
        Level::WARN,
        &["message", "cards"],
        &SITE,
    );
    emit(&SITE, [Some(&BEYOND_THE_COBOL_TABLE), Some(&cards)]);
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

/// The rows of a whole-table read, by `key`: the keyed file of `CBSTM03B` the table stands for
/// (CBSTM03B-004, CBSTM03B-005). `key` is the table's primary key, so no two rows share one.
///
/// # Errors
///
/// [`JobError::Abend`] on the path `failed` names when the read failed (CBSTM03A-012).
fn keyed<K: Ord, R>(
    read: Result<Vec<R>, PersistenceError>,
    key: impl Fn(&R) -> K,
    failed: ReadFailure,
) -> Result<BTreeMap<K, R>, JobError> {
    let rows = read.map_err(|error| abend(failed, Some(&error)))?;
    Ok(rows.into_iter().map(|row| (key(&row), row)).collect())
}

/// The record a keyed read found, the status `00` of `CBSTM03B` (CBSTM03B-006).
///
/// # Errors
///
/// [`JobError::Abend`] on the path `missing` names when no row has the key, status `23`
/// (CBSTM03A-010, CBSTM03A-011).
fn found<T>(record: Option<T>, missing: ReadFailure) -> Result<T, JobError> {
    record.ok_or_else(|| abend(missing, None))
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
    display_line(failure.error_line(), cause);
    display_line(failure.return_code_line(), None);
    display_line(ABENDING_PROGRAM, None);
    JobError::Abend {
        code: Some(ABEND_RETURN_CODE),
        reason: failure.reason(),
    }
}

/// One `DISPLAY` line of 9999-ABEND-PROGRAM: an `ERROR` event whose message is `line` exactly,
/// with the field `cause`, the persistence error's text, only when `cause` is given.
fn display_line(line: &'static str, cause: Option<&PersistenceError>) {
    /// The callsite of the event, shared by every `DISPLAY` line.
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    /// The event's metadata: `ERROR`, this module's target, the message and `cause`.
    static METADATA: Metadata<'static> = event_metadata(
        "DISPLAY",
        module_path!(),
        Level::ERROR,
        &["message", "cause"],
        &SITE,
    );
    let cause = cause.map(tracing::field::display);
    emit(
        &SITE,
        [Some(&line), cause.as_ref().map(|cause| cause as &dyn Value)],
    );
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
    /// Text records written so far, the records the PDF is rendered from once they are all
    /// written: they are read back from `STATEMNT.PS` then, never kept here (CBSTM03A-051).
    text_records: usize,
    /// Statements written so far, one per cross-reference record (CBSTM03A-009).
    statements: usize,
    /// HTML records written so far.
    html_records: usize,
}

impl StatementFiles {
    /// Replaces the three files in the directory `lock` holds: `STEP030` deletes the previous
    /// run's statement files and `CBSTM03A` opens both outputs before its first read; the PDF of
    /// the previous run goes with them (CBSTM03A-002, CBSTM03A-004). Holding the lock, the run
    /// removes no file another run is writing.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] when a previous file cannot be removed or a new one cannot be created;
    /// the files already created are removed again.
    async fn replace(lock: &DirectoryLock) -> Result<Self, JobError> {
        Ok(Self {
            text: OutputFile::replace(lock, TEXT_FILE).await?,
            html: OutputFile::replace(lock, HTML_FILE).await?,
            pdf: OutputFile::replace(lock, PDF_FILE).await?,
            text_records: 0,
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
    async fn write(&mut self, statement: &Statement) -> Result<(), JobError> {
        for record in &statement.text_lines {
            self.text.write_record(record).await?;
        }
        for record in &statement.html_records {
            self.html.write_record(record).await?;
        }
        self.text_records += statement.text_lines.len();
        self.html_records += statement.html_records.len();
        self.statements += 1;
        Ok(())
    }

    /// Renders and writes `STATEMNT.PDF` from the text records, read back from `STATEMNT.PS`
    /// through the run's own handle one at a time, then completes `STATEMNT.PS`, `STATEMNT.HTML`
    /// and `STATEMNT.PDF` in that order, and returns the outcome of a normal end (CBSTM03A-050,
    /// CBSTM03A-051). With no statement the text and HTML files are empty and the PDF has one
    /// empty page. The reading and the rendering block, so they run as one blocking step.
    ///
    /// # Errors
    ///
    /// [`JobError::Render`] when the PDF cannot be rendered, and [`JobError::Io`] when a file
    /// cannot be written, read back or completed. None of the three files remains then: the
    /// unfinished ones are removed by their guards and the completed ones here, so that a run
    /// leaves all three files or none (CBSTM03A-002).
    async fn finish(self) -> Result<JobOutcome, JobError> {
        let Self {
            mut text,
            html,
            mut pdf,
            text_records,
            statements,
            html_records,
        } = self;
        let bytes = text
            .reread(READING_STATEMENT_TEXT, move |reader| {
                render_text(reader, text_records)
            })
            .await??;
        pdf.write_bytes(bytes).await?;
        let text_identity = text.identity();
        let text_path = text.finish().await?;
        let html_identity = html.identity();
        let html_path = match html.finish().await {
            Ok(path) => path,
            Err(error) => {
                discard(vec![(text_path, text_identity)]).await;
                return Err(error);
            }
        };
        let pdf_path = match pdf.finish().await {
            Ok(path) => path,
            Err(error) => {
                discard(vec![(text_path, text_identity), (html_path, html_identity)]).await;
                return Err(error);
            }
        };
        Ok(JobOutcome {
            written: vec![text_path, html_path, pdf_path],
            return_code: 0,
            counts: vec![
                (JobCounter::Statements, count(statements)),
                (JobCounter::TextRecords, count(text_records)),
                (JobCounter::HtmlRecords, count(html_records)),
            ],
        })
    }
}

/// The `STATEMNT.PDF` bytes, rendered from the text records `reader` reads back from
/// `STATEMNT.PS`, streamed into `crate::pdf::render` one at a time, so that the PDF shows what
/// the file holds and no more than one record is in memory (CBSTM03A-051). Runs inside a
/// blocking step.
///
/// # Errors
///
/// - [`JobError::Io`] with context `reading statement text` when the file cannot be read, or
///   holds anything but `expected` records of 80 bytes each followed by LF, with
///   `ErrorKind::InvalidData`; the rendering is discarded then.
/// - [`JobError::Render`] when a record cannot be rendered.
fn render_text(reader: impl BufRead, expected: usize) -> Result<Vec<u8>, JobError> {
    let mut records = TextRecords {
        reader,
        read: 0,
        failure: None,
    };
    let rendered = crate::pdf::render(&mut records);
    if let Some(kind) = records.failure {
        return Err(JobError::Io {
            context: READING_STATEMENT_TEXT,
            kind,
        });
    }
    let bytes = rendered?;
    if records.read == expected {
        Ok(bytes)
    } else {
        Err(JobError::Io {
            context: READING_STATEMENT_TEXT,
            kind: ErrorKind::InvalidData,
        })
    }
}

/// The text records of `STATEMNT.PS` as `render_text` reads them back: 80 bytes, then LF, each
/// (CBSTM03A-031). The first failure ends the sequence and is kept for `render_text`, which
/// discards the rendering then.
struct TextRecords<R> {
    /// The reader over the file.
    reader: R,
    /// Records given so far.
    read: usize,
    /// The kind of the failure that ended the sequence, if one did.
    failure: Option<ErrorKind>,
}

impl<R: BufRead> Iterator for TextRecords<R> {
    type Item = [u8; STMT_RECORD_LEN];

    fn next(&mut self) -> Option<Self::Item> {
        if self.failure.is_some() {
            return None;
        }
        match next_text_record(&mut self.reader) {
            Ok(Some(record)) => {
                self.read += 1;
                Some(record)
            }
            Ok(None) => None,
            Err(error) => {
                self.failure = Some(error.kind());
                None
            }
        }
    }
}

/// The next text record of `reader`, `None` at the end of the file.
///
/// # Errors
///
/// The I/O error of a failed read, or one of kind `ErrorKind::InvalidData` for a record that
/// ends before its 80 bytes and LF, or whose 81st byte is not LF.
fn next_text_record(reader: &mut impl BufRead) -> io::Result<Option<[u8; STMT_RECORD_LEN]>> {
    if reader.fill_buf()?.is_empty() {
        return Ok(None);
    }
    let mut line = [0; STMT_RECORD_LEN + 1];
    reader.read_exact(&mut line).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            io::Error::from(ErrorKind::InvalidData)
        } else {
            error
        }
    })?;
    match line.split_last() {
        Some((b'\n', record)) => <[u8; STMT_RECORD_LEN]>::try_from(record)
            .map(Some)
            .map_err(|_| io::Error::from(ErrorKind::InvalidData)),
        _ => Err(io::Error::from(ErrorKind::InvalidData)),
    }
}

/// Removes the files of this run already completed when a later one could not be, so that the
/// run leaves none of the three (CBSTM03A-002, CBSTM03A-049). The run still holds the
/// directory's lock, and each path is removed only while it names the file of its identity,
/// the one the run completed, so a file another process put under the name stays. Best effort:
/// the job fails with the completion error whatever happens here, and each file that could not
/// be removed is reported as cleanup `removing completed statement file`.
async fn discard(completed: Vec<(PathBuf, FileIdentity)>) {
    super::remove_outputs(REMOVING_COMPLETED_STATEMENT, completed).await;
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicTimestamp, PicU, PicX};
    use domain::batch::cbstm03a::{HTML_RECORD_LEN, STMT_RECORD_LEN, Statement};
    use domain::batch::loads::decode_fixture;
    use domain::money::{Decimal, Money};
    use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord, TransactionRecord};
    use domain::sensitive::CardNumber;
    use persistence::{
        AccountCustomerChain, AccountRepository, ChainLink, CustomerRepository, PersistenceError,
        TransactionRepository, TransactionRows,
    };
    use runtime::telemetry::JobCounter;
    use test_support::trace_capture;
    use tracing::instrument::WithSubscriber as _;

    use super::{
        DirectoryLock, HTML_FILE, JobError, PDF_FILE, ReadFailure, StatementFiles, TEXT_FILE,
        abend, found, in_key_order, keyed, render_text, transaction_census, write_statements,
    };
    use crate::jobs::LOCK_NAME;
    use crate::pdf::RenderError;

    type TestResult = Result<(), Box<dyn Error>>;

    /// Runs `test` to completion on a current-thread runtime, the runtime tokio's test attribute
    /// builds by default, without the lint attributes that attribute's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// Runs `test` to completion on a multi-thread runtime of two workers, for a test whose
    /// spawned run must make progress while the test's own thread blocks.
    fn block_on_two_workers(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(test)
    }

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

    /// A cross-reference record of a synthetic card number, in no fixture file, for customer
    /// and account `cust_id`; `cust_id` identifies it in the assertions, so no card number is
    /// compared or shown.
    fn xref(card: [u8; 16], cust_id: u64) -> Result<CardXrefRecord, Box<dyn Error>> {
        xref_to(card, cust_id, cust_id)
    }

    /// A cross-reference record of a synthetic card number for customer `cust_id` and account
    /// `acct_id`.
    fn xref_to(
        card: [u8; 16],
        cust_id: u64,
        acct_id: u64,
    ) -> Result<CardXrefRecord, Box<dyn Error>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(card)),
            cust_id: PicU::new(cust_id)?,
            acct_id: PicU::new(acct_id)?,
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
            let table = keyed(Ok(vec![7_u8, 9]), |row| u16::from(*row), failed)?;
            assert_eq!(found(table.get(&7), missing).ok(), Some(&7));
            let errors = [
                (found(table.get(&8), missing).err(), missing),
                (
                    keyed(Err(PersistenceError::Unavailable), |row: &u8| *row, failed).err(),
                    failed,
                ),
            ];
            for (error, path) in errors {
                let error = error.ok_or("a read with no record gave one")?;
                assert_eq!(
                    error.to_string(),
                    format!("ABEND: {} (return code 16)", path.reason())
                );
            }
        }
        Ok(())
    }

    /// The census reads every transaction once, through the streamed read alone, and counts
    /// its cards, keeping none; an empty transaction set, a read that fails to start and one
    /// that fails at a row each abend at `TRNXFILE`, before any statement.
    // audit: CBSTM03A-004
    // audit: CBSTM03A-006
    // audit: CBSTM03A-007
    // audit: CBSTM03A-008
    // audit: CBSTM03A-012
    #[test]
    fn the_census_counts_every_transaction_or_abends_at_trnxfile() -> TestResult {
        block_on(async {
            let held = vec![
                transaction(CARD_1, 1)?,
                transaction(CARD_1, 2)?,
                transaction(CARD_2, 3)?,
            ];
            let mut store =
                CountingStore::holding(None)?.with_transactions(held, CensusEnd::Normally);
            let census = transaction_census(&mut store).await?;
            assert_eq!(census.card_count(), 2);
            assert!(census.within_cobol_bounds());
            assert_eq!(
                (store.census_reads, store.card_reads, store.other_calls),
                (1, 0, 0)
            );

            let cases = [
                (
                    Vec::new(),
                    CensusEnd::Normally,
                    ReadFailure::TransactionsEmpty,
                ),
                (
                    vec![transaction(CARD_1, 1)?],
                    CensusEnd::AtStart,
                    ReadFailure::TransactionsUnreadable,
                ),
                (
                    vec![transaction(CARD_1, 1)?],
                    CensusEnd::AtRow,
                    ReadFailure::TransactionsUnreadable,
                ),
            ];
            for (held, end, failure) in cases {
                let mut store = CountingStore::holding(None)?.with_transactions(held, end);
                let error = transaction_census(&mut store)
                    .await
                    .err()
                    .ok_or("a census of no readable transaction")?;
                assert_eq!(error.exit_code(), Some(16), "{end:?}");
                assert_eq!(
                    error.to_string(),
                    format!("ABEND: {} (return code 16)", failure.reason()),
                    "{end:?}"
                );
                assert_eq!((store.census_reads, store.other_calls), (1, 0), "{end:?}");
            }
            Ok(())
        })
    }

    /// A transaction set beyond the 51 cards of the COBOL table, or beyond 10 transactions on a
    /// card, is counted in full and reported by one warning that carries the card count and no
    /// card number; a set within the bounds gives no warning.
    // audit: CBSTM03A-008
    #[test]
    fn a_census_beyond_the_cobol_table_warns_of_blocked_parity() -> TestResult {
        block_on(async {
            let cases: [(u64, u64, bool); 4] = [
                (51, 1, false),
                (51, 10, false),
                (52, 1, true),
                (1, 11, true),
            ];
            for (cards, per_card, warned) in cases {
                let mut held = Vec::new();
                for card in 1..=cards {
                    for n in 1..=per_card {
                        held.push(transaction(numbered_card(card)?, card * 100 + n)?);
                    }
                }
                let mut store =
                    CountingStore::holding(None)?.with_transactions(held, CensusEnd::Normally);
                let capture = trace_capture::capture();
                let census = transaction_census(&mut store).await?;
                let warnings: Vec<String> = capture
                    .lines()
                    .into_iter()
                    .filter(|line| line.contains(r#""level":"WARN""#))
                    .collect();
                let shown = capture.text();
                drop(capture);
                let label = format!("{cards} x {per_card}");
                assert_eq!(census.card_count(), usize::try_from(cards)?, "{label}");
                assert_eq!(census.within_cobol_bounds(), !warned, "{label}");
                if warned {
                    let [warning] = warnings.as_slice() else {
                        return Err(format!("{label}: {} warnings", warnings.len()).into());
                    };
                    assert!(warning.contains("(X-CREASTMT-H2)"), "{label}: {warning}");
                    assert!(
                        warning.contains(&format!(r#""cards":{cards}"#)),
                        "{label}: {warning}"
                    );
                } else {
                    assert!(warnings.is_empty(), "{label}: {warnings:?}");
                }
                let first = String::from_utf8(numbered_card(1)?.to_vec())?;
                assert!(!shown.contains(&first), "{label}: a card number was traced");
            }
            Ok(())
        })
    }

    /// Each statement reads its card's transactions by key once its customer and account are
    /// found, and prints them in transaction-ID order: two, one and none for the three cards. A
    /// card whose transactions cannot be read abends at `TRNXFILE` after that card's masters.
    // audit: CBSTM03A-012
    // audit: CBSTM03A-013
    // audit: CBSTM03A-049
    #[test]
    fn each_statement_reads_its_card_by_key_after_its_masters() -> TestResult {
        block_on(async {
            let held = vec![
                transaction(CARD_1, 1)?,
                transaction(CARD_1, 3)?,
                transaction(CARD_2, 2)?,
            ];
            let xrefs = vec![xref(CARD_1, 1)?, xref(CARD_2, 2)?, xref(CARD_3, 1)?];
            let scratch = Scratch::new()?;
            let dir = scratch.statements();
            let lock = DirectoryLock::acquire(&dir).await?;
            let mut files = StatementFiles::replace(&lock).await?;
            let mut store =
                CountingStore::holding(None)?.with_transactions(held.clone(), CensusEnd::Normally);
            write_statements(&mut store, &xrefs, &mut files).await?;
            assert_eq!(
                (store.customer_reads, store.account_reads, store.card_reads),
                (1, 1, 3)
            );
            assert_eq!((store.census_reads, store.other_calls), (0, 0));
            // 19 text records a statement, and one more for each of its transactions.
            assert_eq!((files.statements, files.text_records), (3, 19 * 3 + 3));
            files.finish().await?;
            let text = fs::read(dir.join(TEXT_FILE))?;
            let ids: Vec<&[u8]> = [16, 17, 21 + 16]
                .iter()
                .filter_map(|&at| text.chunks(STMT_RECORD_LEN + 1).nth(at)?.get(..16))
                .collect();
            assert_eq!(
                ids,
                [
                    b"0000000000000001",
                    b"0000000000000003",
                    b"0000000000000002"
                ]
            );

            let mut files = StatementFiles::replace(&lock).await?;
            let mut store = CountingStore::holding(Some(Unreadable::Transactions))?
                .with_transactions(held, CensusEnd::Normally);
            let error = write_statements(&mut store, &xrefs, &mut files)
                .await
                .err()
                .ok_or("a statement was written without its transactions")?;
            let failure = ReadFailure::TransactionsUnreadable;
            assert_eq!(failure.error_line(), "ERROR READING TRNXFILE");
            assert_eq!(
                error.to_string(),
                format!("ABEND: {} (return code 16)", failure.reason())
            );
            assert_eq!(
                (store.customer_reads, store.account_reads, store.card_reads),
                (1, 1, 1)
            );
            assert_eq!(files.statements, 0);
            drop(files);
            lock.release().await;
            Ok(())
        })
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
        block_on(async {
            let scratch = Scratch::new()?;
            let dir = scratch.statements();
            previous_run(&dir)?;
            let lock = DirectoryLock::acquire(&dir).await?;
            let mut files = StatementFiles::replace(&lock).await?;
            assert_eq!(
                names(&dir)?,
                [LOCK_NAME, HTML_FILE, PDF_FILE, TEXT_FILE].map(String::from)
            );
            for name in [TEXT_FILE, HTML_FILE, PDF_FILE] {
                assert!(
                    fs::read(dir.join(name))?.is_empty(),
                    "{name} kept old bytes"
                );
            }
            files.write(&statement(b"AB", b"<")).await?;
            files.write(&statement(b"C", b">")).await?;
            let outcome = files.finish().await?;
            assert_eq!(
                outcome.written,
                [TEXT_FILE, HTML_FILE, PDF_FILE].map(|name| dir.join(name))
            );
            assert_eq!(outcome.return_code, 0);
            assert_eq!(
                outcome.counts,
                [
                    (JobCounter::Statements, 2),
                    (JobCounter::TextRecords, 3),
                    (JobCounter::HtmlRecords, 2)
                ]
            );
            let text = [b'A', b'B', b'C'].map(|byte| [byte; STMT_RECORD_LEN]);
            assert_eq!(fs::read(dir.join(TEXT_FILE))?, lf_records(&text));
            let html = [b'<', b'>'].map(|byte| [byte; HTML_RECORD_LEN]);
            assert_eq!(fs::read(dir.join(HTML_FILE))?, lf_records(&html));
            assert!(fs::read(dir.join(PDF_FILE))?.starts_with(b"%PDF-"));
            lock.release().await;
            assert_eq!(
                names(&dir)?,
                [HTML_FILE, PDF_FILE, TEXT_FILE].map(String::from)
            );
            Ok(())
        })
    }

    /// With no cross-reference record the run ends normally with two empty statement files and
    /// a PDF of one empty page.
    // audit: CBSTM03A-009
    // audit: CBSTM03A-051
    #[test]
    fn no_cross_reference_gives_empty_statement_files() -> TestResult {
        block_on(async {
            let scratch = Scratch::new()?;
            let dir = scratch.statements();
            let lock = DirectoryLock::acquire(&dir).await?;
            let outcome = StatementFiles::replace(&lock).await?.finish().await?;
            assert_eq!(outcome.written.len(), 3);
            assert!(fs::read(dir.join(TEXT_FILE))?.is_empty());
            assert!(fs::read(dir.join(HTML_FILE))?.is_empty());
            assert!(fs::read(dir.join(PDF_FILE))?.starts_with(b"%PDF-"));
            assert_eq!(
                outcome.counts,
                [
                    (JobCounter::Statements, 0),
                    (JobCounter::TextRecords, 0),
                    (JobCounter::HtmlRecords, 0)
                ]
            );
            Ok(())
        })
    }

    /// A run that ends early, as every abend returns, and a run whose PDF cannot be rendered
    /// both leave none of the three files, the previous run's included.
    // audit: CBSTM03A-002
    // audit: CBSTM03A-049
    #[test]
    fn a_failed_run_leaves_no_statement_file() -> TestResult {
        block_on(async {
            let scratch = Scratch::new()?;
            let dir = scratch.statements();
            previous_run(&dir)?;
            let lock = DirectoryLock::acquire(&dir).await?;
            let mut files = StatementFiles::replace(&lock).await?;
            files.write(&statement(b"A", b"<")).await?;
            drop(files);
            assert_eq!(names(&dir)?, [LOCK_NAME], "an abended run left a file");

            previous_run(&dir)?;
            let mut files = StatementFiles::replace(&lock).await?;
            files.write(&statement(b"AB", b"<")).await?;
            files.write(&statement(&[0x01], b">")).await?;
            let error = files
                .finish()
                .await
                .err()
                .ok_or("a record of no text was rendered")?;
            assert!(
                matches!(error, JobError::Render(RenderError::NonText { line: 3 })),
                "{error}"
            );
            assert_eq!(names(&dir)?, [LOCK_NAME], "a failed render left a file");
            lock.release().await;
            assert!(names(&dir)?.is_empty(), "the lock stayed");
            Ok(())
        })
    }

    /// A second run over the same directory waits for the first, announced by one warning, so
    /// it neither removes nor reports the first run's files while that run writes them, whether
    /// the first completes them or fails and removes them; once the first has ended, the second
    /// replaces them with its own, and its three files are all it leaves.
    // audit: CBSTM03A-002
    // audit: CBSTM03A-049
    // audit: CBSTM03A-050
    #[test]
    fn a_second_run_waits_for_the_first_then_replaces_its_files() -> TestResult {
        block_on_two_workers(async {
            for first_completes in [true, false] {
                let scratch = Scratch::new()?;
                let dir = scratch.statements();
                let first_lock = DirectoryLock::acquire(&dir).await?;
                let mut first = StatementFiles::replace(&first_lock).await?;
                first.write(&statement(b"A", b"<")).await?;

                let capture = trace_capture::capture();
                let target = dir.clone();
                let second = tokio::spawn(
                    async move {
                        let lock = DirectoryLock::acquire(&target).await?;
                        let mut files = StatementFiles::replace(&lock).await?;
                        files.write(&statement(b"B", b">")).await?;
                        let outcome = files.finish().await;
                        lock.release().await;
                        outcome
                    }
                    .with_subscriber(capture.dispatch()),
                );
                let mut waiting = false;
                for _ in 0..1000 {
                    if capture.contains("waiting for it to end") {
                        waiting = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(waiting, "the second run did not wait");
                let label = if first_completes {
                    "completed"
                } else {
                    "failed"
                };
                if first_completes {
                    first.finish().await?;
                    let a = [[b'A'; STMT_RECORD_LEN]];
                    assert_eq!(fs::read(dir.join(TEXT_FILE))?, lf_records(&a));
                } else {
                    drop(first);
                    assert_eq!(names(&dir)?, [LOCK_NAME], "the failed run left a file");
                }
                assert!(
                    !second.is_finished(),
                    "{label}: the second run went on under the lock"
                );
                first_lock.release().await;
                let second = second.await??;
                assert_eq!(
                    second.written,
                    [TEXT_FILE, HTML_FILE, PDF_FILE].map(|name| dir.join(name)),
                    "{label}"
                );
                let b = [[b'B'; STMT_RECORD_LEN]];
                assert_eq!(fs::read(dir.join(TEXT_FILE))?, lf_records(&b), "{label}");
                let html = [[b'>'; HTML_RECORD_LEN]];
                assert_eq!(fs::read(dir.join(HTML_FILE))?, lf_records(&html), "{label}");
                assert!(
                    fs::read(dir.join(PDF_FILE))?.starts_with(b"%PDF-"),
                    "{label}"
                );
                assert_eq!(
                    names(&dir)?,
                    [HTML_FILE, PDF_FILE, TEXT_FILE].map(String::from),
                    "{label}"
                );
            }
            Ok(())
        })
    }

    /// The text records read back for the PDF are taken whole, in order; a short last record,
    /// a record not ended by LF, or a file holding more or fewer records than were written is
    /// invalid data, and no PDF is returned then.
    // audit: CBSTM03A-051
    #[test]
    fn statement_text_is_read_back_as_whole_records() -> TestResult {
        let records = lf_records(&[[b'A'; STMT_RECORD_LEN], [b'B'; STMT_RECORD_LEN]]);
        let mut unterminated = records.clone();
        if let Some(last) = unterminated.last_mut() {
            *last = b'B';
        }
        let short = records.get(..records.len() - 2).ok_or("two records")?;
        assert!(render_text(records.as_slice(), 2)?.starts_with(b"%PDF-"));
        let invalid: [(&[u8], usize); 4] =
            [(short, 2), (&unterminated, 2), (&records, 3), (&records, 1)];
        for (bytes, expected) in invalid {
            let error = render_text(bytes, expected)
                .err()
                .ok_or("invalid statement text was rendered")?;
            assert!(
                matches!(
                    error,
                    JobError::Io {
                        context: "reading statement text",
                        kind: ErrorKind::InvalidData
                    }
                ),
                "{} bytes, {expected} records: {error}",
                bytes.len()
            );
        }
        Ok(())
    }

    /// The masters are read whole, once each, and never by key: customers when the first
    /// statement needs its customer, accounts once that customer is found. No cross-reference
    /// reads neither; a missing first customer ends the run before `accounts` is read; each
    /// missing or unreadable master ends it on its own abend; a card's transactions are read
    /// only once its masters are found; nothing is written to a table.
    // audit: CBSTM03A-009
    // audit: CBSTM03A-010
    // audit: CBSTM03A-011
    // audit: CBSTM03A-012
    // audit: CBSTM03B-004
    // audit: CBSTM03B-005
    // audit: CBSTM03B-006
    #[test]
    fn masters_are_read_whole_once_in_customer_then_account_order() -> TestResult {
        block_on(async {
            let cases = [
                MasterCase {
                    xrefs: vec![xref(CARD_1, 1)?, xref(CARD_2, 2)?, xref(CARD_3, 1)?],
                    unreadable: None,
                    outcome: Ok(3),
                    reads: (1, 1),
                    card_reads: 3,
                },
                MasterCase {
                    xrefs: Vec::new(),
                    unreadable: None,
                    outcome: Ok(0),
                    reads: (0, 0),
                    card_reads: 0,
                },
                MasterCase {
                    xrefs: vec![xref(CARD_1, 999)?, xref(CARD_2, 1)?],
                    unreadable: None,
                    outcome: Err(ReadFailure::CustomerNotFound),
                    reads: (1, 0),
                    card_reads: 0,
                },
                MasterCase {
                    xrefs: vec![xref(CARD_1, 1)?, xref_to(CARD_2, 2, 999)?],
                    unreadable: None,
                    outcome: Err(ReadFailure::AccountNotFound),
                    reads: (1, 1),
                    card_reads: 1,
                },
                MasterCase {
                    xrefs: vec![xref(CARD_1, 1)?],
                    unreadable: Some(Unreadable::Customers),
                    outcome: Err(ReadFailure::CustomerUnreadable),
                    reads: (1, 0),
                    card_reads: 0,
                },
                MasterCase {
                    xrefs: vec![xref(CARD_1, 1)?],
                    unreadable: Some(Unreadable::Accounts),
                    outcome: Err(ReadFailure::AccountUnreadable),
                    reads: (1, 1),
                    card_reads: 0,
                },
            ];
            for case in cases {
                let scratch = Scratch::new()?;
                let lock = DirectoryLock::acquire(&scratch.statements()).await?;
                let mut files = StatementFiles::replace(&lock).await?;
                let mut store = CountingStore::holding(case.unreadable)?
                    .with_transactions(vec![transaction(CARD_1, 1)?], CensusEnd::Normally);
                let written = write_statements(&mut store, &case.xrefs, &mut files).await;
                let label = format!("{:?} of {} records", case.outcome, case.xrefs.len());
                match (written, case.outcome) {
                    (Ok(()), Ok(statements)) => assert_eq!(files.statements, statements, "{label}"),
                    (Err(error), Err(failure)) => assert_eq!(
                        error.to_string(),
                        format!("ABEND: {} (return code 16)", failure.reason()),
                        "{label}"
                    ),
                    (written, _) => return Err(format!("{label}: {written:?}").into()),
                }
                assert_eq!(
                    (store.customer_reads, store.account_reads),
                    case.reads,
                    "{label}"
                );
                assert_eq!(store.card_reads, case.card_reads, "{label}");
                assert_eq!(store.other_calls, 0, "{label}: a keyed read or a write");
            }
            Ok(())
        })
    }

    /// Three synthetic card numbers, in no fixture file.
    const CARD_1: [u8; 16] = *b"4000000000000001";
    const CARD_2: [u8; 16] = *b"4000000000000002";
    const CARD_3: [u8; 16] = *b"4000000000000003";

    /// Synthetic card number `n`, in no fixture file: `4`, then `n` in 15 digits.
    fn numbered_card(n: u64) -> Result<[u8; 16], Box<dyn Error>> {
        Ok(<[u8; 16]>::try_from(format!("4{n:015}").as_bytes())?)
    }

    /// One run of `write_statements` over the fixture masters: the cross-references, the master
    /// that cannot be read, the statements written or the abend, the whole-table reads of
    /// customers and accounts, and the reads of a card's transactions.
    struct MasterCase {
        xrefs: Vec<CardXrefRecord>,
        unreadable: Option<Unreadable>,
        outcome: Result<usize, ReadFailure>,
        reads: (usize, usize),
        card_reads: usize,
    }

    /// The table a [`CountingStore`] cannot read: a master whole, or a card's transactions.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Unreadable {
        Customers,
        Accounts,
        Transactions,
    }

    /// How the census read of a [`CountingStore`] ends.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum CensusEnd {
        /// After its rows, at the end of the table.
        Normally,
        /// Before any row: the read fails to start.
        AtStart,
        /// After its rows, at a row that cannot be read.
        AtRow,
    }

    /// `customers`, `accounts` and `transactions` in memory, counting what a run asks of them:
    /// whole-table reads of each master, the census read and the reads by card of the
    /// transactions, and keyed reads or writes of any other kind, of which `creastmt` makes none.
    struct CountingStore {
        customers: Vec<CustomerRecord>,
        accounts: Vec<AccountRecord>,
        /// The transactions, in card order, then transaction ID.
        transactions: Vec<TransactionRecord>,
        unreadable: Option<Unreadable>,
        census_end: CensusEnd,
        customer_reads: usize,
        account_reads: usize,
        census_reads: usize,
        card_reads: usize,
        other_calls: usize,
    }

    impl CountingStore {
        /// The customers and accounts of `custdata.txt` and `acctdata.txt`, read in place, and
        /// no transaction, with `unreadable` failing its read.
        fn holding(unreadable: Option<Unreadable>) -> Result<Self, Box<dyn Error>> {
            Ok(Self {
                customers: fixture::<CustomerRecord>("custdata.txt")?,
                accounts: fixture::<AccountRecord>("acctdata.txt")?,
                transactions: Vec::new(),
                unreadable,
                census_end: CensusEnd::Normally,
                customer_reads: 0,
                account_reads: 0,
                census_reads: 0,
                card_reads: 0,
                other_calls: 0,
            })
        }

        /// This store holding `transactions`, in card order, with a census read that ends as
        /// `census_end` says.
        fn with_transactions(
            self,
            transactions: Vec<TransactionRecord>,
            census_end: CensusEnd,
        ) -> Self {
            Self {
                transactions,
                census_end,
                ..self
            }
        }

        /// The outcome of a call `creastmt` never makes, counted.
        fn other<T>(&mut self) -> std::future::Ready<Result<T, PersistenceError>> {
            self.other_calls += 1;
            std::future::ready(Err(PersistenceError::Unavailable))
        }

        /// The rows of a whole-table read of `table`, or its failure.
        fn all<R: Clone>(
            &self,
            table: Unreadable,
            rows: &[R],
        ) -> std::future::Ready<Result<Vec<R>, PersistenceError>> {
            std::future::ready(if self.unreadable == Some(table) {
                Err(PersistenceError::Unavailable)
            } else {
                Ok(rows.to_vec())
            })
        }
    }

    impl CustomerRepository for CountingStore {
        fn find_by_id(
            &mut self,
            _id: PicU<9>,
        ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn lock_by_id(
            &mut self,
            _id: PicU<9>,
        ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn find_all(
            &mut self,
        ) -> impl Future<Output = Result<Vec<CustomerRecord>, PersistenceError>> + Send {
            self.customer_reads += 1;
            self.all(Unreadable::Customers, &self.customers)
        }

        fn insert(
            &mut self,
            _record: &CustomerRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            self.other()
        }

        fn update(
            &mut self,
            _record: &CustomerRecord,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            self.other()
        }

        fn replace_all(
            &mut self,
            _records: &[CustomerRecord],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            self.other()
        }
    }

    impl AccountRepository for CountingStore {
        fn find_with_customer(
            &mut self,
            _acct_id: PicU<11>,
        ) -> impl Future<Output = AccountCustomerChain> + Send {
            self.other_calls += 1;
            std::future::ready(AccountCustomerChain {
                xref: ChainLink::NotFound,
                account: ChainLink::NotReached,
                customer: ChainLink::NotReached,
            })
        }

        fn find_by_id(
            &mut self,
            _id: PicU<11>,
        ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn lock_by_id(
            &mut self,
            _id: PicU<11>,
        ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn find_all(
            &mut self,
        ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send {
            self.account_reads += 1;
            self.all(Unreadable::Accounts, &self.accounts)
        }

        fn insert(
            &mut self,
            _record: &AccountRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            self.other()
        }

        fn update(
            &mut self,
            _record: &AccountRecord,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            self.other()
        }

        fn replace_all(
            &mut self,
            _records: &[AccountRecord],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            self.other()
        }
    }

    impl TransactionRepository for CountingStore {
        fn find_by_id(
            &mut self,
            _id: &PicX<16>,
        ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send
        {
            self.other()
        }

        fn find_last(
            &mut self,
        ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send
        {
            self.other()
        }

        fn next_id(&mut self) -> impl Future<Output = Result<PicX<16>, PersistenceError>> + Send {
            self.other()
        }

        fn insert(
            &mut self,
            _record: &TransactionRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            self.other()
        }

        fn page_after(
            &mut self,
            _start: &PicX<16>,
            _limit: i64,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn page_before(
            &mut self,
            _start: &PicX<16>,
            _limit: i64,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn find_all_in_card_order(
            &mut self,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.other()
        }

        fn stream_in_card_order(
            &mut self,
        ) -> impl Future<Output = Result<TransactionRows<'_>, PersistenceError>> + Send {
            self.census_reads += 1;
            let mut rows: Vec<Result<TransactionRecord, PersistenceError>> =
                self.transactions.iter().cloned().map(Ok).collect();
            let read = match self.census_end {
                CensusEnd::AtStart => Err(PersistenceError::Unavailable),
                CensusEnd::AtRow => {
                    rows.push(Err(PersistenceError::Unavailable));
                    Ok(TransactionRows::from_results(rows))
                }
                CensusEnd::Normally => Ok(TransactionRows::from_results(rows)),
            };
            std::future::ready(read)
        }

        fn find_by_card(
            &mut self,
            card: &CardNumber,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.card_reads += 1;
            std::future::ready(if self.unreadable == Some(Unreadable::Transactions) {
                Err(PersistenceError::Unavailable)
            } else {
                Ok(self
                    .transactions
                    .iter()
                    .filter(|record| record.card_num == *card)
                    .cloned()
                    .collect())
            })
        }

        fn find_processed_between(
            &mut self,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.other()
        }
    }

    /// The records of the fixture `file`, read in place from `app/data/ASCII/`.
    fn fixture<R: FixedRecord>(file: &str) -> Result<Vec<R>, Box<dyn Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file);
        Ok(decode_fixture::<R>(&fs::read(path)?)?
            .into_iter()
            .map(|line| line.record)
            .collect())
    }

    /// A purchase on `card` with the transaction ID `n`, in 16 digits.
    fn transaction(card: [u8; 16], n: u64) -> Result<TransactionRecord, Box<dyn Error>> {
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
            card_num: CardNumber::new(PicX::from_array(card)),
            orig_ts: PicTimestamp::decode(b"2022-06-10 19:27:53.000000")?,
            proc_ts: PicTimestamp::decode(b"2022-07-01-10.15.42.370000")?,
            filler: PicX::spaces(),
        })
    }
}
