//! `intcalc`: batch job INTCALC, whose `STEP15` runs the monthly interest calculator `CBACT04C`
//! with `PARM='2022071800'` (`INTCALC.jcl:22`; AAP 0.3.3.2, 0.3.4.3). Every rule is
//! `domain::batch::cbact04c`'s; this module reads the four tables the program opens, drives
//! `Intcalc` over the category balances in key order and applies the effects it returns, after
//! the `CBACT04C-` rows of `modern/audit/batch/INTCALC.md` and, over the fixture corpus, the
//! `FIXINT-` rows of `modern/audit/fixtures.md`.
//!
//! # Flow
//!
//! 1. `--parm-date` arrives as the domain's typed `ParmDate`: the command line has already
//!    refused any value that is not 10 or more printable ASCII bytes, so a refused PARM shows no
//!    line, reads no table and creates no file (CBACT04C-001).
//! 2. `START OF EXECUTION OF PROGRAM CBACT04C`, then the opens (CBACT04C-002). The run first
//!    opens its session, the `PgSession` that holds its writer claim and runs every statement
//!    of the run, and claims `accounts` for writing on it, the `OPEN I-O` of
//!    `0300-ACCTFILE-OPEN` (CBACT04C-039): the opens before it are for input
//!    and claim nothing, so the claim comes before the first read. `accounts`, `card_xref` and
//!    `disclosure_groups` are then read whole into `Intcalc::new`, which serves the program's
//!    keyed reads from them, the cross-references grouped by account as the alternate index over
//!    `XREF-ACCT-ID` holds them (CBACT04C-004 to CBACT04C-011). The next `SYSTRAN` generation is
//!    then created under
//!    `Config::systran_dir`, before any category balance is read, as `0400-TRANFILE-OPEN`
//!    creates `SYSTRAN(+1)` (CBACT04C-024).
//! 3. Every `tran_cat_balances` row, in the KSDS key order its repository returns, is shown as
//!    its record line (CBACT04C-003), then goes to `Intcalc::process`, and its effects are
//!    applied in the order returned: an account update rewrites that `accounts` row
//!    (CBACT04C-026), a `DEFAULT` fallback shows its two lines (CBACT04C-010), and a system
//!    transaction appends its 350 bytes and LF to the generation file (CBACT04C-024). The
//!    interest, the running total and the balance store are the domain's (`monthly_interest`
//!    and `store`, ARITH-001 to ARITH-017); this module does no arithmetic on money.
//! 4. At the end of the file `Intcalc::finish` runs and only its counters are kept. Its
//!    unapplied total is deliberately never persisted: the loop's
//!    `ELSE PERFORM 1050-UPDATE-ACCOUNT` is never reached, so the last account in key order
//!    keeps its balance and cycle fields while its SYSTRAN records are still written (E08;
//!    CBACT04C-027, ARITH-017, FIXINT-903).
//! 5. The generation file is flushed, synced and kept. Its path is the one path returned, and
//!    the run returns code 0, as `RETURN-CODE` is never set (CBACT04C-030). The run's session is
//!    then closed, which gives up the claim on `accounts`, as `9300-ACCTFILE-CLOSE` ends the
//!    program's hold on the cluster (CBACT04C-039), and `END OF EXECUTION OF PROGRAM CBACT04C`
//!    is shown (CBACT04C-030).
//!
//! # One writer of the account file
//!
//! `ACCTFILE` is defined `SHAREOPTIONS(2 3)` (`ACCTFILE.jcl:42`) and allocated `DISP=SHR`
//! (`INTCALC.jcl:33`), so only VSAM keeps two writers apart: it fails the `OPEN I-O` of a second
//! one with status `93`. The job keeps that rule through `PgSession::claim_writer`. A claim
//! another run holds, a second `intcalc` or a `posttran` holding `accounts`, ends the job with
//! [`JobError::Abend`] and `ERROR OPENING ACCOUNT MASTER FILE` before any table is read and
//! before the generation is created (CBACT04C-034, CBACT04C-039). Without it two runs would each
//! rewrite every balance from their own start-of-run read, and one run's interest would be lost.
//!
//! The claim fences the run's rewrites because every statement of the run, each read and each
//! rewrite, runs on the session that holds it, a connection that is never replaced. A session
//! the server ends, through `pg_terminate_backend`, `idle_session_timeout` or a lost network,
//! takes the claim with it, and the run's next statement fails on it, a read or a rewrite: the
//! run ends there with the abend of that read or rewrite (CBACT04C-032, CBACT04C-033) and
//! rewrites nothing after its claim is gone, so a second run that claims the file then is the
//! only writer. The claim is held to the run's
//! end and given up by closing the session on success; on every error the session drops with
//! the run, which closes it and ends the lock on the server. A close that fails is reported in a
//! warning event and changes nothing else: every rewrite is done, and the session's connection
//! is closed either way, which ends its lock.
//!
//! # Every rewrite commits when it is made
//!
//! A VSAM `REWRITE` has no unit of work to back out, so each account update is its own
//! autocommitted statement on the run's session, never part of a job transaction, and the
//! updates made before an abend stand (CBACT04C-026, CBACT04C-038). When the first record of a
//! new account abends a lookup, the previous account's `1050-UPDATE-ACCOUNT` has already run, as
//! it comes before `1100-GET-ACCT-DATA`; the domain returns that update with its error, and the
//! job persists it before it ends (CBACT04C-005, CBACT04C-007, CBACT04C-008, CBACT04C-011).
//! Should that rewrite fail, its failure is the abend, as it is in the program.
//!
//! # Abends
//!
//! Every failure path ends in `9999-ABEND-PROGRAM`, after the lines the run showed before the
//! failure, its `DISPLAY` lines emitted in program order as `ERROR` events whose message is the
//! line (X-INTCALC-4): first, when the failure has an underlying error, one event naming the
//! file operation the call replaces and carrying the error, an SQLSTATE, a constraint or column
//! name, or a static step and an I/O error kind; then the path's failure line; the status line
//! of `9910-DISPLAY-IO-STATUS` (`file_status_line`); and `ABENDING PROGRAM`, whose field
//! `abend_code` records the abend code 999 `CEE3ABD` receives (CBACT04C-038). The job then
//! returns [`JobError::Abend`] with the failure line as its reason:
//!
//! - `ERROR OPENING TRANSACTION CATEGORY BALANCE`: the one `PostgreSQL` connection that stands
//!   for the input opens fails (CBACT04C-034, X-INTCALC-7): the run's session cannot be opened,
//!   or `main` cannot connect at all and the job never starts. In that case `main` shows
//!   `START OF EXECUTION OF PROGRAM CBACT04C` through `show_start` (CBACT04C-002), emits its
//!   connection event, then these lines through `show_failed_connection`, and reports its own
//!   connection error, exit status 1, in place of the [`JobError::Abend`].
//! - `ERROR OPENING ACCOUNT MASTER FILE`: another run holds the claim on `accounts`, status `93`
//!   (CBACT04C-039), or the claim fails as a database call.
//! - `ERROR READING ACCOUNT FILE`, `ERROR READING XREF FILE`, `ERROR READING DISCLOSURE GROUP
//!   FILE` or `ERROR READING TRANSACTION CATEGORY FILE`: the read of `accounts`, `card_xref`,
//!   `disclosure_groups` or `tran_cat_balances` fails (CBACT04C-033), as every read does once the
//!   session is lost (CBACT04C-039) and against a database whose schema has not been applied.
//! - `ERROR OPENING TRANSACTION FILE`: the generation cannot be allocated or created, generation
//!   9999 of `SYSTRAN` existing included, which is left as it is (CBACT04C-034). The event names
//!   the setting of the directory, `systran under CARDDEMO_VAR_DIR`, never its path.
//! - `ERROR RE-WRITING ACCOUNT FILE`: a rewrite finds no row, status `23`, or fails as a
//!   statement (CBACT04C-032).
//! - `ERROR WRITING TRANSACTION RECORD`: a SYSTRAN record cannot be written (CBACT04C-031).
//! - `ERROR CLOSING TRANSACTION FILE`: the generation cannot be flushed or synced; no
//!   `END OF EXECUTION OF PROGRAM CBACT04C` line follows (CBACT04C-035).
//!
//! Status `23` of a rewrite that finds no row is the COBOL's own, `FILE STATUS IS: NNNN0023`
//! (CBACT04C-036). Every other path shows `93` for the held claim and `30`, a permanent error,
//! as the stand-in for the rest: both are `HALTED(H4)` and no parity result, since no VSAM status
//! maps to a `PostgreSQL` or file-system failure (CBACT04C-037, X-INTCALC-H3).
//!
//! A lookup the domain refuses, of an account, a cross-reference or a `DEFAULT` disclosure
//! record (CBACT04C-005, CBACT04C-007, CBACT04C-008, CBACT04C-011), emits the lines of
//! `IntcalcError::sysout_lines` the same way, `ABENDING PROGRAM` with its `abend_code` last, and
//! ends with [`JobError::Intcalc`].
//!
//! `9999-ABEND-PROGRAM` sets no return code (CBACT04C-038), so no abend carries one. After any
//! error the unfinished generation file is removed by the guard of `OutputFile`, as
//! `DISP=(NEW,CATLG,DELETE)` deletes it at abend: no path is printed and the next run takes the
//! same generation number (CBACT04C-038).
//!
//! # SYSTRAN stays a file
//!
//! The SYSTRAN records go to the generation file only and are never inserted into
//! `transactions`: merging them into the transaction file is COMBTRAN's work, which is out of
//! scope (AAP 0.2.3). `tran_cat_balances`, `card_xref` and `disclosure_groups` are only read, so
//! `accounts` is the one table that changes (CBACT04C-029).
//!
//! # The `DEFAULT` group (E07)
//!
//! Every fixture account leaves `ACCT-GROUP-ID` blank, so every disclosure lookup misses and the
//! domain reads the `DEFAULT` group with the same type and category (CBACT04C-010,
//! FIXINT-902). This module never substitutes a group of its own.
//!
//! # Job log and cardholder data
//!
//! Every `DISPLAY` line of `CBACT04C` is one tracing event of the job span whose message is the
//! displayed text, in program order (X-INTCALC-4). A run that ends normally emits, as `INFO`
//! events:
//!
//! - `START OF EXECUTION OF PROGRAM CBACT04C`, first, before the session opens (CBACT04C-002);
//! - for every category balance read, its 50 bytes as codec A encodes them, before that record's
//!   break and lookups (CBACT04C-003);
//! - for every disclosure lookup that falls back to `DEFAULT`, which the domain reports as
//!   `IntcalcEffect::DefaultGroupFallback`, `DISCLOSURE GROUP RECORD MISSING` and then
//!   `TRY WITH DEFAULT GROUP CODE`, after the break's rewrite (CBACT04C-010);
//! - `END OF EXECUTION OF PROGRAM CBACT04C`, once the generation and the session are closed
//!   (CBACT04C-030); the path `main` prints from [`JobOutcome::written`] follows it (AAP
//!   0.3.4.3).
//!
//! A run that abends emits the lines it reached, then those of its failure path as `ERROR`
//! events, with their cause events, as "Abends" lists them, and no end line. A session close
//! that fails adds a warning that names the close and the error only. Each SYSTRAN record
//! carries the full `XREF-CARD-NUM` (CBACT04C-021) in the local generation file only, which is
//! batch output and not egress. The lines emitted show account IDs and category balance records
//! at most, which hold no card number, CVV, SSN or password; the cause events never show a path
//! or a value, the job's errors are the fixed texts of [`JobError`] and its counts are plain
//! numbers, so nothing it emits carries a card number (Cardholder Data Redaction).
//!
//! # Rules
//!
//! - Single Schema Executor: no schema work here; against a database whose schema has not been
//!   applied the job abends at its first read, `ERROR READING ACCOUNT FILE` (CBACT04C-033).
//! - R4: every statement is a call of `persistence` on the run's session: a repository call, the
//!   writer claim of `PgSession::claim_writer`, or the session's close.
//! - Local-Only Runtime: the job writes `PostgreSQL` and the directory `Config::systran_dir`
//!   names, nothing else.
//! - Typed Dates and Timestamps: `ParmDate` is passed on as it was parsed, never rendered as
//!   text here, and both SYSTRAN timestamps come typed from the domain and the injected `Clock`
//!   (CBACT04C-022).

