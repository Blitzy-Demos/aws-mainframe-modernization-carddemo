//! The service of endpoint 14, `POST /api/v1/reports/transactions` (transaction `CR00`, program
//! CORPT00C): the report selection, its dates and the confirmation, then the job deck written to
//! a local generation file in place of TDQ `JOBS` (AAP 0.3.4.4).
//!
//! # Flow
//!
//! 1. The ten inputs of `CORPT0A` reach `domain::online::corpt00c::edit` as their raw text, the
//!    raw-edit-input exception of Typed Dates and Timestamps, with the state's injected clock for
//!    the Monthly and Yearly windows. The first edit that fails is the answer, and nothing is
//!    written (CORPT00C-006). Which custom dates pass is `HALTED(H5)` and stays in the domain.
//! 2. `domain::online::corpt00c::confirm` reads `confirm`: blank and unknown values are the
//!    audited 400s, `N` or `n` answers an empty message with no deck (CORPT00C-032), and `Y` or
//!    `y` submits (CORPT00C-031).
//! 3. A submission writes the 17 records of `JOB-DATA-1`, `/*EOF` the last (CORPT00C-034,
//!    CORPT00C-035), to `TRNRPT00.G<nnnn>` in `CARDDEMO_INTRDR_DIR`: the generation
//!    `runtime::gdg::next_generation` names, created exclusively, each record its 80 bytes and a
//!    line feed (CORPT00C-036). The answer is the submission message and the file's name alone
//!    (CORPT00C-038).
//! 4. A generation that cannot be named, created or written is `Unable to Write TDQ (JOBS)...`
//!    with status 500 (CORPT00C-037). What the program `DISPLAY`s to the job log, its `RESP` and
//!    `RESP2`, becomes one error event carrying the step and the I/O error kind only.
//!
//! # Boundaries
//!
//! - The endpoint reads and writes no table and runs no report (CORPT00C-039); the operator runs
//!   `carddemo-batch tranrept`, as JES ran the submitted job. This module is the only part of the
//!   crate's request path that touches the filesystem (AAP 0.5.3), and the deck goes to the local
//!   configured directory only, with no stream, object store or broker (Local-Only Runtime).
//! - Neither the response nor any event names the directory or the full path, only the
//!   generation file's name, and no report or statement content leaves (Cardholder Data
//!   Redaction).
//! - Every request writes a file of its own, so two decks never interleave. When two submissions
//!   race for one generation, the exclusive create of the second fails, and it names the
//!   generation again, one above the first's, and writes its deck there: each is answered with
//!   its own generation and the submission message, as `WRITEQ TD` accepts every write, and a
//!   generation file is never overwritten (CORPT00C-036, CORPT00C-038). What JES makes of decks
//!   whose records interleave stays halt candidate X-CORPT00C-H2.
//! - A deck whose write fails after its file was created is removed, so no incomplete deck is
//!   left for a reader to pick up. The bytes a failed COBOL write leaves are halt candidate
//!   X-CORPT00C-H2 and no parity claim rests on them.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::iter;
use std::path::Path;

use domain::clock::Clock;
use domain::copybook::PicX;
use domain::online::corpt00c::{
    self, ConfirmOutcome, DECK_RECORD_LEN, ReportFailure, ReportInput, ReportKind,
};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
// The domain's `ReportRequest` is the edited report, so the request DTO takes another name here.
use crate::dto::corpt00c::ReportRequest as ReportRequestDto;
use crate::error::ServiceError;

/// The generation base of the job deck: the job name `TRNRPT00` of record 1 (CORPT00C-034,
/// CORPT00C-036).
const DECK_BASE: &str = "TRNRPT00";

/// The terminator after every 80-byte record, as every generation file holds its fixed-length
/// records (AAP 0.3.4.3).
const RECORD_TERMINATOR: u8 = b'\n';

