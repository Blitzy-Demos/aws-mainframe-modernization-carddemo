//! `carddemo-batch cbpaup0j`: job CBPAUP0J, whose `STEP01` runs the IMS batch message program
//! `CBPAUP0C` under PSB `PSBPAUTB` to purge the expired pending authorizations
//! (`CBPAUP0J.jcl:24-37`; AAP 0.3.4.3). Every rule is `domain::batch::cbpaup0c`'s; this module
//! reads, deletes and commits in the program's order, after the rows of
//! `modern/audit/batch/CBPAUP0J.md`.
//!
//! # Flow
//!
//! 1. The `--sysin` record goes through the domain's total parser, [`params`], before anything
//!    else: `main` calls it right after the command line is parsed, before the configuration is
//!    read or the pool opened, so a refused record ends the run with no SYSOUT line, no database
//!    connection, no table touched and a non-zero exit (CBPAUP0C-002). [`run`] receives the
//!    parameters it accepted and reads today's `YYDDD` once from the injected clock
//!    (CBPAUP0C-001).
//! 2. A transaction is opened and the summaries are read one at a time in ascending `acct_id`,
//!    the root-key order of the unqualified `GN` (CBPAUP0C-009): each is read, with a row lock,
//!    only when the previous one has been processed, as the one after the last `acct_id`
//!    processed, and the read that finds none ends the loop. As `GN` under IMS program isolation
//!    holds the root it read until the sync point, the lock holds until the interval's
//!    checkpoint commit: a CP00 decision for an account the purge has read waits for that
//!    commit, and the purge waits for a decision that holds the next summary, then reads it as
//!    committed, so the delete test always starts from the stored count CP00 last committed.
//! 3. Each summary's details are read in ascending (`auth_date_9c`, `auth_time_9c`) order, the
//!    twin order of `GNP` (CBPAUP0C-010). A detail the domain qualifies is deleted by its key in
//!    the open transaction (CBPAUP0C-011, CBPAUP0C-012, CBPAUP0C-015). After the last detail the
//!    summary is deleted when the domain's duplicated approved-count test holds, and
//!    `ON DELETE CASCADE` removes the details still under it, as IMS deletes a root's dependents
//!    with it (CBPAUP0C-016).
//! 4. A checkpoint is a commit. When the domain's text comparison of the summary count with the
//!    checkpoint frequency holds after a summary, the open transaction is committed and a new one
//!    begun (CBPAUP0C-018, CBPAUP0C-020); after the last summary it is always committed, the final
//!    `9000-TAKE-CHECKPOINT` (CBPAUP0C-019). The run then returns code 0 (CBPAUP0C-022) and writes
//!    no file, so `written` is empty.
//!
//! # No summary write-back (E30)
//!
//! `CBPAUP0C` issues no `REPL`: the subtractions of CBPAUP0C-013 and CBPAUP0C-014 change only the
//! domain's working copy of the summary, so this job never updates a summary row. A summary that
//! survives keeps every stored count and amount, `credit_balance` (`PA-CREDIT-BALANCE`) and
//! `approved_auth_amt` included (CBPAUP0C-017, ARITH-049; E30). The "summary adjustments" of AAP
//! 0.3.4.3 exist only in memory, as the audit's code-governs resolution states. The purge changes
//! authorization state without CP00's credit-limit guard, as audited (AAP 0.3.4.2, "Scope"), and
//! takes no process-local lock.
//!
//! # Abends (CBPAUP0C-023 to CBPAUP0C-028)
//!
//! `9999-ABEND` displays `CBPAUP0C ABENDING ...`, moves 16 to `RETURN-CODE` and ends with
//! `GOBACK` (`CBPAUP0C.cbl:377-383`): a normal end of the BMP, at which IMS commits the deletes
//! since the last checkpoint while the failed command changed nothing (CBPAUP0C-028). A failed
//! statement aborts a `PostgreSQL` transaction, so every read and delete runs inside a savepoint
//! of the open transaction and a failure rolls back to that savepoint alone. The job then commits
//! the open transaction and returns [`JobError::Abend`] with the domain's return code 16
//! (`domain::batch::cbpaup0c::ABEND_RETURN_CODE`). A delete that finds no row is a `DLET` status
//! other than spaces, as IMS `GE` is, and abends the same way (CBPAUP0C-025, CBPAUP0C-026). When
//! the failure is a checkpoint's commit, nothing of the open interval can be committed
//! (CBPAUP0C-027; X-CBPAUP0J-4, X-CBPAUP0J-H5), and the abend's reason says so, as it does when
//! the commit at the abend itself fails.
//!
//! A database call carries no IMS status, so the failure lines show the domain's stand-in for
//! `DIBSTAT` (CBPAUP0C-029, halt H4); the cause is one `ERROR` event naming the DL/I call it
//! replaces and the error's own text, an SQLSTATE, a constraint or a column only.
//!
//! # Halts
//!
//! A `YYDDD` difference beyond ±9999 and a count taken below −9999 have no single COBOL value
//! (halt candidate X-CBPAUP0J-H1, with X-ARITH-6), and the domain answers them with a
//! `PurgeHalt`. The job then commits the open interval, whose deletes the program makes under
//! every `TRUNC` setting, and ends with [`JobError::Abend`] and no return code, so the case is
//! reported blocked rather than decided.
//!
//! # SYSOUT
//!
//! The domain keeps the program's `DISPLAY` lines in program order, with `PARM RECEIVED` masked
//! unless it is the default card or blank (CBPAUP0C-003, X-CBPAUP0J-7). Each line becomes one
//! tracing event whose message is the line's text exactly, `INFO` for a normal run and `ERROR`
//! for the lines an abend adds. The events reach standard output as the JSON stream of
//! `runtime::telemetry`, so the stdout lines that are not JSON stay the written paths alone, of
//! which this job has none (AAP 0.3.4.3, 0.3.4.7).
//!
//! # Rules
//!
//! - Single Schema Executor: no schema work here; the job fails at its first query against a
//!   database whose schema has not been applied.
//! - Cardholder Data Redaction: details carry `PA-CARD-NUM`, and nothing here formats a record or
//!   a key. The SYSOUT lines are the domain's, which show counts, dates and `PA-ACCT-ID` only,
//!   and every [`JobError`] reason is static text.
//! - R4: every statement is a repository call of `persistence`, on the job's transaction.