use copybook::FixedRecord;
use domain::batch::cbact04c::{
    ABEND_CODE, ABEND_LINE, CLOSE_TRANSACT_ERROR, DISCLOSURE_MISSING_LINE, END_LINE, Intcalc,
    IntcalcEffect, IntcalcError, OPEN_ACCOUNT_ERROR, OPEN_TCATBAL_ERROR, OPEN_TRANSACT_ERROR,
    READ_ACCOUNT_ERROR, READ_DISCGRP_ERROR, READ_TCATBAL_ERROR, READ_XREF_ERROR,
    REWRITE_ACCOUNT_ERROR, START_LINE, TRY_DEFAULT_LINE, WRITE_TRANSACT_ERROR, file_status_line,
};
use domain::clock::Clock;
use domain::records::{AccountRecord, TranCatBalanceRecord};
use persistence::{
    AccountRepository, CardXrefRepository, Dataset, DisclosureGroupRepository, PersistenceError,
    PgSession, PgStore, TranCatBalanceRepository,
};
use runtime::telemetry::{JobCounter, event_enabled};
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{self, FieldSet, Value};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use super::{JobContext, JobError, JobOutcome, OutputFile, SYSTRAN_DIR_SETTING, count};
use crate::cli::IntcalcArgs;

/// The generation base of the SYSTRAN file, `AWS.M2.CARDDEMO.SYSTRAN(+1)` of the `TRANSACT` DD
/// (`INTCALC.jcl:37-41`; CBACT04C-024).
const SYSTRAN_BASE: &str = "SYSTRAN";

/// `9300-ACCTFILE-CLOSE`, which the close of the run's session, holding the `accounts` claim,
/// stands for, named in the warning of a close that failed.
const CLOSE_ACCTFILE_OP: &str = "CLOSE ACCTFILE";

// The file operation each database or file call replaces, named in the event that reports its
// failure.

/// `0000-TCATBALF-OPEN`, the first of the four input opens the run's one session stands for
/// (X-INTCALC-7).
const OPEN_TCATBALF_OP: &str = "OPEN TCATBALF";
/// `0300-ACCTFILE-OPEN`, which the writer claim on `accounts` stands for (CBACT04C-039).
const OPEN_ACCTFILE_OP: &str = "OPEN ACCTFILE";
/// The keyed reads of `1100-GET-ACCT-DATA`, done at once.
const READ_ACCTFILE_OP: &str = "READ ACCTFILE";
/// The keyed reads of `1110-GET-XREF-DATA`, done at once.
const READ_XREFFILE_OP: &str = "READ XREFFILE";
/// The keyed reads of `1200-GET-INTEREST-RATE` and `1200-A-GET-DEFAULT-INT-RATE`, done at once.
const READ_DISCGRP_OP: &str = "READ DISCGRP";
/// The reads of `1000-TCATBALF-GET-NEXT`, done at once.
const READ_TCATBALF_OP: &str = "READ TCATBALF";
/// `0400-TRANFILE-OPEN`, the creation of the SYSTRAN generation.
const OPEN_TRANFILE_OP: &str = "OPEN TRANFILE";
/// The `REWRITE` of `1050-UPDATE-ACCOUNT`.
const REWRITE_ACCTFILE_OP: &str = "REWRITE ACCTFILE";
/// The `WRITE` of `1300-B-WRITE-TX`.
const WRITE_TRANFILE_OP: &str = "WRITE TRANFILE";
/// `9400-TRANFILE-CLOSE`, the flush and sync of the SYSTRAN generation.
const CLOSE_TRANFILE_OP: &str = "CLOSE TRANFILE";

/// The file status `9910-DISPLAY-IO-STATUS` shows after a failure line, as `file_status_line`
/// renders it (CBACT04C-036, CBACT04C-037).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// `23`, record not found: the status of a `REWRITE` that finds no record, shown as
    /// `FILE STATUS IS: NNNN0023` (CBACT04C-032, CBACT04C-036).
    NotFound,
    /// `93`, resource not available: VSAM's refusal of a second writer's `OPEN I-O`
    /// (CBACT04C-039). `HALTED(H4)`: the status VSAM returns there is not claimed as parity
    /// (CBACT04C-037).
    ResourceUnavailable,
    /// `30`, a permanent error: the stand-in for every other database or file-system failure.
    /// `HALTED(H4)`: no VSAM status maps to such a failure, so it is not claimed as parity
    /// (CBACT04C-037, X-INTCALC-H3).
    Permanent,
}

