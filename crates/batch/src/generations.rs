//! Generation data groups (GDGs) as files, and the record-file helpers every batch job shares.
//!
//! # Generations
//!
//! A GDG generation is the file `<DATASET>.G<nnnn>V00` under `CARDDEMO_BATCH_DIR`
//! (`Settings::batch_dir`, default `./var/batch/`), for example `DALYREJS.G0001V00`.
//!
//! - `(0)` is the newest cataloged generation: [`Generations::current`] and
//!   [`Generations::require_current`] (COMBTRAN SORTIN `TRANSACT.BKUP(0)`, `SYSTRAN(0)`
//!   [app/jcl/COMBTRAN.jcl:L23-26]).
//! - `(+1)` is [`Generations::begin_next`]: the newest number + 1, starting at `G0001V00`. Every
//!   `(+1)` DD is `DISP=(NEW,CATLG,DELETE)` [app/jcl/POSTTRAN.jcl:L34-38]
//!   [app/jcl/INTCALC.jcl:L37-41] [app/jcl/TRANBKP.jcl:L29-33] [app/jcl/COMBTRAN.jcl:L33-37]
//!   [app/jcl/TRANREPT.jcl:L29-33, L51-55, L76-80]. The generation is written to a hidden
//!   temporary sibling; [`PendingGeneration::catalog`] links it into place when the step ends
//!   normally, and a [`PendingGeneration`] dropped uncataloged deletes it. An abend, a reward halt
//!   or an early `?` return therefore leaves the previous generation current, while generations
//!   cataloged by earlier steps stay.
//! - Within one job `(+1)` names the same generation in every step (TRANREPT's SORT reads
//!   `TRANSACT.BKUP(+1)` [app/jcl/TRANREPT.jcl:L38-39], COMBTRAN's REPRO reads
//!   `TRANSACT.COMBINED(+1)` [app/jcl/COMBTRAN.jcl:L43-44]). A later step reads the path of the
//!   [`Generation`] its earlier step cataloged.
//! - Every in-scope GDG is `LIMIT(5) SCRATCH` [app/jcl/DEFGDGB.jcl:STEP05]
//!   [app/jcl/DALYREJS.jcl:STEP05]: cataloging deletes the oldest generations until the newest
//!   [`LIMIT`] remain.
//!
//! # Lock files
//!
//! JES serializes jobs that allocate the same dataset with `DISP=NEW`, `MOD` or `OLD` through the
//! dataset enqueue. A [`LockFile`] stands for that enqueue: a hidden file created exclusively and
//! holding the process id, deleted when the value is dropped. A job that finds the file fails
//! with [`GenerationError::Locked`]: it neither waits nor takes the lock over. A job killed before
//! it could release the lock (`SIGKILL`, a power loss) leaves the file behind, and every later
//! job that needs the lock is refused until an operator removes it, which is safe only while no
//! batch job is running. Different locks are different files and are held side by side.
//!
//! - Allocation lock: JES enqueues a GDG that a job allocates `DISP=(NEW,CATLG,DELETE)`
//!   [app/jcl/POSTTRAN.jcl:L34-38]. [`Generations::begin_next`] takes `.<DATASET>.lock` in the
//!   catalog directory before it reads the current generation, and the [`PendingGeneration`]
//!   holds it until it is cataloged, roll-off included, or dropped, so two `(+1)` allocations of
//!   one GDG can never pick the same number. A held allocation lock is reported as
//!   [`GenerationError::InUse`], which also names the process the lock file records and whether
//!   it is running ([`LockHolder`]), read when the allocation is refused.
//! - Statement lock: `creastmt` takes `.STATEMNT.lock` in `CARDDEMO_STATEMENT_DIR` for the
//!   statement datasets it deletes and writes, from before `STEP030` to the end of the job.
//!
//! Every temporary file is created exclusively under a name unique to its process and allocation,
//! and [`PendingGeneration::catalog`] publishes a generation as a hard link to its final name,
//! which fails when that name exists, so a cataloged generation is never replaced.
//!
//! A job killed while it writes a generation also leaves that generation's temporary file
//! `.<DATASET>.G<nnnn>V00.tmp-<pid>-<seq>` behind. [`Generations::begin_next`] sweeps such orphans
//! once it holds the allocation lock and before it reads the current generation: it deletes each
//! regular file of that GDG named exactly so whose process id is its own, because the lock it
//! holds proves that the process has no pending generation of the GDG, or is that of a process
//! that is not running. Whether a process is running is read from Linux's `/proc/<pid>`; where
//! `/proc` is not mounted that cannot be told, and the file is kept. A sweep never fails the
//! allocation: a file that cannot be deleted is logged. Lock files are never swept.
//!
//! # Record files
//!
//! Record files use the ASCII codec layouts at full copybook width, each record followed by `LF`
//! ([`PendingGeneration::append_records`]). [`RecordReader`] reads them back frame by frame, each
//! frame the copybook width of record bytes and then that `LF`, one record at a time, so no job
//! holds a whole file; its records and errors are those `read_records`, the whole-file decode the
//! tests hold it to, gives for the same file. Alphanumeric fields copy bytes, so an `LF` or `CR` inside a record is
//! record data and never a record boundary, and a file that is not a whole number of such frames
//! does not decode. Print and statement files are
//! fixed-width text lines with `LF` terminators, checked by [`encode_fixed_lines`]: `TRANREPT`
//! 133 bytes (`LRECL=133` [app/jcl/TRANREPT.jcl:L78]), STMTFILE 80 and HTMLFILE 100
//! [app/cbl/CBSTM03A.CBL:L44-47]. A line must have exactly that width and hold no `LF` or `CR`,
//! because either byte would split one fixed-width record into several physical records or
//! merge it into its terminator; such a line is rejected, never trimmed, escaped or replaced,
//! and nothing is written. The statement files are cataloged datasets, not GDGs, and are
//! replaced whole by [`write_replacing`].
//!
//! # Sort work files
//!
//! A SORT step whose input does not fit its memory spills sorted runs to work datasets, the
//! `SORTWK` files DFSORT allocates for the step and deletes when it ends.
//! [`Generations::create_work_file`] creates one in the catalog directory, exclusively, as the
//! hidden temporary file `.SORTWK.tmp-<pid>-<seq>`, opened for reading and writing, and unlinks
//! that name at once: nothing names the file while the step runs, the system frees its space
//! when the step drops it, and a job killed at any point leaves nothing behind. Where the name
//! cannot be unlinked it stays until the work file is dropped, which deletes it. Neither
//! [`Generations::list`] nor the orphan sweep of [`Generations::begin_next`] ever takes such a
//! name for a generation or its temporary file. A [`WorkFileWriter`] appends through a buffer;
//! once finished, the [`WorkFile`] reads any part written back through buffered readers, as
//! often as the step needs. Every failure is a [`GenerationError::Io`] on the work file.
//!
//! # Execution
//!
//! Every file-system, codec and CPU operation of a generation is synchronous, and a job, a future
//! on Tokio's multi-threaded runtime, runs each one on Tokio's blocking pool: through
//! [`Generations::begin_next_async`], [`PendingGeneration::append_records_async`],
//! [`PendingGeneration::append_lines_async`], [`PendingGeneration::catalog_async`],
//! [`PendingGeneration::discard_async`], [`RecordReader::open_async`],
//! [`RecordReader::next_batch_async`] and [`count_records_async`], or through [`blocking`] for
//! any other phase. The work runs inside the caller's span, so its events and span records land
//! in the job span. The errors and outcomes are those of the synchronous operation each one runs;
//! a blocking task the runtime cancels because it is shutting down is reported as a
//! [`GenerationError::Io`] on the path the operation addressed. A job discards every generation it
//! abandons with [`PendingGeneration::discard_async`]; dropping a [`PendingGeneration`]
//! uncataloged stays the safety net of a panic.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::panic;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use carddemo_codec::layouts::{CodecError, Layout, encode_ascii_lf};
use carddemo_codec::{Charset, CodecErrorKind};
use tracing::Dispatch;

/// `LIMIT(5) SCRATCH`: the number of generations each GDG keeps [app/jcl/DEFGDGB.jcl:STEP05]
/// [app/jcl/DALYREJS.jcl:STEP05].
pub(crate) const LIMIT: usize = 5;

/// The first absolute generation number, `G0001V00`.
const FIRST_NUMBER: u16 = 1;

/// The highest absolute generation number, `G9999V00`.
const MAX_NUMBER: u16 = 9999;

/// How many temporary names one write tries before it gives up. A name is taken only by a file
/// left behind by an earlier process that had the same process id, so one retry nearly always
/// suffices.
const TEMP_ATTEMPTS: u32 = 64;

/// The sequence number that makes each temporary name of this process unique.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The error of a write to, or a catalog of, a generation whose temporary file is closed.
const CLOSED: &str = "generation file is already closed";

/// The name every sort work file is created under, as the hidden temporary file
/// `.SORTWK.tmp-<pid>-<seq>` of the catalog directory: DFSORT's `SORTWK` work datasets.
const SORT_WORK: &str = "SORTWK";

/// A GDG base that an in-scope job writes. `TCATBALF.BKUP`, also defined by DEFGDGB, has no
/// variant because no in-scope job writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dataset {
    /// `AWS.M2.CARDDEMO.DALYREJS` [app/jcl/DALYREJS.jcl:STEP05]: the POSTTRAN rejects, 430-byte
    /// records [app/jcl/POSTTRAN.jcl:L34-38].
    DalyRejs,
    /// `AWS.M2.CARDDEMO.SYSTRAN` [app/jcl/DEFGDGB.jcl:STEP05]: the INTCALC interest transactions,
    /// 350-byte records [app/jcl/INTCALC.jcl:L37-41].
    SysTran,
    /// `AWS.M2.CARDDEMO.TRANSACT.BKUP` [app/jcl/DEFGDGB.jcl:STEP05]: the transaction master backup
    /// that TRANBKP and TRANREPT write, 350-byte records [app/jcl/TRANBKP.jcl:L29-33]
    /// [app/jcl/TRANREPT.jcl:L29-33].
    TransactBkup,
    /// `AWS.M2.CARDDEMO.TRANSACT.COMBINED` [app/jcl/DEFGDGB.jcl:STEP05]: the COMBTRAN sort output,
    /// 350-byte records [app/jcl/COMBTRAN.jcl:L33-37].
    TransactCombined,
    /// `AWS.M2.CARDDEMO.TRANSACT.DALY` [app/jcl/DEFGDGB.jcl:STEP05]: the TRANREPT date-filtered
    /// sort output, 350-byte records [app/jcl/TRANREPT.jcl:L51-55].
    TransactDaly,
    /// `AWS.M2.CARDDEMO.TRANREPT` [app/jcl/DEFGDGB.jcl:STEP05]: the CBTRN03C transaction report,
    /// 133-byte lines [app/jcl/TRANREPT.jcl:L76-80].
    TranRept,
}

impl Dataset {
    /// The dataset name after the `AWS.M2.CARDDEMO.` qualifier: the generation file prefix.
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DalyRejs => "DALYREJS",
            Self::SysTran => "SYSTRAN",
            Self::TransactBkup => "TRANSACT.BKUP",
            Self::TransactCombined => "TRANSACT.COMBINED",
            Self::TransactDaly => "TRANSACT.DALY",
            Self::TranRept => "TRANREPT",
        }
    }
}

/// A cataloged generation: its GDG, its absolute number and its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Generation {
    /// The GDG the generation belongs to.
    pub(crate) dataset: Dataset,
    /// The absolute generation number, 1 to 9999.
    pub(crate) number: u16,
    /// The generation file, `<root>/<DATASET>.G<nnnn>V00`.
    pub(crate) path: PathBuf,
}

impl fmt::Display for Generation {
    /// The generation file name, for example `DALYREJS.G0001V00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&generation_name(self.dataset, self.number))
    }
}

/// The generation catalog: the GDGs under one directory, `Settings::batch_dir`.
#[derive(Debug, Clone)]
pub(crate) struct Generations {
    root: PathBuf,
}

/// A record delimiter byte that a fixed-width line must not contain, as
/// [`GenerationError::LineDelimiter`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delimiter {
    /// `LF`, `0x0A`: the terminator of every record.
    Lf,
    /// `CR`, `0x0D`: as the last byte of a line it would join the `LF` after it into a `CRLF`
    /// terminator.
    Cr,
}

impl Delimiter {
    /// The delimiter that `byte` is, or `None` for every other byte.
    const fn of(byte: u8) -> Option<Self> {
        match byte {
            b'\n' => Some(Self::Lf),
            b'\r' => Some(Self::Cr),
            _ => None,
        }
    }
}

impl fmt::Display for Delimiter {
    /// The delimiter's name and byte value, for example `LF (0x0A)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lf => "LF (0x0A)",
            Self::Cr => "CR (0x0D)",
        })
    }
}

