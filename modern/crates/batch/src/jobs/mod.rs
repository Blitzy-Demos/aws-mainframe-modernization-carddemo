//! The jobs of `carddemo-batch`, one module per in-scope JCL job (AAP 0.2.3, 0.3.4.3), and the
//! contract they share: [`JobContext`] in, [`JobOutcome`] or [`JobError`] out, and every output
//! file written through [`OutputFile`].
//!
//! | Module | JCL job | Program | Audit in `modern/audit/batch/` |
//! |---|---|---|---|
//! | `loads` | `ACCTFILE`, `CARDFILE`, `CUSTFILE`, `XREFFILE`, `DISCGRP`, `TCATBALF`, `TRANCATG`, `TRANTYPE` | `IDCAMS` `REPRO` | `LOADS.md` |
//! | `dusrsecj` | `DUSRSECJ` | `IEBGENER`, then `IDCAMS` `REPRO` | `LOADS.md` |
//! | `posttran` | `POSTTRAN` | `CBTRN02C` | `POSTTRAN.md` |
//! | `intcalc` | `INTCALC` | `CBACT04C` | `INTCALC.md` |
//! | `creastmt` | `CREASTMT` | `CBSTM03A` with its subroutine `CBSTM03B` | `CREASTMT.md` |
//! | `tranrept` | `TRANREPT` | `CBTRN03C` | `TRANREPT.md` |
//! | `cbpaup0j` | `CBPAUP0J` | `CBPAUP0C` | `CBPAUP0J.md` |
//! | `mnttrdb2` | `MNTTRDB2` | `COBTUPDT` | `MNTTRDB2.md` |
//!
//! # Layering
//!
//! - Every business rule comes from `domain::batch`; a job only drives it in the order the COBOL
//!   program runs, reading its inputs and applying the effects the rule returns.
//! - SQL is issued only by `persistence`, through its repository traits on `PgStore` or on a
//!   `PgTx` the job opens with `PgStore::begin` (R4); no job holds query text or reaches the
//!   database driver itself.
//! - Generation names come from `runtime::gdg`, and every directory from `runtime::config::Config`
//!   (Local-Only Runtime: files only through `std::fs`, under the directories `Config` names).
//! - The current date and time come only from the injected `Clock` of [`JobContext`].
//! - No job applies the schema (Single Schema Executor): that is the schema executor binary of
//!   the `runtime` crate, run before any job, and a job started against a database whose schema
//!   has not been applied fails at its first query with [`JobError::Persistence`].
//!
//! # Outputs
//!
//! A sequential output is written as fixed-length records, each followed by LF (AAP 0.3.4.3).
//! A `(+1)` generation (`DALYREJS`, `SYSTRAN`, `TRANREPT`) takes its name from
//! `runtime::gdg::next_generation` and is created exclusively, as a `NEW` data set never replaces
//! another; the statement files of `creastmt` keep fixed names and replace the previous run's.
//! A job returns every path it wrote in [`JobOutcome::written`], and `main` prints them.
//!
//! # Redaction
//!
//! The `Display` and `Debug` of [`JobError`] are the text `main` writes to standard error and to
//! the job's tracing event. They hold no record bytes and no card number, CVV, SSN, password or
//! hash (Cardholder Data Redaction): an I/O failure carries a static context and the
//! `std::io::ErrorKind`, never a path built from data, and a wrapped domain or persistence error
//! is shown through its own `Display` only where that text carries none of them.

pub(crate) mod cbpaup0j;
pub(crate) mod creastmt;
pub(crate) mod dusrsecj;
pub(crate) mod intcalc;
pub(crate) mod loads;
pub(crate) mod mnttrdb2;
pub(crate) mod posttran;
pub(crate) mod tranrept;

use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, ErrorKind, IntoInnerError, Write};
use std::path::{Path, PathBuf};

use domain::batch::cbact04c::IntcalcError;
use domain::batch::cbstm03a::StatementError;
use domain::batch::cbtrn02c::PosttranError;
use domain::batch::cbtrn03c::ReportError;
use domain::batch::cobtupdt::InputError;
use domain::batch::loads::LoadError;
use domain::clock::Clock;
use persistence::{PersistenceError, PgStore};
use runtime::config::Config;

