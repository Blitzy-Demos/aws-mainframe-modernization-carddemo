//! `batch mnttrdb2`: the MNTTRDB2 job, whose one step runs `COBTUPDT` under `IKJEFT01` as
//! `DSN SYSTEM(DAZ1)` / `RUN PROGRAM(COBTUPDT) PLAN(CARDDEMO)` over the `INPFILE` dataset
//! [app/app-transaction-type-db2/jcl/MNTTRDB2.jcl:L21-30], here the `--input` file
//! (P-COBTUPDT-004).
//!
//! The COBTUPDT paragraphs fall through in their source order, and the job keeps it:
//!
//! 1. `0001-OPEN-FILES`: [`spool_input`] reads the input one line at a time, never holding
//!    more than [`LINE_LIMIT`] bytes of a line, frames every 53-byte record and writes it to a
//!    work file of the run in `CARDDEMO_BATCH_DIR` ([`InputSpool`]), on Tokio's blocking pool,
//!    inside the job span, before any change. A file that cannot be read, or whose records do
//!    not fit `WS-INPUT-REC`, ends the job with exit 16 at the first such record
//!    (P-COBTUPDT-011, E-36), so an endless input such as `/dev/zero` fails at once, and a
//!    record that cannot be framed anywhere in the file stops the run before its first record
//!    is treated.
//! 2. `1001-READ-NEXT-RECORDS` and `1002-READ-RECORDS`: every record, in file order, is read
//!    back from the work file [`SPOOL_BATCH_RECORDS`] at a time ([`InputSpool::next_batch`]),
//!    each read on the blocking pool, and shown with its `PROCESSING` display (P-COBTUPDT-005).
//!    Memory holds one line and the work file's write buffer while the input is read, then one
//!    batch of records, never the file.
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
//! commits, 0 when it fails, the counts reported undone then going to one `warn` event. Those
//! 0 counts are reported values: a `COMMIT` the server rejects undoes every statement, but one
//! whose answer is lost, its connection gone, may have committed them all, so the master is
//! read again before a rerun. A work file that cannot be read back ends the run with exit 16
//! before the commit: the unit of work is rolled back, and the counts are reported as for a
//! failed commit, over the records treated before. A run that ends before its records are
//! treated leaves every count 0.

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
use crate::generations::{
    GenerationError, Generations, WorkFile, WorkFileWriter, blocking, create_private_dir,
};

/// The label an over-long `INPFILE` record is reported under: the program whose 53-byte
/// `WS-INPUT-REC` it does not fit.
const INPUT_LAYOUT: &str = "COBTUPDT";

/// The most bytes [`next_record`] reads of one `INPFILE` line: the 53 bytes of `WS-INPUT-REC`,
/// the two of a `CR LF` line end, and one more. A line read whole within them has its length
/// measured; a line with no `LF` among them is longer than the record whatever follows, so it is
/// refused without reading any more of it.
const LINE_LIMIT: usize = INPUT_REC_LEN + b"\r\n".len() + 1;

/// One frame of the `INPFILE` spool ([`spool_records`]): the 53 bytes of `WS-INPUT-REC` as
/// padded from its line, then `LF`.
const SPOOL_FRAME: usize = INPUT_REC_LEN + 1;

/// The records the second pass reads back from the `INPFILE` spool with one read on Tokio's
/// blocking pool ([`InputSpool::next_batch`]), so it holds at most this many records at a time:
/// a bound on memory, not a COBOL value. COBTUPDT reads one record at a time.
const SPOOL_BATCH_RECORDS: usize = 512;

/// What a batch directory that cannot be prepared, or a work file that cannot be created,
/// written or flushed, is logged under.
const SPOOL_WRITE_FAILED: &str = "MNTTRDB2 INPFILE spool write failed";

/// What a work file that cannot be read back is logged under.
const SPOOL_READ_FAILED: &str = "MNTTRDB2 INPFILE spool read failed";

/// Why a line of an `INPFILE` cannot be read as a record. The text follows `INPFILE <path>: `
/// in the setup failure.
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

