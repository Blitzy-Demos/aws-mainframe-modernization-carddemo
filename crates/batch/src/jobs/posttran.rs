//! `batch posttran`: the POSTTRAN job, whose one step `STEP15` runs CBTRN02C over the daily
//! transaction file [app/jcl/POSTTRAN.jcl:L23-42], plus the reward accrual the replacement adds
//! to every posted record (AAP 0.3.5).
//!
//! The job only orchestrates. Validation, posting, the reject reasons and the return code belong
//! to `carddemo_domain::rules::posting`, the accrual to `RewardEngine`. The CBTRN02C paragraphs
//! run in their COBOL order (P-CBTRN02C-004):
//!
//! 1. `0000-DALYTRAN-OPEN`: the `--input` file, DD `DALYTRAN`, is opened.
//! 2. `0100-TRANFILE-OPEN`: `OPEN OUTPUT` of the transaction master, which fails when the live
//!    master already holds records (E-32).
//! 3. `0300-DALYREJS-OPEN`: `DALYREJS(+1)` is allocated, DD `DALYREJS` with
//!    `DISP=(NEW,CATLG,DELETE)` and `LRECL=430` [app/jcl/POSTTRAN.jcl:L34-38].
//! 4. The main loop over the records in file order, numbered from 1 (P-CBTRN02C-006).
//! 5. `9300-DALYREJS-CLOSE` catalogs `DALYREJS(+1)`, empty after a run without rejects, and the
//!    run counts and `RETURN-CODE` follow (P-CBTRN02C-020).
//!
//! `0200-XREFFILE-OPEN`, `0400-ACCTFILE-OPEN` and `0500-TCATBALF-OPEN` (P-CBTRN02C-222,
//! P-CBTRN02C-224, P-CBTRN02C-225) have no step of their own: those files are tables reached
//! through the pool `jobs::dispatch` opened, and a failure to open it is a setup failure.
//!
//! # Units of work
//!
//! Every record runs in its own database transaction. A posted record commits its CBTRN02C
//! updates and its reward accrual together (P-CBTRN02C-019). A rejected record wrote nothing, so
//! its unit is rolled back. When a posting write abends, the writes the record completed before
//! it are committed, because every posting write persists on a later failure
//! (`RECOVERY(NONE)`, P-CBTRN02C-015 to P-CBTRN02C-019). When the reward engine fails, the
//! record's unit is rolled back whole, posting updates included.
//!
//! # How a run ends
//!
//! | Outcome | Exit code | `DALYREJS(+1)` |
//! |---|---|---|
//! | Every record read | 4 with rejects, else 0 | cataloged |
//! | `9999-ABEND-PROGRAM` | 16, the `DISPLAY` lines and `ABEND 999` on stderr | deleted |
//! | Reward-engine halt | 8, the record number and TRAN-ID on stderr | deleted |
//! | Setup failure, undecodable input | 16 | deleted |
//!
//! Records committed before an abend or a halt stay committed, as the COBOL's VSAM updates do,
//! while the uncataloged `DALYREJS(+1)` is deleted with the rejects written so far, as
//! `DISP=(…,DELETE)` deletes it on abnormal end. The previous generation stays `(0)`.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::slice;

use carddemo_codec::framing::{Framed, decode_ascii_file};
use carddemo_codec::layouts::{DailyTranLayout, DailyTranRecord, RejectRecord, RejectRecordLayout};
use carddemo_domain::clock::{Clock, FixedClock};
use carddemo_domain::entities::{Account, CardXref, DailyTran};
use carddemo_domain::messages::{
    ACCOUNT_RECORD_NOT_FOUND, transactions_processed_message, transactions_rejected_message,
};
use carddemo_domain::numeric::{Alnum, NumericCode};
use carddemo_domain::ports::TransactionReader;
use carddemo_domain::rewards::{AccrualInput, RewardEngine, RewardError};
use carddemo_domain::rules::posting::{self, PostingAbend, PostingCounts, Validation};
use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;
use tracing::field::display;

