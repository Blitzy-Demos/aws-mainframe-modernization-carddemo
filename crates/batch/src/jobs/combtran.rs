//! `batch combtran`: the COMBTRAN job [app/jcl/COMBTRAN.jcl], which combines the transaction
//! master backup with the interest transactions INTCALC generated and loads the result into the
//! live transaction master.
//!
//! | Step | JCL | Function | Audit rows |
//! |---|---|---|---|
//! | `STEP05R` | `SORT` of `TRANSACT.BKUP(0)` then `SYSTRAN(0)`, `SORT FIELDS=(TRAN-ID,A)` with `TRAN-ID,1,16,CH`, into `TRANSACT.COMBINED(+1)` [app/jcl/COMBTRAN.jcl:L22-37] | `sort_step` | P-COMBTRAN-001, P-COMBTRAN-004, P-COMBTRAN-009 |
//! | `STEP10` | `IDCAMS REPRO INFILE(TRANSACT) OUTFILE(TRANVSAM)` of `TRANSACT.COMBINED(+1)` into `TRANSACT.VSAM.KSDS`, without `REPLACE` [app/jcl/COMBTRAN.jcl:L41-48] | `repro_step` | P-COMBTRAN-002, P-COMBTRAN-003, P-COMBTRAN-005 to P-COMBTRAN-008 |
//!
//! Neither step has a `COND`, so `STEP10` runs whenever `STEP05R` ends normally, and the job
//! ends with the condition code of the `REPRO`: [`ExitStatus::ReproCondition`], which carries
//! no other meaning (an 8 here is never a reward halt).
//!
//! `STEP05R` runs whole on Tokio's blocking pool, inside the job span, and `STEP10` reads the
//! combined file there, once before its load and in batches during it. A blocking task the
//! runtime cancels while shutting down is a setup failure.
//!
//! # Memory
//!
//! Neither step holds a whole file, so the job's memory does not grow with its files. `STEP05R`
//! reads `TRANSACT.BKUP(0)` and then `SYSTRAN(0)` one record at a time and sorts them as DFSORT
//! sorts an input larger than its memory: in chunks of at most [`CHUNK_RECORDS`] records. An
//! input that fits one chunk is sorted in memory alone. A larger one is written chunk by chunk,
//! each chunk sorted, as runs to a sort work file in the catalog directory, the `SORTWK` work
//! dataset ([`crate::generations`], "Sort work files"). While there are more runs than
//! [`FAN_IN`], each group of [`FAN_IN`] consecutive runs is merged into one run on a new work
//! file. No list of the runs is kept: every run of a work file but its last holds the same
//! number of records, so how many runs it holds and where each lies follow from that number and
//! the records on the file ([`RunLayout`]). The last runs are merged twice: once to check the
//! equal keys (E-38), before `TRANSACT.COMBINED(+1)` is allocated, and once to write it,
//! [`RECORD_BATCH`] records at a time. The work files are gone when the step ends, however it
//! ends. `STEP10` reads `TRANSACT.COMBINED(+1)` twice: once whole, one record at a time, to check
//! and count it before the load begins, and once [`RECORD_BATCH`] records at a time for the load.
//!
//! Nothing the job holds grows with its input: at most one chunk of [`CHUNK_RECORDS`] records,
//! at most [`FAN_IN`] run readers with their buffers, one batch of [`RECORD_BATCH`] records, and
//! a fixed number of counters.
//!
//! # The load (E-09)
//!
//! The live master is the set of rows the view `transaction_master` holds. `REPRO` without
//! `REPLACE` neither clears its target nor replaces a record, so each combined record goes, in
//! combined order and inside one unit of work, through
//! `PgStore::load_combined_record`: a TRAN-ID already live is not loaded and the live row keeps
//! its content; a record equal to a row outside the live master in TRAN-ID and content makes
//! that row live again, so its reward accruals stay linked; any other record becomes a new live
//! row. Rows loaded before `REPRO` stops at its error limit stay loaded, as records written to
//! the KSDS do.
//!
//! # Failures without a COBOL counterpart
//!
//! - `STEP05R` failures, before `TRANSACT.COMBINED(+1)` is cataloged, are setup failures
//!   (exit 16): a `(0)` generation that is not cataloged, a SORTIN file that cannot be read or
//!   decoded, a sort work file that cannot be created, written or read back (`SORTWK write
//!   failed`, `SORTWK read failed`), and a failure to allocate, write or catalog
//!   `TRANSACT.COMBINED(+1)`. No `TRANSACT.COMBINED(+1)` is then cataloged, `STEP10` does not
//!   run and the live master is untouched. A SORTIN that cannot be read or decoded is named by its dataset, generation and
//!   path. The job span holds the two SORTIN generations in `input` once both are resolved,
//!   `read` the SORTIN records read before the failure (the `TRANSACT.BKUP(0)` records when
//!   `SYSTRAN(0)` is the one that fails), and `posted` 0 and `rejected` 0.
//! - `STEP10` failures, after `TRANSACT.COMBINED(+1)` is cataloged, are setup failures (exit 16)
//!   too: a combined file that cannot be read or decoded, a unit of work that cannot begin, a
//!   record that cannot be loaded, and a commit that fails. The cataloged
//!   `TRANSACT.COMBINED(+1)` stays, and the live master is as the run found it, because the load
//!   never began or is rolled back whole, so a rerun starts from the same live master. The job
//!   span holds `posted` 0 and in `rejected` the duplicate records met before the failure, and
//!   one `warn` event gives the records the uncommitted load inserted, revived and found
//!   duplicate.
//! - Two different records with one TRAN-ID in the SORT input stop the job before
//!   `TRANSACT.COMBINED(+1)` is allocated: an equivalence halt (exit 16, E-38).

use std::fmt;
use std::mem;
use std::ops::{ControlFlow, Range};
use std::slice;
use std::sync::Arc;

use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
use carddemo_codec::{Charset, Layout};
use carddemo_domain::rules::transaction_report::TranId;
use carddemo_persistence::repos::LoadOutcome;
use carddemo_persistence::tx::{Db, UnitOfWork};
use tracing::Span;
use tracing::field::display;

use super::{JobContext, JobCounts, count_of, setup_failure, span_count};
use crate::exit::ExitStatus;
use crate::generations::{
    Dataset, Generation, GenerationError, Generations, PendingGeneration, RecordReader, WorkFile,
    WorkFileWriter, WorkReader, blocking, count_records_async,
};

/// The default `ERRORLIMIT` of `REPRO`: the number of failed records at which it stops
/// (P-COMBTRAN-003). The COMBTRAN `REPRO` names no `ERRORLIMIT` [app/jcl/COMBTRAN.jcl:L48].
const ERROR_LIMIT: usize = 4;

/// The condition code `REPRO` ends with after one to three duplicate records (P-COMBTRAN-003,
/// P-COMBTRAN-008).
const DUPLICATE_CONDITION: u8 = 8;

/// The condition code `REPRO` ends with when it stops at [`ERROR_LIMIT`] (P-COMBTRAN-003,
/// P-COMBTRAN-006, P-COMBTRAN-007).
const ERROR_LIMIT_CONDITION: u8 = 12;

/// The `halt` escalation for SORT input whose equal keys have no defined order.
const EQUAL_KEY_ESCALATION: &str = "E-38";

/// The SORTIN records `STEP05R` holds and sorts in memory at once: a bound on the records the
/// job holds, not a COBOL value. An input of at most this many records is sorted in memory
/// alone; a larger one is written to a sort work file in sorted runs of this many records.
const CHUNK_RECORDS: usize = 4096;

/// The sorted runs `STEP05R` merges at once: a bound on the work-file readers, and the frames
/// they buffer, that the job holds together.
const FAN_IN: usize = 16;

/// The combined records `STEP05R` appends to `TRANSACT.COMBINED(+1)` at once and `STEP10` reads
/// from it at once: a bound on the records the job holds, not a COBOL value. `REPRO` copies one
/// record at a time.
const RECORD_BATCH: usize = 500;

/// The bytes of a SORTIN position in a sorted-run frame: a big-endian `u64`.
const POSITION_LEN: usize = size_of::<u64>();

/// The bytes of one sorted-run frame on a sort work file: the record's position in the SORTIN
/// concatenation, then the 350-byte `CVTRA05Y` record as `TRANSACT.COMBINED(+1)` holds it.
const RUN_FRAME: usize = POSITION_LEN + TransactionLayout::LEN;

/// The bounds of the `STEP05R` sort: [`SORT_LIMITS`] in the job. The tests pass smaller ones, so
/// that a few records already spill to work files and cascade.
#[derive(Debug, Clone, Copy)]
struct SortLimits {
    /// The records sorted in memory at once; below 1 counts as 1.
    chunk: usize,
    /// The runs merged at once; below 2 counts as 2.
    fan_in: usize,
}

/// The sort bounds of the job: [`CHUNK_RECORDS`] and [`FAN_IN`].
const SORT_LIMITS: SortLimits = SortLimits {
    chunk: CHUNK_RECORDS,
    fan_in: FAN_IN,
};

/// COMBTRAN: `STEP05R` on the blocking pool, then `STEP10` on the generation it cataloged.
pub(super) async fn run(ctx: &JobContext, db: &Db) -> ExitStatus {
    let generations = ctx.generations.clone();
    let counts = Arc::clone(&ctx.counts);
    let sorted = match blocking(move || sort_step(&generations, &counts)).await {
        Ok(sorted) => sorted,
        Err(cancelled) => return setup_failure("STEP05R SORT did not complete", cancelled),
    };
    match sorted {
        Ok(combined) => repro_step(db, &combined, &ctx.counts).await,
        Err(status) => status,
    }
}

/// The SORTIN concatenation once read: its two `(0)` generations, in concatenation order, and
/// their records counted.
struct SortIn {
    /// `TRANSACT.BKUP(0)`, the first SORTIN.
    backup: Generation,
    /// The records of `TRANSACT.BKUP(0)`: the concatenation positions below this are its.
    backup_records: usize,
    /// `SYSTRAN(0)`, the second SORTIN.
    systran: Generation,
    /// The records of both.
    records: usize,
}

