//! `batch mnttrdb2`: the MNTTRDB2 job, whose one step runs `COBTUPDT` under `IKJEFT01` as
//! `DSN SYSTEM(DAZ1)` / `RUN PROGRAM(COBTUPDT) PLAN(CARDDEMO)` over the `INPFILE` dataset
//! [app/app-transaction-type-db2/jcl/MNTTRDB2.jcl:L21-30], here the `--input` file
//! (P-COBTUPDT-004).
//!
//! The COBTUPDT paragraphs fall through in their source order, and the job keeps it:
//!
//! 1. `0001-OPEN-FILES`: [`load_records`] reads the whole input and frames its 53-byte
//!    records. A file that cannot be read, or whose records do not fit `WS-INPUT-REC`, ends
//!    the job with exit 16 before any change (P-COBTUPDT-011, E-36).
//! 2. `1001-READ-NEXT-RECORDS` and `1002-READ-RECORDS`: every record, in file order, is shown
//!    with its `PROCESSING` display (P-COBTUPDT-005).
//! 3. `1003-TREAT-RECORD` and the statement it performs: [`treat_record`] hands the record to
//!    `carddemo_domain::rules::tran_type_batch::apply`, which changes the Db2 master
//!    `CARDDEMO.TRANSACTION_TYPE` only, never the `TRANTYPE` processing copy the reports read
//!    (P-COBTUPDT-006 to P-COBTUPDT-009). A failed record performs `9999-ABEND`, which displays
//!    `WS-RETURN-MSG`, sets return code 4 and lets the run go on (P-COBTUPDT-010).
//! 4. `2001-CLOSE-STOP` and `STOP RUN`: the unit of work commits every statement that
//!    succeeded, and the job ends with the return code `9999-ABEND` left (P-COBTUPDT-012).
//!
//! All records go through one unit of work, and each statement runs under its own savepoint
//! in the persistence adapter, so a failed statement changes nothing and leaves the others in
//! place, as Db2 does for a statement that ends with a negative `SQLCODE`. Each `DISPLAY`
//! becomes one `tracing` event inside the job span: `info` for the `PROCESSING` line and the
//! success displays, `warn` for the `9999-ABEND` display. The job span receives the counts
//! `read`, `written` (inserts and updates), `deleted` and `rejected` (failed records).

use std::fmt;
use std::fs;
use std::path::Path;

use carddemo_codec::CodecError;
use carddemo_codec::framing::{pad, split_ascii};
use carddemo_domain::messages::OPEN_FILE_NOT_OK;
use carddemo_domain::rules::tran_type_batch::{
    self, INPUT_REC_LEN, MaintRecord, MaintRun, RecordOutcome,
};
use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;

use super::{JobContext, setup_failure};
use crate::cli::Mnttrdb2Args;
use crate::exit::ExitStatus;

/// The label an over-long `INPFILE` record is reported under: the program whose 53-byte
/// `WS-INPUT-REC` it does not fit.
const INPUT_LAYOUT: &str = "COBTUPDT";

/// COBTUPDT 0001-OPEN-FILES through STOP RUN — P-COBTUPDT-004 to P-COBTUPDT-012, E-36.
///
/// Runs MNTTRDB2 over `args.input`: loads every record, applies each to the Db2 transaction
/// type master in file order inside one unit of work, commits it, and returns
/// [`ExitStatus::ReturnCode`] with 4 when any record failed, else 0.
///
/// The job reads no setting, clock or generation, so the context is unused.
///
/// Each of these ends the job with a setup failure (exit 16):
///
/// - the input cannot be read or framed (P-COBTUPDT-011, E-36), before any change;
/// - the unit of work cannot begin, before any change;
/// - the commit at `STOP RUN` fails, which leaves every statement of the run undone.
pub(super) async fn run(_ctx: &JobContext, db: &Db, args: &Mnttrdb2Args) -> ExitStatus {
    let records = match load_records(&args.input) {
        Ok(records) => records,
        Err(status) => return status,
    };
    let mut uow = match db.begin().await {
        Ok(uow) => uow,
        Err(err) => return setup_failure("MNTTRDB2 unit of work could not begin", err),
    };
    let mut run = MaintRun::default();
    let mut counts = Counts::default();
    for (index, record) in records.iter().enumerate() {
        let outcome = treat_record(&mut uow, &mut run, index + 1, record).await;
        counts.add(&outcome);
    }
    Span::current().record("read", counts.read);
    if let Err(err) = uow.commit().await {
        return setup_failure("MNTTRDB2 STOP RUN commit failed", err);
    }
    let span = Span::current();
    span.record("written", counts.written);
    span.record("deleted", counts.deleted);
    span.record("rejected", counts.failed);
    ExitStatus::ReturnCode(run.return_code())
}