use super::{JobContext, setup_failure};
use crate::cli::PosttranArgs;
use crate::exit::ExitStatus;
use crate::generations::{Dataset, PendingGeneration};

/// The `ABCODE` that `9999-ABEND-PROGRAM` passes to `CEE3ABD` — P-CBTRN02C-021
/// [app/cbl/CBTRN02C.cbl:L707-711].
const ABCODE: u32 = 999;

/// Runs POSTTRAN `STEP15` over the `--input` file and returns how it ended (see the module
/// documentation).
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &PosttranArgs) -> ExitStatus {
    match post_daily_file(ctx, db, &args.input).await {
        Ok(status) | Err(status) => status,
    }
}

/// CBTRN02C PROCEDURE DIVISION — P-CBTRN02C-004, P-CBTRN02C-006, P-CBTRN02C-020
/// [app/cbl/CBTRN02C.cbl:L193-234]: the opens, the main loop, the close of `DALYREJS` and the
/// end-of-job counts. `Err` carries the status of a run that stopped early.
async fn post_daily_file(
    ctx: &JobContext,
    db: &Db,
    input: &Path,
) -> Result<ExitStatus, ExitStatus> {
    let file = open_daily(input)?;
    open_tranfile(db).await?;
    let mut rejects = open_rejects(ctx)?;
    let records = read_daily(file, input)?;
    let mut tally = Tally::default();
    let outcome = post_records(ctx, db, &records, &mut rejects, &mut tally).await;
    tally.record_on_span();
    outcome?;
    close_rejects(rejects)?;
    tally.display_counts();
    Ok(ExitStatus::ReturnCode(tally.counts.return_code()))
}

/// CBTRN02C 0000-DALYTRAN-OPEN — P-CBTRN02C-221 [app/cbl/CBTRN02C.cbl:L236-252]: `OPEN INPUT`
/// of DD `DALYTRAN`, the `--input` file. A file that cannot be opened abends before any other
/// file is touched. The operating-system error is logged, never shown as a file status.
fn open_daily(input: &Path) -> Result<File, ExitStatus> {
    File::open(input).map_err(|err| {
        let cause = format!("{}: {err}", input.display());
        abend_status(&PostingAbend::dalytran_open(), None, Some(&cause))
    })
}

/// CBTRN02C 0100-TRANFILE-OPEN — P-CBTRN02C-005, P-CBTRN02C-219
/// [app/cbl/CBTRN02C.cbl:L254-270]: `OPEN OUTPUT` of a transaction master defined without
/// `REUSE` fails with status 37 when it already holds records. The run then abends before
/// anything is read or written, so a `posttran` without a `tranbkp` before it changes nothing
/// (E-32).
///
/// The live master is probed with one read in a unit of work that writes nothing. A database
/// failure there has no COBOL counterpart and is a setup failure.
async fn open_tranfile(db: &Db) -> Result<(), ExitStatus> {
    const WHAT: &str = "live transaction master read failed";
    let mut uow = db.begin().await.map_err(|err| setup_failure(WHAT, err))?;
    let highest = uow
        .store()
        .highest_transaction()
        .await
        .map_err(|err| setup_failure(WHAT, err))?;
    uow.rollback()
        .await
        .map_err(|err| setup_failure(WHAT, err))?;
    match highest {
        None => Ok(()),
        Some(_) => Err(abend_status(
            &PostingAbend::tranfile_open_not_empty(),
            None,
            None,
        )),
    }
}