/// STEP05R `PGM=SORT` [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001, P-COMBTRAN-004,
/// P-COMBTRAN-009: [`sort_step_within`] the job's bounds, [`SORT_LIMITS`].
///
/// # Errors
///
/// The errors of [`sort_step_within`].
fn sort_step(generations: &Generations, counts: &JobCounts) -> Result<Generation, ExitStatus> {
    sort_step_within(generations, counts, SORT_LIMITS)
}

/// STEP05R `PGM=SORT` [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001, P-COMBTRAN-004,
/// P-COMBTRAN-009, sorting within `limits`.
///
/// Resolves `TRANSACT.BKUP(0)` and `SYSTRAN(0)` and records their generation names, in SORTIN
/// concatenation order and separated by a comma (`TRANSACT.BKUP.G0001V00,SYSTRAN.G0001V00`), in
/// `input` on the job span before either is read, so a SORTIN that cannot be read still shows
/// the inputs. Then reads both, in that order and one record at a time, into a [`Sort`]
/// ([`read_sortin`]), checks the sorted records' equal keys ([`check_equal_keys`]) and writes
/// them to `TRANSACT.COMBINED(+1)` ([`write_combined`]). Sets `read` in `counts` to the SORTIN
/// records read, once `TRANSACT.BKUP(0)` is read and again once `SYSTRAN(0)` is, so a
/// `SYSTRAN(0)` that cannot be read leaves the `TRANSACT.BKUP(0)` records counted. `posted` and
/// `rejected` stay 0 when the step fails, because `STEP10` then never runs. Every sort work file
/// is deleted when the step returns, however it ends.
///
/// Synchronous throughout: `run` calls it on the blocking pool, where the job span is current.
///
/// # Errors
///
/// - A setup failure (exit 16) when a `(0)` generation is not cataloged or cannot be read, or
///   `TRANSACT.COMBINED(+1)` cannot be allocated, written or cataloged. The audit records no
///   condition code for these, and a missing input is a JCL error that runs no step at all. A
///   SORTIN that cannot be read or decoded is named by its dataset, generation and path
///   ([`read_sortin`]).
/// - A setup failure (exit 16) when a sort work file cannot be created, written or read back,
///   which the COBOL has no counterpart for: `SORTWK write failed` or `SORTWK read failed`,
///   with the work file's path. A work file that ends holding other bytes than the frames its
///   runs are computed from is `SORTWK write failed` too ([`finish_work`]).
/// - An equivalence halt (exit 16, E-38) when two different records share a TRAN-ID. Nothing is
///   allocated or written.
fn sort_step_within(
    generations: &Generations,
    counts: &JobCounts,
    limits: SortLimits,
) -> Result<Generation, ExitStatus> {
    let backup = current_sortin(generations, Dataset::TransactBkup)?;
    let systran = current_sortin(generations, Dataset::SysTran)?;
    Span::current().record("input", display(format_args!("{backup},{systran}")));
    let mut sort = Sort::new(generations, limits);
    let backup_records = read_sortin(&backup, &mut sort)?;
    counts.set_read(count_of(backup_records));
    let systran_records = read_sortin(&systran, &mut sort)?;
    let records = backup_records + systran_records;
    counts.set_read(count_of(records));
    let sortin = SortIn {
        backup,
        backup_records,
        systran,
        records,
    };
    let sorted = sort.finish()?;
    check_equal_keys(&sorted, &sortin)?;
    write_combined(generations, &sorted, &sortin)
}

/// STEP05R `SORTOUT` [app/jcl/COMBTRAN.jcl:L33-37] — P-COMBTRAN-001, P-COMBTRAN-004,
/// P-COMBTRAN-009: the records of `sorted`, the SORTIN concatenation `sortin` in TRAN-ID order,
/// written to `TRANSACT.COMBINED(+1)` by one more pass of the final merge, [`RECORD_BATCH`]
/// records at a time: 350-byte `CVTRA05Y` records as read, FILLER included (`DCB=(*.SORTIN)`),
/// each followed by `LF`. Records the number of combined records in `written` on the job span.
///
/// # Errors
///
/// - A setup failure (exit 16) when `TRANSACT.COMBINED(+1)` cannot be allocated, written or
///   cataloged, or a sort work file cannot be read back. The uncataloged generation is then
///   deleted.
fn write_combined(
    generations: &Generations,
    sorted: &Sorted,
    sortin: &SortIn,
) -> Result<Generation, ExitStatus> {
    let mut pending = generations
        .begin_next(Dataset::TransactCombined)
        .map_err(|err| setup_failure("TRANSACT.COMBINED(+1) allocation failed", err))?;
    let mut pass = sorted.pass()?;
    let mut batch = Vec::with_capacity(RECORD_BATCH);
    let mut written = 0;
    while let Some((_, record)) = pass.next_record()? {
        batch.push(record);
        if batch.len() == RECORD_BATCH {
            written += append_combined(&mut pending, &mut batch)?;
        }
    }
    written += append_combined(&mut pending, &mut batch)?;
    let combined = pending
        .catalog()
        .map_err(|err| setup_failure("TRANSACT.COMBINED(+1) catalog failed", err))?;
    Span::current().record("written", span_count(written));
    tracing::info!(
        backup = %sortin.backup,
        systran = %sortin.systran,
        generation = %combined,
        records_in = sortin.records,
        records_out = written,
        "SORT FIELDS=(TRAN-ID,A) completed"
    );
    Ok(combined)
}

/// STEP05R `SORTOUT` [app/jcl/COMBTRAN.jcl:L33-37] — P-COMBTRAN-001: appends `batch`, the next
/// combined records, to `pending`, `TRANSACT.COMBINED(+1)`, and empties it. Returns the
/// records appended.
///
/// # Errors
///
/// A setup failure (exit 16) when `TRANSACT.COMBINED(+1)` cannot be written.
fn append_combined(
    pending: &mut PendingGeneration,
    batch: &mut Vec<TransactionRecord>,
) -> Result<usize, ExitStatus> {
    pending
        .append_records::<TransactionLayout>(batch)
        .map_err(|err| setup_failure("TRANSACT.COMBINED(+1) write failed", err))?;
    let appended = batch.len();
    batch.clear();
    Ok(appended)
}

/// The `(0)` generation of `dataset`, one SORTIN of the concatenation.
///
/// # Errors
///
/// A setup failure (exit 16) when no generation is cataloged, naming the dataset
/// (`SYSTRAN(0): no generation is cataloged`), or the catalog cannot be listed.
fn current_sortin(generations: &Generations, dataset: Dataset) -> Result<Generation, ExitStatus> {
    generations
        .require_current(dataset)
        .map_err(|err| setup_failure("SORTIN generation not cataloged", err))
}

/// STEP05R `SORTIN` [app/jcl/COMBTRAN.jcl:L23-26] — P-COMBTRAN-001: the records of
/// `generation`, one SORTIN of the concatenation, read one at a time with the `CVTRA05Y` layout
/// ([`RecordReader`]) and added to `sort` in file order. Returns the number of records read.
///
/// # Errors
///
/// - A setup failure (exit 16) when the file cannot be read or decoded. Both SORTIN datasets
///   hold `CVTRA05Y` records, so the text names the one that failed by its dataset, its
///   generation and its path before the cause, for example `SYSTRAN(0) SYSTRAN.G0001V00 at
///   var/batch/SYSTRAN.G0001V00: record file decode failed: CVTRA05Y record 1 …`. The cause is
///   the one `read_records` gives for the file. The same text is the `error` of the logged
///   event.
/// - The errors of [`Sort::push`].
fn read_sortin(generation: &Generation, sort: &mut Sort<'_>) -> Result<usize, ExitStatus> {
    let records = RecordReader::<TransactionLayout>::open(&generation.path)
        .map_err(|err| sortin_failure(generation, &err))?;
    let mut read = 0;
    for record in records {
        sort.push(record.map_err(|err| sortin_failure(generation, &err))?)?;
        read += 1;
    }
    Ok(read)
}

/// The setup failure (exit 16) of the STEP05R `SORTIN` `generation` [app/jcl/COMBTRAN.jcl:L23-26]
/// that cannot be read or decoded, `err` the cause: the text [`read_sortin`] describes, a
/// failure the COBOL has no counterpart for.
fn sortin_failure(generation: &Generation, err: &GenerationError) -> ExitStatus {
    setup_failure(
        "SORTIN read failed",
        format!(
            "{}(0) {generation} at {}: {err}",
            generation.dataset.name(),
            generation.path.display()
        ),
    )
}

/// The `STEP05R` sort in progress — P-COMBTRAN-001: the SORTIN records not yet in a sorted run,
/// each with its position in the concatenation, the records added so far and, once a chunk has
/// spilled, the sort work file. It holds at most `limits.chunk` records and keeps no list of the
/// runs it spills: every run but the last holds `limits.chunk` records, so [`Sort::finish`]
/// derives them from the records added ([`RunLayout`]).
struct Sort<'g> {
    /// The catalog whose directory holds the work files.
    generations: &'g Generations,
    /// The bounds of the sort.
    limits: SortLimits,
    /// The records not yet in a run, with their positions, in input order until sorted.
    chunk: Vec<(usize, TransactionRecord)>,
    /// The records added so far: the position of the next.
    records: usize,
    /// The work file, holding the runs in SORTIN order, once the first chunk has spilled.
    work: Option<WorkFileWriter>,
}

/// A sorted run on a sort work file: the [`RUN_FRAME`] frames of consecutive SORTIN records in
/// TRAN-ID order, from byte `start`, `len` bytes long. Computed from a [`RunLayout`] when a
/// merge opens the run, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    /// The offset of the run's first frame.
    start: u64,
    /// The bytes of the run's frames.
    len: u64,
}

/// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the sorted runs
/// on one sort work file, described by two numbers instead of a list, so the description does
/// not grow with the input. The file holds `records` [`RUN_FRAME`] frames from offset 0, in
/// runs of `run_records` consecutive frames each, the last run holding those left over: a spill
/// writes every chunk but the last full, and a merge of `fan_in` consecutive runs makes every
/// merged run but the last `fan_in` times as long ([`RunLayout::merged`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunLayout {
    /// The records of every run but the last: at least 1, and at most `records` once there is
    /// a record.
    run_records: usize,
    /// The records on the work file.
    records: usize,
}

impl RunLayout {
    /// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: `records`
    /// records in runs of `run_records`. Below 1 counts as 1, and more than `records` counts as
    /// `records`: one run holding them all.
    fn new(run_records: usize, records: usize) -> Self {
        Self {
            run_records: run_records.clamp(1, records.max(1)),
            records,
        }
    }

    /// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the number
    /// of runs, `records` divided by `run_records` and rounded up: 0 without records.
    fn count(self) -> usize {
        self.records.div_ceil(self.run_records.max(1))
    }

    /// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: run `index`,
    /// counted from 0: `run_records` frames from frame `index * run_records`, or the frames left
    /// for the last run. A run past the last is empty, at the end of the file.
    fn run(self, index: usize) -> Run {
        let first = index.saturating_mul(self.run_records).min(self.records);
        let frames = self.run_records.min(self.records.saturating_sub(first));
        Run {
            start: frame_bytes(first),
            len: frame_bytes(frames),
        }
    }

    /// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the runs
    /// numbered `indexes`, in order, each computed as it is taken ([`RunLayout::run`]).
    fn runs(self, indexes: Range<usize>) -> impl Iterator<Item = Run> {
        indexes.map(move |index| self.run(index))
    }

    /// STEP05R `SORT` intermediate merges [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the
    /// runs once each group of `fan_in` consecutive runs is merged into one, as [`cascade`]
    /// writes them to the next work file: `fan_in` times as long, and one run when that would
    /// exceed the records.
    fn merged(self, fan_in: usize) -> Self {
        Self::new(self.run_records.saturating_mul(fan_in), self.records)
    }

    /// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the bytes of
    /// every run, which the work file holds and nothing more.
    fn bytes(self) -> u64 {
        frame_bytes(self.records)
    }
}

/// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the bytes of
/// `frames` [`RUN_FRAME`] frames on a sort work file. No work file reaches `u64::MAX` bytes; the
/// count saturates there rather than fail.
fn frame_bytes(frames: usize) -> u64 {
    let frame = u64::try_from(RUN_FRAME).unwrap_or(u64::MAX);
    u64::try_from(frames).map_or(u64::MAX, |frames| frames.saturating_mul(frame))
}

impl<'g> Sort<'g> {
    /// STEP05R `PGM=SORT` [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: an empty sort within
    /// `limits`, whose work files `generations` creates.
    const fn new(generations: &'g Generations, limits: SortLimits) -> Self {
        Self {
            generations,
            limits,
            chunk: Vec::new(),
            records: 0,
            work: None,
        }
    }

    /// STEP05R `SORTIN` [app/jcl/COMBTRAN.jcl:L23-26] — P-COMBTRAN-001: adds `record`, the next
    /// record of the SORTIN concatenation, at the next position. A full chunk is first written
    /// to the work file as a sorted run ([`Sort::spill`]).
    ///
    /// # Errors
    ///
    /// The errors of [`Sort::spill`].
    fn push(&mut self, record: TransactionRecord) -> Result<(), ExitStatus> {
        if self.chunk.len() >= self.limits.chunk.max(1) {
            self.spill()?;
        }
        self.chunk.push((self.records, record));
        self.records += 1;
        Ok(())
    }

    /// STEP05R `SORT FIELDS=(TRAN-ID,A)` [app/jcl/COMBTRAN.jcl:L28-30] — P-COMBTRAN-001: sorts
    /// the chunk ([`sort_chunk`]) and appends it to the work file, created by the first spill,
    /// as the next run, which leaves the chunk empty.
    ///
    /// # Errors
    ///
    /// A setup failure (exit 16) when the work file cannot be created or written:
    /// `SORTWK write failed`.
    fn spill(&mut self) -> Result<(), ExitStatus> {
        sort_chunk(&mut self.chunk);
        let work = match self.work.take() {
            Some(work) => work,
            None => create_work_file(self.generations)?,
        };
        let work = self.work.insert(work);
        for (position, record) in self.chunk.drain(..) {
            write_frame(work, position, &record)?;
        }
        Ok(())
    }

    /// STEP05R `SORT FIELDS=(TRAN-ID,A)` [app/jcl/COMBTRAN.jcl:L28-30] — P-COMBTRAN-001: the end
    /// of the input. An input that fits one chunk is sorted in memory; otherwise the last chunk
    /// is spilled too and the runs are merged down to at most `limits.fan_in` ([`cascade`]).
    /// [`Sort::push`] spills a chunk only once it holds `limits.chunk` records, so every run but
    /// the last holds that many, and the runs are the [`RunLayout`] of `limits.chunk` and the
    /// records added.
    ///
    /// # Errors
    ///
    /// The errors of [`Sort::spill`], [`finish_work`] and [`cascade`].
    fn finish(mut self) -> Result<Sorted, ExitStatus> {
        if self.work.is_some() && !self.chunk.is_empty() {
            self.spill()?;
        }
        match self.work {
            None => {
                sort_chunk(&mut self.chunk);
                Ok(Sorted::InMemory(self.chunk))
            }
            Some(work) => {
                let layout = RunLayout::new(self.limits.chunk.max(1), self.records);
                let work = finish_work(work, layout)?;
                cascade(self.generations, work, layout, self.limits.fan_in.max(2))
            }
        }
    }
}

/// `SORT FIELDS=(TRAN-ID,A)` with `TRAN-ID,1,16,CH` [app/jcl/COMBTRAN.jcl:L28-30] —
/// P-COMBTRAN-001, P-COMBTRAN-009: one chunk of the SORT input in ascending order of its 16
/// TRAN-ID bytes, `CH` being the unsigned byte order. Records with one key keep their input
/// order: each position is unique, so ordering by key and then position is the stable sort by
/// key, done in place.
fn sort_chunk(chunk: &mut [(usize, TransactionRecord)]) {
    chunk.sort_unstable_by_key(|(position, record)| {
        (*record.transaction.tran_id.as_bytes(), *position)
    });
}

/// STEP05R `SORT` intermediate merges [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: while
/// `layout` gives `work` more runs than `fan_in`, at least 2, merges each group of `fan_in`
/// consecutive runs ([`Merge`]), taken by their numbers from `layout`, into one run on a new
/// work file, which then replaces `work`, deleting it; the runs on the new file are
/// [`RunLayout::merged`]. Only the runs of the group being merged are open, so the cascade
/// holds at most `fan_in` run readers and two layouts, however many runs there are. Runs of
/// consecutive SORTIN records merged in run order stay runs of consecutive records, so every
/// merge keeps records with one key in position order.
///
/// # Errors
///
/// A setup failure (exit 16) when a work file cannot be created, written or read back:
/// `SORTWK write failed` or `SORTWK read failed`. The errors of [`finish_work`].
fn cascade(
    generations: &Generations,
    mut work: WorkFile,
    mut layout: RunLayout,
    fan_in: usize,
) -> Result<Sorted, ExitStatus> {
    let fan_in = fan_in.max(2);
    while layout.count() > fan_in {
        let mut next = create_work_file(generations)?;
        let runs = layout.count();
        for first in (0..runs).step_by(fan_in) {
            let group = first..first.saturating_add(fan_in).min(runs);
            let mut merge = Merge::open(&work, layout.runs(group))?;
            while let Some((position, record)) = merge.next_record()? {
                write_frame(&mut next, position, &record)?;
            }
        }
        layout = layout.merged(fan_in);
        work = finish_work(next, layout)?;
    }
    Ok(Sorted::Spilled { work, layout })
}

/// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: ends the
/// writing of `work`, whose runs `layout` gives, and returns the file to read them back from.
/// The runs are computed from `layout`, not recorded as they are written, so the file must hold
/// exactly the [`RUN_FRAME`] frames of its records.
///
/// # Errors
///
/// A setup failure (exit 16) when the work file holds other than those bytes, or cannot be
/// flushed: `SORTWK write failed`.
fn finish_work(work: WorkFileWriter, layout: RunLayout) -> Result<WorkFile, ExitStatus> {
    let (written, expected) = (work.offset(), layout.bytes());
    if written != expected {
        return Err(setup_failure(
            "SORTWK write failed",
            format!(
                "the sort work file holds {written} bytes, not the {expected} bytes of {} \
                 {RUN_FRAME}-byte sorted-run frames",
                layout.records
            ),
        ));
    }
    work.finish().map_err(work_write_failure)
}

/// The SORT input in TRAN-ID order, ready for the final merge: the one chunk in memory, or at
/// most the fan-in of sorted runs on a sort work file, in SORTIN order, which a [`RunLayout`]
/// describes.
enum Sorted {
    /// The whole input, sorted, each record with its position.
    InMemory(Vec<(usize, TransactionRecord)>),
    /// The work file and the layout of its runs.
    Spilled {
        /// The work file, deleted when the value is dropped.
        work: WorkFile,
        /// Its runs, in SORTIN order: at most the fan-in.
        layout: RunLayout,
    },
}

impl Sorted {
    /// STEP05R `SORT` final merge [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: a new pass
    /// over the records in TRAN-ID order, from the first. Each call opens every run of the
    /// layout from its first frame, so the records are passed over as often as the step needs.
    ///
    /// # Errors
    ///
    /// A setup failure (exit 16) when the work file cannot be read back: `SORTWK read failed`.
    fn pass(&self) -> Result<Pass<'_>, ExitStatus> {
        match self {
            Self::InMemory(records) => Ok(Pass::InMemory(records.iter())),
            Self::Spilled { work, layout } => {
                Merge::open(work, layout.runs(0..layout.count())).map(Pass::Merge)
            }
        }
    }
}

