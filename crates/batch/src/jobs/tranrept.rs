//! `batch tranrept`: the TRANREPT job [app/jcl/TRANREPT.jcl], step for step the procedure
//! [app/proc/TRANREPT.prc]. Its three steps run in JCL order, and none has a `COND`, so each one
//! runs whatever the condition code of the step before it:
//!
//! 1. `STEP05R` `EXEC PROC=REPROC` (P-TRANREPT-001): the live transaction master to
//!    `TRANSACT.BKUP(+1)`, through the backup TRANBKP shares ([`backup_live_master`]).
//! 2. `STEP05R` `EXEC PGM=SORT` (P-TRANREPT-002): the backup records whose processing date lies
//!    in the range, sorted by card number, to `TRANSACT.DALY(+1)` ([`sort_step`]).
//! 3. `STEP10R` `EXEC PGM=CBTRN03C` (P-TRANREPT-003): the report of those records to
//!    `TRANREPT(+1)`, 133-byte lines ([`report_step`]).
//!
//! `--start-date` and `--end-date` are both the `SYMNAMES` dates of the sort and the `DATEPARM`
//! record of CBTRN03C, as the CR00 job writes them (E-34, P-CBTRN03C-467). The selection, the sort
//! and the report lines come from `carddemo_domain::rules::transaction_report`, which the CR00
//! endpoint shares, so the two reports cannot drift apart. CBTRN03C reads the cross-reference and
//! the `TRANTYPE` and `TRANCATG` processing copies through the store, never the Db2 masters, so an
//! admin edit of a type reaches this report only after `batch seed --only tranextr`.
//!
//! Each `(+1)` generation is cataloged when its step ends normally. A step that fails leaves its
//! own generation uncataloged and deleted, as `DISP=(NEW,CATLG,DELETE)` does, while the
//! generations of the steps before it stay cataloged.
//!
//! Every file-system, codec and sorting phase of the three steps runs on Tokio's blocking pool,
//! inside the job span: the whole SORT step through [`blocking`], and the generation allocation,
//! reads, appends, catalogs and discards of the other two through the asynchronous methods of
//! [`crate::generations`].
//!
//! No step holds its whole input, so the job's memory stays bounded however large the live master
//! grows: REPROC copies the master a page at a time, the SORT step streams `TRANSACT.BKUP(+1)`
//! and sorts the records it keeps externally, in runs of [`SORT_RUN_RECORDS`] records that it
//! spills to DFSORT work files in `CARDDEMO_BATCH_DIR` and merges [`SORT_MERGE_FAN_IN`] at a time
//! ([`sort_step`]), and CBTRN03C reads `TRANSACT.DALY(+1)` [`TRANFILE_CHUNK_RECORDS`] records at
//! a time ([`report_records`]).

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::sync::Arc;
use std::{io, mem};

use carddemo_codec::layouts::{TransactionLayout, TransactionRecord, encode_ascii_lf};
use carddemo_codec::{Charset, Layout};
use carddemo_domain::format::REPORT_RECORD_LEN;
use carddemo_domain::messages::{
    ABENDING_PROGRAM, END_OF_EXECUTION_OF_PROGRAM_CBTRN03C, ERROR_CLOSING_REPORT_FILE,
    ERROR_OPENING_CROSS_REF_FILE, ERROR_OPENING_REPTFILE, ERROR_OPENING_TRANFILE,
    ERROR_OPENING_TRANSACTION_CATG_FILE, ERROR_OPENING_TRANSACTION_TYPE_FILE,
    ERROR_READING_TRANSACTION_FILE, ERROR_WRITING_REPTFILE, file_status_message,
};
use carddemo_domain::rules::transaction_report::{
    self, Alnum, CardNumber, ReportAbend, ReportRun, ReportSortKey, TranCategoryCode, TranTypeCode,
    TranTypeFileReader, Transaction, TransactionRowId, UnpinnedSortKey, XrefReader,
};
use carddemo_domain::rules::{PortError, PortResult};
use carddemo_persistence::tx::{Db, UnitOfWork};
use tracing::Span;

use super::{JobContext, JobCounts, backup_live_master, count_of, setup_failure, span_count};
use crate::cli::TranreptArgs;
use crate::exit::{ExitStatus, one_line};
use crate::generations::{
    Dataset, Generation, GenerationError, Generations, PendingGeneration, RecordReader, WorkFile,
    WorkFileWriter, WorkReader, blocking,
};

/// `ABCODE` 999, which CBTRN03C `9999-ABEND-PROGRAM` passes to `CEE3ABD`
/// [app/cbl/CBTRN03C.cbl:L626-630] — P-CBTRN03C-484.
const ABCODE: u32 = 999;

/// `MOVE 23 TO IO-STATUS`, the status the three lookups display on `INVALID KEY`
/// [app/cbl/CBTRN03C.cbl:L488] [app/cbl/CBTRN03C.cbl:L498] [app/cbl/CBTRN03C.cbl:L508] —
/// P-CBTRN03C-485.
const INVALID_KEY_STATUS: [u8; 2] = *b"23";

/// The status displayed when a generation file of CBTRN03C fails: a file-system error has no
/// file status of its own, so it shows as 30, the permanent I/O error without further
/// information, as the CBACT04C rules show a database failure.
const PERMANENT_ERROR_STATUS: [u8; 2] = *b"30";

/// Category `0000`: with a type code of spaces, the key [`open_lookup_files`] opens `TRANCATG` by.
const OPEN_CATEGORY: TranCategoryCode = match TranCategoryCode::new(0) {
    Ok(category) => category,
    Err(_) => panic!("0 fits the four digits of TRAN-CAT-CD"),
};

/// `TRANFILE`, the `TRANSACT.DALY(+1)` that [`open_tranfile`] opened, read record by record
/// through `1000-TRANFILE-GET-NEXT` ([`report_records`]), a chunk at a time on Tokio's blocking
/// pool ([`TranFile::next_chunk`]).
struct TranFile {
    /// The file, as errors name it.
    path: PathBuf,
    /// The open file; `None` while a chunk is read on the blocking pool, and for good once a
    /// cancelled read lost it.
    reader: Option<RecordReader<TransactionLayout>>,
    /// The error a chunk met after the records it returned, raised by the next `READ`.
    failed: Option<GenerationError>,
}

impl TranFile {
    /// CBTRN03C `0000-TRANFILE-OPEN` [app/cbl/CBTRN03C.cbl:L376-392] — P-CBTRN03C-468: `OPEN
    /// INPUT` of `daly` on Tokio's blocking pool, at its first record.
    ///
    /// # Errors
    ///
    /// The [`GenerationError::Io`] of a file that cannot be opened, as
    /// [`RecordReader::open_async`] reports it.
    async fn open(daly: &Generation) -> Result<Self, GenerationError> {
        let reader = RecordReader::open_async(daly.path.clone()).await?;
        Ok(Self {
            path: daly.path.clone(),
            reader: Some(reader),
            failed: None,
        })
    }

    /// CBTRN03C `1000-TRANFILE-GET-NEXT` [app/cbl/CBTRN03C.cbl:L248-272] — P-CBTRN03C-474: the
    /// next records of the file, at most `max` and at least one unless the file has ended, read
    /// and decoded on Tokio's blocking pool inside the caller's span (see [`blocking`]). An empty
    /// chunk is the end of the file.
    ///
    /// The `READ`s are sequential: when a record cannot be read after others of the chunk were,
    /// those records are returned and its error is held back for the next call, so the caller
    /// reports every record before the failing `READ`, as the COBOL read loop does.
    ///
    /// # Errors
    ///
    /// The error [`RecordReader`] yields for the first record of the chunk, or the error held
    /// back: a [`GenerationError::Io`] on the file when it cannot be read, or the
    /// [`GenerationError::Codec`] of a record that does not decode. A [`GenerationError::Io`] on
    /// the file when the runtime cancels the read because it is shutting down; the open file is
    /// then lost, and every later call fails the same way.
    async fn next_chunk(&mut self, max: usize) -> Result<Vec<TransactionRecord>, GenerationError> {
        if let Some(failed) = self.failed.take() {
            return Err(failed);
        }
        let Some(mut reader) = self.reader.take() else {
            return Err(GenerationError::Io {
                path: self.path.clone(),
                source: io::Error::other("TRANFILE was lost by a cancelled read"),
            });
        };
        let max = max.max(1);
        let lent = blocking(move || {
            let mut records = Vec::with_capacity(max);
            let mut failed = None;
            while records.len() < max {
                match reader.next() {
                    Some(Ok(record)) => records.push(record),
                    Some(Err(err)) => {
                        failed = Some(err);
                        break;
                    }
                    None => break,
                }
            }
            (reader, records, failed)
        })
        .await;
        let (reader, records, failed) = lent.map_err(|cancelled| GenerationError::Io {
            path: self.path.clone(),
            source: io::Error::other(cancelled),
        })?;
        self.reader = Some(reader);
        match failed {
            Some(err) if records.is_empty() => Err(err),
            failed => {
                self.failed = failed;
                Ok(records)
            }
        }
    }
}