/// CBTRN02C 0300-DALYREJS-OPEN — P-CBTRN02C-223 [app/cbl/CBTRN02C.cbl:L291-307]: `OPEN OUTPUT`
/// of DD `DALYREJS`, the new generation `DALYREJS(+1)` [app/jcl/POSTTRAN.jcl:L34-38]. Any
/// failure to allocate it abends.
fn open_rejects(ctx: &JobContext) -> Result<PendingGeneration, ExitStatus> {
    ctx.generations
        .begin_next(Dataset::DalyRejs)
        .map_err(|err| abend_status(&PostingAbend::dalyrejs_open(), None, Some(&err)))
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007 [app/cbl/CBTRN02C.cbl:L345-369]: the
/// records of DD `DALYTRAN` in file order, decoded with the `CVTRA06Y` layout. End of file is
/// the end of the returned list.
///
/// The file is read whole at the first `READ`, so a read failure abends before any record is
/// processed.
fn read_daily(mut file: File, input: &Path) -> Result<Vec<Framed<DailyTranRecord>>, ExitStatus> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|err| {
        let cause = format!("{}: {err}", input.display());
        abend_status(&PostingAbend::dalytran_read(), None, Some(&cause))
    })?;
    // A record that does not decode with CVTRA06Y (a non-digit in a numeric field, a body
    // longer than 350 bytes) has no audited outcome: the COBOL would go on computing with its
    // bytes, with results its compiler options decide. Decoding the whole file first stops
    // such a run before any record is posted, as a setup failure (exit 16) whose text names
    // the 1-based record number, rather than guessing what CBTRN02C would do with it.
    decode_ascii_file::<DailyTranLayout>(&bytes)
        .map_err(|err| setup_failure("daily transaction record does not decode", err))
}

/// CBTRN02C main loop — P-CBTRN02C-006 [app/cbl/CBTRN02C.cbl:L202-219]: each record in file
/// order, numbered from 1, is counted, validated by `1500-VALIDATE-TRAN` (P-CBTRN02C-008 to
/// P-CBTRN02C-011), then posted or counted and written as a reject.
///
/// Records run strictly one after another, never batched or concurrently, because the cycle
/// totals a posted record leaves take part in the limit check of the next one.
async fn post_records(
    ctx: &JobContext,
    db: &Db,
    records: &[Framed<DailyTranRecord>],
    rejects: &mut PendingGeneration,
    tally: &mut Tally,
) -> Result<(), ExitStatus> {
    for (framed, record_number) in records.iter().zip(1_usize..) {
        let daily = &framed.record;
        tally.counts.read += 1;
        let mut uow = db
            .begin()
            .await
            .map_err(|err| setup_failure("unit of work could not begin", err))?;
        let validation = posting::validate(&mut uow.store(), &daily.daily_tran).await;
        match validation {
            Ok(Validation::Accepted { xref, account }) => {
                post_record(ctx, uow, record_number, &daily.daily_tran, &xref, account).await?;
                tally.posted += 1;
            }
            Ok(Validation::Rejected {
                reason,
                description,
            }) => {
                uow.rollback()
                    .await
                    .map_err(|err| setup_failure("read-only unit of work rollback failed", err))?;
                write_reject(rejects, daily, record_number, reason, description)?;
                tally.counts.rejected += 1;
            }
            Err(abend) => {
                return Err(posting_abend(uow, &abend, record_number, &daily.daily_tran).await);
            }
        }
    }
    Ok(())
}

