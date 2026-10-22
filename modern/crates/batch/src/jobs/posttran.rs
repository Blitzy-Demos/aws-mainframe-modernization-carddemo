//! `carddemo-batch posttran`: job POSTTRAN, whose `STEP15` runs `CBTRN02C` to post the daily
//! transaction file (`POSTTRAN.jcl:23-42`; AAP 0.3.4.3). Every rule is
//! `domain::batch::cbtrn02c`'s; this module opens the files in the program's order, applies the
//! writes each posting returns, writes the rejects and reproduces the failure paths, after the
//! `CBTRN02C-` rows of `modern/audit/batch/POSTTRAN.md`.
//!
//! # Flow
//!
//! 1. `START OF EXECUTION OF PROGRAM CBTRN02C`, then the six opens in the program's order
//!    (CBTRN02C-031, CBTRN02C-034):
//!    - `DALYTRAN`: the `--dalytran` file is read in place, never written (CBTRN02C-033), and
//!      decoded whole by codec A, line n as record n (CBTRN02C-001). A line codec A refuses, or
//!      whose body is not one 350-byte record, ends the job before any other effect: no table
//!      changes and no reject generation is created (CBTRN02C-032, CBTRN02C-045).
//!    - `TRANFILE`: the run opens its session, the `PgSession` that holds its writer claims and
//!      runs every statement of the run, and claims `transactions` for writing on it
//!      (CBTRN02C-046); then `transactions` must hold no row, the only state in which the
//!      `OPEN OUTPUT` of the cluster, defined without `REUSE`, succeeds (CBTRN02C-030).
//!    - `XREFFILE`, `ACCTFILE` and `TCATBALF`: `card_xref`, `accounts` and `tran_cat_balances`
//!      are read whole into [`Posttran::new`], which serves the program's keyed reads and holds
//!      the values each posting leaves for the next record (CBTRN02C-004, CBTRN02C-005,
//!      CBTRN02C-016, CBTRN02C-022). The `OPEN I-O` of `ACCTFILE` and of `TCATBALF` claims
//!      `accounts` and `tran_cat_balances` for writing on the session before the table is read
//!      (CBTRN02C-046); `XREFFILE` is opened for input, which claims nothing.
//!    - `DALYREJS`: the next `DALYREJS` generation in `Config::dalyrejs_dir`, created before the
//!      first record is read, so that it exists, empty, when nothing is rejected (CBTRN02C-028).
//! 2. Each record goes to [`Posttran::process`] in input order (CBTRN02C-002). A posting's writes
//!    are issued in `2000-POST-TRANSACTION`'s order: the category balance (`2700`), the account
//!    (`2800`), then the transaction (`2900`) (CBTRN02C-015 to CBTRN02C-024). A rejected record is
//!    written to the generation as its 430 bytes and LF: the 350 input bytes, the four-digit
//!    reason and the 76-byte description (CBTRN02C-026, CBTRN02C-027).
//! 3. After the last record the generation is closed, the two count lines and the end line follow
//!    (CBTRN02C-034), and the job returns the generation's path, which `main` prints once the
//!    file is complete, and exit status 4 when any record was rejected, 0 otherwise
//!    (CBTRN02C-028, CBTRN02C-029, CBTRN02C-035). The counters are `processed` and `rejected`
//!    (CBTRN02C-036).
//! 4. Once the end line is out, the run's session is closed, which gives up its three writer
//!    claims, as the closes of `TRANFILE`, `ACCTFILE` and `TCATBALF` end the program's hold on
//!    the clusters (CBTRN02C-046).
//!
//! # One writer per cluster
//!
//! `TRANFILE`, `ACCTFILE` and `TCATBALF` are defined `SHAREOPTIONS(2 3)` (`TRANFILE.jcl:55`,
//! `ACCTFILE.jcl:42`, `TCATBALF.jcl:42`) and allocated `DISP=SHR` (`POSTTRAN.jcl:28,39,41`), so
//! nothing but VSAM keeps two writers apart, and VSAM fails the second writer's `OPEN` with
//! status `93`. The job keeps that rule through `PgSession::claim_writer`: each of the three
//! opens takes its table's claim on the run's session before the table is read, and a claim
//! another run holds, a second `posttran` or an `intcalc` holding `accounts`, takes the open's
//! failure path. Without it two runs would each rewrite the balances from their own
//! start-of-run read, and the later write would wipe out the other run's postings.
//!
//! The claims fence the run's writes because every statement of the run, each read and each
//! write, runs on the session that holds them, a connection that is never replaced. A session
//! the server ends, through `pg_terminate_backend`, `idle_session_timeout` or a lost network,
//! takes the claims with it, and the run's next statement fails on it: the run abends there
//! with exit status 12, through that statement's failure path, and writes nothing after its
//! claims are gone, so a second run that claims the tables then is the only writer. The claims
//! are held to the run's end and given up by closing the session on success; on every failure
//! path the session drops with the run, which closes it and ends the locks on the server. A
//! close that fails is reported in a warning event and changes nothing else: the run's writes
//! are done, and the session's connection is closed either way, which ends its locks.
//!
//! # No unit of work
//!
//! VSAM keeps every completed write, so a run that fails part way leaves that record's category
//! balance and account changed and every earlier record's effects in place (CBTRN02C-015,
//! CBTRN02C-025, CBTRN02C-044). Each write is therefore its own statement on the run's session
//! (`PgSession`), in autocommit mode, committed as it completes, and never part of a transaction
//! a later failure would undo; no transaction is opened here (X-POSTTRAN-1). A rerun after such
//! a failure meets the non-empty `transactions` of CBTRN02C-030, as the mainframe rerun meets a
//! non-empty cluster.
//!
//! # Failure paths
//!
//! | Operation | Line shown first | Status shown | Rules |
//! |---|---|---|---|
//! | `--dalytran` cannot be opened for reading | `ERROR OPENING DALYTRAN` | stand-in `0030` | CBTRN02C-031 |
//! | `--dalytran` opens but cannot be read, as a directory | `ERROR READING DALYTRAN FILE` | stand-in `0030` | CBTRN02C-038 |
//! | a line codec A refuses, or a body that is not 350 bytes | none; an event names the file, the line and the failing rule | none | CBTRN02C-032, CBTRN02C-045 |
//! | another run holds the claim on `transactions` | `ERROR OPENING TRANSACTION FILE` | `9243`, not claimed as parity | CBTRN02C-046 |
//! | the run's session cannot be opened, the claim on `transactions` fails, or `transactions` cannot be read | `ERROR OPENING TRANSACTION FILE` | stand-in `0030` | CBTRN02C-031 |
//! | `transactions` holds a row | `ERROR OPENING TRANSACTION FILE` | `0037`, not claimed as parity | CBTRN02C-030 |
//! | `card_xref` or `accounts` cannot be read; an account update fails | none; an event names the operation and the error | none | CBTRN02C-039 |
//! | the generation cannot be allocated or created | `ERROR OPENING DALY REJECTS FILE` | stand-in `0030` | CBTRN02C-031 |
//! | another run holds the claim on `accounts` | `ERROR OPENING ACCOUNT MASTER FILE` | `9243`, not claimed as parity | CBTRN02C-046 |
//! | the claim on `accounts` fails | `ERROR OPENING ACCOUNT MASTER FILE` | stand-in `0030` | CBTRN02C-031 |
//! | another run holds the claim on `tran_cat_balances` | `ERROR OPENING TRANSACTION BALANCE FILE` | `9243`, not claimed as parity | CBTRN02C-046 |
//! | the claim on `tran_cat_balances` fails | `ERROR OPENING TRANSACTION BALANCE FILE` | stand-in `0030` | CBTRN02C-031 |
//! | `tran_cat_balances` cannot be read | `ERROR READING TRANSACTION BALANCE FILE` | stand-in `0030` | CBTRN02C-038 |
//! | a category balance insert fails | `ERROR WRITING TRANSACTION BALANCE FILE` | `0022` for a duplicate key, else stand-in `0030` | CBTRN02C-038 |
//! | a category balance update fails or matches no row | `ERROR REWRITING TRANSACTION BALANCE FILE` | stand-in `0030`, or `0023` for no row | CBTRN02C-038 |
//! | a transaction insert fails | `ERROR WRITING TO TRANSACTION FILE` | `0022` for a duplicate key, else stand-in `0030` | CBTRN02C-025 |
//! | a reject record cannot be written | `ERROR WRITING TO REJECTS FILE` | stand-in `0030` | CBTRN02C-038 |
//! | the generation cannot be flushed or synced | `ERROR CLOSING DAILY REJECTS FILE` | `0000` | CBTRN02C-040 |
//!
//! - The status line is `9910-DISPLAY-IO-STATUS`'s, `FILE STATUS IS: NNNN` and four characters
//!   (CBTRN02C-041). `0022` for a duplicate transaction key is the parity value (CBTRN02C-025),
//!   and so is `0000` for the reject close, which shows `XREFFILE-STATUS` after the
//!   cross-reference close has set it to `00` (CBTRN02C-040). Every other status is
//!   `HALTED(H4)`: a `PostgreSQL` or file-system error has no file status, so the job shows `0023`
//!   for a rewrite that found no row, `0037` for the non-empty cluster IBM documents
//!   (X-POSTTRAN-H1), `9243`, the line of status `93`, for a cluster another run holds open
//!   (CBTRN02C-046), and `0030`, the permanent-error status, for any other failure, none of them
//!   a parity result (CBTRN02C-042, X-POSTTRAN-H3).
//! - Every path ends with `ABENDING PROGRAM` and [`JobError::Abend`] with exit status 12, the
//!   value X-POSTTRAN-2 proposes for the user abend `U0999` that `CEE3ABD` raises: neither 0 nor
//!   4 (CBTRN02C-043). The unfinished generation is then dropped, and its guard removes it, so no
//!   path is printed and the next run takes the same generation number, as
//!   `DISP=(NEW,CATLG,DELETE)` deletes it at abend; nothing is rolled back (CBTRN02C-044).
//! - Before the program's lines, a failed database call adds one event naming the file operation
//!   it replaces and the error, whose text is an SQLSTATE, a constraint or a column only.
//! - A session lost during the run has no row of its own: the statement after the loss fails,
//!   whichever it is, and takes its row above (CBTRN02C-046).
//! - A run counter that outgrows 64 bits ends the job with [`JobError::Posttran`]; no run can
//!   reach it, and it has no COBOL path (CBTRN02C-036).
//!
//! # Defects reproduced
//!
//! The domain reproduces them: reason `0103` overwriting `0102` (E09, CBTRN02C-008), the
//! high-order truncation of `WS-TEMP-BAL` (E10, CBTRN02C-006), the negative cycle debit adding
//! its magnitude (ARITH-033) and the `FILLER` a created category balance takes over
//! (CBTRN02C-017). The job's share is the reason `0109` of a failed account rewrite, which has no
//! effect: an update that matches no row is passed over and the transaction is still written
//! (CBTRN02C-023, X-POSTTRAN-3). None of them is corrected here (Behaviour constraint).
//!
//! # Job log
//!
//! Each `DISPLAY` the program performs is one tracing event whose message is the line's text
//! exactly, in program order: `INFO` for the job-log lines of a run (the start line, one
//! `TCATBAL record not found for key : ` line before each category balance is created, the two
//! count lines and the end line) and `ERROR` for the lines a failure path adds (CBTRN02C-016,
//! CBTRN02C-034, X-POSTTRAN-4). The events reach standard output as the JSON stream of
//! `runtime::telemetry`, so the stdout lines that are not JSON stay the written path alone.
//!
//! # Rules governing this module
//!
//! - Single Schema Executor: no schema work here. Against a database whose schema has not been
//!   applied, the check of `transactions` fails first and the job ends on
//!   `ERROR OPENING TRANSACTION FILE` (CBTRN02C-031).
//! - Cardholder Data Redaction: the reject records and `transactions.card_num` keep the full card
//!   number, a local file and a table rather than egress. No event, error or counter carries a
//!   card number or a record's bytes: a refused line is named by its number and the rule it
//!   breaks, a database error by its SQLSTATE and constraint, and every [`JobError`] reason is
//!   static text (CBTRN02C-037). No record is ever formatted here.
//! - Typed Dates and Timestamps: `TRAN-PROC-TS` comes typed from the domain, the injected clock's
//!   instant in layout `D` (CBTRN02C-013, E33); no timestamp text is built here.
//! - Total Parsers Over Untrusted Input: the input is decoded only by codec A's total decoder,
//!   and nothing here indexes input bytes.
//! - Local-Only Runtime: the job writes only the generation under `Config::dalyrejs_dir` and the
//!   `PostgreSQL` tables.
//! - R4: every statement is a call of `persistence` on the run's session: a repository call, a
//!   writer claim of `PgSession::claim_writer`, or the session's close.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use copybook::FixedRecord;
use domain::batch::cbtrn02c::{Posting, Posttran, RecordOutcome, START_LINE, TcatbalWrite};
use domain::batch::loads::{LoadError, decode_fixture};
use domain::records::{AccountRecord, DailyTransactionRecord, RejectRecord, TransactionRecord};
use persistence::{
    AccountRepository, CardXrefRepository, Dataset, PersistenceError, PgSession, PgStore,
    TranCatBalanceRepository, TransactionRepository,
};
use runtime::telemetry::JobCounter;

