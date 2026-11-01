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
//!   temporary sibling; [`PendingGeneration::catalog`] renames it into place when the step ends
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
//! The catalog takes no lock. JES serializes jobs that allocate the same GDG through the dataset
//! enqueue; here one operator runs the jobs one at a time, as the README cycle does.
//!
//! # Record files
//!
//! Record files use the ASCII codec layouts at full copybook width with `LF` terminators
//! ([`PendingGeneration::append_records`], [`read_records`]). Print and statement files are
//! fixed-width text lines with `LF` terminators, checked by [`encode_fixed_lines`]: `TRANREPT`
//! 133 bytes (`LRECL=133` [app/jcl/TRANREPT.jcl:L78]), STMTFILE 80 and HTMLFILE 100
//! [app/cbl/CBSTM03A.CBL:L44-47]. The statement files are cataloged datasets, not GDGs, and are
//! replaced whole by [`write_replacing`].

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process;

use carddemo_codec::framing::{decode_ascii_file, encode_ascii_lf};
use carddemo_codec::{CodecError, Layout};

/// `LIMIT(5) SCRATCH`: the number of generations each GDG keeps [app/jcl/DEFGDGB.jcl:STEP05]
/// [app/jcl/DALYREJS.jcl:STEP05].
pub(crate) const LIMIT: usize = 5;

/// The first absolute generation number, `G0001V00`.
const FIRST_NUMBER: u16 = 1;

/// The highest absolute generation number, `G9999V00`.
const MAX_NUMBER: u16 = 9999;

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
    /// empty GDG. It creates the catalog directory and opens the hidden temporary file
    /// `.<DATASET>.G<nnnn>V00.tmp-<pid>` that the generation is written to until it is cataloged.
    ///
    /// # Errors
    ///
    /// - [`GenerationError::Exhausted`] when `G9999V00` is the newest generation.
    /// - [`GenerationError::Io`] when the directory cannot be read or created, or the temporary
    ///   file cannot be created.
    pub(crate) fn begin_next(
        &self,
        dataset: Dataset,
    ) -> Result<PendingGeneration, GenerationError> {
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
        fs::create_dir_all(&self.root).map_err(|source| io_error(&self.root, source))?;
        let name = generation_name(dataset, number);
        let final_path = self.root.join(&name);
        let temp_path = self.root.join(temp_name(OsStr::new(&name)));
        let file = File::create(&temp_path).map_err(|source| io_error(&temp_path, source))?;
        Ok(PendingGeneration {
            dataset,
            number,
            temp_path,
            final_path,
            writer: Some(BufWriter::new(file)),
            root: self.root.clone(),
            cataloged: false,
        })
    }
}

/// A `(+1)` generation being written: the `DISP=(NEW,CATLG,DELETE)` DD of a running step.
///
/// [`PendingGeneration::catalog`] is the normal end of the step. Dropping the value without
/// cataloging it is the abnormal end: the temporary file is deleted and no generation is added.
#[must_use = "a pending generation is deleted unless it is cataloged"]
#[derive(Debug)]
pub(crate) struct PendingGeneration {
    dataset: Dataset,
    number: u16,
    temp_path: PathBuf,
    final_path: PathBuf,
    /// Taken only by `catalog` and `drop`, both of which consume the generation.
    writer: Option<BufWriter<File>>,
    root: PathBuf,
    cataloged: bool,
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
    /// [`encode_fixed_lines`] encodes them. Nothing is written when a line has the wrong width.
    ///
    /// # Errors
    ///
    /// [`GenerationError::LineWidth`] as [`encode_fixed_lines`] reports it, and
    /// [`GenerationError::Io`] when the temporary file cannot be written.
    pub(crate) fn append_lines(
        &mut self,
        lines: &[String],
        width: usize,
    ) -> Result<(), GenerationError> {
        let bytes = encode_fixed_lines(lines, width)?;
        self.write(&bytes)
    }

    /// Catalogs the generation at the normal end of its step: flushes and syncs the temporary
    /// file, renames it to `<DATASET>.G<nnnn>V00`, then scratches the oldest generations until the
    /// newest [`LIMIT`] remain. A generation with nothing appended catalogs as an empty file, as
    /// `DALYREJS(+1)` does after a run without rejects.
    ///
    /// # Errors
    ///
    /// [`GenerationError::Io`] when the flush, the sync or the rename fails. The temporary file is
    /// then deleted and no generation is added.
    pub(crate) fn catalog(mut self) -> Result<Generation, GenerationError> {
        if let Some(writer) = self.writer.take() {
            let file = writer
                .into_inner()
                .map_err(|e| io_error(&self.temp_path, e.into_error()))?;
            file.sync_all()
                .map_err(|source| io_error(&self.temp_path, source))?;
            drop(file);
        }
        fs::rename(&self.temp_path, &self.final_path)
            .map_err(|source| io_error(&self.final_path, source))?;
        self.cataloged = true;
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
        let written = match self.writer.as_mut() {
            Some(writer) => writer.write_all(bytes),
            None => Err(io::Error::other("generation file is already closed")),
        };
        written.map_err(|source| io_error(&self.temp_path, source))
    }
}