/// CBTRN02C 2000-POST-TRANSACTION — P-CBTRN02C-013 to P-CBTRN02C-019, P-CBTRN02C-218
/// [app/cbl/CBTRN02C.cbl:L424-444], then the reward accrual (AAP 0.3.5), in the unit of work
/// `uow` the validation read in.
///
/// The clock is read once, so the record's `TRAN-PROC-TS` (P-CBTRN02C-014, P-CBTRN02C-220) and
/// its accrual time are the same instant; `CARDDEMO_CLOCK_OVERRIDE` pins both. The accrual
/// credits `XREF-ACCT-ID`, the account the validation read. The unit commits only when both the
/// posting and the accrual succeeded.
async fn post_record(
    ctx: &JobContext,
    mut uow: UnitOfWork,
    record_number: usize,
    tran: &DailyTran,
    xref: &CardXref,
    account: Account,
) -> Result<(), ExitStatus> {
    let at = ctx.clock.now();
    let result = posting::post(&mut uow.store(), tran, xref, account, &FixedClock(at)).await;
    let posted = match result {
        Ok(posted) => posted,
        Err(abend) => return Err(posting_abend(uow, &abend, record_number, tran).await),
    };
    if posted.account_rewrite_failed {
        // The REWRITE INVALID KEY path: reason 109 is set but never counted or written to
        // DALYREJS, and the record is still posted (P-CBTRN02C-018, P-CBTRN02C-218, E-15).
        tracing::warn!(
            record_number,
            tran_id = %tran.dalytran_id,
            "{ACCOUNT_RECORD_NOT_FOUND}"
        );
    }
    let input = AccrualInput::for_transaction(&posted.transaction, xref.xref_acct_id);
    let accrued = RewardEngine::accrue(&mut uow.rewards(), input, at).await;
    let accrual = match accrued {
        Ok(accrual) => accrual,
        Err(err) => return Err(reward_halt(uow, &err, record_number, tran).await),
    };
    uow.commit()
        .await
        .map_err(|err| setup_failure("posted record commit failed", err))?;
    tracing::debug!(
        record_number,
        tran_id = %tran.dalytran_id,
        accrued = accrual.is_some(),
        "record posted"
    );
    Ok(())
}

/// CBTRN02C 2500-WRITE-REJECT-REC — P-CBTRN02C-001 to P-CBTRN02C-003, P-CBTRN02C-012
/// [app/cbl/CBTRN02C.cbl:L446-464]: appends the 430-byte `REJECT-RECORD` of `daily` to
/// `DALYREJS(+1)`, the 350 bytes of the input record as read, the reason as `9(04)` and the
/// description moved into `X(76)`, and logs the reject. A failure to write it abends.
fn write_reject(
    rejects: &mut PendingGeneration,
    daily: &DailyTranRecord,
    record_number: usize,
    reason: u16,
    description: &'static str,
) -> Result<(), ExitStatus> {
    let record = Some((record_number, &daily.daily_tran));
    // The rule rejects with 100 to 103 only, which the 9(04) field always holds; a reason it
    // could not hold would leave a record that cannot be written.
    let reason_code = NumericCode::<4>::new(u32::from(reason))
        .map_err(|err| abend_status(&PostingAbend::reject_write(), record, Some(&err)))?;
    let reject = RejectRecord::new(daily, reason_code, Alnum::move_from(description));
    rejects
        .append_records::<RejectRecordLayout>(slice::from_ref(&reject))
        .map_err(|err| abend_status(&PostingAbend::reject_write(), record, Some(&err)))?;
    tracing::warn!(
        record_number,
        tran_id = %daily.daily_tran.dalytran_id,
        reason,
        description,
        "record rejected"
    );
    Ok(())
}

/// The reward-engine halt (AAP 0.3.5, 0.5.3.5): the record's unit of work is rolled back, so
/// neither its CBTRN02C updates nor its accrual persist, and the run stops with exit 8. The
/// records before it stay committed, and nothing is added to `DALYREJS`, because the caller
/// drops its pending generation uncataloged.
async fn reward_halt(
    uow: UnitOfWork,
    err: &RewardError,
    record_number: usize,
    tran: &DailyTran,
) -> ExitStatus {
    if let Err(rollback) = uow.rollback().await {
        tracing::error!(record_number, error = %rollback, "halted record rollback failed");
    }
    let tran_id = tran.dalytran_id.to_string();
    tracing::error!(
        record_number,
        tran_id = tran_id.as_str(),
        error = %err,
        "reward engine halt"
    );
    ExitStatus::RewardHalt {
        record_number,
        tran_id,
        cause: err.to_string(),
    }
}