/// The `TRANFILE` records CBTRN03C reads with one read on Tokio's blocking pool, so the report
/// step holds at most this many records of `TRANSACT.DALY(+1)` at a time.
const TRANFILE_CHUNK_RECORDS: usize = 512;

/// The records one sorted run of the `STEP05R` sort holds in memory before it is spilled to a
/// work file: the bound on the `SORTIN` records the sort holds at a time.
const SORT_RUN_RECORDS: usize = 4096;

/// The runs one merge of the `STEP05R` sort reads at once, each through its own work file.
const SORT_MERGE_FAN_IN: usize = 64;

/// The records the `STEP05R` sort writes to `TRANSACT.DALY(+1)` or a work file with one append.
const SORT_WRITE_RECORDS: usize = 512;

/// The bytes of one record of a sorted run on its work file: the 350-byte `CVTRA05Y` record as
/// `TRANSACT.DALY(+1)` holds it, then `LF`, the frame of every record file.
const RUN_FRAME: usize = TransactionLayout::LEN + 1;

/// The event of the setup failure when a DFSORT work file (`SORTWK`) cannot be created.
const SORTWK_ALLOCATION_FAILED: &str = "TRANSACT.DALY(+1) sort work file allocation failed";

/// The event of the setup failure when a DFSORT work file cannot be written.
const SORTWK_WRITE_FAILED: &str = "TRANSACT.DALY(+1) sort work file write failed";

/// The event of the setup failure when a DFSORT work file cannot be flushed once its run is
/// written.
const SORTWK_CLOSE_FAILED: &str = "TRANSACT.DALY(+1) sort work file close failed";

/// The event of the setup failure when a DFSORT work file cannot be read back.
const SORTWK_READ_FAILED: &str = "TRANSACT.DALY(+1) sort work file read failed";

/// A sorted run of the `STEP05R` sort — P-TRANREPT-002, spilled to a DFSORT work file of its own
/// ([`Generations::create_work_file`]), which is deleted when the run is dropped.
struct Run {
    /// The work file, holding the run's records as [`RUN_FRAME`]-byte frames from offset 0.
    work: WorkFile,
    /// The bytes the run holds.
    len: u64,
}

/// One `SORTIN` record the `STEP05R` sort keeps, with its place in the sort output.
type SortEntry = (ReportSortKey, TransactionRecord);

/// TRANREPT [app/jcl/TRANREPT.jcl] — P-TRANREPT-001 to P-TRANREPT-003: REPROC, SORT and
/// CBTRN03C over the dates of `args`.
///
/// Every step runs, since none has a `COND`. SORT and CBTRN03C end with condition code 0, so a
/// job whose steps all end normally ends with the condition code of the REPROC step: 0, or 12
/// when the live master was empty (`Backup::status`, E-37), after an empty sort output and a
/// report of zero totals. A failed step ends the job with its status and runs no later step:
///
/// - REPROC and SORT: a generation or database failure, or a record the sort cannot place, exit 16.
/// - CBTRN03C: an abend, exit 16 with its `DISPLAY` lines and `ABEND 999`, a failed open of a
///   file included, or a database connection failure outside the program's file statements,
///   exit 16.
///
/// The SORT step runs on Tokio's blocking pool; a runtime that shuts down before it starts ends
/// the job with a setup failure (exit 16) and allocates nothing.
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &TranreptArgs) -> ExitStatus {
    let start = Alnum::<10>::move_from(&args.start_date);
    let end = Alnum::<10>::move_from(&args.end_date);
    let backup = match backup_live_master(ctx, db).await {
        Ok(backup) => backup,
        Err(status) => return status,
    };
    let generations = ctx.generations.clone();
    let counts = Arc::clone(&ctx.counts);
    let bkup = backup.generation.clone();
    let daly = match blocking(move || sort_step(&generations, &counts, &bkup, &start, &end)).await {
        Ok(Ok(daly)) => daly,
        Ok(Err(status)) => return status,
        Err(cancelled) => {
            return setup_failure("TRANSACT.DALY(+1) sort did not complete", cancelled);
        }
    };
    match report_step(ctx, db, &daly, start, end).await {
        Ok(()) => backup.status(),
        Err(status) => status,
    }
}

/// TRANREPT `STEP05R` `EXEC PGM=SORT` [app/jcl/TRANREPT.jcl:L37-55] — P-TRANREPT-002: `SORTIN`
/// `TRANSACT.BKUP(+1)`, `INCLUDE COND=(TRAN-PROC-DT,GE,PARM-START-DATE,AND,TRAN-PROC-DT,LE,
/// PARM-END-DATE)` with `TRAN-PROC-DT,305,10,CH`, `SORT FIELDS=(TRAN-CARD-NUM,A)` with
/// `TRAN-CARD-NUM,263,16,ZD`, and `SORTOUT` `TRANSACT.DALY(+1)` `DCB=(*.SORTIN)`.
///
/// Allocates `TRANSACT.DALY(+1)` in `generations`, reads every record of `bkup`, writes the
/// records the selection keeps in the sort order ([`select_runs`]) and catalogs the generation.
/// The number of backup records is set in `read` in `counts`, once every record is read, whose
/// `posted` and `rejected` stay 0 because TRANREPT loads no record and turns none away, and the
/// number of sorted records is recorded in `written` on the current span.
///
/// The sort is external, as DFSORT's is: it holds at most [`SORT_RUN_RECORDS`] kept records in
/// memory, spills each full run to a DFSORT work file (`SORTWK`, [`Generations::create_work_file`])
/// in `CARDDEMO_BATCH_DIR` and merges the runs [`SORT_MERGE_FAN_IN`] at a time
/// ([`sort_step_within`]). Every work file is deleted once its run is merged, and on every error.
///
/// The step is synchronous from the allocation to the catalog: [`run`] runs it on Tokio's
/// blocking pool, inside the job span, so an uncataloged `TRANSACT.DALY(+1)` is deleted there.
///
/// # Errors
///
/// - A record the sort cannot place, as [`select_runs`] finds it: exit 16
///   ([`unpinned_sort_key`]), once every record is read.
/// - A failure to allocate, read, write or catalog a generation or a work file is a setup
///   failure (exit 16), because the audit records no condition code for it. `TRANSACT.DALY(+1)`
///   is then not cataloged, and `TRANSACT.BKUP(+1)` stays cataloged.
fn sort_step(
    generations: &Generations,
    counts: &JobCounts,
    bkup: &Generation,
    start: &Alnum<10>,
    end: &Alnum<10>,
) -> Result<Generation, ExitStatus> {
    sort_step_within(generations, counts, bkup, start, end, SortLimits::DEFAULT)
}

/// The memory bounds of the TRANREPT `STEP05R` external sort — P-TRANREPT-002. The job sorts
/// within [`SortLimits::DEFAULT`]; the tests pass tiny bounds to force spills and merge passes.
#[derive(Debug, Clone, Copy)]
struct SortLimits {
    /// The kept records a run holds before it is spilled; at least 1.
    run_records: usize,
    /// The runs one merge reads at once; at least 2.
    fan_in: usize,
    /// The records one append writes; at least 1.
    write_records: usize,
}

impl SortLimits {
    /// The job's bounds: [`SORT_RUN_RECORDS`], [`SORT_MERGE_FAN_IN`] and [`SORT_WRITE_RECORDS`].
    const DEFAULT: Self = Self {
        run_records: SORT_RUN_RECORDS,
        fan_in: SORT_MERGE_FAN_IN,
        write_records: SORT_WRITE_RECORDS,
    };

    /// TRANREPT `STEP05R` — P-TRANREPT-002: the bounds raised to the least each one can be, a
    /// run of one record, a merge of two runs and an append of one record.
    fn usable(self) -> Self {
        Self {
            run_records: self.run_records.max(1),
            fan_in: self.fan_in.max(2),
            write_records: self.write_records.max(1),
        }
    }
}

/// [`sort_step`] within the memory bounds `limits`: TRANREPT `STEP05R` `EXEC PGM=SORT`
/// [app/jcl/TRANREPT.jcl:L37-55] — P-TRANREPT-002.
///
/// The kept records reach `TRANSACT.DALY(+1)` in the order of
/// `transaction_report::select_for_report`, each one the `SORTIN` record unchanged, FILLER and
/// preserved bytes included, as DFSORT copies a record whole. Without a spill the one run is
/// sorted in memory and written; otherwise the runs are merged ([`reduce_runs`], [`Merge`]).
///
/// # Errors
///
/// As [`sort_step`].
fn sort_step_within(
    generations: &Generations,
    counts: &JobCounts,
    bkup: &Generation,
    start: &Alnum<10>,
    end: &Alnum<10>,
    limits: SortLimits,
) -> Result<Generation, ExitStatus> {
    let limits = limits.usable();
    let mut sortout = generations
        .begin_next(Dataset::TransactDaly)
        .map_err(|err| setup_failure("TRANSACT.DALY(+1) allocation failed", err))?;
    let mut sortin = RecordReader::<TransactionLayout>::open(&bkup.path)
        .map_err(|err| setup_failure("TRANSACT.BKUP(+1) read failed", err))?;
    let (read, selection) = select_runs(&mut sortin, generations, start, end, limits)?;
    counts.set_read(count_of(read));
    let written = match selection {
        Selection::Unpinned(err) => return Err(unpinned_sort_key(&err)),
        Selection::InMemory(mut run) => {
            run.sort_by_key(|&(key, _)| key);
            let mut records = run.into_iter().map(|(_, record)| record);
            write_sortout(&mut sortout, || Ok(records.next()), limits)?
        }
        Selection::Spilled(runs) => {
            let runs = reduce_runs(generations, runs, limits)?;
            let mut merge = Merge::new(&runs)?;
            write_sortout(&mut sortout, || merge.next_record(), limits)?
        }
    };
    let daly = sortout
        .catalog()
        .map_err(|err| setup_failure("TRANSACT.DALY(+1) catalog failed", err))?;
    Span::current().record("written", span_count(written));
    tracing::info!(
        generation = %daly,
        read,
        written,
        "transactions in the date range sorted by card number"
    );
    Ok(daly)
}

