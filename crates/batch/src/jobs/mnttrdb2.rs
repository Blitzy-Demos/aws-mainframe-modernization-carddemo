//! `batch mnttrdb2`: the MNTTRDB2 job, whose one step runs `COBTUPDT` under `IKJEFT01` as
//! `DSN SYSTEM(DAZ1)` / `RUN PROGRAM(COBTUPDT) PLAN(CARDDEMO)` over the `INPFILE` dataset
//! [app/app-transaction-type-db2/jcl/MNTTRDB2.jcl:L21-30], here the `--input` file
//! (P-COBTUPDT-004).
//!
//! The COBTUPDT paragraphs fall through in their source order, and the job keeps it:
//!
//! 1. `0001-OPEN-FILES`: [`load_records`] reads the input one line at a time, never holding
//!    more than [`LINE_LIMIT`] bytes of a line, and frames every 53-byte record on Tokio's
//!    blocking pool, inside the job span, before any change. A file that cannot be read, or
//!    whose records do not fit `WS-INPUT-REC`, ends the job with exit 16 at the first such
//!    record (P-COBTUPDT-011, E-36), so an endless input such as `/dev/zero` fails at once.
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
//! success displays, `warn` for the `9999-ABEND` display. The job's counts receive `read` and
//! `rejected` (failed records) once every record is treated, before the `STOP RUN` commit, and
//! then `posted` (inserts, updates and deletes), and the job span `written` (inserts and
//! updates) and `deleted`, as the commit leaves the master: the statements' counts when it
//! commits, 0 when it fails and every statement is undone, the undone counts then going to one
//! `warn` event. A run that ends before its records are treated leaves every count 0.

use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use carddemo_codec::layouts::{CodecError, pad};
use carddemo_domain::messages::OPEN_FILE_NOT_OK;
use carddemo_domain::rules::tran_type_batch::{
    self, INPUT_REC_LEN, MaintRecord, MaintRun, RecordOutcome,
};
use carddemo_persistence::tx::{Db, UnitOfWork};
use tracing::Span;

use super::{JobContext, count_of, setup_failure, span_count};
use crate::cli::Mnttrdb2Args;
use crate::exit::ExitStatus;
use crate::generations::blocking;

/// The label an over-long `INPFILE` record is reported under: the program whose 53-byte
/// `WS-INPUT-REC` it does not fit.
const INPUT_LAYOUT: &str = "COBTUPDT";

/// The most bytes [`parse_input`] reads of one `INPFILE` line: the 53 bytes of `WS-INPUT-REC`,
/// the two of a `CR LF` line end, and one more. A line read whole within them has its length
/// measured; a line with no `LF` among them is longer than the record whatever follows, so it is
/// refused without reading any more of it.
const LINE_LIMIT: usize = INPUT_REC_LEN + b"\r\n".len() + 1;

/// Why the records of an `INPFILE` cannot be loaded. The text follows `INPFILE <path>: ` in the
/// setup failure.
#[derive(Debug, thiserror::Error)]
enum InputError {
    /// The file cannot be read: the operating-system error, as the open reports it.
    #[error(transparent)]
    Read(#[from] io::Error),
    /// A line read whole whose body is longer than `WS-INPUT-REC`: the codec error with the
    /// measured length, located at its 1-based record number.
    #[error(transparent)]
    Record(#[from] CodecError),
    /// A line with no `LF` within its first [`LINE_LIMIT`] bytes, at the 1-based record
    /// number `record`. Only those bytes were read, so the text gives no length.
    #[error(
        "{layout} record {record}: line longer than the {len}-byte record length, with no line \
         end within its first {limit} bytes",
        layout = INPUT_LAYOUT,
        len = INPUT_REC_LEN,
        limit = LINE_LIMIT
    )]
    Unbounded {
        /// The 1-based record number of the line.
        record: usize,
    },
}