/// A failure to read, write or catalog a generation or a record file.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GenerationError {
    /// A file-system operation failed on `path`: a generation, its temporary file, the catalog
    /// directory or a record file.
    #[error("I/O error on {}: {source}", path.display())]
    Io {
        /// The file or directory the operation addressed.
        path: PathBuf,
        /// The operating-system error.
        source: io::Error,
    },
    /// `(+1)` was requested while `G9999V00` is the newest generation. z/OS wraps around to
    /// `G0001V00`; that is not emulated.
    #[error("{dataset}(+1): G9999V00 is the newest generation and wrap-around is not emulated")]
    Exhausted {
        /// The dataset name, as [`Dataset::name`] gives it.
        dataset: &'static str,
    },
    /// `(+1)` was requested while the allocation lock of the GDG exists: another batch job is
    /// allocating it, or a job killed before it could release the lock left the file behind.
    /// The text names the lock file and its holder, and tells the operator when the file may be
    /// removed.
    #[error(
        "{dataset}(+1): the allocation lock {} is held by {holder}; remove that file only when \
         no batch job is running, because a job killed before releasing the lock leaves it \
         behind",
        path.display()
    )]
    InUse {
        /// The dataset name, as [`Dataset::name`] gives it.
        dataset: &'static str,
        /// The lock file, `<root>/.<DATASET>.lock`.
        path: PathBuf,
        /// The process the lock file records, read when the allocation was refused.
        holder: LockHolder,
    },
    /// A [`LockFile`] exists: another batch job holds the lock, or a job killed before it could
    /// release the lock left the file behind. The `(+1)` allocation lock reports this as
    /// [`GenerationError::InUse`].
    #[error(
        "another batch job holds the {name} {}; remove that file only when no batch job is \
         running, because a job killed before releasing the lock leaves it behind",
        path.display()
    )]
    Locked {
        /// What the lock stands for, as [`LockFile::acquire`] names it.
        name: &'static str,
        /// The lock file.
        path: PathBuf,
    },
    /// `(0)` was requested while no generation is cataloged.
    #[error("{dataset}(0): no generation is cataloged")]
    NotCataloged {
        /// The dataset name, as [`Dataset::name`] gives it.
        dataset: &'static str,
    },
    /// A fixed-width line does not have the record width in bytes.
    #[error("line {line} is {actual} bytes, expected {expected}")]
    LineWidth {
        /// The 1-based position of the line among the lines passed.
        line: usize,
        /// The record width in bytes.
        expected: usize,
        /// The line length in bytes.
        actual: usize,
    },
    /// A fixed-width line holds a record delimiter, which would split it into more records or
    /// merge it into the terminator that follows it.
    #[error(
        "line {line} holds {delimiter} at byte {position}; a fixed-width line cannot contain a \
         record delimiter"
    )]
    LineDelimiter {
        /// The 1-based position of the line among the lines passed.
        line: usize,
        /// The 1-based position of the delimiter byte within the line.
        position: usize,
        /// The delimiter found.
        delimiter: Delimiter,
    },
    /// A record file did not decode with its layout.
    #[error("record file decode failed: {0}")]
    Codec(#[from] CodecError),
}

impl Generations {
    /// The catalog of the GDGs under `root`. Nothing is created until a generation is written, so
    /// jobs that write none, such as `migrate` and `seed`, leave the file system untouched.
    #[must_use]
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The cataloged generations of `dataset`, oldest first. Only files named exactly
    /// `<DATASET>.G<nnnn>V00` with `nnnn` from `0001` to `9999` count; temporary files and every
    /// other name are ignored. A missing catalog directory holds no generations.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] when the catalog directory or one of its entries cannot be read.
    pub(crate) fn list(&self, dataset: Dataset) -> Result<Vec<Generation>, GenerationError> {
        list_generations(&self.root, dataset)
    }

    /// The `(0)` generation of `dataset`: the newest cataloged, if any.
    ///
    /// # Errors
    ///
    /// As [`Generations::list`].
    pub(crate) fn current(&self, dataset: Dataset) -> Result<Option<Generation>, GenerationError> {
        Ok(self.list(dataset)?.pop())
    }

    /// The `(0)` generation of `dataset`, which a step reads as input.
    ///
    /// # Errors
    ///
    /// [`GenerationError::NotCataloged`] when `dataset` has no generation, and the errors of
    /// [`Generations::list`].
    pub(crate) fn require_current(&self, dataset: Dataset) -> Result<Generation, GenerationError> {
        self.current(dataset)?.ok_or(GenerationError::NotCataloged {
            dataset: dataset.name(),
        })
    }

    /// Allocates the `(+1)` generation of `dataset`: the newest number + 1, or `G0001V00` for an
    /// empty GDG. It creates the catalog directory, takes the allocation lock `.<DATASET>.lock`,
    /// deletes the temporary files that killed jobs left for `dataset` (the module's "Lock files"
    /// section), reads the current generation, and creates the hidden temporary file
    /// `.<DATASET>.G<nnnn>V00.tmp-<pid>-<seq>` that the generation is written to until it is
    /// cataloged. The returned [`PendingGeneration`] holds the lock; every error releases it.
    ///
    /// The sweep deletes a regular file named exactly `.<DATASET>.G<nnnn>V00.tmp-<pid>-<seq>`
    /// whose `<pid>` is this process, which holds no other pending generation of `dataset` while
    /// it holds the lock, or a process that `/proc` shows is not running. A file whose process is
    /// running, or whose state cannot be told, is kept. Each deletion is logged at `info`; a sweep
    /// failure is logged at `warn` and does not fail the allocation.
    ///
    /// # Errors
    ///
    /// - [`GenerationError::InUse`] when the allocation lock of `dataset` exists. Nothing is
    ///   created or changed.
    /// - [`GenerationError::Exhausted`] when `G9999V00` is the newest generation.
    /// - [`GenerationError::Io`] when the directory cannot be read or created, or the lock file or
    ///   the temporary file cannot be created.
    pub(crate) fn begin_next(
        &self,
        dataset: Dataset,
    ) -> Result<PendingGeneration, GenerationError> {
        fs::create_dir_all(&self.root).map_err(|source| io_error(&self.root, source))?;
        let lock = allocation_lock(&self.root, dataset)?;
        sweep_orphaned_temps(&self.root, dataset);
        let number = match self.current(dataset)? {
            None => FIRST_NUMBER,
            Some(newest) => newest
                .number
                .checked_add(1)
                .filter(|number| *number <= MAX_NUMBER)
                .ok_or(GenerationError::Exhausted {
                    dataset: dataset.name(),
                })?,
        };
        let name = generation_name(dataset, number);
        let final_path = self.root.join(&name);
        let (temp_path, file) = create_temp(&final_path, OsStr::new(&name))?;
        Ok(PendingGeneration {
            dataset,
            number,
            temp_path,
            final_path,
            writer: Some(BufWriter::new(file)),
            root: self.root.clone(),
            cataloged: false,
            _lock: lock,
        })
    }

    /// [`Generations::begin_next`] on Tokio's blocking pool, inside the caller's span (see
    /// [`blocking`]).
    ///
    /// # Errors
    ///
    /// The errors of [`Generations::begin_next`], and a [`GenerationError::Io`] on the catalog
    /// directory when the runtime cancels the allocation because it is shutting down; nothing
    /// is then allocated.
    pub(crate) async fn begin_next_async(
        &self,
        dataset: Dataset,
    ) -> Result<PendingGeneration, GenerationError> {
        let generations = self.clone();
        blocking(move || generations.begin_next(dataset))
            .await
            .unwrap_or_else(|cancelled| Err(io_error(&self.root, io::Error::other(cancelled))))
    }

    /// Creates a sort work file in the catalog directory, the module's "Sort work files"
    /// section: the hidden temporary file `.SORTWK.tmp-<pid>-<seq>`, created exclusively and
    /// opened for reading and writing, whose name is unlinked at once. A name that cannot be
    /// unlinked is logged at `warn` and deleted when the work file is dropped.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] on the work file when it cannot be created, as for a temporary
    /// generation file.
    pub(crate) fn create_work_file(&self) -> Result<WorkFileWriter, GenerationError> {
        let (path, file) = create_temp(&self.root.join(SORT_WORK), OsStr::new(SORT_WORK))?;
        let linked = match fs::remove_file(&path) {
            Ok(()) => false,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "sort work file not unlinked; it is deleted when its step ends"
                );
                true
            }
        };
        Ok(WorkFileWriter {
            writer: BufWriter::new(file),
            offset: 0,
            name: WorkName { path, linked },
        })
    }
}

/// A held lock file, which stands for the dataset enqueue JES takes for a job as the module's
/// "Lock files" section describes. Dropping the value deletes the file and so releases the lock.
#[derive(Debug)]
pub(crate) struct LockFile {
    /// The lock file.
    path: PathBuf,
    /// What the lock stands for, for example `generation allocation lock`, as
    /// [`GenerationError::Locked`] and the release warning name it.
    name: &'static str,
}

impl LockFile {
    /// Takes the lock `name` by creating the file `path` exclusively and writing the process id
    /// to it, so an operator can tell which process holds it. The directory of `path` must
    /// exist.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Locked`] when `path` exists: the lock is neither waited for nor taken
    /// over, and the file is left as it is. [`GenerationError::Io`] when the file cannot be
    /// created or written; a file created but not written is deleted again.
    pub(crate) fn acquire(path: PathBuf, name: &'static str) -> Result<Self, GenerationError> {
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                return Err(GenerationError::Locked { name, path });
            }
            Err(source) => return Err(io_error(&path, source)),
        };
        let lock = Self { path, name };
        writeln!(file, "{}", process::id()).map_err(|source| io_error(&lock.path, source))?;
        Ok(lock)
    }
}

impl Drop for LockFile {
    /// Releases the lock. Best effort, because a destructor has no caller to report to: a lock
    /// file that cannot be deleted is logged, and the next job that takes the lock reports it as
    /// held.
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "{} not released",
                self.name
            );
        }
    }
}

/// Takes the allocation lock of one GDG, `<root>/.<DATASET>.lock`: the dataset enqueue JES takes
/// for a job that allocates the GDG `DISP=(NEW,CATLG,DELETE)` [app/jcl/POSTTRAN.jcl:L34-38].
///
/// # Errors
///
/// [`GenerationError::InUse`] when the lock file exists, with the holder the file records, and
/// [`GenerationError::Io`] when it cannot be created or written, as [`LockFile::acquire`]
/// reports them.
fn allocation_lock(root: &Path, dataset: Dataset) -> Result<LockFile, GenerationError> {
    let path = root.join(format!(".{}.lock", dataset.name()));
    LockFile::acquire(path, "generation allocation lock").map_err(|err| match err {
        GenerationError::Locked { path, .. } => GenerationError::InUse {
            dataset: dataset.name(),
            holder: LockHolder::read(&path),
            path,
        },
        other => other,
    })
}

/// The holder of a lock file a refused allocation found: the process id the file records and
/// whether that process is running, read once, when the allocation is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockHolder {
    /// The lock file records process `pid`.
    Process {
        /// The process id the lock file holds.
        pid: u32,
        /// Whether that process is running, as [`process_running`] tells it: `None` when that
        /// cannot be told.
        running: Option<bool>,
    },
    /// The lock file cannot be read or holds no process id, for example while its holder is
    /// still writing it.
    Unknown,
}

impl LockHolder {
    /// The holder the lock file `path` records: the process id [`LockFile::acquire`] writes,
    /// one decimal number followed by a line break.
    fn read(path: &Path) -> Self {
        let pid = fs::read_to_string(path)
            .ok()
            .and_then(|text| decimal(text.trim_end_matches(['\n', '\r'])))
            .and_then(|pid| u32::try_from(pid).ok());
        match pid {
            Some(pid) => Self::Process {
                pid,
                running: process_running(pid),
            },
            None => Self::Unknown,
        }
    }
}

impl fmt::Display for LockHolder {
    /// For example `process 4242, which is not running`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Process {
                pid,
                running: Some(true),
            } => write!(f, "process {pid}, which is running"),
            Self::Process {
                pid,
                running: Some(false),
            } => write!(f, "process {pid}, which is not running"),
            Self::Process { pid, running: None } => {
                write!(f, "process {pid}, which may or may not be running")
            }
            Self::Unknown => f.write_str("an unknown process"),
        }
    }
}

/// A `(+1)` generation being written: the `DISP=(NEW,CATLG,DELETE)` DD of a running step.
///
/// [`PendingGeneration::catalog`] is the normal end of the step. Dropping the value without
/// cataloging it is the abnormal end: the temporary file is deleted and no generation is added.
/// Either way the allocation lock of the GDG is released last. A job ends the step with
/// [`PendingGeneration::catalog_async`] or [`PendingGeneration::discard_async`], which do the
/// same on Tokio's blocking pool.
#[must_use = "a pending generation is deleted unless it is cataloged"]
#[derive(Debug)]
pub(crate) struct PendingGeneration {
    dataset: Dataset,
    number: u16,
    temp_path: PathBuf,
    final_path: PathBuf,
    /// Taken by `catalog` and `drop`, both of which consume the generation, and lent to the
    /// blocking pool by an asynchronous append, which puts it back unless the runtime cancels the
    /// append. `None` on a generation not consumed means the file is closed and possibly
    /// incomplete: writes fail and `catalog` refuses it.
    writer: Option<BufWriter<File>>,
    root: PathBuf,
    cataloged: bool,
    /// Held, never read: fields drop after `Drop::drop` and after the body of `catalog`, so the
    /// lock is released only once the temporary file is deleted or the generation is cataloged
    /// and rolled off.
    _lock: LockFile,
}

impl PendingGeneration {
    /// Appends `records` encoded with the ASCII layout `L`: each at its full copybook width,
    /// followed by `LF`. `posttran` calls it once per reject, so `DALYREJS(+1)` grows record by
    /// record.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] when the temporary file cannot be written.
    pub(crate) fn append_records<L: Layout>(
        &mut self,
        records: &[L::Record],
    ) -> Result<(), GenerationError> {
        self.write(&encode_ascii_lf::<L>(records))
    }