impl Drop for PendingGeneration {
    /// `DISP=(…,DELETE)` on abnormal end: an uncataloged generation's temporary file is deleted.
    /// Best effort, because a destructor has no caller to report to: a failure is logged.
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
/// # Errors
///
/// [`GenerationError::Io`] when the file cannot be read, and [`GenerationError::Codec`] for the
/// first record that does not decode, located at its 1-based record number.
pub(crate) fn read_records<L: Layout>(path: &Path) -> Result<Vec<L::Record>, GenerationError> {
    let bytes = fs::read(path).map_err(|source| io_error(path, source))?;
    Ok(decode_ascii_file::<L>(&bytes)?
        .into_iter()
        .map(|framed| framed.record)
        .collect())
}

/// Encodes fixed-width text lines as a record file: each line, which must be exactly `width`
/// bytes, followed by `LF`. Lines are never padded or truncated, so a width error in the code
/// that formats them surfaces here instead of shifting the records of the file.
///
/// # Errors
///
/// [`GenerationError::LineWidth`] for the first line whose byte length is not `width`.
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
        out.extend_from_slice(text.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

/// Replaces the file `path` with `bytes` as a whole: writes and syncs a hidden temporary sibling,
/// then renames it over `path`, so a reader never sees a partly written file. The parent
/// directory is created first. `creastmt` writes `STATEMNT.PS`, `STATEMNT.HTML` and
/// `STATEMNT.PS.PDF` this way, because they are cataloged datasets, not GDGs.
///
/// # Errors
///
/// [`GenerationError::Io`] when `path` names no file, or when the directory cannot be created or
/// the file cannot be written, synced or renamed. The temporary file is then deleted and `path`
/// keeps its previous content.
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
    let temp_path = path.with_file_name(temp_name(file_name));
    let replaced = write_synced(&temp_path, bytes)
        .and_then(|()| fs::rename(&temp_path, path).map_err(|source| io_error(path, source)));
    if replaced.is_err() {
        remove_temp(&temp_path);
    }
    replaced
}

/// Creates `path`, writes `bytes` to it and syncs it to disk.
fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), GenerationError> {
    let mut file = File::create(path).map_err(|source| io_error(path, source))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| io_error(path, source))
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

/// The hidden temporary name `.<file name>.tmp-<pid>` a file is written under before it is
/// renamed into place. The leading dot keeps it out of [`Generations::list`].
fn temp_name(file_name: &OsStr) -> OsString {
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(format!(".tmp-{}", process::id()));
    name
}

/// Deletes the temporary file `path`, reporting whether it was there. Best effort: a failure
/// other than an absent file is logged, because the callers are already failing or dropping.
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
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;

    use carddemo_codec::{Charset, CodecError, CodecErrorKind, FieldKind, FieldSpec, Layout};

    use super::{
        Dataset, Generation, GenerationError, Generations, LIMIT, encode_fixed_lines, read_records,
        write_replacing,
    };

    /// A test directory under the system temporary directory, removed when the test ends.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(test: &str) -> Self {
            let path = env::temp_dir().join(format!("carddemo-gdg-{}-{test}", process::id()));
            // A directory left by an earlier run with the same process id is stale.
            let _ = fs::remove_dir_all(&path);
            Self(path)
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
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
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

        assert_eq!(dir.names(), ["DALYREJS.G0001V00"]);
        assert_eq!(
            generations
                .current(Dataset::DalyRejs)
                .expect("catalog lists"),
            Some(first)
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
        pending
            .append_records::<SixBytes>(&[*b"GHIJKL", *b"MNOPQR"])
            .expect("records append");
        let records = pending.catalog().expect("generation catalogs");
        assert_eq!(
            fs::read(&records.path).expect("generation is readable"),
            b"ABCDEF\nGHIJKL\nMNOPQR\n"
        );
        assert_eq!(
            read_records::<SixBytes>(&records.path).expect("records decode"),
            [*b"ABCDEF", *b"GHIJKL", *b"MNOPQR"]
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
        let long = dir.path().join("TRANSACT.BKUP.G0001V00");
        fs::write(&long, b"ABCDEF\nGHIJKLM\n").expect("file writes");
        let error = read_records::<SixBytes>(&long).expect_err("record 2 is too long");
        assert!(matches!(
            &error,
            GenerationError::Codec(CodecError {
                record: Some(2),
                kind: CodecErrorKind::RecordTooLong { stored_len: 7 },
                ..
            })
        ));
        assert_eq!(
            error.to_string(),
            "record file decode failed: TEST-REC record 2: stored length 7 exceeds the record length"
        );
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
}