impl Status {
    /// The two status characters `9910-DISPLAY-IO-STATUS` receives.
    const fn code(self) -> [u8; 2] {
        match self {
            Self::NotFound => *b"23",
            Self::ResourceUnavailable => *b"93",
            Self::Permanent => *b"30",
        }
    }
}

/// Runs INTCALC with the PARM of `args`: rewrites every account in key order but the last,
/// writes the next `SYSTRAN` generation under `Config::systran_dir` and returns its path,
/// return code 0, and the counts `records` (category balances read), `transactions` (SYSTRAN
/// records written) and `accounts` (accounts rewritten).
///
/// # Errors
///
/// [`JobError::Abend`] with the failure line of the path, after its job-log lines (see
/// "Abends"):
///
/// - `ERROR OPENING TRANSACTION CATEGORY BALANCE` when the run's session cannot be opened, and
///   `ERROR OPENING ACCOUNT MASTER FILE` when another run holds the claim on `accounts` or the
///   claim fails, before any table is read or file created (CBACT04C-034, CBACT04C-039).
/// - `ERROR READING ACCOUNT FILE`, `ERROR READING XREF FILE`,
///   `ERROR READING DISCLOSURE GROUP FILE` or `ERROR READING TRANSACTION CATEGORY FILE` when
///   that table cannot be read, as every statement fails once the session is lost
///   (CBACT04C-033, CBACT04C-039).
/// - `ERROR OPENING TRANSACTION FILE` when the generation cannot be allocated or created, or
///   generation 9999 of `SYSTRAN` exists (CBACT04C-034).
/// - `ERROR RE-WRITING ACCOUNT FILE` when a rewrite finds no row or fails (CBACT04C-032).
/// - `ERROR WRITING TRANSACTION RECORD` when a SYSTRAN record cannot be written, and
///   `ERROR CLOSING TRANSACTION FILE` when the generation cannot be flushed or synced
///   (CBACT04C-031, CBACT04C-035).
///
/// [`JobError::Intcalc`] for the lookup abends of `CBACT04C` (CBACT04C-005, CBACT04C-007,
/// CBACT04C-008, CBACT04C-011), after the account rewrite that preceded the failed lookup is
/// persisted and the lookup's lines are emitted.
///
/// Every rewrite committed before an error stands, the generation file is removed and the run's
/// session is dropped, which gives up its claim on `accounts` (CBACT04C-038).
pub(crate) async fn run(ctx: &JobContext<'_>, args: &IntcalcArgs) -> Result<JobOutcome, JobError> {
    show_start();
    // Every statement of the run goes to its one session, the one holding its claim on
    // `accounts`, so that a session the server ends takes the claim and the run's next statement
    // with it, and no rewrite follows the loss (CBACT04C-039). Each statement on the session
    // commits on its own, as each VSAM rewrite stands once made (CBACT04C-026), so the job opens
    // no transaction.
    let mut session = open_acctfile(ctx.store).await?;
    let accounts = AccountRepository::find_all(&mut session)
        .await
        .map_err(|error| shown_failure(READ_ACCTFILE_OP, READ_ACCOUNT_ERROR, &error))?;
    let xrefs = CardXrefRepository::find_all(&mut session)
        .await
        .map_err(|error| shown_failure(READ_XREFFILE_OP, READ_XREF_ERROR, &error))?;
    let groups = DisclosureGroupRepository::find_all(&mut session)
        .await
        .map_err(|error| shown_failure(READ_DISCGRP_OP, READ_DISCGRP_ERROR, &error))?;
    let mut intcalc = Intcalc::new(args.parm_date.clone(), accounts, xrefs, groups);
    // The event names the setting of the directory, never its path: the job returns its abend,
    // so `main` cannot name it in the error.
    let mut systran = OutputFile::next_generation(&ctx.config.systran_dir(), SYSTRAN_BASE)
        .await
        .map_err(|error| {
            let error = error.naming_output_dir(SYSTRAN_DIR_SETTING);
            file_failure(OPEN_TRANFILE_OP, OPEN_TRANSACT_ERROR, &error)
        })?;
    let balances = TranCatBalanceRepository::find_all(&mut session)
        .await
        .map_err(|error| shown_failure(READ_TCATBALF_OP, READ_TCATBAL_ERROR, &error))?;
    let rewritten = post_interest(
        &mut intcalc,
        &balances,
        ctx.clock,
        &mut session,
        &mut systran,
    )
    .await?;
    // E08: the summary's unapplied last account and total are discarded, never rewritten;
    // only its counters are reported (CBACT04C-027).
    let summary = intcalc.finish();
    // `9400-TRANFILE-CLOSE`: a failure removes the generation and ends the run before the
    // `END OF EXECUTION` line (CBACT04C-035).
    let path = systran
        .finish()
        .await
        .map_err(|error| file_failure(CLOSE_TRANFILE_OP, CLOSE_TRANSACT_ERROR, &error))?;
    close_acctfile(session).await;
    // Every file is closed: the last line of the job log, which `main`'s path line follows
    // (CBACT04C-030).
    show_info_line(END_LINE);
    Ok(JobOutcome {
        written: vec![path],
        return_code: 0,
        counts: vec![
            (JobCounter::Records, summary.records_read()),
            (JobCounter::Transactions, summary.transactions_written()),
            (JobCounter::Accounts, count(rewritten)),
        ],
    })
}

/// `0300-ACCTFILE-OPEN`, the `OPEN I-O` of the account cluster, which admits one writer: opens
/// the run's session, the one connection that stands for the input opens before it, and claims
/// `accounts` on it (`CBACT04C.cbl:234-305`; CBACT04C-034, CBACT04C-039). Returns the session,
/// on which the run then issues every statement.
///
/// # Errors
///
/// [`JobError::Abend`] with no return code (CBACT04C-038): with the first open's
/// `ERROR OPENING TRANSACTION CATEGORY BALANCE` and the stand-in status when the session cannot
/// be opened (CBACT04C-034, X-INTCALC-7); with the account file's
/// `ERROR OPENING ACCOUNT MASTER FILE` and status `93` when another run holds the claim, and
/// with that line and the stand-in status when the claim fails as a statement (CBACT04C-039).
/// The session, when it was opened, is dropped as the error returns.
async fn open_acctfile(store: &PgStore) -> Result<PgSession, JobError> {
    let mut session = store
        .session()
        .await
        .map_err(|error| shown_failure(OPEN_TCATBALF_OP, OPEN_TCATBAL_ERROR, &error))?;
    match session.claim_writer(Dataset::Accounts).await {
        Ok(true) => Ok(session),
        Ok(false) => Err(abend(OPEN_ACCOUNT_ERROR, Status::ResourceUnavailable)),
        Err(error) => Err(shown_failure(OPEN_ACCTFILE_OP, OPEN_ACCOUNT_ERROR, &error)),
    }
}

/// `9300-ACCTFILE-CLOSE` for the claim: the run's session is closed once the generation is
/// complete, which gives the claim up (CBACT04C-039).
///
/// A close that fails neither changes the run's outcome nor leaves the claim held: every
/// rewrite has completed, and the session's connection is closed whether or not the unlock was
/// answered, which ends the lock on the server. The failure is reported in one warning event
/// naming the close and the error, and the run keeps its outcome.
async fn close_acctfile(session: PgSession) {
    if let Err(error) = session.close().await {
        emit(
            &CLAIM_RELEASE_WARN,
            [
                &CLAIM_RELEASE_FAILED,
                &CLOSE_ACCTFILE_OP,
                &field::display(&error),
            ],
        );
    }
}