    /// Appends fixed-width text lines of `width` bytes, each followed by `LF`, as
    /// [`encode_fixed_lines`] encodes them. Nothing is written when a line has the wrong width or
    /// holds `LF` or `CR`.
    ///
    /// Jobs append lines only through [`PendingGeneration::append_lines_async`]; this synchronous
    /// form is the reference the tests hold it to.
    ///
    /// # Errors
    ///
    /// [`GenerationError::LineWidth`] and [`GenerationError::LineDelimiter`] as
    /// [`encode_fixed_lines`] reports them, and [`GenerationError::Io`] when the temporary file
    /// cannot be written.
    #[cfg(test)]
    pub(crate) fn append_lines(
        &mut self,
        lines: &[String],
        width: usize,
    ) -> Result<(), GenerationError> {
        let bytes = encode_fixed_lines(lines, width)?;
        self.write(&bytes)
    }

    /// [`PendingGeneration::append_records`] on Tokio's blocking pool, inside the caller's span
    /// (see [`blocking`]): the records are encoded and written there.
    ///
    /// # Errors
    ///
    /// The errors of [`PendingGeneration::append_records`]. When the runtime cancels the append
    /// because it is shutting down, a [`GenerationError::Io`] on the temporary file: its writer
    /// is lost, so every later write fails and [`PendingGeneration::catalog`] refuses the
    /// generation.
    pub(crate) async fn append_records_async<L>(
        &mut self,
        records: Vec<L::Record>,
    ) -> Result<(), GenerationError>
    where
        L: Layout + 'static,
        L::Record: Send + 'static,
    {
        self.write_async(move || Ok(encode_ascii_lf::<L>(&records)))
            .await
    }

    /// Appends fixed-width text lines of `width` bytes, each followed by `LF`, as
    /// [`encode_fixed_lines`] encodes them, on Tokio's blocking pool, inside the caller's span
    /// (see [`blocking`]): the lines are checked, encoded and written there. Nothing is written
    /// when a line has the wrong width or holds `LF` or `CR`.
    ///
    /// # Errors
    ///
    /// [`GenerationError::LineWidth`] and [`GenerationError::LineDelimiter`] as
    /// [`encode_fixed_lines`] reports them, [`GenerationError::Io`] when the temporary file
    /// cannot be written, and a [`GenerationError::Io`] on the temporary file when the runtime
    /// cancels the append, as [`PendingGeneration::append_records_async`] describes.
    pub(crate) async fn append_lines_async(
        &mut self,
        lines: Vec<String>,
        width: usize,
    ) -> Result<(), GenerationError> {
        self.write_async(move || encode_fixed_lines(&lines, width))
            .await
    }

    /// [`PendingGeneration::catalog`] on Tokio's blocking pool, inside the caller's span (see
    /// [`blocking`]), so its `generation cataloged` and roll-off events land there.
    ///
    /// # Errors
    ///
    /// The errors of [`PendingGeneration::catalog`], and a [`GenerationError::Io`] on
    /// `<DATASET>.G<nnnn>V00` when the runtime cancels the catalog because it is shutting down.
    /// The runtime then drops the generation uncataloged, which deletes its temporary file and
    /// releases the lock.
    pub(crate) async fn catalog_async(self) -> Result<Generation, GenerationError> {
        let final_path = self.final_path.clone();
        blocking(move || self.catalog())
            .await
            .unwrap_or_else(|cancelled| Err(io_error(&final_path, io::Error::other(cancelled))))
    }

    /// The abnormal end of the step on Tokio's blocking pool, inside the caller's span (see
    /// [`blocking`]): drops the generation uncataloged, which deletes its temporary file and
    /// then releases the allocation lock, as dropping it does. When the runtime cancels the
    /// discard because it is shutting down, a warning is logged; the runtime still drops the
    /// generation, with the same effect.
    pub(crate) async fn discard_async(self) {
        let generation = generation_name(self.dataset, self.number);
        if let Err(cancelled) = blocking(move || drop(self)).await {
            tracing::warn!(
                %generation,
                error = %cancelled,
                "uncataloged generation discard cancelled by runtime shutdown"
            );
        }
    }

    /// Catalogs the generation at the normal end of its step: flushes and syncs the temporary
    /// file, hard-links it to `<DATASET>.G<nnnn>V00`, deletes the temporary name, then scratches
    /// the oldest generations until the newest [`LIMIT`] remain, and finally releases the
    /// allocation lock. The link never replaces an existing file. A generation with nothing
    /// appended catalogs as an empty file, as `DALYREJS(+1)` does after a run without rejects.
    ///
    /// Once linked, the generation is cataloged: a temporary name or an old generation that
    /// cannot be deleted afterwards is logged, not returned.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] when the flush, the sync or the link fails, including when
    /// `<DATASET>.G<nnnn>V00` already exists (`AlreadyExists`, reported on that path), and on the
    /// temporary file when that file is already closed because a cancelled asynchronous append
    /// lost its writer, so it may be incomplete. The temporary file is then deleted, an existing
    /// file of that name is left as it was, and no generation is added.
    pub(crate) fn catalog(mut self) -> Result<Generation, GenerationError> {
        let Some(writer) = self.writer.take() else {
            return Err(io_error(&self.temp_path, io::Error::other(CLOSED)));
        };
        let file = writer
            .into_inner()
            .map_err(|e| io_error(&self.temp_path, e.into_error()))?;
        file.sync_all()
            .map_err(|source| io_error(&self.temp_path, source))?;
        drop(file);
        fs::hard_link(&self.temp_path, &self.final_path)
            .map_err(|source| io_error(&self.final_path, source))?;
        self.cataloged = true;
        remove_temp(&self.temp_path);
        let generation = Generation {
            dataset: self.dataset,
            number: self.number,
            path: self.final_path.clone(),
        };
        tracing::info!(
            generation = %generation,
            path = %generation.path.display(),
            "generation cataloged"
        );
        scratch_beyond_limit(&self.root, self.dataset);
        Ok(generation)
    }

    /// Writes `bytes` to the temporary file through its buffer.
    fn write(&mut self, bytes: &[u8]) -> Result<(), GenerationError> {
        write_buffered(self.writer.as_mut(), &self.temp_path, bytes)
    }

    /// Runs `encode`, then writes the bytes it returns to the temporary file, both on Tokio's
    /// blocking pool. The writer is lent to the blocking work and put back whatever its outcome,
    /// so an encoding error writes nothing and a write error is the one
    /// [`PendingGeneration::write`] returns. A cancelled blocking task loses the writer and is
    /// reported as a [`GenerationError::Io`] on the temporary file.
    async fn write_async<E>(&mut self, encode: E) -> Result<(), GenerationError>
    where
        E: FnOnce() -> Result<Vec<u8>, GenerationError> + Send + 'static,
    {
        let mut writer = self.writer.take();
        let temp_path = self.temp_path.clone();
        let lent = blocking(move || {
            let written =
                encode().and_then(|bytes| write_buffered(writer.as_mut(), &temp_path, &bytes));
            (writer, written)
        })
        .await;
        match lent {
            Ok((writer, written)) => {
                self.writer = writer;
                written
            }
            Err(cancelled) => Err(io_error(&self.temp_path, io::Error::other(cancelled))),
        }
    }
}

impl Drop for PendingGeneration {
    /// `DISP=(…,DELETE)` on abnormal end: an uncataloged generation's temporary file is deleted,
    /// and then the allocation lock is released. Best effort, because a destructor has no caller
    /// to report to: a failure is logged.
    fn drop(&mut self) {
        if self.cataloged {
            return;
        }
        drop(self.writer.take());
        if remove_temp(&self.temp_path) {
            tracing::info!(
                generation = %generation_name(self.dataset, self.number),
                "uncataloged generation deleted"
            );
        }
    }
}

/// Reads every record of the ASCII record file `path` with the layout `L`, in file order: a
/// generation such as `TRANSACT.BKUP(0)`, `SYSTRAN(0)` or `TRANSACT.DALY(+1)`.
///
/// The file is read as [`PendingGeneration::append_records`] writes it: consecutive frames of
/// `L::LEN` record bytes, each followed by `LF`. Each record's bytes are decoded whole, whatever
/// they hold, so a record whose alphanumeric field holds `LF` or `CR` reads back unchanged.
///
/// # Errors
///
/// [`GenerationError::Io`] when the file cannot be read, and [`GenerationError::Codec`] for the
/// first record that does not decode, located at its 1-based record number: a final frame
/// shorter than `L::LEN` bytes and its `LF`, a byte other than `LF` after a record, or a field
/// error of the layout.
///
/// Jobs read record files only through [`RecordReader`], which never holds a whole file; this
/// whole-file form, the codec's own decode of the file, is the reference the tests hold it to.
#[cfg(test)]
pub(crate) fn read_records<L: Layout>(path: &Path) -> Result<Vec<L::Record>, GenerationError> {
    let bytes = fs::read(path).map_err(|source| io_error(path, source))?;
    Ok(carddemo_codec::layouts::decode_ascii_lf::<L>(&bytes)?)
}

/// A record file read one record at a time through a buffer: `read_records` streamed, so a
/// job holds one frame of the file instead of the whole file. COMBTRAN reads its SORTIN
/// `TRANSACT.BKUP(0)` and `SYSTRAN(0)` [app/jcl/COMBTRAN.jcl:L23-26] and its `REPRO` input
/// `TRANSACT.COMBINED(+1)` [app/jcl/COMBTRAN.jcl:L43-44] this way, and TRANREPT its SORTIN
/// `TRANSACT.BKUP(+1)` [app/jcl/TRANREPT.jcl:L38-39] and the CBTRN03C `TRANFILE`
/// `TRANSACT.DALY(+1)`.
///
/// Frames are read as `read_records` reads them: `L::LEN` record bytes, decoded whole, then
/// `LF`. The iterator yields the records in file order and then `None`. In place of the first
/// record that does not decode it yields the error `read_records` returns for the same file,
/// and `None` after it:
///
/// - [`GenerationError::Io`] on the path when the file cannot be read;
/// - [`GenerationError::Codec`] located at the record's 1-based number: a final frame shorter
///   than `L::LEN` bytes and its `LF` ([`CodecErrorKind::PartialFrame`], carrying the bytes left
///   in the file), a byte other than `LF` after a record ([`CodecErrorKind::Separator`]), or a
///   field error of the layout.
pub(crate) struct RecordReader<L: Layout> {
    /// The record file, as errors name it.
    path: PathBuf,
    /// The open file, read through its buffer.
    file: BufReader<File>,
    /// One frame: `L::LEN` record bytes and the byte after them.
    frame: Vec<u8>,
    /// The records yielded so far, which numbers the next one.
    records: usize,
    /// Whether the end of the file or an error has been met: every later item is `None`.
    done: bool,
    /// The layout, which only types the records.
    layout: PhantomData<fn() -> L>,
}

impl<L: Layout> RecordReader<L> {
    /// Opens the record file `path` at its first record.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] on `path` when the file cannot be opened, as `read_records`
    /// reports it.
    pub(crate) fn open(path: &Path) -> Result<Self, GenerationError> {
        let file = File::open(path).map_err(|source| io_error(path, source))?;
        Ok(Self {
            path: path.to_path_buf(),
            file: BufReader::new(file),
            frame: vec![0; L::LEN + 1],
            records: 0,
            done: false,
            layout: PhantomData,
        })
    }

    /// The next records of the file in file order, `limit` of them, or fewer at the end of the
    /// file. Record numbers run on from the batch before, so an error names the record as
    /// `read_records` does.
    ///
    /// # Errors
    ///
    /// The error the iterator yields in place of a record; the records read before it in this
    /// batch are dropped.
    pub(crate) fn next_batch(&mut self, limit: usize) -> Result<Vec<L::Record>, GenerationError> {
        self.by_ref().take(limit).collect()
    }

    /// Reads and decodes the next frame: `None` at the end of the file.
    fn read_record(&mut self) -> Result<Option<L::Record>, GenerationError> {
        let filled =
            fill(&mut self.file, &mut self.frame).map_err(|source| io_error(&self.path, source))?;
        if filled == 0 {
            return Ok(None);
        }
        let number = self.records + 1;
        let error = |kind| GenerationError::Codec(CodecError::new(L::NAME, kind).at_record(number));
        if filled < self.frame.len() {
            return Err(error(CodecErrorKind::PartialFrame { frame_len: filled }));
        }
        let (body, separator) = self.frame.split_at(L::LEN);
        if let Some(&byte) = separator.first().filter(|byte| **byte != b'\n') {
            return Err(error(CodecErrorKind::Separator { byte }));
        }
        let record = L::decode(body, Charset::Ascii).map_err(|e| e.at_record(number))?;
        self.records = number;
        Ok(Some(record))
    }
}