use super::{
    DALYREJS_DIR_SETTING, JobContext, JobError, JobOutcome, OutputFile, READING_INPUT, blocking, io,
};

/// The generation base of the reject file, `AWS.M2.CARDDEMO.DALYREJS(+1)` (`POSTTRAN.jcl:38`;
/// CBTRN02C-028).
const REJECTS_BASE: &str = "DALYREJS";

/// The name the events of a refused input line give the file: its DD name, never the path
/// (CBTRN02C-032).
const DALYTRAN: &str = "DALYTRAN";

/// The exit status of every `9999-ABEND-PROGRAM` path. `CEE3ABD` with abend code 999 ends the
/// step with user abend `U0999`, which carries no return code; 12 is the value X-POSTTRAN-2
/// proposes: neither 0 nor 4, apart from the usage error 2 and the return code 16 of
/// `CBPAUP0C`, and the value `CBTRN02C` moves to `APPL-RESULT` on every failure that leads to the
/// abend (CBTRN02C-043).
const ABEND_EXIT_STATUS: u8 = 12;

/// `DISPLAY 'ABENDING PROGRAM'`, the last line of every failure path (`CBTRN02C.cbl:708`;
/// CBTRN02C-043).
const ABEND_LINE: &str = "ABENDING PROGRAM";

/// The failed open of the input (`CBTRN02C.cbl:247`; CBTRN02C-031).
const OPEN_DALYTRAN: &str = "ERROR OPENING DALYTRAN";

/// A failed read of the input once it is open (`CBTRN02C.cbl:363`; CBTRN02C-038).
const READ_DALYTRAN: &str = "ERROR READING DALYTRAN FILE";

/// The failed open of the transaction file (`CBTRN02C.cbl:265`; CBTRN02C-030, CBTRN02C-031,
/// CBTRN02C-046).
const OPEN_TRANFILE: &str = "ERROR OPENING TRANSACTION FILE";

/// The failed open of the reject generation (`CBTRN02C.cbl:302`; CBTRN02C-031).
const OPEN_DALYREJS: &str = "ERROR OPENING DALY REJECTS FILE";

/// The failed open of the account file (`CBTRN02C.cbl:320`; CBTRN02C-031, CBTRN02C-046).
const OPEN_ACCTFILE: &str = "ERROR OPENING ACCOUNT MASTER FILE";

/// The failed open of the category balance file (`CBTRN02C.cbl:338`; CBTRN02C-031,
/// CBTRN02C-046).
const OPEN_TCATBALF: &str = "ERROR OPENING TRANSACTION BALANCE FILE";

/// A failed write of a reject record (`CBTRN02C.cbl:460`; CBTRN02C-038).
const WRITE_DALYREJS: &str = "ERROR WRITING TO REJECTS FILE";

