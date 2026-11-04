//! `batch tranbkp`: the TRANBKP job [app/jcl/TRANBKP.jcl], its steps run in their JCL order.
//!
//! - `STEP05R` (P-TRANBKP-001, P-TRANBKP-002): `EXEC PROC=REPROC`, the `REPRO` of
//!   `TRANSACT.VSAM.KSDS` into `TRANSACT.BKUP(+1)` [app/jcl/TRANBKP.jcl:L23-33]
//!   [app/proc/REPROC.prc:L21-28], through [`backup_live_master_in`].
//! - `STEP05` (P-TRANBKP-003, P-TRANBKP-005): `IDCAMS DELETE` of the cluster and of its
//!   alternate index, each followed by `IF MAXCC LE 08 THEN SET MAXCC = 0`, with no `COND`
//!   [app/jcl/TRANBKP.jcl:L37-46]. The live transaction master is emptied.
//! - `STEP10` (P-TRANBKP-004): `IDCAMS DEFINE CLUSTER … KEYS(16 0) RECORDSIZE(350 350)` with
//!   `COND=(4,LT)` [app/jcl/TRANBKP.jcl:L51-67]. Nothing further is written: the emptied live
//!   master is the empty cluster.
//!
//! The live transaction master is the set of rows flagged as in the master (E-09). Emptying it
//! takes every row out of the master and deletes the rows outside it that no reward accrual
//! references, so accrued transactions stay the targets of their accruals and the reward
//! history survives the emptied file.
//!
//! All three steps run in one unit of work that first excludes every writer of the live master
//! (`PgStore::exclude_master_writers`), as the closed CICS files do on the mainframe. A CT02 or
//! CB00 add, a POSTTRAN posting or a COMBTRAN load already in flight is waited for, so its
//! records are backed up; one that starts later waits until the emptied master is committed.
//! The backup is cataloged before any record leaves the master, and the records emptied are
//! exactly the records backed up: no transaction is retired, or deleted, without its backup.
//!
//! The job ends with the condition code of `STEP05R`, the highest of its steps: 0 when the live
//! master held records, 12 when it was empty (P-TRANBKP-006, E-37).

use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;

use super::{Backup, JobContext, backup_live_master_in, count_of, setup_failure, span_count};
use crate::exit::ExitStatus;

/// The highest condition code of the earlier steps at which `STEP10` still runs: `COND=(4,LT)`
/// bypasses the step as soon as 4 is less than the code of any step before it
/// [app/jcl/TRANBKP.jcl:L51] (P-TRANBKP-004).
const STEP10_COND_LIMIT: i32 = 4;

/// What the failure log of the job's unit of work names when it cannot begin or cannot exclude
/// the writers of the live master.
const EXCLUSION_FAILED: &str = "live transaction master writer exclusion failed";

/// What the failure log of `STEP05` names, for each of its database calls.
const EMPTYING_FAILED: &str = "live transaction master emptying failed";

/// Runs TRANBKP: `STEP05R`, then `STEP05` and `STEP10`, all in one unit of work that excludes
/// every writer of the live master, and returns the condition code of the job.
pub(super) async fn run(ctx: &JobContext, db: &Db) -> ExitStatus {
    let mut uow = match begin_excluding_writers(db).await {
        Ok(uow) => uow,
        Err(status) => return status,
    };
    let backup = match step05r_backup(ctx, &mut uow).await {
        Ok(backup) => backup,
        // Halt. The audit records a condition code for an empty input only (P-TRANBKP-002); a
        // REPRO that fails to allocate, read, write or catalog has none, so the outcome of the
        // later steps is not proven. STEP05 has no COND and the COBOL would still delete the
        // cluster; the job instead stops with the backup's setup failure (exit 16) and the live
        // transaction master keeps every record: nothing is emptied without its backup. The
        // unit of work is dropped uncommitted, which rolls it back and releases its locks.
        Err(status) => return status,
    };
    let status = backup.status();
    if let Err(failure) = step05_step10_empty_master(uow, step10_runs(status.code())).await {
        return failure;
    }
    status
}

/// TRANBKP `STEP05R` to `STEP10` [app/jcl/TRANBKP.jcl:L23-67] — P-TRANBKP-001, P-TRANBKP-003,
/// P-TRANBKP-005: begins the one unit of work TRANBKP runs in and holds the live transaction
/// master still in it with `PgStore::exclude_master_writers`. From here until the commit of
/// `STEP05`, every writer of the master waits and every read the job makes sees the same
/// committed records.
///
/// # Errors
///
/// A setup failure (exit 16) when the unit of work cannot begin or the locks cannot be taken:
/// the audit records no condition code for either. Nothing has been backed up or emptied, and a
/// unit of work already begun is dropped uncommitted, which rolls it back.
async fn begin_excluding_writers(db: &Db) -> Result<UnitOfWork, ExitStatus> {
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure(EXCLUSION_FAILED, err))?;
    uow.store()
        .exclude_master_writers()
        .await
        .map_err(|err| setup_failure(EXCLUSION_FAILED, err))?;
    Ok(uow)
}