impl<L> RecordReader<L>
where
    L: Layout + 'static,
    L::Record: Send + 'static,
{
    /// [`RecordReader::open`] on Tokio's blocking pool, inside the caller's span (see
    /// [`blocking`]).
    ///
    /// # Errors
    ///
    /// The errors of [`RecordReader::open`], and a [`GenerationError::Io`] on `path` when the
    /// runtime cancels the open because it is shutting down.
    pub(crate) async fn open_async(path: PathBuf) -> Result<Self, GenerationError> {
        let error_path = path.clone();
        blocking(move || Self::open(&path))
            .await
            .unwrap_or_else(|cancelled| Err(io_error(&error_path, io::Error::other(cancelled))))
    }

    /// [`RecordReader::next_batch`] on Tokio's blocking pool, inside the caller's span (see
    /// [`blocking`]): the reader is lent to the blocking work and returned with the batch, so
    /// the next batch reads on from where this one ended.
    ///
    /// # Errors
    ///
    /// The errors of [`RecordReader::next_batch`], and a [`GenerationError::Io`] on the file
    /// when the runtime cancels the read because it is shutting down. Either way the reader is
    /// consumed, because the file is read no further.
    pub(crate) async fn next_batch_async(
        mut self,
        limit: usize,
    ) -> Result<(Self, Vec<L::Record>), GenerationError> {
        let error_path = self.path.clone();
        blocking(move || self.next_batch(limit).map(|batch| (self, batch)))
            .await
            .unwrap_or_else(|cancelled| Err(io_error(&error_path, io::Error::other(cancelled))))
    }
}

impl<L: Layout> Iterator for RecordReader<L> {
    type Item = Result<L::Record, GenerationError>;

    /// The next record, or the error that ends the reading; `None` once either has ended it.
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let item = self.read_record().transpose();
        self.done = !matches!(item, Some(Ok(_)));
        item
    }
}

/// The number of records of the record file `path`, each read and decoded as `read_records`
/// reads it but none kept, on Tokio's blocking pool, inside the caller's span (see
/// [`blocking`]): the check that the whole file reads, holding one frame at a time.
///
/// # Errors
///
/// The errors of `read_records` for the same file, and a [`GenerationError::Io`] on `path`
/// when the runtime cancels the read because it is shutting down.
pub(crate) async fn count_records_async<L>(path: PathBuf) -> Result<usize, GenerationError>
where
    L: Layout + 'static,
{
    let error_path = path.clone();
    blocking(move || {
        RecordReader::<L>::open(&path)?.try_fold(0, |records, record| record.map(|_| records + 1))
    })
    .await
    .unwrap_or_else(|cancelled| Err(io_error(&error_path, io::Error::other(cancelled))))
}

/// A sort work file being written: one of the `SORTWK` work datasets DFSORT allocates for a
/// SORT step whose input does not fit its memory and deletes when the step ends, such as
/// COMBTRAN `STEP05R` [app/jcl/COMBTRAN.jcl:L22-37]; see the module's "Sort work files"
/// section. [`Generations::create_work_file`] creates it.
///
/// Bytes are appended through a buffer ([`WorkFileWriter::write`]), and
/// [`WorkFileWriter::finish`] flushes them and returns the [`WorkFile`] they are read back
/// from.
pub(crate) struct WorkFileWriter {
    /// The file, written through its buffer. Declared before `name`, so the file is closed
    /// before a name still linking it is deleted.
    writer: BufWriter<File>,
    /// The bytes appended so far.
    offset: u64,
    /// The name the file was created under.
    name: WorkName,
}

impl WorkFileWriter {
    /// Appends `bytes` through the buffer.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] on the work file when it cannot be written.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<(), GenerationError> {
        self.writer
            .write_all(bytes)
            .map_err(|source| io_error(&self.name.path, source))?;
        self.offset = self.offset.saturating_add(byte_count(bytes.len()));
        Ok(())
    }

    /// The bytes appended so far: the offset in the file at which the next write starts.
    pub(crate) const fn offset(&self) -> u64 {
        self.offset
    }

    /// Ends the writing: flushes the buffer and returns the file to read back.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] on the work file when the buffer cannot be flushed. The file is
    /// then closed and deleted.
    pub(crate) fn finish(self) -> Result<WorkFile, GenerationError> {
        let Self { writer, name, .. } = self;
        match writer.into_inner() {
            Ok(file) => Ok(WorkFile { file, name }),
            Err(error) => Err(io_error(&name.path, error.into_error())),
        }
    }
}

/// A sort work file once written ([`WorkFileWriter::finish`]): read back through
/// [`WorkFile::reader`], from any offset and as many times as the step needs, and deleted when
/// dropped.
pub(crate) struct WorkFile {
    /// The file, shared by every reader of it. Declared before `name`, so the file is closed
    /// before a name still linking it is deleted.
    file: File,
    /// The name the file was created under.
    name: WorkName,
}

impl WorkFile {
    /// A buffered reader of the `len` bytes written from offset `start`: the file rewound to a
    /// part written earlier. Readers of one file read side by side, each from its own position,
    /// because each read seeks to that position first.
    pub(crate) fn reader(&self, start: u64, len: u64) -> WorkReader<'_> {
        WorkReader {
            path: &self.name.path,
            reader: BufReader::new(WorkSegment {
                file: &self.file,
                position: start,
                end: start.saturating_add(len),
            }),
        }
    }
}

/// A buffered reader of one part of a [`WorkFile`].
pub(crate) struct WorkReader<'a> {
    /// The work file, as errors name it.
    path: &'a Path,
    /// The part, read through its buffer.
    reader: BufReader<WorkSegment<'a>>,
}

impl WorkReader<'_> {
    /// Reads the next `frame.len()` bytes of the part into `frame`: `true` when they were read
    /// whole, `false` at the end of the part.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] on the work file when it cannot be read, or when the part ends
    /// inside the frame (`UnexpectedEof`).
    pub(crate) fn read_frame(&mut self, frame: &mut [u8]) -> Result<bool, GenerationError> {
        match fill(&mut self.reader, frame) {
            Ok(read) if read == frame.len() => Ok(true),
            Ok(0) => Ok(false),
            Ok(read) => Err(io_error(
                self.path,
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "the sort work file ends {read} bytes into a {}-byte frame",
                        frame.len()
                    ),
                ),
            )),
            Err(source) => Err(io_error(self.path, source)),
        }
    }

    /// The [`GenerationError::Io`] on the work file for a frame read whole whose bytes are not
    /// what the step wrote: `error` says why (`InvalidData`).
    pub(crate) fn invalid_data(
        &self,
        error: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> GenerationError {
        io_error(self.path, io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// The bytes of a [`WorkFile`] from `position` to `end`, read through the handle every reader of
/// the file shares: each read seeks to this part's own position first.
struct WorkSegment<'a> {
    /// The shared handle.
    file: &'a File,
    /// The offset of the next byte to read.
    position: u64,
    /// The offset just past the part.
    end: u64,
}

impl Read for WorkSegment<'_> {
    /// Reads at most the bytes left in the part: 0 once it is read whole.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_sub(self.position);
        let wanted = usize::try_from(left).map_or(buffer.len(), |left| left.min(buffer.len()));
        if wanted == 0 {
            return Ok(0);
        }
        let mut file = self.file;
        file.seek(SeekFrom::Start(self.position))?;
        let read = file.read(&mut buffer[..wanted])?;
        self.position = self.position.saturating_add(byte_count(read));
        Ok(read)
    }
}

/// The name a sort work file was created under, and whether that name still links the file.
struct WorkName {
    /// The hidden temporary path, `<root>/.SORTWK.tmp-<pid>-<seq>`.
    path: PathBuf,
    /// Whether the unlink right after creation failed, so the name still links the file.
    linked: bool,
}

impl Drop for WorkName {
    /// Deletes the name when it still links the file, once the file is closed. Best effort,
    /// because a destructor has no caller to report to: a failure other than an absent file is
    /// logged.
    fn drop(&mut self) {
        if !self.linked {
            return;
        }
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %self.path.display(),
                %error,
                "sort work file not deleted"
            ),
        }
    }
}

/// Reads from `reader` until `buffer` is full or the input ends, retrying a read the operating
/// system interrupted, as `read_exact` does. Returns the bytes read: fewer than `buffer` holds
/// only at the end of the input.
fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while let Some(rest) = buffer.get_mut(filled..).filter(|rest| !rest.is_empty()) {
        match reader.read(rest) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// `len` bytes as a file offset. No length reaches `u64::MAX`; the conversion saturates there
/// rather than fail.
fn byte_count(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// Runs `work` on Tokio's blocking pool and returns its value: the boundary every synchronous
/// file-system, codec and CPU phase of a batch job crosses, so that the job future itself only
/// awaits.
///
/// `work` runs inside the caller's current span and under the caller's default dispatcher, both
/// captured when this future is first polled, so every event it emits and every
/// `tracing::Span::current().record(..)` it makes lands exactly where it would, had the caller
/// run `work` inline: in the job span, on the job's subscriber.
///
/// # Errors
///
/// The [`tokio::task::JoinError`] of a blocking task the runtime cancelled because it is shutting
/// down before `work` started; `work` is then dropped without running. That is the only error.
///
/// # Panics
///
/// Resumes, on the caller, the panic `work` raised, with its original payload, as the unwinding
/// of an inline call would. Panics, as `tokio::task::spawn_blocking` does, when called outside a
/// Tokio runtime.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, tokio::task::JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    let dispatch = tracing::dispatcher::get_default(Dispatch::clone);
    tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || span.in_scope(work))
    })
    .await
    .or_else(|error| match error.try_into_panic() {
        Ok(payload) => panic::resume_unwind(payload),
        Err(cancelled) => Err(cancelled),
    })
}

/// Encodes fixed-width text lines as a record file: each line, which must be exactly `width`
/// bytes and hold no `LF` or `CR`, followed by `LF`. Lines are never padded, truncated, trimmed,
/// escaped or otherwise altered, so a width or content error in the code that formats them
/// surfaces here instead of shifting, splitting or merging the records of the file. Each line is
/// checked in order, width first; the first error is returned and nothing is encoded.
///
/// # Errors
///
/// - [`GenerationError::LineWidth`] for a line whose byte length is not `width`.
/// - [`GenerationError::LineDelimiter`] for a line of the right width that holds `LF` or `CR`,
///   located at the first such byte.
pub(crate) fn encode_fixed_lines(
    lines: &[String],
    width: usize,
) -> Result<Vec<u8>, GenerationError> {
    let mut out = Vec::with_capacity(lines.len().saturating_mul(width.saturating_add(1)));
    for (text, line) in lines.iter().zip(1..) {
        if text.len() != width {
            return Err(GenerationError::LineWidth {
                line,
                expected: width,
                actual: text.len(),
            });
        }
        if let Some((delimiter, position)) = text
            .bytes()
            .zip(1..)
            .find_map(|(byte, position)| Delimiter::of(byte).map(|delimiter| (delimiter, position)))
        {
            return Err(GenerationError::LineDelimiter {
                line,
                position,
                delimiter,
            });
        }
        out.extend_from_slice(text.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

/// Replaces the file `path` with `bytes` as a whole: writes and syncs a hidden temporary sibling
/// created exclusively under a name of its own, then renames it over `path`, so a reader never
/// sees a partly written file and concurrent writers of the same path never share a temporary
/// file; the last rename wins. The parent directory is created first. `creastmt` writes
/// `STATEMNT.PS`, `STATEMNT.HTML` and `STATEMNT.PS.PDF` this way, because they are cataloged
/// datasets, not GDGs: each run replaces them.
///
/// # Errors
///
/// [`GenerationError::Io`] when `path` names no file, or when the directory or the temporary
/// file cannot be created, or the file cannot be written, synced or renamed. The temporary file
/// is then deleted and `path` keeps its previous content.
pub(crate) fn write_replacing(path: &Path, bytes: &[u8]) -> Result<(), GenerationError> {
    let Some(file_name) = path.file_name() else {
        return Err(io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"),
        ));
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    }
    let (temp_path, file) = create_temp(path, file_name)?;
    let replaced = write_synced(file, &temp_path, bytes)
        .and_then(|()| fs::rename(&temp_path, path).map_err(|source| io_error(path, source)));
    if replaced.is_err() {
        remove_temp(&temp_path);
    }
    replaced
}

/// Writes `bytes` through `writer`, the buffer of the temporary file `temp_path`. A missing
/// writer means the file is already closed, and the write fails on `temp_path`.
fn write_buffered(
    writer: Option<&mut BufWriter<File>>,
    temp_path: &Path,
    bytes: &[u8],
) -> Result<(), GenerationError> {
    let written = match writer {
        Some(writer) => writer.write_all(bytes),
        None => Err(io::Error::other(CLOSED)),
    };
    written.map_err(|source| io_error(temp_path, source))
}

/// Writes `bytes` to `file`, the newly created file at `path`, syncs it to disk and closes it.
fn write_synced(mut file: File, path: &Path, bytes: &[u8]) -> Result<(), GenerationError> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| io_error(path, source))
}

/// Creates the hidden temporary sibling of `path`, whose file name is `file_name`, exclusively:
/// a name that already exists is never opened, truncated or deleted, and the next sequence
/// number is tried instead. The file is opened for reading as well as writing, so a sort work
/// file is read back through the handle it was written through.
///
/// # Errors
///
/// [`GenerationError::Io`] when the file cannot be created, or when [`TEMP_ATTEMPTS`] names in a
/// row are taken (`AlreadyExists`, reported on the last name tried).
fn create_temp(path: &Path, file_name: &OsStr) -> Result<(PathBuf, File), GenerationError> {
    let mut attempt = 1;
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = path.with_file_name(temp_name(file_name, sequence));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                if attempt == TEMP_ATTEMPTS {
                    return Err(io_error(&temp_path, source));
                }
                attempt += 1;
            }
            Err(source) => return Err(io_error(&temp_path, source)),
        }
    }
}