use crate::pdf::RenderError;

/// What every job runs with, built once by `main` after the configuration is read and the pool
/// is open.
pub(crate) struct JobContext<'a> {
    /// The connection pool. A job reads through it, or opens a unit of work with
    /// `PgStore::begin` and commits it where the COBOL program's writes stand.
    pub(crate) store: &'a PgStore,
    /// The process configuration: the output, statement and fixture directories.
    pub(crate) config: &'a Config,
    /// The only source of the current date and time.
    pub(crate) clock: &'a dyn Clock,
}

/// What a job that ran to its end reports.
#[derive(Debug)]
pub(crate) struct JobOutcome {
    /// Every file the job wrote, in write order, each path exactly as the job built it from
    /// `Config` (relative paths stay relative, nothing is canonicalised). `main` prints them one
    /// per line; jobs that write no file leave it empty.
    pub(crate) written: Vec<PathBuf>,
    /// The program's return code, which becomes the process exit code: 0 for a clean run, or
    /// the code the job's audit gives, such as 4 when `posttran` rejected a record or `mnttrdb2`
    /// met an SQL error.
    pub(crate) return_code: u8,
    /// Named counters for the job's tracing event, such as `("records", 50)`. Plain counts,
    /// never record content.
    pub(crate) counts: Vec<(&'static str, u64)>,
}

/// Why a job ended without its outcome.
///
/// Every variant's text is redaction-safe (Cardholder Data Redaction): `Display` and `Debug`
/// write the same text, and neither ever formats a record, a key value or the `Debug` of a
/// wrapped error.
pub(crate) enum JobError {
    /// A local file could not be read or written. `context` is a static description of the
    /// step, such as `reading input`; the path is never shown.
    Io {
        /// The step that failed.
        context: &'static str,
        /// What the operating system reported.
        kind: ErrorKind,
    },
    /// Codec A or the in-stream extraction refused a fixture or the `DUSRSECJ` deck (`loads`,
    /// `dusrsecj`). Its text names a line, a field, an offset and a width only.
    Load(LoadError),
    /// A repository call failed. Its text names an SQLSTATE, a constraint or a column only.
    Persistence(PersistenceError),
    /// `STATEMNT.PDF` could not be rendered (`creastmt`). Its text names a record number at
    /// most.
    Render(RenderError),
    /// argon2id could not hash a password (`dusrsecj`). No detail is kept: it could only be
    /// about the password.
    Password,
    /// A `CBTRN02C` run counter overflowed (`posttran`).
    Posttran(PosttranError),
    /// A `CBACT04C` abend path or PARM refusal (`intcalc`). The account IDs and codes the
    /// domain error holds are not shown.
    Intcalc(IntcalcError),
    /// A `CBSTM03A` abend path (`creastmt`).
    Statement(StatementError),
    /// A `CBTRN03C` abend path (`tranrept`). The card number, type and category codes the
    /// domain error holds are not shown.
    Report(ReportError),
    /// A line of the `mnttrdb2` input is no `COBTUPDT` record. Its text names the line, the
    /// field, the offset and the width only.
    Input(InputError),
    /// An audited abend path of the job's program: `reason` is the program's static `DISPLAY`
    /// text or a description of the path, and `code` the return code it sets, if any.
    Abend {
        /// The return code the program sets before it ends, which becomes the exit code.
        code: Option<u8>,
        /// Static text naming the abend; never a key or a value.
        reason: &'static str,
    },
}

impl JobError {
    /// The process exit code the error carries: the `code` of [`JobError::Abend`], `None` for
    /// every other variant, for which `main` exits with `ExitCode::FAILURE`.
    ///
    /// `cbpaup0j` abends carry `domain::batch::cbpaup0c::ABEND_RETURN_CODE`, 16, which
    /// `9999-ABEND` moves to `RETURN-CODE` before its `GOBACK` (`CBPAUP0C.cbl:377-383`). The
    /// `9999-ABEND-PROGRAM` abends of `CBTRN02C` and `CBACT04C` call `CEE3ABD` with abend code
    /// 999 and set no return code (`CBTRN02C.cbl:707-711`), so they carry `code: None` unless
    /// their audit gives one.
    pub(crate) fn exit_code(&self) -> Option<u8> {
        match self {
            Self::Abend { code, .. } => *code,
            _ => None,
        }
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, kind } => write!(f, "{context}: {kind}"),
            // Verified texts: line numbers, field names, offsets, widths and failure classes.
            Self::Load(error) => fmt::Display::fmt(error, f),
            Self::Input(error) => fmt::Display::fmt(error, f),
            // Verified texts: SQLSTATE, constraint and column names only.
            Self::Persistence(error) => fmt::Display::fmt(error, f),
            // Verified texts: a record number at most.
            Self::Render(error) => fmt::Display::fmt(error, f),
            // Verified texts: fixed sentences.
            Self::Posttran(error) => fmt::Display::fmt(error, f),
            Self::Statement(error) => fmt::Display::fmt(error, f),
            Self::Password => f.write_str("password hashing failed"),
            Self::Intcalc(error) => write_intcalc(error, f),
            Self::Report(error) => f.write_str(report_text(*error)),
            Self::Abend {
                code: Some(code),
                reason,
            } => write!(f, "ABEND: {reason} (return code {code})"),
            Self::Abend { code: None, reason } => write!(f, "ABEND: {reason}"),
        }
    }
}