/// What the `STEP05R` selection kept of `SORTIN`, once every record is read.
enum Selection {
    /// Every kept record, in input order, when no run was spilled.
    InMemory(Vec<SortEntry>),
    /// The sorted runs in input order, the last one included, when a run was spilled.
    Spilled(Vec<Run>),
    /// The first kept record, in input order, whose card number the sort cannot place.
    Unpinned(UnpinnedSortKey),
}

/// TRANREPT `STEP05R` `INCLUDE COND` and `SORT FIELDS` [app/jcl/TRANREPT.jcl:L37-55] —
/// P-TRANREPT-002, the read phase of the external sort: reads every record of `sortin`, keeps
/// those [`transaction_report::report_sort_key`] places between `start` and `end`, and gathers
/// them in runs of `limits.run_records` records, each spilled to a work file in the catalog
/// directory of `generations` when full ([`spill`], [`Runs::push`]). Returns the number of
/// records read and the selection.
///
/// `SORT FIELDS` has no `EQUALS`, so DFSORT leaves the order of one card's records undefined
/// (E-38, a halt escalation). The job does not stop on equal card numbers: it takes the order the
/// audit's disposition records and its report lines assume, TRAN-ID within a card, from the rule
/// the CR00 report shares, so both reports give the same lines. Only a card number the
/// zoned-decimal sort cannot place stops it: from that record on nothing is kept and every run
/// is dropped, but the rest of `sortin` is still read, so the count covers every record and a
/// later record that cannot be read still fails the step first.
///
/// # Errors
///
/// A setup failure (exit 16) when a record of `sortin` cannot be read or decoded, or a work file
/// cannot be allocated, written or read; every work file is then deleted.
fn select_runs(
    sortin: &mut RecordReader<TransactionLayout>,
    generations: &Generations,
    start: &Alnum<10>,
    end: &Alnum<10>,
    limits: SortLimits,
) -> Result<(usize, Selection), ExitStatus> {
    let mut run: Vec<SortEntry> = Vec::new();
    let mut runs = Runs::default();
    let mut unpinned = None;
    let mut read = 0;
    // A backup record carries no row identity; its record number stands in for one.
    for number in record_numbers() {
        let Some(next) = sortin.next() else {
            break;
        };
        let record = next.map_err(|err| setup_failure("TRANSACT.BKUP(+1) read failed", err))?;
        read += 1;
        if unpinned.is_some() {
            continue;
        }
        let tran = Transaction::from_new(number, record.transaction.clone());
        match transaction_report::report_sort_key(start, end, &tran) {
            Ok(Some(key)) => run.push((key, record)),
            Ok(None) => {}
            Err(err) => {
                unpinned = Some(err);
                run = Vec::new();
                runs = Runs::default();
            }
        }
        if run.len() >= limits.run_records {
            let spilled = spill(generations, &mut run, limits)?;
            runs.push(generations, spilled, limits)?;
        }
    }
    let selection = if let Some(err) = unpinned {
        Selection::Unpinned(err)
    } else if runs.is_empty() {
        Selection::InMemory(run)
    } else {
        if !run.is_empty() {
            let last = spill(generations, &mut run, limits)?;
            runs.push(generations, last, limits)?;
        }
        Selection::Spilled(runs.into_ordered())
    };
    Ok((read, selection))
}

/// The DFSORT run spill of TRANREPT `STEP05R` — P-TRANREPT-002: sorts `run` stably by key and
/// writes its records to a new work file in the catalog directory of `generations` in that
/// order, `limits.write_records` at a time, leaving `run` empty.
///
/// # Errors
///
/// A setup failure (exit 16) when the work file cannot be created, written or flushed; it is
/// then deleted.
fn spill(
    generations: &Generations,
    run: &mut Vec<SortEntry>,
    limits: SortLimits,
) -> Result<Run, ExitStatus> {
    run.sort_by_key(|&(key, _)| key);
    let mut records = run.drain(..).map(|(_, record)| record);
    write_work_file(generations, || Ok(records.next()), limits)
}

/// The DFSORT merge of TRANREPT `STEP05R` — P-TRANREPT-002 into a work file: merges `runs`, in
/// input order, into one run on a new work file in the catalog directory of `generations`,
/// written `limits.write_records` records at a time. The caller drops the runs merged, which
/// deletes their work files, once the merge ends.
///
/// # Errors
///
/// A setup failure (exit 16) when a run cannot be read, or the work file cannot be created,
/// written or flushed; the new work file is then deleted.
fn merge_to_work_file(
    generations: &Generations,
    runs: &[Run],
    limits: SortLimits,
) -> Result<Run, ExitStatus> {
    let mut merge = Merge::new(runs)?;
    write_work_file(generations, || merge.next_record(), limits)
}

/// The DFSORT work-file write of TRANREPT `STEP05R` — P-TRANREPT-002: every record `next`
/// yields, in that order, into a new work file in the catalog directory of `generations`
/// ([`Generations::create_work_file`]), as [`RUN_FRAME`]-byte frames, `limits.write_records`
/// records at a time. Returns the work file as a run.
///
/// # Errors
///
/// The first error of `next`, and a setup failure (exit 16) when the work file cannot be
/// created, written or flushed; the work file is then deleted.
fn write_work_file(
    generations: &Generations,
    next: impl FnMut() -> Result<Option<TransactionRecord>, ExitStatus>,
    limits: SortLimits,
) -> Result<Run, ExitStatus> {
    let mut work: WorkFileWriter = generations
        .create_work_file()
        .map_err(|err| setup_failure(SORTWK_ALLOCATION_FAILED, err))?;
    write_chunked(next, limits.write_records, |chunk| {
        work.write(&encode_ascii_lf::<TransactionLayout>(chunk))
            .map_err(|err| setup_failure(SORTWK_WRITE_FAILED, err))
    })?;
    let len = work.offset();
    let work = work
        .finish()
        .map_err(|err| setup_failure(SORTWK_CLOSE_FAILED, err))?;
    Ok(Run { work, len })
}

/// The DFSORT `SORTOUT` write of TRANREPT `STEP05R` — P-TRANREPT-002: every record `next`
/// yields, in that order, appended to `TRANSACT.DALY(+1)`, `limits.write_records` records at a
/// time. Returns the number of records written.
///
/// # Errors
///
/// The first error of `next`, and a setup failure (exit 16) when `TRANSACT.DALY(+1)` cannot be
/// written.
fn write_sortout(
    sortout: &mut PendingGeneration,
    next: impl FnMut() -> Result<Option<TransactionRecord>, ExitStatus>,
    limits: SortLimits,
) -> Result<usize, ExitStatus> {
    write_chunked(next, limits.write_records, |chunk| {
        sortout
            .append_records::<TransactionLayout>(chunk)
            .map_err(|err| setup_failure("TRANSACT.DALY(+1) write failed", err))
    })
}

/// The DFSORT intermediate merge passes of TRANREPT `STEP05R` — P-TRANREPT-002: merges
/// consecutive groups of `limits.fan_in` runs of `runs`, which are in input order, each into one
/// run ([`merge_to_work_file`]), pass after pass, until at most `limits.fan_in` runs remain, still
/// in input order. A group of one run passes through unchanged.
///
/// # Errors
///
/// The setup failures of [`merge_to_work_file`]; every work file is then deleted.
fn reduce_runs(
    generations: &Generations,
    mut runs: Vec<Run>,
    limits: SortLimits,
) -> Result<Vec<Run>, ExitStatus> {
    while runs.len() > limits.fan_in {
        let mut merged = Vec::with_capacity(runs.len().div_ceil(limits.fan_in));
        let mut rest = runs.into_iter();
        loop {
            let group: Vec<Run> = rest.by_ref().take(limits.fan_in).collect();
            match group.len() {
                0 => break,
                1 => merged.extend(group),
                _ => merged.push(merge_to_work_file(generations, &group, limits)?),
            }
        }
        runs = merged;
    }
    Ok(runs)
}

/// The sorted runs of a TRANREPT `STEP05R` sort in progress — P-TRANREPT-002, by merge level, so
/// the open work files stay few however many runs the input fills: level 0 holds runs spilled
/// from memory, and level `n + 1` runs that each merge `fan_in` runs of level `n`.
///
/// The levels keep the input in order: every run of a higher level holds earlier records than
/// every run of a lower one, and the runs of one level are in input order, because a level is
/// merged only when it is full and its merge joins the end of the level above.
#[derive(Default)]
struct Runs {
    /// The runs of each level, level 0 first.
    levels: Vec<Vec<Run>>,
}