/// TRANBKP `STEP05R` — P-TRANBKP-001, P-TRANBKP-002, P-TRANBKP-006: `EXEC PROC=REPROC` with
/// `PRC001.FILEIN` `TRANSACT.VSAM.KSDS` and `PRC001.FILEOUT` `TRANSACT.BKUP(+1)`
/// `DISP=(NEW,CATLG,DELETE)`, `LRECL=350` [app/jcl/TRANBKP.jcl:L23-33]
/// [app/proc/REPROC.prc:L21-28], through the REPROC step TRANBKP shares with TRANREPT, read in
/// the job's writer-excluding unit of work `uow`.
///
/// Returns the cataloged backup. Its [`Backup::status`] is the step's condition code: 0, or 12
/// when the live transaction master was empty and the backup was cataloged empty (E-37).
///
/// Once the backup is cataloged, `read` in the job's counts is set to the live records `REPRO`
/// read, as many as it wrote (`written`, recorded on the job span by the shared REPROC step).
/// `posted` and `rejected` stay 0: TRANBKP loads no record and turns none away. A backup that
/// fails is discarded uncataloged and leaves `read` 0: no live record was backed up.
///
/// # Errors
///
/// The setup failure (exit 16) of [`backup_live_master_in`] when the live transaction master
/// cannot be read or the generation cannot be allocated, written or cataloged.
async fn step05r_backup(ctx: &JobContext, uow: &mut UnitOfWork) -> Result<Backup, ExitStatus> {
    let backup = backup_live_master_in(ctx, uow).await?;
    ctx.counts.set_read(count_of(backup.records));
    tracing::info!(
        step = "STEP05R",
        generation = %backup.generation,
        path = %backup.generation.path.display(),
        condition_code = backup.status().code(),
        "TRANSACT.BKUP(+1) step ended"
    );
    Ok(backup)
}

/// Whether `STEP10` runs after earlier steps whose highest condition code is
/// `prior_condition_code`: `COND=(4,LT)` [app/jcl/TRANBKP.jcl:L51] (P-TRANBKP-004).
///
/// `STEP05` always ends with 0, because `IF MAXCC LE 08 THEN SET MAXCC = 0` follows each of its
/// `DELETE`s (P-TRANBKP-003), so only the code of `STEP05R` decides.
const fn step10_runs(prior_condition_code: i32) -> bool {
    prior_condition_code <= STEP10_COND_LIMIT
}

/// TRANBKP `STEP05` and `STEP10` — P-TRANBKP-003, P-TRANBKP-004, P-TRANBKP-005: the `DELETE` of
/// `TRANSACT.VSAM.KSDS` and `TRANSACT.VSAM.AIX` [app/jcl/TRANBKP.jcl:L37-46], then the `DEFINE`
/// of the empty cluster [app/jcl/TRANBKP.jcl:L51-67], as the E-09 representation performs them.
///
/// In the job's unit of work `uow`, which excluded every writer before `STEP05R` read the
/// backup, every row is taken out of the live transaction master and the rows outside it that
/// no reward accrual references are deleted; then the unit commits, which releases the writers.
/// The rows taken out are therefore exactly the rows the cataloged backup holds. The deleted-row
/// count is recorded in `deleted` on the job span, and one event reports both counts and whether
/// `STEP10` ran.
///
/// `STEP05` has no `COND`, so this runs after an empty backup too. `STEP10` is then bypassed and
/// the COBOL leaves the cluster undefined; a live master cannot be left undefined, so it stays
/// empty, which is what the `DEFINE` would have given (E-37).
///
/// # Errors
///
/// A setup failure (exit 16) when the emptying fails or the commit fails: the audit records no
/// condition code for a failed `DELETE` beyond the `MAXCC` reset of a missing data set. The unit
/// of work is then dropped uncommitted, which rolls it back, so the live master is unchanged and
/// the cataloged backup stays as written.
async fn step05_step10_empty_master(
    mut uow: UnitOfWork,
    step10_runs: bool,
) -> Result<(), ExitStatus> {
    let (demoted, deleted) = uow
        .store()
        .empty_live_master()
        .await
        .map_err(|err| setup_failure(EMPTYING_FAILED, err))?;
    uow.commit()
        .await
        .map_err(|err| setup_failure(EMPTYING_FAILED, err))?;
    Span::current().record("deleted", span_count(deleted));
    tracing::info!(
        step = "STEP05",
        demoted,
        deleted,
        step10_bypassed = !step10_runs,
        "live transaction master emptied"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::step10_runs;
    use crate::exit::ExitStatus;

    #[test]
    fn cond_4_lt_bypasses_step10_above_4() {
        // The two STEP05R outcomes (0: P-TRANBKP-001 and P-TRANBKP-004; 12: P-TRANBKP-002)
        // and the codes around the COND=(4,LT) boundary.
        let cases = [
            (ExitStatus::Success.code(), true),
            (ExitStatus::ReproCondition(12).code(), false),
            (4, true),
            (5, false),
            (8, false),
        ];
        for (code, expected) in cases {
            assert_eq!(step10_runs(code), expected, "condition code {code}");
        }
    }
}