/// The setting that names the job-deck directory, as the events and errors about that directory
/// name it in place of its path: `CARDDEMO_INTRDR_DIR`, which defaults to `jobs` under
/// `CARDDEMO_VAR_DIR` (`runtime::config`).
pub(crate) const DECK_DIR_SETTING: &str =
    "CARDDEMO_INTRDR_DIR, or jobs under CARDDEMO_VAR_DIR when CARDDEMO_INTRDR_DIR is unset";

/// Submit the transaction report: the edits, the confirmation and, when confirmed, the job deck
/// (CORPT00C-006 to CORPT00C-039).
///
/// Returns `(message, job_deck)`: the submission message with the generation file's name, such
/// as `("Monthly report submitted for printing ...", Some("TRNRPT00.G0001"))` (CORPT00C-038), or
/// the empty message of a cleared screen with `None` (CORPT00C-032). The acting user is verified
/// by the extractor and decides nothing here.
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the audited [`ReportFailure`]: a failed edit or confirmation
///   (400; CORPT00C-007 to CORPT00C-033), or `Unable to Write TDQ (JOBS)...` when the generation
///   file cannot be named, created or written (500; CORPT00C-037).
/// - [`ServiceError::Internal`]: the generation file's name is not UTF-8, which leaves nothing
///   written, or the writer task ended without an answer.
pub(crate) async fn submit(
    state: &AppState,
    _user: &SignedOnUser,
    request: ReportRequestDto,
) -> Result<(String, Option<String>), ServiceError<ReportFailure>> {
    let report = match decide(request, &*state.clock).map_err(ServiceError::Rule)? {
        ConfirmOutcome::Submit(report) => report,
        ConfirmOutcome::Cleared => return Ok((String::new(), None)),
    };
    let deck = deck_bytes(&corpt00c::job_deck(&report));
    let dir = state.intrdr_dir.clone();
    // The writes block, so they run off the async workers. A blocking task is not cancelled when
    // its request is dropped, so a dropped request still ends with a whole deck or none.
    let written = tokio::task::spawn_blocking(move || write_deck(&dir, &deck))
        .await
        .unwrap_or(Err(DeckError::Task));
    answer(report.kind(), written)
}

/// The edits and the confirmation of `request`, read from its ten inputs as typed
/// (CORPT00C-007 to CORPT00C-033, CORPT00C-042).
fn decide(request: ReportRequestDto, clock: &dyn Clock) -> Result<ConfirmOutcome, ReportFailure> {
    let ReportRequestDto {
        monthly,
        yearly,
        custom,
        start_month,
        start_day,
        start_year,
        end_month,
        end_day,
        end_year,
        confirm,
    } = request;
    let input = ReportInput {
        monthly: &monthly,
        yearly: &yearly,
        custom: &custom,
        start_month: &start_month,
        start_day: &start_day,
        start_year: &start_year,
        end_month: &end_month,
        end_day: &end_day,
        end_year: &end_year,
        confirm: &confirm,
    };
    let report = corpt00c::edit(&input, clock)?;
    corpt00c::confirm(report, input.confirm)
}

/// The deck as its file holds it: every record's 80 bytes followed by a line feed, in order
/// (CORPT00C-035, CORPT00C-036).
fn deck_bytes(records: &[PicX<DECK_RECORD_LEN>]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| {
            record
                .as_bytes()
                .iter()
                .copied()
                .chain(iter::once(RECORD_TERMINATOR))
        })
        .collect()
}

/// The step of a deck write that failed, as the error event names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeckStep {
    /// Naming the generation: creating or reading the directory.
    Generation,
    /// Creating the generation file exclusively.
    Create,
    /// Writing the records and syncing them to disk.
    Write,
}

impl DeckStep {
    /// The step's name in the error event.
    const fn name(self) -> &'static str {
        match self {
            Self::Generation => "generation",
            Self::Create => "create",
            Self::Write => "write",
        }
    }
}

/// Why no deck was submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeckError {
    /// A step of the write failed: the `WRITEQ TD` failure of CORPT00C-037. `removed` tells
    /// whether the generation file this request created is gone; it is `true` when the step
    /// created none.
    Io {
        step: DeckStep,
        kind: io::ErrorKind,
        removed: bool,
    },
    /// The generation file's name is not UTF-8, so it could not be answered; nothing was
    /// created.
    FileName,
    /// The writer task ended without an answer.
    Task,
}