impl Runs {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: whether no run was spilled.
    fn is_empty(&self) -> bool {
        self.levels.iter().all(Vec::is_empty)
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002: adds `run`, the latest of the input, to level 0, and
    /// merges each level that reaches `limits.fan_in` runs into one run of the level above
    /// ([`merge_to_work_file`]).
    ///
    /// # Errors
    ///
    /// The setup failures of [`merge_to_work_file`].
    fn push(
        &mut self,
        generations: &Generations,
        run: Run,
        limits: SortLimits,
    ) -> Result<(), ExitStatus> {
        let mut run = run;
        let mut level = 0;
        loop {
            if self.levels.len() == level {
                self.levels.push(Vec::new());
            }
            let runs = &mut self.levels[level];
            runs.push(run);
            if runs.len() < limits.fan_in {
                return Ok(());
            }
            let full = mem::take(runs);
            run = merge_to_work_file(generations, &full, limits)?;
            level += 1;
        }
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002: every run in input order, the highest level first.
    fn into_ordered(self) -> Vec<Run> {
        self.levels.into_iter().rev().flatten().collect()
    }
}

/// The DFSORT k-way merge of TRANREPT `STEP05R` — P-TRANREPT-002: the records of sorted runs in
/// key order, of equal keys the record of the earlier run first, so merging runs given in input
/// order yields one stable sort of their records, the order
/// `transaction_report::select_for_report` gives. It holds one record and one buffered reader of
/// each run that is not yet exhausted; the runs themselves, and their work files, belong to the
/// caller.
struct Merge<'a> {
    /// The head of every run not yet exhausted, the least first.
    heads: BinaryHeap<Reverse<MergeHead<'a>>>,
}

impl<'a> Merge<'a> {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: the merge of `runs`, given in input order, reading
    /// the first record of each.
    ///
    /// # Errors
    ///
    /// A setup failure (exit 16) when a run cannot be read ([`RunReader::next_record`]).
    fn new(runs: &'a [Run]) -> Result<Self, ExitStatus> {
        let mut heads = BinaryHeap::with_capacity(runs.len());
        for (index, run) in runs.iter().enumerate() {
            let mut reader = RunReader::new(run);
            if let Some(record) = reader.next_record()? {
                heads.push(Reverse(MergeHead::new(index, record, reader)));
            }
        }
        Ok(Self { heads })
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002: the next record in the merged order, or `None` once
    /// every run is exhausted.
    ///
    /// # Errors
    ///
    /// A setup failure (exit 16) when a run cannot be read ([`RunReader::next_record`]).
    fn next_record(&mut self) -> Result<Option<TransactionRecord>, ExitStatus> {
        let Some(Reverse(mut head)) = self.heads.pop() else {
            return Ok(None);
        };
        match head.reader.next_record()? {
            Some(next) => {
                let record = mem::replace(&mut head.record, next);
                head.key = spilled_key(&head.record);
                self.heads.push(Reverse(head));
                Ok(Some(record))
            }
            None => Ok(Some(head.record)),
        }
    }
}

/// The first record of one run of a [`Merge`] not yet merged, with the run's key for it, the
/// run's position in input order and the rest of the run. Heads compare by key, then by run
/// position, never by record.
struct MergeHead<'a> {
    /// The key of `record`.
    key: ReportSortKey,
    /// The run's position among the runs merged, in input order.
    run: usize,
    /// The record.
    record: TransactionRecord,
    /// The rest of the run.
    reader: RunReader<'a>,
}

impl<'a> MergeHead<'a> {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: the head `record` of the run at position `run`,
    /// whose rest `reader` reads.
    fn new(run: usize, record: TransactionRecord, reader: RunReader<'a>) -> Self {
        Self {
            key: spilled_key(&record),
            run,
            record,
            reader,
        }
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002, E-38: the place of the head in the merge, its key,
    /// then its run's position.
    fn place(&self) -> (ReportSortKey, usize) {
        (self.key, self.run)
    }
}

impl PartialEq for MergeHead<'_> {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: heads are equal when their places are.
    fn eq(&self, other: &Self) -> bool {
        self.place() == other.place()
    }
}

impl Eq for MergeHead<'_> {}

impl PartialOrd for MergeHead<'_> {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: the total order of [`MergeHead::cmp`].
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeHead<'_> {
    /// TRANREPT `STEP05R` — P-TRANREPT-002, E-38: heads order by their places, so of equal keys
    /// the head of the earlier run comes first.
    fn cmp(&self, other: &Self) -> Ordering {
        self.place().cmp(&other.place())
    }
}

/// TRANREPT `STEP05R` — P-TRANREPT-002: the key `transaction_report::report_sort_key` gave
/// `record` before it was spilled. A run holds only records the selection kept, so the key is
/// their card number and TRAN-ID.
fn spilled_key(record: &TransactionRecord) -> ReportSortKey {
    ReportSortKey {
        card_number: record.transaction.tran_card_num,
        tran_id: record.transaction.tran_id,
    }
}

/// The records of one [`Run`], read back from the start of its work file through a buffer, one
/// [`RUN_FRAME`]-byte frame at a time.
struct RunReader<'a> {
    /// The run's bytes on its work file.
    reader: WorkReader<'a>,
    /// One frame: the record bytes and the `LF` after them.
    frame: Vec<u8>,
}