use copybook::PicS;
use domain::batch::cbpaup0c::{
    CheckpointAction, DetailAction, Purge, PurgeFailure, PurgeHalt, PurgeParams, SummaryAction,
    SummaryDisposition, SysinError,
};
use domain::records::{PendingAuthDetailRecord, PendingAuthSummaryRecord};
use persistence::{
    PendingAuthDetailRepository, PendingAuthSummaryRepository, PersistenceError, PgTx,
};
use runtime::telemetry::JobCounter;

use super::{JobContext, JobError, JobOutcome};
use crate::cli::Cbpaup0jArgs;

/// The reason of an abend whose deletes since the last checkpoint stand: the `DISPLAY` of
/// `9999-ABEND` (`CBPAUP0C.cbl:380`; CBPAUP0C-028).
const ABENDING: &str = "CBPAUP0C ABENDING ...";

/// The reason of an abend whose open checkpoint interval could not be committed, because the
/// failure was that commit or the commit at the abend failed too (CBPAUP0C-027, CBPAUP0C-028;
/// X-CBPAUP0J-4).
const ABENDING_UNCOMMITTED: &str =
    "CBPAUP0C ABENDING ... (the deletes since the last checkpoint are not committed)";

// The DL/I call each database call replaces, named in the event that reports its failure.

/// `2000-FIND-NEXT-AUTH-SUMMARY` (CBPAUP0C-009, CBPAUP0C-023).
const GN_SUMMARY: &str = "GN PAUTSUM0";
/// `3000-FIND-NEXT-AUTH-DTL` (CBPAUP0C-010, CBPAUP0C-024).
const GNP_DETAIL: &str = "GNP PAUTDTL1";
/// `5000-DELETE-AUTH-DTL` (CBPAUP0C-015, CBPAUP0C-025).
const DLET_DETAIL: &str = "DLET PAUTDTL1";
/// `6000-DELETE-AUTH-SUMMARY` (CBPAUP0C-016, CBPAUP0C-026).
const DLET_SUMMARY: &str = "DLET PAUTSUM0";
/// `9000-TAKE-CHECKPOINT`, and the commit at the end of an abended run (CBPAUP0C-018 to
/// CBPAUP0C-020, CBPAUP0C-027, CBPAUP0C-028).
const CHKP: &str = "CHKP";
/// The transaction that opens a checkpoint interval, which IMS starts implicitly: its failure is
/// the failure of the interval's first call, a `GN` (CBPAUP0C-023).
const INTERVAL_START: &str = "start of a checkpoint interval";

