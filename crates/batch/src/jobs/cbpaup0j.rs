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
//!
//! The card is unrestricted text, so no telemetry carries it whole. The span's `sysin` field and
//! the `PARM RECEIVED` event hold only the 17-byte `PRM-INFO` receiver, with the card's byte
//! length and whether the receiver left any of it out, as [`BoundedSysin`] defines them.

use carddemo_domain::clock::{Clock, yyddd};
use carddemo_domain::numeric::Alnum;
use carddemo_domain::rules::auth_purge::{
    self, PRM_INFO_LEN, PurgeAbend, PurgeCounts, PurgeParams,
};
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
///    the same rows as the COBOL's checkpoints. Every store statement, the failed one included,
///    runs under its own savepoint, so an abend leaves the unit committable. The rule therefore
///    takes no checkpoint, and the checkpoint frequencies are only reported.
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
    log_parameters(&BoundedSysin::of(&args.sysin), &params);
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
/// What the telemetry of a run shows of its `--sysin` card: the `PRM-INFO` receiver
/// `ACCEPT PRM-INFO FROM SYSIN` fills [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L189],
/// the card's length and whether the receiver left any of the card out. The job span
/// ([`super::dispatch`]) and the `PARM RECEIVED` event both record these values and never the
/// card, so a long card cannot reach a span attribute, an event field or the span context of the
/// job's events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BoundedSysin {
    /// `PRM-INFO` as `auth_purge::prm_info` fills it from the card, the bytes `parse_sysin`
    /// reads: the first 17 bytes of the first line, padded with spaces. Its `Display` is the
    /// item's text, one character per byte with trailing spaces kept, so it never shows more
    /// than these 17 bytes, even when the cut splits a multi-byte UTF-8 character.
    pub(super) prm_info: Alnum<PRM_INFO_LEN>,
    /// The length of the card in bytes, as given.
    pub(super) len: usize,
    /// Whether the card holds a byte `PRM-INFO` did not take: a byte past the 17th of its first
    /// line, the line end (`LF` or `CR LF`), or anything after it. It is false exactly when the
    /// card is one line of at most 17 bytes without a line end, so that `PRM-INFO` begins with
    /// the whole card byte for byte. The padding is spaces, never a line-end byte, so a line end
    /// anywhere in the card sets it.
    pub(super) truncated: bool,
}

impl BoundedSysin {
    /// The bounded view of the `--sysin` card `sysin`.
    #[must_use]
    pub(super) fn of(sysin: &str) -> Self {
        let prm_info = Alnum::from_array(auth_purge::prm_info(sysin));
        Self {
            prm_info,
            len: sysin.len(),
            truncated: !prm_info.as_bytes().starts_with(sysin.as_bytes()),
        }
    }
}

/// CBPAUP0C 1000-INITIALIZE — P-CBPAUP0C-001.
///
/// `DISPLAY 'CBPAUP0C PARM RECEIVED :' PRM-INFO`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L192]: the receiver the card filled, with
/// the card's length and truncation flag ([`BoundedSysin`]) and the values `1000-INITIALIZE`
/// derives from it.
fn log_parameters(sysin: &BoundedSysin, params: &PurgeParams) {
    tracing::info!(
        sysin = %sysin.prm_info,
        sysin_len = sysin.len,
        sysin_truncated = sysin.truncated,
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
    use std::io;
    use std::sync::{Arc, Mutex};

    use carddemo_domain::rules::auth_purge::{self, PRM_INFO_LEN, PurgeCounts};
    use clap::Parser;

    use super::{BoundedSysin, log_parameters, totals};
    use crate::cli::{Cli, Command};
    use crate::jobs::job_span;

    /// The `SYSIN` card of `CBPAUP0J` [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L36-37].
    const JCL_SYSIN: &str = "00,00001,00001,Y";

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            let bytes = self.0.lock().expect("capture buffer").clone();
            String::from_utf8(bytes).expect("JSON lines are UTF-8")
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The JCL card followed by 5000 bytes `PRM-INFO` never takes: 5016 bytes in all.
    fn long_card() -> String {
        format!("{JCL_SYSIN}{}", "X".repeat(5000))
    }

    #[test]
    fn the_bounded_card_is_the_prm_info_receiver_with_length_and_truncation() {
        // P-CBPAUP0C-001: PRM-INFO takes the first 17 bytes of the first line.
        let long = long_card();
        let cases = [
            (JCL_SYSIN, "00,00001,00001,Y ", 16, false),
            ("99,99999,12345,YN", "99,99999,12345,YN", 17, false),
            ("", "                 ", 0, false),
            (long.as_str(), "00,00001,00001,YX", 5016, true),
            ("00,00001,00001,Y\n", "00,00001,00001,Y ", 17, true),
            ("15,00010,00020,Y\r\n", "15,00010,00020,Y ", 18, true),
            (
                "15,00010,00020,Y\n00,00001,00001,N",
                "15,00010,00020,Y ",
                33,
                true,
            ),
            // `é` is 0xC3 0xA9; the cut keeps 0xC3, shown as its Latin-1 character.
            ("00,00001,00001,Y\u{e9}", "00,00001,00001,Y\u{c3}", 18, true),
        ];
        for (sysin, prm_info, len, truncated) in cases {
            let label: String = sysin.chars().take(20).collect();
            let bounded = BoundedSysin::of(sysin);
            let shown = bounded.prm_info.to_string();
            assert_eq!(shown, prm_info, "{len}-byte card {label:?}");
            assert_eq!(shown.chars().count(), PRM_INFO_LEN, "{label:?}");
            assert_eq!(
                (bounded.len, bounded.truncated),
                (len, truncated),
                "{label:?}"
            );
        }
    }

    #[test]
    fn a_long_card_reaches_the_job_span_and_its_event_only_bounded() {
        let card = long_card();
        let cli = Cli::try_parse_from(["batch", "cbpaup0j", "--sysin", card.as_str()])
            .unwrap_or_else(|e| panic!("cbpaup0j --sysin: {e}"));
        let Command::Cbpaup0j(args) = &cli.command else {
            panic!("not cbpaup0j: {:?}", cli.command);
        };
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) =
            carddemo_runtime::telemetry::subscriber_with_writer("batch", "info", move || {
                sink.clone()
            })
            .expect("valid directives");
        tracing::subscriber::with_default(subscriber, || {
            let span = job_span("cbpaup0j", &cli.command);
            span.in_scope(|| {
                log_parameters(
                    &BoundedSysin::of(&args.sysin),
                    &auth_purge::parse_sysin(&args.sysin),
                );
            });
        });
        guard.shutdown().expect("the tracer provider shuts down");

        let text = capture.text();
        assert!(
            !text.contains("XX"),
            "the card's excess bytes were logged: {text}"
        );
        let line_with = |marker: &str| {
            text.lines()
                .find(|line| line.contains(marker))
                .unwrap_or_else(|| panic!("no line with {marker}: {text}"))
        };
        // The span attributes, then the event fields and its span context. The length is a
        // number in the event and may be its decimal text among the span attributes.
        for line in [line_with(r#""type":"span""#), line_with("PARM RECEIVED")] {
            for field in [
                r#""sysin":"00,00001,00001,YX""#,
                r#""sysin_truncated":true"#,
            ] {
                assert!(line.contains(field), "{field} missing from {line}");
            }
            assert!(
                line.contains(r#""sysin_len":5016"#) || line.contains(r#""sysin_len":"5016""#),
                "sysin_len 5016 missing from {line}"
            );
        }
    }

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