/// One pass of [`Sorted::pass`]: over the records in memory, or the merge of the runs.
enum Pass<'a> {
    /// The records in memory, already in order.
    InMemory(slice::Iter<'a, (usize, TransactionRecord)>),
    /// The runs being merged.
    Merge(Merge<'a>),
}

impl Pass<'_> {
    /// STEP05R `SORT FIELDS=(TRAN-ID,A)` [app/jcl/COMBTRAN.jcl:L28-30] — P-COMBTRAN-001: the
    /// next record in TRAN-ID order with its SORTIN position: `None` after the last.
    ///
    /// # Errors
    ///
    /// The errors of [`Merge::next_record`].
    fn next_record(&mut self) -> Result<Option<(usize, TransactionRecord)>, ExitStatus> {
        match self {
            Self::InMemory(records) => Ok(records.next().cloned()),
            Self::Merge(merge) => merge.next_record(),
        }
    }
}

/// STEP05R `SORT` merge [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: the merge of sorted
/// runs of one work file. The next record is the head with the lowest TRAN-ID bytes and, among
/// equal keys, of the earliest run, so records with one key come in run order.
struct Merge<'a> {
    /// The runs, in SORTIN order, each with its next record.
    runs: Vec<RunHead<'a>>,
}

/// One run being merged: its reader and the record it yields next, `None` once read whole.
struct RunHead<'a> {
    /// The run's frames on the work file.
    reader: WorkReader<'a>,
    /// The next record of the run, with its SORTIN position.
    head: Option<(usize, TransactionRecord)>,
}

impl<'a> Merge<'a> {
    /// STEP05R `SORT` merge [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: opens `runs` of
    /// `work`, in SORTIN order and at most the fan-in of them, from their first frames.
    ///
    /// # Errors
    ///
    /// The errors of [`read_frame`].
    fn open(work: &'a WorkFile, runs: impl IntoIterator<Item = Run>) -> Result<Self, ExitStatus> {
        let runs = runs
            .into_iter()
            .map(|run| {
                let mut reader = work.reader(run.start, run.len);
                let head = read_frame(&mut reader)?;
                Ok(RunHead { reader, head })
            })
            .collect::<Result<Vec<_>, ExitStatus>>()?;
        Ok(Self { runs })
    }

    /// STEP05R `SORT FIELDS=(TRAN-ID,A)` [app/jcl/COMBTRAN.jcl:L28-30] — P-COMBTRAN-001,
    /// P-COMBTRAN-009: the next record by (TRAN-ID bytes, run index), with its SORTIN position:
    /// `None` once every run is read whole.
    ///
    /// # Errors
    ///
    /// The errors of [`read_frame`].
    fn next_record(&mut self) -> Result<Option<(usize, TransactionRecord)>, ExitStatus> {
        let lowest = self
            .runs
            .iter()
            .enumerate()
            .filter_map(|(index, run)| {
                run.head
                    .as_ref()
                    .map(|(_, record)| (*record.transaction.tran_id.as_bytes(), index))
            })
            .min();
        let Some((_, index)) = lowest else {
            return Ok(None);
        };
        let run = &mut self.runs[index];
        let next = read_frame(&mut run.reader)?;
        Ok(mem::replace(&mut run.head, next))
    }
}

/// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: creates a sort
/// work file in the catalog directory of `generations`, as DFSORT allocates `SORTWK` for a step
/// whose input does not fit its memory.
///
/// # Errors
///
/// A setup failure (exit 16) when it cannot be created: `SORTWK write failed`.
fn create_work_file(generations: &Generations) -> Result<WorkFileWriter, ExitStatus> {
    generations.create_work_file().map_err(work_write_failure)
}

/// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: writes the
/// [`RUN_FRAME`] frame of `record`, at SORTIN `position`, to `work`: the position as a
/// big-endian `u64`, then the 350-byte `CVTRA05Y` record as `TRANSACT.COMBINED(+1)` holds it.
///
/// # Errors
///
/// A setup failure (exit 16) when the work file cannot be written: `SORTWK write failed`.
fn write_frame(
    work: &mut WorkFileWriter,
    position: usize,
    record: &TransactionRecord,
) -> Result<(), ExitStatus> {
    let position = u64::try_from(position).map_err(|err| {
        setup_failure(
            "SORTWK write failed",
            format!("SORTIN position {position}: {err}"),
        )
    })?;
    work.write(&position.to_be_bytes())
        .and_then(|()| work.write(&TransactionLayout::encode(record, Charset::Ascii)))
        .map_err(work_write_failure)
}

/// STEP05R `SORT` work dataset [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001: reads the next
/// frame of a sorted run from `reader`: the SORTIN position and the record, as [`write_frame`]
/// wrote them, or `None` after the run's last frame.
///
/// # Errors
///
/// A setup failure (exit 16) when the work file cannot be read, ends inside a frame, or holds a
/// frame that does not read back as written: `SORTWK read failed`, with the work file's path.
fn read_frame(
    reader: &mut WorkReader<'_>,
) -> Result<Option<(usize, TransactionRecord)>, ExitStatus> {
    let mut frame = [0_u8; RUN_FRAME];
    if !reader.read_frame(&mut frame).map_err(work_read_failure)? {
        return Ok(None);
    }
    let invalid = |what: String| work_read_failure(reader.invalid_data(what));
    let Some((position, record)) = frame.split_first_chunk::<POSITION_LEN>() else {
        return Err(invalid(format!(
            "a sorted-run frame is not {RUN_FRAME} bytes"
        )));
    };
    let position = usize::try_from(u64::from_be_bytes(*position))
        .map_err(|err| invalid(format!("sorted-run position: {err}")))?;
    let record = TransactionLayout::decode(record, Charset::Ascii)
        .map_err(|err| invalid(format!("sorted-run record: {err}")))?;
    Ok(Some((position, record)))
}

/// The setup failure (exit 16) of a STEP05R sort work file [app/jcl/COMBTRAN.jcl:L22-37] that
/// cannot be created or written, `err` the I/O error on its path: a failure the COBOL has no
/// counterpart for.
fn work_write_failure(err: GenerationError) -> ExitStatus {
    setup_failure("SORTWK write failed", err)
}

/// The setup failure (exit 16) of a STEP05R sort work file [app/jcl/COMBTRAN.jcl:L22-37] that
/// cannot be read back, `err` the I/O error on its path: a failure the COBOL has no counterpart
/// for.
fn work_read_failure(err: GenerationError) -> ExitStatus {
    setup_failure("SORTWK read failed", err)
}

/// Two different records of the SORT input with one TRAN-ID, by their positions in the SORTIN
/// concatenation, counted from 0, `first` before `second`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EqualKeyConflict {
    tran_id: TranId,
    first: usize,
    second: usize,
}

/// `SORT FIELDS=(TRAN-ID,A)` without `EQUALS` [app/jcl/COMBTRAN.jcl:L28-30] — P-COMBTRAN-001,
/// P-COMBTRAN-009: the equal keys of `sorted`, the SORTIN concatenation `sortin` in TRAN-ID
/// order, checked by one pass of the final merge before `TRANSACT.COMBINED(+1)` is allocated
/// ([`first_conflict`]).
///
/// The SORT names no `EQUALS`, and the installation default acts as `NOEQUALS` for fixed-length
/// records, so the audit gives records with equal keys no order (E-38). Equal-key records that
/// are identical give the same output in any order and are kept, in input order.
///
/// # Errors
///
/// - An equivalence halt (exit 16, E-38) for the first [`EqualKeyConflict`] in key order: two
///   records with one TRAN-ID that differ in any byte. Their order, and with it the record
///   `REPRO` loads, is what the COBOL leaves undefined, so no order is chosen for them. The
///   halt is logged at `error` and nothing is allocated or written.
/// - A setup failure (exit 16) when a sort work file cannot be read back.
fn check_equal_keys(sorted: &Sorted, sortin: &SortIn) -> Result<(), ExitStatus> {
    let Some(conflict) = first_conflict(sorted)? else {
        return Ok(());
    };
    let halt = EqualKeyHalt {
        conflict,
        backup: &sortin.backup,
        backup_records: sortin.backup_records,
        systran: &sortin.systran,
    };
    tracing::error!(
        tran_id = %conflict.tran_id,
        escalation = EQUAL_KEY_ESCALATION,
        "{halt}"
    );
    Err(ExitStatus::equivalence_halt(&halt, EQUAL_KEY_ESCALATION))
}

/// `SORT FIELDS=(TRAN-ID,A)` [app/jcl/COMBTRAN.jcl:L28-30] — P-COMBTRAN-009: the first two
/// neighbours in the TRAN-ID order of `sorted` that share a TRAN-ID and differ, by their
/// positions in the SORTIN concatenation, or `None`.
///
/// # Errors
///
/// A setup failure (exit 16) when a sort work file cannot be read back.
fn first_conflict(sorted: &Sorted) -> Result<Option<EqualKeyConflict>, ExitStatus> {
    let mut pass = sorted.pass()?;
    let Some((mut first, mut earlier)) = pass.next_record()? else {
        return Ok(None);
    };
    while let Some((second, later)) = pass.next_record()? {
        if earlier.transaction.tran_id == later.transaction.tran_id && earlier != later {
            return Ok(Some(EqualKeyConflict {
                tran_id: earlier.transaction.tran_id,
                first,
                second,
            }));
        }
        (first, earlier) = (second, later);
    }
    Ok(None)
}

/// The report of an [`EqualKeyConflict`], naming each record by its generation and its record
/// number there.
struct EqualKeyHalt<'a> {
    conflict: EqualKeyConflict,
    backup: &'a Generation,
    backup_records: usize,
    systran: &'a Generation,
}

impl EqualKeyHalt<'_> {
    /// The SORTIN record at concatenation `position`, for example
    /// `SYSTRAN.G0002V00 record 7`.
    fn locate(&self, position: usize) -> String {
        match position.checked_sub(self.backup_records) {
            None => format!("{} record {}", self.backup, position + 1),
            Some(offset) => format!("{} record {}", self.systran, offset + 1),
        }
    }
}