/// The main loop of `CBACT04C` over `balances`, in order (`CBACT04C.cbl:188-222`): each record
/// is shown as its job-log line (CBACT04C-003), then goes to `intcalc`, and its effects are
/// applied as returned, an account update through `accounts`, a `DEFAULT` fallback as its two
/// job-log lines (CBACT04C-010) and a SYSTRAN record appended to `systran` (CBACT04C-024,
/// CBACT04C-026). Returns the number of accounts rewritten.
///
/// # Errors
///
/// The first failure, as [`run`] reports it: [`JobError::Abend`] with
/// `ERROR RE-WRITING ACCOUNT FILE` or `ERROR WRITING TRANSACTION RECORD` (CBACT04C-031,
/// CBACT04C-032), or [`JobError::Intcalc`] for a lookup abend, after its lines. A lookup abend
/// carrying the previous account's update has that update rewritten first (CBACT04C-005,
/// CBACT04C-038).
async fn post_interest<A: AccountRepository>(
    intcalc: &mut Intcalc,
    balances: &[TranCatBalanceRecord],
    clock: &dyn Clock,
    accounts: &mut A,
    systran: &mut OutputFile,
) -> Result<usize, JobError> {
    let mut rewritten = 0;
    for balance in balances {
        // `DISPLAY TRAN-CAT-BAL-RECORD` follows the read and comes before the break and the
        // lookups, so a rewrite or lookup that abends follows it (CBACT04C-003).
        show_info_line(&record_line(balance));
        let effects = match intcalc.process(balance, clock) {
            Ok(effects) => effects,
            Err(error) => {
                if let Some(account) = error.rewritten_before_abend() {
                    rewrite(accounts, account).await?;
                }
                return Err(lookup_abend(error));
            }
        };
        for effect in effects {
            match effect {
                IntcalcEffect::AccountUpdate(account) => {
                    rewrite(accounts, &account).await?;
                    rewritten += 1;
                }
                IntcalcEffect::DefaultGroupFallback => show_default_fallback(),
                IntcalcEffect::SystemTransaction(record) => {
                    systran
                        .write_record(&record.encode())
                        .await
                        .map_err(|error| {
                            file_failure(WRITE_TRANFILE_OP, WRITE_TRANSACT_ERROR, &error)
                        })?;
                }
            }
        }
    }
    Ok(rewritten)
}

/// `REWRITE FD-ACCTFILE-REC FROM ACCOUNT-RECORD` of `1050-UPDATE-ACCOUNT`, committed as it is
/// made (`CBACT04C.cbl:356-369`; CBACT04C-026).
///
/// # Errors
///
/// [`JobError::Abend`] with the program's `ERROR RE-WRITING ACCOUNT FILE` (CBACT04C-032): with
/// `FILE STATUS IS: NNNN0023` when no row has the account's key, the status `23` of a `REWRITE`
/// that finds no record (CBACT04C-036), and after the event carrying the error, with the
/// stand-in status, when the statement fails (CBACT04C-037).
async fn rewrite<A: AccountRepository>(
    accounts: &mut A,
    account: &AccountRecord,
) -> Result<(), JobError> {
    match AccountRepository::update(accounts, account).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(abend(REWRITE_ACCOUNT_ERROR, Status::NotFound)),
        Err(error) => Err(shown_failure(
            REWRITE_ACCTFILE_OP,
            REWRITE_ACCOUNT_ERROR,
            &error,
        )),
    }
}

/// The failure of repository call `operation` on the path that displays `line`, with the
/// stand-in status: the event carrying the error, then the path's lines and the abend.
fn shown_failure(
    operation: &'static str,
    line: &'static str,
    error: &PersistenceError,
) -> JobError {
    report(operation, error);
    abend(line, Status::Permanent)
}

/// The failure of local-file call `operation` on the path that displays `line`, with the
/// stand-in status: the event carrying the error, then the path's lines and the abend.
fn file_failure(operation: &'static str, line: &'static str, error: &JobError) -> JobError {
    report_file(operation, error);
    abend(line, Status::Permanent)
}

/// Reports why a repository call failed, for the operator: the file operation it replaces and
/// the error's text, which names an SQLSTATE, a constraint or a column only (Cardholder Data
/// Redaction).
fn report(operation: &'static str, error: &PersistenceError) {
    emit(
        &DATABASE_CALL_ERROR,
        [&DATABASE_CALL_FAILED, &operation, &field::display(error)],
    );
}

/// Reports why a local-file call failed: the file operation and the error's text, a static step
/// and the I/O error kind, or the generation limit and the `SYSTRAN` base, never the path.
fn report_file(operation: &'static str, error: &JobError) {
    emit(
        &FILE_OPERATION_ERROR,
        [&FILE_OPERATION_FAILED, &operation, &field::display(error)],
    );
}

/// `9999-ABEND-PROGRAM` after the failure line `line` and the status line of `status`, as
/// [`show_abend`] emits them. The abend carries no return code, and the caller returns it at
/// once, which drops the unfinished generation and so removes it.
fn abend(line: &'static str, status: Status) -> JobError {
    show_abend(line, status);
    JobError::Abend {
        code: None,
        reason: line,
    }
}

/// `DISPLAY 'START OF EXECUTION OF PROGRAM CBACT04C'`, the first line of every run whose PARM
/// was accepted, before any open: one `INFO` event whose message is the line (X-INTCALC-4;
/// CBACT04C-002). [`run`] shows it first, and `main` shows it before its connection event when
/// it cannot connect, as the connection is the first open (CBACT04C-034).
pub(crate) fn show_start() {
    show_info_line(START_LINE);
}

/// The job log of the first open's failure when `main` cannot connect to `PostgreSQL`, the
/// connection that stands for the four input opens: `ERROR OPENING TRANSACTION CATEGORY BALANCE`,
/// the stand-in status line and `ABENDING PROGRAM`, as for a session that cannot be opened
/// (CBACT04C-034, CBACT04C-038, X-INTCALC-7). `main` shows [`show_start`]'s line and its own
/// connection event, which carries the error, before them (CBACT04C-002), and reports the
/// connection error itself.
pub(crate) fn show_failed_connection() {
    show_abend(OPEN_TCATBAL_ERROR, Status::Permanent);
}

/// `DISPLAY TRAN-CAT-BAL-RECORD` of the main loop: the 50 bytes of `balance` as codec A encodes
/// them, the 17-byte key, `TRAN-CAT-BAL` in zoned form and the 22-byte `FILLER`, one character
/// per byte, so that the line is total over any byte (`CBACT04C.cbl:193`; CBACT04C-003). The
/// record holds no card number, CVV, SSN or password, so no byte is masked (Cardholder Data
/// Redaction).
fn record_line(balance: &TranCatBalanceRecord) -> String {
    balance.encode().into_iter().map(char::from).collect()
}

/// The `INVALID KEY` lines of a disclosure lookup that falls back to `DEFAULT`, in order, each
/// an `INFO` event whose message is the line: `DISCLOSURE GROUP RECORD MISSING`, then
/// `TRY WITH DEFAULT GROUP CODE` (`CBACT04C.cbl:418-419`; CBACT04C-010, X-INTCALC-4). The lookup
/// that finds no `DEFAULT` record either shows them as `ERROR` events through [`lookup_abend`]
/// (CBACT04C-011), never here.
fn show_default_fallback() {
    show_info_line(DISCLOSURE_MISSING_LINE);
    show_info_line(TRY_DEFAULT_LINE);
}

/// The `DISPLAY` lines of a failure path: the failure line `line` and the status line of
/// `status`, each an `ERROR` event whose message is the line (X-INTCALC-4; CBACT04C-036,
/// CBACT04C-037), then `ABENDING PROGRAM` (CBACT04C-038).
fn show_abend(line: &'static str, status: Status) {
    show_error_line(line);
    show_error_line(&file_status_line(status.code()));
    abending_program();
}

/// `DISPLAY 'ABENDING PROGRAM'` and the `CALL 'CEE3ABD'` that follows it: one `ERROR` event
/// whose field `abend_code` records `ABCODE`, 999 (`CBACT04C.cbl:628-632`; CBACT04C-038,
/// X-INTCALC-3).
fn abending_program() {
    emit(&ABEND_ERROR, [&ABEND_LINE, &ABEND_CODE]);
}

/// A lookup abend the domain returns: each line `CBACT04C` displays for it, in order and as
/// `ERROR` events, `ABENDING PROGRAM` with its abend code among them (CBACT04C-005,
/// CBACT04C-007, CBACT04C-008, CBACT04C-011, CBACT04C-038), then the error. The lines show an
/// account ID at most, never a card number.
fn lookup_abend(error: IntcalcError) -> JobError {
    for line in error.sysout_lines() {
        if line == ABEND_LINE {
            abending_program();
        } else {
            show_error_line(&line);
        }
    }
    JobError::Intcalc(error)
}

/// Shows `line` as one `INFO` event of the job log whose message is the line (X-INTCALC-4).
fn show_info_line(line: &str) {
    emit(&JOB_LOG_INFO, [&line]);
}