/// COBTUPDT 0001-OPEN-FILES, 1002-READ-RECORDS — P-COBTUPDT-004, P-COBTUPDT-005,
/// P-COBTUPDT-011, E-36.
///
/// The records of the `INPFILE` at `path`, in file order, as `READ TR-RECORD NEXT RECORD INTO
/// WS-INPUT-REC` leaves them.
///
/// COBTUPDT itself only displays `OPEN FILE NOT OK` after a failed `OPEN` and then reads
/// without ever reaching `AT END` [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L82-99];
/// under JCL a missing `INPFILE` fails allocation and the step never runs. The audited
/// disposition is exit 16 before any change (E-36), so every record is framed here, before
/// the unit of work begins.
///
/// A record longer than 53 bytes falls under the same disposition. `INPFILE` is the
/// `RECORDING MODE F` file whose 53-byte record `WS-INPUT-VARS`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L39-46] is read into `WS-INPUT-REC`, so a
/// longer record cannot occur in the dataset the step opens; input holding one is a file
/// COBTUPDT cannot open as its FD declares, not a record to cut to 53 bytes.
///
/// # Errors
///
/// The setup failure (exit 16) of a file that cannot be read or holds a record longer than
/// 53 bytes, logged under [`OPEN_FILE_NOT_OK`] with the path and the cause.
fn load_records(path: &Path) -> Result<Vec<MaintRecord>, ExitStatus> {
    let bytes = fs::read(path).map_err(|err| open_failure(path, &err))?;
    let records = parse_input(&bytes).map_err(|err| open_failure(path, &err))?;
    tracing::info!(
        input = %path.display(),
        records = records.len(),
        "INPFILE opened"
    );
    Ok(records)
}

/// The exit-16 setup failure of an `INPFILE` that cannot be opened as COBTUPDT declares it
/// (P-COBTUPDT-011, E-36), reporting `path` and `err`.
fn open_failure(path: &Path, err: &impl fmt::Display) -> ExitStatus {
    setup_failure(
        OPEN_FILE_NOT_OK,
        format!("INPFILE {}: {err}", path.display()),
    )
}

/// COBTUPDT 1002-READ-RECORDS — P-COBTUPDT-001 to P-COBTUPDT-003, P-COBTUPDT-005.
///
/// Splits `bytes` into its lines (`LF`, `CR LF`, or none after the last), pads each body
/// with spaces to the 53 bytes of `WS-INPUT-REC` and parses it. An empty line is a record of
/// spaces, which `1003-TREAT-RECORD` rejects as an invalid type; nothing after the last line
/// end is no record, so an empty file has none and the run ends at once with return code 0.
///
/// # Errors
///
/// The [`CodecError`] of the first record longer than 53 bytes, located at its 1-based
/// record number.
fn parse_input(bytes: &[u8]) -> Result<Vec<MaintRecord>, CodecError> {
    split_ascii(bytes)
        .into_iter()
        .enumerate()
        .map(|(index, (_frame, body))| {
            pad(INPUT_LAYOUT, body, INPUT_REC_LEN)
                .map(|padded| tran_type_batch::parse_record(&padded))
                .map_err(|err| err.at_record(index + 1))
        })
        .collect()
}

/// COBTUPDT 1002-READ-RECORDS, 1003-TREAT-RECORD, 9999-ABEND — P-COBTUPDT-005 to
/// P-COBTUPDT-010.
///
/// Shows the `PROCESSING` display of record `number`, applies it to the Db2 master through the
/// store of `uow`, adds the outcome to `run`, and shows the display that follows:
///
/// - the success display of the paragraph (`RECORD INSERTED SUCCESSFULLY`, `RECORD UPDATED
///   SUCCESSFULLY`, `RECORD DELETED SUCCESSFULLY`, `IGNORING COMMENTED LINE`) as `info`;
/// - for a failed record, the 80-byte `WS-RETURN-MSG` line `9999-ABEND` displays, as `warn`;
/// - for an insert ending with a positive `SQLCODE` its `EVALUATE` does not test, which
///   COBTUPDT displays nothing for, an `info` event without COBOL text.
async fn treat_record(
    uow: &mut UnitOfWork,
    run: &mut MaintRun,
    number: usize,
    record: &MaintRecord,
) -> RecordOutcome {
    let tran_type = record.code.trimmed();
    tracing::info!(record = number, tran_type = %tran_type, "{}", record.processing_message());
    let outcome = tran_type_batch::apply(&mut uow.store(), record).await;
    if let Some(abend_line) = run.record(&outcome) {
        tracing::warn!(record = number, tran_type = %tran_type, "{abend_line}");
    } else if let Some(text) = outcome.display_text() {
        tracing::info!(record = number, tran_type = %tran_type, "{text}");
    } else {
        tracing::info!(
            record = number,
            tran_type = %tran_type,
            "statement ended with a positive SQLCODE COBTUPDT does not test"
        );
    }
    outcome
}