// Hand-written so that no inner `Debug`, which may show record values, is ever reached.
impl fmt::Debug for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for JobError {}

/// The text of a `CBACT04C` failure. The `Display` of the four abend variants names the account
/// ID and, for the disclosure lookup, the record's type and category codes, so each gets a fixed
/// text naming the failed lookup instead; the two PARM refusals name a length or a byte class
/// only and are shown as the domain writes them.
fn write_intcalc(error: &IntcalcError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let text = match error {
        IntcalcError::ParmDateTooShort { .. } | IntcalcError::ParmDateNotPrintable => {
            return fmt::Display::fmt(error, f);
        }
        IntcalcError::AccountNotFound { .. } => {
            "CBACT04C: no account record for a category balance"
        }
        IntcalcError::XrefNotFound { .. } => "CBACT04C: no card cross-reference for an account",
        IntcalcError::XrefDuplicate { .. } => {
            "CBACT04C: more than one card cross-reference for an account"
        }
        IntcalcError::DefaultDisclosureMissing { .. } => {
            "CBACT04C: no DEFAULT disclosure group for a category balance"
        }
    };
    f.write_str(text)
}

/// The text of a `CBTRN03C` failure. The domain's `Display` is the program's `INVALID ...` job-log
/// line, which shows the redacted card number or the raw type and category bytes of the record,
/// so each variant gets a fixed text naming the failed lookup instead.
fn report_text(error: ReportError) -> &'static str {
    match error {
        ReportError::XrefNotFound { .. } => "CBTRN03C: card number not in the card cross-reference",
        ReportError::TypeNotFound { .. } => "CBTRN03C: transaction type not found",
        ReportError::CategoryNotFound { .. } => "CBTRN03C: transaction category not found",
        ReportError::InvalidCardNumber => "CBTRN03C: TRAN-CARD-NUM is not 16 digits",
    }
}

impl From<LoadError> for JobError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}

impl From<PersistenceError> for JobError {
    fn from(error: PersistenceError) -> Self {
        Self::Persistence(error)
    }
}

impl From<RenderError> for JobError {
    fn from(error: RenderError) -> Self {
        Self::Render(error)
    }
}

impl From<PosttranError> for JobError {
    fn from(error: PosttranError) -> Self {
        Self::Posttran(error)
    }
}

impl From<IntcalcError> for JobError {
    fn from(error: IntcalcError) -> Self {
        Self::Intcalc(error)
    }
}