impl DeckError {
    /// A failure of `step` before any file was created.
    fn before_create(step: DeckStep, error: &io::Error) -> Self {
        Self::Io {
            step,
            kind: error.kind(),
            removed: true,
        }
    }
}

/// Writes `deck` to the next generation of [`DECK_BASE`] in `dir` and returns the generation
/// file's name (CORPT00C-036).
///
/// The name is checked before the file is created, so a name that cannot be answered leaves
/// nothing written. A file created and then not fully written is removed.
///
/// `WRITEQ TD` accepts the deck of every concurrent `CR00` task, so a request whose exclusive
/// create is refused because another request took the generation first names the generation
/// again and creates that one; nothing is ever overwritten. It retries only while [`retries`]
/// allows: each refused name must be a later generation than the one refused before it. A
/// concurrent request always leaves a later name to take, because the rescan counts the
/// generation it took, while generation numbers end at `runtime::gdg::MAX_GENERATION`, after
/// which the generation step fails. So a request makes at most one attempt per generation number,
/// and a refusal that names no later generation, which no concurrent deck explains, is the write
/// failure of CORPT00C-037.
fn write_deck(dir: &Path, deck: &[u8]) -> Result<String, DeckError> {
    let mut last_refused: Option<String> = None;
    loop {
        let path = runtime::gdg::next_generation(dir, DECK_BASE)
            .map_err(|error| DeckError::before_create(DeckStep::Generation, &error))?;
        let file_name = path
            .file_name()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .ok_or(DeckError::FileName)?;
        let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    && retries(&file_name, last_refused.as_deref()) =>
            {
                last_refused = Some(file_name);
                continue;
            }
            Err(error) => return Err(DeckError::before_create(DeckStep::Create, &error)),
        };
        write_records(file, deck).map_err(|error| DeckError::Io {
            step: DeckStep::Write,
            kind: error.kind(),
            removed: fs::remove_file(&path).is_ok(),
        })?;
        return Ok(file_name);
    }
}

/// Whether a request whose exclusive create of generation file `refused` found it taken names
/// the generation again: only when no create was refused before, or `refused` is a later
/// generation than `last_refused`, the name refused before it (CORPT00C-036).
///
/// Both names are [`DECK_BASE`], `.G` and four digits, so their order as text is the order of
/// their generation numbers. A name no later than the one refused before means that what stands
/// in the way is not a concurrent deck: a name the generation scan does not count, such as a
/// differently cased name on a case-insensitive filesystem, or a generation removed again after a
/// failed write.
fn retries(refused: &str, last_refused: Option<&str>) -> bool {
    last_refused.is_none_or(|last| refused > last)
}

/// Checks, before `carddemo-api` serves, that `dir` can hold job decks: an existing directory,
/// or no entry at all, which the first submission creates with its missing parents.
///
/// Only an entry that can never be the directory is refused: `dir` exists and is no directory,
/// a dangling link included, or a path through something that is not one. Any other failure to
/// read it, such as a permission, is left to each submission, which answers it with CORPT00C-037
/// and an event naming [`DECK_DIR_SETTING`], so the server keeps serving every other endpoint.
/// The check reads the entry once; a directory replaced by a file later still gets that answer.
///
/// # Errors
///
/// `io::ErrorKind::NotADirectory` for a refused entry. Only the kind is kept: the error's own
/// text could name the path.
pub(crate) fn check_deck_dir(dir: &Path) -> Result<(), io::ErrorKind> {
    match fs::metadata(dir) {
        Ok(found) if found.is_dir() => Ok(()),
        Ok(_) => Err(io::ErrorKind::NotADirectory),
        // Nothing there, unless a link stands in the directory's place and leads nowhere.
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::symlink_metadata(dir) {
            Ok(_) => Err(io::ErrorKind::NotADirectory),
            Err(_) => Ok(()),
        },
        Err(error) if error.kind() == io::ErrorKind::NotADirectory => {
            Err(io::ErrorKind::NotADirectory)
        }
        Err(_) => Ok(()),
    }
}