/// COBTUPDT 0001-OPEN-FILES through STOP RUN — P-COBTUPDT-004 to P-COBTUPDT-012, E-36.
///
/// Runs MNTTRDB2 over `args.input`: loads every record, applies each to the Db2 transaction
/// type master in file order inside one unit of work, commits it, and returns
/// [`ExitStatus::ReturnCode`] with 4 when any record failed, else 0.
///
/// The job reads no setting, clock or generation; of the context it uses only the job's counts.
/// [`load_records`] runs on Tokio's blocking pool, inside the job span, before the unit of work
/// begins.
///
/// Each of these ends the job with a setup failure (exit 16):
///
/// - the input cannot be read or framed (P-COBTUPDT-011, E-36), or the runtime shuts down
///   before it is read, before any change and with every count 0;
/// - the unit of work cannot begin, before any change and with every count 0;
/// - the commit at `STOP RUN` fails, which leaves every statement of the run undone. The job's
///   counts keep the `read` and `rejected` set before the commit and `posted` stays 0, the job
///   span records `written` and `deleted` as 0, and one `warn` event carries the inserts,
///   updates and deletes undone.
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &Mnttrdb2Args) -> ExitStatus {
    let input = args.input.clone();
    let records = match blocking(move || load_records(&input)).await {
        Ok(Ok(records)) => records,
        Ok(Err(status)) => return status,
        Err(cancelled) => return setup_failure("INPFILE read did not complete", cancelled),
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
    ctx.counts.set_read(count_of(counts.read));
    ctx.counts.set_rejected(count_of(counts.failed));
    let span = Span::current();
    if let Err(err) = uow.commit().await {
        let status = setup_failure("MNTTRDB2 STOP RUN commit failed", err);
        span.record("written", 0_i64);
        span.record("deleted", 0_i64);
        tracing::warn!(
            undone_written = counts.written,
            undone_deleted = counts.deleted,
            written = 0_usize,
            deleted = 0_usize,
            "MNTTRDB2 STOP RUN commit failed: every statement undone, 0 types written or deleted"
        );
        return status;
    }
    ctx.counts.set_posted(count_of(counts.applied()));
    span.record("written", span_count(counts.written));
    span.record("deleted", span_count(counts.deleted));
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
/// The file is read through a [`BufReader`] one line at a time by [`parse_input`], as COBTUPDT
/// reads one record at a time, so memory holds the framed records and never more than
/// [`LINE_LIMIT`] bytes of the line being read: a device or a file without line ends, such as
/// `/dev/zero`, fails at its first record instead of being read whole.
///
/// # Errors
///
/// The setup failure (exit 16) of a file that cannot be opened or read or holds a record longer
/// than 53 bytes, logged under [`OPEN_FILE_NOT_OK`] with the path and the cause.
fn load_records(path: &Path) -> Result<Vec<MaintRecord>, ExitStatus> {
    let file = File::open(path).map_err(|err| open_failure(path, &err))?;
    let records = parse_input(BufReader::new(file)).map_err(|err| open_failure(path, &err))?;
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
/// Reads `reader` line by line, each line through a [`LINE_LIMIT`]-byte window, pads each body
/// with spaces to the 53 bytes of `WS-INPUT-REC` and parses it. The lines are framed as
/// `carddemo_codec::framing::split_ascii` frames a whole file: a line ends at `LF`; a `CR`
/// directly before that `LF` belongs to the line end and any other `CR` is record data; a final
/// line without `LF` is a record; nothing after the last `LF` is no record, so an empty file has
/// none and the run ends at once with return code 0. An empty line is a record of spaces, which
/// `1003-TREAT-RECORD` rejects as an invalid type. Every byte of a body is kept as read.
///
/// # Errors
///
/// For the first line that cannot be a record, located at its 1-based record number, nothing
/// after it being read:
///
/// - [`InputError::Record`], the [`CodecError`] with the measured length, for a body longer than
///   53 bytes whose line ended within [`LINE_LIMIT`] bytes;
/// - [`InputError::Unbounded`] for a line with no `LF` within [`LINE_LIMIT`] bytes;
/// - [`InputError::Read`] when `reader` fails.
fn parse_input<R: BufRead>(mut reader: R) -> Result<Vec<MaintRecord>, InputError> {
    // A usize of at most 56 always fits the u64 that `Read::take` counts in.
    let window = LINE_LIMIT as u64;
    let mut records = Vec::new();
    let mut line = Vec::with_capacity(LINE_LIMIT);
    loop {
        line.clear();
        let read = reader.by_ref().take(window).read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(records);
        }
        let number = records.len() + 1;
        let body = match line.strip_suffix(b"\n") {
            Some(ended) => ended.strip_suffix(b"\r").unwrap_or(ended),
            None if read == LINE_LIMIT => return Err(InputError::Unbounded { record: number }),
            None => line.as_slice(),
        };
        let padded = pad(INPUT_LAYOUT, body, INPUT_REC_LEN).map_err(|err| err.at_record(number))?;
        records.push(tran_type_batch::parse_record(&padded));
    }
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

/// The record counts the job reports: `read`, `failed` (as `rejected`) and [`Counts::applied`]
/// (as `posted`) in the job's counts, `written` and `deleted` on the job span.
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
    /// The records whose statement changed the master: the types inserted, updated or deleted.
    const fn applied(&self) -> usize {
        self.written + self.deleted
    }

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
    use std::fs;
    use std::io::{self, BufReader, Read};
    use std::path::{Path, PathBuf};

    use carddemo_codec::CodecError;
    use carddemo_codec::framing::{pad, split_ascii};
    use carddemo_domain::messages::{NO_RECORDS_FOUND, PROCESSING};
    use carddemo_domain::rules::tran_type_batch::{
        INPUT_REC_LEN, MaintRecord, RecordOutcome, parse_record,
    };

    use super::{Counts, INPUT_LAYOUT, InputError, LINE_LIMIT, load_records, parse_input};
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
            format!("{PROCESSING}A08BALANCE TRANSFER")
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

    /// The records `split_ascii` frames in the whole of `bytes`, each padded and parsed: the
    /// framing [`parse_input`] keeps while it reads line by line.
    fn split_ascii_records(bytes: &[u8]) -> Result<Vec<MaintRecord>, CodecError> {
        split_ascii(bytes)
            .into_iter()
            .enumerate()
            .map(|(index, (_frame, body))| {
                pad(INPUT_LAYOUT, body, INPUT_REC_LEN)
                    .map(|padded| parse_record(&padded))
                    .map_err(|err| err.at_record(index + 1))
            })
            .collect()
    }

    /// A line of `len` bytes: `A10` followed by spaces.
    fn line_of(len: usize) -> String {
        format!("{:<len$}", "A10")
    }

    #[test]
    fn line_by_line_framing_is_the_split_ascii_framing() {
        let inputs: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"\n".to_vec(),
            b"\n\n".to_vec(),
            b"\r\n".to_vec(),
            b"\r".to_vec(),
            b"A01\r".to_vec(),
            b"A01\rX\n".to_vec(),
            b"A01\r\r\n".to_vec(),
            b"U01PURCHASE\r\nD02".to_vec(),
            b"*\n\nD04\n".to_vec(),
            // Bytes outside ASCII are record data, kept byte for byte.
            b"A0\xC3\xA9 CAF\xC3\x89 \xFF\x00\n".to_vec(),
            format!("{}\n", line_of(53)).into_bytes(),
            format!("{}\r\n", line_of(53)).into_bytes(),
            line_of(53).into_bytes(),
            format!("{}\n{}\nD10\n", line_of(53), line_of(54)).into_bytes(),
            FIXTURE.to_vec(),
        ];
        for input in &inputs {
            let read = match parse_input(input.as_slice()) {
                Ok(records) => Ok(records),
                Err(InputError::Record(err)) => Err(err),
                Err(other) => panic!("{:?}: {other}", String::from_utf8_lossy(input)),
            };
            assert_eq!(
                read,
                split_ascii_records(input),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn a_record_longer_than_ws_input_rec_is_located_and_refused() {
        let unbounded = |record: usize| {
            format!(
                "COBTUPDT record {record}: line longer than the 53-byte record length, with no \
                 line end within its first 56 bytes"
            )
        };
        let measured = |record: usize, len: usize| {
            format!("COBTUPDT record {record}: stored length {len} exceeds the record length")
        };
        // A line read whole within the 56-byte window reports its length; a longer one does not.
        let cases = [
            (
                format!("{}\n{}\nD10\n", line_of(53), line_of(54)),
                measured(2, 54),
            ),
            (format!("{}\r\n", line_of(54)), measured(1, 54)),
            (format!("{}\n", line_of(55)), measured(1, 55)),
            (line_of(54), measured(1, 54)),
            (line_of(55), measured(1, 55)),
            // A CR that no LF follows is record data.
            (format!("{}\r", line_of(53)), measured(1, 54)),
            (format!("{}\r\n", line_of(55)), unbounded(1)),
            (format!("{}\n", line_of(56)), unbounded(1)),
            (line_of(56), unbounded(1)),
            (format!("D01\n{}\nD02\n", line_of(1_000)), unbounded(2)),
        ];
        for (input, expected) in cases {
            let err = parse_input(input.as_bytes())
                .err()
                .unwrap_or_else(|| panic!("{input:?} must be refused"));
            assert_eq!(err.to_string(), expected, "{input:?}");
        }
    }

    /// A reader that adds every byte `inner` hands out to `bytes`.
    struct Counted<'a, R> {
        inner: R,
        bytes: &'a mut usize,
    }

    impl<R: Read> Read for Counted<'_, R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buf)?;
            *self.bytes += read;
            Ok(read)
        }
    }

    #[test]
    fn an_endless_line_is_refused_after_one_window() {
        // (records before the endless line, the byte it repeats, its record number)
        let cases: [(&[u8], u8, usize); 3] =
            [(b"", 0, 1), (b"D01\n", b'X', 2), (b"*\r\n", b'\r', 2)];
        for (before, byte, record) in cases {
            let mut bytes = 0;
            let reader = Counted {
                inner: before.chain(io::repeat(byte)),
                bytes: &mut bytes,
            };
            let refused = parse_input(BufReader::with_capacity(LINE_LIMIT, reader));
            assert!(
                matches!(refused, Err(InputError::Unbounded { record: at }) if at == record),
                "{before:?} then {byte:#04X} forever: {refused:?}"
            );
            assert!(
                bytes <= before.len() + LINE_LIMIT,
                "{before:?} then {byte:#04X} forever: {bytes} bytes read"
            );
        }
    }

    #[test]
    fn inputs_that_cannot_be_framed_are_setup_failures_naming_the_file() {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let absent = fixtures.join("mnttrdb2_input.absent");
        let os_error = |path: &Path| {
            fs::read(path)
                .err()
                .unwrap_or_else(|| panic!("{} must not be readable", path.display()))
                .to_string()
        };
        let cases = [
            (absent.clone(), os_error(&absent)),
            (fixtures.clone(), os_error(&fixtures)),
            (
                PathBuf::from("/dev/zero"),
                "COBTUPDT record 1: line longer than the 53-byte record length, with no line end \
                 within its first 56 bytes"
                    .to_owned(),
            ),
        ];
        for (path, cause) in cases {
            let status = load_records(&path).err();
            assert_eq!(
                status,
                Some(ExitStatus::SetupFailure(format!(
                    "INPFILE {}: {cause}",
                    path.display()
                ))),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn readable_inputs_load_their_records() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mnttrdb2_input.txt");
        let cases = [(fixture, 10), (PathBuf::from("/dev/null"), 0)];
        for (path, count) in cases {
            let loaded = load_records(&path)
                .unwrap_or_else(|status| panic!("{}: {status:?}", path.display()));
            assert_eq!(loaded.len(), count, "{}", path.display());
        }
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