/// Shows `line` as one `ERROR` event of the job log whose message is the line (X-INTCALC-4).
fn show_error_line(line: &str) {
    emit(&JOB_LOG_ERROR, [&line]);
}

/// The name of the job-log callsites, whose events carry a displayed line as their message.
const JOB_LOG: &str = "CBACT04C job log";

/// The fields of a job-log line: its message, the line.
const JOB_LOG_FIELDS: &[&str; 1] = &["message"];

/// The fields of `ABENDING PROGRAM`: its message, the line, and the abend code.
const ABEND_FIELDS: &[&str; 2] = &["message", "abend_code"];

/// The fields of an event that reports a failed call: its message, the file operation the call
/// replaces and the error's text.
const FAILURE_FIELDS: &[&str; 3] = &["message", "operation", "error"];

/// The message of the warning of a session close that failed, in [`close_acctfile`].
const CLAIM_RELEASE_FAILED: &str =
    "CBACT04C writer claim release failed; the closed session ends the claim";

/// The message of the event of a failed repository call, in [`report`].
const DATABASE_CALL_FAILED: &str = "CBACT04C database call failed";

/// The message of the event of a failed local-file call, in [`report_file`].
const FILE_OPERATION_FAILED: &str = "CBACT04C file operation failed";

/// The callsite of one event of this job, whose metadata names `N` fields, `message` first.
///
/// [`event_metadata`] and [`emit`] take the same `N`, so an event always carries exactly one
/// value per field it declares.
struct EventSite<const N: usize>(DefaultCallsite);

impl<const N: usize> EventSite<N> {
    /// The callsite of the event that `metadata` describes.
    const fn new(metadata: &'static Metadata<'static>) -> Self {
        Self(DefaultCallsite::new(metadata))
    }
}

/// The metadata of the event `name` at `level`, with the field `names`, for the callsite `site`.
///
/// The target and the module path are this module's path, the target a `tracing` macro gives an
/// event here, so `RUST_LOG` directives select the event exactly as they would select a macro's.
/// No source file or line is given: the JSON output and the span export print neither.
const fn event_metadata<const N: usize>(
    name: &'static str,
    level: Level,
    names: &'static [&'static str; N],
    site: &'static EventSite<N>,
) -> Metadata<'static> {
    Metadata::new(
        name,
        module_path!(),
        level,
        None,
        None,
        Some(module_path!()),
        FieldSet::new(names, Identifier(&site.0)),
        Kind::EVENT,
    )
}

/// Emits one event of `site` in the current span with `values`, one per field the site names
/// and in that order, when the current subscriber enables it.
///
/// The decision is `runtime::telemetry::event_enabled`, which takes the steps an event macro
/// takes, so the level filters, the JSON layer and the span export treat the event exactly as
/// they treat a macro's.
fn emit<const N: usize>(site: &'static EventSite<N>, values: [&dyn Value; N]) {
    let callsite = &site.0;
    if !event_enabled(callsite) {
        return;
    }
    let metadata = callsite.metadata();
    let values = values.map(Some);
    Event::dispatch(metadata, &metadata.fields().value_set_all(&values));
}

// Written out: the `tracing` event macros, `info!`, `warn!` and `error!`, expand to an
// `allow(unused_imports)` lint suppression.
/// The `INFO` callsite of a job-log line.
static JOB_LOG_INFO: EventSite<1> = EventSite::new(&JOB_LOG_INFO_METADATA);

/// The metadata of [`JOB_LOG_INFO`].
static JOB_LOG_INFO_METADATA: Metadata<'static> =
    event_metadata(JOB_LOG, Level::INFO, JOB_LOG_FIELDS, &JOB_LOG_INFO);

/// The `ERROR` callsite of a job-log line of a failure path.
static JOB_LOG_ERROR: EventSite<1> = EventSite::new(&JOB_LOG_ERROR_METADATA);

/// The metadata of [`JOB_LOG_ERROR`].
static JOB_LOG_ERROR_METADATA: Metadata<'static> =
    event_metadata(JOB_LOG, Level::ERROR, JOB_LOG_FIELDS, &JOB_LOG_ERROR);

/// The `ERROR` callsite of `ABENDING PROGRAM` and its abend code, in [`abending_program`].
static ABEND_ERROR: EventSite<2> = EventSite::new(&ABEND_ERROR_METADATA);

/// The metadata of [`ABEND_ERROR`].
static ABEND_ERROR_METADATA: Metadata<'static> =
    event_metadata(ABEND_LINE, Level::ERROR, ABEND_FIELDS, &ABEND_ERROR);

/// The `WARN` callsite of a session close that failed, in [`close_acctfile`].
static CLAIM_RELEASE_WARN: EventSite<3> = EventSite::new(&CLAIM_RELEASE_WARN_METADATA);

/// The metadata of [`CLAIM_RELEASE_WARN`].
static CLAIM_RELEASE_WARN_METADATA: Metadata<'static> = event_metadata(
    CLAIM_RELEASE_FAILED,
    Level::WARN,
    FAILURE_FIELDS,
    &CLAIM_RELEASE_WARN,
);

/// The `ERROR` callsite of a failed repository call, in [`report`].
static DATABASE_CALL_ERROR: EventSite<3> = EventSite::new(&DATABASE_CALL_ERROR_METADATA);

/// The metadata of [`DATABASE_CALL_ERROR`].
static DATABASE_CALL_ERROR_METADATA: Metadata<'static> = event_metadata(
    DATABASE_CALL_FAILED,
    Level::ERROR,
    FAILURE_FIELDS,
    &DATABASE_CALL_ERROR,
);

/// The `ERROR` callsite of a failed local-file call, in [`report_file`].
static FILE_OPERATION_ERROR: EventSite<3> = EventSite::new(&FILE_OPERATION_ERROR_METADATA);