/// Writes `deck` to `file`, syncs it and closes it.
///
/// `sync_all` rather than `flush`, which does nothing on a `File`: a deck answered as submitted
/// is on disk, and a write error the filesystem defers surfaces here rather than after the
/// answer.
fn write_records(mut file: File, deck: &[u8]) -> io::Result<()> {
    file.write_all(deck)?;
    file.sync_all()
}

/// The answer to a confirmed request of report `kind`, from the outcome of its deck write
/// (CORPT00C-037, CORPT00C-038).
///
/// A failed step is logged with the step and the I/O error kind, the counterpart of the
/// program's `DISPLAY` of `RESP` and `RESP2`, and with [`DECK_DIR_SETTING`], the setting the
/// operator changes, never with the directory or a path.
fn answer(
    kind: ReportKind,
    written: Result<String, DeckError>,
) -> Result<(String, Option<String>), ServiceError<ReportFailure>> {
    match written {
        Ok(file_name) => Ok((corpt00c::submitted_message(kind), Some(file_name))),
        Err(DeckError::Io {
            step,
            kind: io_kind,
            removed,
        }) => {
            tracing::error!(
                step = step.name(),
                io_error = %io_kind,
                partial_deck_removed = removed,
                setting = DECK_DIR_SETTING,
                "job deck not written"
            );
            Err(ServiceError::Rule(corpt00c::tdq_write_failure()))
        }
        Err(DeckError::FileName) => {
            tracing::error!("job deck not written: generation file name is not UTF-8");
            Err(ServiceError::Internal)
        }
        Err(DeckError::Task) => {
            tracing::error!("job deck writer task ended without an answer");
            Err(ServiceError::Internal)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Barrier;
    use std::thread;

    use chrono::NaiveDate;
    use domain::FixedClock;
    use domain::online::corpt00c::{
        self, ConfirmOutcome, DECK_RECORD_COUNT, DECK_RECORD_LEN, ReportKind,
    };
    use test_support::trace_capture;

    use super::{
        DECK_DIR_SETTING, DeckError, DeckStep, ReportRequestDto, answer, check_deck_dir, decide,
        deck_bytes, retries, write_deck,
    };
    use crate::error::{ScreenError, ServiceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The deck of a Monthly report under the `FixedClock` instant 2022-07-18, record by record
    /// without padding (CORPT00C-034, CORPT00C-008, CORPT00C-009).
    const MONTHLY_DECK: [&str; DECK_RECORD_COUNT] = [
        "//TRNRPT00 JOB 'TRAN REPORT',CLASS=A,MSGCLASS=0,",
        "// NOTIFY=&SYSUID",
        "//*",
        "//JOBLIB JCLLIB ORDER=('AWS.M2.CARDDEMO.PROC')",
        "//*",
        "//STEP10 EXEC PROC=TRANREPT",
        "//*",
        "//STEP05R.SYMNAMES DD *",
        "TRAN-CARD-NUM,263,16,ZD",
        "TRAN-PROC-DT,305,10,CH",
        "PARM-START-DATE,C'2022-07-01'",
        "PARM-END-DATE,C'2022-07-31'",
        "/*",
        "//STEP10R.DATEPARM DD *",
        "2022-07-01 2022-07-31",
        "/*",
        "/*EOF",
    ];

    /// A directory private to one test of this process, absent at creation and removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(test_name: &str) -> io::Result<Self> {
            let path = std::env::temp_dir().join(format!(
                "carddemo-corpt00c-{}-{test_name}",
                std::process::id()
            ));
            remove_tree(&path)?;
            Ok(Self(path))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            if let Err(error) = remove_tree(&self.0) {
                tracing::error!(
                    operation = "remove scratch directory",
                    io_error = %error.kind(),
                    "scratch directory not removed"
                );
            }
        }
    }

    /// Removes `path` and everything below it; an absent path is no error.
    fn remove_tree(path: &Path) -> io::Result<()> {
        match fs::remove_dir_all(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn clock() -> Result<FixedClock, &'static str> {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_opt(10, 30, 0))
            .map(FixedClock::new)
            .ok_or("invalid test instant")
    }

    fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, &'static str> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or("invalid test date")
    }

    /// A Monthly request with `confirm`.
    fn monthly(confirm: &str) -> ReportRequestDto {
        ReportRequestDto {
            monthly: "X".to_owned(),
            confirm: confirm.to_owned(),
            ..ReportRequestDto::default()
        }
    }

    /// The deck of a confirmed Monthly request, as its file holds it.
    fn monthly_deck() -> Result<Vec<u8>, Box<dyn StdError>> {
        match decide(monthly("Y"), &clock()?)? {
            ConfirmOutcome::Submit(report) => Ok(deck_bytes(&corpt00c::job_deck(&report))),
            ConfirmOutcome::Cleared => Err("a confirmed request was cleared".into()),
        }
    }

    /// The failure message `decide` answers `request` with.
    fn refusal(request: ReportRequestDto) -> Result<String, Box<dyn StdError>> {
        let failure = decide(request, &clock()?)
            .err()
            .ok_or("the request was not refused")?;
        Ok(failure.message().to_owned())
    }

    #[test]
    fn the_edits_and_the_confirmation_decide_before_anything_is_written() -> TestResult {
        // audit: CORPT00C-029
        assert_eq!(
            refusal(ReportRequestDto {
                confirm: "Y".to_owned(),
                ..ReportRequestDto::default()
            })?,
            "Select a report type to print report..."
        );
        // audit: CORPT00C-030
        assert_eq!(
            refusal(monthly(""))?,
            "Please confirm to print the Monthly report..."
        );
        // audit: CORPT00C-033
        assert_eq!(
            refusal(monthly("X"))?,
            "\"X\" is not a valid value to confirm..."
        );
        // audit: CORPT00C-032
        assert_eq!(decide(monthly("n"), &clock()?)?, ConfirmOutcome::Cleared);

        // audit: CORPT00C-031
        let ConfirmOutcome::Submit(report) = decide(monthly("Y"), &clock()?)? else {
            return Err("a confirmed Monthly request was not submitted".into());
        };
        assert_eq!(report.kind(), ReportKind::Monthly);
        assert_eq!(
            (report.start(), report.end()),
            (date(2022, 7, 1)?, date(2022, 7, 31)?)
        );

        // audit: CORPT00C-010
        let yearly = ReportRequestDto {
            yearly: "X".to_owned(),
            confirm: "y".to_owned(),
            ..ReportRequestDto::default()
        };
        let ConfirmOutcome::Submit(report) = decide(yearly, &clock()?)? else {
            return Err("a confirmed Yearly request was not submitted".into());
        };
        assert_eq!(
            (report.kind(), report.start(), report.end()),
            (ReportKind::Yearly, date(2022, 1, 1)?, date(2022, 12, 31)?)
        );
        Ok(())
    }

    #[test]
    fn the_deck_is_seventeen_records_of_eighty_bytes_each_ended_by_a_line_feed() -> TestResult {
        // audit: CORPT00C-034
        // audit: CORPT00C-035
        // audit: CORPT00C-036
        let deck = monthly_deck()?;
        let expected: Vec<u8> = MONTHLY_DECK
            .iter()
            .flat_map(|text| format!("{text:<DECK_RECORD_LEN$}\n").into_bytes())
            .collect();
        assert_eq!(deck, expected);
        assert_eq!(deck.len(), 1377);
        let records: Vec<&[u8]> = deck.split_inclusive(|byte| *byte == b'\n').collect();
        assert_eq!(records.len(), DECK_RECORD_COUNT);
        assert!(
            records
                .iter()
                .all(|record| record.len() == DECK_RECORD_LEN + 1 && record.ends_with(b"\n"))
        );
        Ok(())
    }

    #[test]
    fn each_deck_takes_the_next_generation_file_and_answers_its_name_only() -> TestResult {
        let scratch = ScratchDir::new("generations")?;
        let deck = monthly_deck()?;

        // audit: CORPT00C-036
        let first = write_deck(scratch.path(), &deck);
        assert_eq!(first, Ok("TRNRPT00.G0001".to_owned()));
        assert_eq!(fs::read(scratch.path().join("TRNRPT00.G0001"))?, deck);
        let second = write_deck(scratch.path(), &deck);
        assert_eq!(second, Ok("TRNRPT00.G0002".to_owned()));
        assert_eq!(fs::read(scratch.path().join("TRNRPT00.G0002"))?, deck);
        assert_eq!(fs::read(scratch.path().join("TRNRPT00.G0001"))?, deck);

        // audit: CORPT00C-038
        let (message, job_deck) = answer(ReportKind::Monthly, second)
            .map_err(|_| "a written deck was not answered as submitted")?;
        assert_eq!(message, "Monthly report submitted for printing ...");
        let job_deck = job_deck.ok_or("no job_deck")?;
        assert_eq!(job_deck, "TRNRPT00.G0002");
        assert!(!job_deck.contains(['/', '\\']));
        Ok(())
    }

    /// Decks written at once from many threads into one directory are each submitted: a write
    /// whose generation another took first names the next one, so the names are exactly
    /// `G0001` onward and every file holds one whole deck.
    #[test]
    fn concurrent_decks_each_take_a_generation_of_their_own() -> TestResult {
        const WRITERS: usize = 32;
        let scratch = ScratchDir::new("concurrent")?;
        let deck = monthly_deck()?;
        let start = Barrier::new(WRITERS);
        let written: Vec<Result<String, DeckError>> = thread::scope(|scope| {
            let writers: Vec<_> = (0..WRITERS)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        write_deck(scratch.path(), &deck)
                    })
                })
                .collect();
            writers
                .into_iter()
                .map(|writer| writer.join().unwrap_or(Err(DeckError::Task)))
                .collect()
        });

        // audit: CORPT00C-036
        let mut names = written
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("a concurrent deck was not submitted: {error:?}"))?;
        names.sort_unstable();
        let expected: Vec<String> = (1..=WRITERS)
            .map(|generation| format!("TRNRPT00.G{generation:04}"))
            .collect();
        assert_eq!(names, expected);
        for name in &expected {
            assert_eq!(fs::read(scratch.path().join(name))?, deck, "{name}");
        }
        assert_eq!(fs::read_dir(scratch.path())?.count(), WRITERS);
        Ok(())
    }

    /// A refused create is retried only while each refused name is a later generation than the
    /// one refused before it.
    #[test]
    fn a_refused_generation_is_retried_only_while_the_names_advance() {
        // audit: CORPT00C-036
        // audit: CORPT00C-037
        let cases = [
            ("TRNRPT00.G0001", None, true),
            ("TRNRPT00.G0010", Some("TRNRPT00.G0009"), true),
            ("TRNRPT00.G9999", Some("TRNRPT00.G0001"), true),
            ("TRNRPT00.G0009", Some("TRNRPT00.G0009"), false),
            ("TRNRPT00.G0008", Some("TRNRPT00.G0009"), false),
        ];
        for (refused, last_refused, retried) in cases {
            assert_eq!(
                retries(refused, last_refused),
                retried,
                "{refused} after {last_refused:?}"
            );
        }
    }

    /// A regular file in place of the job-deck directory, or of one of its ancestors, is the TDQ
    /// write failure. Its event names the generation step, the true cause and the setting the
    /// operator changes, never the path, and the file is left as it was.
    #[test]
    fn a_directory_that_cannot_hold_generations_is_the_tdq_write_failure() -> TestResult {
        let scratch = ScratchDir::new("regular-file")?;
        fs::create_dir_all(scratch.path())?;
        let not_a_directory = scratch.path().join("jobs");
        fs::write(&not_a_directory, b"")?;

        for dir in [not_a_directory.clone(), not_a_directory.join("decks")] {
            let traces = trace_capture::capture();
            // audit: CORPT00C-037
            let written = write_deck(&dir, &monthly_deck()?);
            assert_eq!(
                written,
                Err(DeckError::Io {
                    step: DeckStep::Generation,
                    kind: io::ErrorKind::NotADirectory,
                    removed: true,
                })
            );
            assert_eq!(fs::read(&not_a_directory)?, b"");
            let answered = answer(ReportKind::Monthly, written);
            assert_eq!(
                answered,
                Err(ServiceError::Rule(corpt00c::tdq_write_failure()))
            );
            let error = answered
                .err()
                .map(ScreenError::from)
                .ok_or("the failure was answered as a success")?;
            assert_eq!(
                (error.status().as_u16(), error.message(), error.field()),
                (500, "Unable to Write TDQ (JOBS)...", Some("monthly"))
            );
            for field in [
                r#""step":"generation""#.to_owned(),
                r#""io_error":"not a directory""#.to_owned(),
                format!(r#""setting":"{DECK_DIR_SETTING}""#),
            ] {
                assert!(traces.contains(&field), "the event lacks {field}");
            }
            let shown = scratch.path().to_string_lossy();
            assert!(!traces.contains(&shown), "an event names the directory");
        }
        Ok(())
    }

    /// The startup check refuses only an entry that can never be the job-deck directory: a
    /// regular file, a path through one, or a link to nothing. An existing directory and a
    /// missing one, which the first submission creates, pass, and nothing is created.
    #[test]
    fn the_startup_check_refuses_only_what_can_never_be_the_directory() -> TestResult {
        let scratch = ScratchDir::new("startup-check")?;
        fs::create_dir_all(scratch.path().join("decks"))?;
        let file = scratch.path().join("file");
        fs::write(&file, b"")?;
        #[cfg(unix)]
        let dangling = {
            let link = scratch.path().join("dangling");
            std::os::unix::fs::symlink(scratch.path().join("missing-target"), &link)?;
            Some((link, Err(io::ErrorKind::NotADirectory)))
        };
        #[cfg(not(unix))]
        let dangling = None;

        let cases = [
            (scratch.path().join("decks"), Ok(())),
            (scratch.path().join("missing"), Ok(())),
            (scratch.path().join("missing").join("jobs"), Ok(())),
            (file.clone(), Err(io::ErrorKind::NotADirectory)),
            (file.join("jobs"), Err(io::ErrorKind::NotADirectory)),
        ];
        for (dir, expected) in cases.into_iter().chain(dangling) {
            assert_eq!(check_deck_dir(&dir), expected, "{}", dir.display());
        }
        assert!(!scratch.path().join("missing").exists());
        assert_eq!(fs::read(&file)?, b"");
        Ok(())
    }

    #[test]
    fn a_write_after_the_create_and_an_unanswerable_outcome_are_failures() {
        // audit: CORPT00C-037
        let write = DeckError::Io {
            step: DeckStep::Write,
            kind: io::ErrorKind::StorageFull,
            removed: true,
        };
        assert_eq!(
            answer(ReportKind::Custom, Err(write)),
            Err(ServiceError::Rule(corpt00c::tdq_write_failure()))
        );
        let create = DeckError::Io {
            step: DeckStep::Create,
            kind: io::ErrorKind::AlreadyExists,
            removed: true,
        };
        assert_eq!(
            answer(ReportKind::Yearly, Err(create)),
            Err(ServiceError::Rule(corpt00c::tdq_write_failure()))
        );
        for internal in [DeckError::FileName, DeckError::Task] {
            assert_eq!(
                answer(ReportKind::Monthly, Err(internal)),
                Err(ServiceError::Internal)
            );
        }
        assert_eq!(
            [DeckStep::Generation, DeckStep::Create, DeckStep::Write].map(DeckStep::name),
            ["generation", "create", "write"]
        );
    }
}
