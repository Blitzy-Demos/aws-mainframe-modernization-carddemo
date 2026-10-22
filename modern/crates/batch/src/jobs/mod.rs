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
//! another; the statement files of `creastmt` keep fixed names and replace the previous run's,
//! which only the holder of the directory's [`DirectoryLock`] may do (CBSTM03A-002).
//! A job returns every path it wrote in [`JobOutcome::written`], and `main` prints them.
//!
//! Every output file is created readable and writable by its owner only, and every output
//! directory a job creates is accessible to its owner only: the files hold full card numbers,
//! names, addresses and balances, which stay byte for byte as the COBOL writes them and are
//! protected by their permissions instead.
//!
//! # Blocking work
//!
//! A job runs on the async runtime, so every step that can block a thread for long, a file
//! read, write, sync or removal, a password hash or the PDF rendering, runs on the runtime's
//! blocking pool through [`blocking`] and is awaited there and then: the job's order of reads
//! and writes is the program's, and no task outlives the step that started it. Tracing events
//! are emitted only once the step is back on the job's task, inside the job's span. The two
//! exceptions are the guards of an unfinished [`OutputFile`] and of a [`DirectoryLock`] not
//! released, which clean up in place when dropped, since a drop cannot await.
//!
//! # Redaction
//!
//! The `Display` and `Debug` of [`JobError`] are the text `main` writes to standard error and to
//! the job's tracing event. They hold no record bytes and no card number, CVV, SSN, password or
//! hash (Cardholder Data Redaction): an I/O failure carries a static context and the
//! `std::io::ErrorKind`, never a path, and one on the job's output directory also the setting
//! that names the directory, such as `reports under CARDDEMO_VAR_DIR`; a reached generation
//! limit names the static base only, and a wrapped domain or persistence error is shown through
//! its own `Display` only where that text carries none of them.
//!
//! The one path an error shows is the input file of a load (`loads`, `dusrsecj`), as the job got
//! it: the `--input` the operator gave, or its default, the fixture in `CARDDEMO_FIXTURE_DIR` or
//! the `DUSRSECJ` deck, never canonicalised (an [`InputPath`]). It is operator configuration,
//! not data built from records, and the operator needs it to tell which file failed (LOADS-016,
//! LOADS-017, LOADS-019). Its control characters are escaped, so the report stays one line; no
//! byte of the file is shown.

pub(crate) mod cbpaup0j;
pub(crate) mod creastmt;
pub(crate) mod dusrsecj;
pub(crate) mod intcalc;
pub(crate) mod loads;
pub(crate) mod mnttrdb2;
pub(crate) mod posttran;
pub(crate) mod tranrept;

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
#[cfg(target_os = "linux")]
use std::io::Read as _;
use std::io::{self, BufReader, ErrorKind, Seek, SeekFrom, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd as _;
#[cfg(target_os = "linux")]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use domain::batch::cbact04c::IntcalcError;
use domain::batch::cbstm03a::StatementError;
use domain::batch::cbtrn02c::PosttranError;
use domain::batch::cbtrn03c::ReportError;
use domain::batch::cobtupdt::InputError;
use domain::batch::loads::LoadError;
use domain::clock::Clock;
use persistence::{PersistenceError, PgStore};
use runtime::config::Config;
use runtime::telemetry::JobCounter;

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
    /// The job's counters, such as `(JobCounter::Records, 50)`, which `main` records on the job
    /// span, each under its field name (`records`). Plain counts, never record content.
    pub(crate) counts: Vec<(JobCounter, u64)>,
}

/// Why a job ended without its outcome.
///
/// Every variant's text is redaction-safe (Cardholder Data Redaction): `Display` and `Debug`
/// write the same text, and neither ever formats a record, a key value or the `Debug` of a
/// wrapped error.
pub(crate) enum JobError {
    /// A local file other than the input of a load could not be read or written: an output, or
    /// the input of `posttran` or `mnttrdb2`. `context` is a static description of the step,
    /// such as `reading input`; the path is never shown.
    Io {
        /// The step that failed.
        context: &'static str,
        /// What the operating system reported.
        kind: ErrorKind,
    },
    /// A step on the job's output directory failed: allocating a generation, creating or locking
    /// the directory, or creating an output file in it. Its text names the step, the setting
    /// that names the directory, such as `reports under CARDDEMO_VAR_DIR`, and what the
    /// operating system reported, never the path. [`JobError::naming_output_dir`] makes it from
    /// the [`JobError::Io`] of such a step.
    OutputDirectory {
        /// The step that failed.
        context: &'static str,
        /// The configuration that names the directory, in words: the variable, and the
        /// subdirectory when the directory lies under `CARDDEMO_VAR_DIR`.
        setting: &'static str,
        /// What the operating system reported.
        kind: ErrorKind,
    },
    /// The input file of a load could not be read (`loads`, `dusrsecj`; LOADS-019). Its text
    /// names the path as the job got it and what the operating system reported.
    UnreadableInput {
        /// The file the load tried to read.
        input: InputPath,
        /// What the operating system reported.
        kind: ErrorKind,
    },
    /// Codec A or the in-stream extraction refused a fixture or the `DUSRSECJ` deck (`loads`,
    /// `dusrsecj`). Its text names the input file, then a line, a field, an offset and a width
    /// only, never a byte of the line (LOADS-016, LOADS-017).
    Load {
        /// The file the load read.
        input: InputPath,
        /// What codec A or the extraction refused.
        error: LoadError,
    },
    /// Generation 9999, `runtime::gdg::MAX_GENERATION`, of the `(+1)` output `base` already
    /// exists, so no further generation is named and none is overwritten
    /// ([`OutputFile::next_generation`]). `posttran`, `intcalc` and `tranrept` each report it as
    /// the cause of the abend of their failed output open, so none of them returns it. Its text
    /// names the step, the limit and the base only, never the directory.
    GenerationLimit {
        /// The generation base, such as `SYSTRAN`.
        base: &'static str,
    },
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

    /// The error with `setting`, the configuration that names the job's output directory, when
    /// it is the [`JobError::Io`] of a step on that directory: allocating a generation, creating
    /// or locking the directory, or creating an output file in it, each of which only
    /// [`OutputFile`] and [`DirectoryLock`] take. That error becomes
    /// [`JobError::OutputDirectory`], so the operator learns which setting to correct without
    /// the path being shown. Every other error is returned as it is, its exit code unchanged.
    ///
    /// `main` calls this for the jobs that write files, each of which writes into one directory
    /// only, so every such step of the job is on that directory.
    pub(crate) fn naming_output_dir(self, setting: &'static str) -> Self {
        match self {
            Self::Io { context, kind } if OUTPUT_DIRECTORY_STEPS.contains(&context) => {
                Self::OutputDirectory {
                    context,
                    setting,
                    kind,
                }
            }
            other => other,
        }
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, kind } => write!(f, "{context}: {kind}"),
            Self::OutputDirectory {
                context,
                setting,
                kind,
            } => write!(f, "{context} ({setting}): {kind}"),
            Self::UnreadableInput { input, kind } => write!(f, "{READING_INPUT} {input}: {kind}"),
            Self::GenerationLimit { base } => write!(
                f,
                "{ALLOCATING_GENERATION}: generation limit {limit} reached for {base}",
                limit = runtime::gdg::MAX_GENERATION
            ),
            // Verified texts: line numbers, field names, offsets, widths and failure classes.
            Self::Load { input, error } => write!(f, "{input}: {error}"),
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

/// The input file of a load (`loads`, `dusrsecj`) as the job got it: the `--input` path as
/// given, or its default, the fixture path `LoadArgs::input_path` resolves from
/// `CARDDEMO_FIXTURE_DIR` or the `DUSRSECJ` deck; never canonicalised, so the text an error
/// shows is the path the operator can act on (LOADS-010, LOADS-019).
///
/// `Display` writes the path as `Path::display` does, with each control character replaced by
/// its `char::escape_default` form, so a CR or LF in a path cannot split the one-line report
/// `main` writes; every other character is written as it is.
pub(crate) struct InputPath(PathBuf);

impl InputPath {
    /// The input `path`, kept as it is.
    pub(crate) fn new(path: &Path) -> Self {
        Self(path.to_path_buf())
    }
}

impl fmt::Display for InputPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The same text as `Path::display`: invalid UTF-8 becomes U+FFFD.
        for character in self.0.to_string_lossy().chars() {
            if character.is_control() {
                fmt::Display::fmt(&character.escape_default(), f)?;
            } else {
                fmt::Write::write_char(f, character)?;
            }
        }
        Ok(())
    }
}

/// The whole content of the input file at `path`, read in place (`mnttrdb2`). The COBOL program
/// opens its input first, so an unreadable input fails the job before it writes anything. Its
/// failure events name the file by its DD, never by its path; the loads read through
/// [`read_named_input`] instead, and `posttran` opens and then reads its input itself, because
/// `CBTRN02C` shows a failed open and a failed read on different paths (CBTRN02C-031,
/// CBTRN02C-038).
///
/// The read blocks its thread, so the job calls this inside the one [`blocking`] step that also
/// decodes the bytes, with [`READING_INPUT`] as the step's context.
///
/// # Errors
///
/// [`JobError::Io`] with context `reading input` when the file cannot be read.
fn read_input(path: &Path) -> Result<Vec<u8>, JobError> {
    fs::read(path).map_err(io(READING_INPUT))
}