impl<'a> RunReader<'a> {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: a reader of `run` from its first record.
    fn new(run: &'a Run) -> Self {
        Self {
            reader: run.work.reader(0, run.len),
            frame: vec![0; RUN_FRAME],
        }
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002: the next record of the run, as
    /// [`write_work_file`] wrote it, or `None` after its last frame.
    ///
    /// # Errors
    ///
    /// A setup failure (exit 16) when the work file cannot be read, ends inside a frame, or holds
    /// a frame that does not read back as written: `LF` missing after the record bytes, or a
    /// record that does not decode.
    fn next_record(&mut self) -> Result<Option<TransactionRecord>, ExitStatus> {
        let read_failure = |err| setup_failure(SORTWK_READ_FAILED, err);
        if !self
            .reader
            .read_frame(&mut self.frame)
            .map_err(read_failure)?
        {
            return Ok(None);
        }
        let (body, separator) = self.frame.split_at(TransactionLayout::LEN);
        if separator != b"\n" {
            let err = self
                .reader
                .invalid_data("a sorted-run frame does not end with LF");
            return Err(read_failure(err));
        }
        TransactionLayout::decode(body, Charset::Ascii)
            .map(Some)
            .map_err(|err| read_failure(self.reader.invalid_data(err)))
    }
}

/// The DFSORT `SORTOUT` and work-file writes of TRANREPT `STEP05R` — P-TRANREPT-002: every record
/// `next` yields, in that order, through `append`, at most `chunk` records per append. Returns the
/// number of records written.
///
/// # Errors
///
/// The first error of `next` or `append`.
fn write_chunked(
    mut next: impl FnMut() -> Result<Option<TransactionRecord>, ExitStatus>,
    chunk: usize,
    mut append: impl FnMut(&[TransactionRecord]) -> Result<(), ExitStatus>,
) -> Result<usize, ExitStatus> {
    let mut pending = Vec::with_capacity(chunk);
    let mut written = 0;
    while let Some(record) = next()? {
        pending.push(record);
        if pending.len() >= chunk {
            append(&pending)?;
            written += pending.len();
            pending.clear();
        }
    }
    if !pending.is_empty() {
        append(&pending)?;
        written += pending.len();
    }
    Ok(written)
}

/// The stop for a record the `STEP05R` sort cannot place: its card number holds a byte other than
/// `0` to `9`, whose zoned-decimal position no audit row pins, so the job stops with exit 16
/// rather than guess where DFSORT puts the record. The text names the TRAN-ID, never the card
/// number. SORT passes no `CEE3ABD` code, so no `ABEND` line follows it.
fn unpinned_sort_key(err: &UnpinnedSortKey) -> ExitStatus {
    tracing::error!(error = %err, "TRANSACT.DALY(+1) not written");
    ExitStatus::abend_from(err, None)
}

/// TRANREPT `STEP10R` `EXEC PGM=CBTRN03C` [app/jcl/TRANREPT.jcl:L59-80] — P-TRANREPT-003,
/// P-CBTRN03C-001, P-CBTRN03C-002: the report of `daly` over `start` to `end`.
///
/// Opens the program's files in the order of its `PROCEDURE DIVISION`
/// [app/cbl/CBTRN03C.cbl:L161-166]: `TRANFILE` ([`open_tranfile`]), then `TRANREPT(+1)`
/// (`LRECL=133`, [`open_reptfile`]), then, in [`report_program`], the three lookup files. It
/// writes the report into `TRANREPT(+1)` and catalogs it once the program has closed its files,
/// so a failure while the files close leaves no report cataloged, as the abnormal end of the step
/// deletes it.
///
/// The reads, the allocation, the catalog and the deletion of an uncataloged `TRANREPT(+1)` run
/// on Tokio's blocking pool, inside the job span, as do the appends of [`report_program`].
///
/// # Errors
///
/// - A CBTRN03C abend, exit 16 with `ABEND 999`: a file that cannot be opened
///   ([`open_tranfile`], [`open_reptfile`], [`open_lookup_files`]), a failed lookup
///   ([`lookup_abend`]), a `TRANFILE` record that cannot be read ([`tranfile_read_abend`]), or a
///   `TRANREPT(+1)` that cannot be written or closed ([`file_abend`]).
/// - A failure to begin or end the unit of work is a setup failure (exit 16): it is the database
///   connection itself, which no COBOL file statement has.
///
/// On every error `TRANREPT(+1)` is left uncataloged and deleted, with every line already written
/// to it, since `DISP=(NEW,CATLG,DELETE)` [app/jcl/TRANREPT.jcl:L76-80] catalogs it only when the
/// step ends normally. The header blocks of CBTRN03C `1120-WRITE-HEADERS`
/// [app/cbl/CBTRN03C.cbl:L324-341] — P-CBTRN03C-475, four writes each ([`write_lines`]), are
/// therefore deleted with the rest of the report on a later failure.
async fn report_step(
    ctx: &JobContext,
    db: &Db,
    daly: &Generation,
    start: Alnum<10>,
    end: Alnum<10>,
) -> Result<(), ExitStatus> {
    let tranfile = open_tranfile(daly).await?;
    let mut reptfile = open_reptfile(ctx).await?;
    let (records, lines) = match report_program(db, tranfile, start, end, &mut reptfile).await {
        Ok(counts) => counts,
        Err(status) => {
            reptfile.discard_async().await;
            return Err(status);
        }
    };
    let report = reptfile
        .catalog_async()
        .await
        .map_err(|err| file_abend(ERROR_CLOSING_REPORT_FILE, "TRANREPT(+1) close failed", &err))?;
    tracing::info!(
        generation = %report,
        records,
        lines,
        "{END_OF_EXECUTION_OF_PROGRAM_CBTRN03C}"
    );
    Ok(())
}

/// CBTRN03C `0000-TRANFILE-OPEN` [app/cbl/CBTRN03C.cbl:L376-392] — P-CBTRN03C-468, the first of
/// the six opens: `OPEN INPUT` of `TRANSACT.DALY(+1)`, the program's `TRANFILE` and its only
/// input of transactions, on Tokio's blocking pool, before any other file is opened.
///
/// A file that cannot be opened fails the `OPEN`. Its records are read only once every file is
/// open, each by the `READ` of `1000-TRANFILE-GET-NEXT` (P-CBTRN03C-474, [`report_records`]), so
/// a record that cannot be read or decoded is the failure of that `READ`.
///
/// # Errors
///
/// The `ERROR OPENING TRANFILE` abend ([`tranfile_open_abend`]) when the file cannot be opened.
async fn open_tranfile(daly: &Generation) -> Result<TranFile, ExitStatus> {
    TranFile::open(daly)
        .await
        .map_err(|err| tranfile_open_abend(&err))
}

/// CBTRN03C `0100-REPTFILE-OPEN` [app/cbl/CBTRN03C.cbl:L394-410] — P-CBTRN03C-469: `OPEN OUTPUT`
/// of DD `TRANREPT`, the new generation `TRANREPT(+1)` [app/jcl/TRANREPT.jcl:L76-80], allocated
/// on Tokio's blocking pool.
///
/// Any failure to allocate it abends with `ERROR OPENING REPTFILE` and status 30
/// ([`PERMANENT_ERROR_STATUS`]), as the POSTTRAN open of `DALYREJS(+1)` does: IBM's table gives
/// `OPEN OUTPUT` no "file not present" status, and a file-system error has no QSAM status of its
/// own. A held allocation lock ([`GenerationError::InUse`]) puts the error's text, one line
/// naming the lock file and the process it records, before the `DISPLAY` lines, as the
/// allocation message precedes the program's output in the job log.
///
/// # Errors
///
/// The `ERROR OPENING REPTFILE` abend when `TRANREPT(+1)` cannot be allocated.
async fn open_reptfile(ctx: &JobContext) -> Result<PendingGeneration, ExitStatus> {
    ctx.generations
        .begin_next_async(Dataset::TranRept)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "TRANREPT(+1) allocation failed");
            let cause =
                matches!(err, GenerationError::InUse { .. }).then(|| one_line(&err.to_string()));
            ExitStatus::Abend {
                lines: cause
                    .into_iter()
                    .chain(abend_display(
                        ERROR_OPENING_REPTFILE.to_owned(),
                        PERMANENT_ERROR_STATUS,
                    ))
                    .collect(),
                abcode: Some(ABCODE),
            }
        })
}

/// CBTRN03C `0200-CARDXREF-OPEN`, `0300-TRANTYPE-OPEN` and `0400-TRANCATG-OPEN`
/// [app/cbl/CBTRN03C.cbl:L412-464] — P-CBTRN03C-470 to P-CBTRN03C-472: `OPEN INPUT` of the
/// three lookup files, in that order, in the unit of work `uow`.
///
/// A table has no `OPEN` of its own, so each is opened by the random `READ` its lookup issues,
/// for a key of spaces, outside the [`ReportRun`] and its memo: the table is open when the store
/// answers with a record or with [`PortError::NotFound`]. Any other answer fails the `OPEN`, with
/// the paragraph's `DISPLAY` and status 30, the database failure without further information, so
/// a lookup table that cannot be read abends before any `TRANFILE` record is read.
///
/// `0500-DATEPARM-OPEN` [app/cbl/CBTRN03C.cbl:L466-482] — P-CBTRN03C-473, the last open, has no
/// counterpart: the `DATEPARM` record is the job's `--start-date` and `--end-date` (E-34), which
/// no file holds.
///
/// # Errors
///
/// The open abend of the first table that cannot be read.
async fn open_lookup_files(uow: &mut UnitOfWork) -> Result<(), ExitStatus> {
    let mut store = uow.store();
    let blank_type = TranTypeCode::move_from("");
    let xref = store.read_xref_by_card(CardNumber::move_from("")).await;
    opened(xref, ERROR_OPENING_CROSS_REF_FILE, "CARDXREF open failed")?;
    let tran_type = store.read_tran_type_file(blank_type).await;
    opened(
        tran_type,
        ERROR_OPENING_TRANSACTION_TYPE_FILE,
        "TRANTYPE open failed",
    )?;
    let category = store
        .read_tran_category_file(blank_type, OPEN_CATEGORY)
        .await;
    opened(
        category,
        ERROR_OPENING_TRANSACTION_CATG_FILE,
        "TRANCATG open failed",
    )
}

/// The outcome of the `OPEN` that [`open_lookup_files`] made by the `READ` `read`: open on a
/// record or [`PortError::NotFound`], otherwise the abend after `display` with status 30. The
/// event logs `what` and the store's answer.
fn opened<T>(
    read: PortResult<T>,
    display: &'static str,
    what: &'static str,
) -> Result<(), ExitStatus> {
    match read {
        Ok(_) | Err(PortError::NotFound) => Ok(()),
        Err(err) => {
            tracing::error!(error = %err, "{what}");
            Err(program_abend(display.to_owned(), PERMANENT_ERROR_STATUS))
        }
    }
}

/// CBTRN03C `PROCEDURE DIVISION` [app/cbl/CBTRN03C.cbl:L159-217] — P-CBTRN03C-001,
/// P-CBTRN03C-002: the report of `tranfile` over `start` to `end`, written to `reptfile`, from the
/// lookup opens to the closes. Returns the number of `TRANFILE` records and of report lines
/// written.
///
/// Opens a read-only unit of work for the `CARDXREF`, `TRANTYPE` and `TRANCATG` lookups, opens the
/// three files in it ([`open_lookup_files`]), and starts the [`ReportRun`] with the `DATEPARM`
/// dates (CBTRN03C `0550-DATEPARM-READ` [app/cbl/CBTRN03C.cbl:L220-243] — P-CBTRN03C-467). The
/// repository holds no content for the `DATEPARM` dataset [app/jcl/TRANREPT.jcl:L73-74], so
/// `start` and `end`, the job's `--start-date` and `--end-date`, stand in for its record: the same
/// dates the `STEP05R` sort selects by, as the CR00 job writes them into both `SYMNAMES` and
/// `DATEPARM` (E-34, resolved, whose disposition uses the start and end dates).
///
/// [`report_records`] reports every record and [`ReportRun::finish`] adds the end-of-input
/// totals. The unit of work is then rolled back, since it writes nothing, before the caller
/// catalogs `reptfile`.
///
/// # Errors
///
/// The errors [`report_step`] lists, except the `TRANFILE` and `TRANREPT(+1)` opens and the
/// catalog of `TRANREPT(+1)`. `reptfile` is left to the caller, which discards it.
async fn report_program(
    db: &Db,
    mut tranfile: TranFile,
    start: Alnum<10>,
    end: Alnum<10>,
    reptfile: &mut PendingGeneration,
) -> Result<(usize, usize), ExitStatus> {
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure("CARDXREF, TRANTYPE and TRANCATG open failed", err))?;
    open_lookup_files(&mut uow).await?;
    tracing::info!(
        start_date = %start.as_text(),
        end_date = %end.as_text(),
        "report date range read"
    );
    let mut run = ReportRun::new(start, end);
    let (records, mut lines) = report_records(&mut run, &mut uow, &mut tranfile, reptfile).await?;
    let totals = run.finish();
    lines += totals.len();
    write_lines(reptfile, totals).await?;
    uow.rollback()
        .await
        .map_err(|err| setup_failure("CARDXREF, TRANTYPE and TRANCATG close failed", err))?;
    Ok((records, lines))
}