/// The run parameters of the `--sysin` record of `args`, through the domain's total parser
/// (CBPAUP0C-002). `main` calls it right after the command line is parsed, before it reads the
/// configuration or opens the pool, and hands the parameters to [`run`].
///
/// # Errors
///
/// [`JobError::Abend`] with no code when the record is longer than the 80-byte card or holds a
/// byte outside printable ASCII. Its reason is static text that shows neither the record nor the
/// length or offset the domain error carries.
pub(crate) fn params(args: &Cbpaup0jArgs) -> Result<PurgeParams, JobError> {
    args.purge_params().map_err(sysin_refused)
}

/// Runs CBPAUP0C over `pending_auth_summaries` and `pending_auth_details` with `params`, the
/// SYSIN record [`params`] accepted.
///
/// # Errors
///
/// - [`JobError::Abend`] with code 16 when a read, a delete or a commit fails (CBPAUP0C-023 to
///   CBPAUP0C-028), after the deletes that succeeded since the last checkpoint are committed.
/// - [`JobError::Abend`] with no code when the domain halts a detail (X-CBPAUP0J-H1).
pub(crate) async fn run(ctx: &JobContext<'_>, params: PurgeParams) -> Result<JobOutcome, JobError> {
    let mut purge = Purge::new(params, ctx.clock);
    match purge_all(ctx, &mut purge).await {
        Ok(()) => Ok(finished(purge)),
        Err(stopped) => Err(abended(purge, stopped)),
    }
}

/// Why the summary loop ended before the end of the database.
#[derive(Clone, Copy)]
enum Stop {
    /// A read, a delete or a commit failed: `9999-ABEND` (CBPAUP0C-023 to CBPAUP0C-027).
    Failed(PurgeFailure),
    /// The domain cannot decide a detail (X-CBPAUP0J-H1).
    Halted(PurgeHalt),
}

/// How the summary loop ended early, and whether every delete that succeeded is committed.
#[derive(Clone, Copy)]
struct Stopped {
    /// What ended the loop.
    stop: Stop,
    /// `false` when the open checkpoint interval could not be committed.
    committed: bool,
}

impl Stopped {
    /// A stop with no interval open, after a commit that succeeded.
    const fn closed(stop: Stop) -> Self {
        Self {
            stop,
            committed: true,
        }
    }

    /// A stop inside the open interval `tx`, which is committed now, as IMS commits at the
    /// program's normal end: the statement that failed was rolled back to its savepoint, so the
    /// deletes that succeeded stand (CBPAUP0C-028).
    async fn in_interval(stop: Stop, tx: PgTx<'_>) -> Self {
        let committed = match tx.commit().await {
            Ok(()) => true,
            Err(error) => {
                report(CHKP, &error);
                false
            }
        };
        Self { stop, committed }
    }
}

/// `MAIN-PARA` from the first summary read to the final checkpoint (CBPAUP0C-009 to
/// CBPAUP0C-019). Each summary is read, and locked, only when the previous one has been
/// processed: the one after the last `acct_id` processed, in root-key order, until the read finds
/// none, the `GB` that ends the loop. After an in-loop checkpoint the run goes on with the summary
/// after the last one processed, which is not claimed as parity (X-CBPAUP0J-H5).
async fn purge_all(ctx: &JobContext<'_>, purge: &mut Purge) -> Result<(), Stopped> {
    let Some(mut tx) = begin(ctx).await else {
        return Err(Stopped::closed(Stop::Failed(PurgeFailure::SummaryRead)));
    };
    let mut after = None;
    loop {
        let Some(read) = read_next_summary(&mut tx, after).await else {
            let stop = Stop::Failed(PurgeFailure::SummaryRead);
            return Err(Stopped::in_interval(stop, tx).await);
        };
        let Some(summary) = read else {
            break;
        };
        let action = match purge_summary(&mut tx, purge, &summary).await {
            Ok(action) => action,
            Err(stop) => return Err(Stopped::in_interval(stop, tx).await),
        };
        after = Some(summary.acct_id);
        if action.checkpoint == CheckpointAction::Take {
            commit(tx, PurgeFailure::Checkpoint).await?;
            purge.checkpoint_taken();
            // The interval's first call is the next `GN`, so a transaction that cannot start is
            // that read failing, with every delete so far committed (CBPAUP0C-023).
            let Some(next) = begin(ctx).await else {
                return Err(Stopped::closed(Stop::Failed(PurgeFailure::SummaryRead)));
            };
            tx = next;
        }
    }
    commit(tx, PurgeFailure::FinalCheckpoint).await
}