/// The whole content of the input file of a load at `input` (`loads`, `dusrsecj`), read in
/// place, as [`read_input`] reads it, inside the job's one [`blocking`] step. The table is not
/// touched before this read succeeds (LOADS-011, LOADS-019).
///
/// # Errors
///
/// [`JobError::UnreadableInput`] naming `input` and the error's kind when the file cannot be
/// read, as when it does not exist or is a directory (LOADS-019).
fn read_named_input(input: &Path) -> Result<Vec<u8>, JobError> {
    fs::read(input).map_err(|error| JobError::UnreadableInput {
        input: InputPath::new(input),
        kind: error.kind(),
    })
}

/// The conversion of what codec A or the in-stream extraction refused in the load input at
/// `input` into [`JobError::Load`], which names the file as well as the line and the rule
/// (LOADS-016, LOADS-017).
fn load_refused(input: &Path) -> impl FnOnce(LoadError) -> JobError + '_ {
    move |error| JobError::Load {
        input: InputPath::new(input),
        error,
    }
}

/// Runs `work` on the runtime's blocking pool and waits for its result, so that a step that
/// blocks its thread, file I/O, a password hash or the PDF rendering, never stalls the job's
/// task (W011-F05).
///
/// The call is awaited where it is made, never left running: the job goes on only once `work`
/// has finished, so its steps keep the program's order, and the caller maps what `work`
/// returns exactly as it did when the step ran inline. `work` emits no tracing event, which
/// would fall outside the job's span; the caller reports its outcome once back on the task.
///
/// # Errors
///
/// [`JobError::Io`] with `context` and `ErrorKind::Interrupted` when the runtime cancelled the
/// task before it ran, which it does only while shutting down.
///
/// # Panics
///
/// When `work` panics, the panic is resumed here, as it would have panicked inline.
async fn blocking<T: Send + 'static>(
    context: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, JobError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => Ok(value),
        Err(error) => match error.try_into_panic() {
            Ok(payload) => std::panic::resume_unwind(payload),
            Err(_cancelled) => Err(JobError::Io {
                context,
                kind: ErrorKind::Interrupted,
            }),
        },
    }
}

/// The conversion of an I/O failure of step `context` into [`JobError::Io`], keeping only the
/// error's kind: the error's own text can name the path.
fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> JobError {
    move |error| JobError::Io {
        context,
        kind: error.kind(),
    }
}

/// The conversion of a failure of `runtime::gdg::next_generation` for base `base`:
/// [`JobError::GenerationLimit`] when generation 9999 of the base exists, which the error kind
/// alone cannot tell, and otherwise [`JobError::Io`] with context `allocating generation`,
/// keeping only the kind, as [`io()`] does.
fn generation_failure(base: &'static str) -> impl FnOnce(std::io::Error) -> JobError {
    move |error| {
        if runtime::gdg::is_limit_reached(&error) {
            JobError::GenerationLimit { base }
        } else {
            io(ALLOCATING_GENERATION)(error)
        }
    }
}

/// `len`, a number of records or lines, as a counter of [`JobOutcome::counts`].
fn count(len: usize) -> u64 {
    // TRUNCATION: `usize` is at most 64 bits wide on every target Rust supports, so the
    // conversion is exact; it could only saturate past 2^64 records, which no job can write.
    u64::try_from(len).unwrap_or(u64::MAX)
}

// The static contexts of [`JobError::Io`]: the step that failed, never a path.

/// Reading a job's input file.
const READING_INPUT: &str = "reading input";
/// Creating an output directory and naming the next `(+1)` generation in it.
const ALLOCATING_GENERATION: &str = "allocating generation";
/// Creating the directory of a fixed-name output.
const CREATING_OUTPUT_DIRECTORY: &str = "creating output directory";
/// Taking the [`DirectoryLock`] of an output directory.
const LOCKING_OUTPUT_DIRECTORY: &str = "locking output directory";
/// Removing the previous run's file of a fixed name.
const REMOVING_PREVIOUS_OUTPUT: &str = "removing previous output";
/// Creating an output file.
const CREATING_OUTPUT_FILE: &str = "creating output file";
/// Writing buffered bytes to an output file.
const WRITING_OUTPUT: &str = "writing output";
/// Writing the last buffered bytes and syncing an output file.
const FINISHING_OUTPUT: &str = "finishing output";

/// The steps on a job's output directory, whose errors [`JobError::naming_output_dir`] makes name
/// the directory's setting.
const OUTPUT_DIRECTORY_STEPS: [&str; 4] = [
    ALLOCATING_GENERATION,
    CREATING_OUTPUT_DIRECTORY,
    LOCKING_OUTPUT_DIRECTORY,
    CREATING_OUTPUT_FILE,
];

// The settings [`JobError::naming_output_dir`] takes, one per job that writes files: `main`
// names them in the error a job returns, and `posttran`, `intcalc` and `tranrept` in the event
// of their generation open, whose failure each reports itself before its abend.

/// The setting of `posttran`'s `DALYREJS` generations: `dalyrejs` under `CARDDEMO_VAR_DIR`.
pub(crate) const DALYREJS_DIR_SETTING: &str = "dalyrejs under CARDDEMO_VAR_DIR";

/// The setting of `intcalc`'s `SYSTRAN` generations: `systran` under `CARDDEMO_VAR_DIR`.
pub(crate) const SYSTRAN_DIR_SETTING: &str = "systran under CARDDEMO_VAR_DIR";

/// The setting of `tranrept`'s `TRANREPT` generations: `reports` under `CARDDEMO_VAR_DIR`.
pub(crate) const REPORTS_DIR_SETTING: &str = "reports under CARDDEMO_VAR_DIR";

/// The setting of `creastmt`'s statement files: `CARDDEMO_STATEMENT_DIR` when it is set and
/// `statements` under `CARDDEMO_VAR_DIR` otherwise (`runtime::config`), so the text names both.
pub(crate) const STATEMENT_DIR_SETTING: &str = "CARDDEMO_STATEMENT_DIR, or statements under \
                                                CARDDEMO_VAR_DIR when CARDDEMO_STATEMENT_DIR is \
                                                unset";

/// The cleanup operation of an output a job created and does not keep: the guard of an
/// unfinished [`OutputFile`].
const REMOVING_UNFINISHED_OUTPUT: &str = "removing unfinished output";

/// The cleanup operation of the entries of an output directory's [`DirectoryLock`].
const REMOVING_DIRECTORY_LOCK: &str = "removing output directory lock";

/// Bytes an [`OutputFile`] buffers before they reach the file: the default capacity of
/// `std::io::BufWriter`, which the outputs were written through before their I/O moved to the
/// blocking pool, so that a write fails at the call it failed at then.
const BUFFER_CAPACITY: usize = 8 * 1024;

/// The shortest time between two attempts of a run waiting for a held output directory.
const LOCK_RETRY: Duration = Duration::from_millis(100);

/// The name of the entry that is the [`DirectoryLock`] of an output directory.
const LOCK_NAME: &str = ".carddemo-batch.lock";

/// The first part of the name at which a run binds its lock socket before linking it as
/// [`LOCK_NAME`].
#[cfg(target_os = "linux")]
const CLAIM_PREFIX: &str = ".carddemo-batch.claim.";

/// The first part of the name to which a lock whose holder has ended is moved to be removed.
#[cfg(target_os = "linux")]
const STALE_PREFIX: &str = ".carddemo-batch.stale.";

/// Tells apart the claim and stale names one process makes.
#[cfg(target_os = "linux")]
static ENTRY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The Unix mode of an output file: read and write for its owner only (W005-F06).
#[cfg(unix)]
const OWNER_ONLY_FILE: u32 = 0o600;

/// The Unix mode of an output directory a job creates: full access for its owner only.
#[cfg(unix)]
const OWNER_ONLY_DIR: u32 = 0o700;

/// The options every output file is created with: exclusively, failing when anything is already
/// at the path, and open for reading as well as writing, so that a job can read its own output
/// back (`creastmt`).
///
/// On Unix the file is created with mode `0600`, which the process umask can only narrow: its
/// owner may read and write it, the group and others never (W005-F06). On other platforms it
/// takes the default access control its directory gives. The content is never altered for this:
/// it keeps the full card numbers and personal data the COBOL writes.
fn output_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, OWNER_ONLY_FILE);
    options
}

/// Creates the output directory `dir` and every missing ancestor, each accessible to its owner
/// only: mode `0700` on Unix, which the umask can only narrow, and the default access control of
/// the parent elsewhere. A directory that already exists keeps its permissions and is no error.
///
/// # Errors
///
/// The I/O error of a directory that could not be created: `ErrorKind::NotADirectory` when `dir`
/// or one of its ancestors exists and is not a directory, such as a regular file. A recursive
/// create reports such an entry as already existing, which names no true cause, so that report
/// becomes this kind.
fn create_output_dir(dir: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, OWNER_ONLY_DIR);
    // A recursive create succeeds whenever `dir` ends up a directory, so an existing entry it
    // reports is never one.
    builder.create(dir).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            io::Error::new(
                ErrorKind::NotADirectory,
                "the output directory exists and is not a directory",
            )
        } else {
            error
        }
    })
}

/// Which file a directory entry names, so that a cleanup removes a name only while it still
/// names the file the run created (W005-F01): on Unix the device and inode numbers, which no
/// other file shares while the file exists. Elsewhere it holds nothing and every entry counts as
/// the file, so cleanup there goes by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    /// The device of the file system holding the file.
    #[cfg(unix)]
    dev: u64,
    /// The file's inode number on that device.
    #[cfg(unix)]
    ino: u64,
}

impl FileIdentity {
    /// The identity of the file `metadata` describes.
    #[cfg(unix)]
    fn of(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt as _;

        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }

    /// The identity of the file `metadata` describes: none to tell, off Unix.
    #[cfg(not(unix))]
    fn of(_metadata: &fs::Metadata) -> Self {
        Self {}
    }
}

/// Removes `path` while it still names the file `identity` identifies, an output or lock entry
/// this run created and does not keep. A name that is gone, or that names another file, as when
/// another run has put its own file there, holds nothing of this run's: the cleanup's outcome is
/// reached and the other file stays. The check and the removal are two steps, so a file put
/// under the name between them by a process that does not take the directory's lock would be
/// removed; every `carddemo-batch` run that replaces a fixed name takes the lock first.
///
/// # Errors
///
/// The kind of any other failure, the one thing about it [`cleanup_failed`] reports.
fn remove_output(path: &Path, identity: FileIdentity) -> Result<(), ErrorKind> {
    match fs::symlink_metadata(path) {
        Ok(found) if FileIdentity::of(&found) == identity => remove_by_name(path),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.kind()),
    }
}

/// Removes `path` by name: a file whose identity is checked already, or one the calling step
/// created a moment ago under a name no other run uses, such as a claim name or a file whose
/// identity could not be read. A file already gone is no failure.
///
/// # Errors
///
/// The kind of any other failure.
fn remove_by_name(path: &Path) -> Result<(), ErrorKind> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.kind()),
    }
}

/// Reports that cleanup `operation` could not remove an output, so that the operator learns a
/// partial or abandoned file may remain (W015-F02). The event names the static operation and
/// the error kind only, never the path, which a job builds from its directories, nor a byte of
/// the file (Cardholder Data Redaction).
fn cleanup_failed(operation: &'static str, kind: ErrorKind) {
    tracing::error!(operation, error_kind = %kind, "output file cleanup failed");
}

/// The failure of a [`blocking`] step that may leave behind an entry it created: the error the
/// job returns and, when the step could not remove the entry again, the cleanup operation and
/// the error kind, which [`Failure::reported`] passes to [`cleanup_failed`] once the step is back
/// on the job's task.
struct Failure {
    /// What the job returns.
    error: JobError,
    /// The cleanup that failed on the way, if one did.
    leftover: Option<(&'static str, ErrorKind)>,
}

impl Failure {
    /// `error`, after cleanup `operation` ended with `cleanup`.
    fn after_cleanup(
        error: JobError,
        operation: &'static str,
        cleanup: Result<(), ErrorKind>,
    ) -> Self {
        Self {
            error,
            leftover: cleanup.err().map(|kind| (operation, kind)),
        }
    }

    /// Reports the failed cleanup, if any, then gives the error. Called on the job's task.
    fn reported(self) -> JobError {
        if let Some((operation, kind)) = self.leftover {
            cleanup_failed(operation, kind);
        }
        self.error
    }
}

impl From<JobError> for Failure {
    fn from(error: JobError) -> Self {
        Self {
            error,
            leftover: None,
        }
    }
}

/// Removes `outputs`, each path while it still names the file of its identity (see
/// [`remove_output`]), in one [`blocking`] step, then reports each that could not be removed,
/// once back on the job's task. Cleanup is best effort: the caller returns the error that made
/// it clean up, whatever happens here. A step the runtime cancelled removed nothing, so each
/// path is reported with `ErrorKind::Interrupted`.
async fn remove_outputs(operation: &'static str, outputs: Vec<(PathBuf, FileIdentity)>) {
    let total = outputs.len();
    let removed = blocking(operation, move || {
        outputs
            .iter()
            .map(|(path, identity)| remove_output(path, *identity))
            .collect::<Vec<_>>()
    })
    .await;
    match removed {
        Ok(outcomes) => {
            for kind in outcomes.into_iter().filter_map(Result::err) {
                cleanup_failed(operation, kind);
            }
        }
        Err(_cancelled) => {
            for _ in 0..total {
                cleanup_failed(operation, ErrorKind::Interrupted);
            }
        }
    }
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
/// The file is created exclusively and owner-only (W005-F06), and its identity is recorded at
/// creation: its guard, and a failed [`OutputFile::finish`], remove the path only while it
/// still names that file, so a cleanup leaves a file another run has put under the name (see
/// [`remove_output`]). A fixed name is replaced only under the directory's
/// [`DirectoryLock`], so no other `carddemo-batch` run replaces the file meanwhile (W005-F01).
///
/// Every file operation runs as a [`blocking`] step the call awaits (W011-F05). Writes are
/// buffered as `std::io::BufWriter` buffers them, [`BUFFER_CAPACITY`] bytes at most, so a write
/// reaches the file, and can fail, at the same call as when the file was written inline. The
/// guard is the flag `armed` rather than the handle, which a step in flight may still share when
/// a cancelled job drops the value: the file is removed all the same.
pub(crate) struct OutputFile {
    /// Where the file is, as built from the directory the job passed.
    path: PathBuf,
    /// Which file `path` named when this value created it.
    identity: FileIdentity,
    /// The open file, shared with the blocking step that writes, syncs or reads it.
    file: Arc<File>,
    /// The bytes appended since the last write to the file.
    pending: Vec<u8>,
    /// Whether dropping the value removes the file: set at creation, cleared by
    /// [`OutputFile::finish`], which keeps or removes the file itself.
    armed: bool,
}

impl OutputFile {
    /// Creates generation `(+1)` of base `name` in `dir`, `<dir>/<name>.G<nnnn>`, through
    /// `runtime::gdg::next_generation` (`posttran` `DALYREJS`, `intcalc` `SYSTRAN`, `tranrept`
    /// `TRANREPT`). A missing `dir` is created owner-only first, so the generation naming finds
    /// it in place and creates nothing.
    ///
    /// # Errors
    ///
    /// - [`JobError::GenerationLimit`] when generation 9999 of `name` already exists, which is
    ///   left as it is.
    /// - [`JobError::Io`] with context `allocating generation` when the directory cannot be
    ///   created or read, or `name` is no valid base, and with context `creating output file`
    ///   when the file cannot be created, as when another run took the same generation first.
    pub(crate) async fn next_generation(dir: &Path, name: &'static str) -> Result<Self, JobError> {
        let dir = dir.to_path_buf();
        blocking(ALLOCATING_GENERATION, move || {
            create_output_dir(&dir).map_err(io(ALLOCATING_GENERATION))?;
            let path =
                runtime::gdg::next_generation(&dir, name).map_err(generation_failure(name))?;
            Self::create(path)
        })
        .await?
        .map_err(Failure::reported)
    }

    /// Creates `<dir>/<file_name>` in place of the file of that name a previous run wrote, in
    /// the directory `lock` holds, which [`DirectoryLock::acquire`] has created. This is the
    /// fixed-name replacement of `CREASTMT.JCL`, whose `STEP030` deletes the previous
    /// `STATEMNT.PS` and `STATEMNT.HTML` with `IEFBR14` and `DISP=(MOD,DELETE,DELETE)` before
    /// `STEP040` writes them again (`creastmt`; CBSTM03A-002). Requiring the lock makes the
    /// removal by name safe: no other run can be writing that file (W005-F01).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `removing previous output` or `creating output file` for
    /// the step that failed. A missing previous file is no error.
    pub(crate) async fn replace(lock: &DirectoryLock, file_name: &str) -> Result<Self, JobError> {
        let path = lock.dir().join(file_name);
        blocking(REMOVING_PREVIOUS_OUTPUT, move || {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(io(REMOVING_PREVIOUS_OUTPUT)(error).into()),
            }
            Self::create(path)
        })
        .await?
        .map_err(Failure::reported)
    }

    /// Creates the file at `path`, failing when anything is already there, and records its
    /// identity from the open handle. The guard is armed only once both are done, so a failed
    /// creation never removes another file. A file whose identity cannot be read is removed
    /// again by name, as the call created it a moment ago under a name only this run uses: a
    /// fresh generation, or a fixed name under the directory's lock. Runs inside a [`blocking`]
    /// step.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `creating output file` when the file cannot be created or
    /// its identity read, with cleanup `removing unfinished output` when it then cannot be
    /// removed.
    fn create(path: PathBuf) -> Result<Self, Failure> {
        let file = output_file_options()
            .open(&path)
            .map_err(io(CREATING_OUTPUT_FILE))?;
        let identity = match file.metadata() {
            Ok(metadata) => FileIdentity::of(&metadata),
            Err(error) => {
                drop(file);
                return Err(Failure::after_cleanup(
                    io(CREATING_OUTPUT_FILE)(error),
                    REMOVING_UNFINISHED_OUTPUT,
                    remove_by_name(&path),
                ));
            }
        };
        Ok(Self {
            path,
            identity,
            file: Arc::new(file),
            pending: Vec::with_capacity(BUFFER_CAPACITY),
            armed: true,
        })
    }

    /// Which file this is: what a later cleanup of its path compares with, once
    /// [`OutputFile::finish`] has given up the value (`creastmt`, which removes its completed
    /// files when a later one fails).
    pub(crate) fn identity(&self) -> FileIdentity {
        self.identity
    }

    /// Appends one fixed-length record and its LF terminator (AAP 0.3.4.3).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the bytes cannot be written.
    pub(crate) async fn write_record(&mut self, record: &[u8]) -> Result<(), JobError> {
        self.append(record).await?;
        self.append(b"\n").await
    }

    /// Appends `bytes` as they are, with no terminator: the content of `STATEMNT.PDF`
    /// (`creastmt`). Bytes of at least [`BUFFER_CAPACITY`] go to the file at once, after any
    /// bytes buffered before them, without being copied.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the bytes cannot be written.
    pub(crate) async fn write_bytes(&mut self, bytes: Vec<u8>) -> Result<(), JobError> {
        if bytes.len() < BUFFER_CAPACITY {
            return self.append(&bytes).await;
        }
        self.flush().await?;
        self.write_through(bytes).await
    }

    /// Reads the file back from its start through the job's own handle, never by reopening the
    /// path: `work` gets the reader in a [`blocking`] step and its result is returned. The
    /// buffered bytes are written first, so `work` reads every byte appended so far, and the
    /// handle is left at the end of the file, so a later append still follows them
    /// (`creastmt`, which renders `STATEMNT.PDF` from `STATEMNT.PS`).
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the buffered bytes cannot be
    /// written, and with `context` when the handle cannot be positioned or the step was
    /// cancelled. A failed read is `work`'s to report through its result.
    pub(crate) async fn reread<T: Send + 'static>(
        &mut self,
        context: &'static str,
        work: impl FnOnce(BufReader<&File>) -> T + Send + 'static,
    ) -> Result<T, JobError> {
        self.flush().await?;
        let file = Arc::clone(&self.file);
        blocking(context, move || -> io::Result<T> {
            let mut handle: &File = &file;
            handle.seek(SeekFrom::Start(0))?;
            let value = work(BufReader::new(handle));
            handle.seek(SeekFrom::End(0))?;
            Ok(value)
        })
        .await?
        .map_err(io(context))
    }