/// The generations of `dataset` under `root`, oldest first; see [`Generations::list`].
fn list_generations(root: &Path, dataset: Dataset) -> Result<Vec<Generation>, GenerationError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(root, source)),
    };
    let mut generations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| io_error(root, source))?;
        let Some(number) = entry
            .file_name()
            .to_str()
            .and_then(|name| parse_number(dataset, name))
        else {
            continue;
        };
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| io_error(&path, source))?;
        if file_type.is_file() {
            generations.push(Generation {
                dataset,
                number,
                path,
            });
        }
    }
    generations.sort_unstable_by_key(|generation| generation.number);
    Ok(generations)
}

/// `SCRATCH`: deletes the oldest generations of `dataset` until the newest [`LIMIT`] remain.
///
/// Runs after the new generation is cataloged, so a failure here is logged and not returned: the
/// step's output is in place and its database work committed, and reporting the step as failed
/// would invite a rerun that repeats that work. The next catalog retries the roll-off.
fn scratch_beyond_limit(root: &Path, dataset: Dataset) {
    let generations = match list_generations(root, dataset) {
        Ok(generations) => generations,
        Err(error) => {
            tracing::warn!(dataset = dataset.name(), %error, "generation roll-off skipped");
            return;
        }
    };
    let excess = generations.len().saturating_sub(LIMIT);
    for oldest in generations.iter().take(excess) {
        match fs::remove_file(&oldest.path) {
            Ok(()) => tracing::info!(generation = %oldest, "generation scratched"),
            Err(error) => tracing::warn!(
                generation = %oldest,
                path = %oldest.path.display(),
                %error,
                "generation scratch failed"
            ),
        }
    }
}

/// Deletes the temporary files of `dataset` under `root` that killed jobs left behind: the sweep
/// [`Generations::begin_next`] makes while it holds the allocation lock of `dataset`.
///
/// A regular file named exactly as [`temp_name`] names a generation of `dataset` is deleted when
/// the process id in its name ([`temp_name_pid`]) is this process, which holds no other pending
/// generation of `dataset` while it holds the lock, or a process [`process_running`] shows is not
/// running. Every other file is kept, a file whose process is running or may be running included.
///
/// The allocation goes on whatever the sweep meets: a directory, an entry or a file that cannot
/// be read or deleted is logged at `warn` and skipped, and each deletion is logged at `info`. A
/// file already gone when it is deleted needs nothing.
fn sweep_orphaned_temps(root: &Path, dataset: Dataset) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(
                dataset = dataset.name(),
                path = %root.display(),
                %error,
                "orphaned temporary generation file sweep skipped"
            );
            return;
        }
    };
    let own_pid = process::id();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(
                    dataset = dataset.name(),
                    path = %root.display(),
                    %error,
                    "orphaned temporary generation file sweep stopped"
                );
                return;
            }
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| temp_name_pid(dataset, name))
        else {
            continue;
        };
        let path = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() => {}
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "orphaned temporary generation file not checked"
                );
                continue;
            }
        }
        if pid != own_pid && process_running(pid) != Some(false) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => tracing::info!(
                dataset = dataset.name(),
                path = %path.display(),
                pid,
                "orphaned temporary generation file deleted"
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                dataset = dataset.name(),
                path = %path.display(),
                pid,
                %error,
                "orphaned temporary generation file not deleted"
            ),
        }
    }
}

/// The absolute generation number of `file_name` when it is exactly
/// `<DATASET>.G<nnnn>V00` for `dataset`, with `nnnn` from `0001` to `9999`.
fn parse_number(dataset: Dataset, file_name: &str) -> Option<u16> {
    let digits = file_name
        .strip_prefix(dataset.name())?
        .strip_prefix(".G")?
        .strip_suffix("V00")?;
    if digits.len() != 4 {
        return None;
    }
    let number = digits.bytes().try_fold(0_u16, |number, byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
        number.checked_mul(10)?.checked_add(u16::from(digit))
    })?;
    (number >= FIRST_NUMBER).then_some(number)
}

/// The generation file name `<DATASET>.G<nnnn>V00`.
fn generation_name(dataset: Dataset, number: u16) -> String {
    format!("{}.G{number:04}V00", dataset.name())
}

/// The hidden temporary name `.<file name>.tmp-<pid>-<seq>` a file is written under before it is
/// linked or renamed into place. The process id separates concurrent jobs and the sequence
/// number separates the writes of one process. The leading dot keeps it out of
/// [`Generations::list`].
fn temp_name(file_name: &OsStr, sequence: u64) -> OsString {
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".tmp-{}-{sequence}", process::id()));
    name
}

/// The process id in `file_name` when it is exactly the temporary name [`temp_name`] gives a
/// generation of `dataset`: `.<DATASET>.G<nnnn>V00.tmp-<pid>-<seq>`, with `nnnn` as
/// [`parse_number`] takes it and `<pid>` and `<seq>` as [`decimal`] takes them. `None` for
/// every other name, the temporary names of other datasets and of statement files included.
fn temp_name_pid(dataset: Dataset, file_name: &str) -> Option<u32> {
    let (generation, suffix) = file_name.strip_prefix('.')?.split_once(".tmp-")?;
    parse_number(dataset, generation)?;
    let (pid, sequence) = suffix.split_once('-')?;
    decimal(sequence)?;
    u32::try_from(decimal(pid)?).ok()
}

/// The value of `text` when it is a decimal number as this module writes one, a process id or a
/// sequence number: ASCII digits only, without a sign or a leading zero (`0` itself excepted),
/// within `u64`.
fn decimal(text: &str) -> Option<u64> {
    let canonical =
        text.bytes().all(|byte| byte.is_ascii_digit()) && (text == "0" || !text.starts_with('0'));
    if canonical { text.parse().ok() } else { None }
}

/// Whether process `pid` is running, as Linux's process file system tells it: `/proc/<pid>`
/// exists while the process does, until its parent reaps it, and for a thread id as well.
/// `None` when that cannot be told: `/proc` is not mounted (it shows no `/proc/self`), as on
/// systems without it, or the lookup fails other than with "not found".
fn process_running(pid: u32) -> Option<bool> {
    let proc_root = Path::new("/proc");
    if !matches!(proc_root.join("self").try_exists(), Ok(true)) {
        return None;
    }
    match fs::symlink_metadata(proc_root.join(pid.to_string())) {
        Ok(_) => Some(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// The id of a process that has exited and been reaped, as a job killed while it held a lock or
/// wrote a temporary file leaves its id behind: a child of the test binary run to completion.
///
/// # Panics
///
/// When the child cannot be run, or when [`process_running`] does not show it gone, so a test
/// that needs a process that is not running fails where that cannot be told.
#[cfg(test)]
pub(crate) fn exited_pid() -> u32 {
    let mut child = process::Command::new(std::env::current_exe().expect("the test binary path"))
        .arg("--list")
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()
        .expect("a child process starts");
    let pid = child.id();
    child.wait().expect("the child process exits");
    assert_eq!(process_running(pid), Some(false), "process {pid} is gone");
    pid
}

/// Deletes the temporary file `path`, reporting whether it was there. Best effort: a failure
/// other than an absent file is logged, because the callers are already failing or dropping, or,
/// in [`PendingGeneration::catalog`], have already cataloged the generation.
fn remove_temp(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "temporary generation file not deleted"
            );
            false
        }
    }
}