/// One summary, from its details to its delete and checkpoint tests (`CBPAUP0C.cbl:144-164`):
/// the details the domain qualifies are deleted, then the summary when the domain says so. The
/// domain's working copy of the summary is never written back (CBPAUP0C-017; E30).
///
/// # Errors
///
/// [`Stop::Failed`] with the DL/I call that failed, and [`Stop::Halted`] when the domain halts a
/// detail. The statement that failed was rolled back to its savepoint.
async fn purge_summary(
    tx: &mut PgTx<'_>,
    purge: &mut Purge,
    summary: &PendingAuthSummaryRecord,
) -> Result<SummaryAction, Stop> {
    let acct_id = summary.acct_id;
    let mut pass = purge.begin_summary(summary);
    let Some(details) = read_details(tx, acct_id).await else {
        return Err(Stop::Failed(PurgeFailure::DetailRead));
    };
    for detail in &details {
        if pass.on_detail(detail).map_err(Stop::Halted)? == DetailAction::Delete
            && !deleted(delete_detail(tx, acct_id, detail).await, DLET_DETAIL)
        {
            return Err(Stop::Failed(PurgeFailure::DetailDelete));
        }
    }
    let action = pass.end_summary();
    if action.delete_summary == SummaryDisposition::Delete
        && !deleted(delete_summary(tx, acct_id).await, DLET_SUMMARY)
    {
        return Err(Stop::Failed(PurgeFailure::SummaryDelete));
    }
    Ok(action)
}

/// Opens the transaction of a checkpoint interval; `None`, after reporting the cause, when it
/// cannot start.
async fn begin(ctx: &JobContext<'_>) -> Option<PgTx<'static>> {
    match ctx.store.begin().await {
        Ok(tx) => Some(tx),
        Err(error) => {
            report(INTERVAL_START, &error);
            None
        }
    }
}

/// Commits the open interval: a checkpoint (CBPAUP0C-018, CBPAUP0C-019).
///
/// # Errors
///
/// A [`Stopped`] with `failure` when the commit fails, which rolls the interval back: nothing of
/// it is committed (CBPAUP0C-027; X-CBPAUP0J-4).
async fn commit(tx: PgTx<'_>, failure: PurgeFailure) -> Result<(), Stopped> {
    tx.commit().await.map_err(|error| {
        report(CHKP, &error);
        Stopped {
            stop: Stop::Failed(failure),
            committed: false,
        }
    })
}

/// The summary after account `after` in ascending `acct_id`, or the first when `after` is
/// `None`, read and locked in a savepoint of `tx` when the purge reaches it, as the unqualified
/// `GN` reads the next root (CBPAUP0C-009). A summary CP00 holds is waited for and read as CP00
/// committed it. The released savepoint leaves the lock with `tx`, so it holds until the
/// interval's checkpoint commit, as IMS program isolation holds a root until the sync point.
/// `Some(None)` is the end of the database, IMS `GB`; `None` when the read fails
/// (CBPAUP0C-023).
async fn read_next_summary(
    tx: &mut PgTx<'_>,
    after: Option<PicS<11>>,
) -> Option<Option<PendingAuthSummaryRecord>> {
    let mut point = savepoint(tx, GN_SUMMARY).await?;
    let result = PendingAuthSummaryRepository::lock_next(&mut point, after).await;
    settle(point, result, GN_SUMMARY).await
}

/// The details of account `acct_id` in key order, read in a savepoint of `tx` (CBPAUP0C-010);
/// `None` when the read fails (CBPAUP0C-024).
async fn read_details(
    tx: &mut PgTx<'_>,
    acct_id: PicS<11>,
) -> Option<Vec<PendingAuthDetailRecord>> {
    let mut point = savepoint(tx, GNP_DETAIL).await?;
    let result = PendingAuthDetailRepository::find_all_by_acct_id(&mut point, acct_id).await;
    settle(point, result, GNP_DETAIL).await
}

/// Deletes `detail` of account `acct_id` by its key in a savepoint of `tx` (CBPAUP0C-015). The
/// key's account comes from the summary, as the segment holds none (CIPAUDTY-029).
async fn delete_detail(
    tx: &mut PgTx<'_>,
    acct_id: PicS<11>,
    detail: &PendingAuthDetailRecord,
) -> Option<bool> {
    let mut point = savepoint(tx, DLET_DETAIL).await?;
    let result = PendingAuthDetailRepository::delete(
        &mut point,
        acct_id,
        detail.auth_date_9c,
        detail.auth_time_9c,
    )
    .await;
    settle(point, result, DLET_DETAIL).await
}

/// Deletes the summary of account `acct_id`, and through `ON DELETE CASCADE` the details still
/// under it, in a savepoint of `tx` (CBPAUP0C-016).
async fn delete_summary(tx: &mut PgTx<'_>, acct_id: PicS<11>) -> Option<bool> {
    let mut point = savepoint(tx, DLET_SUMMARY).await?;
    let result = PendingAuthSummaryRepository::delete(&mut point, acct_id).await;
    settle(point, result, DLET_SUMMARY).await
}

/// Opens a savepoint of `tx` for one call; `None`, after reporting the cause, when it cannot.
async fn savepoint<'t>(tx: &'t mut PgTx<'_>, operation: &'static str) -> Option<PgTx<'t>> {
    match tx.savepoint().await {
        Ok(point) => Some(point),
        Err(error) => {
            report(operation, &error);
            None
        }
    }
}