    /// Writes the bytes, syncs the file to disk, keeps it and returns its path, the one to
    /// report in [`JobOutcome::written`].
    ///
    /// The file is synced as well as written, so that a write the operating system could only
    /// fail later, such as on a full disk, fails the job instead of leaving a short file behind.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `finishing output` when the buffered bytes cannot be
    /// written or synced. The file is removed then, as the job ends without it, while its path
    /// still names it; a removal that fails is reported as cleanup `removing unfinished output`
    /// (W015-F02), and the error returned is still the write's.
    pub(crate) async fn finish(mut self) -> Result<PathBuf, JobError> {
        let pending = std::mem::take(&mut self.pending);
        let file = Arc::clone(&self.file);
        let completed = blocking(FINISHING_OUTPUT, move || {
            (&*file).write_all(&pending).and_then(|()| file.sync_all())
        })
        .await
        .and_then(|written| written.map_err(io(FINISHING_OUTPUT)));
        // From here this call, not `Drop`, keeps or removes the file.
        self.armed = false;
        let path = std::mem::take(&mut self.path);
        match completed {
            Ok(()) => Ok(path),
            Err(error) => {
                remove_outputs(REMOVING_UNFINISHED_OUTPUT, vec![(path, self.identity)]).await;
                Err(error)
            }
        }
    }

    /// Appends `bytes` with the policy of `std::io::BufWriter::write_all`: the buffered bytes are
    /// written first when `bytes` does not fit beside them, then `bytes` of at least
    /// [`BUFFER_CAPACITY`] go to the file at once and shorter ones are buffered.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when a write fails.
    async fn append(&mut self, bytes: &[u8]) -> Result<(), JobError> {
        let spare = BUFFER_CAPACITY.saturating_sub(self.pending.len());
        if bytes.len() > spare {
            self.flush().await?;
        }
        if bytes.len() >= BUFFER_CAPACITY {
            self.write_through(bytes.to_vec()).await
        } else {
            self.pending.extend_from_slice(bytes);
            Ok(())
        }
    }

    /// Writes the buffered bytes, if any, to the file.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the write fails.
    async fn flush(&mut self) -> Result<(), JobError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::replace(&mut self.pending, Vec::with_capacity(BUFFER_CAPACITY));
        self.write_through(bytes).await
    }

    /// Writes `bytes` to the file in one [`blocking`] step.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `writing output` when the write fails or the step was
    /// cancelled.
    async fn write_through(&self, bytes: Vec<u8>) -> Result<(), JobError> {
        let file = Arc::clone(&self.file);
        blocking(WRITING_OUTPUT, move || (&*file).write_all(&bytes))
            .await?
            .map_err(io(WRITING_OUTPUT))
    }
}

impl Drop for OutputFile {
    fn drop(&mut self) {
        if self.armed {
            // The last-resort guard of a job that returns early or is cancelled. A drop cannot
            // await a blocking step, and a step spawned here would be one nobody waits for, so
            // the file is removed in place. The buffered bytes are discarded with the value; the
            // handle closes once its last owner drops it, and removing a file still open is
            // allowed on Unix, while Windows std opens every file with delete sharing. The path
            // is removed only while it still names this file (W005-F01), and a failed removal is
            // reported like every other cleanup failure (W015-F02).
            if let Err(kind) = remove_output(&self.path, self.identity) {
                cleanup_failed(REMOVING_UNFINISHED_OUTPUT, kind);
            }
        }
    }
}

/// The exclusive use of one output directory by one run at a time, across processes and
/// namespaces: the guard of the three fixed-name statement files of `creastmt` (W005-F01;
/// CBSTM03A-002, CBSTM03A-049, CBSTM03A-050).
///
/// `STATEMNT.PS`, `STATEMNT.HTML` and `STATEMNT.PDF` keep their names from run to run: a run
/// removes the previous run's files by name. Two runs over one directory could therefore remove
/// each other's files, or report another run's bytes under their own path. A run takes this lock
/// before it removes the previous files and keeps it until it has completed or removed its own,
/// so the three files are one set that one run at a time touches, and a run's paths always hold
/// its own bytes. A lock inside the process would not do: two runs are two `carddemo-batch`
/// processes, possibly in two containers that share the directory. The lock is therefore an
/// entry of the directory itself, [`LOCK_NAME`], which every process that reaches the directory
/// finds, whatever its network, mount or PID namespace. A run that finds the directory held
/// waits until the holder ends, as a z/OS job waits on the enqueue of a data set another job has
/// allocated, rather than failing.
///
/// - Linux: the lock name is a Unix socket file that listens while its run holds the lock and
///   never accepts a connection. A connection to a socket file reaches the socket through the
///   file's inode, from any namespace that sees the directory, and succeeds exactly while the
///   holding process keeps the socket open, which the kernel ends when the process ends, however
///   it ends. A run takes the lock by binding a listening socket at a claim name of its own,
///   `.carddemo-batch.claim.<pid>.<nanos>.<sequence>`, linking that file as the lock name, which
///   fails while the name exists, and removing the claim name. As the socket listens before the
///   lock name exists, a refused connection to a lock name that names the same file before and
///   after proves its holder ended without releasing it, as a killed run does: that lock is
///   broken, moved to a `.carddemo-batch.stale.` name and removed there, and taken at once,
///   without waiting. A run that finds a live holder keeps its connection, which the kernel ends
///   when the holder's socket closes, and tries again then, at most once per [`LOCK_RETRY`]; so
///   each waiting run queues one connection on the holder's socket, which `std` listens with at
///   the system's maximum backlog, and once that many runs wait, a further run's connection
///   blocks until the holder ends, which is the same wait, though without its warning. Every
///   entry is addressed as
///   `/proc/self/fd/<descriptor>/<name>` through a handle on the directory kept for the lock's
///   lifetime, so that a socket path fits the 108 bytes of a socket address whatever the length
///   of the directory's own path: a Linux run needs `/proc` mounted. One race remains: when a
///   lock is broken just as a second run takes the freed name, the breaking run finds the moved
///   file is not the dead one and links it back, unless a third run has taken the name
///   meanwhile; it needs a killed holder and three runs within one probe.
/// - Other platforms: the lock is the file `.carddemo-batch.lock`, created exclusively and
///   removed at release while it is still the file the run created. A run killed before its end
///   leaves the file behind, and the next run then waits until an operator removes it.
///
/// [`DirectoryLock::release`] gives the lock up once the run is done with its files; a value
/// dropped unreleased gives it up in place.
pub(crate) struct DirectoryLock {
    /// The directory held, as the job named it.
    dir: PathBuf,
    /// The lock while this run holds it, until [`DirectoryLock::release`] or the drop gives it
    /// up.
    held: Option<Held>,
}

impl DirectoryLock {
    /// Takes the lock of `dir`, creating `dir` and its missing ancestors owner-only first (see
    /// [`create_output_dir`]). A lock whose holder has ended is taken over at once. When a live
    /// run holds the lock, one warning event says so and the call waits until that run ends.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `creating output directory` when `dir` cannot be created,
    /// and with context `locking output directory` when the lock cannot be taken for any other
    /// reason than another run holding it; a lock entry the attempt made and could not remove
    /// again is reported as cleanup `removing output directory lock` (W015-F02).
    pub(crate) async fn acquire(dir: &Path) -> Result<Self, JobError> {
        let dir = dir.to_path_buf();
        let target = dir.clone();
        let first = blocking(CREATING_OUTPUT_DIRECTORY, move || {
            create_output_dir(&target).map_err(io(CREATING_OUTPUT_DIRECTORY))?;
            Held::attempt(&target)
        })
        .await?
        .map_err(Failure::reported)?;
        let held = match first {
            Attempt::Taken(held) => held,
            Attempt::Busy(busy) => {
                tracing::warn!("another run holds the output directory; waiting for it to end");
                blocking(LOCKING_OUTPUT_DIRECTORY, move || Held::wait(busy))
                    .await?
                    .map_err(Failure::reported)?
            }
        };
        Ok(Self {
            dir,
            held: Some(held),
        })
    }