/// CBTRN02C 9999-ABEND-PROGRAM — P-CBTRN02C-021 [app/cbl/CBTRN02C.cbl:L707-711], reached from a
/// failing read or write of `1500-VALIDATE-TRAN` or `2000-POST-TRANSACTION`.
///
/// The writes the record completed before the failure stay, as the "Persists on later failure"
/// column records for every posting write (`persists: RECOVERY(NONE)`, P-CBTRN02C-015 to
/// P-CBTRN02C-019), so `uow` is committed before the run ends with exit 16. No accrual was made
/// for the record.
async fn posting_abend(
    uow: UnitOfWork,
    abend: &PostingAbend,
    record_number: usize,
    tran: &DailyTran,
) -> ExitStatus {
    if let Err(err) = uow.commit().await {
        tracing::error!(
            record_number,
            error = %err,
            "completed writes of the abending record not committed"
        );
    }
    let cause = abend.cause.as_ref().map(|cause| cause as &dyn fmt::Display);
    abend_status(abend, Some((record_number, tran)), cause)
}

/// CBTRN02C 9300-DALYREJS-CLOSE — P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L637-653]: the normal
/// end of the step, where `DISP=(NEW,CATLG,DELETE)` catalogs `DALYREJS(+1)`, empty after a run
/// without rejects. A failure abends and adds no generation, while every posted record stays
/// committed, as the COBOL's VSAM updates do.
fn close_rejects(rejects: PendingGeneration) -> Result<(), ExitStatus> {
    rejects
        .catalog()
        .map_err(|err| abend_status(&PostingAbend::dalyrejs_close(), None, Some(&err)))?;
    Ok(())
}

/// CBTRN02C 9999-ABEND-PROGRAM — P-CBTRN02C-021 [app/cbl/CBTRN02C.cbl:L707-711]: logs `abend`
/// with the record it stopped at and the failure behind it, then returns the abend that prints
/// its `DISPLAY` lines and `ABEND 999` and exits 16.
fn abend_status(
    abend: &PostingAbend,
    record: Option<(usize, &DailyTran)>,
    cause: Option<&dyn fmt::Display>,
) -> ExitStatus {
    let file_status = (!abend.file_status.is_empty()).then_some(abend.file_status.as_str());
    let record_number = record.map(|(number, _)| number);
    let tran_id = record.map(|(_, tran)| display(tran.dalytran_id));
    let cause = cause.map(display);
    tracing::error!(
        paragraph = abend.paragraph,
        file_status,
        record_number,
        tran_id,
        cause,
        "{}",
        abend.display
    );
    ExitStatus::abend_from(abend, Some(ABCODE))
}

/// `WS-COUNTERS` of the run — P-CBTRN02C-006, P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L184-186]:
/// the records read and rejected, plus the records posted, which the job span reports.
#[derive(Debug, Default)]
struct Tally {
    /// `WS-TRANSACTION-COUNT` and `WS-REJECT-COUNT`.
    counts: PostingCounts,
    /// The records `2000-POST-TRANSACTION` posted.
    posted: u64,
}

impl Tally {
    /// Records `read`, `posted` and `rejected` on the job span.
    fn record_on_span(&self) {
        let span = Span::current();
        span.record("read", self.counts.read);
        span.record("posted", self.posted);
        span.record("rejected", self.counts.rejected);
    }

    /// CBTRN02C end of job — P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L227-228]: the
    /// `TRANSACTIONS PROCESSED :` and `TRANSACTIONS REJECTED  :` displays, as one event whose
    /// fields carry the counts and the two display lines.
    fn display_counts(&self) {
        tracing::info!(
            processed = self.counts.read,
            rejected = self.counts.rejected,
            processed_display = transactions_processed_message(self.counts.read).as_str(),
            rejected_display = transactions_rejected_message(self.counts.rejected).as_str(),
            "end of job counts"
        );
    }
}