/// CBTRN03C main loop [app/cbl/CBTRN03C.cbl:L170-206] — P-CBTRN03C-003, P-CBTRN03C-004,
/// P-CBTRN03C-474: every `TRANFILE` record, in file order, through [`ReportRun::process`], each
/// line it returns written to `reptfile` as it comes. Returns the number of `TRANFILE` records
/// read and of lines written.
///
/// The `READ`s of `1000-TRANFILE-GET-NEXT` [app/cbl/CBTRN03C.cbl:L248-272] take the records
/// [`TRANFILE_CHUNK_RECORDS`] at a time from Tokio's blocking pool, so the step holds one chunk
/// of `TRANSACT.DALY(+1)`, never the whole file. A record that cannot be read fails its own
/// `READ`, after every record before it was reported, as the sequential `READ` loop meets it.
///
/// Each record becomes a [`Transaction`] whose row identity is its record number, counted from 1:
/// the file holds `CVTRA05Y` bytes only, and CBTRN03C reads no row identity.
///
/// The run reads each distinct `TRANTYPE` and `TRANCATG` key once (`1500-B-LOOKUP-TRANTYPE`,
/// `1500-C-LOOKUP-TRANCATG` — P-CBTRN03C-005) and each card's `CARDXREF` record once at its card
/// break, so the job issues one lookup per distinct key rather than two per record.
///
/// # Errors
///
/// The abend of a record that cannot be read ([`tranfile_read_abend`]), of a failed lookup
/// ([`lookup_abend`]) or of a failed write ([`write_lines`]).
async fn report_records(
    run: &mut ReportRun,
    uow: &mut UnitOfWork,
    tranfile: &mut TranFile,
    reptfile: &mut PendingGeneration,
) -> Result<(usize, usize), ExitStatus> {
    let mut store = uow.store();
    let mut numbers = record_numbers();
    let mut records = 0;
    let mut written = 0;
    loop {
        let chunk = tranfile
            .next_chunk(TRANFILE_CHUNK_RECORDS)
            .await
            .map_err(|err| tranfile_read_abend(&err))?;
        if chunk.is_empty() {
            return Ok((records, written));
        }
        records += chunk.len();
        // The chunk leads the zip, so its end takes no record number from `numbers`.
        for (record, number) in chunk.into_iter().zip(&mut numbers) {
            let tran = Transaction::from_new(number, record.transaction);
            let lines = run
                .process(&mut store, &tran)
                .await
                .map_err(|abend| lookup_abend(&abend))?;
            let count = lines.len();
            write_lines(reptfile, lines).await?;
            written += count;
        }
    }
}

/// CBTRN03C `1111-WRITE-REPORT-REC` [app/cbl/CBTRN03C.cbl:L343-359] — P-CBTRN03C-476: `lines`
/// appended to `TRANREPT(+1)` as 133-byte records, checked, encoded and written on Tokio's
/// blocking pool.
///
/// `lines` are the lines a [`ReportRun`] returns, header blocks included, so this is also where
/// the job writes CBTRN03C `1120-WRITE-HEADERS` [app/cbl/CBTRN03C.cbl:L324-341] —
/// P-CBTRN03C-475: the four writes `REPORT-NAME-HEADER`, `WS-BLANK-LINE`, `TRANSACTION-HEADER-1`
/// and `TRANSACTION-HEADER-2` of each block. Like every other line they reach only the
/// uncataloged `TRANREPT(+1)`, which a later failure of the step deletes ([`report_step`]).
///
/// # Errors
///
/// The `ERROR WRITING REPTFILE` abend ([`file_abend`]) when a line cannot be written.
async fn write_lines(
    reptfile: &mut PendingGeneration,
    lines: Vec<String>,
) -> Result<(), ExitStatus> {
    reptfile
        .append_lines_async(lines, REPORT_RECORD_LEN)
        .await
        .map_err(|err| file_abend(ERROR_WRITING_REPTFILE, "TRANREPT(+1) write failed", &err))
}

/// The record numbers 1, 2, … as row identities, for the records of a file that carries none.
fn record_numbers() -> impl Iterator<Item = TransactionRowId> {
    // A record file holds far fewer than `i64::MAX` records, since every record takes 351 bytes
    // of a file system, so the numbering never reaches the end of the range.
    (1_i64..).map(TransactionRowId::from)
}

/// CBTRN03C `1500-A-LOOKUP-XREF`, `1500-B-LOOKUP-TRANTYPE` and `1500-C-LOOKUP-TRANCATG` on
/// `INVALID KEY` [app/cbl/CBTRN03C.cbl:L484-512] — P-CBTRN03C-005, P-CBTRN03C-485: the literal
/// and the key, then status 23 and the abend.
///
/// Every failed lookup the rule reports displays 23, the literal the paragraphs move, whatever
/// the store answered. The event carries the paragraph and the store's answer but not the key, so
/// a card number stays out of the logs and appears only in the job's stderr display, the
/// counterpart of the COBOL job log.
fn lookup_abend(abend: &ReportAbend) -> ExitStatus {
    tracing::error!(
        paragraph = abend.paragraph,
        error = %abend,
        cause = %abend.cause,
        "CBTRN03C lookup failed"
    );
    program_abend(abend.display_line(), INVALID_KEY_STATUS)
}

/// CBTRN03C `0000-TRANFILE-OPEN` [app/cbl/CBTRN03C.cbl:L376-392] — P-CBTRN03C-468, when
/// `TRANSACT.DALY(+1)` cannot be opened: `ERROR OPENING TRANFILE`.
fn tranfile_open_abend(err: &GenerationError) -> ExitStatus {
    file_abend(ERROR_OPENING_TRANFILE, "TRANSACT.DALY(+1) open failed", err)
}

/// CBTRN03C `1000-TRANFILE-GET-NEXT` [app/cbl/CBTRN03C.cbl:L248-272] — P-CBTRN03C-474, when a
/// record of `TRANSACT.DALY(+1)` cannot be read or does not decode: `ERROR READING TRANSACTION
/// FILE`.
fn tranfile_read_abend(err: &GenerationError) -> ExitStatus {
    file_abend(
        ERROR_READING_TRANSACTION_FILE,
        "TRANSACT.DALY(+1) read failed",
        err,
    )
}

/// The abend of a CBTRN03C file statement that failed with `err`: the paragraph's `display`,
/// then status 30 ([`PERMANENT_ERROR_STATUS`]). The event logs `what` and the error.
fn file_abend(display: &'static str, what: &'static str, err: &GenerationError) -> ExitStatus {
    tracing::error!(error = %err, "{what}");
    program_abend(display.to_owned(), PERMANENT_ERROR_STATUS)
}

/// CBTRN03C `9910-DISPLAY-IO-STATUS` and `9999-ABEND-PROGRAM` [app/cbl/CBTRN03C.cbl:L626-646] —
/// P-CBTRN03C-484, P-CBTRN03C-485: the abend after the paragraph's `display`, whose stderr lines
/// are `display`, `FILE STATUS IS: NNNN` with `status`, `ABENDING PROGRAM` and `ABEND 999`, with
/// exit 16.
///
/// The lines are composed here rather than taken from the `Display` of a domain error, because
/// every abend path displays the file status text before `ABENDING PROGRAM`, and the report
/// rule's error renders only its paragraph and literal.
fn program_abend(display: String, status: [u8; 2]) -> ExitStatus {
    ExitStatus::Abend {
        lines: abend_display(display, status),
        abcode: Some(ABCODE),
    }
}