    /// The directory the lock holds.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Gives the lock up in a [`blocking`] step: removes the lock entry while it is still this
    /// run's, then closes it, so that a waiting run takes the lock. `creastmt` calls this once
    /// its files are completed or removed. A removal that fails is reported as cleanup
    /// `removing output directory lock` (W015-F02); on Linux the entry left is the lock of a run
    /// that has ended, which the next run breaks.
    pub(crate) async fn release(mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        match blocking(REMOVING_DIRECTORY_LOCK, move || held.release()).await {
            Ok(Ok(())) => {}
            Ok(Err(kind)) => cleanup_failed(REMOVING_DIRECTORY_LOCK, kind),
            // A cancelled step never ran: the lock closed with it and its entry stays.
            Err(_cancelled) => cleanup_failed(REMOVING_DIRECTORY_LOCK, ErrorKind::Interrupted),
        }
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            // The last-resort release of a run that returns early or is cancelled before
            // `release`. A drop cannot await a blocking step, and a step spawned here would be
            // one nobody waits for, so the lock is given up in place. A failed removal is
            // reported like every other cleanup failure (W015-F02).
            if let Err(kind) = held.release() {
                cleanup_failed(REMOVING_DIRECTORY_LOCK, kind);
            }
        }
    }
}

/// What one attempt at the lock of an output directory found.
enum Attempt {
    /// The lock is this run's.
    Taken(Held),
    /// Another run holds it: what the wait for that run's end needs.
    Busy(Busy),
}

/// The lock of an output directory on Linux, while this run holds it.
#[cfg(target_os = "linux")]
struct Held {
    /// The open directory, through which every lock entry is addressed.
    directory: File,
    /// The socket the lock name names, listening until this value drops; it never accepts.
    _listener: UnixListener,
    /// The identity of the socket file, by which the release knows the lock name is its own.
    identity: FileIdentity,
}

/// What a run that found a live holder of the lock waits with, on Linux.
#[cfg(target_os = "linux")]
struct Busy {
    /// The open directory, as the lock keeps it.
    directory: File,
    /// The connection to the holder's socket, which the kernel ends when that socket closes.
    holder: UnixStream,
}

/// What a probe of an existing lock name found, on Linux.
#[cfg(target_os = "linux")]
enum Probe {
    /// A live run holds the lock: the connection made to its socket.
    Live(UnixStream),
    /// The name is gone, or names another file than before: link again.
    Changed,
    /// The name still names the socket of a run that ended without giving the lock up.
    Dead(FileIdentity),
}

#[cfg(target_os = "linux")]
impl Held {
    /// One attempt at the lock of the existing directory `dir`, through a new handle on it.
    /// Runs inside a [`blocking`] step.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `locking output directory` when `dir` cannot be opened,
    /// and as [`Held::attempt_in`].
    fn attempt(dir: &Path) -> Result<Attempt, Failure> {
        let directory = File::open(dir).map_err(io(LOCKING_OUTPUT_DIRECTORY))?;
        Self::attempt_in(directory)
    }

    /// One attempt at the lock of the open `directory`: binds a listening socket at a claim name
    /// of its own, links it as the lock name, breaking on the way a lock whose holder has ended,
    /// and removes the claim name, which leaves the socket reachable through the lock name alone.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `locking output directory` for the step that failed. A lock
    /// taken whose claim name cannot be removed is released again; a lock entry the attempt made
    /// and could not remove is reported as cleanup `removing output directory lock`.
    fn attempt_in(directory: File) -> Result<Attempt, Failure> {
        let (listener, claim) =
            Self::bind_claim(&directory).map_err(io(LOCKING_OUTPUT_DIRECTORY))?;
        let lock = Self::entry(&directory, LOCK_NAME);
        let linked = fs::symlink_metadata(&claim).and_then(|metadata| {
            let identity = FileIdentity::of(&metadata);
            Self::link(&directory, &claim, &lock).map(|holder| (identity, holder))
        });
        let unclaimed = remove_by_name(&claim);
        match (linked, unclaimed) {
            (Ok((identity, None)), Ok(())) => Ok(Attempt::Taken(Self {
                directory,
                _listener: listener,
                identity,
            })),
            (Ok((identity, None)), Err(kind)) => {
                let held = Self {
                    directory,
                    _listener: listener,
                    identity,
                };
                Err(Failure::after_cleanup(
                    JobError::Io {
                        context: LOCKING_OUTPUT_DIRECTORY,
                        kind,
                    },
                    REMOVING_DIRECTORY_LOCK,
                    held.release(),
                ))
            }
            (Ok((_, Some(holder))), Ok(())) => Ok(Attempt::Busy(Busy { directory, holder })),
            (Ok((_, Some(_))), Err(kind)) => Err(JobError::Io {
                context: LOCKING_OUTPUT_DIRECTORY,
                kind,
            }
            .into()),
            (Err(error), unclaimed) => Err(Failure::after_cleanup(
                io(LOCKING_OUTPUT_DIRECTORY)(error),
                REMOVING_DIRECTORY_LOCK,
                unclaimed,
            )),
        }
    }