impl From<StatementError> for JobError {
    fn from(error: StatementError) -> Self {
        Self::Statement(error)
    }
}

impl From<ReportError> for JobError {
    fn from(error: ReportError) -> Self {
        Self::Report(error)
    }
}

impl From<InputError> for JobError {
    fn from(error: InputError) -> Self {
        Self::Input(error)
    }
}

/// The whole content of the input file at `path`, read in place (`loads`, `dusrsecj`,
/// `posttran`, `mnttrdb2`). The COBOL programs open their input first, so an unreadable input
/// fails the job before it writes anything.
///
/// # Errors
///
/// [`JobError::Io`] with context `reading input` when the file cannot be read.
fn read_input(path: &Path) -> Result<Vec<u8>, JobError> {
    fs::read(path).map_err(io("reading input"))
}

/// The conversion of an I/O failure of step `context` into [`JobError::Io`], keeping only the
/// error's kind: the error's own text can name the path.
fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> JobError {
    move |error| JobError::Io {
        context,
        kind: error.kind(),
    }
}

/// `len`, a number of records or lines held in memory, as a counter of [`JobOutcome::counts`].
fn count(len: usize) -> u64 {
    // TRUNCATION: `usize` is at most 64 bits wide on every target Rust supports, so the
    // conversion is exact; it could only saturate past 2^64 records, which no job can hold.
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// One sequential output file of a job, deleted unless the job finishes it.
///
/// The JCL allocates every output `DISP=(NEW,CATLG,DELETE)`: `DALYREJS(+1)` in `POSTTRAN.jcl`
/// (lines 34-38), `SYSTRAN(+1)` in `INTCALC.jcl`, `TRANREPT(+1)` in `TRANREPT.jcl`, and
/// `STATEMNT.PS` and `STATEMNT.HTML` in step `STEP040` of `CREASTMT.JCL`. A data set is
/// catalogued when its step ends normally and deleted when it abends, so a value dropped without
/// [`OutputFile::finish`], as when a job returns an error, removes its file; `finish` flushes
/// it, keeps it and returns its path.
///
/// The file is created exclusively and only by this type, so the file it removes is always one
/// it created.
pub(crate) struct OutputFile {
    /// Where the file is, as built from the directory the job passed.
    path: PathBuf,
    /// The open file; `None` once [`OutputFile::finish`] has taken it, which disarms the guard.
    writer: Option<BufWriter<File>>,
}

impl OutputFile {
    /// Creates generation `(+1)` of base `name` in `dir`, `<dir>/<name>.G<nnnn>`, through
    /// `runtime::gdg::next_generation`, which creates `dir` when it is missing (`posttran`
    /// `DALYREJS`, `intcalc` `SYSTRAN`, `tranrept` `TRANREPT`).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `allocating generation` when the directory cannot be
    /// created or read or the generation limit is reached, and with context
    /// `creating output file` when the file cannot be created, as when another run took the
    /// same generation first.
    pub(crate) fn next_generation(dir: &Path, name: &str) -> Result<Self, JobError> {
        let path = runtime::gdg::next_generation(dir, name).map_err(io("allocating generation"))?;
        Self::create(path)
    }

    /// Creates `<dir>/<file_name>` in place of the file of that name a previous run wrote,
    /// creating `dir` when it is missing. This is the fixed-name replacement of `CREASTMT.JCL`,
    /// whose `STEP030` deletes the previous `STATEMNT.PS` and `STATEMNT.HTML` with `IEFBR14` and
    /// `DISP=(MOD,DELETE,DELETE)` before `STEP040` writes them again (`creastmt`).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `creating output directory`, `removing previous output`
    /// or `creating output file` for the step that failed. A missing previous file is no
    /// error.
    pub(crate) fn replace(dir: &Path, file_name: &str) -> Result<Self, JobError> {
        fs::create_dir_all(dir).map_err(io("creating output directory"))?;
        let path = dir.join(file_name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(io("removing previous output")(error)),
        }
        Self::create(path)
    }

    /// Creates the file at `path`, failing when anything is already there. The guard is armed
    /// only once the file exists, so a failed creation never removes another file.
    fn create(path: PathBuf) -> Result<Self, JobError> {
        let file = File::create_new(&path).map_err(io("creating output file"))?;
        Ok(Self {
            path,
            writer: Some(BufWriter::new(file)),
        })
    }

    /// Appends one fixed-length record and its LF terminator (AAP 0.3.4.3).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the bytes cannot be written.
    pub(crate) fn write_record(&mut self, record: &[u8]) -> Result<(), JobError> {
        let writer = self.writer()?;
        writer
            .write_all(record)
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(io("writing output"))
    }

    /// Appends `bytes` as they are, with no terminator: the content of `STATEMNT.PDF`
    /// (`creastmt`).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the bytes cannot be written.
    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), JobError> {
        self.writer()?
            .write_all(bytes)
            .map_err(io("writing output"))
    }

    /// Flushes the file to disk, keeps it and returns its path, the one to report in
    /// [`JobOutcome::written`].
    ///
    /// The file is synced as well as flushed, so that a write the operating system could only
    /// fail later, such as on a full disk, fails the job instead of leaving a short file behind.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `finishing output` when the buffered bytes cannot be
    /// written or synced; the file is removed then, as the job ends without it.
    pub(crate) fn finish(mut self) -> Result<PathBuf, JobError> {
        let path = std::mem::take(&mut self.path);
        // Taking the writer disarms the guard of `Drop`.
        let flushed = self.writer.take().map_or(Ok(()), |writer| {
            writer
                .into_inner()
                .map_err(IntoInnerError::into_error)
                .and_then(|file| file.sync_all())
        });
        match flushed {
            Ok(()) => Ok(path),
            Err(error) => {
                // Best effort: the job fails with the write error whether or not this succeeds.
                let _ = fs::remove_file(&path);
                Err(io("finishing output")(error))
            }
        }
    }

    /// The open file. It is present for as long as the value exists, because only `finish`,
    /// which consumes the value, and `Drop` take it; the error is never met in practice.
    fn writer(&mut self) -> Result<&mut BufWriter<File>, JobError> {
        self.writer.as_mut().ok_or(JobError::Io {
            context: "writing output",
            kind: ErrorKind::Other,
        })
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            // The buffered bytes are discarded rather than flushed into a file about to go, and
            // the file is closed before it is removed.
            let (file, _unwritten) = writer.into_parts();
            drop(file);
            // Best effort: a drop cannot report a failure.
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use copybook::PicU;
    use domain::batch::cbact04c::IntcalcError;
    use domain::batch::cbtrn03c::ReportError;

    use super::{JobError, OutputFile, read_input};

    type TestResult = Result<(), Box<dyn Error>>;

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
                "carddemo-batch-jobs-{}-{nanos}-{sequence}",
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

    #[test]
    fn records_end_with_lf() -> TestResult {
        let scratch = Scratch::new()?;
        let mut output = OutputFile::next_generation(scratch.path(), "DALYREJS")?;
        output.write_record(b"rec1")?;
        output.write_record(b"rec2")?;
        let path = output.finish()?;
        assert_eq!(fs::read(&path)?, b"rec1\nrec2\n");
        Ok(())
    }

    #[test]
    fn raw_bytes_get_no_terminator() -> TestResult {
        let scratch = Scratch::new()?;
        let mut output = OutputFile::replace(scratch.path(), "STATEMNT.PDF")?;
        output.write_bytes(b"%PDF-1.3")?;
        output.write_bytes(b" %%EOF")?;
        let path = output.finish()?;
        assert_eq!(fs::read(&path)?, b"%PDF-1.3 %%EOF");
        Ok(())
    }

    /// A file dropped unfinished is removed, whichever constructor made it, and its generation
    /// number is free again, as a data set deleted at abend is never catalogued.
    #[test]
    fn guard_deletes_an_unfinished_file() -> TestResult {
        let scratch = Scratch::new()?;
        let outputs = [
            OutputFile::next_generation(scratch.path(), "SYSTRAN")?,
            OutputFile::replace(scratch.path(), "STATEMNT.HTML")?,
        ];
        for mut output in outputs {
            output.write_record(b"partial")?;
            let path = output.path.clone();
            assert!(path.exists(), "{}", path.display());
            drop(output);
            assert!(!path.exists(), "{}", path.display());
        }
        let again = OutputFile::next_generation(scratch.path(), "SYSTRAN")?.finish()?;
        assert_eq!(again, scratch.path().join("SYSTRAN.G0001"));
        Ok(())
    }

    #[test]
    fn replace_removes_the_previous_file() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("statements");
        let mut first = OutputFile::replace(&dir, "STATEMNT.PS")?;
        first.write_record(b"old")?;
        let first = first.finish()?;
        let mut second = OutputFile::replace(&dir, "STATEMNT.PS")?;
        second.write_record(b"new")?;
        let second = second.finish()?;
        assert_eq!(first, dir.join("STATEMNT.PS"));
        assert_eq!(second, first);
        assert_eq!(fs::read(&second)?, b"new\n");
        Ok(())
    }

    #[test]
    fn generation_numbers_increase() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("dalyrejs");
        let first = OutputFile::next_generation(&dir, "DALYREJS")?.finish()?;
        let second = OutputFile::next_generation(&dir, "DALYREJS")?.finish()?;
        assert_eq!(first, dir.join("DALYREJS.G0001"));
        assert_eq!(second, dir.join("DALYREJS.G0002"));
        assert!(first.exists() && second.exists());
        Ok(())
    }

    /// An unreadable input is an I/O error that names the step and the kind, never the path.
    #[test]
    fn unreadable_input_names_the_step_only() -> TestResult {
        let scratch = Scratch::new()?;
        let missing = scratch.path().join("dailytran.txt");
        let error = read_input(&missing)
            .err()
            .ok_or("a missing input was read")?;
        assert!(matches!(
            error,
            JobError::Io {
                context: "reading input",
                kind: ErrorKind::NotFound
            }
        ));
        let text = error.to_string();
        assert_eq!(text, format!("reading input: {}", ErrorKind::NotFound));
        assert!(!text.contains("dailytran"));
        Ok(())
    }

    /// Each error's text, which `Debug` repeats, its exit code, and the record values it must
    /// not show.
    #[test]
    fn error_texts_and_exit_codes() -> TestResult {
        let cases: [(JobError, &str, Option<u8>, &[&str]); 6] = [
            (
                JobError::Abend {
                    code: Some(16),
                    reason: "CBPAUP0C ABENDING ...",
                },
                "ABEND: CBPAUP0C ABENDING ... (return code 16)",
                Some(16),
                &[],
            ),
            (
                JobError::Abend {
                    code: None,
                    reason: "ERROR WRITING TO TRANSACTION FILE",
                },
                "ABEND: ERROR WRITING TO TRANSACTION FILE",
                None,
                &[],
            ),
            (JobError::Password, "password hashing failed", None, &[]),
            (
                JobError::Report(ReportError::XrefNotFound {
                    redacted_card: *b"************5740",
                }),
                "CBTRN03C: card number not in the card cross-reference",
                None,
                &["5740"],
            ),
            (
                JobError::Report(ReportError::CategoryNotFound {
                    type_cd: *b"07",
                    cat_cd: PicU::new(9)?,
                }),
                "CBTRN03C: transaction category not found",
                None,
                &["07", "0009"],
            ),
            (
                JobError::Intcalc(IntcalcError::AccountNotFound {
                    acct_id: PicU::new(50)?,
                    rewritten: None,
                }),
                "CBACT04C: no account record for a category balance",
                None,
                &["00000000050", "50"],
            ),
        ];
        for (error, text, exit_code, hidden) in cases {
            let shown = error.to_string();
            assert_eq!(shown, text);
            assert_eq!(format!("{error:?}"), shown);
            assert_eq!(error.exit_code(), exit_code, "{shown}");
            for value in hidden {
                assert!(!shown.contains(value), "{shown} shows {value}");
            }
        }
        Ok(())
    }
}