/// The `DISPLAY` lines of a CBTRN03C abend: `display`, `FILE STATUS IS: NNNN` with `status`
/// (`9910-DISPLAY-IO-STATUS`) and `ABENDING PROGRAM` (`9999-ABEND-PROGRAM`).
fn abend_display(display: String, status: [u8; 2]) -> Vec<String> {
    vec![
        display,
        file_status_message(status),
        ABENDING_PROGRAM.to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::{fs, io};

    use carddemo_codec::layouts::TransactionLayout;
    use carddemo_codec::{Charset, Layout};
    use carddemo_domain::format::REPORT_RECORD_LEN;
    use carddemo_domain::messages::{
        ABENDING_PROGRAM, ERROR_OPENING_CROSS_REF_FILE, ERROR_OPENING_TRANFILE,
        ERROR_OPENING_TRANSACTION_CATG_FILE, ERROR_OPENING_TRANSACTION_TYPE_FILE,
        ERROR_READING_TRANSACTION_FILE, ERROR_WRITING_REPTFILE, FILE_STATUS_IS,
        INVALID_CARD_NUMBER, INVALID_TRANSACTION_TYPE,
    };
    use carddemo_domain::numeric::{Alnum, CardNumber, TranId, TranTypeCode, TransactionRowId};
    use carddemo_domain::ports::PortError;
    use carddemo_domain::rules::transaction_report::{
        LookupKey, ReportAbend, Transaction, UnpinnedSortKey, select_for_report,
    };

    use super::{
        JobCounts, SortLimits, TranFile, file_abend, lookup_abend, opened, record_numbers,
        sort_step_within, tranfile_open_abend, tranfile_read_abend, write_lines,
    };
    use crate::exit::ExitStatus;
    use crate::generations::{Dataset, Generation, GenerationError, Generations};

    /// The TRANREPT default range [app/jcl/TRANREPT.jcl:L44-45] (P-TRANREPT-002).
    const START: &str = "2022-01-01";
    const END: &str = "2022-07-06";

    /// One 350-byte `CVTRA05Y` record in ASCII, each field padded with spaces to its width
    /// [app/cpy/CVTRA05Y.cpy:L4-18]; the processing timestamp is `proc_date` followed by the
    /// time POSTTRAN stamps (P-CBTRN02C-216).
    fn record_bytes(tran_id: &str, card: &str, proc_date: &str, filler: &str) -> Vec<u8> {
        let proc_ts = format!("{proc_date}-12.00.00.000000");
        let fields = [
            (tran_id, 16),
            ("01", 2),
            ("0001", 4),
            ("POS TERM", 10),
            ("Purchase at Example Store", 100),
            ("0000001000{", 11),
            ("800000001", 9),
            ("Example Store", 50),
            ("Example City", 50),
            ("12345", 10),
            (card, 16),
            ("2022-06-10 19:27:53.000000", 26),
            (proc_ts.as_str(), 26),
            (filler, 20),
        ];
        let bytes: Vec<u8> = fields
            .iter()
            .flat_map(|&(text, width)| format!("{text:<width$}").into_bytes())
            .collect();
        assert_eq!(bytes.len(), TransactionLayout::LEN, "{tran_id}");
        bytes
    }

    /// `records` as a record file: each record followed by `LF`.
    fn framed(records: &[Vec<u8>]) -> Vec<u8> {
        records
            .iter()
            .flat_map(|record| record.iter().copied().chain([b'\n']))
            .collect()
    }

    /// The `TRANSACT.DALY(+1)` the in-memory selection gives for the backup `records`: the
    /// records `select_for_report` keeps, in its order, each one copied whole.
    fn selected_bytes(records: &[Vec<u8>]) -> Vec<u8> {
        let transactions = records.iter().zip(1_i64..).map(|(bytes, number)| {
            let record = TransactionLayout::decode(bytes, Charset::Ascii)
                .unwrap_or_else(|e| panic!("test record does not decode: {e}"));
            Transaction::from_new(TransactionRowId::from(number), record.transaction)
        });
        let selected = select_for_report(
            &Alnum::move_from(START),
            &Alnum::move_from(END),
            transactions,
        )
        .expect("every selected card number holds digits only");
        let kept: Vec<Vec<u8>> = selected
            .iter()
            .map(|tran| {
                let index = usize::try_from(tran.transaction_id.get() - 1)
                    .expect("record numbers count from 1");
                records[index].clone()
            })
            .collect();
        framed(&kept)
    }

    /// The bounds the sort tests run within: the job's, and tiny ones that spill every one or
    /// two kept records and merge two or three runs at a time, so spills, the merge levels of
    /// [`Runs`], the intermediate passes of [`reduce_runs`] and chunked appends all run.
    const LIMITS: [SortLimits; 4] = [
        SortLimits::DEFAULT,
        SortLimits {
            run_records: 1,
            fan_in: 2,
            write_records: 1,
        },
        SortLimits {
            run_records: 2,
            fan_in: 2,
            write_records: 3,
        },
        SortLimits {
            run_records: 2,
            fan_in: 3,
            write_records: 2,
        },
    ];

    /// The batch directory after a sort that ended normally: the backup and the sort output,
    /// with no work file, temporary file or lock.
    const CATALOGED: [&str; 2] = ["TRANSACT.BKUP.G0001V00", "TRANSACT.DALY.G0001V00"];

    /// The batch directory after a sort that stopped or failed: the backup alone.
    const BKUP_ONLY: [&str; 1] = ["TRANSACT.BKUP.G0001V00"];

    /// What one run of the sort step over a test backup left behind.
    struct Sorted {
        /// The step's outcome, with the bytes of the `TRANSACT.DALY(+1)` it cataloged.
        daly: Result<Vec<u8>, ExitStatus>,
        /// The `read` count the step set.
        read: u64,
        /// The file names left in the batch directory, sorted.
        names: Vec<String>,
    }

    /// Runs [`sort_step_within`] within `limits`, over the default range, on a
    /// `TRANSACT.BKUP(+1)` holding `bkup` in a batch directory of its own named after `test`,
    /// which is removed afterwards.
    fn sort_backup(test: &str, bkup: &[u8], limits: SortLimits) -> Sorted {
        let dir = std::env::temp_dir().join(format!(
            "carddemo-tranrept-sort-{test}-{}",
            std::process::id()
        ));
        if let Err(e) = fs::remove_dir_all(&dir) {
            assert!(
                e.kind() == io::ErrorKind::NotFound,
                "stale {} is not removable: {e}",
                dir.display()
            );
        }
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        let path = dir.join("TRANSACT.BKUP.G0001V00");
        fs::write(&path, bkup).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        let backup = Generation {
            dataset: Dataset::TransactBkup,
            number: 1,
            path,
        };
        let counts = JobCounts::default();
        let daly = sort_step_within(
            &Generations::new(&dir),
            &counts,
            &backup,
            &Alnum::move_from(START),
            &Alnum::move_from(END),
            limits,
        )
        .map(|daly| fs::read(&daly.path).unwrap_or_else(|e| panic!("read {daly}: {e}")));
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("list {}: {e}", dir.display()))
            .map(|entry| {
                entry
                    .expect("directory entry is readable")
                    .file_name()
                    .into_string()
                    .expect("file name is UTF-8")
            })
            .collect();
        names.sort();
        fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
        Sorted {
            daly,
            read: counts.read(),
            names,
        }
    }

    /// The stderr lines of a CBTRN03C abend after `display` with file status `digits`, as the
    /// audit records them (P-CBTRN03C-484, P-CBTRN03C-485).
    fn abend_lines(display: &str, digits: &str) -> Vec<String> {
        vec![
            display.to_owned(),
            format!("{FILE_STATUS_IS}00{digits}"),
            ABENDING_PROGRAM.to_owned(),
            "ABEND 999".to_owned(),
        ]
    }

    #[test]
    fn the_sort_keeps_the_range_orders_by_card_then_tran_id_and_copies_records_whole() {
        let in_card_2_late = record_bytes("0000000000000003", "4111000000000002", "2022-07-01", "");
        let in_card_2_early = record_bytes(
            "0000000000000001",
            "4111000000000002",
            END,
            "FILLER BYTES KEPT",
        );
        let in_card_1 = record_bytes("0000000000000002", "4111000000000001", START, "");
        let after_range = record_bytes("0000000000000004", "4111000000000000", "2022-07-07", "");
        let before_range = record_bytes("0000000000000005", "4111000000000000", "2021-12-31", "");
        let sortin = framed(&[
            in_card_2_late.clone(),
            after_range,
            in_card_2_early.clone(),
            before_range,
            in_card_1.clone(),
        ]);
        let expected = framed(&[in_card_1, in_card_2_early, in_card_2_late]);
        for (index, limits) in LIMITS.into_iter().enumerate() {
            let sorted = sort_backup(&format!("range-{index}"), &sortin, limits);
            assert_eq!(sorted.daly, Ok(expected.clone()), "{limits:?}");
            assert_eq!(sorted.read, 5, "{limits:?}");
            assert_eq!(sorted.names, CATALOGED, "{limits:?}: no work file is left");
        }
    }

    /// Within every bound, the external sort writes exactly the records of the in-memory
    /// selection `select_for_report`, in its order, records of equal keys in input order, each
    /// copied whole, and leaves no work file behind.
    #[test]
    fn the_external_sort_writes_the_in_memory_selection_within_every_bound() {
        let dates = [
            "2021-12-31",
            START,
            "2022-03-15",
            "2022-07-01",
            END,
            "2022-07-07",
            "2022-07-01",
        ];
        // TRAN-ID repeats every 5 records and the card every 3, so records 15 apart share a key.
        let records: Vec<Vec<u8>> = (0..40_usize)
            .map(|i| {
                record_bytes(
                    &format!("{:016}", i % 5),
                    &format!("41110000000000{:02}", i % 3),
                    dates[i % dates.len()],
                    &format!("RECORD {i}"),
                )
            })
            .collect();
        let expected = selected_bytes(&records);
        assert!(
            expected.len() > 20 * (TransactionLayout::LEN + 1),
            "the tiny bounds spill many runs"
        );
        for (index, limits) in LIMITS.into_iter().enumerate() {
            let sorted = sort_backup(&format!("merge-{index}"), &framed(&records), limits);
            assert_eq!(sorted.daly, Ok(expected.clone()), "{limits:?}");
            assert_eq!(sorted.read, 40, "{limits:?}");
            assert_eq!(sorted.names, CATALOGED, "{limits:?}: no work file is left");
        }
    }

    /// A card number with a non-digit byte stops the sort only inside the range, after every
    /// backup record is read and every run spilled before it is dropped, and shows no card
    /// number; a backup record that does not decode fails the step first, even after such a
    /// record, and leaves `read` at 0. A stopped or failed sort catalogs no `TRANSACT.DALY(+1)`.
    #[test]
    fn the_sort_stops_on_a_non_digit_card_only_inside_the_range_after_reading_the_backup() {
        let card = "4111X00000000001";
        let good =
            |tran_id: &str, card_number: &str| record_bytes(tran_id, card_number, "2022-07-01", "");
        let with_card_on = |proc_date: &str| {
            vec![
                good("0000000000000003", "4111000000000002"),
                good("0000000000000002", "4111000000000001"),
                record_bytes("0000000000000009", card, proc_date, ""),
                good("0000000000000001", "4111000000000001"),
            ]
        };
        let stop = ExitStatus::abend_from(
            &UnpinnedSortKey {
                tran_id: TranId::move_from("0000000000000009"),
            },
            None,
        );
        let mut undecodable = framed(&with_card_on("2022-07-01"));
        undecodable.extend_from_slice(b"PARTIAL");
        let decode_failure = ExitStatus::setup(
            "record file decode failed: CVTRA05Y record 5: final frame of 7 bytes is shorter \
             than the record length plus LF",
        );
        let outside = with_card_on("2022-07-07");
        let cases = [
            (
                framed(&with_card_on("2022-07-01")),
                Err(stop),
                4_u64,
                &BKUP_ONLY[..],
            ),
            (
                framed(&outside),
                Ok(selected_bytes(&outside)),
                4,
                &CATALOGED[..],
            ),
            (undecodable, Err(decode_failure), 0, &BKUP_ONLY[..]),
        ];
        for (case, (bkup, expected, read, names)) in cases.into_iter().enumerate() {
            for (index, limits) in LIMITS.into_iter().enumerate() {
                let sorted = sort_backup(&format!("stop-{case}-{index}"), &bkup, limits);
                assert_eq!(sorted.daly, expected, "case {case}, {limits:?}");
                assert_eq!(sorted.read, read, "case {case}, {limits:?}");
                assert_eq!(sorted.names, names, "case {case}, {limits:?}");
                if let Err(status) = &sorted.daly {
                    assert_eq!(status.code(), 16);
                    assert!(
                        status.render().iter().all(|line| !line.contains(card)),
                        "the stop must not show the card number: {:?}",
                        status.render()
                    );
                }
            }
        }
    }

    #[test]
    fn every_cbtrn03c_abend_displays_its_text_the_file_status_and_abend_999() {
        let Err(codec) = TransactionLayout::decode(b"short", Charset::Ascii) else {
            panic!("a 5-byte record must not decode");
        };
        let card = "4111000000000050";
        let cases = [
            (
                lookup_abend(&ReportAbend {
                    paragraph: "1500-A-LOOKUP-XREF",
                    display: INVALID_CARD_NUMBER,
                    key: LookupKey::Card(CardNumber::move_from(card)),
                    cause: PortError::NotFound,
                }),
                abend_lines(&format!("{INVALID_CARD_NUMBER}{card}"), "23"),
            ),
            (
                lookup_abend(&ReportAbend {
                    paragraph: "1500-B-LOOKUP-TRANTYPE",
                    display: INVALID_TRANSACTION_TYPE,
                    key: LookupKey::TranType(TranTypeCode::move_from("09")),
                    cause: PortError::Unavailable("connection reset".to_owned()),
                }),
                abend_lines(&format!("{INVALID_TRANSACTION_TYPE}09"), "23"),
            ),
            (
                tranfile_read_abend(&GenerationError::Codec(codec)),
                abend_lines(ERROR_READING_TRANSACTION_FILE, "30"),
            ),
            (
                tranfile_read_abend(&GenerationError::Io {
                    path: PathBuf::from("TRANSACT.DALY.G0001V00"),
                    source: io::Error::other("unreadable"),
                }),
                abend_lines(ERROR_READING_TRANSACTION_FILE, "30"),
            ),
            (
                tranfile_open_abend(&GenerationError::Io {
                    path: PathBuf::from("TRANSACT.DALY.G0001V00"),
                    source: io::Error::other("not found"),
                }),
                abend_lines(ERROR_OPENING_TRANFILE, "30"),
            ),
            (
                file_abend(
                    ERROR_WRITING_REPTFILE,
                    "TRANREPT(+1) write failed",
                    &GenerationError::Io {
                        path: PathBuf::from("TRANREPT.G0001V00"),
                        source: io::Error::other("disk full"),
                    },
                ),
                abend_lines(ERROR_WRITING_REPTFILE, "30"),
            ),
        ];
        for (status, expected) in cases {
            assert_eq!(status.code(), 16, "{expected:?}");
            assert_eq!(status.render(), expected);
        }
    }

    #[test]
    fn a_lookup_table_is_open_on_a_record_or_not_found_and_abends_on_any_other_answer() {
        assert_eq!(opened(Ok(()), ERROR_OPENING_CROSS_REF_FILE, "open"), Ok(()));
        assert_eq!(
            opened::<()>(
                Err(PortError::NotFound),
                ERROR_OPENING_TRANSACTION_TYPE_FILE,
                "open"
            ),
            Ok(())
        );
        let failures = [
            PortError::Unavailable("relation \"trancatg_file\" does not exist".to_owned()),
            PortError::Contention,
        ];
        for cause in failures {
            let status = opened::<()>(Err(cause), ERROR_OPENING_TRANSACTION_CATG_FILE, "open")
                .map_err(|status| (status.code(), status.render()));
            assert_eq!(
                status,
                Err((16, abend_lines(ERROR_OPENING_TRANSACTION_CATG_FILE, "30")))
            );
        }
    }

    #[tokio::test]
    async fn a_report_line_that_cannot_be_written_abends_and_catalogs_no_report() {
        let dir =
            std::env::temp_dir().join(format!("carddemo-tranrept-write-{}", std::process::id()));
        let generations = Generations::new(dir.clone());
        let mut reptfile = generations
            .begin_next_async(Dataset::TranRept)
            .await
            .unwrap_or_else(|e| panic!("allocate TRANREPT(+1) under {}: {e}", dir.display()));
        let full_width = write_lines(&mut reptfile, vec![" ".repeat(REPORT_RECORD_LEN)]).await;
        let short = write_lines(&mut reptfile, vec!["short".to_owned()]).await;
        reptfile.discard_async().await;
        let cataloged = generations.current(Dataset::TranRept);
        fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
        assert_eq!(full_width, Ok(()));
        assert_eq!(
            short.map_err(|status| status.render()),
            Err(abend_lines(ERROR_WRITING_REPTFILE, "30"))
        );
        assert!(matches!(cataloged, Ok(None)), "{cataloged:?}");
    }

    /// CBTRN03C `1000-TRANFILE-GET-NEXT` (P-CBTRN03C-474) reads `TRANFILE` sequentially: a chunk
    /// returns the records before a record that cannot be read, and the next `READ` raises that
    /// record's error, located at its number, as `read_records` reports it.
    #[tokio::test]
    async fn a_tranfile_chunk_returns_the_records_before_a_failing_read_then_its_error() {
        let records: Vec<Vec<u8>> = ["0000000000000001", "0000000000000002", "0000000000000003"]
            .iter()
            .map(|tran_id| record_bytes(tran_id, "4111000000000001", START, ""))
            .collect();
        let mut bytes = framed(&records);
        bytes.extend_from_slice(b"PARTIAL");
        let dir =
            std::env::temp_dir().join(format!("carddemo-tranrept-chunk-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
        let daly = Generation {
            dataset: Dataset::TransactDaly,
            number: 1,
            path: dir.join("TRANSACT.DALY.G0001V00"),
        };
        fs::write(&daly.path, &bytes).unwrap_or_else(|e| panic!("write {daly}: {e}"));
        let encoded = |chunk: Vec<_>| -> Vec<Vec<u8>> {
            chunk
                .iter()
                .map(|record| TransactionLayout::encode(record, Charset::Ascii))
                .collect()
        };
        let mut tranfile = TranFile::open(&daly)
            .await
            .unwrap_or_else(|e| panic!("open {daly}: {e}"));
        let first = tranfile.next_chunk(2).await.map(encoded);
        let second = tranfile.next_chunk(2).await.map(encoded);
        let third = tranfile.next_chunk(2).await.map_err(|err| err.to_string());
        fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
        assert_eq!(first.ok(), Some(records[..2].to_vec()));
        assert_eq!(second.ok(), Some(records[2..].to_vec()));
        assert_eq!(
            third,
            Err(
                "record file decode failed: CVTRA05Y record 4: final frame of 7 bytes is shorter \
                 than the record length plus LF"
                    .to_owned()
            )
        );
    }

    #[test]
    fn record_numbers_count_from_one() {
        let numbers: Vec<i64> = record_numbers()
            .take(3)
            .map(TransactionRowId::get)
            .collect();
        assert_eq!(numbers, [1, 2, 3]);
    }
}