/// The metadata of [`FILE_OPERATION_ERROR`].
static FILE_OPERATION_ERROR_METADATA: Metadata<'static> = event_metadata(
    FILE_OPERATION_FAILED,
    Level::ERROR,
    FAILURE_FIELDS,
    &FILE_OPERATION_ERROR,
);

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::btree_map::Entry;
    use std::error::Error;
    use std::fs;
    use std::future::poll_fn;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::Poll;
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::{NaiveDate, Timelike};
    use copybook::{FixedRecord, PicDate, PicU, PicX, TsForm};
    use domain::batch::cbact04c::{
        DEFAULT_GROUP, DISCLOSURE_MISSING_LINE, Intcalc, IntcalcError, ParmDate,
        REWRITE_ACCOUNT_ERROR, TRY_DEFAULT_LINE,
    };
    use domain::clock::FixedClock;
    use domain::money::{Decimal, Money, Rate};
    use domain::records::{
        AccountRecord, CardXrefRecord, DisclosureGroupRecord, TranCatBalanceRecord,
        TransactionRecord,
    };
    use domain::sensitive::CardNumber;
    use persistence::{
        AccountCustomerChain, AccountRepository, CardXrefRepository, ChainLink, Dataset,
        DisclosureGroupRepository, PersistenceError, PgStore, TranCatBalanceRepository,
    };
    use runtime::config::Config;
    use test_support::fixtures::{fixture_path, start_loaded_postgres};
    use test_support::postgres::PostgresGuard;
    use test_support::trace_capture::{self, TraceCapture};

    use super::{OPEN_ACCOUNT_ERROR, post_interest, run};
    use crate::cli::IntcalcArgs;
    use crate::jobs::{JobContext, JobError, OutputFile, posttran};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Width of a `CVTRA05Y` SYSTRAN record (CBACT04C-024).
    const SYSTRAN_WIDTH: usize = 350;

    /// The shape of both SYSTRAN timestamps: layout `D`, `YYYY-MM-DD-HH.MM.SS.ffffff`, with
    /// the fraction cut to hundredths, `9` standing for any ASCII digit (CBACT04C-022).
    const DB2_HUNDREDTHS: &[u8; 26] = b"9999-99-99-99.99.99.990000";

    /// Nanoseconds in one hundredth of a second, the finest unit of a SYSTRAN timestamp
    /// (CBACT04C-022).
    const NANOS_PER_HUNDREDTH: u32 = 10_000_000;

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
                "carddemo-batch-intcalc-{}-{nanos}-{sequence}",
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

    /// The configuration of a run whose outputs go under `var_dir`, every other variable unset.
    fn var_config(var_dir: &Path) -> Result<Config, Box<dyn Error>> {
        let var_dir = var_dir.display().to_string();
        Ok(Config::from_lookup(|name| {
            (name == "CARDDEMO_VAR_DIR").then(|| var_dir.clone())
        })?)
    }

    /// Every account, read back through `store`.
    async fn accounts(store: &PgStore) -> Result<Vec<AccountRecord>, Box<dyn Error>> {
        let mut store = store.clone();
        Ok(AccountRepository::find_all(&mut store).await?)
    }

    /// A fixture-loaded database after `posttran` has posted `dailytran.txt` to it, the
    /// `INTEREST-POSTED` scenario of `fixtures.md`, whose category balances earn interest
    /// (FIXINT-901). The reject generation goes to `var_dir`.
    async fn posted_database(var_dir: &Path) -> Result<PostgresGuard, Box<dyn Error>> {
        let guard = start_loaded_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let config = var_config(var_dir)?;
        let clock = clock()?;
        let ctx = JobContext {
            store: &store,
            config: &config,
            clock: &clock,
        };
        posttran::run(&ctx, &fixture_path("dailytran.txt")).await?;
        Ok(guard)
    }

    /// The accounts one interest run leaves over the tables `store` holds, computed by the loop
    /// over the in-memory table, its SYSTRAN records going to a generation in `dir` that is
    /// dropped unfinished.
    async fn after_one_run(
        store: &PgStore,
        dir: &Path,
    ) -> Result<Vec<AccountRecord>, Box<dyn Error>> {
        let mut reads = store.clone();
        let before = AccountRepository::find_all(&mut reads).await?;
        let mut single = Intcalc::new(
            ParmDate::DEFAULT,
            before.clone(),
            CardXrefRepository::find_all(&mut reads).await?,
            DisclosureGroupRepository::find_all(&mut reads).await?,
        );
        let balances = TranCatBalanceRepository::find_all(&mut reads).await?;
        let mut ledger = Ledger::holding(&before, false);
        let mut systran = OutputFile::next_generation(dir, "SYSTRAN").await?;
        post_interest(&mut single, &balances, &clock()?, &mut ledger, &mut systran).await?;
        let after: Vec<AccountRecord> = ledger.rows.into_values().collect();
        if after == before {
            return Err("one interest run over the posted tables rewrites no account".into());
        }
        Ok(after)
    }

    /// The names of the files in `dir`, sorted; none when `dir` does not exist.
    fn file_names(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            names.push(entry?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    /// An in-memory `accounts` table. `update` rewrites a row it holds and answers `false` for
    /// a key it does not, as `PgStore` does; with `failing` set every update fails as a
    /// statement. `rewrites` lists the keys rewritten, in order.
    struct Ledger {
        rows: BTreeMap<PicU<11>, AccountRecord>,
        rewrites: Vec<PicU<11>>,
        failing: bool,
    }

    impl Ledger {
        fn holding(accounts: &[AccountRecord], failing: bool) -> Self {
            Self {
                rows: accounts
                    .iter()
                    .map(|account| (account.id, account.clone()))
                    .collect(),
                rewrites: Vec::new(),
                failing,
            }
        }
    }

    impl AccountRepository for Ledger {
        fn find_with_customer(
            &mut self,
            _acct_id: PicU<11>,
        ) -> impl Future<Output = AccountCustomerChain> + Send {
            std::future::ready(AccountCustomerChain {
                xref: ChainLink::NotFound,
                account: ChainLink::NotReached,
                customer: ChainLink::NotReached,
            })
        }

        fn find_by_id(
            &mut self,
            id: PicU<11>,
        ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
            std::future::ready(Ok(self.rows.get(&id).cloned()))
        }

        fn lock_by_id(
            &mut self,
            id: PicU<11>,
        ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
            std::future::ready(Ok(self.rows.get(&id).cloned()))
        }

        fn find_all(
            &mut self,
        ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send {
            std::future::ready(Ok(self.rows.values().cloned().collect()))
        }

        fn insert(
            &mut self,
            record: &AccountRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            let outcome = match self.rows.entry(record.id) {
                Entry::Vacant(row) => {
                    row.insert(record.clone());
                    Ok(())
                }
                Entry::Occupied(_) => Err(PersistenceError::Duplicate {
                    constraint: Some("accounts_pkey".to_owned()),
                }),
            };
            std::future::ready(outcome)
        }

        fn update(
            &mut self,
            record: &AccountRecord,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            let outcome = if self.failing {
                Err(PersistenceError::Unavailable)
            } else if let Some(row) = self.rows.get_mut(&record.id) {
                *row = record.clone();
                self.rewrites.push(record.id);
                Ok(true)
            } else {
                Ok(false)
            };
            std::future::ready(outcome)
        }

        fn replace_all(
            &mut self,
            records: &[AccountRecord],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            self.rows = records
                .iter()
                .map(|record| (record.id, record.clone()))
                .collect();
            std::future::ready(Ok(crate::jobs::count(self.rows.len())))
        }
    }

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, Box<dyn Error>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    /// Account `id` with balance `bal_cents`, non-zero cycle fields and the blank group of every
    /// fixture account (E07).
    fn account(id: u64, bal_cents: i64) -> Result<AccountRecord, Box<dyn Error>> {
        Ok(AccountRecord {
            id: PicU::new(id)?,
            active_status: PicX::from_array(*b"Y"),
            curr_bal: money(bal_cents)?,
            credit_limit: money(500_000)?,
            cash_credit_limit: money(100_000)?,
            open_date: PicDate::BLANK,
            expiraion_date: PicDate::BLANK,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: money(12_345)?,
            curr_cyc_debit: money(-6_789)?,
            addr_zip: PicX::from_array(*b"A000000000"),
            group_id: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// A synthetic card number for account `id`, in no fixture file.
    fn card(id: u8) -> CardNumber {
        let mut digits = *b"4000000000000000";
        if let Some(last) = digits.last_mut() {
            *last = b'0'.saturating_add(id);
        }
        CardNumber::new(PicX::from_array(digits))
    }

    fn xref(id: u8) -> Result<CardXrefRecord, Box<dyn Error>> {
        Ok(CardXrefRecord {
            card_num: card(id),
            cust_id: PicU::new(u64::from(id))?,
            acct_id: PicU::new(u64::from(id))?,
            filler: PicX::spaces(),
        })
    }

    /// Category balance (`acct_id`, `01`, `0001`) of `bal_cents`.
    fn balance(acct_id: u64, bal_cents: i64) -> Result<TranCatBalanceRecord, Box<dyn Error>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(*b"01"),
            cd: PicU::new(1)?,
            bal: money(bal_cents)?,
            filler: PicX::spaces(),
        })
    }

    /// A run over accounts 1 and 2 and their cards, with only the `DEFAULT` rate of 15.00 for
    /// type `01`, category `0001`, as `discgrp.txt:18` holds it (CBACT04C-010).
    fn intcalc() -> Result<Intcalc, Box<dyn Error>> {
        let default_rate = DisclosureGroupRecord {
            acct_group_id: DEFAULT_GROUP,
            tran_type_cd: PicX::from_array(*b"01"),
            tran_cat_cd: PicU::new(1)?,
            int_rate: Rate::new(Decimal::new(1_500, 2))?,
            filler: PicX::spaces(),
        };
        Ok(Intcalc::new(
            ParmDate::DEFAULT,
            [account(1, 10_000)?, account(2, 20_000)?],
            [xref(1)?, xref(2)?],
            [default_rate],
        ))
    }

    fn clock() -> Result<FixedClock, Box<dyn Error>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_opt(0, 0, 0))
            .ok_or("2022-07-18 00:00:00 is a valid instant")?;
        Ok(FixedClock::new(at))
    }

    /// The raw SYSTRAN records of `bytes`, the content of a finished generation, after checking
    /// that each is 350 bytes followed by LF (CBACT04C-024).
    fn systran_records(bytes: &[u8]) -> Result<Vec<&[u8]>, Box<dyn Error>> {
        let lines = bytes.chunks_exact(SYSTRAN_WIDTH + 1);
        if !lines.remainder().is_empty() {
            return Err("the generation ends in a partial record".into());
        }
        lines
            .map(|line| match line.split_at(SYSTRAN_WIDTH) {
                (raw, b"\n") => Ok(raw),
                _ => Err("a SYSTRAN record is not followed by LF".into()),
            })
            .collect()
    }

    /// Whether `text` has the shape of [`DB2_HUNDREDTHS`]. Time fields are compared by format
    /// only, never by the clock's value (AAP 0.5.6).
    fn is_db2_hundredths(text: &[u8]) -> bool {
        text.len() == DB2_HUNDREDTHS.len()
            && text
                .iter()
                .zip(DB2_HUNDREDTHS)
                .all(|(&byte, &shape)| match shape {
                    b'9' => byte.is_ascii_digit(),
                    _ => byte == shape,
                })
    }

    /// Runs `test` to completion on a current-thread runtime with I/O and time enabled, the
    /// runtime tokio's test attribute builds, without the lint attributes that attribute's
    /// expansion carries. The test runs on the calling thread, so a trace capture the test
    /// starts sees every event of it.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// Drives `first` and `second` together on the calling task until both have finished and
    /// returns their outputs in that order. Every wake polls each future not yet finished and
    /// never a finished one again, so neither waits for the other to end, as tokio's join macro
    /// drives its futures, without the pinning and lint attributes that macro's expansion
    /// carries.
    async fn join<A: Future, B: Future>(first: A, second: B) -> (A::Output, B::Output) {
        let mut first = Box::pin(first);
        let mut second = Box::pin(second);
        let mut first_output = None;
        let mut second_output = None;
        poll_fn(|cx| {
            if first_output.is_none() {
                if let Poll::Ready(output) = first.as_mut().poll(cx) {
                    first_output = Some(output);
                }
            }
            if second_output.is_none() {
                if let Poll::Ready(output) = second.as_mut().poll(cx) {
                    second_output = Some(output);
                }
            }
            match (first_output.take(), second_output.take()) {
                (Some(first), Some(second)) => Poll::Ready((first, second)),
                (first, second) => {
                    first_output = first;
                    second_output = second;
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// Account 1 is rewritten at the break of account 2 with its interest added and both cycle
    /// fields zeroed; account 2, the last in key order, keeps every field (E08), while the
    /// records of both are written in key order with consecutive suffixes.
    // audit: CBACT04C-024
    // audit: CBACT04C-026
    // audit: CBACT04C-027
    // audit: FIXINT-903
    #[test]
    fn every_account_but_the_last_is_rewritten() -> TestResult {
        block_on(async {
            let scratch = Scratch::new()?;
            let before = [account(1, 10_000)?, account(2, 20_000)?];
            let mut ledger = Ledger::holding(&before, false);
            let mut run = intcalc()?;
            let mut systran = OutputFile::next_generation(scratch.path(), "SYSTRAN").await?;
            let balances = [balance(1, 120_000)?, balance(2, 240_000)?];
            let rewritten =
                post_interest(&mut run, &balances, &clock()?, &mut ledger, &mut systran).await?;
            let summary = run.finish();
            let path = systran.finish().await?;

            assert_eq!(rewritten, 1);
            assert_eq!(ledger.rewrites, [PicU::new(1)?]);
            let first = ledger.rows.get(&PicU::new(1)?).ok_or("account 1 is held")?;
            // 100.00 and the 15.00 of (1200.00 * 15.00) / 1200 (ARITH-001, ARITH-013).
            assert_eq!(first.curr_bal, money::<10>(11_500)?);
            assert_eq!(first.curr_cyc_credit, Money::<10>::ZERO);
            assert_eq!(first.curr_cyc_debit, Money::<10>::ZERO);
            assert_eq!(ledger.rows.get(&PicU::new(2)?), before.get(1));
            assert_eq!(summary.records_read(), 2);
            assert_eq!(summary.transactions_written(), 2);
            assert_eq!(summary.unapplied_last_account(), Some(PicU::new(2)?));

            assert_eq!(path, scratch.path().join("SYSTRAN.G0001"));
            let bytes = fs::read(&path)?;
            let records = systran_records(&bytes)?;
            let expected = [
                ("2022071800000001", 1, 1_500),
                ("2022071800000002", 2, 3_000),
            ];
            assert_eq!(records.len(), expected.len());
            for (raw, (id, owner, cents)) in records.into_iter().zip(expected) {
                let record = TransactionRecord::decode(raw)?;
                assert_eq!(record.id, PicX::from_text(id)?, "{id}");
                assert_eq!(record.amt, money::<9>(cents)?, "{id}");
                assert!(record.card_num == card(owner), "{id}: card number");
                // Time fields are compared by format only (AAP 0.5.6): one clock reading, in layout
                // `D` and cut to hundredths, fills both fields (CBACT04C-022).
                let orig_text = raw.get(278..304).ok_or("TRAN-ORIG-TS is in the record")?;
                let proc_text = raw.get(304..330).ok_or("TRAN-PROC-TS is in the record")?;
                assert!(is_db2_hundredths(orig_text), "{id}: TRAN-ORIG-TS");
                assert!(is_db2_hundredths(proc_text), "{id}: TRAN-PROC-TS");
                assert_eq!(orig_text, proc_text, "{id}: one reading for both fields");
                assert_eq!(
                    record.orig_ts, record.proc_ts,
                    "{id}: one reading for both fields"
                );
                let stamp = record.orig_ts.value().ok_or("TRAN-ORIG-TS is not blank")?;
                assert_eq!(stamp.form(), TsForm::Db2, "{id}: layout D");
                assert_eq!(
                    stamp.value().nanosecond() % NANOS_PER_HUNDREDTH,
                    0,
                    "{id}: hundredths"
                );
            }
            Ok(())
        })
    }

    /// How an abending run must end.
    #[derive(Clone, Copy, Debug)]
    enum Ends {
        /// `ERROR RE-WRITING ACCOUNT FILE`: the rewrite found no row.
        RewriteNotFound,
        /// `ERROR RE-WRITING ACCOUNT FILE`: the rewrite failed as a statement.
        StoreFailure,
        /// The account lookup at a break found no record.
        AccountNotFound,
    }

    /// One abending run: the accounts the table holds, whether every update fails, the accounts
    /// of the two category balances, the expected end, the rewrites that stand and the messages
    /// of the events the run emits after the second balance's record line, in order.
    struct Case {
        held: &'static [u64],
        failing: bool,
        keys: [u64; 2],
        ends: Ends,
        kept: &'static [u64],
        log: &'static [&'static str],
    }

    /// The job log of a rewrite that finds no row: the failure line, status `23` and the abend
    /// (CBACT04C-032, CBACT04C-036, CBACT04C-038).
    const REWRITE_NOT_FOUND_LOG: &[&str] = &[
        "ERROR RE-WRITING ACCOUNT FILE",
        "FILE STATUS IS: NNNN0023",
        "ABENDING PROGRAM",
    ];

    /// The abending runs of [`abends_keep_the_rewrites_made_before_them`]: a rewrite that finds
    /// no row, a rewrite that fails as a statement, a lookup that finds no account after a
    /// rewrite stands, and a rewrite that finds no row before a lookup that would find none.
    const ABENDS: [Case; 4] = [
        Case {
            held: &[2],
            failing: false,
            keys: [1, 2],
            ends: Ends::RewriteNotFound,
            kept: &[],
            log: REWRITE_NOT_FOUND_LOG,
        },
        Case {
            held: &[1, 2],
            failing: true,
            keys: [1, 2],
            ends: Ends::StoreFailure,
            kept: &[],
            log: &[
                "CBACT04C database call failed",
                "ERROR RE-WRITING ACCOUNT FILE",
                "FILE STATUS IS: NNNN0030",
                "ABENDING PROGRAM",
            ],
        },
        Case {
            held: &[1, 2],
            failing: false,
            keys: [1, 3],
            ends: Ends::AccountNotFound,
            kept: &[1],
            log: &[
                "ACCOUNT NOT FOUND: 00000000003",
                "ERROR READING ACCOUNT FILE",
                "FILE STATUS IS: NNNN0023",
                "ABENDING PROGRAM",
            ],
        },
        Case {
            held: &[2],
            failing: false,
            keys: [1, 3],
            ends: Ends::RewriteNotFound,
            kept: &[],
            log: REWRITE_NOT_FOUND_LOG,
        },
    ];

    /// `TRAN-CAT-BAL PIC S9(09)V99` of [`balance`]`(_, 120_000)`, 1200.00, as its record line
    /// shows it: eleven digits, the last overpunched with the positive sign, `0` as `{`
    /// (PIC-005).
    const BALANCE_1200_ZONED: &str = "0000012000{";

    /// The record line of [`balance`]`(acct_id, 120_000)`: the key (`acct_id` in 11 digits, type
    /// `01`, category `0001`), the zoned balance and the 22 spaces of its `FILLER` (CBACT04C-003).
    fn record_shown(acct_id: u64) -> String {
        format!("{acct_id:011}010001{BALANCE_1200_ZONED}{:22}", "")
    }

    /// The job log of an abending run over the balances of accounts `first` and `second`: the
    /// first's record line and `DEFAULT` fallback pair, the second's record line, as the line
    /// comes before the break it causes, then `tail` (CBACT04C-003, CBACT04C-010).
    fn abend_log([first, second]: [u64; 2], tail: &[&str]) -> Vec<String> {
        let mut log = vec![
            record_shown(first),
            DISCLOSURE_MISSING_LINE.to_owned(),
            TRY_DEFAULT_LINE.to_owned(),
            record_shown(second),
        ];
        log.extend(tail.iter().map(|line| (*line).to_owned()));
        log
    }

    /// The messages of the JSON events `capture` holds, in order.
    fn messages(capture: &TraceCapture) -> Vec<String> {
        capture
            .lines()
            .iter()
            .filter_map(|line| {
                let (_, rest) = line.split_once(r#""message":""#)?;
                let (message, _) = rest.split_once('"')?;
                Some(message.to_owned())
            })
            .collect()
    }

    /// Each abend ends the loop with its error, keeps the rewrites committed before it, the one
    /// made at the failing break included, and leaves no generation file once the output is
    /// dropped. A rewrite that precedes a failed lookup and fails itself is the abend. Its job
    /// log is the first balance's record line and `DEFAULT` fallback pair, the second balance's
    /// record line, as the line comes before the break it causes, then the path's lines, after
    /// the event carrying the error when the rewrite failed as a statement, ending with
    /// `ABENDING PROGRAM`, which records abend code 999.
    // audit: CBACT04C-003
    // audit: CBACT04C-010
    // audit: CBACT04C-005
    // audit: CBACT04C-032
    // audit: CBACT04C-036
    // audit: CBACT04C-038
    #[test]
    fn abends_keep_the_rewrites_made_before_them() -> TestResult {
        block_on(async {
            for Case {
                held,
                failing,
                keys,
                ends,
                kept,
                log,
            } in ABENDS
            {
                let scratch = Scratch::new()?;
                let accounts = held
                    .iter()
                    .map(|&id| account(id, 10_000))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut ledger = Ledger::holding(&accounts, failing);
                let mut run = intcalc()?;
                let mut systran = OutputFile::next_generation(scratch.path(), "SYSTRAN").await?;
                let balances = keys
                    .into_iter()
                    .map(|id| balance(id, 120_000))
                    .collect::<Result<Vec<_>, _>>()?;
                let capture = trace_capture::capture();
                let error =
                    post_interest(&mut run, &balances, &clock()?, &mut ledger, &mut systran)
                        .await
                        .err()
                        .ok_or("the run abends")?;
                let matched = match ends {
                    Ends::RewriteNotFound | Ends::StoreFailure => matches!(
                        &error,
                        JobError::Abend { code: None, reason } if *reason == REWRITE_ACCOUNT_ERROR
                    ),
                    Ends::AccountNotFound => matches!(
                        &error,
                        JobError::Intcalc(IntcalcError::AccountNotFound { .. })
                    ),
                };
                assert!(matched, "{ends:?} ended with {error}");
                assert_eq!(messages(&capture), abend_log(keys, log), "{ends:?}");
                assert!(
                    capture.lines().iter().any(|line| {
                        line.contains(r#""message":"ABENDING PROGRAM""#)
                            && line.contains(r#""abend_code":999"#)
                            && line.contains(r#""level":"ERROR""#)
                    }),
                    "{ends:?}: no ERROR event `ABENDING PROGRAM` records abend code 999"
                );
                drop(capture);
                let kept = kept
                    .iter()
                    .map(|&id| PicU::new(id))
                    .collect::<Result<Vec<PicU<11>>, _>>()?;
                assert_eq!(ledger.rewrites, kept, "{ends:?}");
                drop(systran);
                assert_eq!(fs::read_dir(scratch.path())?.count(), 0, "{ends:?}");
            }
            Ok(())
        })
    }

    /// While another run holds the claim on `accounts`, the `OPEN I-O` of the account file
    /// fails: the job ends with `ERROR OPENING ACCOUNT MASTER FILE` and no return code, reads
    /// and rewrites nothing and creates no generation. Once the claim is given up, the run posts
    /// one run's interest and gives its own claim up at its end.
    // audit: CBACT04C-034
    // audit: CBACT04C-038
    // audit: CBACT04C-039
    #[test]
    fn an_account_file_another_run_holds_ends_the_job_before_any_effect() -> TestResult {
        block_on(async {
            let scratch = Scratch::new()?;
            let guard = posted_database(&scratch.path().join("posttran")).await?;
            let store = PgStore::new(guard.pool().clone());
            let config = var_config(scratch.path())?;
            let clock = clock()?;
            let ctx = JobContext {
                store: &store,
                config: &config,
                clock: &clock,
            };
            let args = IntcalcArgs {
                parm_date: ParmDate::DEFAULT,
            };
            let before = accounts(&store).await?;

            let mut held = store.session().await?;
            assert!(
                held.claim_writer(Dataset::Accounts).await?,
                "the test claims the free account file"
            );
            let error = run(&ctx, &args)
                .await
                .err()
                .ok_or("a run passed an account file another run holds")?;
            assert!(
                matches!(&error, JobError::Abend { code: None, reason } if *reason == OPEN_ACCOUNT_ERROR),
                "{error}"
            );
            assert_eq!(error.exit_code(), None);
            assert_eq!(
                error.to_string(),
                "ABEND: ERROR OPENING ACCOUNT MASTER FILE"
            );
            assert!(
                accounts(&store).await? == before,
                "a refused run rewrote an account"
            );
            assert!(file_names(&config.systran_dir())?.is_empty());
            held.close().await?;

            let once = after_one_run(&store, &scratch.path().join("expected")).await?;
            let outcome = run(&ctx, &args).await?;
            assert_eq!(outcome.return_code, 0);
            assert_eq!(
                outcome.written,
                [config.systran_dir().join("SYSTRAN.G0001")]
            );
            assert!(
                accounts(&store).await? == once,
                "the accounts differ from one run's interest"
            );
            let mut free = store.session().await?;
            assert!(
                free.claim_writer(Dataset::Accounts).await?,
                "a finished run still holds the account file"
            );
            free.close().await?;
            Ok(())
        })
    }

    /// Two runs started together over the same database: one claims the account file and posts
    /// its interest, the other meets that claim before its first read and ends, so every account
    /// holds what one run leaves, as the in-memory table computes it over the same tables, and
    /// one generation exists.
    // audit: CBACT04C-039
    #[test]
    fn of_two_runs_at_once_one_posts_interest_and_the_other_is_refused() -> TestResult {
        block_on(async {
            let scratch = Scratch::new()?;
            let guard = posted_database(&scratch.path().join("posttran")).await?;
            let store = PgStore::new(guard.pool().clone());
            let config = var_config(scratch.path())?;
            let clock = clock()?;
            let ctx = JobContext {
                store: &store,
                config: &config,
                clock: &clock,
            };
            let args = IntcalcArgs {
                parm_date: ParmDate::DEFAULT,
            };
            let once = after_one_run(&store, &scratch.path().join("expected")).await?;

            let (first, second) = join(run(&ctx, &args), run(&ctx, &args)).await;
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
            assert_eq!(posted.return_code, 0);
            assert_eq!(posted.written, [config.systran_dir().join("SYSTRAN.G0001")]);
            assert!(
                matches!(&refused, JobError::Abend { code: None, reason } if *reason == OPEN_ACCOUNT_ERROR),
                "{refused}"
            );
            assert_eq!(file_names(&config.systran_dir())?, ["SYSTRAN.G0001"]);
            assert!(
                accounts(&store).await? == once,
                "the accounts differ from one run's interest"
            );
            Ok(())
        })
    }
}