/// Ends the savepoint `point` of one call by its `result`: released when the call succeeded, so
/// its work joins the interval, and rolled back when it failed, so the interval stays committable
/// (CBPAUP0C-028). `None` when the call or the release failed.
async fn settle<T>(
    point: PgTx<'_>,
    result: Result<T, PersistenceError>,
    operation: &'static str,
) -> Option<T> {
    match result {
        Ok(value) => match point.commit().await {
            Ok(()) => Some(value),
            Err(error) => {
                report(operation, &error);
                None
            }
        },
        Err(error) => {
            report(operation, &error);
            // A rollback that fails too means the connection is gone; the interval's commit
            // then fails and the abend's reason says the interval is not committed.
            if let Err(rollback) = point.rollback().await {
                report(operation, &rollback);
            }
            None
        }
    }
}

/// Whether a `DLET` ended with status spaces. A delete that found no row is a status other than
/// spaces, as IMS `GE` is, so it fails the call like an error does (CBPAUP0C-025,
/// CBPAUP0C-026).
fn deleted(outcome: Option<bool>, operation: &'static str) -> bool {
    match outcome {
        Some(true) => true,
        Some(false) => {
            tracing::error!(operation, "CBPAUP0C delete found no row");
            false
        }
        None => false,
    }
}

/// Reports why a database call failed, for the operator: the DL/I call it replaces and the
/// error's text, which names an SQLSTATE, a constraint or a column only (Cardholder Data
/// Redaction).
fn report(operation: &'static str, error: &PersistenceError) {
    tracing::error!(operation, error = %error, "CBPAUP0C database call failed");
}

/// The normal end: the final checkpoint, the end lines and return code 0 (CBPAUP0C-019,
/// CBPAUP0C-021, CBPAUP0C-022). The counters are the program's four totals and the commits taken.
fn finished(purge: Purge) -> JobOutcome {
    let report = purge.finish();
    display(report.sysout(), usize::MAX);
    JobOutcome {
        written: Vec::new(),
        return_code: report.return_code(),
        counts: vec![
            (JobCounter::SummariesRead, report.summaries_read()),
            (JobCounter::SummariesDeleted, report.summaries_deleted()),
            (JobCounter::DetailsRead, report.details_read()),
            (JobCounter::DetailsDeleted, report.details_deleted()),
            (JobCounter::Checkpoints, report.checkpoints()),
        ],
    }
}

/// The early end: `9999-ABEND` with its failure lines and return code 16 (CBPAUP0C-023 to
/// CBPAUP0C-028), or a halted detail, which shows no further line and carries no return code.
fn abended(purge: Purge, stopped: Stopped) -> JobError {
    match stopped.stop {
        Stop::Failed(failure) => {
            let shown = purge.sysout().len();
            let report = purge.abend(failure);
            display(report.sysout(), shown);
            JobError::Abend {
                code: Some(report.return_code()),
                reason: if stopped.committed {
                    ABENDING
                } else {
                    ABENDING_UNCOMMITTED
                },
            }
        }
        Stop::Halted(halt) => {
            display(purge.sysout(), usize::MAX);
            JobError::Abend {
                code: None,
                reason: halted_reason(halt, stopped.committed),
            }
        }
    }
}