/// A failed read of the category balance file (`CBTRN02C.cbl:489`; CBTRN02C-038).
const READ_TCATBAL: &str = "ERROR READING TRANSACTION BALANCE FILE";

/// A failed write of a new category balance (`CBTRN02C.cbl:520`; CBTRN02C-038).
const WRITE_TCATBAL: &str = "ERROR WRITING TRANSACTION BALANCE FILE";

/// A failed rewrite of a category balance (`CBTRN02C.cbl:538`; CBTRN02C-038).
const REWRITE_TCATBAL: &str = "ERROR REWRITING TRANSACTION BALANCE FILE";

/// A failed write of a transaction (`CBTRN02C.cbl:574`; CBTRN02C-025).
const WRITE_TRANFILE: &str = "ERROR WRITING TO TRANSACTION FILE";

/// A failed close of the reject generation (`CBTRN02C.cbl:648`; CBTRN02C-040).
const CLOSE_DALYREJS: &str = "ERROR CLOSING DAILY REJECTS FILE";

// The reasons of the paths that display no line of their own: static text naming the failure,
// never a key, a value or a byte of the input (CBTRN02C-037).

/// A line codec A refuses (CBTRN02C-032).
const LINE_REFUSED: &str = "a DALYTRAN line is no CVTRA06Y record";

/// A line whose body is not the 350-byte record (CBTRN02C-045).
const LINE_NOT_A_RECORD: &str = "a DALYTRAN line is not one 350-byte record";

/// The cross-reference could not be read (CBTRN02C-039).
const XREF_READ_FAILED: &str = "the card cross-reference could not be read";

/// The account file could not be read (CBTRN02C-039).
const ACCOUNTS_READ_FAILED: &str = "the account file could not be read";

/// An account could not be rewritten (CBTRN02C-039).
const ACCOUNT_REWRITE_FAILED: &str = "an account could not be rewritten";

// The file operation each database or file call replaces, named in the event that reports its
// failure.

/// `0000-DALYTRAN-OPEN`.
const OPEN_DALYTRAN_OP: &str = "OPEN DALYTRAN";
/// The reads of `1000-DALYTRAN-GET-NEXT`, done at once.
const READ_DALYTRAN_OP: &str = "READ DALYTRAN";
/// `0100-TRANFILE-OPEN`.
const OPEN_TRANFILE_OP: &str = "OPEN TRANFILE";
/// The keyed reads of `1500-A-LOOKUP-XREF`, done at once.
const READ_XREFFILE_OP: &str = "READ XREFFILE";
/// `0300-DALYREJS-OPEN`.
const OPEN_DALYREJS_OP: &str = "OPEN DALYREJS";
/// `0400-ACCTFILE-OPEN`.
const OPEN_ACCTFILE_OP: &str = "OPEN ACCTFILE";
/// `0500-TCATBALF-OPEN`.
const OPEN_TCATBALF_OP: &str = "OPEN TCATBALF";
/// The keyed reads of `1500-B-LOOKUP-ACCT`, done at once.
const READ_ACCTFILE_OP: &str = "READ ACCTFILE";
/// The keyed reads of `2700-UPDATE-TCATBAL`, done at once.
const READ_TCATBALF_OP: &str = "READ TCATBALF";
/// The `WRITE` of `2700-A-CREATE-TCATBAL-REC`.
const WRITE_TCATBALF_OP: &str = "WRITE TCATBALF";
/// The `REWRITE` of `2700-B-UPDATE-TCATBAL-REC`.
const REWRITE_TCATBALF_OP: &str = "REWRITE TCATBALF";
/// The `REWRITE` of `2800-UPDATE-ACCOUNT-REC`.
const REWRITE_ACCTFILE_OP: &str = "REWRITE ACCTFILE";
/// The `WRITE` of `2900-WRITE-TRANSACTION-FILE`.
const WRITE_TRANFILE_OP: &str = "WRITE TRANFILE";
/// The `WRITE` of `2500-WRITE-REJECT-REC`.
const WRITE_DALYREJS_OP: &str = "WRITE DALYREJS";
/// `9300-DALYREJS-CLOSE`.
const CLOSE_DALYREJS_OP: &str = "CLOSE DALYREJS";
/// `9100-TRANFILE-CLOSE`, `9400-ACCTFILE-CLOSE` and `9500-TCATBALF-CLOSE`, which the close of
/// the run's session, and with it of the three writer claims it holds, stands for.
const CLOSE_CLAIMED_FILES_OP: &str = "CLOSE TRANFILE ACCTFILE TCATBALF";

/// The file status `9910-DISPLAY-IO-STATUS` shows after a failure line (CBTRN02C-041). Every
/// status but `93` is numeric and does not start with `9`, so its line is
/// `FILE STATUS IS: NNNN`, `00` and the two status characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// `00`: the reject close shows `XREFFILE-STATUS`, which the cross-reference close has just
    /// set to `00` (CBTRN02C-040, X-POSTTRAN-6).
    CrossReferenceClosed,
    /// `22`, a duplicate key (CBTRN02C-025).
    DuplicateKey,
    /// `23`, record not found: the stand-in for a rewrite that matched no row, `HALTED(H4)`
    /// (CBTRN02C-042).
    NotFound,
    /// `37`, `OPEN OUTPUT` of a cluster defined without `REUSE` that holds records, as IBM
    /// documents it; halt candidate X-POSTTRAN-H1, not claimed as parity (CBTRN02C-030).
    ClusterNotEmpty,
    /// `30`, a permanent error: the stand-in for every other database or file-system failure,
    /// `HALTED(H4)` (CBTRN02C-042, X-POSTTRAN-H3).
    Permanent,
    /// `93`, resource not available: VSAM fails the `OPEN OUTPUT` or `OPEN I-O` of a
    /// `SHAREOPTIONS(2 3)` cluster that another job has open for output or update
    /// (CBTRN02C-046). The status starts with `9`, so the line shows `9`, then the binary value
    /// of the second character's EBCDIC byte as three digits, `3` being `X'F3'`, 243
    /// (CBTRN02C-041). `HALTED(H4)`: the status VSAM returns there is not claimed as parity
    /// (CBTRN02C-042).
    ResourceUnavailable,
}

impl Status {
    /// The status shown for a failed repository call: a duplicate key is `22`, as VSAM reports
    /// it (CBTRN02C-025); every other error has no file status.
    const fn of(error: &PersistenceError) -> Self {
        if error.is_duplicate() {
            Self::DuplicateKey
        } else {
            Self::Permanent
        }
    }

    /// The line `9910-DISPLAY-IO-STATUS` displays (CBTRN02C-041).
    const fn line(self) -> &'static str {
        match self {
            Self::CrossReferenceClosed => "FILE STATUS IS: NNNN0000",
            Self::DuplicateKey => "FILE STATUS IS: NNNN0022",
            Self::NotFound => "FILE STATUS IS: NNNN0023",
            Self::ClusterNotEmpty => "FILE STATUS IS: NNNN0037",
            Self::Permanent => "FILE STATUS IS: NNNN0030",
            Self::ResourceUnavailable => "FILE STATUS IS: NNNN9243",
        }
    }
}

/// One failure path of `CBTRN02C`: what it displays before `ABENDING PROGRAM`, and the reason of
/// the [`JobError::Abend`] it ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Failure {
    /// The `DISPLAY` naming the failed operation and the status shown after it; `None` for a path
    /// with no line of its own (CBTRN02C-032, CBTRN02C-039, CBTRN02C-045).
    shown: Option<(&'static str, Status)>,
    /// Static text naming the abend.
    reason: &'static str,
}

impl Failure {
    /// A path that displays `line` and the status line of `status`; its reason is `line`.
    const fn shown(line: &'static str, status: Status) -> Self {
        Self {
            shown: Some((line, status)),
            reason: line,
        }
    }

    /// A path with no line of its own before `ABENDING PROGRAM`.
    const fn unshown(reason: &'static str) -> Self {
        Self {
            shown: None,
            reason,
        }
    }
}

