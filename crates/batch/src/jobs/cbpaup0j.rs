//! `batch cbpaup0j`: the job `CBPAUP0J`
//! [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L24-37], whose one step runs the IMS BMP
//! program `CBPAUP0C` (`DFSRRC00 PARM='BMP,CBPAUP0C,PSBPAUTB'`) to delete expired pending
//! authorizations [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl].
//!
//! The job reads the `SYSIN` card from `--sysin`, takes the day from the processing clock, runs
//! `carddemo_domain::rules::auth_purge::purge` in one unit of work and commits it. The purge
//! logic, the summary-delete condition that tests the approved count twice (E-16) and the
//! in-memory count decrements (E-35) all live in the rule.
//!
//! `DISPLAY` output becomes `tracing` events inside the job span: the parameters, the day, then
//! either the four end-of-job counts or the abend. The span's `read` and `deleted` fields carry
//! the summaries and details read and deleted.

use carddemo_domain::clock::{Clock, yyddd};
use carddemo_domain::rules::auth_purge::{self, PurgeAbend, PurgeCounts, PurgeParams};
use carddemo_persistence::Db;
use tracing::Span;

use super::{JobContext, setup_failure};
use crate::cli::Cbpaup0jArgs;
use crate::exit::ExitStatus;

/// CBPAUP0C MAIN-PARA — P-CBPAUP0C-001 to P-CBPAUP0C-009, P-CBPAUP0C-012.
///
/// Runs the purge [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L136-180]:
///
/// 1. `1000-INITIALIZE` reads `PRM-INFO` from the `SYSIN` card and applies its defaults
///    (P-CBPAUP0C-001), and `ACCEPT CURRENT-YYDDD FROM DAY` reads the day from the processing
///    clock, so `CARDDEMO_CLOCK_OVERRIDE` pins it (P-CBPAUP0C-012).
/// 2. The rule sweeps every summary and its details in one unit of work.
/// 3. The unit of work is committed whether the sweep ends normally or abends. `EXEC DLI CHKP`
///    (P-CBPAUP0C-008), the normal end and the `GOBACK` of `9999-ABEND` all commit the deletes
///    made so far (P-CBPAUP0C-005, P-CBPAUP0C-009), so one commit after the rule returns leaves
///    the same rows as the COBOL's checkpoints. The rule therefore takes no checkpoint, and the
///    checkpoint frequencies are only reported.
///
/// A normal end exits 0 after logging the four counts the COBOL displays
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L171-178]. An abend exits 16 with the
/// rule's `DISPLAY` text on stderr and no `ABEND` code line, because `9999-ABEND` moves 16 to
/// `RETURN-CODE` and returns rather than calling `CEE3ABD`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L377-383].
///
/// A failure to begin or commit the unit of work also exits 16. Such a failure has no COBOL
/// text, so it is reported as a setup failure. The commit stands in for the final
/// `9000-TAKE-CHECKPOINT`, where a failed `CHKP` performs `9999-ABEND` and so also ends with
/// 16 (P-CBPAUP0C-008). After a failed commit, none of the run's deletes persist.
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &Cbpaup0jArgs) -> ExitStatus {
    let params = auth_purge::parse_sysin(&args.sysin);
    log_parameters(&args.sysin, &params);
    let today = yyddd(ctx.clock.now());
    tracing::info!(today_yyddd = today, "TODAYS DATE");

    let mut uow = match db.begin().await {
        Ok(uow) => uow,
        Err(err) => return setup_failure("pending authorization purge could not begin", err),
    };
    let outcome = auth_purge::purge(&mut uow.store(), &params, today).await;
    // 9999-ABEND displays its text before the GOBACK that commits.
    if let Err(abend) = &outcome {
        log_abend(abend);
    }
    // The span totals are recorded only once the deletes they count are committed.
    if let Err(err) = uow.commit().await {
        return setup_failure("pending authorization purge commit failed", err);
    }
    match outcome {
        Ok(counts) => {
            record_totals(counts);
            log_counts(counts);
            ExitStatus::Success
        }
        Err(abend) => {
            record_totals(abend.counts);
            ExitStatus::abend_from(&abend, None)
        }
    }
}

/// CBPAUP0C 1000-INITIALIZE — P-CBPAUP0C-001.
///
/// `DISPLAY 'CBPAUP0C PARM RECEIVED :' PRM-INFO`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L192]: the card as given, with the values
/// `1000-INITIALIZE` derives from it.
fn log_parameters(sysin: &str, params: &PurgeParams) {
    tracing::info!(
        sysin,
        expiry_days = params.expiry_days,
        chkp_freq = params.chkp_freq,
        chkp_dis_freq = params.chkp_dis_freq,
        debug = params.debug,
        "PARM RECEIVED"
    );
}

/// The job-span totals of `counts`: rows read (summaries and details), then rows deleted
/// (summaries and details deleted one by one). Saturating, so an implausible count cannot
/// panic the run.
const fn totals(counts: PurgeCounts) -> (u64, u64) {
    (
        counts.summaries_read.saturating_add(counts.details_read),
        counts
            .summaries_deleted
            .saturating_add(counts.details_deleted),
    )
}

/// Records [`totals`] in the `read` and `deleted` fields of the job span.
fn record_totals(counts: PurgeCounts) {
    let (read, deleted) = totals(counts);
    let span = Span::current();
    span.record("read", read);
    span.record("deleted", deleted);
}

/// CBPAUP0C MAIN-PARA — P-CBPAUP0C-002, P-CBPAUP0C-003, P-CBPAUP0C-005, P-CBPAUP0C-007.
///
/// The end-of-job `DISPLAY` lines of `WS-NO-SUMRY-READ`, `WS-NO-SUMRY-DELETED`,
/// `WS-NO-DTL-READ` and `WS-NO-DTL-DELETED`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L173-176], as one event.
fn log_counts(counts: PurgeCounts) {
    tracing::info!(
        summaries_read = counts.summaries_read,
        summaries_deleted = counts.summaries_deleted,
        details_read = counts.details_read,
        details_deleted = counts.details_deleted,
        "expired pending authorizations purged"
    );
}

/// CBPAUP0C 9999-ABEND — P-CBPAUP0C-009.
///
/// `DISPLAY 'CBPAUP0C ABENDING ...'` and the failure that led to it, with the counts of the
/// work done before it, which the job then commits as the `GOBACK` does.
fn log_abend(abend: &PurgeAbend) {
    tracing::error!(
        error = %abend,
        cause = ?abend.cause,
        summaries_read = abend.counts.summaries_read,
        summaries_deleted = abend.counts.summaries_deleted,
        details_read = abend.counts.details_read,
        details_deleted = abend.counts.details_deleted,
        "pending authorization purge abended"
    );
}

#[cfg(test)]
mod tests {
    use carddemo_domain::rules::auth_purge::PurgeCounts;

    use super::totals;

    #[test]
    fn totals_add_summaries_and_details() {
        let cases = [
            (PurgeCounts::default(), (0, 0)),
            (
                PurgeCounts {
                    summaries_read: 3,
                    summaries_deleted: 1,
                    details_read: 7,
                    details_deleted: 4,
                },
                (10, 5),
            ),
            (
                PurgeCounts {
                    summaries_read: u64::MAX,
                    summaries_deleted: u64::MAX,
                    details_read: 1,
                    details_deleted: 1,
                },
                (u64::MAX, u64::MAX),
            ),
        ];
        for (counts, expected) in cases {
            assert_eq!(totals(counts), expected, "{counts:?}");
        }
    }
}