// The `DISPLAY` lines become tracing events, not plain standard-output lines as
// `CBPAUP0J.md` words it: AAP 0.3.4.3 keeps the plain stdout lines for written paths, and the
// POSTTRAN and INTCALC audits map their `DISPLAY` lines to events the same way, which keeps the
// audited text and order on standard output without mixing it with paths.
/// Shows the program's SYSOUT lines in program order, one event each whose message is the line's
/// text exactly: `INFO` before index `abend_from` and `ERROR` from it on, the lines an abend adds.
fn display(lines: &[String], abend_from: usize) {
    for (index, line) in lines.iter().enumerate() {
        if index < abend_from {
            tracing::info!("{line}");
        } else {
            tracing::error!("{line}");
        }
    }
}

// `JobError` has no SYSIN variant; the refusal is the audited path of CBPAUP0C-002, so it is an
// `Abend` without a return code, and its fixed text keeps the length and offset the domain error
// names out of the job log as well as the record itself.
/// The error of a SYSIN record the domain refuses (CBPAUP0C-002): static text naming the class of
/// the refusal, never a byte of the record, which may hold a protected value.
fn sysin_refused(error: SysinError) -> JobError {
    JobError::Abend {
        code: None,
        reason: match error {
            SysinError::TooLong { .. } => "SYSIN record refused: longer than the 80-byte card",
            SysinError::NotPrintable { .. } => {
                "SYSIN record refused: a byte outside printable ASCII"
            }
        },
    }
}