/// `transactions` holds a row when the job starts: `ERROR OPENING TRANSACTION FILE` and status
/// `37`, the outcome halt candidate X-POSTTRAN-H1 points to, which also keeps a file from being
/// posted twice (CBTRN02C-030).
const CLUSTER_NOT_EMPTY: Failure = Failure {
    shown: Some((OPEN_TRANFILE, Status::ClusterNotEmpty)),
    reason: "ERROR OPENING TRANSACTION FILE: transactions already holds a row \
             (BLOCKED, X-POSTTRAN-H1)",
};

/// One of the program's three opens for output or update, each of which claims its table for
/// writing before the table is read (CBTRN02C-046).
struct WriterOpen {
    /// The table claimed.
    dataset: Dataset,
    /// The file operation the claim replaces, named in the event of a claim that fails as a
    /// database call.
    operation: &'static str,
    /// The open's failure line, shown on every failure of its claim (CBTRN02C-031).
    line: &'static str,
    /// The abend reason of a claim another run holds: the failure line and the cause, static
    /// text naming the table only.
    in_use: &'static str,
}

impl WriterOpen {
    /// The path of a claim another run holds: the open's failure line, the status line of `93`
    /// and the abend (CBTRN02C-041, CBTRN02C-046).
    const fn refused(&self) -> Failure {
        Failure {
            shown: Some((self.line, Status::ResourceUnavailable)),
            reason: self.in_use,
        }
    }
}

/// `0100-TRANFILE-OPEN`, the `OPEN OUTPUT` of the transaction cluster (CBTRN02C-046).
const TRANFILE: WriterOpen = WriterOpen {
    dataset: Dataset::Transactions,
    operation: OPEN_TRANFILE_OP,
    line: OPEN_TRANFILE,
    in_use: "ERROR OPENING TRANSACTION FILE: another run is writing transactions",
};

/// `0400-ACCTFILE-OPEN`, the `OPEN I-O` of the account cluster (CBTRN02C-046).
const ACCTFILE: WriterOpen = WriterOpen {
    dataset: Dataset::Accounts,
    operation: OPEN_ACCTFILE_OP,
    line: OPEN_ACCTFILE,
    in_use: "ERROR OPENING ACCOUNT MASTER FILE: another run is writing accounts",
};

/// `0500-TCATBALF-OPEN`, the `OPEN I-O` of the category balance cluster (CBTRN02C-046).
const TCATBALF: WriterOpen = WriterOpen {
    dataset: Dataset::TranCatBalances,
    operation: OPEN_TCATBALF_OP,
    line: OPEN_TCATBALF,
    in_use: "ERROR OPENING TRANSACTION BALANCE FILE: another run is writing tran_cat_balances",
};

/// Runs POSTTRAN over the daily transaction file at `dalytran`: posts every record that passes
/// validation, writes the rejected ones to the next `DALYREJS` generation under
/// `Config::dalyrejs_dir`, and returns that generation's path, the program's return code, 4 when
/// any record was rejected and 0 otherwise, and the counters `processed` and `rejected`
/// (CBTRN02C-028, CBTRN02C-035, CBTRN02C-036).
///
/// # Errors
///
/// - [`JobError::Abend`] with exit status 12 on every failure path of the module documentation,
///   after its job-log lines and `ABENDING PROGRAM` (CBTRN02C-025, CBTRN02C-030 to CBTRN02C-032,
///   CBTRN02C-038 to CBTRN02C-046). The writes completed before the failure stand, the reject
///   generation is removed and the run's session is dropped, which gives its writer claims up.
///   A session lost during the run fails the statement that follows the loss, which takes that
///   statement's path (CBTRN02C-046).
/// - [`JobError::Posttran`] when a run counter outgrows 64 bits (CBTRN02C-036).
pub(crate) async fn run(ctx: &JobContext<'_>, dalytran: &Path) -> Result<JobOutcome, JobError> {
    tracing::info!("{START_LINE}");
    let records = open_dalytran(dalytran).await?;
    // Every statement of the run goes to its one session, the one holding its writer claims, so
    // that a session the server ends takes the claims and the run's next statement with it, and
    // no write follows the loss (CBTRN02C-046). Each statement on the session commits on its
    // own, as VSAM keeps each completed write (CBTRN02C-015, X-POSTTRAN-1).
    let mut session = open_tranfile(ctx.store).await?;
    let xrefs = CardXrefRepository::find_all(&mut session)
        .await
        .map_err(|error| unshown_failure(READ_XREFFILE_OP, XREF_READ_FAILED, &error))?;
    let mut rejects = open_dalyrejs(&ctx.config.dalyrejs_dir()).await?;
    open_for_writing(&mut session, &ACCTFILE).await?;
    let accounts = AccountRepository::find_all(&mut session)
        .await
        .map_err(|error| unshown_failure(READ_ACCTFILE_OP, ACCOUNTS_READ_FAILED, &error))?;
    open_for_writing(&mut session, &TCATBALF).await?;
    let tcatbals = TranCatBalanceRepository::find_all(&mut session)
        .await
        .map_err(|error| shown_failure(READ_TCATBALF_OP, READ_TCATBAL, &error))?;
    let mut posttran = Posttran::new(accounts, xrefs, tcatbals);
    for daily in &records {
        match posttran.process(daily, ctx.clock)? {
            RecordOutcome::Posted(posting) => apply_posting(&mut session, &posting).await?,
            RecordOutcome::Rejected(reject) => write_reject(&mut rejects, &reject).await?,
        }
    }
    let summary = posttran.finish();
    let path = close_dalyrejs(rejects).await?;
    // Unlike jobs that discard their program's job log, this one emits it: the POSTTRAN audit
    // maps every `DISPLAY` to one event, and these lines carry counts only (CBTRN02C-034).
    for line in summary.sysout_lines() {
        tracing::info!("{line}");
    }
    close_claimed_files(session).await;
    Ok(JobOutcome {
        written: vec![path],
        return_code: summary.return_code(),
        counts: vec![
            (JobCounter::Processed, summary.processed()),
            (JobCounter::Rejected, summary.rejected()),
        ],
    })
}

/// `0000-DALYTRAN-OPEN` and every `1000-DALYTRAN-GET-NEXT` at once: the whole input, read in
/// place and decoded by codec A in file order, line n as record n (CBTRN02C-001, CBTRN02C-033).
/// An input of no bytes holds no record (CBTRN02C-029).
///
/// # Errors
///
/// [`JobError::Abend`] when the file cannot be opened for reading (`ERROR OPENING DALYTRAN`,
/// CBTRN02C-031), when it opens but cannot be read (`ERROR READING DALYTRAN FILE`,
/// CBTRN02C-038), when codec A refuses a line (CBTRN02C-032), or when a line's body is not the
/// 350-byte record, which is never padded into one (CBTRN02C-045). Nothing else has happened
/// then.
///
/// The read and the decoding block, so they run as one step on the blocking pool, awaited at
/// once; the step only returns what failed, and the events of the failure are emitted here,
/// in the job's span, as when the step ran inline (W011-F05).
async fn open_dalytran(path: &Path) -> Result<Vec<DailyTransactionRecord>, JobError> {
    let path = path.to_path_buf();
    let decoded = blocking(READING_INPUT, move || {
        let bytes = read_dalytran(&path)?;
        decode_fixture::<DailyTransactionRecord>(&bytes).map_err(DalytranFailure::Refused)
    })
    .await;
    let lines = match decoded {
        Ok(Ok(lines)) => lines,
        Ok(Err(DalytranFailure::Unopenable(error))) | Err(error) => {
            return Err(file_failure(OPEN_DALYTRAN_OP, OPEN_DALYTRAN, &error));
        }
        Ok(Err(DalytranFailure::Unreadable(error))) => {
            return Err(file_failure(READ_DALYTRAN_OP, READ_DALYTRAN, &error));
        }
        Ok(Err(DalytranFailure::Refused(error))) => return Err(refused(&error)),
    };
    let mut records = Vec::with_capacity(lines.len());
    for (index, line) in lines.into_iter().enumerate() {
        if line.framing.body_len != DailyTransactionRecord::WIDTH {
            return Err(not_a_record(index.saturating_add(1), line.framing.body_len));
        }
        records.push(line.record);
    }
    Ok(records)
}