/// The record counts the job span receives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    /// Records read, comments and failed records included.
    read: usize,
    /// Types inserted or updated.
    written: usize,
    /// Types deleted.
    deleted: usize,
    /// Records for which `9999-ABEND` was performed.
    failed: usize,
}

impl Counts {
    /// Counts one record with its `outcome`.
    fn add(&mut self, outcome: &RecordOutcome) {
        self.read += 1;
        match outcome {
            RecordOutcome::Inserted | RecordOutcome::Updated => self.written += 1,
            RecordOutcome::Deleted => self.deleted += 1,
            RecordOutcome::Failed { .. } => self.failed += 1,
            RecordOutcome::Ignored | RecordOutcome::Unreported => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use carddemo_codec::CodecErrorKind;
    use carddemo_domain::messages::NO_RECORDS_FOUND;
    use carddemo_domain::rules::tran_type_batch::{MaintRecord, RecordOutcome, parse_record};

    use super::{Counts, load_records, parse_input};
    use crate::exit::ExitStatus;

    /// The MNTTRDB2 fixture input, 53-byte records each followed by `LF`.
    const FIXTURE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mnttrdb2_input.txt"
    ));

    /// The record `text` gives once padded to the 53 bytes of `WS-INPUT-REC`.
    fn record(text: &str) -> MaintRecord {
        parse_record(format!("{text:<53}").as_bytes())
    }

    fn records(texts: &[&str]) -> Vec<MaintRecord> {
        texts.iter().map(|text| record(text)).collect()
    }

    #[test]
    fn the_fixture_reads_as_its_lines_in_file_order() {
        let parsed = parse_input(FIXTURE).unwrap_or_else(|e| panic!("fixture must frame: {e}"));
        let lines: Vec<&str> = std::str::from_utf8(FIXTURE)
            .unwrap_or_else(|e| panic!("fixture is ASCII: {e}"))
            .lines()
            .collect();
        assert_eq!(parsed, records(&lines));
        assert_eq!(parsed.len(), 10);
        assert_eq!(
            parsed[1].processing_message().trim_end(),
            "PROCESSING   A08BALANCE TRANSFER"
        );
    }

    #[test]
    fn every_line_frame_pads_to_ws_input_rec() {
        let cases: [(&[u8], &[&str]); 6] = [
            (b"", &[]),
            (b"A10NEW TYPE\n", &["A10NEW TYPE"]),
            (b"U01PURCHASE\r\nD02\n", &["U01PURCHASE", "D02"]),
            (b"D03", &["D03"]),
            (b"*\n\nD04\n", &["*", "", "D04"]),
            (b"A05X\nU06Y", &["A05X", "U06Y"]),
        ];
        for (input, expected) in cases {
            let parsed = parse_input(input)
                .unwrap_or_else(|e| panic!("{:?} must frame: {e}", String::from_utf8_lossy(input)));
            assert_eq!(
                parsed,
                records(expected),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn a_record_longer_than_ws_input_rec_is_located_and_refused() {
        let full = format!("{:<53}", "A10FITS EXACTLY");
        let long = format!("{full}X");
        let input = format!("{full}\n{long}\nD10\n");
        let err = parse_input(input.as_bytes()).expect_err("a 54-byte record must be refused");
        assert_eq!(err.layout, "COBTUPDT");
        assert_eq!(err.record, Some(2));
        assert_eq!(err.kind, CodecErrorKind::RecordTooLong { stored_len: 54 });
    }

    #[test]
    fn an_unreadable_input_is_a_setup_failure_naming_the_file() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mnttrdb2_input.absent");
        let status = load_records(&path).expect_err("a missing INPFILE must end the job");
        assert_eq!(status.code(), 16);
        let ExitStatus::SetupFailure(text) = status else {
            panic!("a missing INPFILE is a setup failure: {status:?}");
        };
        assert!(text.starts_with("INPFILE "), "{text}");
        assert!(text.contains("mnttrdb2_input.absent"), "{text}");
    }

    #[test]
    fn counts_tally_each_outcome() {
        let mut counts = Counts::default();
        let outcomes = [
            RecordOutcome::Inserted,
            RecordOutcome::Updated,
            RecordOutcome::Deleted,
            RecordOutcome::Ignored,
            RecordOutcome::Unreported,
            RecordOutcome::Failed {
                message: NO_RECORDS_FOUND.to_owned(),
            },
            RecordOutcome::Inserted,
        ];
        for outcome in &outcomes {
            counts.add(outcome);
        }
        assert_eq!(
            counts,
            Counts {
                read: 7,
                written: 3,
                deleted: 1,
                failed: 1,
            }
        );
    }
}