/// The reason of a run the domain halted, and whether the open interval was committed.
const fn halted_reason(halt: PurgeHalt, committed: bool) -> &'static str {
    match (halt, committed) {
        (PurgeHalt::DayDifference, true) => {
            "BLOCKED (X-CBPAUP0J-H1): a YYDDD day difference beyond the four digits of \
             WS-DAY-DIFF has no single value"
        }
        (PurgeHalt::DayDifference, false) => {
            "BLOCKED (X-CBPAUP0J-H1): a YYDDD day difference beyond the four digits of \
             WS-DAY-DIFF has no single value; the deletes since the last checkpoint are not \
             committed"
        }
        (PurgeHalt::AuthorizationCount, true) => {
            "BLOCKED (X-ARITH-6): an authorization count below the four digits of its field has \
             no single value"
        }
        (PurgeHalt::AuthorizationCount, false) => {
            "BLOCKED (X-ARITH-6): an authorization count below the four digits of its field has \
             no single value; the deletes since the last checkpoint are not committed"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::pin::pin;
    use std::time::Duration;

    use chrono::NaiveDate;
    use copybook::{Count, PicS, PicU, PicX};
    use domain::batch::cbpaup0c::{Purge, PurgeHalt, PurgeParams, SysinError};
    use domain::clock::FixedClock;
    use domain::money::{Decimal, Money};
    use domain::records::{PendingAuthDetailRecord, PendingAuthSummaryRecord};
    use domain::sensitive::CardNumber;
    use persistence::{PendingAuthDetailRepository, PendingAuthSummaryRepository, PgStore, PgTx};
    use runtime::config::Config;
    use runtime::telemetry::JobCounter;
    use tokio::time;

    use super::{
        ABENDING, ABENDING_UNCOMMITTED, DLET_DETAIL, JobError, deleted, halted_reason, run,
        sysin_refused,
    };
    use crate::jobs::JobContext;

    type TestResult = Result<(), Box<dyn Error>>;

    /// How long the run is watched before the test treats it as waiting for a row lock.
    const LOCK_WAIT: Duration = Duration::from_millis(500);

    /// The run's SYSIN: details expire after one day, the default frequencies, debug off.
    const SYSIN: &[u8] = b"01,00001,00001,N";

    /// The account below the decided one, whose summary the purge reads and deletes first.
    const PURGED: i64 = 7;

    /// The account CP00 approves while the purge runs.
    const DECIDED: i64 = 49;

    /// Noon of 2022-07-18, the run's today.
    fn clock() -> Result<FixedClock, Box<dyn Error>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_opt(12, 0, 0))
            .ok_or("2022-07-18 12:00:00 is a valid instant")?;
        Ok(FixedClock::new(at))
    }

    /// The summary of account `acct` as CP00 creates it (COPAUA0C-092), with `declined`
    /// declines totalling `declined_cents` hundredths and no approval.
    fn summary(
        acct: i64,
        declined: i16,
        declined_cents: i64,
    ) -> Result<PendingAuthSummaryRecord, Box<dyn Error>> {
        let mut record = PendingAuthSummaryRecord::initialized(PicS::new(acct)?, PicU::new(50)?);
        record.declined_auth_cnt = Count::new(declined);
        record.declined_auth_amt = Money::new(Decimal::new(declined_cents, 2))?;
        Ok(record)
    }

    /// A detail of day `yyddd` at `hhmmssmmm` for `cents` hundredths, keyed as CP00 keys it
    /// (COPAUA0C-098): an approval `00`, or a decline `05`/`4100` with no approved amount. The
    /// card number is blank, as the purge never reads it.
    fn detail(
        yyddd: u32,
        hhmmssmmm: u64,
        approved: bool,
        cents: i64,
    ) -> Result<PendingAuthDetailRecord, Box<dyn Error>> {
        let amount = Money::new(Decimal::new(cents, 2))?;
        let (resp_code, resp_reason, approved_amt) = if approved {
            (*b"00", *b"0000", amount)
        } else {
            (*b"05", *b"4100", Money::ZERO)
        };
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::complement_from(u64::from(yyddd))?,
            auth_time_9c: PicS::complement_from(hhmmssmmm)?,
            orig_date: PicX::from_array(*b"220718"),
            orig_time: PicX::spaces(),
            card_num: CardNumber::new(PicX::spaces()),
            auth_type: PicX::spaces(),
            card_expiry_date: PicX::spaces(),
            message_type: PicX::spaces(),
            message_source: PicX::spaces(),
            auth_id_code: PicX::spaces(),
            auth_resp_code: PicX::from_array(resp_code),
            auth_resp_reason: PicX::from_array(resp_reason),
            processing_code: PicU::new(0)?,
            transaction_amt: amount,
            approved_amt,
            merchant_catagory_code: PicX::spaces(),
            acqr_country_code: PicX::spaces(),
            pos_entry_mode: PicU::new(0)?,
            merchant_id: PicX::spaces(),
            merchant_name: PicX::spaces(),
            merchant_city: PicX::spaces(),
            merchant_state: PicX::spaces(),
            merchant_zip: PicX::spaces(),
            transaction_id: PicX::spaces(),
            match_status: PicX::from_array(*b"P"),
            auth_fraud: PicX::spaces(),
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// Seeds `PURGED`, with no approval and no detail, and `DECIDED` as CP00 left it after a
    /// `05`/`4100` reply of today: declined only, with that decline's detail. Returns
    /// `DECIDED`'s summary and detail.
    async fn seed(
        store: &PgStore,
        today: u32,
    ) -> Result<(PendingAuthSummaryRecord, PendingAuthDetailRecord), Box<dyn Error>> {
        let mut store = store.clone();
        PendingAuthSummaryRepository::insert(&mut store, &summary(PURGED, 0, 0)?).await?;
        let declined = summary(DECIDED, 1, 9_999_900)?;
        PendingAuthSummaryRepository::insert(&mut store, &declined).await?;
        let decline = detail(today, 120_000_000, false, 9_999_900)?;
        PendingAuthDetailRepository::insert(&mut store, declined.acct_id, &decline).await?;
        Ok((declined, decline))
    }

    /// CP00's approval of 1.00 a minute after the decline, on `cp00`, which holds the lock of
    /// `locked`: the summary write, then the detail insert (COPAUA0C-097). Returns what it
    /// wrote.
    async fn approve(
        cp00: &mut PgTx<'_>,
        locked: &PendingAuthSummaryRecord,
        today: u32,
    ) -> Result<(PendingAuthSummaryRecord, PendingAuthDetailRecord), Box<dyn Error>> {
        let mut approved = locked.clone();
        approved.approved_auth_cnt = Count::new(1);
        approved.approved_auth_amt = Money::new(Decimal::new(100, 2))?;
        approved.credit_balance = Money::new(Decimal::new(100, 2))?;
        assert!(PendingAuthSummaryRepository::update(&mut *cp00, &approved).await?);
        let approval = detail(today, 120_100_000, true, 100)?;
        PendingAuthDetailRepository::insert(&mut *cp00, approved.acct_id, &approval).await?;
        Ok((approved, approval))
    }

    /// The purge reads each summary when it reaches it and locks it, so an approval CP00
    /// commits while the purge waits for that summary is in the count its delete test uses:
    /// the summary was declined only when the run started and is kept, column for column as
    /// CP00 committed it and with both of today's details, while the summary read before it is
    /// deleted. A copy of every summary read at the start would have deleted it.
    // audit: CBPAUP0C-009
    // audit: CBPAUP0C-016
    // audit: CBPAUP0C-017
    #[tokio::test]
    async fn an_approval_committed_while_the_purge_waits_keeps_its_summary() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let config = Config::from_lookup(|_| None)?;
        let clock = clock()?;
        let ctx = JobContext {
            store: &store,
            config: &config,
            clock: &clock,
        };
        let params = PurgeParams::parse(SYSIN)?;
        let today = Purge::new(params.clone(), &clock).today();
        let (declined, decline) = seed(&store, today).await?;

        let mut cp00 = store.begin().await?;
        let (locked, created) = cp00.lock_or_create(&declined).await?;
        assert_eq!((&locked, created), (&declined, false));
        let mut purge = pin!(run(&ctx, params));
        assert!(
            time::timeout(LOCK_WAIT, &mut purge).await.is_err(),
            "the purge waits for the decision that holds the summary"
        );
        let (approved, approval) = approve(&mut cp00, &locked, today).await?;
        cp00.commit().await?;
        let outcome = purge.await?;

        assert_eq!(outcome.return_code, 0);
        assert_eq!(
            outcome.counts,
            [
                (JobCounter::SummariesRead, 2),
                (JobCounter::SummariesDeleted, 1),
                (JobCounter::DetailsRead, 2),
                (JobCounter::DetailsDeleted, 0),
                (JobCounter::Checkpoints, 1),
            ]
        );
        let mut reader = store.clone();
        let kept = PendingAuthSummaryRepository::find_by_id(&mut reader, approved.acct_id).await?;
        assert_eq!(kept, Some(approved.clone()));
        let purged = PendingAuthSummaryRepository::find_by_id(&mut reader, PicS::new(PURGED)?);
        assert_eq!(purged.await?, None);
        let details = reader.find_all_by_acct_id(approved.acct_id).await?;
        assert_eq!(details, [approval, decline]);
        Ok(())
    }

    /// A refused SYSIN record ends the job without a return code, so the exit is non-zero, and
    /// its text shows neither the record nor the length or offset the domain error carries
    /// (CBPAUP0C-002).
    #[test]
    fn a_refused_sysin_is_an_abend_without_code_or_record_bytes() {
        let cases = [
            (
                SysinError::TooLong { len: 81 },
                "ABEND: SYSIN record refused: longer than the 80-byte card",
                "81",
            ),
            (
                SysinError::NotPrintable { offset: 3 },
                "ABEND: SYSIN record refused: a byte outside printable ASCII",
                "3",
            ),
        ];
        for (error, text, hidden) in cases {
            let job_error = sysin_refused(error);
            assert_eq!(job_error.exit_code(), None);
            let shown = job_error.to_string();
            assert_eq!(shown, text);
            assert!(!shown.contains(hidden), "{shown}");
        }
    }

    /// The two abend reasons keep the `9999-ABEND` text and carry return code 16 once wrapped
    /// (CBPAUP0C-028).
    #[test]
    fn abend_reasons_start_with_the_program_display() {
        for reason in [ABENDING, ABENDING_UNCOMMITTED] {
            assert!(reason.starts_with("CBPAUP0C ABENDING ..."), "{reason}");
            let error = JobError::Abend {
                code: Some(16),
                reason,
            };
            assert_eq!(error.exit_code(), Some(16));
        }
        assert_eq!(ABENDING, "CBPAUP0C ABENDING ...");
        assert!(ABENDING_UNCOMMITTED.ends_with("not committed)"));
    }

    /// Each halt names its candidate, and the uncommitted form says so.
    #[test]
    fn halted_reasons_name_the_candidate_and_the_commit() {
        let cases = [
            (PurgeHalt::DayDifference, "X-CBPAUP0J-H1"),
            (PurgeHalt::AuthorizationCount, "X-ARITH-6"),
        ];
        for (halt, candidate) in cases {
            let committed = halted_reason(halt, true);
            let lost = halted_reason(halt, false);
            assert!(committed.starts_with("BLOCKED (") && committed.contains(candidate));
            assert!(!committed.contains("not committed"), "{committed}");
            assert!(lost.starts_with(committed), "{lost}");
            assert!(lost.ends_with("are not committed"), "{lost}");
        }
    }

    /// Only a delete that removed its row ends with status spaces (CBPAUP0C-025,
    /// CBPAUP0C-026).
    #[test]
    fn only_a_removed_row_is_a_successful_delete() {
        assert!(deleted(Some(true), DLET_DETAIL));
        assert!(!deleted(Some(false), DLET_DETAIL));
        assert!(!deleted(None, DLET_DETAIL));
    }
}