/// A [`GenerationError::Io`] on `path`.
fn io_error(path: &Path, source: io::Error) -> GenerationError {
    GenerationError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::env;
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use carddemo_codec::{
        Charset, CodecError, CodecErrorKind, FieldKind, FieldSpec, Layout, TransactionLayout,
    };
    use carddemo_runtime::telemetry::subscriber_with_writer;
    use tracing::field::Empty;
    use tracing::{Instrument, Span};

    use super::{
        Dataset, Delimiter, Generation, GenerationError, Generations, LIMIT, LockHolder,
        RecordReader, blocking, count_records_async, create_temp, encode_fixed_lines, exited_pid,
        process_running, read_records, remove_temp, write_replacing, write_synced,
    };

    /// A test directory under the system temporary directory, removed when the test ends.
    struct TestDir(PathBuf);

    impl TestDir {
        /// A directory left by an earlier run with the same process id is stale and is removed
        /// first. A failure to remove it, other than its absence, fails the test before it starts.
        fn new(test: &str) -> Self {
            let path = env::temp_dir().join(format!("carddemo-gdg-{}-{test}", process::id()));
            Self::remove(&path).unwrap_or_else(|e| {
                panic!("test directory {} is not removable: {e}", path.display())
            });
            Self(path)
        }

        /// Removes `path` and everything under it. A directory that is not there, as when the
        /// test never created it, is already removed, so `NotFound` counts as success.
        fn remove(path: &Path) -> io::Result<()> {
            match fs::remove_dir_all(path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                removed => removed,
            }
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// The file names in the directory, sorted.
        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .expect("test directory is readable")
                .map(|entry| {
                    entry
                        .expect("directory entry is readable")
                        .file_name()
                        .into_string()
                        .expect("file name is UTF-8")
                })
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TestDir {
        /// Removes the directory. A failure other than its absence names the directory and the
        /// error: it fails the test, or, while the test is already panicking, goes to stderr,
        /// because a second panic during unwinding would abort the test binary.
        fn drop(&mut self) {
            if let Err(e) = Self::remove(&self.0) {
                let message = format!("test directory {} is not removable: {e}", self.0.display());
                if thread::panicking() {
                    eprintln!("{message}");
                } else {
                    panic!("{message}");
                }
            }
        }
    }

    /// A regular file where the directory belongs cannot be removed as a directory, so each
    /// removal path of [`TestDir`] meets an error other than `NotFound` and names the path:
    /// `new` and `drop` panic, and a drop while a panic is already unwinding reports to stderr
    /// and lets the original panic through instead of aborting the test binary.
    #[test]
    fn test_dir_reports_removal_errors_other_than_absence() {
        let blocked = TestDir::new("cleanup-errors").path().to_path_buf();
        assert!(
            !blocked.exists(),
            "the dropped helper found no directory to remove"
        );

        fs::write(&blocked, b"").expect("the blocking file is writable");
        let created = std::panic::catch_unwind(|| TestDir::new("cleanup-errors"));
        let dropped = std::panic::catch_unwind(|| drop(TestDir(blocked.clone())));
        let unwound: thread::Result<()> = std::panic::catch_unwind(|| {
            let _dir = TestDir(blocked.clone());
            panic!("the test body failed");
        });
        let cleaned = fs::remove_file(&blocked);

        let Err(created) = created else {
            panic!("new removes a stale regular file")
        };
        let Err(dropped) = dropped else {
            panic!("drop removes a regular file")
        };
        let Err(unwound) = unwound else {
            panic!("the test body does not panic")
        };
        cleaned.expect("the blocking file is removable");
        let removal = format!("test directory {} is not removable: ", blocked.display());
        for payload in [&created, &dropped] {
            let message = payload
                .downcast_ref::<String>()
                .expect("a formatted panic message");
            assert!(message.starts_with(&removal), "{message}");
            assert!(
                message.len() > removal.len(),
                "the error follows: {message}"
            );
        }
        assert_eq!(
            unwound.downcast_ref::<&str>(),
            Some(&"the test body failed"),
            "the original panic unwinds without a second one"
        );
    }

    /// A six-byte `X(6)` test record, so the record helpers are exercised without a copybook.
    struct SixBytes;

    impl Layout for SixBytes {
        const LEN: usize = 6;
        const NAME: &'static str = "TEST-REC";
        const FIELDS: &'static [FieldSpec] = &[FieldSpec::new("TEST-DATA", 0, 6, FieldKind::Alnum)];
        type Record = [u8; 6];

        fn decode(bytes: &[u8], _charset: Charset) -> Result<[u8; 6], CodecError> {
            bytes.try_into().map_err(|_| {
                CodecError::new(
                    Self::NAME,
                    CodecErrorKind::Length {
                        expected: Self::LEN,
                        actual: bytes.len(),
                    },
                )
            })
        }

        fn encode(record: &[u8; 6], _charset: Charset) -> Vec<u8> {
            record.to_vec()
        }
    }

    fn catalog_empty(generations: &Generations, dataset: Dataset) -> Generation {
        generations
            .begin_next(dataset)
            .expect("generation allocates")
            .catalog()
            .expect("generation catalogs")
    }

    fn numbers(generations: &Generations, dataset: Dataset) -> Vec<u16> {
        generations
            .list(dataset)
            .expect("catalog lists")
            .iter()
            .map(|generation| generation.number)
            .collect()
    }

    #[test]
    fn first_catalogs_are_g0001_then_g0002() {
        let dir = TestDir::new("first-catalogs");
        let generations = Generations::new(dir.path());

        let first = catalog_empty(&generations, Dataset::DalyRejs);
        assert_eq!(
            first,
            Generation {
                dataset: Dataset::DalyRejs,
                number: 1,
                path: dir.path().join("DALYREJS.G0001V00"),
            }
        );
        assert_eq!(fs::read(&first.path).expect("generation is readable"), b"");

        let second = catalog_empty(&generations, Dataset::DalyRejs);
        assert_eq!(second.to_string(), "DALYREJS.G0002V00");
        assert_eq!(second.path, dir.path().join("DALYREJS.G0002V00"));
        assert_eq!(
            generations
                .current(Dataset::DalyRejs)
                .expect("catalog lists"),
            Some(second)
        );
        assert_eq!(dir.names(), ["DALYREJS.G0001V00", "DALYREJS.G0002V00"]);
    }

    #[test]
    fn catalog_scratches_all_but_the_newest_five() {
        let dir = TestDir::new("limit");
        let generations = Generations::new(dir.path());
        for _ in 0..7 {
            catalog_empty(&generations, Dataset::TransactBkup);
        }
        assert_eq!(LIMIT, 5);
        assert_eq!(
            numbers(&generations, Dataset::TransactBkup),
            [3, 4, 5, 6, 7]
        );
        assert_eq!(
            dir.names(),
            [
                "TRANSACT.BKUP.G0003V00",
                "TRANSACT.BKUP.G0004V00",
                "TRANSACT.BKUP.G0005V00",
                "TRANSACT.BKUP.G0006V00",
                "TRANSACT.BKUP.G0007V00",
            ]
        );
    }

    #[test]
    fn dropped_pending_generation_leaves_no_file() {
        let dir = TestDir::new("dropped");
        let generations = Generations::new(dir.path());
        let first = catalog_empty(&generations, Dataset::DalyRejs);

        let mut pending = generations
            .begin_next(Dataset::DalyRejs)
            .expect("generation allocates");
        pending
            .append_records::<SixBytes>(&[*b"REJECT"])
            .expect("record appends");
        drop(pending);

        assert_eq!(dir.names(), ["DALYREJS.G0001V00"], "the lock is released");
        assert_eq!(
            generations
                .current(Dataset::DalyRejs)
                .expect("catalog lists"),
            Some(first)
        );
        assert_eq!(
            catalog_empty(&generations, Dataset::DalyRejs).to_string(),
            "DALYREJS.G0002V00",
            "the dropped number is allocated again"
        );
    }

    #[test]
    fn pending_generation_holds_the_allocation_lock_until_cataloged() {
        let dir = TestDir::new("in-use");
        let generations = Generations::new(dir.path());
        let lock = dir.path().join(".DALYREJS.lock");

        let mut first = generations
            .begin_next(Dataset::DalyRejs)
            .expect("generation allocates");
        first
            .append_records::<SixBytes>(&[*b"FIRST1"])
            .expect("record appends");
        assert_eq!(
            fs::read_to_string(&lock).expect("lock is readable"),
            format!("{}\n", process::id())
        );
        let names = dir.names();
        assert_eq!(names.len(), 2, "{names:?}");
        let sequence = names[0]
            .strip_prefix(&format!(".DALYREJS.G0001V00.tmp-{}-", process::id()))
            .expect("temporary name holds the process id");
        assert!(sequence.parse::<u64>().is_ok(), "{names:?}");
        assert_eq!(names[1], ".DALYREJS.lock");

        let error = generations
            .begin_next(Dataset::DalyRejs)
            .expect_err("the allocation is held");
        let holder = LockHolder::Process {
            pid: process::id(),
            running: Some(true),
        };
        assert!(
            matches!(
                &error,
                GenerationError::InUse { dataset: "DALYREJS", path, holder: found }
                    if *path == lock && *found == holder
            ),
            "{error:?}"
        );
        assert_eq!(
            dir.names(),
            names,
            "a refused allocation creates nothing and sweeps nothing"
        );

        let first = first.catalog().expect("generation catalogs");
        assert_eq!(first.to_string(), "DALYREJS.G0001V00");
        assert_eq!(
            fs::read(&first.path).expect("generation is readable"),
            b"FIRST1\n"
        );
        assert_eq!(
            dir.names(),
            ["DALYREJS.G0001V00"],
            "no lock or temporary file is left"
        );

        assert_eq!(
            catalog_empty(&generations, Dataset::DalyRejs).to_string(),
            "DALYREJS.G0002V00"
        );
        assert_eq!(dir.names(), ["DALYREJS.G0001V00", "DALYREJS.G0002V00"]);
    }

    /// The id of a process that is running: this test process's parent.
    fn running_pid() -> u32 {
        let pid = std::os::unix::process::parent_id();
        assert_eq!(process_running(pid), Some(true), "process {pid} runs");
        pid
    }

    /// A lock file left behind refuses the allocation whatever it holds, is never taken over,
    /// and leaves the directory unchanged, an orphaned temporary file included: the sweep runs
    /// only once the lock is held. The text names the lock file and the holder it records.
    #[test]
    fn stale_allocation_lock_refuses_allocation() {
        let exited = exited_pid();
        let running = running_pid();
        let cases = [
            (
                format!("{exited}\n"),
                LockHolder::Process {
                    pid: exited,
                    running: Some(false),
                },
                format!("process {exited}, which is not running"),
            ),
            (
                format!("{running}\n"),
                LockHolder::Process {
                    pid: running,
                    running: Some(true),
                },
                format!("process {running}, which is running"),
            ),
            (
                "not a process id\n".to_owned(),
                LockHolder::Unknown,
                "an unknown process".to_owned(),
            ),
            (
                String::new(),
                LockHolder::Unknown,
                "an unknown process".to_owned(),
            ),
        ];
        for (content, holder, holder_text) in cases {
            let dir = TestDir::new("stale-lock");
            fs::create_dir_all(dir.path()).expect("directory creates");
            let lock = dir.path().join(".DALYREJS.lock");
            fs::write(&lock, &content).expect("file writes");
            let orphan = format!(".DALYREJS.G0001V00.tmp-{exited}-0");
            fs::write(dir.path().join(&orphan), b"REJECT\n").expect("file writes");
            let generations = Generations::new(dir.path());

            let error = generations
                .begin_next(Dataset::DalyRejs)
                .expect_err("the lock exists");
            assert!(
                matches!(
                    &error,
                    GenerationError::InUse { dataset: "DALYREJS", path, holder: found }
                        if *path == lock && *found == holder
                ),
                "{content:?}: {error:?}"
            );
            assert_eq!(
                error.to_string(),
                format!(
                    "DALYREJS(+1): the allocation lock {} is held by {holder_text}; remove that \
                     file only when no batch job is running, because a job killed before \
                     releasing the lock leaves it behind",
                    lock.display()
                ),
                "{content:?}"
            );
            assert_eq!(
                dir.names(),
                [orphan.as_str(), ".DALYREJS.lock"],
                "{content:?}: nothing is created or swept"
            );
            assert_eq!(
                fs::read_to_string(&lock).expect("lock is readable"),
                content,
                "the lock is never taken over"
            );
        }
    }

    /// The allocation deletes only the regular files named as a temporary generation file of its
    /// own dataset whose process is this one or is not running, before it creates its own.
    #[test]
    fn allocation_sweeps_only_orphaned_temporary_files_of_its_dataset() {
        let own = process::id();
        let exited = exited_pid();
        let running = running_pid();
        // (name, created as a directory, kept by the sweep)
        let cases = [
            (format!(".DALYREJS.G0001V00.tmp-{own}-7"), false, false),
            (format!(".DALYREJS.G0002V00.tmp-{exited}-0"), false, false),
            (format!(".DALYREJS.G0003V00.tmp-{running}-0"), false, true),
            (format!(".DALYREJS.G0003V00.tmp-{exited}-1"), true, true),
            (format!(".SYSTRAN.G0001V00.tmp-{exited}-0"), false, true),
            (
                format!(".TRANSACT.BKUP.G0001V00.tmp-{exited}-0"),
                false,
                true,
            ),
            (format!(".STATEMNT.PS.tmp-{exited}-0"), false, true),
            (format!(".DALYREJS.G0001V00.tmp-{exited}"), false, true),
            (format!(".DALYREJS.G0001V00.tmp-{exited}-"), false, true),
            (format!(".DALYREJS.G0001V00.tmp--{exited}-0"), false, true),
            (format!(".DALYREJS.G0001V00.tmp-0{exited}-0"), false, true),
            (format!(".DALYREJS.G0001V00.tmp-{exited}-0x"), false, true),
            (
                format!(".DALYREJS.G0001V00.tmp-{exited}-0.old"),
                false,
                true,
            ),
            (format!(".DALYREJS.G001V00.tmp-{exited}-0"), false, true),
            (format!(".DALYREJS.G0000V00.tmp-{exited}-0"), false, true),
            (format!(".DALYREJS.G0001V01.tmp-{exited}-0"), false, true),
            (format!("DALYREJS.G0001V00.tmp-{exited}-0"), false, true),
            (format!(".XDALYREJS.G0001V00.tmp-{exited}-0"), false, true),
            (
                ".DALYREJS.G0001V00.tmp-99999999999-0".to_owned(),
                false,
                true,
            ),
            ("DALYREJS.G0004V00".to_owned(), false, true),
        ];
        let dir = TestDir::new("sweep");
        fs::create_dir_all(dir.path()).expect("directory creates");
        for (name, directory, _) in &cases {
            let path = dir.path().join(name);
            if *directory {
                fs::create_dir(&path).expect("directory creates");
            } else {
                fs::write(&path, b"REJECT\n").expect("file writes");
            }
        }
        let generations = Generations::new(dir.path());

        let pending = generations
            .begin_next(Dataset::DalyRejs)
            .expect("the sweep never fails the allocation");

        for (name, _, kept) in &cases {
            assert_eq!(dir.path().join(name).exists(), *kept, "{name}");
        }
        drop(pending);
        let mut left: Vec<&str> = cases
            .iter()
            .filter(|(_, _, kept)| *kept)
            .map(|(name, _, _)| name.as_str())
            .collect();
        left.sort_unstable();
        assert_eq!(dir.names(), left, "the allocation's own files are gone");
        assert_eq!(
            catalog_empty(&generations, Dataset::DalyRejs).to_string(),
            "DALYREJS.G0005V00",
            "the cataloged generation stays the newest"
        );
    }

    #[test]
    fn pending_generations_of_different_datasets_coexist() {
        let dir = TestDir::new("coexist");
        let generations = Generations::new(dir.path());

        let backup = generations
            .begin_next(Dataset::TransactBkup)
            .expect("backup allocates");
        let daily = generations
            .begin_next(Dataset::TransactDaly)
            .expect("daily allocates");
        let report = generations
            .begin_next(Dataset::TranRept)
            .expect("report allocates");

        assert_eq!(
            report.catalog().expect("report catalogs").to_string(),
            "TRANREPT.G0001V00"
        );
        assert_eq!(
            daily.catalog().expect("daily catalogs").to_string(),
            "TRANSACT.DALY.G0001V00"
        );
        assert_eq!(
            backup.catalog().expect("backup catalogs").to_string(),
            "TRANSACT.BKUP.G0001V00"
        );
        assert_eq!(
            dir.names(),
            [
                "TRANREPT.G0001V00",
                "TRANSACT.BKUP.G0001V00",
                "TRANSACT.DALY.G0001V00"
            ]
        );
    }

    #[test]
    fn catalog_never_replaces_an_existing_generation() {
        let dir = TestDir::new("no-clobber");
        let generations = Generations::new(dir.path());
        let mut pending = generations
            .begin_next(Dataset::DalyRejs)
            .expect("generation allocates");
        pending
            .append_records::<SixBytes>(&[*b"NEWREC"])
            .expect("record appends");
        let existing = dir.path().join("DALYREJS.G0001V00");
        fs::write(&existing, b"KEPT\n").expect("file writes");

        let error = pending.catalog().expect_err("the generation name is taken");
        assert!(
            matches!(
                &error,
                GenerationError::Io { path, source }
                    if *path == existing && source.kind() == io::ErrorKind::AlreadyExists
            ),
            "{error:?}"
        );
        assert_eq!(fs::read(&existing).expect("file is readable"), b"KEPT\n");
        assert_eq!(
            dir.names(),
            ["DALYREJS.G0001V00"],
            "the temporary file and the lock are removed"
        );
    }

    #[test]
    fn missing_directory_has_no_current_generation() {
        let dir = TestDir::new("missing");
        let root = dir.path().join("absent");
        let generations = Generations::new(&root);

        assert_eq!(
            generations
                .current(Dataset::SysTran)
                .expect("catalog lists"),
            None
        );
        let error = generations
            .require_current(Dataset::SysTran)
            .expect_err("nothing is cataloged");
        assert!(matches!(
            error,
            GenerationError::NotCataloged { dataset: "SYSTRAN" }
        ));
        assert_eq!(error.to_string(), "SYSTRAN(0): no generation is cataloged");
        assert!(!root.exists(), "reading the catalog creates no directory");
    }

    #[test]
    fn encode_fixed_lines_requires_exact_width_and_appends_lf() {
        let line = "R".repeat(133);
        let encoded = encode_fixed_lines(&[line.clone(), line.clone()], 133).expect("lines encode");
        assert_eq!(encoded, format!("{line}\n{line}\n").into_bytes());
        assert_eq!(encode_fixed_lines(&[], 133).expect("no lines encode"), b"");

        let short = "S".repeat(132);
        let error = encode_fixed_lines(&[line, short], 133).expect_err("line 2 is short");
        assert!(matches!(
            error,
            GenerationError::LineWidth {
                line: 2,
                expected: 133,
                actual: 132,
            }
        ));
        assert_eq!(error.to_string(), "line 2 is 132 bytes, expected 133");

        // Width counts bytes: two-byte characters make a 2-character line 4 bytes long.
        let wide = "éé".to_owned();
        assert!(matches!(
            encode_fixed_lines(&[wide], 2),
            Err(GenerationError::LineWidth { actual: 4, .. })
        ));
    }

    #[test]
    fn encode_fixed_lines_rejects_record_delimiters() {
        let report_line = format!("A\n{}", "B".repeat(131));
        let cases: [(&[&str], usize, usize, usize, Delimiter); 5] = [
            (&["A\nBC"], 4, 1, 2, Delimiter::Lf),
            (&["ABC\r"], 4, 1, 4, Delimiter::Cr),
            (&["\r\nAB"], 4, 1, 1, Delimiter::Cr),
            (&[report_line.as_str()], 133, 1, 2, Delimiter::Lf),
            (&["ABCD", "AB\nC"], 4, 2, 3, Delimiter::Lf),
        ];
        for (texts, width, line, position, delimiter) in cases {
            let lines: Vec<String> = texts.iter().map(|text| (*text).to_owned()).collect();
            let result = encode_fixed_lines(&lines, width);
            assert!(
                matches!(
                    result,
                    Err(GenerationError::LineDelimiter { line: l, position: p, delimiter: d })
                        if l == line && p == position && d == delimiter
                ),
                "{texts:?}: {result:?}"
            );
        }

        let error = encode_fixed_lines(&["A\nBC".to_owned()], 4).expect_err("line 1 holds LF");
        assert_eq!(
            error.to_string(),
            "line 1 holds LF (0x0A) at byte 2; a fixed-width line cannot contain a record \
             delimiter"
        );
        assert_eq!(
            encode_fixed_lines(&["AB\r\n".to_owned()], 4)
                .expect_err("line 1 holds CR")
                .to_string(),
            "line 1 holds CR (0x0D) at byte 3; a fixed-width line cannot contain a record \
             delimiter"
        );
        // Width is checked first: a short line holding LF is a width error.
        assert!(matches!(
            encode_fixed_lines(&["A\n".to_owned()], 4),
            Err(GenerationError::LineWidth {
                line: 1,
                expected: 4,
                actual: 2,
            })
        ));
    }

    #[test]
    fn list_ignores_foreign_files() {
        let dir = TestDir::new("foreign");
        fs::create_dir_all(dir.path().join("DALYREJS.G0006V00")).expect("directory creates");
        for name in [
            "DALYREJS.G0001V01",
            "DALYREJS.G0000V00",
            "DALYREJS.G12V00",
            "DALYREJS.G00002V00",
            "DALYREJS.GABCDV00",
            "DALYREJS.G0003V00.tmp",
            ".DALYREJS.G0002V00.tmp-1",
            "XDALYREJS.G0005V00",
            "SYSTRAN.G0001V00",
            "DALYREJS.G0004V00",
        ] {
            fs::write(dir.path().join(name), b"").expect("file writes");
        }
        let generations = Generations::new(dir.path());

        assert_eq!(numbers(&generations, Dataset::DalyRejs), [4]);
        assert_eq!(numbers(&generations, Dataset::SysTran), [1]);
        assert_eq!(numbers(&generations, Dataset::TranRept), [0_u16; 0]);
    }

    #[test]
    fn records_and_lines_round_trip_through_generations() {
        let dir = TestDir::new("round-trip");
        let generations = Generations::new(dir.path());

        let mut pending = generations
            .begin_next(Dataset::SysTran)
            .expect("generation allocates");
        pending
            .append_records::<SixBytes>(&[*b"ABCDEF"])
            .expect("record appends");
        // An `X(n)` field copies bytes, so `LF`, `CR LF` and a final `CR` are record data that
        // reads back unchanged.
        pending
            .append_records::<SixBytes>(&[*b"GH\nJKL", *b"MN\r\nQR", *b"STUVW\r"])
            .expect("records append");
        let records = pending.catalog().expect("generation catalogs");
        assert_eq!(
            fs::read(&records.path).expect("generation is readable"),
            b"ABCDEF\nGH\nJKL\nMN\r\nQR\nSTUVW\r\n"
        );
        assert_eq!(
            read_records::<SixBytes>(&records.path).expect("records decode"),
            [*b"ABCDEF", *b"GH\nJKL", *b"MN\r\nQR", *b"STUVW\r"]
        );

        let mut report = generations
            .begin_next(Dataset::TranRept)
            .expect("generation allocates");
        report
            .append_lines(&["HEAD".to_owned()], 4)
            .expect("line appends");
        assert!(matches!(
            report.append_lines(&["BAD".to_owned()], 4),
            Err(GenerationError::LineWidth { line: 1, .. })
        ));
        assert!(matches!(
            report.append_lines(&["GOOD".to_owned(), "BA\nD".to_owned()], 4),
            Err(GenerationError::LineDelimiter {
                line: 2,
                position: 3,
                delimiter: Delimiter::Lf,
            })
        ));
        let report = report.catalog().expect("generation catalogs");
        assert_eq!(report.path, dir.path().join("TRANREPT.G0001V00"));
        assert_eq!(
            fs::read(&report.path).expect("report is readable"),
            b"HEAD\n"
        );
    }

    #[test]
    fn read_records_reports_io_and_codec_errors() {
        let dir = TestDir::new("read-errors");
        let missing = dir.path().join("SYSTRAN.G0001V00");
        let error = read_records::<SixBytes>(&missing).expect_err("the file is absent");
        assert!(matches!(&error, GenerationError::Io { path, .. } if *path == missing));
        assert!(
            error
                .to_string()
                .starts_with(&format!("I/O error on {}: ", missing.display())),
            "{error}"
        );

        fs::create_dir_all(dir.path()).expect("directory creates");
        let file = dir.path().join("TRANSACT.BKUP.G0001V00");
        let cases: [(&[u8], CodecErrorKind, &str); 2] = [
            (
                b"ABCDEF\nGHIJKLM\n",
                CodecErrorKind::Separator { byte: b'M' },
                "byte 0x4D instead of LF after a full-width record",
            ),
            (
                b"ABCDEF\nGHI",
                CodecErrorKind::PartialFrame { frame_len: 3 },
                "final frame of 3 bytes is shorter than the record length plus LF",
            ),
        ];
        for (bytes, kind, text) in cases {
            fs::write(&file, bytes).expect("file writes");
            let error = read_records::<SixBytes>(&file).expect_err("record 2 is not framed");
            assert!(
                matches!(
                    &error,
                    GenerationError::Codec(CodecError {
                        record: Some(2),
                        kind: found,
                        ..
                    }) if *found == kind
                ),
                "{error}"
            );
            assert_eq!(
                error.to_string(),
                format!("record file decode failed: TEST-REC record 2: {text}")
            );
        }
    }

    /// A 350-byte `CVTRA05Y` record with `tran_id` and `desc`, the other fields fixed and FILLER
    /// spaces: `TRAN-AMT` `0000001000{` is +100.00.
    fn cvtra05y(tran_id: &str, desc: &str) -> String {
        let record = format!(
            "{tran_id:<16}010001{source:<10}{desc:<100}0000001000{{800000000{name:<50}\
             {city:<50}{zip:<10}4111111111111111{ts}{ts}{filler:<20}",
            source = "POS TERM",
            name = "MERCHANT",
            city = "CITY",
            zip = "12345",
            ts = "2022-07-18 00:00:00.000000",
            filler = "",
        );
        assert_eq!(record.len(), 350, "{record:?}");
        record
    }

    /// For a sound file and for each fault `read_records` reports, the reader yields what
    /// `read_records` returns: the same records, or the records before the fault and then the
    /// same error, and nothing after it.
    #[test]
    fn record_reader_yields_the_records_and_the_error_read_records_gives() {
        let dir = TestDir::new("record-reader");
        fs::create_dir_all(dir.path()).expect("directory creates");
        let first = cvtra05y("0000000000000001", "FIRST");
        // An `X(n)` field copies bytes, so `LF` and `CR` inside a record are record data.
        let second = cvtra05y("2022\n71800000001", "SECOND\r\nLINE");
        let mut bad_amount = cvtra05y("0000000000000003", "BAD").into_bytes();
        bad_amount[132..143].copy_from_slice(b"00000XX145F");
        // (case, the file's bytes; `None` leaves the file absent)
        let cases: [(&str, Option<Vec<u8>>); 6] = [
            ("sound", Some(format!("{first}\n{second}\n").into_bytes())),
            (
                "amount",
                Some(
                    [
                        format!("{first}\n").into_bytes(),
                        bad_amount,
                        b"\n".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (
                "truncated",
                Some(format!("{first}\n{}", &second[..341]).into_bytes()),
            ),
            ("crlf", Some(format!("{first}\r\n").into_bytes())),
            ("empty", Some(Vec::new())),
            ("missing", None),
        ];
        for (case, bytes) in cases {
            let path = dir.path().join(format!("SYSTRAN.G0001V00.{case}"));
            if let Some(bytes) = &bytes {
                fs::write(&path, bytes).expect("file writes");
            }
            let expected = read_records::<TransactionLayout>(&path);
            let actual = RecordReader::<TransactionLayout>::open(&path).and_then(|mut reader| {
                let items: Vec<_> = reader.by_ref().collect();
                assert!(
                    reader.next().is_none(),
                    "{case}: nothing after the end or the error"
                );
                assert!(
                    items.iter().skip_while(|item| item.is_ok()).count() <= 1,
                    "{case}: an error is the last item"
                );
                items.into_iter().collect::<Result<Vec<_>, _>>()
            });
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "{case}");
            assert_eq!(
                actual.err().map(|error| error.to_string()),
                expected.err().map(|error| error.to_string()),
                "{case}"
            );
        }
    }

    /// Batches read on where the batch before ended, so record numbers run on across them, and
    /// the asynchronous forms read what the synchronous ones do; the count reads the whole file
    /// as `read_records` does.
    #[tokio::test]
    async fn record_batches_and_counts_read_what_read_records_reads() {
        let dir = TestDir::new("record-batches");
        fs::create_dir_all(dir.path()).expect("directory creates");
        let sound = dir.path().join("SYSTRAN.G0001V00");
        fs::write(&sound, b"AAAAAA\nBBBBBB\nCCCCCC\nDDDDDD\nEEEEEE\n").expect("file writes");
        let short = dir.path().join("SYSTRAN.G0002V00");
        fs::write(&short, b"AAAAAA\nBBBBBB\nCCCCCC\nDDD").expect("file writes");
        let missing = dir.path().join("SYSTRAN.G0003V00");

        let mut reader = RecordReader::<SixBytes>::open(&sound).expect("file opens");
        let batches: Vec<_> = (0..4)
            .map(|_| reader.next_batch(2).expect("batch reads"))
            .collect();
        assert_eq!(
            batches,
            [
                vec![*b"AAAAAA", *b"BBBBBB"],
                vec![*b"CCCCCC", *b"DDDDDD"],
                vec![*b"EEEEEE"],
                vec![],
            ]
        );
        let mut reader = RecordReader::<SixBytes>::open_async(sound.clone())
            .await
            .expect("file opens");
        let mut lent = Vec::new();
        for _ in 0..4 {
            let (returned, batch) = reader.next_batch_async(2).await.expect("batch reads");
            reader = returned;
            lent.push(batch);
        }
        assert_eq!(lent, batches);

        let expected = read_records::<SixBytes>(&short).expect_err("record 4 is partial");
        let mut reader = RecordReader::<SixBytes>::open(&short).expect("file opens");
        assert_eq!(reader.next_batch(2).expect("batch reads").len(), 2);
        let error = reader
            .next_batch(2)
            .expect_err("the second batch meets record 4");
        assert_eq!(format!("{error:?}"), format!("{expected:?}"));
        let reader = RecordReader::<SixBytes>::open_async(short.clone())
            .await
            .expect("file opens");
        let (reader, _) = reader.next_batch_async(2).await.expect("batch reads");
        let error = reader
            .next_batch_async(2)
            .await
            .err()
            .expect("the second batch meets record 4");
        assert_eq!(format!("{error:?}"), format!("{expected:?}"));
        let opened = RecordReader::<SixBytes>::open_async(missing.clone())
            .await
            .err();
        assert_eq!(
            format!("{opened:?}"),
            format!("{:?}", read_records::<SixBytes>(&missing).err())
        );

        for path in [sound, short, missing] {
            let expected = format!(
                "{:?}",
                read_records::<SixBytes>(&path).map(|records| records.len())
            );
            let actual = format!("{:?}", count_records_async::<SixBytes>(path.clone()).await);
            assert_eq!(actual, expected, "{}", path.display());
        }
    }

    /// A work file is unlinked as soon as it is created, reads back any part written to it, side
    /// by side and more than once, and reports a part that ends inside a frame and bytes that
    /// are not what was written as I/O errors on its path.
    #[test]
    fn work_files_read_back_their_parts_and_leave_no_file() {
        let dir = TestDir::new("work-file");
        fs::create_dir_all(dir.path()).expect("directory creates");
        let generations = Generations::new(dir.path());

        let mut writer = generations.create_work_file().expect("work file creates");
        let path = writer.name.path.clone();
        let name = path
            .file_name()
            .and_then(OsStr::to_str)
            .expect("work file name is UTF-8");
        assert!(
            name.starts_with(&format!(".SORTWK.tmp-{}-", process::id())),
            "{name}"
        );
        assert!(dir.names().is_empty(), "the name is unlinked at once");
        writer.write(b"AAAA").expect("work file writes");
        let middle = writer.offset();
        writer.write(b"BBBBCCCC").expect("work file writes");
        assert_eq!((middle, writer.offset()), (4, 12));
        let work = writer.finish().expect("work file flushes");

        for _ in 0..2 {
            let mut readers = [work.reader(0, middle), work.reader(middle, 8)];
            let mut frame = [0_u8; 4];
            let mut read = Vec::new();
            for index in [1, 0, 1, 0, 1] {
                let whole = readers[index]
                    .read_frame(&mut frame)
                    .expect("work file reads");
                read.push(whole.then(|| frame.to_vec()));
            }
            assert_eq!(
                read,
                [
                    Some(b"BBBB".to_vec()),
                    Some(b"AAAA".to_vec()),
                    Some(b"CCCC".to_vec()),
                    None,
                    None,
                ]
            );
        }

        let mut odd = work.reader(0, 6);
        let mut frame = [0_u8; 4];
        assert!(odd.read_frame(&mut frame).expect("work file reads"));
        let error = odd
            .read_frame(&mut frame)
            .expect_err("the part ends inside a frame");
        assert!(
            matches!(&error, GenerationError::Io { path: at, source }
                if *at == path && source.kind() == io::ErrorKind::UnexpectedEof),
            "{error:?}"
        );
        let error = odd.invalid_data("not a frame");
        assert_eq!(
            error.to_string(),
            format!("I/O error on {}: not a frame", path.display())
        );
        drop(work);
        assert!(dir.names().is_empty(), "no work file is left");
    }

    #[test]
    fn begin_next_stops_after_g9999() {
        let dir = TestDir::new("exhausted");
        fs::create_dir_all(dir.path()).expect("directory creates");
        fs::write(dir.path().join("TRANSACT.COMBINED.G9999V00"), b"").expect("file writes");
        let generations = Generations::new(dir.path());

        let error = generations
            .begin_next(Dataset::TransactCombined)
            .expect_err("G9999V00 is the newest");
        assert!(matches!(
            error,
            GenerationError::Exhausted {
                dataset: "TRANSACT.COMBINED"
            }
        ));
        assert_eq!(
            dir.names(),
            ["TRANSACT.COMBINED.G9999V00"],
            "no temporary file is left"
        );
        assert_eq!(
            catalog_empty(&generations, Dataset::TransactDaly).to_string(),
            "TRANSACT.DALY.G0001V00"
        );
    }

    #[test]
    fn write_replacing_creates_the_directory_and_replaces_the_file() {
        let dir = TestDir::new("replacing");
        let path = dir.path().join("statements").join("STATEMNT.PS");

        write_replacing(&path, b"first\n").expect("file writes");
        write_replacing(&path, b"second\n").expect("file is replaced");

        assert_eq!(fs::read(&path).expect("file is readable"), b"second\n");
        let names: Vec<_> = fs::read_dir(dir.path().join("statements"))
            .expect("directory is readable")
            .map(|entry| entry.expect("directory entry is readable").file_name())
            .collect();
        assert_eq!(names, ["STATEMNT.PS"], "no temporary file is left");

        assert!(matches!(
            write_replacing(Path::new("/"), b""),
            Err(GenerationError::Io { .. })
        ));
    }

    /// Holds the first temporary allocation of a target while the second is made, so the two
    /// lifetimes overlap on every run rather than only when the scheduler interleaves writers.
    #[test]
    fn outstanding_temporary_files_of_one_target_are_distinct() {
        let dir = TestDir::new("outstanding-temps");
        fs::create_dir_all(dir.path()).expect("directory creates");
        let target = dir.path().join("STATEMNT.PS");
        let file_name = OsStr::new("STATEMNT.PS");

        let (first, first_file) = create_temp(&target, file_name).expect("first temporary creates");
        let (second, second_file) =
            create_temp(&target, file_name).expect("second temporary creates beside the first");
        assert_ne!(first, second);

        let prefix = format!(".STATEMNT.PS.tmp-{}-", process::id());
        let mut temp_names = Vec::new();
        for temp in [&first, &second] {
            assert_eq!(temp.parent(), Some(dir.path()), "{}", temp.display());
            let name = temp
                .file_name()
                .and_then(OsStr::to_str)
                .expect("temporary file name is UTF-8");
            let sequence = name.strip_prefix(&prefix);
            assert!(
                sequence.is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
                }),
                "{name}"
            );
            temp_names.push(name.to_owned());
        }
        temp_names.sort();

        write_synced(first_file, &first, b"first\n").expect("first temporary writes");
        write_synced(second_file, &second, b"second\n").expect("second temporary writes");
        assert_eq!(fs::read(&first).expect("first is readable"), b"first\n");
        assert_eq!(fs::read(&second).expect("second is readable"), b"second\n");
        assert_eq!(dir.names(), temp_names, "the target is never created");

        assert!(remove_temp(&first), "first temporary is deleted");
        assert!(remove_temp(&second), "second temporary is deleted");
        let left = dir.names();
        assert!(left.is_empty(), "no temporary file is left: {left:?}");
    }

    #[test]
    fn concurrent_write_replacing_never_shares_a_temporary_file() {
        let dir = TestDir::new("replacing-race");
        let path = dir.path().join("STATEMNT.PS");
        let target = path.as_path();
        let payloads: [&[u8]; 2] = [b"first\n", b"second\n"];

        for _ in 0..50 {
            thread::scope(|scope| {
                let writers =
                    payloads.map(|bytes| scope.spawn(move || write_replacing(target, bytes)));
                for writer in writers {
                    writer
                        .join()
                        .expect("writer does not panic")
                        .expect("file is replaced");
                }
            });
            let content = fs::read(&path).expect("file is readable");
            assert!(payloads.contains(&content.as_slice()), "{content:?}");
        }
        assert_eq!(dir.names(), ["STATEMNT.PS"], "no temporary file is left");
    }

    /// A shared in-memory sink standing in for standard output, also used by the job-span
    /// tests of `crate::jobs`.
    #[derive(Clone, Default)]
    pub(crate) struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        /// The JSON lines written so far.
        pub(crate) fn lines(&self) -> Vec<String> {
            let bytes = self.0.lock().expect("capture buffer").clone();
            String::from_utf8(bytes)
                .expect("JSON lines are UTF-8")
                .lines()
                .map(str::to_owned)
                .collect()
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

    #[tokio::test]
    async fn blocking_returns_the_value_of_work_run_on_the_blocking_pool() {
        let caller = thread::current().id();
        let (value, worker) = blocking(|| (6 * 7, thread::current().id()))
            .await
            .expect("the runtime is running");
        assert_eq!(value, 42);
        assert_ne!(worker, caller, "the work ran on another thread");
    }

    #[tokio::test]
    async fn a_panic_in_blocking_work_resumes_on_the_caller() {
        let caller = tokio::spawn(blocking(|| -> u8 { panic!("blocking work failed") }));
        let error = caller.await.expect_err("the caller unwinds");
        assert!(error.is_panic(), "{error:?}");
        assert_eq!(
            error.into_panic().downcast_ref::<&str>(),
            Some(&"blocking work failed"),
            "the original payload is resumed"
        );
    }

    /// Runs blocking work inside a job span under the batch telemetry subscriber, installed as
    /// the default of the caller's thread only, and checks that the event and the span record the
    /// work makes on its pool thread reach that subscriber inside that span. Only callsites of
    /// this test are asserted on: a callsite shared with tests that run without a subscriber can
    /// have its interest cached as `never` by a concurrent first registration.
    #[tokio::test]
    async fn blocking_work_runs_in_the_callers_span_and_dispatcher() {
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) = subscriber_with_writer("batch", "info", move || sink.clone())
            .expect("valid directives");
        let caller = thread::current().id();
        let worker = {
            let _default = tracing::subscriber::set_default(subscriber);
            let span = tracing::info_span!("job", job = "blocking-test", written = Empty);
            blocking(|| {
                Span::current().record("written", 7);
                tracing::info!("inside blocking work");
                thread::current().id()
            })
            .instrument(span)
            .await
            .expect("the runtime is running")
        };
        guard.shutdown().expect("the tracer provider shuts down");
        assert_ne!(worker, caller, "the work ran on another thread");

        let lines = capture.lines();
        let event = lines
            .iter()
            .find(|line| line.contains(r#""message":"inside blocking work""#))
            .unwrap_or_else(|| panic!("no event from the blocking work: {lines:?}"));
        assert!(
            event.contains(r#""span":{"job":"blocking-test","written":7,"name":"job"}"#),
            "{event}"
        );
        let span = lines
            .iter()
            .find(|line| line.contains(r#""type":"span""#))
            .unwrap_or_else(|| panic!("no span line: {lines:?}"));
        assert!(
            span.contains(r#""job":"blocking-test""#) && span.contains(r#""written":7"#),
            "{span}"
        );
    }

    #[tokio::test]
    async fn async_methods_write_what_the_sync_methods_do() {
        let sync_dir = TestDir::new("sync-path");
        let async_dir = TestDir::new("async-path");
        let sync = Generations::new(sync_dir.path());
        let asynchronous = Generations::new(async_dir.path());

        let mut sync_records = sync
            .begin_next(Dataset::SysTran)
            .expect("generation allocates");
        let mut async_records = asynchronous
            .begin_next_async(Dataset::SysTran)
            .await
            .expect("generation allocates");
        for batch in [vec![*b"ABCDEF"], vec![*b"GHIJKL", *b"MNOPQR"]] {
            sync_records
                .append_records::<SixBytes>(&batch)
                .expect("records append");
            async_records
                .append_records_async::<SixBytes>(batch)
                .await
                .expect("records append");
        }

        let mut sync_report = sync
            .begin_next(Dataset::TranRept)
            .expect("generation allocates");
        let mut async_report = asynchronous
            .begin_next_async(Dataset::TranRept)
            .await
            .expect("generation allocates");
        let line_batches: [&[&str]; 3] = [&["HEAD"], &["BAD"], &["GOOD", "BA\nD"]];
        for texts in line_batches {
            let lines: Vec<String> = texts.iter().map(|text| (*text).to_owned()).collect();
            let expected = format!("{:?}", sync_report.append_lines(&lines, 4));
            let actual = format!("{:?}", async_report.append_lines_async(lines, 4).await);
            assert_eq!(actual, expected, "{texts:?}");
        }

        let cataloged = [
            (
                sync_records.catalog().expect("generation catalogs"),
                async_records
                    .catalog_async()
                    .await
                    .expect("generation catalogs"),
                &b"ABCDEF\nGHIJKL\nMNOPQR\n"[..],
            ),
            (
                sync_report.catalog().expect("generation catalogs"),
                async_report
                    .catalog_async()
                    .await
                    .expect("generation catalogs"),
                &b"HEAD\n"[..],
            ),
        ];
        for (expected, actual, bytes) in &cataloged {
            assert_eq!(
                (actual.dataset, actual.number),
                (expected.dataset, expected.number)
            );
            assert_eq!(actual.path, async_dir.path().join(expected.to_string()));
            assert_eq!(
                fs::read(&expected.path).expect("generation is readable"),
                *bytes
            );
            assert_eq!(
                fs::read(&actual.path).expect("generation is readable"),
                *bytes
            );
        }
        assert_eq!(
            async_dir.names(),
            sync_dir.names(),
            "no lock or temporary file is left"
        );
    }

    #[tokio::test]
    async fn discard_async_deletes_the_temporary_file_and_releases_the_lock() {
        let dir = TestDir::new("discard-async");
        let generations = Generations::new(dir.path());
        let first = catalog_empty(&generations, Dataset::DalyRejs);

        let mut pending = generations
            .begin_next_async(Dataset::DalyRejs)
            .await
            .expect("generation allocates");
        pending
            .append_records_async::<SixBytes>(vec![*b"REJECT"])
            .await
            .expect("record appends");
        let names = dir.names();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names[0].starts_with(".DALYREJS.G0002V00.tmp-"), "{names:?}");
        assert_eq!(names[1], ".DALYREJS.lock", "{names:?}");

        pending.discard_async().await;
        assert_eq!(
            dir.names(),
            ["DALYREJS.G0001V00"],
            "the temporary file and the lock are removed"
        );
        assert_eq!(
            generations
                .current(Dataset::DalyRejs)
                .expect("catalog lists"),
            Some(first)
        );
        assert_eq!(
            catalog_empty(&generations, Dataset::DalyRejs).to_string(),
            "DALYREJS.G0002V00",
            "the discarded number is allocated again"
        );
    }

    /// A generation whose writer is gone, as a cancelled asynchronous append leaves it, is
    /// refused by both catalog paths and deleted.
    #[tokio::test]
    async fn catalog_refuses_a_generation_whose_file_is_closed() {
        for asynchronous in [false, true] {
            let dir = TestDir::new(&format!("closed-{asynchronous}"));
            let generations = Generations::new(dir.path());
            let mut pending = generations
                .begin_next(Dataset::SysTran)
                .expect("generation allocates");
            pending
                .append_records::<SixBytes>(&[*b"PARTLY"])
                .expect("record appends");
            drop(pending.writer.take());
            let temp = pending.temp_path.clone();

            let result = if asynchronous {
                pending.catalog_async().await
            } else {
                pending.catalog()
            };
            let error = result.expect_err("a closed generation is refused");
            assert!(
                matches!(
                    &error,
                    GenerationError::Io { path, source }
                        if *path == temp && source.to_string() == "generation file is already closed"
                ),
                "{asynchronous}: {error:?}"
            );
            let left = dir.names();
            assert!(
                left.is_empty(),
                "{asynchronous}: the temporary file and the lock are removed: {left:?}"
            );
            assert_eq!(
                generations
                    .current(Dataset::SysTran)
                    .expect("catalog lists"),
                None,
                "{asynchronous}: nothing is cataloged"
            );
        }
    }
}