impl fmt::Display for EqualKeyHalt<'_> {
    /// Two lines: the conflict, then why it halts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "TRANSACT.COMBINED(+1) not written: TRAN-ID {} keys two different records, {} and {}",
            self.conflict.tran_id,
            self.locate(self.conflict.first),
            self.locate(self.conflict.second)
        )?;
        f.write_str(
            "SORT FIELDS=(TRAN-ID,A) without EQUALS leaves their order undefined, and with it the \
             record REPRO loads",
        )
    }
}

/// STEP10 `PGM=IDCAMS` `REPRO INFILE(TRANSACT) OUTFILE(TRANVSAM)` [app/jcl/COMBTRAN.jcl:L41-48] —
/// P-COMBTRAN-002, P-COMBTRAN-003, P-COMBTRAN-005 to P-COMBTRAN-008.
///
/// Reads `TRANSACT.COMBINED(+1)`, the `TRANSACT` DD of the step, from the file `STEP05R`
/// cataloged, twice, on the blocking pool and never whole. The first pass reads and decodes
/// every record, holding one at a time, and counts them ([`count_records_async`]). The second
/// loads the records into the live master in file order by [`load_records`], inside one unit
/// of work, [`RECORD_BATCH`] records at a time, and the step commits. Once the load is
/// committed, [`finish`] sets `posted` in `counts` to the loaded records and `rejected` to the
/// duplicate records, and the step ends with the `REPRO` condition code:
///
/// - 0 when every record loaded, an empty file included (P-COMBTRAN-002, P-COMBTRAN-005);
/// - 8 after one to three duplicate records (P-COMBTRAN-003, P-COMBTRAN-008);
/// - 12 when the fourth duplicate record stops the load (P-COMBTRAN-003, P-COMBTRAN-006,
///   P-COMBTRAN-007).
///
/// A setup failure (exit 16) ends the step, through [`not_committed`], when the file cannot be
/// read or decoded or the unit of work cannot begin, before any record is loaded; when a record
/// cannot be loaded, or the file cannot be read again during the load, after which the load is
/// rolled back; or when the commit fails. Each leaves the cataloged `TRANSACT.COMBINED(+1)` in
/// place and the live master as the run found it, and leaves `posted` 0 and `rejected` the
/// duplicate records met before the failure.
async fn repro_step(db: &Db, combined: &Generation, counts: &JobCounts) -> ExitStatus {
    let mut tally = ReproTally::default();
    let read = match count_records_async::<TransactionLayout>(combined.path.clone()).await {
        Ok(read) => read,
        Err(err) => return not_committed(tally, counts, combined_read_failure(err)),
    };
    let mut uow = match db.begin().await {
        Ok(uow) => uow,
        Err(err) => {
            return not_committed(
                tally,
                counts,
                setup_failure("live transaction master load failed", err),
            );
        }
    };
    if let Err(status) = load_records(&mut uow, combined, &mut tally).await {
        if let Err(err) = uow.rollback().await {
            tracing::error!(error = %err, "live transaction master load not rolled back");
        }
        return not_committed(tally, counts, status);
    }
    if let Err(err) = uow.commit().await {
        return not_committed(
            tally,
            counts,
            setup_failure("live transaction master load failed", err),
        );
    }
    finish(tally, read, counts)
}

/// The setup failure (exit 16) of STEP10's `TRANSACT` DD, `TRANSACT.COMBINED(+1)`
/// [app/jcl/COMBTRAN.jcl:L43-44], that cannot be read or decoded, `err` the cause: a failure the
/// COBOL has no counterpart for.
fn combined_read_failure(err: GenerationError) -> ExitStatus {
    setup_failure("TRANSACT.COMBINED(+1) read failed", err)
}

/// The `REPRO` record loop of STEP10 [app/jcl/COMBTRAN.jcl:L48] — P-COMBTRAN-003: each record of
/// `combined`, in file order, through `PgStore::load_combined_record`, counted in `tally`, until
/// [`ReproTally::count`] stops the load at the error limit. The file is read
/// [`RECORD_BATCH`] records at a time on the blocking pool ([`RecordReader::next_batch_async`]),
/// and record numbers run on across the batches.
///
/// A duplicate record emits one `IDC3308I DUPLICATE RECORD` warning with its record number and
/// TRAN-ID, and the stop one `IDC31467I MAXIMUM ERROR LIMIT REACHED` warning.
///
/// # Errors
///
/// - A setup failure (exit 16) naming the record number and TRAN-ID of the first record whose
///   load fails. The audit records no condition code for it. `tally` then holds the records
///   counted before that one.
/// - A setup failure (exit 16) when the file cannot be read or decoded, which [`repro_step`]
///   has already checked: `TRANSACT.COMBINED(+1) read failed`. `tally` then holds the records
///   counted before the batch that failed.
async fn load_records(
    uow: &mut UnitOfWork,
    combined: &Generation,
    tally: &mut ReproTally,
) -> Result<(), ExitStatus> {
    let mut reader = RecordReader::<TransactionLayout>::open_async(combined.path.clone())
        .await
        .map_err(combined_read_failure)?;
    let mut record_number: usize = 0;
    loop {
        let (returned, batch) = reader
            .next_batch_async(RECORD_BATCH)
            .await
            .map_err(combined_read_failure)?;
        reader = returned;
        let last = batch.len() < RECORD_BATCH;
        for record in batch {
            record_number += 1;
            let tran_id = record.transaction.tran_id;
            let outcome = uow
                .store()
                .load_combined_record(record.transaction)
                .await
                .map_err(|err| {
                    setup_failure(
                        "live transaction master load failed",
                        format!(
                            "TRANSACT.COMBINED(+1) record {record_number} TRAN-ID {tran_id}: \
                             {err}"
                        ),
                    )
                })?;
            if outcome == LoadOutcome::DuplicateLive {
                tracing::warn!(record_number, tran_id = %tran_id, "IDC3308I DUPLICATE RECORD");
            }
            if tally.count(outcome).is_break() {
                tracing::warn!(
                    record_number,
                    error_limit = ERROR_LIMIT,
                    "IDC31467I MAXIMUM ERROR LIMIT REACHED"
                );
                return Ok(());
            }
        }
        if last {
            return Ok(());
        }
    }
}

/// The end of STEP10 once the load is committed, `read` combined records having been read: the
/// loaded and duplicate records in `counts`, one end event, and the condition code as the job's
/// exit status.
fn finish(tally: ReproTally, read: usize, counts: &JobCounts) -> ExitStatus {
    let condition_code = tally.condition_code();
    let loaded = tally.loaded();
    tally.set_counts(counts, loaded);
    tracing::info!(
        read,
        inserted = tally.inserted,
        revived = tally.revived,
        duplicates = tally.duplicates,
        not_attempted = read.saturating_sub(tally.attempted()),
        condition_code,
        "IDC0005I NUMBER OF RECORDS PROCESSED WAS {loaded}"
    );
    ExitStatus::ReproCondition(condition_code)
}

/// The end of STEP10 when the load is not committed, a failure the COBOL has no counterpart
/// for: `counts` holds `posted` 0, since the live master keeps none of the load, and in
/// `rejected` the duplicate records met before the failure. One `warn` event reports the
/// records the uncommitted load handled (`inserted`, `revived`, `duplicates` and their sum
/// `attempted`) beside `posted` 0. Returns `status`, the failure that ended the step.
fn not_committed(tally: ReproTally, counts: &JobCounts, status: ExitStatus) -> ExitStatus {
    const POSTED: usize = 0;
    tally.set_counts(counts, POSTED);
    tracing::warn!(
        inserted = tally.inserted,
        revived = tally.revived,
        duplicates = tally.duplicates,
        attempted = tally.attempted(),
        posted = POSTED,
        "live transaction master load not committed: {POSTED} records posted"
    );
    status
}

/// The record accounting of one `REPRO` run — P-COMBTRAN-003: what each record's load did, and
/// from that when the run stops and its condition code.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ReproTally {
    /// Records inserted as new live rows.
    inserted: usize,
    /// Records that made a row outside the live master live again.
    revived: usize,
    /// Records not loaded because their TRAN-ID was live: the `REPRO` errors.
    duplicates: usize,
}