/// `0000-DALYTRAN-OPEN`, the file opened for reading, then every `READ` of
/// `1000-DALYTRAN-GET-NEXT` at once, the file read in place to its end (CBTRN02C-033). Runs
/// inside the blocking step of [`open_dalytran`].
///
/// # Errors
///
/// [`DalytranFailure::Unopenable`] when the file cannot be opened for reading (CBTRN02C-031),
/// and [`DalytranFailure::Unreadable`] when it opens but cannot be read, as a directory
/// (CBTRN02C-038), each a [`JobError::Io`] with context `reading input` and the error's kind.
fn read_dalytran(path: &Path) -> Result<Vec<u8>, DalytranFailure> {
    let mut file = File::open(path)
        .map_err(io(READING_INPUT))
        .map_err(DalytranFailure::Unopenable)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(io(READING_INPUT))
        .map_err(DalytranFailure::Unreadable)?;
    Ok(bytes)
}

/// Why the blocking step of [`open_dalytran`] produced no lines, returned unreported so that
/// its events are emitted on the job's task.
enum DalytranFailure {
    /// The file could not be opened for reading: the failed open (CBTRN02C-031).
    Unopenable(JobError),
    /// The file opened but could not be read: the failed read (CBTRN02C-038).
    Unreadable(JobError),
    /// Codec A refused a line (CBTRN02C-032).
    Refused(LoadError),
}

/// `0100-TRANFILE-OPEN`: the `OPEN OUTPUT` of the transaction cluster, which admits one writer
/// (CBTRN02C-046) and succeeds only when the cluster holds no record (CBTRN02C-030). Opens the
/// run's session, claims `transactions` on it, checks the table on it and returns it.
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR OPENING TRANSACTION FILE` when another run holds the claim
/// (status `93`, CBTRN02C-046); when the session cannot be opened, the claim fails or
/// `transactions` cannot be read, the database being unreachable or its schema not applied
/// (CBTRN02C-031); and when it holds a row (X-POSTTRAN-H1). The session, when it was opened, is
/// dropped as the error returns, which gives up the claim it holds.
async fn open_tranfile(store: &PgStore) -> Result<PgSession, JobError> {
    let mut session = store
        .session()
        .await
        .map_err(|error| shown_failure(OPEN_TRANFILE_OP, OPEN_TRANFILE, &error))?;
    open_for_writing(&mut session, &TRANFILE).await?;
    match TransactionRepository::find_last(&mut session).await {
        Ok(None) => Ok(session),
        // The row read is dropped unformatted: it holds a card number (CBTRN02C-037).
        Ok(Some(_)) => Err(abend(CLUSTER_NOT_EMPTY)),
        Err(error) => Err(shown_failure(OPEN_TRANFILE_OP, OPEN_TRANFILE, &error)),
    }
}

/// The claim of the open `open` stands for: its table claimed on the run's session for the
/// run's writes, before the table is read (CBTRN02C-046).
///
/// # Errors
///
/// [`JobError::Abend`] with the open's failure line and status `93` when another run holds the
/// claim (CBTRN02C-046), and with that line and the stand-in status when the claim fails as a
/// database call, a lost session included (CBTRN02C-031, CBTRN02C-042). No table has been read
/// then.
async fn open_for_writing(session: &mut PgSession, open: &WriterOpen) -> Result<(), JobError> {
    match session.claim_writer(open.dataset).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(abend(open.refused())),
        Err(error) => Err(shown_failure(open.operation, open.line, &error)),
    }
}

/// `0300-DALYREJS-OPEN`: the `OPEN OUTPUT` of `DALYREJS(+1)`, the next generation in `dir`,
/// `Config::dalyrejs_dir`, created empty before the first record is read (CBTRN02C-028).
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR OPENING DALY REJECTS FILE` and the stand-in status when the
/// generation cannot be allocated or created (CBTRN02C-031). The event before those lines names
/// the step, the setting of the directory, `dalyrejs under CARDDEMO_VAR_DIR`, and the error kind,
/// never the path, so the operator learns which setting to correct.
async fn open_dalyrejs(dir: &Path) -> Result<OutputFile, JobError> {
    OutputFile::next_generation(dir, REJECTS_BASE)
        .await
        .map_err(|error| {
            let error = error.naming_output_dir(DALYREJS_DIR_SETTING);
            file_failure(OPEN_DALYREJS_OP, OPEN_DALYREJS, &error)
        })
}

/// `9100-TRANFILE-CLOSE`, `9400-ACCTFILE-CLOSE` and `9500-TCATBALF-CLOSE` for the claims: the
/// run's session is closed once the run's job log is complete, which gives its three claims up
/// at once (CBTRN02C-046).
///
/// A close that fails neither changes the run's outcome nor leaves a claim held: every write of
/// the run has completed, and the session's connection is closed whether or not the unlock was
/// answered, which ends the locks on the server. The failure is reported in one warning event
/// naming the closes and the error, and the run keeps its outcome.
async fn close_claimed_files(session: PgSession) {
    if let Err(error) = session.close().await {
        tracing::warn!(
            operation = CLOSE_CLAIMED_FILES_OP,
            error = %error,
            "CBTRN02C writer claim release failed; the closed session ends the claim"
        );
    }
}

/// `2000-POST-TRANSACTION`'s three writes, in its order: the category balance, the account, then
/// the transaction, each committed on the run's session as it completes (CBTRN02C-015).
///
/// # Errors
///
/// [`JobError::Abend`] when a write fails, as every write does once the session is lost; the
/// writes before it stand (CBTRN02C-025, CBTRN02C-038, CBTRN02C-039).
async fn apply_posting(session: &mut PgSession, posting: &Posting) -> Result<(), JobError> {
    write_category_balance(session, &posting.tcatbal).await?;
    rewrite_account(session, &posting.account).await?;
    write_transaction(session, &posting.transaction).await
}

/// `2700-UPDATE-TCATBAL`: a created balance is announced by its job-log line, then written; an
/// updated one is rewritten (CBTRN02C-016, CBTRN02C-017, CBTRN02C-019).
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR WRITING TRANSACTION BALANCE FILE` when the insert fails, and
/// with `ERROR REWRITING TRANSACTION BALANCE FILE` when the update fails or matches no row, the
/// `REWRITE` that ends with status `23` (CBTRN02C-038).
async fn write_category_balance(
    session: &mut PgSession,
    write: &TcatbalWrite,
) -> Result<(), JobError> {
    match write {
        TcatbalWrite::Create(record) => {
            if let Some(line) = write.sysout_line() {
                tracing::info!("{line}");
            }
            TranCatBalanceRepository::insert(session, record)
                .await
                .map_err(|error| shown_failure(WRITE_TCATBALF_OP, WRITE_TCATBAL, &error))
        }
        TcatbalWrite::Update(record) => {
            match TranCatBalanceRepository::update(session, record).await {
                Ok(true) => Ok(()),
                Ok(false) => {
                    tracing::error!(
                        operation = REWRITE_TCATBALF_OP,
                        "CBTRN02C rewrite found no row"
                    );
                    Err(abend(Failure::shown(REWRITE_TCATBAL, Status::NotFound)))
                }
                Err(error) => Err(shown_failure(REWRITE_TCATBALF_OP, REWRITE_TCATBAL, &error)),
            }
        }
    }
}

/// `2800-UPDATE-ACCOUNT-REC`: the account rewritten with its new balance and cycle fields
/// (CBTRN02C-020, CBTRN02C-021).
///
/// An update that matches no row is the rewrite's `INVALID KEY`, which moves reason `0109` after
/// the reject decision has been taken: the record stays posted and uncounted as a reject, and its
/// transaction is still written (CBTRN02C-023, X-POSTTRAN-3).
///
/// # Errors
///
/// [`JobError::Abend`] when the update fails, which the COBOL has no status test for: an event
/// carrying the error, then `ABENDING PROGRAM` (CBTRN02C-039).
async fn rewrite_account(session: &mut PgSession, account: &AccountRecord) -> Result<(), JobError> {
    match AccountRepository::update(session, account).await {
        Ok(true | false) => Ok(()),
        Err(error) => Err(unshown_failure(
            REWRITE_ACCTFILE_OP,
            ACCOUNT_REWRITE_FAILED,
            &error,
        )),
    }
}

/// `2900-WRITE-TRANSACTION-FILE`: the transaction written under its `TRAN-ID` (CBTRN02C-024).
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR WRITING TO TRANSACTION FILE` and status `22` when a row
/// already has the ID, as when an earlier record of the run had the same `DALYTRAN-ID`, or the
/// stand-in status for any other failure (CBTRN02C-025).
async fn write_transaction(
    session: &mut PgSession,
    transaction: &TransactionRecord,
) -> Result<(), JobError> {
    TransactionRepository::insert(session, transaction)
        .await
        .map_err(|error| shown_failure(WRITE_TRANFILE_OP, WRITE_TRANFILE, &error))
}