/// Why [`spool_records`] stops before the end of the `INPFILE`.
#[derive(Debug, thiserror::Error)]
enum SpoolError {
    /// The `INPFILE` cannot be read, or a line of it cannot be a record.
    #[error(transparent)]
    Input(#[from] InputError),
    /// The work file cannot be written or flushed: the I/O error on it.
    #[error(transparent)]
    Spool(#[from] GenerationError),
}

/// COBTUPDT 0001-OPEN-FILES through STOP RUN — P-COBTUPDT-004 to P-COBTUPDT-012, E-36.
///
/// Runs MNTTRDB2 over `args.input` in two passes: [`spool_input`] frames every record and
/// spools it to a work file, then [`treat_records`] reads the records back in file order and
/// applies each to the Db2 transaction type master inside one unit of work, which is
/// committed. Returns [`ExitStatus::ReturnCode`] with 4 when any record failed, else 0.
///
/// The job reads no clock and writes no generation. Of the context it uses the job's counts,
/// and the batch directory, `CARDDEMO_BATCH_DIR`, with its generation catalog, which hold the
/// run's work file. [`spool_input`] runs on Tokio's blocking pool, inside the job span, before
/// the unit of work begins; each read back of the work file and its release run there too.
///
/// Each of these ends the job with a setup failure (exit 16):
///
/// - the input cannot be read or framed (P-COBTUPDT-011, E-36), the batch directory cannot be
///   prepared, the work file cannot be created or written, or the runtime shuts down before the
///   input is read, before any change and with every count 0;
/// - the unit of work cannot begin, before any change and with every count 0;
/// - the work file cannot be read back, or the runtime shuts down during a read back. The unit
///   of work is rolled back and never committed, so no statement of the run persists. The job's
///   counts keep the `read` and `rejected` of the records treated before and `posted` stays 0,
///   the job span records `written` and `deleted` as 0, and one `warn` event carries the
///   inserts, updates and deletes it reports undone ([`report_undone`]);
/// - the commit at `STOP RUN` fails. A `COMMIT` the server rejects leaves every statement of
///   the run undone; one whose answer is lost, its connection gone, may have committed them
///   all, so the master is read again before a rerun. Either way the counts are reported as
///   for a read back that fails, over every record of the run.
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &Mnttrdb2Args) -> ExitStatus {
    let generations = ctx.generations.clone();
    let batch_dir = ctx.settings.batch_dir.clone();
    let input = args.input.clone();
    let mut spool = match blocking(move || spool_input(&generations, &batch_dir, &input)).await {
        Ok(Ok(spool)) => spool,
        Ok(Err(status)) => return status,
        Err(cancelled) => return setup_failure("INPFILE read did not complete", cancelled),
    };
    let mut uow = match db.begin().await {
        Ok(uow) => uow,
        Err(err) => {
            let status = setup_failure("MNTTRDB2 unit of work could not begin", err);
            spool.release().await;
            return status;
        }
    };
    let mut run = MaintRun::default();
    let mut counts = Counts::default();
    let treated = treat_records(&mut uow, &mut spool, &mut run, &mut counts).await;
    spool.release().await;
    ctx.counts.set_read(count_of(counts.read));
    ctx.counts.set_rejected(count_of(counts.failed));
    if let Err(status) = treated {
        if let Err(err) = uow.rollback().await {
            tracing::warn!(
                error = %err,
                "MNTTRDB2 rollback failed; the unit of work was never committed"
            );
        }
        report_undone(&counts, "MNTTRDB2 INPFILE spool not read back");
        return status;
    }
    if let Err(err) = uow.commit().await {
        let status = setup_failure("MNTTRDB2 STOP RUN commit failed", err);
        report_undone(&counts, "MNTTRDB2 STOP RUN commit failed");
        return status;
    }
    let span = Span::current();
    ctx.counts.set_posted(count_of(counts.applied()));
    span.record("written", span_count(counts.written));
    span.record("deleted", span_count(counts.deleted));
    ExitStatus::ReturnCode(run.return_code())
}

/// Reports a run whose unit of work ends without a commit known to keep its statements, `what`
/// saying why: the job span records `written` and `deleted` as 0, and one `warn` event carries
/// the inserts and updates (`undone_written`) and the deletes (`undone_deleted`) of `counts` it
/// reports undone.
fn report_undone(counts: &Counts, what: &str) {
    let span = Span::current();
    span.record("written", 0_i64);
    span.record("deleted", 0_i64);
    tracing::warn!(
        undone_written = counts.written,
        undone_deleted = counts.deleted,
        written = 0_usize,
        deleted = 0_usize,
        "{what}: every statement undone, 0 types written or deleted"
    );
}

/// COBTUPDT 0001-OPEN-FILES, 1002-READ-RECORDS — P-COBTUPDT-004, P-COBTUPDT-005,
/// P-COBTUPDT-011, E-36.
///
/// Opens the `INPFILE` at `path` and spools its records, in file order, as `READ TR-RECORD NEXT
/// RECORD INTO WS-INPUT-REC` leaves them, to a work file of `generations` ([`spool_records`]),
/// counting them. `batch_dir` is the directory of `generations`, `CARDDEMO_BATCH_DIR`; it is
/// prepared with [`create_private_dir`] once the file is open, because only the allocation of a
/// generation, which MNTTRDB2 never makes, prepares it otherwise.
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
/// The file is read through a [`BufReader`] one line at a time by [`next_record`], as COBTUPDT
/// reads one record at a time, so memory holds never more than [`LINE_LIMIT`] bytes of the line
/// being read and the work file's write buffer, never the records: a device or a file without
/// line ends, such as `/dev/zero`, fails at its first record instead of being read whole, and
/// the records of a file of any length go to the work file, not to memory.
///
/// # Errors
///
/// The setup failure (exit 16), the work file being dropped with it:
///
/// - of a file that cannot be opened or read or holds a record longer than 53 bytes, logged
///   under [`OPEN_FILE_NOT_OK`] with the path and the cause;
/// - of a batch directory that cannot be prepared, or a work file that cannot be created,
///   written or flushed, logged under [`SPOOL_WRITE_FAILED`] with the error, which names the
///   directory or the work file.
fn spool_input(
    generations: &Generations,
    batch_dir: &Path,
    path: &Path,
) -> Result<InputSpool, ExitStatus> {
    let file = File::open(path).map_err(|err| open_failure(path, &err))?;
    let spooled = create_private_dir(batch_dir)
        .and_then(|()| generations.create_work_file())
        .map_err(SpoolError::Spool)
        .and_then(|writer| spool_records(BufReader::new(file), writer));
    let spool = spooled.map_err(|err| match err {
        SpoolError::Input(err) => open_failure(path, &err),
        SpoolError::Spool(err) => setup_failure(SPOOL_WRITE_FAILED, err),
    })?;
    tracing::info!(
        input = %path.display(),
        records = spool.records,
        "INPFILE opened"
    );
    Ok(spool)
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
/// Reads `reader` record by record to the end of the file ([`next_record`]) and writes each
/// record, its 53 `WS-INPUT-REC` bytes as padded from its line and then `LF` ([`SPOOL_FRAME`]),
/// through `writer`. The work file `writer` writes was created exclusively and its name removed
/// at once ([`Generations::create_work_file`]), so it is the run's own and its space is freed
/// when the returned [`InputSpool`] is released or dropped. Memory holds the line being read and
/// the work file's write buffer, never the file.
///
/// # Errors
///
/// The first of: [`SpoolError::Input`], the error of [`next_record`] for the first line that
/// cannot be a record, nothing after it being read; and [`SpoolError::Spool`] when the work file
/// cannot be written or flushed. The work file is dropped with the error.
fn spool_records(
    mut reader: impl BufRead,
    mut writer: WorkFileWriter,
) -> Result<InputSpool, SpoolError> {
    let mut line = Vec::with_capacity(LINE_LIMIT);
    let mut records = 0_usize;
    while let Some(record) = next_record(&mut reader, &mut line, records + 1)? {
        writer.write(&record)?;
        writer.write(b"\n")?;
        records += 1;
    }
    let len = writer.offset();
    let work = writer.finish()?;
    Ok(InputSpool {
        work: Some(work),
        position: 0,
        len,
        records,
    })
}

/// COBTUPDT 1002-READ-RECORDS — P-COBTUPDT-001 to P-COBTUPDT-003, P-COBTUPDT-005.
///
/// The next record of `reader`, record `number` of the `INPFILE`, as the 53 bytes of
/// `WS-INPUT-REC`, or `None` at the end of the file. `line` is the buffer the line is read into.
///
/// The line is read through a [`LINE_LIMIT`]-byte window, and its body is padded with spaces to
/// 53 bytes. The lines are framed as `carddemo_codec::framing::split_ascii` frames a whole file:
/// a line ends at `LF`; a `CR` directly before that `LF` belongs to the line end and any other
/// `CR` is record data; a final line without `LF` is a record; nothing after the last `LF` is no
/// record, so an empty file has none and the run ends at once with return code 0. An empty line
/// is a record of spaces, which `1003-TREAT-RECORD` rejects as an invalid type. Every byte of a
/// body is kept as read.
///
/// # Errors
///
/// For a line that cannot be a record, located at `number`, nothing after its window being read:
///
/// - [`InputError::Record`], the [`CodecError`] with the measured length, for a body longer than
///   53 bytes whose line ended within [`LINE_LIMIT`] bytes;
/// - [`InputError::Unbounded`] for a line with no `LF` within [`LINE_LIMIT`] bytes;
/// - [`InputError::Read`] when `reader` fails.
fn next_record(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    number: usize,
) -> Result<Option<Vec<u8>>, InputError> {
    // A usize of at most 56 always fits the u64 that `Read::take` counts in.
    let window = LINE_LIMIT as u64;
    line.clear();
    let read = reader.by_ref().take(window).read_until(b'\n', line)?;
    if read == 0 {
        return Ok(None);
    }
    let body = match line.strip_suffix(b"\n") {
        Some(ended) => ended.strip_suffix(b"\r").unwrap_or(ended),
        None if read == LINE_LIMIT => return Err(InputError::Unbounded { record: number }),
        None => line.as_slice(),
    };
    let padded = pad(INPUT_LAYOUT, body, INPUT_REC_LEN).map_err(|err| err.at_record(number))?;
    Ok(Some(padded))
}

/// COBTUPDT 1001-READ-NEXT-RECORDS, 1002-READ-RECORDS — P-COBTUPDT-005: the records of the
/// `INPFILE`, every one framed, as [`spool_records`] spooled them to the run's work file, read
/// back in file order [`SPOOL_BATCH_RECORDS`] at a time ([`InputSpool::next_batch`]) and
/// deleted when released ([`InputSpool::release`]) or dropped.
struct InputSpool {
    /// The work file; `None` once a read the runtime cancelled has lost it.
    work: Option<WorkFile>,
    /// The offset of the next frame to read.
    position: u64,
    /// The bytes spooled: the offset just past the last frame.
    len: u64,
    /// The records spooled.
    records: usize,
}

impl InputSpool {
    /// COBTUPDT 1002-READ-RECORDS — P-COBTUPDT-005: the next records in file order, at most
    /// [`SPOOL_BATCH_RECORDS`], read with one read on Tokio's blocking pool ([`read_spooled`]);
    /// none once every record is read.
    ///
    /// # Errors
    ///
    /// A setup failure (exit 16), no record of the batch being returned:
    ///
    /// - The spool holds only framed records, so a work file that cannot be read, ends before
    ///   the bytes spooled or inside a frame, or holds a frame without its `LF` is a storage
    ///   failure, logged under [`SPOOL_READ_FAILED`] with the error, which names the work file.
    /// - The runtime cancels the read because it is shutting down, or a cancelled read lost the
    ///   work file before.
    async fn next_batch(&mut self) -> Result<Vec<MaintRecord>, ExitStatus> {
        const WHAT: &str = "MNTTRDB2 INPFILE spool read did not complete";
        if self.position >= self.len {
            return Ok(Vec::new());
        }
        let Some(work) = self.work.take() else {
            return Err(setup_failure(
                WHAT,
                "the spool was lost by a cancelled read",
            ));
        };
        let (position, len) = (self.position, self.len);
        let (work, read) = blocking(move || {
            let read = read_spooled(&work, position, len);
            (work, read)
        })
        .await
        .map_err(|err| setup_failure(WHAT, err))?;
        self.work = Some(work);
        let (records, next) = read.map_err(|err| setup_failure(SPOOL_READ_FAILED, err))?;
        self.position = next;
        Ok(records)
    }

    /// The end of the spool, once the records are read back or the run stops. Its work file is
    /// dropped on Tokio's blocking pool, inside the job span (see [`blocking`]), which closes it
    /// and frees its space. When the runtime cancels the release because it is shutting down, a
    /// warning is logged; the runtime still drops the work file, with the same effect.
    async fn release(self) {
        let Some(work) = self.work else {
            return;
        };
        if let Err(cancelled) = blocking(move || drop(work)).await {
            tracing::warn!(
                error = %cancelled,
                "MNTTRDB2 INPFILE spool release cancelled by runtime shutdown"
            );
        }
    }
}

/// COBTUPDT 1002-READ-RECORDS — P-COBTUPDT-005: reads `work` from offset `position` up to offset
/// `len`, at most [`SPOOL_BATCH_RECORDS`] frames, each the 53 record bytes [`spool_records`]
/// wrote and their `LF`, and parses each record. Returns the records in file order and the
/// offset of the frame after the last one read.
///
/// # Errors
///
/// [`GenerationError::Io`] on the work file when it cannot be read, ends before `len` or inside
/// a frame, or holds a byte other than `LF` after a record. No record of the batch is returned.
fn read_spooled(
    work: &WorkFile,
    position: u64,
    len: u64,
) -> Result<(Vec<MaintRecord>, u64), GenerationError> {
    // A usize of 54 always fits a u64 file offset.
    let frame_len = SPOOL_FRAME as u64;
    let mut reader = work.reader(position, len.saturating_sub(position));
    let mut frame = [0_u8; SPOOL_FRAME];
    let mut records = Vec::with_capacity(SPOOL_BATCH_RECORDS);
    let mut next = position;
    while next < len && records.len() < SPOOL_BATCH_RECORDS {
        if !reader.read_frame(&mut frame)? {
            return Err(reader.invalid_data("the INPFILE spool ends before the bytes spooled"));
        }
        let (body, separator) = frame.split_at(INPUT_REC_LEN);
        if separator != b"\n" {
            return Err(reader.invalid_data("a spooled INPFILE frame does not end with LF"));
        }
        records.push(tran_type_batch::parse_record(body));
        next = next.saturating_add(frame_len);
    }
    Ok((records, next))
}

/// COBTUPDT 1001-READ-NEXT-RECORDS, 1002-READ-RECORDS, 1003-TREAT-RECORD — P-COBTUPDT-005 to
/// P-COBTUPDT-010.
///
/// Reads every record of `spool` back a batch at a time ([`InputSpool::next_batch`]) and treats
/// each, strictly one after another in file order and numbered from 1, in `uow`
/// ([`treat_record`]), adding its outcome to `run` and `counts`.
///
/// # Errors
///
/// The setup failure of the first batch that cannot be read back. No record of that batch is
/// treated, and `counts` holds the records treated before it.
async fn treat_records(
    uow: &mut UnitOfWork,
    spool: &mut InputSpool,
    run: &mut MaintRun,
    counts: &mut Counts,
) -> Result<(), ExitStatus> {
    loop {
        let batch = spool.next_batch().await?;
        if batch.is_empty() {
            return Ok(());
        }
        for record in &batch {
            let outcome = treat_record(uow, run, counts.read + 1, record).await;
            counts.add(&outcome);
        }
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
    use std::fs::{self, Permissions};
    use std::io::{self, BufRead, BufReader, Read};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use carddemo_codec::CodecError;
    use carddemo_codec::framing::{pad, split_ascii};
    use carddemo_domain::messages::{NO_RECORDS_FOUND, PROCESSING};
    use carddemo_domain::rules::tran_type_batch::{
        INPUT_REC_LEN, MaintRecord, RecordOutcome, parse_record,
    };

    use super::{
        Counts, INPUT_LAYOUT, InputError, InputSpool, LINE_LIMIT, SPOOL_BATCH_RECORDS, SPOOL_FRAME,
        SpoolError, spool_input, spool_records,
    };
    use crate::exit::ExitStatus;
    use crate::generations::{Generations, create_private_dir};

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

    /// A directory of its own for one test, the batch directory its work files are created in,
    /// removed when the test ends.
    struct TestDir(PathBuf);

    impl TestDir {
        /// Starts the test in an empty directory. A directory left by an earlier run of this
        /// process id is removed first, and a failure to remove it, other than its absence,
        /// fails the test before it starts.
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("carddemo-mnttrdb2-{test}-{}", std::process::id()));
            Self::remove(&dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
            fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
            Self(dir)
        }

        /// Removes `dir` and everything under it. A directory that is not there is already
        /// removed, so `NotFound` counts as success.
        fn remove(dir: &Path) -> io::Result<()> {
            match fs::remove_dir_all(dir) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                removed => removed,
            }
        }

        /// The generation catalog of the directory, where the work files go.
        fn generations(&self) -> Generations {
            Generations::new(self.0.clone())
        }

        /// The names of the files in the directory, hidden ones included, sorted.
        fn names(&self) -> Vec<String> {
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

    impl Drop for TestDir {
        /// Removes the directory. A failure other than its absence names the directory and the
        /// error: it fails the test, or, while the test is already panicking, goes to stderr,
        /// because a second panic during unwinding would abort the test binary.
        fn drop(&mut self) {
            if let Err(e) = Self::remove(&self.0) {
                let message = format!("remove {}: {e}", self.0.display());
                if std::thread::panicking() {
                    eprintln!("{message}");
                } else {
                    panic!("{message}");
                }
            }
        }
    }

    /// Every record the second pass reads back from `spool`, batch by batch, with the size of
    /// each batch, each checked to hold at most [`SPOOL_BATCH_RECORDS`] records; or the status
    /// of the first batch that cannot be read back.
    async fn read_back(
        spool: &mut InputSpool,
    ) -> Result<(Vec<MaintRecord>, Vec<usize>), ExitStatus> {
        let mut records = Vec::new();
        let mut sizes = Vec::new();
        loop {
            let batch = spool.next_batch().await?;
            assert!(
                batch.len() <= SPOOL_BATCH_RECORDS,
                "{} records in one batch",
                batch.len()
            );
            if batch.is_empty() {
                return Ok((records, sizes));
            }
            sizes.push(batch.len());
            records.extend(batch);
        }
    }

    /// The records `reader` spools to a work file of `generations` ([`spool_records`]), read
    /// back ([`read_back`]), the spool then released; or the error that stopped the spool.
    async fn round_trip(
        generations: &Generations,
        reader: impl BufRead,
    ) -> Result<Vec<MaintRecord>, SpoolError> {
        let writer = generations
            .create_work_file()
            .unwrap_or_else(|e| panic!("the work file: {e}"));
        let mut spool = spool_records(reader, writer)?;
        let (records, _) = read_back(&mut spool)
            .await
            .unwrap_or_else(|status| panic!("the spool reads back: {status:?}"));
        assert_eq!(
            records.len(),
            spool.records,
            "every spooled record reads back"
        );
        spool.release().await;
        Ok(records)
    }

    #[tokio::test]
    async fn the_fixture_reads_as_its_lines_in_file_order() {
        let dir = TestDir::new("fixture");
        let parsed = round_trip(&dir.generations(), FIXTURE)
            .await
            .unwrap_or_else(|e| panic!("fixture must frame: {e}"));
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
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
    }

    #[tokio::test]
    async fn every_line_frame_pads_to_ws_input_rec() {
        let dir = TestDir::new("pads");
        let generations = dir.generations();
        let cases: [(&[u8], &[&str]); 6] = [
            (b"", &[]),
            (b"A10NEW TYPE\n", &["A10NEW TYPE"]),
            (b"U01PURCHASE\r\nD02\n", &["U01PURCHASE", "D02"]),
            (b"D03", &["D03"]),
            (b"*\n\nD04\n", &["*", "", "D04"]),
            (b"A05X\nU06Y", &["A05X", "U06Y"]),
        ];
        for (input, expected) in cases {
            let parsed = round_trip(&generations, input)
                .await
                .unwrap_or_else(|e| panic!("{:?} must frame: {e}", String::from_utf8_lossy(input)));
            assert_eq!(
                parsed,
                records(expected),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
    }

    /// The records `split_ascii` frames in the whole of `bytes`, each padded and parsed: the
    /// framing [`spool_records`] keeps while it reads line by line.
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

    #[tokio::test]
    async fn line_by_line_framing_is_the_split_ascii_framing() {
        let dir = TestDir::new("framing");
        let generations = dir.generations();
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
            let read = match round_trip(&generations, input.as_slice()).await {
                Ok(records) => Ok(records),
                Err(SpoolError::Input(InputError::Record(err))) => Err(err),
                Err(other) => panic!("{:?}: {other}", String::from_utf8_lossy(input)),
            };
            assert_eq!(
                read,
                split_ascii_records(input),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
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
        let dir = TestDir::new("over-long");
        let generations = dir.generations();
        for (input, expected) in cases {
            let writer = generations
                .create_work_file()
                .unwrap_or_else(|e| panic!("{input:?}: the work file: {e}"));
            let err = spool_records(input.as_bytes(), writer)
                .err()
                .unwrap_or_else(|| panic!("{input:?} must be refused"));
            assert!(matches!(err, SpoolError::Input(_)), "{input:?}: {err:?}");
            assert_eq!(err.to_string(), expected, "{input:?}");
        }
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
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
        let dir = TestDir::new("endless");
        let generations = dir.generations();
        for (before, byte, record) in cases {
            let writer = generations
                .create_work_file()
                .unwrap_or_else(|e| panic!("{before:?}: the work file: {e}"));
            let mut bytes = 0;
            let reader = Counted {
                inner: before.chain(io::repeat(byte)),
                bytes: &mut bytes,
            };
            let refused = spool_records(BufReader::with_capacity(LINE_LIMIT, reader), writer).err();
            assert!(
                matches!(
                    refused,
                    Some(SpoolError::Input(InputError::Unbounded { record: at })) if at == record
                ),
                "{before:?} then {byte:#04X} forever: {refused:?}"
            );
            assert!(
                bytes <= before.len() + LINE_LIMIT,
                "{before:?} then {byte:#04X} forever: {bytes} bytes read"
            );
        }
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
    }

    #[test]
    fn inputs_that_cannot_be_framed_are_setup_failures_naming_the_file() {
        let dir = TestDir::new("unframed");
        let generations = dir.generations();
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
            let status = spool_input(&generations, &dir.0, &path).err();
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
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
    }

    #[tokio::test]
    async fn readable_inputs_load_their_records() {
        let dir = TestDir::new("readable");
        let generations = dir.generations();
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mnttrdb2_input.txt");
        let cases = [(fixture, 10), (PathBuf::from("/dev/null"), 0)];
        for (path, count) in cases {
            let mut spool = spool_input(&generations, &dir.0, &path)
                .unwrap_or_else(|status| panic!("{}: {status:?}", path.display()));
            assert_eq!(spool.records, count, "{}: records spooled", path.display());
            let (loaded, _) = read_back(&mut spool)
                .await
                .unwrap_or_else(|status| panic!("{}: {status:?}", path.display()));
            spool.release().await;
            assert_eq!(loaded.len(), count, "{}: records read back", path.display());
        }
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
    }

    /// `count` `INPFILE` lines of the shapes a maintenance file holds, in turn: an add ended by
    /// `LF`, an update ended by `CR LF`, a delete, a comment, an empty line and a line whose `CR`
    /// is record data.
    fn maintenance_lines(count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for index in 0..count {
            let code = index % 100;
            let line = match index % 6 {
                0 => format!("A{code:02}TYPE {index}\n"),
                1 => format!("U{code:02}DESCRIPTION {index}\r\n"),
                2 => format!("D{code:02}\n"),
                3 => format!("*COMMENT {index}\n"),
                4 => "\n".to_owned(),
                _ => format!("A{code:02}CR\rDATA {index}\n"),
            };
            bytes.extend_from_slice(line.as_bytes());
        }
        bytes
    }

    /// Across the batch boundary, the second pass reads back exactly the records the whole file
    /// frames as `split_ascii` frames it, in file order, in batches of [`SPOOL_BATCH_RECORDS`]
    /// records and a last batch of the rest, never more at a time.
    #[tokio::test]
    async fn the_spool_reads_back_every_record_in_file_order_a_batch_at_a_time() {
        let dir = TestDir::new("batches");
        let generations = dir.generations();
        let input = dir.0.join("INPFILE.txt");
        let counts = [
            1,
            SPOOL_BATCH_RECORDS - 1,
            SPOOL_BATCH_RECORDS,
            SPOOL_BATCH_RECORDS + 1,
            2 * SPOOL_BATCH_RECORDS + 1,
        ];
        for count in counts {
            let bytes = maintenance_lines(count);
            fs::write(&input, &bytes).unwrap_or_else(|e| panic!("write {}: {e}", input.display()));
            let expected = split_ascii_records(&bytes)
                .unwrap_or_else(|e| panic!("{count} records: the lines frame: {e}"));
            assert_eq!(expected.len(), count, "{count} records: one per line");
            let sizes: Vec<usize> = (0..count)
                .step_by(SPOOL_BATCH_RECORDS)
                .map(|start| (count - start).min(SPOOL_BATCH_RECORDS))
                .collect();

            let mut spool = spool_input(&generations, &dir.0, &input)
                .unwrap_or_else(|status| panic!("{count} records: {status:?}"));
            assert_eq!(spool.records, count, "{count} records spooled");
            let read = read_back(&mut spool).await;
            spool.release().await;
            assert_eq!(read, Ok((expected, sizes)), "{count} records");
        }
        assert_eq!(dir.names(), ["INPFILE.txt"], "no spool file is left");
    }

    /// The spool holds only framed records, so one that does not read back as it was written is
    /// a storage failure: a setup failure naming the work file, no record of its batch being
    /// returned, after the full batches read before it.
    #[tokio::test]
    async fn a_spool_that_does_not_read_back_as_written_is_a_setup_failure() {
        let dir = TestDir::new("spool-failures");
        let generations = dir.generations();
        let frame = format!("{:<INPUT_REC_LEN$}\n", "A10NEW TYPE").into_bytes();
        let mut no_lf = frame.clone();
        no_lf[INPUT_REC_LEN] = b'X';
        let full_batch = frame.repeat(SPOOL_BATCH_RECORDS);
        let offset = |len: usize| u64::try_from(len).expect("a test spool length");
        // (case, spooled bytes, spool length, records read back first, the cause)
        let cases = [
            (
                "a frame without its LF",
                no_lf.clone(),
                SPOOL_FRAME,
                0,
                "a spooled INPFILE frame does not end with LF",
            ),
            (
                "a spool that ends inside a frame",
                frame[..INPUT_REC_LEN].to_vec(),
                INPUT_REC_LEN,
                0,
                "the sort work file ends 53 bytes into a 54-byte frame",
            ),
            (
                "a spool shorter than the bytes spooled",
                frame.clone(),
                2 * SPOOL_FRAME,
                0,
                "the INPFILE spool ends before the bytes spooled",
            ),
            (
                "a frame without its LF after a full batch",
                [full_batch.as_slice(), &no_lf].concat(),
                (SPOOL_BATCH_RECORDS + 1) * SPOOL_FRAME,
                SPOOL_BATCH_RECORDS,
                "a spooled INPFILE frame does not end with LF",
            ),
        ];
        for (case, bytes, len, before, cause) in cases {
            let mut writer = generations
                .create_work_file()
                .unwrap_or_else(|e| panic!("{case}: the work file: {e}"));
            writer
                .write(&bytes)
                .unwrap_or_else(|e| panic!("{case}: the work file write: {e}"));
            let work = writer
                .finish()
                .unwrap_or_else(|e| panic!("{case}: the work file flush: {e}"));
            let mut spool = InputSpool {
                work: Some(work),
                position: 0,
                len: offset(len),
                records: len / SPOOL_FRAME,
            };
            if before > 0 {
                let batch = spool
                    .next_batch()
                    .await
                    .unwrap_or_else(|status| panic!("{case}: the first records: {status:?}"));
                assert_eq!(batch, vec![record("A10NEW TYPE"); before], "{case}");
            }
            let failed = spool.next_batch().await;
            spool.release().await;
            let message = match failed {
                Err(ExitStatus::SetupFailure(message)) => message,
                other => panic!("{case}: not a setup failure: {other:?}"),
            };
            let work_file = format!("I/O error on {}/.SORTWK.tmp-", dir.0.display());
            assert!(
                message.starts_with(&work_file) && message.ends_with(&format!(": {cause}")),
                "{case}: {message}"
            );
        }
        assert_eq!(dir.names(), Vec::<String>::new(), "no spool file is left");
    }

    /// A batch directory that cannot hold the run's work file is a setup failure reporting what
    /// [`create_private_dir`] refuses, before any record is spooled, the input left as it is.
    #[test]
    fn an_unusable_batch_directory_is_a_setup_failure_before_any_record_is_spooled() {
        let dir = TestDir::new("unusable");
        let input = dir.0.join("INPFILE.txt");
        fs::write(&input, FIXTURE).unwrap_or_else(|e| panic!("write {}: {e}", input.display()));
        let regular_file = dir.0.join("batch-file");
        fs::write(&regular_file, b"")
            .unwrap_or_else(|e| panic!("write {}: {e}", regular_file.display()));
        let shared = dir.0.join("shared");
        fs::create_dir(&shared)
            .and_then(|()| fs::set_permissions(&shared, Permissions::from_mode(0o777)))
            .unwrap_or_else(|e| panic!("create {}: {e}", shared.display()));
        let cases = [
            ("a regular file", regular_file),
            ("a directory shared with other users", shared.clone()),
        ];
        for (case, batch_dir) in cases {
            let refused = create_private_dir(&batch_dir)
                .err()
                .unwrap_or_else(|| panic!("{case}: {} is refused", batch_dir.display()));
            let status =
                spool_input(&Generations::new(batch_dir.clone()), &batch_dir, &input).err();
            assert_eq!(status, Some(ExitStatus::setup(refused)), "{case}");
        }
        assert_eq!(
            fs::read(&input).ok().as_deref(),
            Some(FIXTURE),
            "the input is unchanged"
        );
        let shared_mode = fs::metadata(&shared)
            .unwrap_or_else(|e| panic!("stat {}: {e}", shared.display()))
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(shared_mode, 0o777, "the shared directory is left as it is");
        let shared_entries = fs::read_dir(&shared)
            .unwrap_or_else(|e| panic!("list {}: {e}", shared.display()))
            .count();
        assert_eq!(shared_entries, 0, "the shared directory receives no file");
        assert_eq!(
            dir.names(),
            ["INPFILE.txt", "batch-file", "shared"],
            "no spool file is left"
        );
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