    /// The path of entry `name` of `directory` through this process's descriptor of it, which
    /// a socket address holds whatever the length of the directory's own path.
    fn entry(directory: &File, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}/{name}", directory.as_raw_fd()))
    }

    /// Binds a listening socket at a new claim name in `directory`, and gives it with the claim
    /// name's path. A name already bound, as one of the same process ID in another PID namespace
    /// may be, is passed over for the next.
    ///
    /// # Errors
    ///
    /// The I/O error of a bind that failed for another reason.
    fn bind_claim(directory: &File) -> io::Result<(UnixListener, PathBuf)> {
        loop {
            let claim = Self::entry(directory, &unique_name(CLAIM_PREFIX));
            match UnixListener::bind(&claim) {
                Ok(listener) => return Ok((listener, claim)),
                Err(error) if error.kind() == ErrorKind::AddrInUse => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// Links `claim` as the lock name `lock`: `None` once linked, or the connection to the
    /// socket of the live run that holds the lock. A lock whose holder has ended is broken on the
    /// way and the link tried again, as it is when the lock name changed during the probe.
    ///
    /// # Errors
    ///
    /// The I/O error of a link, probe or break that failed.
    fn link(directory: &File, claim: &Path, lock: &Path) -> io::Result<Option<UnixStream>> {
        loop {
            match fs::hard_link(claim, lock) {
                Ok(()) => return Ok(None),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            match Self::probe(lock)? {
                Probe::Live(holder) => return Ok(Some(holder)),
                Probe::Changed => {}
                Probe::Dead(dead) => Self::break_dead(directory, lock, dead)?,
            }
        }
    }

    /// Finds out whether the run whose socket the existing lock name `lock` names still runs: a
    /// connection succeeds while that socket listens and is refused once it has closed. The name
    /// is read before and after a refused connection, so that the refusal counts only for the
    /// file the name named throughout.
    ///
    /// # Errors
    ///
    /// The I/O error of a read of the name, or of a connection that failed otherwise.
    fn probe(lock: &Path) -> io::Result<Probe> {
        let before = match fs::symlink_metadata(lock) {
            Ok(metadata) => FileIdentity::of(&metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Probe::Changed),
            Err(error) => return Err(error),
        };
        match UnixStream::connect(lock) {
            Ok(holder) => Ok(Probe::Live(holder)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Probe::Changed),
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                match fs::symlink_metadata(lock) {
                    Ok(metadata) if FileIdentity::of(&metadata) == before => {
                        Ok(Probe::Dead(before))
                    }
                    Ok(_) => Ok(Probe::Changed),
                    Err(error) if error.kind() == ErrorKind::NotFound => Ok(Probe::Changed),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Breaks the lock whose name `lock` still names `dead`, the socket of a run that ended
    /// without giving the lock up: moves the name to a new stale name, then removes that. When
    /// the moved file is not `dead`, a run took the freed name between the probe and the move,
    /// and its lock is linked back under the lock name, unless a third run has taken the name
    /// meanwhile: the one race [`DirectoryLock`] leaves.
    ///
    /// # Errors
    ///
    /// The I/O error of a move, a link back or a removal that failed. The stale name is removed
    /// whatever happened before it.
    fn break_dead(directory: &File, lock: &Path, dead: FileIdentity) -> io::Result<()> {
        let stale = Self::entry(directory, &unique_name(STALE_PREFIX));
        match fs::rename(lock, &stale) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        let moved_dead =
            fs::symlink_metadata(&stale).is_ok_and(|moved| FileIdentity::of(&moved) == dead);
        let restored = if moved_dead {
            Ok(())
        } else {
            match fs::hard_link(&stale, lock) {
                Err(error) if error.kind() != ErrorKind::AlreadyExists => Err(error),
                _ => Ok(()),
            }
        };
        let removed = remove_by_name(&stale).map_err(io::Error::from);
        restored.and(removed)
    }

    /// Waits until the live holder `busy` found ends, then tries again, waiting the same way for
    /// every run that takes the lock first, until it is this run's. Attempts are at least
    /// [`LOCK_RETRY`] apart, so that a holder ending connections without ending is not tried
    /// without pause. Runs inside a [`blocking`] step.
    ///
    /// # Errors
    ///
    /// As [`Held::attempt_in`].
    fn wait(mut busy: Busy) -> Result<Self, Failure> {
        loop {
            let Busy { directory, holder } = busy;
            let started = Instant::now();
            Self::await_end(holder);
            if let Some(rest) = LOCK_RETRY.checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }
            busy = match Self::attempt_in(directory)? {
                Attempt::Taken(held) => return Ok(held),
                Attempt::Busy(next) => next,
            };
        }
    }

    /// Blocks until the connection `holder` ends, which the kernel does when the holder's socket
    /// closes, as its run releases the lock or ends. A holder sends nothing; a byte one sends
    /// anyway is passed over. The end of the connection, or any failure, ends the wait: the next
    /// attempt finds out what happened.
    fn await_end(holder: UnixStream) {
        for received in holder.bytes() {
            if received.is_err() {
                break;
            }
        }
    }

    /// Gives the lock up: removes the lock name while it still names this lock's socket, then
    /// closes the socket and the directory handle as `self` drops. The name goes first, while
    /// the socket still listens, so that no run finds the lock dead and breaks it meanwhile.
    ///
    /// # Errors
    ///
    /// The kind of a removal that failed.
    fn release(self) -> Result<(), ErrorKind> {
        remove_output(&Self::entry(&self.directory, LOCK_NAME), self.identity)
    }
}

/// A new name of `prefix` for an entry of an output directory: the process ID, the time in
/// nanoseconds and the process's next sequence number, so that runs in different PID
/// namespaces, whose process IDs may be equal, still pick different names.
#[cfg(target_os = "linux")]
fn unique_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let sequence = ENTRY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{}.{nanos}.{sequence}", std::process::id())
}

/// The lock of an output directory on other platforms than Linux, while this run holds it.
#[cfg(not(target_os = "linux"))]
struct Held {
    /// The lock file.
    lock_file: PathBuf,
    /// The lock file's identity, by which the release knows the file is still its own.
    identity: FileIdentity,
}

/// What a run that found the lock file present waits with, on other platforms than Linux.
#[cfg(not(target_os = "linux"))]
struct Busy {
    /// The directory held.
    dir: PathBuf,
}

#[cfg(not(target_os = "linux"))]
impl Held {
    /// One attempt at the lock of the existing directory `dir`: creates its lock file
    /// exclusively and records its identity. Busy when the file exists, that is, another run
    /// holds the directory or a killed run left the file. Runs inside a [`blocking`] step.
    ///
    /// # Errors
    ///
    /// [`JobError::Io`] with context `locking output directory` when the lock file cannot be
    /// created, or its identity read, for another reason. A file whose identity cannot be read
    /// is removed again by name, with cleanup `removing output directory lock` when that fails.
    fn attempt(dir: &Path) -> Result<Attempt, Failure> {
        let lock_file = dir.join(LOCK_NAME);
        let file = match output_file_options().open(&lock_file) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Ok(Attempt::Busy(Busy {
                    dir: dir.to_path_buf(),
                }));
            }
            Err(error) => return Err(io(LOCKING_OUTPUT_DIRECTORY)(error).into()),
        };
        match file.metadata() {
            Ok(metadata) => Ok(Attempt::Taken(Self {
                lock_file,
                identity: FileIdentity::of(&metadata),
            })),
            Err(error) => {
                drop(file);
                Err(Failure::after_cleanup(
                    io(LOCKING_OUTPUT_DIRECTORY)(error),
                    REMOVING_DIRECTORY_LOCK,
                    remove_by_name(&lock_file),
                ))
            }
        }
    }

    /// Tries [`Held::attempt`] every [`LOCK_RETRY`] until it takes the lock. Runs inside a
    /// [`blocking`] step.
    ///
    /// # Errors
    ///
    /// As [`Held::attempt`].
    fn wait(busy: Busy) -> Result<Self, Failure> {
        let Busy { dir } = busy;
        loop {
            std::thread::sleep(LOCK_RETRY);
            if let Attempt::Taken(held) = Self::attempt(&dir)? {
                return Ok(held);
            }
        }
    }

    /// Gives the lock up: removes the lock file while it is still the file this run created.
    ///
    /// # Errors
    ///
    /// The kind of a removal that failed.
    fn release(self) -> Result<(), ErrorKind> {
        remove_output(&self.lock_file, self.identity)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::io::{ErrorKind, Read as _};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use copybook::PicU;
    use domain::batch::cbact04c::IntcalcError;
    use domain::batch::cbtrn03c::ReportError;
    use test_support::trace_capture;
    use tracing::instrument::WithSubscriber as _;

    use super::{
        Attempt, DirectoryLock, Failure, Held, JobError, LOCK_NAME, LOCK_RETRY, OutputFile,
        blocking, read_input, read_named_input, remove_outputs,
    };

    type TestResult = Result<(), Box<dyn Error>>;

    /// The entry names of `dir`, sorted.
    fn names(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
        let mut names = fs::read_dir(dir)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        names.sort();
        Ok(names)
    }

    /// Whether one more attempt at the lock of `dir` finds it held. A lock the attempt takes
    /// is released again at once.
    fn held_elsewhere(dir: &Path) -> Result<bool, Box<dyn Error>> {
        match Held::attempt(dir).map_err(Failure::reported)? {
            Attempt::Busy(_) => Ok(true),
            Attempt::Taken(held) => {
                held.release().map_err(std::io::Error::from)?;
                Ok(false)
            }
        }
    }

    /// A socket listening at the lock name of the existing directory `dir`, bound as a run in
    /// another network namespace binds it: the two meet only through the directory.
    #[cfg(target_os = "linux")]
    fn foreign_holder(dir: &Path) -> Result<std::os::unix::net::UnixListener, Box<dyn Error>> {
        let directory = fs::File::open(dir)?;
        Ok(std::os::unix::net::UnixListener::bind(Held::entry(
            &directory, LOCK_NAME,
        ))?)
    }

    /// Tells apart the scratch directories of the tests of one process.
    static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

    /// A directory of its own under the system temporary directory, removed with its content
    /// when dropped. The load tests of `loads` and `dusrsecj` write their inputs into one.
    pub(super) struct Scratch(PathBuf);

    impl Scratch {
        pub(super) fn new() -> Result<Self, Box<dyn Error>> {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "carddemo-batch-jobs-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }

        pub(super) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Records keep their order and LF whether they stay within one buffer or cross it many
    /// times: 2 short records, then 100 records of 430 bytes, five buffers' worth.
    #[tokio::test]
    async fn records_end_with_lf() -> TestResult {
        let long: &[u8] = &[b'R'; 430];
        let cases: [&[&[u8]]; 2] = [&[b"rec1", b"rec2"], &[long; 100]];
        for records in cases {
            let scratch = Scratch::new()?;
            let mut output = OutputFile::next_generation(scratch.path(), "DALYREJS").await?;
            let mut expected = Vec::new();
            for record in records {
                output.write_record(record).await?;
                expected.extend_from_slice(record);
                expected.push(b'\n');
            }
            let path = output.finish().await?;
            assert!(fs::read(&path)? == expected, "{} records", records.len());
        }
        Ok(())
    }

    /// Raw bytes get no terminator, and bytes larger than the buffer, written at once, still
    /// follow the bytes buffered before them.
    #[tokio::test]
    async fn raw_bytes_get_no_terminator() -> TestResult {
        let scratch = Scratch::new()?;
        let lock = DirectoryLock::acquire(scratch.path()).await?;
        let mut output = OutputFile::replace(&lock, "STATEMNT.PDF").await?;
        let large = vec![b'x'; 3 * super::BUFFER_CAPACITY];
        output.write_bytes(b"%PDF-1.3".to_vec()).await?;
        output.write_bytes(large.clone()).await?;
        output.write_bytes(b" %%EOF".to_vec()).await?;
        let path = output.finish().await?;
        let mut expected = b"%PDF-1.3".to_vec();
        expected.extend_from_slice(&large);
        expected.extend_from_slice(b" %%EOF");
        assert!(fs::read(&path)? == expected, "the bytes differ");
        Ok(())
    }

    /// A job reads back every byte it appended, the buffered ones included, through its own
    /// handle, and a later append still follows them.
    #[tokio::test]
    async fn reread_gives_the_bytes_so_far_and_appends_follow_them() -> TestResult {
        let scratch = Scratch::new()?;
        let mut output = OutputFile::next_generation(scratch.path(), "TRANREPT").await?;
        output.write_record(b"first").await?;
        output.write_record(b"second").await?;
        let read = output
            .reread("reading back", |mut reader| {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes).map(|_| bytes)
            })
            .await??;
        assert_eq!(read, b"first\nsecond\n");
        output.write_record(b"third").await?;
        let path = output.finish().await?;
        assert_eq!(fs::read(&path)?, b"first\nsecond\nthird\n");
        Ok(())
    }

    /// A file dropped unfinished is removed, whichever constructor made it, and its generation
    /// number is free again, as a data set deleted at abend is never catalogued.
    #[tokio::test]
    async fn guard_deletes_an_unfinished_file() -> TestResult {
        let scratch = Scratch::new()?;
        let lock = DirectoryLock::acquire(scratch.path()).await?;
        let outputs = [
            OutputFile::next_generation(scratch.path(), "SYSTRAN").await?,
            OutputFile::replace(&lock, "STATEMNT.HTML").await?,
        ];
        for mut output in outputs {
            output.write_record(b"partial").await?;
            let path = output.path.clone();
            assert!(path.exists(), "{}", path.display());
            drop(output);
            assert!(!path.exists(), "{}", path.display());
        }
        let again = OutputFile::next_generation(scratch.path(), "SYSTRAN")
            .await?
            .finish()
            .await?;
        assert_eq!(again, scratch.path().join("SYSTRAN.G0001"));
        Ok(())
    }

    #[tokio::test]
    async fn replace_removes_the_previous_file() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("statements");
        let lock = DirectoryLock::acquire(&dir).await?;
        let mut first = OutputFile::replace(&lock, "STATEMNT.PS").await?;
        first.write_record(b"old").await?;
        let first = first.finish().await?;
        let mut second = OutputFile::replace(&lock, "STATEMNT.PS").await?;
        second.write_record(b"new").await?;
        let second = second.finish().await?;
        assert_eq!(first, dir.join("STATEMNT.PS"));
        assert_eq!(second, first);
        assert_eq!(fs::read(&second)?, b"new\n");
        Ok(())
    }

    #[tokio::test]
    async fn generation_numbers_increase() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("dalyrejs");
        let first = OutputFile::next_generation(&dir, "DALYREJS")
            .await?
            .finish()
            .await?;
        let second = OutputFile::next_generation(&dir, "DALYREJS")
            .await?
            .finish()
            .await?;
        assert_eq!(first, dir.join("DALYREJS.G0001"));
        assert_eq!(second, dir.join("DALYREJS.G0002"));
        assert!(first.exists() && second.exists());
        Ok(())
    }

    /// No generation is named past generation 9999, whose file is left as it was: the error
    /// names the limit and the base, never the directory. Any other failure of the generation
    /// naming, such as a refused base name, stays an I/O error that keeps the kind only.
    #[tokio::test]
    async fn a_reached_generation_limit_is_named_and_overwrites_nothing() -> TestResult {
        let cases = [
            (
                "SYSTRAN",
                "allocating generation: generation limit 9999 reached for SYSTRAN",
            ),
            (
                "SYSTRAN/G",
                "allocating generation: invalid input parameter",
            ),
        ];
        for (base, text) in cases {
            let scratch = Scratch::new()?;
            let last = scratch.path().join("SYSTRAN.G9999");
            fs::write(&last, b"sentinel\n")?;
            let error = OutputFile::next_generation(scratch.path(), base)
                .await
                .err()
                .ok_or("a generation was allocated")?;
            let limit = matches!(error, JobError::GenerationLimit { base: "SYSTRAN" });
            assert_eq!(limit, base == "SYSTRAN", "{error}");
            assert_eq!(error.to_string(), text);
            assert_eq!(fs::read(&last)?, b"sentinel\n");
            assert_eq!(names(scratch.path())?, ["SYSTRAN.G9999"]);
        }
        Ok(())
    }

    /// A regular file in place of an output directory, or of one of its ancestors, is reported
    /// as not a directory by the step that met it, whichever way the job writes, and the file is
    /// left as it was; `main` then names the directory's setting.
    #[tokio::test]
    async fn a_file_in_place_of_an_output_directory_is_not_a_directory() -> TestResult {
        let scratch = Scratch::new()?;
        let file = scratch.path().join("var");
        fs::write(&file, b"sentinel\n")?;
        for dir in [file.clone(), file.join("reports")] {
            let generation = OutputFile::next_generation(&dir, "TRANREPT")
                .await
                .err()
                .ok_or("a generation was allocated")?;
            let lock = DirectoryLock::acquire(&dir)
                .await
                .err()
                .ok_or("a directory lock was taken")?;
            assert_eq!(
                [generation.to_string(), lock.to_string()],
                [
                    "allocating generation: not a directory",
                    "creating output directory: not a directory"
                ],
                "{}",
                dir.display()
            );
            let named = lock.naming_output_dir("statements under CARDDEMO_VAR_DIR");
            assert!(
                matches!(
                    named,
                    JobError::OutputDirectory {
                        context: "creating output directory",
                        setting: "statements under CARDDEMO_VAR_DIR",
                        kind: ErrorKind::NotADirectory,
                    }
                ),
                "{named}"
            );
            assert_eq!(named.exit_code(), None);
        }
        assert_eq!(fs::read(&file)?, b"sentinel\n");
        assert_eq!(names(scratch.path())?, ["var"]);
        Ok(())
    }

    /// Generation and statement files are created readable and writable by their owner only,
    /// and every directory a job creates is accessible to its owner only, under any umask that
    /// leaves the owner's bits (W005-F06).
    #[cfg(unix)]
    #[tokio::test]
    async fn outputs_and_their_new_directories_are_owner_only() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = Scratch::new()?;
        let generations = scratch.path().join("var").join("dalyrejs");
        let statements = scratch.path().join("stmt").join("statements");
        let generation = OutputFile::next_generation(&generations, "DALYREJS")
            .await?
            .finish()
            .await?;
        let lock = DirectoryLock::acquire(&statements).await?;
        let statement = OutputFile::replace(&lock, "STATEMNT.PS")
            .await?
            .finish()
            .await?;
        let expected = [
            (generation, 0o600),
            (statement, 0o600),
            (generations, 0o700),
            (scratch.path().join("var"), 0o700),
            (statements, 0o700),
            (scratch.path().join("stmt"), 0o700),
        ];
        for (path, owner) in expected {
            let mode = fs::metadata(&path)?.permissions().mode();
            assert_eq!(
                mode & 0o077,
                0,
                "{} is open to others: {mode:o}",
                path.display()
            );
            assert_eq!(mode & 0o700, owner, "{}: {mode:o}", path.display());
        }
        Ok(())
    }

    /// A held output directory is held once: another attempt finds it busy, through any path
    /// to it, while another directory stays free; a second run waits, announced by one warning
    /// that names no path, and takes the lock once the holder ends. The lock is the one entry
    /// `.carddemo-batch.lock` of the directory (W005-F01).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_run_at_a_time_holds_an_output_directory() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("statements");
        let held = DirectoryLock::acquire(&dir).await?;
        assert_eq!(held.dir(), dir);
        for path in [dir.clone(), dir.join(".")] {
            assert!(
                held_elsewhere(&path)?,
                "{} was locked twice",
                path.display()
            );
        }
        let other = DirectoryLock::acquire(&scratch.path().join("other")).await?;
        assert_eq!(names(&dir)?, [LOCK_NAME], "the lock is not the one entry");

        let capture = trace_capture::capture();
        let target = dir.clone();
        let waiting = tokio::spawn(
            async move { DirectoryLock::acquire(&target).await }
                .with_subscriber(capture.dispatch()),
        );
        let mut announced = false;
        for _ in 0..1000 {
            if capture.contains(WAITING) {
                announced = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(announced, "the waiting run gave no warning");
        assert!(!waiting.is_finished(), "a held directory was taken");
        drop(held);
        let taken = waiting.await??;
        assert_eq!(taken.dir(), dir);
        let warnings: Vec<String> = capture
            .lines()
            .into_iter()
            .filter(|line| line.contains(WAITING))
            .collect();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .all(|line| line.contains(r#""level":"WARN""#))
        );
        assert!(!capture.contains(&scratch.path().display().to_string()));
        drop(other);
        Ok(())
    }

    /// The warning of a run that waits for a held output directory.
    const WAITING: &str = "another run holds the output directory; waiting for it to end";

    /// A file that cannot be removed is reported with the static cleanup operation and the
    /// error kind, never the path, by a guard and by an explicit cleanup alike; a file already
    /// gone is no failure and reports nothing (W015-F02).
    #[tokio::test]
    async fn a_failed_cleanup_is_reported_without_the_path() -> TestResult {
        let scratch = Scratch::new()?;
        let capture = trace_capture::capture();
        let gone = OutputFile::next_generation(scratch.path(), "SYSTRAN").await?;
        let identity = gone.identity();
        fs::remove_file(&gone.path)?;
        drop(gone);
        remove_outputs(
            COMPLETED,
            vec![(scratch.path().join("STATEMNT.PS"), identity)],
        )
        .await;
        assert!(
            !capture.contains(CLEANUP_FAILED),
            "a missing file was reported"
        );

        // A file in place of the output's directory: the path can no longer be read, so the
        // output it named may remain.
        let dir = scratch.path().join("dalyrejs");
        let stuck = OutputFile::next_generation(&dir, "DALYREJS").await?;
        let path = stuck.path.clone();
        let identity = stuck.identity();
        let moved = scratch.path().join("moved");
        fs::rename(&dir, &moved)?;
        fs::write(&dir, b"")?;
        let kind = fs::symlink_metadata(&path)
            .err()
            .as_ref()
            .map(std::io::Error::kind)
            .ok_or("a path through a file was read")?;
        drop(stuck);
        remove_outputs(COMPLETED, vec![(path.clone(), identity)]).await;
        let reported: Vec<String> = capture
            .lines()
            .into_iter()
            .filter(|line| line.contains(CLEANUP_FAILED))
            .collect();
        assert_eq!(reported.len(), 2, "{reported:?}");
        for (line, operation) in reported
            .iter()
            .zip(["removing unfinished output", COMPLETED])
        {
            assert!(
                line.contains(&format!(r#""operation":"{operation}""#)),
                "{line}"
            );
            assert!(
                line.contains(&format!(r#""error_kind":"{kind}""#)),
                "{line}"
            );
            assert!(line.contains(r#""level":"ERROR""#), "{line}");
        }
        assert!(!capture.contains(&scratch.path().display().to_string()));
        assert!(!capture.contains("DALYREJS"), "an event names the file");
        assert!(
            moved.join("DALYREJS.G0001").exists(),
            "the moved output was removed"
        );
        Ok(())
    }

    /// The message of a failed cleanup.
    const CLEANUP_FAILED: &str = "output file cleanup failed";

    /// The bytes of a file another run puts under a name.
    const INTRUDER_BYTES: &[u8] = b"another run's bytes\n";

    /// What another run, or anything else, puts under the name of a file this run created.
    #[derive(Clone, Copy, Debug)]
    enum Intruder {
        /// A file with bytes of its own, as another run's output.
        File,
        /// A directory.
        Directory,
    }

    impl Intruder {
        /// Moves the file at `path` to `aside`, where it lives on and keeps its identity, then
        /// puts the intruder at `path`.
        fn take_place(self, path: &Path, aside: &Path) -> TestResult {
            fs::rename(path, aside)?;
            match self {
                Self::File => fs::write(path, INTRUDER_BYTES)?,
                Self::Directory => fs::create_dir(path)?,
            }
            Ok(())
        }

        /// Whether `path` still holds the intruder, its bytes included.
        fn intact(self, path: &Path) -> Result<bool, Box<dyn Error>> {
            Ok(match self {
                Self::File => fs::read(path)? == INTRUDER_BYTES,
                Self::Directory => path.is_dir(),
            })
        }

        /// Removes the intruder at `path`.
        fn remove(self, path: &Path) -> TestResult {
            match self {
                Self::File => fs::remove_file(path)?,
                Self::Directory => fs::remove_dir(path)?,
            }
            Ok(())
        }
    }

    /// A cleanup removes a path only while it names the file this run created: a file another
    /// run put under the name, bytes included, or a directory stays, whether the guard of an
    /// unfinished file of either constructor or the removal of a completed file, as `creastmt`
    /// makes it, cleans up; nothing is reported, and the run's own file is removed once its
    /// name names it again (W005-F01).
    #[tokio::test]
    async fn a_cleanup_leaves_what_another_run_put_under_the_name() -> TestResult {
        let scratch = Scratch::new()?;
        let lock = DirectoryLock::acquire(scratch.path()).await?;
        let capture = trace_capture::capture();
        let aside = scratch.path().join("aside");
        for intruder in [Intruder::File, Intruder::Directory] {
            let unfinished = [
                OutputFile::next_generation(scratch.path(), "SYSTRAN").await?,
                OutputFile::replace(&lock, "STATEMNT.PS").await?,
            ];
            for mut output in unfinished {
                output.write_record(b"partial").await?;
                let path = output.path.clone();
                intruder.take_place(&path, &aside)?;
                drop(output);
                assert!(intruder.intact(&path)?, "{intruder:?}: {}", path.display());
                intruder.remove(&path)?;
                fs::remove_file(&aside)?;
            }

            let completed = OutputFile::replace(&lock, "STATEMNT.HTML").await?;
            let identity = completed.identity();
            let path = completed.finish().await?;
            intruder.take_place(&path, &aside)?;
            remove_outputs(COMPLETED, vec![(path.clone(), identity)]).await;
            assert!(intruder.intact(&path)?, "{intruder:?}: a completed file");
            intruder.remove(&path)?;
            fs::rename(&aside, &path)?;
            remove_outputs(COMPLETED, vec![(path.clone(), identity)]).await;
            assert!(!path.exists(), "{intruder:?}: the run's own file stayed");
        }
        assert!(!capture.contains(CLEANUP_FAILED), "{:?}", capture.lines());
        lock.release().await;
        assert!(
            names(scratch.path())?.is_empty(),
            "{:?}",
            names(scratch.path())?
        );
        Ok(())
    }

    /// Holding a directory adds exactly the lock entry, whatever the length of the directory's
    /// path, beyond the 108 bytes of a socket address included; one more attempt finds it held
    /// and leaves no entry of its own; the release removes the entry, and the lock is free
    /// again (W005-F01).
    #[tokio::test]
    async fn a_held_directory_has_one_entry_that_the_release_removes() -> TestResult {
        let scratch = Scratch::new()?;
        let long = scratch.path().join("d".repeat(120)).join("statements");
        assert!(long.as_os_str().len() > 108, "{}", long.display());
        for dir in [scratch.path().join("statements"), long] {
            let lock = DirectoryLock::acquire(&dir).await?;
            assert_eq!(names(&dir)?, [LOCK_NAME], "{}", dir.display());
            assert!(held_elsewhere(&dir)?, "{} was locked twice", dir.display());
            assert_eq!(names(&dir)?, [LOCK_NAME], "{}", dir.display());
            let mut output = OutputFile::replace(&lock, "STATEMNT.PS").await?;
            output.write_record(b"statement").await?;
            let path = output.finish().await?;
            lock.release().await;
            assert_eq!(names(&dir)?, ["STATEMNT.PS"], "{}", dir.display());
            DirectoryLock::acquire(&dir).await?.release().await;
            assert_eq!(names(&dir)?, ["STATEMNT.PS"], "{}", dir.display());
            assert_eq!(fs::read(&path)?, b"statement\n");
        }
        Ok(())
    }

    /// Leaves the lock name of a run that has ended in the existing directory it is given.
    #[cfg(target_os = "linux")]
    type Leave = fn(&Path) -> TestResult;

    /// A lock whose holder ended without releasing it, as a killed run leaves its socket, or an
    /// entry of the lock name that is no socket, is taken over at once: no warning, no wait,
    /// and no claim or stale entry left behind; the lock taken over is held (W005-F01).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_lock_left_by_an_ended_run_is_taken_over_at_once() -> TestResult {
        let leftovers: [(&str, Leave); 2] = [
            ("a socket no process listens on", |dir| {
                drop(foreign_holder(dir)?);
                Ok(())
            }),
            ("a file", |dir| Ok(fs::write(dir.join(LOCK_NAME), b"")?)),
        ];
        for (leftover, leave) in leftovers {
            let scratch = Scratch::new()?;
            let dir = scratch.path().join("statements");
            fs::create_dir(&dir)?;
            leave(&dir)?;
            assert_eq!(names(&dir)?, [LOCK_NAME], "{leftover}");
            let capture = trace_capture::capture();
            let lock = DirectoryLock::acquire(&dir)
                .with_subscriber(capture.dispatch())
                .await?;
            assert!(!capture.contains(WAITING), "{leftover} was waited for");
            assert_eq!(names(&dir)?, [LOCK_NAME], "{leftover}");
            assert!(held_elsewhere(&dir)?, "{leftover}: the lock is not held");
            lock.release().await;
            assert!(names(&dir)?.is_empty(), "{leftover}: {:?}", names(&dir)?);
        }
        Ok(())
    }

    /// A live holder that the waiting run meets only through the directory, as a run of another
    /// network namespace, is waited for, announced by one warning, and the lock is taken once
    /// the holder's socket closes, though its name stays (W005-F01).
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_live_holder_met_only_through_the_directory_is_waited_for() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().join("statements");
        fs::create_dir(&dir)?;
        let holder = foreign_holder(&dir)?;
        let capture = trace_capture::capture();
        let target = dir.clone();
        let waiting = tokio::spawn(
            async move { DirectoryLock::acquire(&target).await }
                .with_subscriber(capture.dispatch()),
        );
        let mut announced = false;
        for _ in 0..1000 {
            if capture.contains(WAITING) {
                announced = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(announced, "the waiting run gave no warning");
        std::thread::sleep(LOCK_RETRY * 3);
        assert!(!waiting.is_finished(), "a held directory was taken");
        drop(holder);
        let taken = waiting.await??;
        assert_eq!(names(&dir)?, [LOCK_NAME]);
        let warnings = capture
            .lines()
            .into_iter()
            .filter(|line| line.contains(WAITING))
            .count();
        assert_eq!(warnings, 1, "{:?}", capture.lines());
        taken.release().await;
        assert!(names(&dir)?.is_empty(), "{:?}", names(&dir)?);
        Ok(())
    }

    /// The cleanup operation the tests pass to `remove_outputs`, as `creastmt` does.
    const COMPLETED: &str = "removing completed statement file";

    /// Work on the blocking pool gives its value, and work that panics panics its caller, as it
    /// would have inline.
    #[tokio::test]
    #[should_panic(expected = "blocking work failed")]
    async fn blocking_work_that_panics_panics_its_caller() {
        assert!(matches!(blocking("testing", || 7).await, Ok(7)));
        let _never: Result<(), JobError> =
            blocking("testing", || panic!("blocking work failed")).await;
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

    /// The input of a load that cannot be read is named as the job got it, never canonicalised,
    /// with the error's kind: a file under a missing directory, a directory, and a path whose
    /// control characters are shown escaped, so the report stays one line.
    // audit: LOADS-019
    #[test]
    fn an_unreadable_load_input_names_its_path_and_kind() -> TestResult {
        let scratch = Scratch::new()?;
        let dir = scratch.path().display().to_string();
        let cases = [
            (
                scratch.path().join("missing/../acctdata.txt"),
                ErrorKind::NotFound,
                format!("{dir}/missing/../acctdata.txt"),
            ),
            (
                scratch.path().to_path_buf(),
                ErrorKind::IsADirectory,
                dir.clone(),
            ),
            (
                scratch.path().join("acct\ndata\r\t\u{1b}\u{e9}.txt"),
                ErrorKind::NotFound,
                format!("{dir}/acct\\ndata\\r\\t\\u{{1b}}\u{e9}.txt"),
            ),
        ];
        for (path, kind, shown) in cases {
            let error = read_named_input(&path)
                .err()
                .ok_or("an unreadable input was read")?;
            assert!(
                matches!(&error, JobError::UnreadableInput { kind: found, .. } if *found == kind),
                "{error}"
            );
            let text = error.to_string();
            assert_eq!(text, format!("reading input {shown}: {kind}"));
            assert_eq!(format!("{error:?}"), text);
            assert!(!text.contains(['\n', '\r']), "{text}");
            assert_eq!(error.exit_code(), None, "{text}");
        }
        Ok(())
    }

    /// Each error's text, which `Debug` repeats, its exit code, and the record values it must
    /// not show.
    #[test]
    fn error_texts_and_exit_codes() -> TestResult {
        let cases: [(JobError, &str, Option<u8>, &[&str]); 7] = [
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
                JobError::GenerationLimit { base: "DALYREJS" },
                "allocating generation: generation limit 9999 reached for DALYREJS",
                None,
                &[],
            ),
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