/// `2500-WRITE-REJECT-REC`: the 430-byte reject record and its LF (CBTRN02C-027, CBTRN02C-028).
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR WRITING TO REJECTS FILE` when the bytes cannot be written
/// (CBTRN02C-038).
async fn write_reject(rejects: &mut OutputFile, reject: &RejectRecord) -> Result<(), JobError> {
    rejects
        .write_record(&reject.encode())
        .await
        .map_err(|error| file_failure(WRITE_DALYREJS_OP, WRITE_DALYREJS, &error))
}

/// `9300-DALYREJS-CLOSE`: the generation flushed, synced and kept; its path (CBTRN02C-028).
///
/// # Errors
///
/// [`JobError::Abend`] with `ERROR CLOSING DAILY REJECTS FILE` and `FILE STATUS IS: NNNN0000`
/// when the file cannot be flushed or synced; the file is removed then (CBTRN02C-040,
/// CBTRN02C-044).
async fn close_dalyrejs(rejects: OutputFile) -> Result<PathBuf, JobError> {
    rejects.finish().await.map_err(|error| {
        report_file(CLOSE_DALYREJS_OP, &error);
        abend(Failure::shown(CLOSE_DALYREJS, Status::CrossReferenceClosed))
    })
}

/// A `DALYTRAN` line codec A refuses: one event naming the file, the line and the rule it breaks,
/// a field, an offset and a width, never a byte of the line, then the abend (CBTRN02C-032).
fn refused(error: &LoadError) -> JobError {
    tracing::error!(
        file = DALYTRAN,
        line = error.line(),
        rule = %error.kind(),
        "DALYTRAN line refused by codec A"
    );
    abend(Failure::unshown(LINE_REFUSED))
}

/// A `DALYTRAN` line whose body is not the 350-byte record: one event naming the file, the line
/// and the body's length against the record's, then the abend (CBTRN02C-045).
fn not_a_record(line: usize, body_len: usize) -> JobError {
    tracing::error!(
        file = DALYTRAN,
        line,
        body_len,
        record_len = DailyTransactionRecord::WIDTH,
        "DALYTRAN line is not one 350-byte record"
    );
    abend(Failure::unshown(LINE_NOT_A_RECORD))
}

/// The failure of repository call `operation` on the path that displays `line`: the event
/// carrying the error, then the path's lines and the abend.
fn shown_failure(
    operation: &'static str,
    line: &'static str,
    error: &PersistenceError,
) -> JobError {
    report(operation, error);
    abend(Failure::shown(line, Status::of(error)))
}

/// The failure of repository call `operation` on a path with no line of its own: the event
/// carrying the error, then the abend with `reason` (CBTRN02C-039).
fn unshown_failure(
    operation: &'static str,
    reason: &'static str,
    error: &PersistenceError,
) -> JobError {
    report(operation, error);
    abend(Failure::unshown(reason))
}

/// The failure of local-file call `operation` on the path that displays `line`, with the
/// stand-in status: the event carrying the error, then the path's lines and the abend.
fn file_failure(operation: &'static str, line: &'static str, error: &JobError) -> JobError {
    report_file(operation, error);
    abend(Failure::shown(line, Status::Permanent))
}

/// Reports why a repository call failed, for the operator: the file operation it replaces and
/// the error's text, which names an SQLSTATE, a constraint or a column only (Cardholder Data
/// Redaction).
fn report(operation: &'static str, error: &PersistenceError) {
    tracing::error!(operation, error = %error, "CBTRN02C database call failed");
}

/// Reports why a local-file call failed: the file operation and the error's text, a static step
/// and the I/O error kind, with the setting of the reject directory for a step on it, or the
/// generation limit and the `DALYREJS` base, never the path.
fn report_file(operation: &'static str, error: &JobError) {
    tracing::error!(operation, error = %error, "CBTRN02C file operation failed");
}

/// `9999-ABEND-PROGRAM` after `failure`: its failure and status lines, then `ABENDING PROGRAM`
/// and the abend with exit status 12 (CBTRN02C-041, CBTRN02C-043). The caller returns the error
/// at once, which drops the unfinished reject generation and so removes it (CBTRN02C-044).
fn abend(failure: Failure) -> JobError {
    if let Some((line, status)) = failure.shown {
        tracing::error!("{line}");
        tracing::error!("{}", status.line());
    }
    tracing::error!("{ABEND_LINE}");
    JobError::Abend {
        code: Some(ABEND_EXIT_STATUS),
        reason: failure.reason,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::NaiveDate;
    use copybook::FixedRecord;
    use domain::batch::cbtrn02c::{RejectReason, START_LINE, reject_record};
    use domain::clock::FixedClock;
    use domain::records::{AccountRecord, DailyTransactionRecord, TranCatBalanceRecord};
    use persistence::{
        AccountRepository, PersistenceError, PgStore, TranCatBalanceRepository,
        TransactionRepository,
    };
    use runtime::config::Config;
    use test_support::fixtures::start_loaded_postgres;
    use test_support::trace_capture::{self, TraceCapture};

    use super::{
        ABEND_LINE, ACCTFILE, CLUSTER_NOT_EMPTY, Failure, JobContext, JobError, LINE_REFUSED,
        OPEN_DALYREJS, OPEN_DALYTRAN, OutputFile, REJECTS_BASE, Status, TCATBALF, TRANFILE,
        WRITE_TRANFILE, XREF_READ_FAILED, abend, close_dalyrejs, open_dalyrejs, open_dalytran, run,
        write_reject,
    };

    type TestResult = Result<(), Box<dyn Error>>;

    /// What a run changes in the database: every account and category balance, and the number
    /// of transactions (the rows themselves hold card numbers and are not kept).
    type Tables = (Vec<AccountRecord>, Vec<TranCatBalanceRecord>, usize);

    /// The status line of status `93` (CBTRN02C-041, CBTRN02C-046).
    const IN_USE_STATUS: &str = "FILE STATUS IS: NNNN9243";

    /// The transactions a run over the fixtures posts (FIXPOST-901).
    const POSTED: usize = 262;

    /// Tells apart the scratch directories of the tests of one process.
    static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

    /// A directory of its own under the system temporary directory, removed with its content
    /// when dropped. Only scratch inputs and outputs go there; no fixture is copied into it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Result<Self, Box<dyn Error>> {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "carddemo-batch-posttran-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Writes `bytes` to the file `name` of the directory and returns its path.
        fn file(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, Box<dyn Error>> {
            let path = self.0.join(name);
            fs::write(&path, bytes)?;
            Ok(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// `app/data/ASCII/dailytran.txt`, read in place.
    fn dailytran() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/dailytran.txt")
    }

    /// Line `n` (1-based) of the daily fixture, without its LF; every line is 350 bytes
    /// (FIXPOST-901).
    fn fixture_line(n: usize) -> Result<Vec<u8>, Box<dyn Error>> {
        let bytes = fs::read(dailytran())?;
        let line = bytes
            .split(|byte| *byte == b'\n')
            .nth(n.saturating_sub(1))
            .ok_or("the fixture has fewer lines")?;
        Ok(line.to_vec())
    }

    /// The card number of a daily record's line, bytes 263-278 (`DALYTRAN-CARD-NUM`), as text.
    fn card_number(line: &[u8]) -> Result<String, Box<dyn Error>> {
        let card = line
            .get(262..278)
            .ok_or("the line is shorter than the record")?;
        Ok(String::from_utf8(card.to_vec())?)
    }

    /// The messages of the captured events, in order.
    fn messages(capture: &TraceCapture) -> Vec<String> {
        message_texts(&capture.lines())
    }

    /// The messages of the job's own events, in order, leaving out the statement events `sqlx`
    /// writes under its own target.
    fn job_messages(capture: &TraceCapture) -> Vec<String> {
        let job = module_path!()
            .strip_suffix("::tests")
            .unwrap_or(module_path!());
        message_texts(&capture.lines_with_target(job))
    }

    /// The `message` member of each line that has one, in order.
    fn message_texts(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|line| {
                let (_, rest) = line.split_once(r#""message":""#)?;
                let (message, _) = rest.split_once('"')?;
                Some(message.to_owned())
            })
            .collect()
    }

    /// The configuration of a run whose outputs go under `var_dir`, every other variable unset.
    fn var_config(var_dir: &Path) -> Result<Config, Box<dyn Error>> {
        let var_dir = var_dir.display().to_string();
        Ok(Config::from_lookup(|name| {
            (name == "CARDDEMO_VAR_DIR").then(|| var_dir.clone())
        })?)
    }

    /// The instant `fixtures.md` posts at, `2022-07-01 12:00:00.000000`.
    fn fixture_clock() -> Result<FixedClock, Box<dyn Error>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 1)
            .and_then(|day| day.and_hms_opt(12, 0, 0))
            .ok_or("2022-07-01 12:00:00 is a valid instant")?;
        Ok(FixedClock::new(at))
    }

    /// The tables a run writes, read back through `store`.
    async fn tables(store: &PgStore) -> Result<Tables, Box<dyn Error>> {
        let mut store = store.clone();
        let accounts = AccountRepository::find_all(&mut store).await?;
        let balances = TranCatBalanceRepository::find_all(&mut store).await?;
        let transactions = TransactionRepository::find_all_in_card_order(&mut store)
            .await?
            .len();
        Ok((accounts, balances, transactions))
    }

    /// The names of the `DALYREJS` generations in `dir`; none when `dir` does not exist.
    fn generations(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("DALYREJS.") {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// The exit code and the reason of an abend, `None` for any other error.
    fn abend_of(error: &JobError) -> Option<(Option<u8>, &'static str)> {
        match error {
            JobError::Abend { code, reason } => Some((*code, *reason)),
            _ => None,
        }
    }

    // audit: CBTRN02C-025
    // audit: CBTRN02C-040
    // audit: CBTRN02C-041
    // audit: CBTRN02C-046
    #[test]
    fn status_lines_are_those_9910_displays() {
        let cases = [
            (Status::CrossReferenceClosed, "FILE STATUS IS: NNNN0000"),
            (Status::DuplicateKey, "FILE STATUS IS: NNNN0022"),
            (Status::NotFound, "FILE STATUS IS: NNNN0023"),
            (Status::Permanent, "FILE STATUS IS: NNNN0030"),
            (Status::ClusterNotEmpty, "FILE STATUS IS: NNNN0037"),
            (Status::ResourceUnavailable, IN_USE_STATUS),
        ];
        for (status, line) in cases {
            assert_eq!(status.line(), line, "{status:?}");
        }
        let duplicate = PersistenceError::Duplicate {
            constraint: Some("transactions_pkey".to_owned()),
        };
        assert_eq!(Status::of(&duplicate), Status::DuplicateKey);
        assert_eq!(
            Status::of(&PersistenceError::Unavailable),
            Status::Permanent
        );
    }

    // audit: CBTRN02C-030
    // audit: CBTRN02C-039
    // audit: CBTRN02C-043
    #[test]
    fn every_failure_path_ends_with_abending_program_and_status_12() {
        let cases: [(Failure, &[&str], &str); 3] = [
            (
                Failure::shown(WRITE_TRANFILE, Status::DuplicateKey),
                &[
                    "ERROR WRITING TO TRANSACTION FILE",
                    "FILE STATUS IS: NNNN0022",
                    ABEND_LINE,
                ],
                "ABEND: ERROR WRITING TO TRANSACTION FILE (return code 12)",
            ),
            (
                Failure::unshown(XREF_READ_FAILED),
                &[ABEND_LINE],
                "ABEND: the card cross-reference could not be read (return code 12)",
            ),
            (
                CLUSTER_NOT_EMPTY,
                &[
                    "ERROR OPENING TRANSACTION FILE",
                    "FILE STATUS IS: NNNN0037",
                    ABEND_LINE,
                ],
                "ABEND: ERROR OPENING TRANSACTION FILE: transactions already holds a row \
                 (BLOCKED, X-POSTTRAN-H1) (return code 12)",
            ),
        ];
        for (failure, lines, text) in cases {
            let capture = trace_capture::capture();
            let error = abend(failure);
            assert_eq!(messages(&capture), *lines, "{text}");
            assert_eq!(error.exit_code(), Some(12), "{text}");
            assert_eq!(error.to_string(), text);
        }
    }

    // audit: CBTRN02C-001
    // audit: CBTRN02C-033
    #[tokio::test]
    async fn the_daily_fixture_decodes_whole_in_input_order() -> TestResult {
        let before = fs::read(dailytran())?;
        let records = open_dalytran(&dailytran()).await?;
        assert_eq!(records.len(), 300);
        for (n, record) in [(1, records.first()), (300, records.last())] {
            let record = record.ok_or("a record is missing")?;
            assert!(record.encode() == fixture_line(n)?, "line {n}");
        }
        assert!(fs::read(dailytran())? == before, "the input was changed");
        Ok(())
    }

    // audit: CBTRN02C-029
    #[tokio::test]
    async fn an_empty_input_holds_no_record() -> TestResult {
        let scratch = Scratch::new()?;
        let empty = scratch.file("empty.txt", b"")?;
        assert!(open_dalytran(&empty).await?.is_empty());
        Ok(())
    }

    /// A line codec A refuses ends the job naming the file, the line and the rule only, never a
    /// byte of the input, with no reject generation created (CBTRN02C-032, CBTRN02C-037).
    // audit: CBTRN02C-032
    // audit: CBTRN02C-037
    #[tokio::test]
    async fn a_refused_line_ends_the_job_naming_its_number_only() -> TestResult {
        let scratch = Scratch::new()?;
        let first = fixture_line(1)?;
        let card = card_number(&first)?;
        let mut long = first.clone();
        long.push(b'X');
        for (name, bad) in [
            ("short.txt", b"QQQ-not-a-record".to_vec()),
            ("long.txt", long),
        ] {
            let mut input = first.clone();
            input.push(b'\n');
            input.extend_from_slice(&bad);
            input.push(b'\n');
            let path = scratch.file(name, &input)?;
            let capture = trace_capture::capture();
            let error = open_dalytran(&path)
                .await
                .err()
                .ok_or("a refused line was decoded")?;
            assert_eq!(abend_of(&error), Some((Some(12), LINE_REFUSED)), "{name}");
            assert_eq!(
                messages(&capture),
                ["DALYTRAN line refused by codec A", ABEND_LINE],
                "{name}"
            );
            assert!(capture.contains(r#""line":2"#), "{name}");
            assert!(capture.contains(r#""file":"DALYTRAN""#), "{name}");
            assert!(!capture.contains("QQQ"), "{name} shows the line's bytes");
            assert!(!capture.contains(&card), "{name} shows a card number");
            assert!(!capture.contains(name), "{name} shows the path");
        }
        Ok(())
    }

    // audit: CBTRN02C-031
    #[tokio::test]
    async fn an_unreadable_input_is_the_failed_dalytran_open() -> TestResult {
        let scratch = Scratch::new()?;
        let missing = scratch.path().join("missing-dailytran.txt");
        let capture = trace_capture::capture();
        let error = open_dalytran(&missing)
            .await
            .err()
            .ok_or("a missing input was read")?;
        assert_eq!(abend_of(&error), Some((Some(12), OPEN_DALYTRAN)));
        assert_eq!(
            messages(&capture),
            [
                "CBTRN02C file operation failed",
                OPEN_DALYTRAN,
                "FILE STATUS IS: NNNN0030",
                ABEND_LINE,
            ]
        );
        assert!(!capture.contains("missing-dailytran"));
        Ok(())
    }

    /// A file in place of the reject directory, or of `CARDDEMO_VAR_DIR` above it, is the failed
    /// `DALYREJS` open: its event names the step, the setting of the directory and the error
    /// kind, never the path, and its lines, the abend and exit status 12 are those of every
    /// failed open. The file is left as it was (CBTRN02C-031).
    // audit: CBTRN02C-031
    #[tokio::test]
    async fn a_file_in_place_of_the_reject_directory_names_its_setting() -> TestResult {
        let scratch = Scratch::new()?;
        let var_file = scratch.file("var", b"sentinel\n")?;
        let var_dir = scratch.path().join("var-dir");
        fs::create_dir_all(&var_dir)?;
        let rejects_file = var_dir.join("dalyrejs");
        fs::write(&rejects_file, b"sentinel\n")?;
        let scratch_name = scratch
            .path()
            .file_name()
            .ok_or("the scratch directory has no name")?
            .to_string_lossy()
            .into_owned();
        for dir in [var_file.join("dalyrejs"), rejects_file.clone()] {
            let shown = dir.display().to_string();
            let capture = trace_capture::capture();
            let error = open_dalyrejs(&dir)
                .await
                .err()
                .ok_or("a reject generation was created")?;
            assert_eq!(abend_of(&error), Some((Some(12), OPEN_DALYREJS)), "{shown}");
            assert_eq!(
                error.to_string(),
                "ABEND: ERROR OPENING DALY REJECTS FILE (return code 12)"
            );
            assert_eq!(
                messages(&capture),
                [
                    "CBTRN02C file operation failed",
                    OPEN_DALYREJS,
                    "FILE STATUS IS: NNNN0030",
                    ABEND_LINE,
                ],
                "{shown}"
            );
            assert!(
                capture.contains(r#""operation":"OPEN DALYREJS""#),
                "{shown}"
            );
            assert!(
                capture.contains(
                    r#""error":"allocating generation (dalyrejs under CARDDEMO_VAR_DIR): not a directory""#
                ),
                "{shown}"
            );
            assert!(!capture.contains(&scratch_name), "{shown} is shown");
        }
        for file in [var_file, rejects_file] {
            assert_eq!(fs::read(&file)?, b"sentinel\n", "{}", file.display());
        }
        Ok(())
    }

    /// The reject generation holds each reject record as its 350 input bytes, the four-digit
    /// reason and the 76-byte description, then LF, and stays once closed (CBTRN02C-027,
    /// CBTRN02C-028).
    // audit: CBTRN02C-027
    // audit: CBTRN02C-028
    #[tokio::test]
    async fn reject_records_are_430_bytes_and_lf_in_the_next_generation() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("dalyrejs");
        let lines = [fixture_line(16)?, fixture_line(32)?];
        let mut rejects = OutputFile::next_generation(&dir, REJECTS_BASE).await?;
        let mut expected = Vec::new();
        for line in &lines {
            let daily = DailyTransactionRecord::decode(line)?;
            write_reject(
                &mut rejects,
                &reject_record(&daily, RejectReason::Overlimit),
            )
            .await?;
            expected.extend_from_slice(line);
            expected.extend_from_slice(b"0102");
            expected.extend_from_slice(format!("{:<76}", "OVERLIMIT TRANSACTION").as_bytes());
            expected.push(b'\n');
        }
        let path = close_dalyrejs(rejects).await?;
        assert_eq!(path, dir.join("DALYREJS.G0001"));
        let written = fs::read(&path)?;
        assert_eq!(written.len(), 2 * 431);
        assert!(written == expected, "the reject records differ");
        let empty = close_dalyrejs(OutputFile::next_generation(&dir, REJECTS_BASE).await?).await?;
        assert_eq!(empty, dir.join("DALYREJS.G0002"));
        assert!(fs::read(&empty)?.is_empty());
        Ok(())
    }

    /// Each open for output or update meets the claim another run holds on its table: the run
    /// shows the open's line, the status line of `93` and `ABENDING PROGRAM`, ends with exit
    /// status 12, changes no table and keeps no reject generation, the one created before the
    /// account open included. Once the claim is given up, the run posts the fixtures and gives
    /// its own claims up at its end (FIXPOST-903).
    // audit: CBTRN02C-031
    // audit: CBTRN02C-044
    // audit: CBTRN02C-046
    #[tokio::test]
    async fn an_open_another_run_holds_ends_the_job_before_any_effect() -> TestResult {
        let guard = start_loaded_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new()?;
        let config = var_config(scratch.path())?;
        let clock = fixture_clock()?;
        let ctx = JobContext {
            store: &store,
            config: &config,
            clock: &clock,
        };
        let before = tables(&store).await?;
        assert_eq!(before.2, 0, "the loaded database holds no transaction");

        for open in [&TRANFILE, &ACCTFILE, &TCATBALF] {
            let mut held = store.session().await?;
            assert!(
                held.claim_writer(open.dataset).await?,
                "the test claims a free {:?}",
                open.dataset
            );
            let capture = trace_capture::capture();
            let error = run(&ctx, &dailytran())
                .await
                .err()
                .ok_or("a run passed an open another run holds")?;
            assert_eq!(
                abend_of(&error),
                Some((Some(12), open.in_use)),
                "{}",
                open.line
            );
            assert_eq!(
                job_messages(&capture),
                [START_LINE, open.line, IN_USE_STATUS, ABEND_LINE],
                "{}",
                open.line
            );
            drop(capture);
            assert!(
                tables(&store).await? == before,
                "{} changed a table",
                open.line
            );
            assert!(
                generations(&config.dalyrejs_dir())?.is_empty(),
                "{}",
                open.line
            );
            held.close().await?;
        }

        let outcome = run(&ctx, &dailytran()).await?;
        assert_eq!(outcome.return_code, 4);
        assert_eq!(
            outcome.written,
            [config.dalyrejs_dir().join("DALYREJS.G0001")]
        );
        assert_eq!(tables(&store).await?.2, POSTED);
        let mut free = store.session().await?;
        for open in [&TRANFILE, &ACCTFILE, &TCATBALF] {
            assert!(
                free.claim_writer(open.dataset).await?,
                "a finished run still holds {:?}",
                open.dataset
            );
        }
        free.close().await?;
        Ok(())
    }

    /// Two runs started together over the same database: one claims the transaction cluster
    /// and posts the fixtures, the other meets that claim at its first open and ends before it
    /// reads a table, so the tables hold one run's postings and one reject generation exists.
    // audit: CBTRN02C-046
    #[tokio::test]
    async fn of_two_runs_at_once_one_posts_and_the_other_is_refused() -> TestResult {
        let guard = start_loaded_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let scratch = Scratch::new()?;
        let config = var_config(scratch.path())?;
        let clock = fixture_clock()?;
        let ctx = JobContext {
            store: &store,
            config: &config,
            clock: &clock,
        };
        let input = dailytran();

        let (first, second) = tokio::join!(run(&ctx, &input), run(&ctx, &input));
        let (posted, refused) = match (first, second) {
            (Ok(posted), Err(refused)) | (Err(refused), Ok(posted)) => (posted, refused),
            (first, second) => {
                return Err(format!(
                    "one run must post and one be refused: {:?}, {:?}",
                    first.map(|outcome| outcome.return_code),
                    second.map(|outcome| outcome.return_code)
                )
                .into());
            }
        };
        assert_eq!(posted.return_code, 4);
        assert_eq!(
            posted.written,
            [config.dalyrejs_dir().join("DALYREJS.G0001")]
        );
        assert_eq!(abend_of(&refused), Some((Some(12), TRANFILE.in_use)));
        assert_eq!(tables(&store).await?.2, POSTED);
        assert_eq!(generations(&config.dalyrejs_dir())?, ["DALYREJS.G0001"]);
        Ok(())
    }
}