impl ReproTally {
    /// Counts `outcome`. Breaks when it is the duplicate that reaches [`ERROR_LIMIT`]: `REPRO`
    /// stops there and processes no later record.
    fn count(&mut self, outcome: LoadOutcome) -> ControlFlow<()> {
        match outcome {
            LoadOutcome::Inserted(_) => self.inserted += 1,
            LoadOutcome::Revived(_) => self.revived += 1,
            LoadOutcome::DuplicateLive => self.duplicates += 1,
        }
        if self.limit_reached() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    /// Sets `posted` in `counts` to `posted`, the records the live master keeps from the load,
    /// and `rejected` to the duplicate records.
    fn set_counts(&self, counts: &JobCounts, posted: usize) {
        counts.set_posted(count_of(posted));
        counts.set_rejected(count_of(self.duplicates));
    }

    /// The records written to the live master: inserted or revived.
    const fn loaded(&self) -> usize {
        self.inserted + self.revived
    }

    /// The records `REPRO` processed, loaded or not.
    const fn attempted(&self) -> usize {
        self.loaded() + self.duplicates
    }

    /// Whether the duplicates reached the error limit.
    const fn limit_reached(&self) -> bool {
        self.duplicates >= ERROR_LIMIT
    }

    /// The `REPRO` condition code: 12 at the error limit, 8 after fewer duplicates, else 0.
    const fn condition_code(&self) -> u8 {
        if self.limit_reached() {
            ERROR_LIMIT_CONDITION
        } else if self.duplicates > 0 {
            DUPLICATE_CONDITION
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;
    use std::slice;

    use carddemo_codec::layouts::TransactionLayout;
    use carddemo_codec::{Charset, Layout};
    use carddemo_domain::clock::{AnyClock, SystemClock};
    use carddemo_domain::numeric::TransactionRowId;
    use carddemo_persistence::repos::LoadOutcome;
    use carddemo_runtime::config::Settings;

    use super::{
        RUN_FRAME, ReproTally, Run, RunLayout, SORT_LIMITS, SortLimits, finish, not_committed,
        sort_step, sort_step_within,
    };
    use crate::cli::Command;
    use crate::exit::ExitStatus;
    use crate::generations::{Dataset, Generation, read_records};
    use crate::jobs::tests::{has_member, job_span_line};
    use crate::jobs::{JobContext, JobCounts, count_of};

    /// A 350-byte `CVTRA05Y` record with `tran_id` and `desc`, the other fields fixed and
    /// FILLER spaces: `TRAN-AMT` `0000001000{` is +100.00.
    fn record(tran_id: &str, desc: &str) -> String {
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

    /// The 350-byte `CVTRA05Y` interest record INTCALC writes for account 1 under `tran_id`, the
    /// PARM-DATE and its six-digit suffix: type 01, category 0005, source `System`, +1.50, no
    /// merchant, the account's card and both timestamps of the processing clock.
    fn interest_record(tran_id: &str) -> String {
        let record = format!(
            "{tran_id:<16}010005{source:<10}{desc:<100}0000000015{{000000000{blank:<110}\
             9680294154603697{ts}{ts}{blank:<20}",
            source = "System",
            desc = "Int. for a/c 00000000001",
            ts = "2022-07-01-12.00.00.000000",
            blank = "",
        );
        assert_eq!(record.len(), 350, "{record:?}");
        record
    }

    /// The bytes of a record file holding `records`, each followed by `LF`.
    fn file_of(records: &[&String]) -> String {
        let mut file = String::new();
        for record in records {
            file.push_str(record);
            file.push('\n');
        }
        file
    }

    /// A batch directory of its own for one test, removed when the test ends.
    struct BatchDir(PathBuf);

    impl BatchDir {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("carddemo-combtran-{test}-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
            Self(dir)
        }

        /// Catalogs the generation file `name` holding `records`.
        fn write(&self, name: &str, records: &[&String]) {
            let path = self.0.join(name);
            fs::write(&path, file_of(records))
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        }

        fn context(&self) -> JobContext {
            let settings = Settings {
                batch_dir: self.0.clone(),
                ..Settings::default()
            };
            JobContext::new(settings, AnyClock::System(SystemClock))
        }

        /// The names of the files in the directory, sorted.
        fn files(&self) -> Vec<String> {
            let entries =
                fs::read_dir(&self.0).unwrap_or_else(|e| panic!("list {}: {e}", self.0.display()));
            let mut names: Vec<String> = entries
                .map(|entry| {
                    let entry = entry.unwrap_or_else(|e| panic!("list {}: {e}", self.0.display()));
                    entry.file_name().to_string_lossy().into_owned()
                })
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for BatchDir {
        /// Removes the directory. A directory that is already gone is not a failure. Any
        /// other removal failure fails the test with the path and the error; while the test
        /// is already panicking it is only reported, because a second panic aborts the run.
        fn drop(&mut self) {
            let Err(error) = fs::remove_dir_all(&self.0) else {
                return;
            };
            if error.kind() == ErrorKind::NotFound {
                return;
            }
            if std::thread::panicking() {
                eprintln!("remove {}: {error}", self.0.display());
            } else {
                panic!("remove {}: {error}", self.0.display());
            }
        }
    }

    fn sorted(dir: &BatchDir) -> Generation {
        sort_step(&dir.context().generations, &JobCounts::default())
            .unwrap_or_else(|status| panic!("{status:?}"))
    }

    fn read(generation: &Generation) -> String {
        fs::read_to_string(&generation.path)
            .unwrap_or_else(|e| panic!("read {}: {e}", generation.path.display()))
    }

    #[test]
    fn sort_step_writes_the_concatenation_in_tran_id_byte_order() {
        let dir = BatchDir::new("order");
        // `CH` is byte order, not numeric order: a space sorts before every digit, a letter
        // after them.
        let backup = [
            record("0000000000000005", "BACKUP 5"),
            record("1234", "BACKUP 1234"),
            record("A000000000000000", "BACKUP A"),
        ];
        let systran = [
            record("2022071800000001", "INTEREST 1"),
            record("1234000000000000", "INTEREST 1234"),
            record("0000000000000002", "INTEREST 2"),
        ];
        dir.write("TRANSACT.BKUP.G0003V00", &backup.iter().collect::<Vec<_>>());
        dir.write("SYSTRAN.G0001V00", &systran.iter().collect::<Vec<_>>());
        let combined = sorted(&dir);
        assert_eq!(
            (combined.dataset, combined.number),
            (Dataset::TransactCombined, 1)
        );
        let expected = [
            &systran[2],
            &backup[0],
            &backup[1],
            &systran[1],
            &systran[0],
            &backup[2],
        ];
        assert_eq!(read(&combined), file_of(&expected));
    }

    #[test]
    fn an_interest_tran_id_holding_lf_sorts_and_reads_back_unchanged() {
        let dir = BatchDir::new("lf-tran-id");
        let backup = [
            record("0000000000000005", "BACKUP 5"),
            record("2022071800000001", "BACKUP 2022071800000001"),
        ];
        dir.write("TRANSACT.BKUP.G0001V00", &backup.iter().collect::<Vec<_>>());
        // `batch intcalc --parm-date $'2022\n71800'`: `PARM-DATE X(10)` copies the LF into the
        // TRAN-ID, and SYSTRAN(+1) is written through the shared record writer.
        let interest = interest_record("2022\n71800000001");
        let decode = |text: &str| {
            TransactionLayout::decode(text.as_bytes(), Charset::Ascii)
                .unwrap_or_else(|e| panic!("{text:?}: {e}"))
        };
        let interest_tran = decode(&interest);
        let mut systran = dir
            .context()
            .generations
            .begin_next(Dataset::SysTran)
            .unwrap_or_else(|e| panic!("SYSTRAN(+1) allocation: {e}"));
        systran
            .append_records::<TransactionLayout>(slice::from_ref(&interest_tran))
            .unwrap_or_else(|e| panic!("SYSTRAN(+1) write: {e}"));
        let systran = systran
            .catalog()
            .unwrap_or_else(|e| panic!("SYSTRAN(+1) catalog: {e}"));
        assert_eq!(read(&systran), file_of(&[&interest]));

        let combined = sorted(&dir);
        // `CH` byte order: LF (0x0A) sorts before the digit 0 (0x30).
        assert_eq!(
            read(&combined),
            file_of(&[&backup[0], &interest, &backup[1]])
        );
        assert_eq!(
            read_records::<TransactionLayout>(&combined.path)
                .unwrap_or_else(|e| panic!("{}: {e}", combined.path.display())),
            [decode(&backup[0]), interest_tran, decode(&backup[1])]
        );
    }

    #[test]
    fn identical_records_with_one_tran_id_are_both_written() {
        let dir = BatchDir::new("identical");
        let twice = record("0000000000000007", "SAME CONTENT");
        let other = record("0000000000000003", "OTHER");
        dir.write("TRANSACT.BKUP.G0001V00", &[&twice]);
        dir.write("SYSTRAN.G0001V00", &[&twice, &other]);
        assert_eq!(read(&sorted(&dir)), file_of(&[&other, &twice, &twice]));
    }

    #[test]
    fn different_records_with_one_tran_id_halt_before_any_output() {
        let dir = BatchDir::new("halt");
        let first = record("0000000000000001", "BACKUP 1");
        let backed_up = record("0000000000000009", "BACKUP 9");
        let interest = record("0000000000000009", "INTEREST 9");
        dir.write("TRANSACT.BKUP.G0002V00", &[&first, &backed_up]);
        dir.write("SYSTRAN.G0004V00", &[&interest]);
        let status = sort_step(&dir.context().generations, &JobCounts::default()).err();
        let expected = ExitStatus::EquivalenceHalt {
            lines: vec![
                "TRANSACT.COMBINED(+1) not written: TRAN-ID 0000000000000009 keys two different \
                 records, TRANSACT.BKUP.G0002V00 record 2 and SYSTRAN.G0004V00 record 1"
                    .to_owned(),
                "SORT FIELDS=(TRAN-ID,A) without EQUALS leaves their order undefined, and with it \
                 the record REPRO loads"
                    .to_owned(),
            ],
            escalation_id: "E-38".to_owned(),
        };
        assert_eq!(status.as_ref(), Some(&expected));
        assert_eq!(expected.code(), 16);
        assert_eq!(dir.files(), ["SYSTRAN.G0004V00", "TRANSACT.BKUP.G0002V00"]);
    }

    #[test]
    fn an_uncataloged_sortin_generation_is_a_setup_failure() {
        let dir = BatchDir::new("missing");
        dir.write(
            "TRANSACT.BKUP.G0001V00",
            &[&record("0000000000000001", "BACKUP")],
        );
        assert_eq!(
            sort_step(&dir.context().generations, &JobCounts::default()).err(),
            Some(ExitStatus::SetupFailure(
                "SYSTRAN(0): no generation is cataloged".to_owned()
            ))
        );
        assert_eq!(dir.files(), ["TRANSACT.BKUP.G0001V00"]);
    }

    /// The 350-byte record `record(tran_id, desc)` with `00000XX145F` over `TRAN-AMT`, bytes
    /// 132 to 142: a non-digit `X` in the numeric field.
    fn bad_amount(tran_id: &str, desc: &str) -> String {
        let mut bytes = record(tran_id, desc).into_bytes();
        bytes[132..143].copy_from_slice(b"00000XX145F");
        String::from_utf8(bytes).unwrap_or_else(|e| panic!("ASCII record: {e}"))
    }

    #[test]
    fn a_sortin_that_cannot_be_decoded_is_named_by_dataset_generation_and_path() {
        let good = record("0000000000000001", "GOOD");
        // (fault, bytes of the corrupt generation file)
        let faults = [
            (
                "amount",
                format!("{}\n", bad_amount("0000000000000001", "BAD")),
            ),
            ("truncated", format!("{good}\n{}", &good[..341])),
            ("crlf", format!("{good}\r\n")),
        ];
        // (dataset, the corrupt generation, the sound one)
        let datasets = [
            (
                "TRANSACT.BKUP",
                "TRANSACT.BKUP.G0002V00",
                "SYSTRAN.G0001V00",
            ),
            ("SYSTRAN", "SYSTRAN.G0001V00", "TRANSACT.BKUP.G0002V00"),
        ];
        let mut texts = BTreeSet::new();
        for (dataset, corrupt, sound) in datasets {
            for (fault, bytes) in &faults {
                let dir = BatchDir::new(&format!("decode-{dataset}-{fault}"));
                dir.write(sound, &[&good]);
                let path = dir.0.join(corrupt);
                fs::write(&path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
                let cause = read_records::<TransactionLayout>(&path)
                    .err()
                    .unwrap_or_else(|| panic!("{corrupt} {fault} must not decode"));

                let status = sort_step(&dir.context().generations, &JobCounts::default()).err();

                let text = format!("{dataset}(0) {corrupt} at {}: {cause}", path.display());
                assert_eq!(
                    status.as_ref().map(ExitStatus::render),
                    Some(vec![format!("SETUP FAILURE: {text}")]),
                    "{corrupt} {fault}"
                );
                assert_eq!(
                    status,
                    Some(ExitStatus::SetupFailure(text.clone())),
                    "{corrupt} {fault}"
                );
                assert_eq!(
                    dir.files(),
                    ["SYSTRAN.G0001V00", "TRANSACT.BKUP.G0002V00"],
                    "{corrupt} {fault}: nothing is written"
                );
                texts.insert(text);
            }
        }
        assert_eq!(
            texts.len(),
            6,
            "each corrupt SORTIN has its own text: {texts:?}"
        );
    }

    #[test]
    fn the_job_span_holds_the_sortin_generations_and_the_step05r_counts() {
        let backup = [
            record("0000000000000001", "BACKUP 1"),
            record("0000000000000009", "BACKUP 9"),
        ];
        let input = r#""input":"TRANSACT.BKUP.G0002V00,SYSTRAN.G0001V00""#;
        // STEP05R alone loads and rejects nothing, so `posted` and `rejected` are 0 on every
        // path; STEP10 sets them once it has loaded the combined file.
        let unloaded = [r#""posted":0"#, r#""rejected":0"#];
        // (case, the SYSTRAN(0) record, span fragments present, span fragments absent). The
        // counts are JSON integers, as `exit_code` is.
        let cases: [(&str, String, &[&str], &[&str]); 3] = [
            // The two TRANSACT.BKUP(0) records were read before SYSTRAN(0) failed.
            (
                "SYSTRAN does not decode",
                bad_amount("0000000000000005", "INTEREST 5"),
                &[input, r#""read":2"#],
                &[r#""written""#],
            ),
            (
                "E-38 halt",
                record("0000000000000009", "INTEREST 9"),
                &[input, r#""read":3"#],
                &[r#""written""#],
            ),
            (
                "sorted",
                record("0000000000000005", "INTEREST 5"),
                &[input, r#""read":3"#, r#""written":3"#],
                &[],
            ),
        ];
        for (case, interest, present, absent) in cases {
            let dir = BatchDir::new(&format!("span-{}", case.replace(' ', "-")));
            dir.write("TRANSACT.BKUP.G0002V00", &backup.iter().collect::<Vec<_>>());
            dir.write("SYSTRAN.G0001V00", &[&interest]);
            let generations = dir.context().generations;
            let counts = JobCounts::default();

            let (span, outcome) = job_span_line(&Command::Combtran, &counts, || {
                sort_step(&generations, &counts)
            });

            assert_eq!(outcome.is_ok(), case == "sorted", "{case}: {outcome:?}");
            for fragment in present.iter().chain(&unloaded) {
                assert!(has_member(&span, fragment), "{case}: {fragment} in {span}");
            }
            for fragment in absent {
                assert!(!span.contains(fragment), "{case}: no {fragment} in {span}");
            }
        }
    }

    /// What a sort scenario ends with.
    enum Sorts {
        /// `TRANSACT.COMBINED.G0001V00` holding these bytes.
        Combined(String),
        /// This status, with nothing written.
        Halts(ExitStatus),
        /// The setup failure of the SORTIN file `corrupt` of `dataset`, whose cause is the error
        /// `read_records` gives for that file, with nothing written.
        Unreadable {
            dataset: &'static str,
            corrupt: &'static str,
        },
    }

    /// One sort scenario: the bytes of `TRANSACT.BKUP.G0001V00` and `SYSTRAN.G0001V00`, and
    /// the outcome.
    struct Scenario {
        name: String,
        backup: String,
        systran: String,
        sorts: Sorts,
    }

    impl Scenario {
        fn new(name: &str, backup: &[&String], systran: &[&String], sorts: Sorts) -> Self {
            Self {
                name: name.to_owned(),
                backup: file_of(backup),
                systran: file_of(systran),
                sorts,
            }
        }
    }

    /// Every sort scenario: [`written_scenarios`], then [`stopping_scenarios`].
    fn scenarios() -> Vec<Scenario> {
        let mut scenarios = written_scenarios();
        scenarios.extend(stopping_scenarios());
        scenarios
    }

    /// The sorts of the tests above that write `TRANSACT.COMBINED(+1)`, the order of many keys
    /// with identical duplicates and a preserved negative-zero amount, and an empty input.
    fn written_scenarios() -> Vec<Scenario> {
        let order_backup = [
            record("0000000000000005", "BACKUP 5"),
            record("1234", "BACKUP 1234"),
            record("A000000000000000", "BACKUP A"),
        ];
        let order_systran = [
            record("2022071800000001", "INTEREST 1"),
            record("1234000000000000", "INTEREST 1234"),
            record("0000000000000002", "INTEREST 2"),
        ];
        let lf_backup = [
            record("0000000000000005", "BACKUP 5"),
            record("2022071800000001", "BACKUP 2022071800000001"),
        ];
        let interest = interest_record("2022\n71800000001");
        let twice = record("0000000000000007", "SAME CONTENT");
        let other = record("0000000000000003", "OTHER");
        // 40 records over 31 keys: the keys that repeat hold identical records.
        let mut many: Vec<String> = (0..40_u32)
            .map(|index| {
                let key = index * 7919 % 31;
                record(&format!("{key:016}"), &format!("KEY {key}"))
            })
            .collect();
        let mut negative_zero = record("0000000000000500", "NEGATIVE ZERO").into_bytes();
        negative_zero[132..143].copy_from_slice(b"0000000000}");
        many.push(String::from_utf8(negative_zero).unwrap_or_else(|e| panic!("ASCII: {e}")));
        let mut many_sorted: Vec<&String> = many.iter().collect();
        many_sorted.sort();
        let (many_backup, many_systran) = many.split_at(25);
        vec![
            Scenario::new(
                "order",
                &order_backup.iter().collect::<Vec<_>>(),
                &order_systran.iter().collect::<Vec<_>>(),
                Sorts::Combined(file_of(&[
                    &order_systran[2],
                    &order_backup[0],
                    &order_backup[1],
                    &order_systran[1],
                    &order_systran[0],
                    &order_backup[2],
                ])),
            ),
            Scenario::new(
                "lf-tran-id",
                &lf_backup.iter().collect::<Vec<_>>(),
                &[&interest],
                Sorts::Combined(file_of(&[&lf_backup[0], &interest, &lf_backup[1]])),
            ),
            Scenario::new(
                "identical",
                &[&twice],
                &[&twice, &other],
                Sorts::Combined(file_of(&[&other, &twice, &twice])),
            ),
            Scenario::new(
                "many",
                &many_backup.iter().collect::<Vec<_>>(),
                &many_systran.iter().collect::<Vec<_>>(),
                Sorts::Combined(file_of(&many_sorted)),
            ),
            Scenario::new("empty", &[], &[], Sorts::Combined(String::new())),
        ]
    }

    /// The sorts that stop before writing: two different records with one key whose first pair
    /// lies in different runs, after an identical pair, and each SORTIN fault of
    /// `a_sortin_that_cannot_be_decoded_is_named_by_dataset_generation_and_path` beside a sound
    /// SORTIN of two records.
    fn stopping_scenarios() -> Vec<Scenario> {
        let key = |n: u8| format!("{n:016}");
        let halt_backup = [
            record(&key(2), "BACKUP 2"),
            record(&key(0), "SAME"),
            record(&key(1), "BACKUP 1"),
        ];
        let halt_systran = [
            record(&key(0), "SAME"),
            record(&key(1), "INTEREST 1"),
            record(&key(2), "INTEREST 2"),
        ];
        let halt = ExitStatus::EquivalenceHalt {
            lines: vec![
                "TRANSACT.COMBINED(+1) not written: TRAN-ID 0000000000000001 keys two different \
                 records, TRANSACT.BKUP.G0001V00 record 3 and SYSTRAN.G0001V00 record 2"
                    .to_owned(),
                "SORT FIELDS=(TRAN-ID,A) without EQUALS leaves their order undefined, and with it \
                 the record REPRO loads"
                    .to_owned(),
            ],
            escalation_id: "E-38".to_owned(),
        };
        let mut scenarios = vec![Scenario::new(
            "halt",
            &halt_backup.iter().collect::<Vec<_>>(),
            &halt_systran.iter().collect::<Vec<_>>(),
            Sorts::Halts(halt),
        )];
        let good = record("0000000000000001", "GOOD");
        let sound = file_of(&[&good, &record("0000000000000003", "GOOD 3")]);
        let faults = [
            ("amount", file_of(&[&bad_amount("0000000000000005", "BAD")])),
            ("truncated", format!("{good}\n{}", &good[..341])),
            ("crlf", format!("{good}\r\n")),
        ];
        for (dataset, corrupt) in [
            ("TRANSACT.BKUP", "TRANSACT.BKUP.G0001V00"),
            ("SYSTRAN", "SYSTRAN.G0001V00"),
        ] {
            for (fault, bytes) in &faults {
                let (backup, systran) = if dataset == "SYSTRAN" {
                    (sound.clone(), bytes.clone())
                } else {
                    (bytes.clone(), sound.clone())
                };
                scenarios.push(Scenario {
                    name: format!("{dataset}-{fault}"),
                    backup,
                    systran,
                    sorts: Sorts::Unreadable { dataset, corrupt },
                });
            }
        }
        scenarios
    }

    /// Each sort scenario ends alike within the job's bounds, where it is sorted in memory, and
    /// within bounds so small that every chunk spills to a sort work file and the runs cascade:
    /// the same combined bytes, or the same status with nothing written, and no work file left.
    #[test]
    fn spilled_sorts_write_and_halt_as_the_in_memory_sort_does() {
        let limits = [
            SORT_LIMITS,
            SortLimits {
                chunk: 1,
                fan_in: 2,
            },
            SortLimits {
                chunk: 2,
                fan_in: 2,
            },
            SortLimits {
                chunk: 3,
                fan_in: 3,
            },
        ];
        let inputs = ["SYSTRAN.G0001V00", "TRANSACT.BKUP.G0001V00"];
        for scenario in scenarios() {
            for limit in limits {
                let case = format!(
                    "{} within chunk {} fan-in {}",
                    scenario.name, limit.chunk, limit.fan_in
                );
                let dir = BatchDir::new(&format!(
                    "spill-{}-{}-{}",
                    scenario.name, limit.chunk, limit.fan_in
                ));
                for (name, bytes) in inputs.iter().zip([&scenario.systran, &scenario.backup]) {
                    let path = dir.0.join(name);
                    fs::write(&path, bytes)
                        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
                }

                let outcome =
                    sort_step_within(&dir.context().generations, &JobCounts::default(), limit);

                match &scenario.sorts {
                    Sorts::Combined(bytes) => {
                        let combined =
                            outcome.unwrap_or_else(|status| panic!("{case}: {status:?}"));
                        assert_eq!(read(&combined), *bytes, "{case}");
                        assert_eq!(
                            dir.files(),
                            [inputs[0], inputs[1], "TRANSACT.COMBINED.G0001V00"],
                            "{case}: no work file is left"
                        );
                    }
                    Sorts::Halts(status) => {
                        assert_eq!(outcome.err().as_ref(), Some(status), "{case}");
                        assert_eq!(dir.files(), inputs, "{case}: nothing is left");
                    }
                    Sorts::Unreadable { dataset, corrupt } => {
                        let path = dir.0.join(corrupt);
                        let decode_error = read_records::<TransactionLayout>(&path)
                            .err()
                            .unwrap_or_else(|| panic!("{case}: {corrupt} must not decode"));
                        let text = format!(
                            "{dataset}(0) {corrupt} at {}: {decode_error}",
                            path.display()
                        );
                        assert_eq!(
                            outcome.err(),
                            Some(ExitStatus::SetupFailure(text)),
                            "{case}"
                        );
                        assert_eq!(dir.files(), inputs, "{case}: nothing is left");
                    }
                }
            }
        }
    }

    /// The bytes of `frames` sorted-run frames.
    fn frames(frames: usize) -> u64 {
        u64::try_from(frames * RUN_FRAME).unwrap_or_else(|e| panic!("{frames} frames: {e}"))
    }

    /// Runs of a [`RunLayout`], each as (run, first frame, frames).
    type ExpectedRuns = &'static [(usize, usize, usize)];

    /// A [`RunLayout`] derives the run count and every run from its two numbers: for runs that
    /// divide the records exactly or leave a short last run, below 1 record, longer than the
    /// records, with no records, and after merges, one of which saturates to a single run. The
    /// first, a middle and the last run are pinned, and a run past the last is empty at the end
    /// of the file; the runs in order tile the file's bytes exactly.
    #[test]
    fn a_run_layout_derives_every_run_from_its_run_size_and_records() {
        // (case, layout, records per run kept, runs, [(run, first frame, frames)])
        let cases: [(&str, RunLayout, usize, usize, ExpectedRuns); 11] = [
            (
                "exact multiple",
                RunLayout::new(4, 12),
                4,
                3,
                &[(0, 0, 4), (1, 4, 4), (2, 8, 4), (3, 12, 0)],
            ),
            (
                "short last run",
                RunLayout::new(4, 10),
                4,
                3,
                &[(0, 0, 4), (1, 4, 4), (2, 8, 2), (3, 10, 0)],
            ),
            (
                "one record per run",
                RunLayout::new(1, 3),
                1,
                3,
                &[(0, 0, 1), (1, 1, 1), (2, 2, 1), (3, 3, 0)],
            ),
            (
                "below 1 counts as 1",
                RunLayout::new(0, 2),
                1,
                2,
                &[(0, 0, 1), (1, 1, 1), (2, 2, 0)],
            ),
            (
                "longer than the records",
                RunLayout::new(4, 3),
                3,
                1,
                &[(0, 0, 3), (1, 3, 0)],
            ),
            (
                "no records",
                RunLayout::new(4, 0),
                1,
                0,
                &[(0, 0, 0), (1, 0, 0)],
            ),
            (
                "merged by 2",
                RunLayout::new(4, 10).merged(2),
                8,
                2,
                &[(0, 0, 8), (1, 8, 2), (2, 10, 0)],
            ),
            (
                "merged by 16, many runs",
                RunLayout::new(3, 3000).merged(16),
                48,
                63,
                &[(0, 0, 48), (31, 1488, 48), (62, 2976, 24), (63, 3000, 0)],
            ),
            (
                "merged past the records saturates to one run",
                RunLayout::new(4, 10).merged(3),
                10,
                1,
                &[(0, 0, 10), (1, 10, 0)],
            ),
            (
                "a run size of usize::MAX is one run, merged or not",
                RunLayout::new(usize::MAX, 7).merged(16),
                7,
                1,
                &[(0, 0, 7), (usize::MAX, 7, 0)],
            ),
            (
                "an exact multiple merged",
                RunLayout::new(2, 8).merged(2),
                4,
                2,
                &[(0, 0, 4), (1, 4, 4), (2, 8, 0)],
            ),
        ];
        for (case, layout, run_records, count, runs) in cases {
            assert_eq!(layout.run_records, run_records, "{case}");
            assert_eq!(layout.count(), count, "{case}");
            assert_eq!(layout.bytes(), frames(layout.records), "{case}");
            for &(index, first, len) in runs {
                assert_eq!(
                    layout.run(index),
                    Run {
                        start: frames(first),
                        len: frames(len),
                    },
                    "{case}: run {index}"
                );
            }
            let end = layout.runs(0..count).try_fold(0, |end, run| {
                (run.start == end && run.len > 0).then_some(run.start + run.len)
            });
            assert_eq!(end, Some(layout.bytes()), "{case}: the runs tile the file");
        }
    }

    /// One `REPRO` row of the audit: the combined file, the records whose TRAN-ID is live, and
    /// the outcome.
    struct LoadRow {
        row: &'static str,
        records: i64,
        duplicates: &'static [i64],
        loaded: usize,
        rejected: usize,
        attempted: usize,
        exit: u8,
    }

    const LOAD_ROWS: [LoadRow; 5] = [
        LoadRow {
            row: "P-COMBTRAN-002 empty file",
            records: 0,
            duplicates: &[],
            loaded: 0,
            rejected: 0,
            attempted: 0,
            exit: 0,
        },
        LoadRow {
            row: "P-COMBTRAN-005",
            records: 312,
            duplicates: &[],
            loaded: 312,
            rejected: 0,
            attempted: 312,
            exit: 0,
        },
        LoadRow {
            row: "P-COMBTRAN-006",
            records: 312,
            duplicates: &[2, 5, 9, 300],
            loaded: 296,
            rejected: 4,
            attempted: 300,
            exit: 12,
        },
        LoadRow {
            row: "P-COMBTRAN-007",
            records: 312,
            duplicates: &[1, 2, 3, 4],
            loaded: 0,
            rejected: 4,
            attempted: 4,
            exit: 12,
        },
        LoadRow {
            row: "P-COMBTRAN-008",
            records: 312,
            duplicates: &[1, 3],
            loaded: 310,
            rejected: 2,
            attempted: 312,
            exit: 8,
        },
    ];

    #[test]
    fn repro_tally_follows_the_audited_load_rows() {
        for case in &LOAD_ROWS {
            let mut tally = ReproTally::default();
            for number in 1..=case.records {
                // Revived and inserted records both count as loaded.
                let outcome = if case.duplicates.contains(&number) {
                    LoadOutcome::DuplicateLive
                } else if number % 2 == 0 {
                    LoadOutcome::Revived(TransactionRowId::from(number))
                } else {
                    LoadOutcome::Inserted(TransactionRowId::from(number))
                };
                if tally.count(outcome).is_break() {
                    break;
                }
            }
            assert_eq!(
                (tally.loaded(), tally.duplicates, tally.attempted()),
                (case.loaded, case.rejected, case.attempted),
                "{}",
                case.row
            );
            let read =
                usize::try_from(case.records).unwrap_or_else(|e| panic!("{}: {e}", case.row));
            let counts = JobCounts::default();
            assert_eq!(
                finish(tally, read, &counts),
                ExitStatus::ReproCondition(case.exit),
                "{}",
                case.row
            );
            // STEP10 leaves `read` to STEP05R.
            assert_eq!(
                (counts.read(), counts.posted(), counts.rejected()),
                (0, count_of(case.loaded), count_of(case.rejected)),
                "{}",
                case.row
            );
        }
    }

    #[test]
    fn a_load_not_committed_posts_nothing_and_keeps_its_duplicates() {
        let mut tally = ReproTally::default();
        for outcome in [
            LoadOutcome::Inserted(TransactionRowId::from(1)),
            LoadOutcome::DuplicateLive,
            LoadOutcome::Revived(TransactionRowId::from(3)),
            LoadOutcome::DuplicateLive,
        ] {
            assert!(tally.count(outcome).is_continue(), "{tally:?}");
        }
        let counts = JobCounts::default();
        let failure = ExitStatus::SetupFailure("commit failed".to_owned());

        assert_eq!(not_committed(tally, &counts, failure.clone()), failure);
        assert_eq!(
            (counts.read(), counts.posted(), counts.rejected()),
            (0, 0, 2)
        );
    }
}
