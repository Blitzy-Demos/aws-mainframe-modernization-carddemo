//! `batch posttran`: the POSTTRAN job, whose one step `STEP15` runs CBTRN02C over the daily
//! transaction file [app/jcl/POSTTRAN.jcl:L23-42], plus the reward accrual the replacement adds
//! to every posted record (AAP 0.3.5).
//!
//! The job only orchestrates. Validation, posting, the reject reasons and the return code belong
//! to `carddemo_domain::rules::posting`, the accrual to `RewardEngine`. The CBTRN02C paragraphs
//! run in their COBOL order (P-CBTRN02C-004):
//!
//! 1. `0000-DALYTRAN-OPEN`: the `--input` file, DD `DALYTRAN`, is opened.
//! 2. `0100-TRANFILE-OPEN`: `OPEN OUTPUT` of the transaction master, which fails when the live
//!    master already holds records (E-32).
//! 3. `0200-XREFFILE-OPEN`: the cross-reference is opened.
//! 4. `0300-DALYREJS-OPEN`: `DALYREJS(+1)` is allocated, DD `DALYREJS` with
//!    `DISP=(NEW,CATLG,DELETE)` and `LRECL=430` [app/jcl/POSTTRAN.jcl:L34-38].
//! 5. `0400-ACCTFILE-OPEN` and `0500-TCATBALF-OPEN`: the accounts and the category balances are
//!    opened.
//! 6. The main loop over the records in file order, numbered from 1 (P-CBTRN02C-006).
//! 7. `9300-DALYREJS-CLOSE` catalogs `DALYREJS(+1)`, empty after a run without rejects, and the
//!    run counts and `RETURN-CODE` follow (P-CBTRN02C-020).
//!
//! The transaction master, `XREFFILE`, `ACCTFILE` and `TCATBALF` are tables reached through the
//! pool `jobs::dispatch` opened, and a failure to open the pool is a setup failure. Each `OPEN`
//! is a check that its table answers: one statement on it, in a unit of work of its own that
//! writes nothing (`open_tranfile`, `open_table`). A table that does not answer, for example
//! one that is missing, is the open that ends with a status other than 00 (P-CBTRN02C-004,
//! P-CBTRN02C-222, P-CBTRN02C-224, P-CBTRN02C-225): the paragraph's text, status 30
//! ([`PERMANENT_ERROR_STATUS`]) and `ABEND 999`, before any record is read.
//!
//! The file work of these paragraphs runs on Tokio's blocking pool, inside the job span, in the
//! order above: the open of `DALYTRAN`, its read and decode into the spool before the main loop
//! and the main loop's reads back from that spool, and the allocation, every reject append, the
//! catalog and the discard of `DALYREJS(+1)`. A blocking task the runtime cancels while shutting
//! down is a setup failure.
//!
//! # Reading `DALYTRAN`
//!
//! Every record of `DALYTRAN` is read and decoded before the first one is processed, so an input
//! that cannot be read, or holds a record that does not decode, stops the run before anything is
//! posted. The file is never held whole: [`spool_daily`] reads it one line at a time, never more
//! than [`LINE_LIMIT`] bytes of a line, and writes each record that decodes to a work file of the
//! run in `CARDDEMO_BATCH_DIR`, whose name is removed as soon as it is created
//! ([`Generations::create_work_file`]); the main loop reads the records back from it
//! [`SPOOL_BATCH_RECORDS`] at a time ([`DailySpool`]). Memory holds one line and then one batch
//! of records, whatever the size of the input, and an input without line ends, such as
//! `/dev/zero`, fails at its first record.
//!
//! # Units of work
//!
//! Every record runs in its own database transaction. A posted record commits its CBTRN02C
//! updates and its reward accrual together (P-CBTRN02C-019). A rejected record wrote nothing, so
//! its unit is rolled back. When a posting write abends, the writes the record completed before
//! it are committed, because every posting write persists on a later failure
//! (`RECOVERY(NONE)`, P-CBTRN02C-015 to P-CBTRN02C-019). When the reward engine fails, the
//! record's unit is rolled back whole, posting updates included.
//!
//! The account read of `1500-B-LOOKUP-ACCT` (P-CBTRN02C-009) locks the account row, and the
//! record's unit holds that lock through the posting, the accrual and its commit or rollback
//! ([`PostingReads`]). CBTRN02C reads `ACCTFILE` unlocked because `CLOSEFIL` has closed
//! `ACCTDAT` to CICS before the batch cycle [app/jcl/CLOSEFIL.jcl:L28]. The replacement has no
//! such step, so the lock is its guard against the online writers, CB00 and CAUP, that rewrite
//! the same row while POSTTRAN runs: `2800-UPDATE-ACCOUNT-REC` rewrites the whole record the
//! validation read, and an unlocked read would let that rewrite overwrite their committed change.
//!
//! # How a run ends
//!
//! | Outcome | Exit code | `DALYREJS(+1)` |
//! |---|---|---|
//! | Every record read | 4 with rejects, else 0 | cataloged |
//! | `9999-ABEND-PROGRAM` | 16, the `DISPLAY` lines and `ABEND 999` on stderr | deleted |
//! | Reward-engine halt | 8, the record number and TRAN-ID on stderr | deleted |
//! | Setup failure, undecodable input | 16 | deleted |
//!
//! Records committed before an abend or a halt stay committed, as the COBOL's VSAM updates do,
//! while the uncataloged `DALYREJS(+1)` is deleted with the rejects written so far, as
//! `DISP=(…,DELETE)` deletes it on abnormal end. The previous generation stays `(0)`.
//!
//! An open, read or write failure of `DALYTRAN` or `DALYREJS(+1)` displays its
//! `9910-DISPLAY-IO-STATUS` line with the status the job derives from the operating-system error:
//! 35 for a missing `--input` file, else 30. A failed open of a table displays 30. When a held
//! allocation lock refuses `DALYREJS(+1)`, one line naming the lock file comes before the
//! `DISPLAY` lines (`open_rejects`). A failed catalog of `DALYREJS(+1)` displays status 00,
//! whatever the operating-system error: the `XREFFILE-STATUS` that `9300-DALYREJS-CLOSE` shows
//! (`close_rejects`).

use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::iter;
use std::path::Path;

use carddemo_codec::CodecErrorKind;
use carddemo_codec::layouts::{
    Charset, CodecError, DailyTranLayout, DailyTranRecord, Layout, RejectRecord,
    RejectRecordLayout, pad,
};
use carddemo_domain::clock::{Clock, FixedClock};
use carddemo_domain::messages::{
    ACCOUNT_RECORD_NOT_FOUND, transactions_processed_message, transactions_rejected_message,
};
use carddemo_domain::rewards::RewardEngine;
use carddemo_domain::rules::posting::{
    self, Account, AccountId, AccountReader, Alnum, CardNumber, CardXref, DailyTran, NumericCode,
    PostingAbend, PostingCounts, TranCatBalanceStore, TransactionReader, Validation, XrefReader,
};
use carddemo_domain::rules::{AccrualInput, PortError, PortResult, RewardError};
use carddemo_persistence::tx::{Db, UnitOfWork};
use tracing::field::display;

use super::{JobContext, JobCounts, setup_failure};
use crate::cli::PosttranArgs;
use crate::exit::{ExitStatus, one_line};
use crate::generations::{
    Dataset, GenerationError, Generations, PendingGeneration, WorkFile, blocking,
};

/// The `ABCODE` that `9999-ABEND-PROGRAM` passes to `CEE3ABD` — P-CBTRN02C-021
/// [app/cbl/CBTRN02C.cbl:L707-711].
const ABCODE: u32 = 999;

/// The `DALYTRAN-STATUS` that `9910-DISPLAY-IO-STATUS` shows when the `--input` file does not
/// exist: 35, IBM's status for an `OPEN INPUT` of a non-optional file that is not present.
const MISSING_FILE_STATUS: [u8; 2] = *b"35";

/// The `DALYTRAN-STATUS` or `DALYREJS-STATUS` shown for every other file-system failure of
/// `DALYTRAN` or `DALYREJS(+1)`: 30, a permanent error with no further information. A
/// file-system error has no QSAM status of its own; `intcalc` and `tranrept` show their
/// generation-file failures with the same status. A table that does not answer its open shows
/// it too: a store failure has no VSAM status of its own either.
const PERMANENT_ERROR_STATUS: [u8; 2] = *b"30";

/// The account key the open check of `ACCTFILE` reads (`open_table`): 0, an `ACCT-ID` no
/// fixture account holds. A record and status 23 both show that the table answers.
const CHECK_ACCOUNT: AccountId = match AccountId::new(0) {
    Ok(id) => id,
    Err(_) => panic!("0 fits the eleven digits of ACCT-ID"),
};

/// The most bytes [`next_daily`] reads of one `DALYTRAN` line: the 350 bytes of
/// `DALYTRAN-RECORD` and the two of a `CR LF` line end. A line read whole within them is framed
/// and measured as the codec frames a whole file; a line with no `LF` among them that does not
/// end the file is longer than the record whatever follows, so it is refused without reading any
/// more of it.
const LINE_LIMIT: usize = DailyTranLayout::LEN + b"\r\n".len();

/// One frame of the `DALYTRAN` spool ([`spool_daily`]): the 350 bytes of a record that decoded,
/// as padded from its line, then `LF`.
const SPOOL_FRAME: usize = DailyTranLayout::LEN + 1;

/// The records the main loop reads back from the `DALYTRAN` spool with one read on Tokio's
/// blocking pool ([`DailySpool::next_batch`]), so it holds at most this many records at a time.
const SPOOL_BATCH_RECORDS: usize = 512;

/// Runs POSTTRAN `STEP15` over the `--input` file and returns how it ended (see the module
/// documentation).
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &PosttranArgs) -> ExitStatus {
    match post_daily_file(ctx, db, &args.input).await {
        Ok(status) | Err(status) => status,
    }
}

/// CBTRN02C PROCEDURE DIVISION — P-CBTRN02C-004, P-CBTRN02C-006, P-CBTRN02C-020
/// [app/cbl/CBTRN02C.cbl:L193-234]: the opens, the main loop, the close of `DALYREJS` and the
/// end-of-job counts. `Err` carries the status of a run that stopped early; once `DALYREJS(+1)`
/// is allocated, such a run discards it on the blocking pool after its status is reported.
async fn post_daily_file(
    ctx: &JobContext,
    db: &Db,
    input: &Path,
) -> Result<ExitStatus, ExitStatus> {
    let file = open_daily(input).await?;
    open_tranfile(db).await?;
    open_table(db, Table::Xref).await?;
    let mut rejects = open_rejects(ctx).await?;
    for table in [Table::Account, Table::Tcatbal] {
        if let Err(status) = open_table(db, table).await {
            rejects.discard_async().await;
            return Err(status);
        }
    }
    let mut spool = match read_daily(ctx, file, input).await {
        Ok(spool) => spool,
        Err(status) => {
            rejects.discard_async().await;
            return Err(status);
        }
    };
    let mut tally = Tally::default();
    let outcome = post_records(ctx, db, &mut spool, &mut rejects, &mut tally).await;
    spool.release().await;
    tally.set_counts(&ctx.counts);
    if let Err(status) = outcome {
        rejects.discard_async().await;
        return Err(status);
    }
    close_rejects(rejects).await?;
    tally.display_counts();
    Ok(ExitStatus::ReturnCode(tally.counts.return_code()))
}

/// CBTRN02C 0000-DALYTRAN-OPEN — P-CBTRN02C-221 [app/cbl/CBTRN02C.cbl:L236-252]: `OPEN INPUT`
/// of DD `DALYTRAN`, the `--input` file, on the blocking pool. A file that cannot be opened
/// abends before any other file is touched, showing status 35 ([`MISSING_FILE_STATUS`]) when
/// the file does not exist and 30 ([`PERMANENT_ERROR_STATUS`]) for any other failure. The
/// operating-system error is logged.
async fn open_daily(input: &Path) -> Result<File, ExitStatus> {
    let path = input.to_path_buf();
    let opened = blocking(move || File::open(path))
        .await
        .map_err(|err| setup_failure("daily transaction file open did not complete", err))?;
    opened.map_err(|err| {
        let file_status = if err.kind() == io::ErrorKind::NotFound {
            MISSING_FILE_STATUS
        } else {
            PERMANENT_ERROR_STATUS
        };
        let cause = format!("{}: {err}", input.display());
        abend_status(
            &PostingAbend::dalytran_open(file_status),
            None,
            Some(&cause),
        )
    })
}

/// CBTRN02C 0100-TRANFILE-OPEN — P-CBTRN02C-004, P-CBTRN02C-005, P-CBTRN02C-219
/// [app/cbl/CBTRN02C.cbl:L254-270]: `OPEN OUTPUT` of a transaction master defined without
/// `REUSE` fails with status 37 when it already holds records. The run then abends before
/// anything is read or written, so a `posttran` without a `tranbkp` before it changes nothing
/// (E-32).
///
/// The live master is probed with one read in a unit of work that writes nothing. A probe that
/// fails is an open that fails: the paragraph's abend, with status 30
/// ([`PERMANENT_ERROR_STATUS`]); the store failure is logged. A unit of work that cannot begin
/// or end is a setup failure.
async fn open_tranfile(db: &Db) -> Result<(), ExitStatus> {
    const WHAT: &str = "live transaction master read failed";
    let mut uow = db.begin().await.map_err(|err| setup_failure(WHAT, err))?;
    let highest = match uow.store().highest_transaction().await {
        Ok(highest) => highest,
        // The unit of work wrote nothing; dropping it rolls it back.
        Err(cause) => {
            let abend = PostingAbend::tranfile_open(PERMANENT_ERROR_STATUS);
            return Err(abend_status(&abend, None, Some(&cause)));
        }
    };
    uow.rollback()
        .await
        .map_err(|err| setup_failure(WHAT, err))?;
    match highest {
        None => Ok(()),
        Some(_) => Err(abend_status(
            &PostingAbend::tranfile_open_not_empty(),
            None,
            None,
        )),
    }
}

/// CBTRN02C 0200-XREFFILE-OPEN, 0400-ACCTFILE-OPEN and 0500-TCATBALF-OPEN — P-CBTRN02C-222,
/// P-CBTRN02C-224, P-CBTRN02C-225: a KSDS of CBTRN02C, other than the transaction master, that
/// is a table here.
#[derive(Debug, Clone, Copy)]
enum Table {
    /// `XREF-FILE`, DD `XREFFILE`, opened `INPUT` by `0200-XREFFILE-OPEN`.
    Xref,
    /// `ACCOUNT-FILE`, DD `ACCTFILE`, opened `I-O` by `0400-ACCTFILE-OPEN`.
    Account,
    /// `TCATBAL-FILE`, DD `TCATBALF`, opened `I-O` by `0500-TCATBALF-OPEN`.
    Tcatbal,
}

impl Table {
    /// The abend of the table's failed open, with status 30 ([`PERMANENT_ERROR_STATUS`]).
    fn open_abend(self) -> PostingAbend {
        match self {
            Self::Xref => PostingAbend::xreffile_open(PERMANENT_ERROR_STATUS),
            Self::Account => PostingAbend::acctfile_open(PERMANENT_ERROR_STATUS),
            Self::Tcatbal => PostingAbend::tcatbalf_open(PERMANENT_ERROR_STATUS),
        }
    }
}

/// CBTRN02C 0200-XREFFILE-OPEN, 0400-ACCTFILE-OPEN and 0500-TCATBALF-OPEN — P-CBTRN02C-222,
/// P-CBTRN02C-224, P-CBTRN02C-225 [app/cbl/CBTRN02C.cbl:L273-343]: the `OPEN` of `table`, whose
/// table must answer one statement in a unit of work of its own that writes nothing and is
/// rolled back ([`table_answers`]). Nothing is read or written before the opens.
///
/// # Errors
///
/// - The paragraph's abend, with status 30, when the table does not answer; the store failure
///   is logged.
/// - A setup failure when the unit of work cannot begin or end.
async fn open_table(db: &Db, table: Table) -> Result<(), ExitStatus> {
    const WHAT: &str = "file open check failed";
    let mut uow = db.begin().await.map_err(|err| setup_failure(WHAT, err))?;
    match table_answers(&mut uow.store(), table).await {
        Ok(()) => uow.rollback().await.map_err(|err| setup_failure(WHAT, err)),
        // The unit of work wrote nothing; dropping it rolls it back.
        Err(cause) => Err(abend_status(&table.open_abend(), None, Some(&cause))),
    }
}

/// CBTRN02C 0200-XREFFILE-OPEN, 0400-ACCTFILE-OPEN and 0500-TCATBALF-OPEN — P-CBTRN02C-222,
/// P-CBTRN02C-224, P-CBTRN02C-225: whether the table of `table` answers one statement of
/// `store`: for `XREFFILE` and `TCATBALF` a sequential read of no records, for `ACCTFILE` a
/// keyed read of
/// [`CHECK_ACCOUNT`], where a record and status 23 both count as an answer. The outcome is
/// discarded.
///
/// # Errors
///
/// The port failure of the statement, other than [`PortError::NotFound`].
async fn table_answers<S>(store: &mut S, table: Table) -> PortResult<()>
where
    S: XrefReader + AccountReader + TranCatBalanceStore,
{
    let answered = match table {
        Table::Xref => store.xrefs_after(None, 0).await.map(|_| ()),
        Table::Account => store.read_account(CHECK_ACCOUNT).await.map(|_| ()),
        Table::Tcatbal => store.tcatbals_after(None, 0).await.map(|_| ()),
    };
    match answered {
        Ok(()) | Err(PortError::NotFound) => Ok(()),
        Err(cause) => Err(cause),
    }
}

/// CBTRN02C 0300-DALYREJS-OPEN — P-CBTRN02C-223 [app/cbl/CBTRN02C.cbl:L291-307]: `OPEN OUTPUT`
/// of DD `DALYREJS`, the new generation `DALYREJS(+1)` [app/jcl/POSTTRAN.jcl:L34-38],
/// allocated on the blocking pool. Any failure to allocate it abends, showing status 30
/// ([`PERMANENT_ERROR_STATUS`]): IBM's table gives `OPEN OUTPUT` no "file not present" status
/// (35 is for `OPEN INPUT`, `I-O` and `EXTEND`), and a file-system error has no QSAM status of
/// its own.
///
/// A held allocation lock ([`GenerationError::InUse`]) puts one line before the `DISPLAY`
/// lines: the error's text, which names the lock file and the process it records and tells the
/// operator to remove the file only when no batch job is running. It comes first because the
/// allocation fails before the program's `OPEN` reports it, as an allocation message precedes
/// the program's output in the job log, and without it nothing on stderr says what refuses
/// every rerun after a killed job.
async fn open_rejects(ctx: &JobContext) -> Result<PendingGeneration, ExitStatus> {
    ctx.generations
        .begin_next_async(Dataset::DalyRejs)
        .await
        .map_err(|err| {
            let abend = PostingAbend::dalyrejs_open(PERMANENT_ERROR_STATUS);
            let status = abend_status(&abend, None, Some(&err));
            if !matches!(err, GenerationError::InUse { .. }) {
                return status;
            }
            let cause = one_line(&err.to_string());
            let display = abend.to_string();
            ExitStatus::Abend {
                lines: iter::once(cause)
                    .chain(display.lines().map(str::to_owned))
                    .collect(),
                abcode: Some(ABCODE),
            }
        })
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007 [app/cbl/CBTRN02C.cbl:L345-369]: every
/// `READ` of DD `DALYTRAN` to the end of the file, made on the blocking pool before the first
/// record is processed. [`spool_daily`] decodes each record with the `CVTRA06Y` layout and
/// spools it, and the main loop reads the records back in file order from the returned
/// [`DailySpool`].
///
/// # Errors
///
/// Each of these stops the run before any record is processed, nothing being posted:
///
/// - A read failure abends, showing status 30 ([`PERMANENT_ERROR_STATUS`]); the
///   operating-system error is logged.
/// - A record that does not decode is a setup failure (exit 16) whose text names its 1-based
///   record number.
/// - A spool that cannot be created or written is a setup failure (exit 16).
/// - A read the runtime cancels while shutting down is a setup failure.
async fn read_daily(ctx: &JobContext, file: File, input: &Path) -> Result<DailySpool, ExitStatus> {
    let generations = ctx.generations.clone();
    let spooled = blocking(move || spool_daily(&generations, BufReader::new(file)))
        .await
        .map_err(|err| setup_failure("daily transaction file read did not complete", err))?;
    spooled.map_err(|err| match err {
        DailyError::Read(err) => {
            let cause = format!("{}: {err}", input.display());
            abend_status(
                &PostingAbend::dalytran_read(PERMANENT_ERROR_STATUS),
                None,
                Some(&cause),
            )
        }
        // A record that does not decode with CVTRA06Y (a non-digit in a numeric field, a body
        // longer than 350 bytes) has no audited outcome: the COBOL would go on computing with
        // its bytes, with results its compiler options decide. Decoding every record before
        // the first is posted stops such a run before any posting, as a setup failure (exit
        // 16) whose text names the 1-based record number, rather than guessing what CBTRN02C
        // would do with it.
        DailyError::Record(err) => setup_failure("daily transaction record does not decode", err),
        DailyError::Spool(err) => setup_failure("daily transaction spool write failed", err),
    })
}

/// Why [`spool_daily`] stops before the end of `DALYTRAN`.
#[derive(Debug)]
enum DailyError {
    /// The file cannot be read: the operating-system error.
    Read(io::Error),
    /// A record that does not decode with `CVTRA06Y`, located at its 1-based record number.
    Record(CodecError),
    /// The spool cannot be created, written or flushed: the I/O error on its work file.
    Spool(GenerationError),
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007 [app/cbl/CBTRN02C.cbl:L345-369]: reads
/// `reader` record by record to the end of the file ([`next_daily`]) and writes each record, its
/// 350 bytes as padded from its line and then `LF` ([`SPOOL_FRAME`]), to a work file of
/// `generations` ([`Generations::create_work_file`]). The work file is created before the first
/// read, in `CARDDEMO_BATCH_DIR`, exclusively, and its name is removed at once, so it is the
/// run's own and its space is freed when the returned [`DailySpool`] is dropped. Memory holds
/// the line being read and the work file's write buffer, never the file.
///
/// # Errors
///
/// The first of: [`DailyError::Spool`] when the work file cannot be created, written or
/// flushed, and the error of [`next_daily`] for the first line that is not a record. The work
/// file is dropped with the error.
fn spool_daily(
    generations: &Generations,
    mut reader: impl BufRead,
) -> Result<DailySpool, DailyError> {
    let mut writer = generations.create_work_file().map_err(DailyError::Spool)?;
    let mut line = Vec::with_capacity(LINE_LIMIT);
    let mut records = 0_usize;
    while let Some(record) = next_daily(&mut reader, &mut line, records + 1)? {
        writer
            .write(&record)
            .and_then(|()| writer.write(b"\n"))
            .map_err(DailyError::Spool)?;
        records += 1;
    }
    let len = writer.offset();
    let work = writer.finish().map_err(DailyError::Spool)?;
    tracing::debug!(records, "daily transaction file read and spooled");
    Ok(DailySpool {
        work: Some(work),
        position: 0,
        len,
        failed: None,
    })
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007 [app/cbl/CBTRN02C.cbl:L345-369]: the next
/// record of `reader`, record `number` of `DALYTRAN`, as its 350 `DALYTRAN-RECORD` bytes once
/// they decode with the `CVTRA06Y` layout, or `None` at the end of the file. `line` is the
/// buffer the line is read into.
///
/// The line is read through a [`LINE_LIMIT`]-byte window, framed as
/// `carddemo_codec::framing::split_ascii` frames a whole file, then padded with spaces and
/// decoded as `decode_ascii_file` decodes it: a line ends at `LF`; a `CR` directly before that
/// `LF` belongs to the line end and any other `CR` is record data; a final line without `LF` is
/// a record, a `CR` ending it record data; nothing after the last `LF` is no record, so an empty
/// file has none; an empty line is a record of spaces. For a file whose lines fit the window,
/// the records and the first error are therefore those `decode_ascii_file` gives for the same
/// bytes.
///
/// # Errors
///
/// For the record, nothing after the window being read:
///
/// - [`DailyError::Record`], the codec error located at `number`:
///   [`CodecErrorKind::RecordTooLong`] for a body longer than 350 bytes, or an error of
///   [`Layout::decode`]. For a line whose window holds no `LF` and does not end the file, only
///   the window is read, so the length it carries is a lower bound ([`over_long_line`]).
/// - [`DailyError::Read`] when `reader` fails.
fn next_daily(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    number: usize,
) -> Result<Option<Vec<u8>>, DailyError> {
    // A usize of 352 always fits the u64 that `Read::take` counts in.
    let window = LINE_LIMIT as u64;
    line.clear();
    let read = reader
        .by_ref()
        .take(window)
        .read_until(b'\n', line)
        .map_err(DailyError::Read)?;
    if read == 0 {
        return Ok(None);
    }
    let at_record = |err: CodecError| DailyError::Record(err.at_record(number));
    let body = match line.strip_suffix(b"\n") {
        Some(ended) => ended.strip_suffix(b"\r").unwrap_or(ended),
        None if read == LINE_LIMIT && !at_end(reader).map_err(DailyError::Read)? => {
            return Err(at_record(over_long_line(line)));
        }
        None => line.as_slice(),
    };
    let record = pad(DailyTranLayout::NAME, body, DailyTranLayout::LEN).map_err(at_record)?;
    DailyTranLayout::decode(&record, Charset::Ascii).map_err(at_record)?;
    Ok(Some(record))
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007: whether `reader` is at the end of the file,
/// the `AT END` of the next `READ`, without consuming anything. A read the operating system
/// interrupted is retried.
///
/// # Errors
///
/// The error of `reader`.
fn at_end(reader: &mut impl BufRead) -> io::Result<bool> {
    loop {
        match reader.fill_buf() {
            Ok(rest) => return Ok(rest.is_empty()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007: the [`CodecErrorKind::RecordTooLong`] error
/// of a `DALYTRAN` line whose `window`, its first [`LINE_LIMIT`] bytes, holds no `LF` while more
/// bytes follow it. Its body is longer than 350 bytes whatever follows, but the line is never
/// read to its end, so the stored length is a lower bound of the body's: the bytes of `window`,
/// less a final `CR` that an `LF` right after it would make part of a `CR LF` line end. When the
/// `LF` does follow `window` directly, it is the length `decode_ascii_file` measures.
fn over_long_line(window: &[u8]) -> CodecError {
    let stored_len = window.strip_suffix(b"\r").unwrap_or(window).len();
    CodecError::new(
        DailyTranLayout::NAME,
        CodecErrorKind::RecordTooLong { stored_len },
    )
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007 [app/cbl/CBTRN02C.cbl:L345-369]: the
/// records of `DALYTRAN`, every one decoded, as [`spool_daily`] spooled them to the run's work
/// file, read back by the main loop in file order, [`SPOOL_BATCH_RECORDS`] at a time
/// ([`DailySpool::next_batch`]), and deleted when dropped ([`DailySpool::release`]).
struct DailySpool {
    /// The work file; `None` once a read the runtime cancelled has lost it.
    work: Option<WorkFile>,
    /// The offset of the next frame to read.
    position: u64,
    /// The bytes spooled: the offset just past the last frame.
    len: u64,
    /// The read failure met after other records of a batch were read, held back so that those
    /// records are processed first and the next [`DailySpool::next_batch`] reports it.
    failed: Option<GenerationError>,
}

impl DailySpool {
    /// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007 [app/cbl/CBTRN02C.cbl:L345-369]: the
    /// next records in file order, at most [`SPOOL_BATCH_RECORDS`], read with one read on the
    /// blocking pool ([`read_spooled`]); none at the end of the file.
    ///
    /// The `READ`s are sequential: when a frame cannot be read after others of the batch were,
    /// those records are returned and the failure is held back for the next call, so the main
    /// loop processes every record before the failing `READ`, as CBTRN02C does.
    ///
    /// # Errors
    ///
    /// - The spool holds only records that decoded, so a spool that cannot be read, ends inside
    ///   a frame or holds a frame that does not read back as written is a storage failure: the
    ///   read failure of the paragraph, which abends showing status 30
    ///   ([`PERMANENT_ERROR_STATUS`]), as a `READ` that fails part way through `DALYTRAN` does.
    ///   The error, with the work file's path, is logged.
    /// - A setup failure when the runtime cancels the read because it is shutting down, or a
    ///   cancelled read lost the work file before.
    async fn next_batch(&mut self) -> Result<Vec<DailyTranRecord>, ExitStatus> {
        const WHAT: &str = "daily transaction spool read did not complete";
        let read_abend = |err: &GenerationError| {
            abend_status(
                &PostingAbend::dalytran_read(PERMANENT_ERROR_STATUS),
                None,
                Some(err),
            )
        };
        if let Some(failed) = self.failed.take() {
            return Err(read_abend(&failed));
        }
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
        let (work, records, read) = blocking(move || {
            let mut records = Vec::with_capacity(SPOOL_BATCH_RECORDS);
            let read = read_spooled(&work, position, len, &mut records);
            (work, records, read)
        })
        .await
        .map_err(|err| setup_failure(WHAT, err))?;
        self.work = Some(work);
        match read {
            Ok(next) => {
                self.position = next;
                Ok(records)
            }
            Err(failed) if records.is_empty() => Err(read_abend(&failed)),
            Err(failed) => {
                self.failed = Some(failed);
                Ok(records)
            }
        }
    }

    /// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007: the end of the spool, once the main
    /// loop has ended. Its work file is dropped on Tokio's blocking pool, inside the job span
    /// (see [`blocking`]), which closes it and frees its space. When the runtime cancels the
    /// release because it is shutting down, a warning is logged; the runtime still drops the
    /// work file, with the same effect.
    async fn release(self) {
        let Some(work) = self.work else {
            return;
        };
        if let Err(cancelled) = blocking(move || drop(work)).await {
            tracing::warn!(
                error = %cancelled,
                "daily transaction spool release cancelled by runtime shutdown"
            );
        }
    }
}

/// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007: reads `work` from offset `position` up to
/// offset `len`, at most [`SPOOL_BATCH_RECORDS`] frames, each the 350 record bytes
/// [`spool_daily`] wrote and their `LF`, and appends each record, decoded with `CVTRA06Y`, to
/// `records`. Returns the offset of the frame after the last one read.
///
/// # Errors
///
/// [`GenerationError::Io`] on the work file when it cannot be read, ends inside a frame, holds a
/// byte other than `LF` after a record, or holds a record that does not decode. The records read
/// before that frame stay in `records`.
fn read_spooled(
    work: &WorkFile,
    position: u64,
    len: u64,
    records: &mut Vec<DailyTranRecord>,
) -> Result<u64, GenerationError> {
    // A usize of 351 always fits a u64 file offset.
    let frame_len = SPOOL_FRAME as u64;
    let mut reader = work.reader(position, len.saturating_sub(position));
    let mut frame = [0_u8; SPOOL_FRAME];
    let mut next = position;
    for _ in 0..SPOOL_BATCH_RECORDS {
        if !reader.read_frame(&mut frame)? {
            break;
        }
        let (body, separator) = frame.split_at(DailyTranLayout::LEN);
        if separator != b"\n" {
            return Err(reader.invalid_data("a spooled DALYTRAN frame does not end with LF"));
        }
        let record = DailyTranLayout::decode(body, Charset::Ascii)
            .map_err(|err| reader.invalid_data(err))?;
        records.push(record);
        next = next.saturating_add(frame_len);
    }
    Ok(next)
}

/// CBTRN02C main loop — P-CBTRN02C-006 [app/cbl/CBTRN02C.cbl:L202-219]: each record in file
/// order, numbered from 1, is counted, validated by `1500-VALIDATE-TRAN` (P-CBTRN02C-008 to
/// P-CBTRN02C-011) through the locking reads of [`PostingReads`], then posted or counted and
/// written as a reject.
///
/// The records are read back from `spool` a batch at a time ([`DailySpool::next_batch`]), a
/// read failure there abending before the next record is processed. They run strictly one after
/// another, never batched or concurrently, because the cycle totals a posted record leaves take
/// part in the limit check of the next one.
async fn post_records(
    ctx: &JobContext,
    db: &Db,
    spool: &mut DailySpool,
    rejects: &mut PendingGeneration,
    tally: &mut Tally,
) -> Result<(), ExitStatus> {
    let mut record_number = 0_usize;
    loop {
        let batch = spool.next_batch().await?;
        if batch.is_empty() {
            return Ok(());
        }
        for daily in &batch {
            record_number += 1;
            tally.counts.read += 1;
            let mut uow = db
                .begin()
                .await
                .map_err(|err| setup_failure("unit of work could not begin", err))?;
            let validation =
                posting::validate(&mut PostingReads(uow.store()), &daily.daily_tran).await;
            match validation {
                Ok(Validation::Accepted { xref, account }) => {
                    post_record(ctx, uow, record_number, &daily.daily_tran, &xref, account).await?;
                    tally.posted += 1;
                }
                Ok(Validation::Rejected {
                    reason,
                    description,
                }) => {
                    uow.rollback().await.map_err(|err| {
                        setup_failure("read-only unit of work rollback failed", err)
                    })?;
                    write_reject(rejects, daily, record_number, reason, description).await?;
                    tally.counts.rejected += 1;
                }
                Err(abend) => {
                    return Err(posting_abend(uow, &abend, record_number, &daily.daily_tran).await);
                }
            }
        }
    }
}

/// CBTRN02C 1500-A-LOOKUP-XREF and 1500-B-LOOKUP-ACCT — P-CBTRN02C-008, P-CBTRN02C-009
/// [app/cbl/CBTRN02C.cbl:L380-399]: the reads `1500-VALIDATE-TRAN` makes, over the record's
/// store `S`.
///
/// The cross-reference reads pass through unchanged. The account read is the locking read
/// `read_account_for_update`, on the record's `PgStore` a `SELECT … FOR UPDATE` under its own
/// savepoint, whose row lock the record's unit of work keeps until it commits or rolls back. A CB00
/// payment or CAUP edit of the account then either waits for the record's commit, or the read waits
/// for the online write's commit and validates and posts against its result; neither change is lost
/// (module documentation, "Units of work").
///
/// The outcomes the rule branches on are unchanged: a missing account is still `NotFound`,
/// reason 101 (P-CBTRN02C-009), and any other failure, a lock that cannot be taken included,
/// still abends (E-77).
struct PostingReads<S>(S);

impl<S: XrefReader + Send> XrefReader for PostingReads<S> {
    /// CBTRN02C 1500-A-LOOKUP-XREF — P-CBTRN02C-008: the plain read of `S`; `CCXREF` is
    /// read-only at run time.
    async fn read_xref_by_card(&mut self, card: CardNumber) -> PortResult<CardXref> {
        self.0.read_xref_by_card(card).await
    }

    /// The alternate-index read of `S`, which CBTRN02C does not make.
    async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
        self.0.read_xref_by_account(id).await
    }

    /// The sequential read of `S`, which CBTRN02C does not make.
    async fn xrefs_after(
        &mut self,
        after: Option<CardNumber>,
        limit: u16,
    ) -> PortResult<Vec<CardXref>> {
        self.0.xrefs_after(after, limit).await
    }
}

impl<S: AccountReader + Send> AccountReader for PostingReads<S> {
    /// CBTRN02C 1500-B-LOOKUP-ACCT — P-CBTRN02C-009 [app/cbl/CBTRN02C.cbl:L393-399]: the
    /// `READ ACCOUNT-FILE` of `XREF-ACCT-ID`, taken as the locking read of `S`, so the account
    /// `2800-UPDATE-ACCOUNT-REC` rewrites is the one the record's unit of work has locked.
    async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
        self.0.read_account_for_update(id).await
    }

    /// The locking read of `S`, unchanged.
    async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
        self.0.read_account_for_update(id).await
    }
}

/// CBTRN02C 2000-POST-TRANSACTION — P-CBTRN02C-013 to P-CBTRN02C-019, P-CBTRN02C-218
/// [app/cbl/CBTRN02C.cbl:L424-444], then the reward accrual (AAP 0.3.5), in the unit of work
/// `uow` the validation read in, which holds the account's row lock ([`PostingReads`]) until
/// it commits or rolls back.
///
/// The clock is read once, so the record's `TRAN-PROC-TS` (P-CBTRN02C-014, P-CBTRN02C-220) and
/// its accrual time are the same instant; `CARDDEMO_CLOCK_OVERRIDE` pins both. The accrual
/// credits `XREF-ACCT-ID`, the account the validation read. The unit commits only when both the
/// posting and the accrual succeeded.
async fn post_record(
    ctx: &JobContext,
    mut uow: UnitOfWork,
    record_number: usize,
    tran: &DailyTran,
    xref: &CardXref,
    account: Account,
) -> Result<(), ExitStatus> {
    let at = ctx.clock.now();
    let result = posting::post(&mut uow.store(), tran, xref, account, &FixedClock(at)).await;
    let posted = match result {
        Ok(posted) => posted,
        Err(abend) => return Err(posting_abend(uow, &abend, record_number, tran).await),
    };
    if posted.account_rewrite_failed {
        // The REWRITE INVALID KEY path: reason 109 is set but never counted or written to
        // DALYREJS, and the record is still posted (P-CBTRN02C-018, P-CBTRN02C-218, E-15).
        tracing::warn!(
            record_number,
            tran_id = %tran.dalytran_id,
            "{ACCOUNT_RECORD_NOT_FOUND}"
        );
    }
    let input = AccrualInput::for_transaction(&posted.transaction, xref.xref_acct_id);
    let accrued = RewardEngine::accrue(&mut uow.rewards(), input, at).await;
    let accrual = match accrued {
        Ok(accrual) => accrual,
        Err(err) => return Err(reward_halt(uow, &err, record_number, tran).await),
    };
    uow.commit()
        .await
        .map_err(|err| setup_failure("posted record commit failed", err))?;
    tracing::debug!(
        record_number,
        tran_id = %tran.dalytran_id,
        accrued = accrual.is_some(),
        "record posted"
    );
    Ok(())
}

/// CBTRN02C 2500-WRITE-REJECT-REC — P-CBTRN02C-001 to P-CBTRN02C-003, P-CBTRN02C-012
/// [app/cbl/CBTRN02C.cbl:L446-464]: appends the 430-byte `REJECT-RECORD` of `daily` to
/// `DALYREJS(+1)`, the 350 bytes of the input record as read, the reason as `9(04)` and the
/// description moved into `X(76)`, and logs the reject. The append runs on the blocking pool.
/// A failure to write it abends, showing status 30 ([`PERMANENT_ERROR_STATUS`]).
async fn write_reject(
    rejects: &mut PendingGeneration,
    daily: &DailyTranRecord,
    record_number: usize,
    reason: u16,
    description: &'static str,
) -> Result<(), ExitStatus> {
    let record = Some((record_number, &daily.daily_tran));
    let abend = PostingAbend::reject_write(PERMANENT_ERROR_STATUS);
    // The rule rejects with 100 to 103 only, which the 9(04) field always holds; a reason it
    // could not hold would leave a record that cannot be written.
    let reason_code = NumericCode::<4>::new(u32::from(reason))
        .map_err(|err| abend_status(&abend, record, Some(&err)))?;
    let reject = RejectRecord::new(daily, reason_code, Alnum::move_from(description));
    rejects
        .append_records_async::<RejectRecordLayout>(vec![reject])
        .await
        .map_err(|err| abend_status(&abend, record, Some(&err)))?;
    tracing::warn!(
        record_number,
        tran_id = %daily.daily_tran.dalytran_id,
        reason,
        description,
        "record rejected"
    );
    Ok(())
}

/// The reward-engine halt (AAP 0.3.5, 0.5.3.5): the record's unit of work is rolled back, so
/// neither its CBTRN02C updates nor its accrual persist, and the run stops with exit 8. The
/// records before it stay committed, and nothing is added to `DALYREJS`, because the caller
/// discards its pending generation uncataloged.
async fn reward_halt(
    uow: UnitOfWork,
    err: &RewardError,
    record_number: usize,
    tran: &DailyTran,
) -> ExitStatus {
    if let Err(rollback) = uow.rollback().await {
        tracing::error!(record_number, error = %rollback, "halted record rollback failed");
    }
    let tran_id = tran.dalytran_id.to_string();
    tracing::error!(
        record_number,
        tran_id = tran_id.as_str(),
        error = %err,
        "reward engine halt"
    );
    ExitStatus::RewardHalt {
        record_number,
        tran_id,
        cause: err.to_string(),
    }
}

/// CBTRN02C 9999-ABEND-PROGRAM — P-CBTRN02C-021 [app/cbl/CBTRN02C.cbl:L707-711], reached from a
/// failing read or write of `1500-VALIDATE-TRAN` or `2000-POST-TRANSACTION`.
///
/// The writes the record completed before the failure stay, as the "Persists on later failure"
/// column records for every posting write (`persists: RECOVERY(NONE)`, P-CBTRN02C-015 to
/// P-CBTRN02C-019), so `uow` is committed before the run ends with exit 16. No accrual was made
/// for the record.
async fn posting_abend(
    uow: UnitOfWork,
    abend: &PostingAbend,
    record_number: usize,
    tran: &DailyTran,
) -> ExitStatus {
    if let Err(err) = uow.commit().await {
        tracing::error!(
            record_number,
            error = %err,
            "completed writes of the abending record not committed"
        );
    }
    let cause = abend.cause.as_ref().map(|cause| cause as &dyn fmt::Display);
    abend_status(abend, Some((record_number, tran)), cause)
}

/// CBTRN02C 9300-DALYREJS-CLOSE — P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L637-653]: the normal
/// end of the step, where `DISP=(NEW,CATLG,DELETE)` catalogs `DALYREJS(+1)`, empty after a run
/// without rejects, on the blocking pool. A failure abends and adds no generation, while every
/// posted record stays committed, as the COBOL's VSAM updates do. The abend shows status 00,
/// not one derived from the operating-system error, which is logged: the paragraph displays
/// `XREFFILE-STATUS` [app/cbl/CBTRN02C.cbl:L649], and `9200-XREFFILE-CLOSE` has just left it 00.
async fn close_rejects(rejects: PendingGeneration) -> Result<(), ExitStatus> {
    rejects
        .catalog_async()
        .await
        .map_err(|err| abend_status(&PostingAbend::dalyrejs_close(), None, Some(&err)))?;
    Ok(())
}

/// CBTRN02C 9999-ABEND-PROGRAM — P-CBTRN02C-021 [app/cbl/CBTRN02C.cbl:L707-711]: logs `abend`
/// with the record it stopped at and the failure behind it, then returns the abend that prints
/// its `DISPLAY` lines and `ABEND 999` and exits 16.
fn abend_status(
    abend: &PostingAbend,
    record: Option<(usize, &DailyTran)>,
    cause: Option<&dyn fmt::Display>,
) -> ExitStatus {
    let file_status = abend
        .file_status
        .map(|status| String::from_utf8_lossy(&status).into_owned());
    let record_number = record.map(|(number, _)| number);
    let tran_id = record.map(|(_, tran)| display(tran.dalytran_id));
    let cause = cause.map(display);
    tracing::error!(
        paragraph = abend.paragraph,
        file_status = file_status.as_deref(),
        record_number,
        tran_id,
        cause,
        "{}",
        abend.display
    );
    ExitStatus::abend_from(abend, Some(ABCODE))
}

/// `WS-COUNTERS` of the run — P-CBTRN02C-006, P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L184-186]:
/// the records read and rejected, plus the records posted, which the job's counts report.
#[derive(Debug, Default)]
struct Tally {
    /// `WS-TRANSACTION-COUNT` and `WS-REJECT-COUNT`.
    counts: PostingCounts,
    /// The records `2000-POST-TRANSACTION` posted.
    posted: u64,
}

impl Tally {
    /// Sets `read`, `posted` and `rejected` in `job`, the job's counts.
    fn set_counts(&self, job: &JobCounts) {
        job.set_read(self.counts.read);
        job.set_posted(self.posted);
        job.set_rejected(self.counts.rejected);
    }

    /// CBTRN02C end of job — P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L227-228]: the
    /// `TRANSACTIONS PROCESSED :` and `TRANSACTIONS REJECTED  :` displays, as one event whose
    /// fields carry the counts and the two display lines.
    fn display_counts(&self) {
        tracing::info!(
            processed = self.counts.read,
            rejected = self.counts.rejected,
            processed_display = transactions_processed_message(self.counts.read).as_str(),
            rejected_display = transactions_rejected_message(self.counts.rejected).as_str(),
            "end of job counts"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use carddemo_codec::framing::decode_ascii_file;
    use carddemo_codec::layouts::{DailyTranLayout, DailyTranRecord};
    use carddemo_codec::{CodecError, CodecErrorKind, Layout};
    use carddemo_domain::clock::{AnyClock, SystemClock};
    use carddemo_domain::entities::{Account, CardXref};
    use carddemo_domain::messages::{
        ABENDING_PROGRAM, ERROR_CLOSING_DAILY_REJECTS_FILE, ERROR_OPENING_DALY_REJECTS_FILE,
        INVALID_CARD_NUMBER_FOUND,
    };
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, Money,
    };
    use carddemo_domain::ports::{AccountReader, PortError, PortResult, XrefReader};
    use carddemo_domain::rules::posting::PostingAbend;
    use carddemo_runtime::config::Settings;

    use super::{
        ABCODE, DailyError, DailySpool, LINE_LIMIT, PostingReads, SPOOL_BATCH_RECORDS,
        close_rejects, open_daily, open_rejects, read_daily, spool_daily, write_reject,
    };
    use crate::exit::ExitStatus;
    use crate::generations::{GenerationError, Generations, LockHolder, exited_pid};
    use crate::jobs::JobContext;

    /// A read a [`RecordingStore`] received, with the arguments it was given.
    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        XrefByCard(CardNumber),
        XrefByAccount(AccountId),
        XrefsAfter(Option<CardNumber>, u16),
        Account(AccountId),
        AccountForUpdate(AccountId),
    }

    /// The outcomes a [`RecordingStore`] answers with: `xref` for both keyed cross-reference
    /// reads, `xrefs` for the browse and `account` for both account reads, whatever the key.
    /// The keys are checked through the recorded calls.
    #[derive(Clone)]
    struct Answers {
        xref: PortResult<CardXref>,
        xrefs: PortResult<Vec<CardXref>>,
        account: PortResult<Account>,
    }

    /// A store that records each read with its arguments and answers it from its [`Answers`].
    struct RecordingStore {
        answers: Answers,
        calls: Vec<Call>,
    }

    impl XrefReader for RecordingStore {
        async fn read_xref_by_card(&mut self, card: CardNumber) -> PortResult<CardXref> {
            self.calls.push(Call::XrefByCard(card));
            self.answers.xref.clone()
        }

        async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
            self.calls.push(Call::XrefByAccount(id));
            self.answers.xref.clone()
        }

        async fn xrefs_after(
            &mut self,
            after: Option<CardNumber>,
            limit: u16,
        ) -> PortResult<Vec<CardXref>> {
            self.calls.push(Call::XrefsAfter(after, limit));
            self.answers.xrefs.clone()
        }
    }

    impl AccountReader for RecordingStore {
        async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
            self.calls.push(Call::Account(id));
            self.answers.account.clone()
        }

        async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
            self.calls.push(Call::AccountForUpdate(id));
            self.answers.account.clone()
        }
    }

    /// Card `n`, zero-padded to the 16 bytes of `XREF-CARD-NUM`.
    fn card(n: u32) -> CardNumber {
        CardNumber::move_from(&format!("{n:016}"))
    }

    fn account_id(n: u32) -> AccountId {
        AccountId::new(u64::from(n)).expect("an 11-digit account number")
    }

    /// The cross-reference of card `n` to customer `n` and account `n`.
    fn xref(n: u32) -> CardXref {
        CardXref {
            xref_card_num: card(n),
            xref_cust_id: CustomerId::new(n).expect("a 9-digit customer number"),
            xref_acct_id: account_id(n),
        }
    }

    /// Account `n` with the other fields of the first `acctdata.txt` record.
    fn account(n: u32) -> Account {
        Account {
            acct_id: account_id(n),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::from_cents(19_400).expect("an amount within S9(10)V99"),
            acct_credit_limit: Money::from_cents(202_000).expect("an amount within S9(10)V99"),
            acct_cash_credit_limit: Money::from_cents(102_000).expect("an amount within S9(10)V99"),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    #[tokio::test]
    async fn validation_reads_lock_the_account_and_pass_the_cross_reference_through() {
        let unavailable = || PortError::Unavailable("connection reset".to_owned());
        // Each case runs every read once, with keys distinct across the reads, so a key that is
        // swapped, replaced or dropped on the way to the store shows in the recorded calls, and
        // an outcome that is altered on the way back shows in the answer.
        let cases = [
            (
                // `NotFound` of the locking read reaches the rule unchanged: reason 101.
                "no record is found",
                Answers {
                    xref: Err(PortError::NotFound),
                    xrefs: Err(PortError::NotFound),
                    account: Err(PortError::NotFound),
                },
                None,
            ),
            (
                "the records found are returned as read",
                Answers {
                    xref: Ok(xref(1)),
                    xrefs: Ok(vec![xref(6), xref(7)]),
                    account: Ok(account(4)),
                },
                Some(card(3)),
            ),
            (
                // Any other failure, a lock that cannot be taken included, still abends (E-77).
                "a failure is returned as raised",
                Answers {
                    xref: Err(unavailable()),
                    xrefs: Err(unavailable()),
                    account: Err(PortError::Contention),
                },
                Some(card(3)),
            ),
        ];
        for (case, answers, after) in cases {
            let mut reads = PostingReads(RecordingStore {
                answers: answers.clone(),
                calls: Vec::new(),
            });

            assert_eq!(
                reads.read_xref_by_card(card(1)).await,
                answers.xref,
                "{case}"
            );
            assert_eq!(
                reads.read_xref_by_account(account_id(2)).await,
                answers.xref,
                "{case}"
            );
            assert_eq!(reads.xrefs_after(after, 3).await, answers.xrefs, "{case}");
            assert_eq!(
                reads.read_account(account_id(4)).await,
                answers.account,
                "{case}"
            );
            assert_eq!(
                reads.read_account_for_update(account_id(5)).await,
                answers.account,
                "{case}"
            );
            // The cross-reference reads reach the store as they were made; the account read
            // becomes the locking read of the same account.
            assert_eq!(
                reads.0.calls,
                [
                    Call::XrefByCard(card(1)),
                    Call::XrefByAccount(account_id(2)),
                    Call::XrefsAfter(after, 3),
                    Call::AccountForUpdate(account_id(4)),
                    Call::AccountForUpdate(account_id(5)),
                ],
                "{case}"
            );
        }
    }

    /// The POSTTRAN fixture input, 300 records of 350 bytes, each followed by `LF`.
    const DAILY: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/data/ASCII/dailytran.txt"
    ));

    /// The width of a `CVTRA06Y` record followed by its `LF`.
    const FRAMED: usize = 351;

    /// A directory of its own for one test, removed when the test ends.
    struct TestDir(PathBuf);

    impl TestDir {
        /// Starts the test in an empty directory. A directory left by an earlier run of this
        /// process id is removed first, and a failure to remove it, other than its absence,
        /// fails the test before it starts.
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("carddemo-posttran-{test}-{}", std::process::id()));
            Self::remove(&dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
            fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
            Self(dir)
        }

        /// Removes `dir` and everything under it. A directory that is not there is already
        /// removed, so `NotFound` counts as success.
        fn remove(dir: &Path) -> std::io::Result<()> {
            match fs::remove_dir_all(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                removed => removed,
            }
        }

        fn context(&self) -> JobContext {
            let settings = Settings {
                batch_dir: self.0.clone(),
                ..Settings::default()
            };
            JobContext::new(settings, AnyClock::System(SystemClock))
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

    /// A regular file where the directory belongs cannot be removed as a directory, so each
    /// removal path of [`TestDir`] meets an error other than `NotFound` and names the path:
    /// `new` and `drop` panic, and a drop while a panic is already unwinding reports to stderr
    /// and lets the original panic through instead of aborting the test binary.
    #[test]
    fn test_dir_reports_removal_errors_other_than_absence() {
        let blocked = TestDir::new("cleanup-errors").0.clone();
        assert!(
            !blocked.exists(),
            "the dropped helper removed its directory"
        );
        drop(TestDir(blocked.clone()));

        fs::write(&blocked, b"").unwrap_or_else(|e| panic!("write {}: {e}", blocked.display()));
        let created = std::panic::catch_unwind(|| TestDir::new("cleanup-errors"));
        let dropped = std::panic::catch_unwind(|| drop(TestDir(blocked.clone())));
        let unwound: std::thread::Result<()> = std::panic::catch_unwind(|| {
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
        cleaned.unwrap_or_else(|e| panic!("remove {}: {e}", blocked.display()));
        let removal = format!("remove {}: ", blocked.display());
        for payload in [&created, &dropped] {
            let message = payload
                .downcast_ref::<String>()
                .unwrap_or_else(|| panic!("a formatted panic message: {payload:?}"));
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

    /// `0000-DALYTRAN-OPEN` and every `1000-DALYTRAN-GET-NEXT` on `path`, spooling in the batch
    /// directory of `ctx`: the number of records the main loop reads back, or the status the
    /// run stops with.
    async fn open_and_read(ctx: &JobContext, path: &Path) -> Result<usize, ExitStatus> {
        let file = open_daily(path).await?;
        let mut spool = read_daily(ctx, file, path).await?;
        let records = read_back(&mut spool).await?.len();
        spool.release().await;
        Ok(records)
    }

    /// Every record the main loop reads back from `spool`, batch by batch, each batch checked
    /// to hold at most [`SPOOL_BATCH_RECORDS`] records; or the status the first failing batch
    /// stops the run with.
    async fn read_back(spool: &mut DailySpool) -> Result<Vec<DailyTranRecord>, ExitStatus> {
        let mut records = Vec::new();
        loop {
            let batch = spool.next_batch().await?;
            assert!(
                batch.len() <= SPOOL_BATCH_RECORDS,
                "{} records",
                batch.len()
            );
            if batch.is_empty() {
                return Ok(records);
            }
            records.extend(batch);
        }
    }

    /// The setup failure of a `DALYTRAN` line whose first [`LINE_LIMIT`] bytes hold no `LF`
    /// while more bytes follow, at 1-based record `record`, its stored length `stored_len`.
    fn over_long(record: usize, stored_len: usize) -> CodecError {
        CodecError::new(
            DailyTranLayout::NAME,
            CodecErrorKind::RecordTooLong { stored_len },
        )
        .at_record(record)
    }

    #[tokio::test]
    async fn the_daily_file_is_opened_read_and_decoded_on_the_blocking_pool() {
        let dir = TestDir::new("daily");
        let ctx = dir.context();
        let mut undecodable = DAILY[..2 * FRAMED].to_vec();
        // DALYTRAN-AMT of the second record starts at byte 132 of its 350; a letter there is
        // not a zoned digit.
        undecodable[FRAMED + 132] = b'X';
        let codec_error = decode_ascii_file::<DailyTranLayout>(&undecodable)
            .expect_err("the second record does not decode");
        assert_eq!(codec_error.record, Some(2), "{codec_error}");
        // Ten copies of the fixture day: 3000 records, read back in several batches.
        let copies = DAILY.repeat(10);
        let mut last_undecodable = copies.clone();
        last_undecodable[copies.len() - FRAMED + 132] = b'X';
        let last_error = decode_ascii_file::<DailyTranLayout>(&last_undecodable)
            .expect_err("the last record does not decode");
        assert_eq!(last_error.record, Some(3000), "{last_error}");

        let fixture = dir.0.join("dailytran.txt");
        let corrupt = dir.0.join("undecodable.txt");
        let many = dir.0.join("copies.txt");
        let corrupt_last = dir.0.join("last-undecodable.txt");
        for (path, bytes) in [
            (&fixture, DAILY),
            (&corrupt, &undecodable[..]),
            (&many, &copies[..]),
            (&corrupt_last, &last_undecodable[..]),
        ] {
            fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        }
        let through_file = fixture.join("absent.txt");
        let cases = [
            ("every fixture record decodes", fixture, Ok(300)),
            (
                "a long file is read back from its spool a batch at a time",
                many,
                Ok(3000),
            ),
            (
                "a long file whose last record does not decode is a setup failure naming it",
                corrupt_last,
                Err(ExitStatus::setup(&last_error)),
            ),
            (
                "a missing file abends in 0000-DALYTRAN-OPEN with status 35",
                dir.0.join("absent.txt"),
                Err(ExitStatus::abend_from(
                    &PostingAbend::dalytran_open(*b"35"),
                    Some(ABCODE),
                )),
            ),
            (
                "a path through a regular file abends in 0000-DALYTRAN-OPEN with status 30",
                through_file,
                Err(ExitStatus::abend_from(
                    &PostingAbend::dalytran_open(*b"30"),
                    Some(ABCODE),
                )),
            ),
            (
                "a file that opens but cannot be read abends in 1000-DALYTRAN-GET-NEXT with \
                 status 30",
                dir.0.clone(),
                Err(ExitStatus::abend_from(
                    &PostingAbend::dalytran_read(*b"30"),
                    Some(ABCODE),
                )),
            ),
            (
                "an undecodable record is a setup failure naming it",
                corrupt,
                Err(ExitStatus::setup(&codec_error)),
            ),
        ];
        for (case, path, expected) in cases {
            assert_eq!(open_and_read(&ctx, &path).await, expected, "{case}");
        }
        assert_eq!(
            dir.names(),
            [
                "copies.txt",
                "dailytran.txt",
                "last-undecodable.txt",
                "undecodable.txt"
            ],
            "only the inputs: no spool file is left behind"
        );
    }

    /// For every input whose lines fit [`LINE_LIMIT`], the records the main loop reads back
    /// from the spool, and the first error, are those `decode_ascii_file` gives for the whole
    /// file: the framing of `split_ascii`, the padding of `pad` and the decode of `CVTRA06Y`.
    #[tokio::test]
    async fn dalytran_is_framed_and_decoded_line_by_line_as_the_codec_decodes_the_file() {
        let dir = TestDir::new("framing");
        let generations = Generations::new(dir.0.clone());
        let first = &DAILY[..DailyTranLayout::LEN];
        let second = &DAILY[FRAMED..FRAMED + DailyTranLayout::LEN];
        let line = |parts: &[&[u8]]| parts.concat();
        let mut non_digit_amount = DAILY[..2 * FRAMED].to_vec();
        non_digit_amount[FRAMED + 132] = b'X';
        let cases: [(&str, Vec<u8>); 12] = [
            ("the fixture day", DAILY.to_vec()),
            ("an empty file has no record", Vec::new()),
            ("CR LF line ends", line(&[first, b"\r\n", second, b"\r\n"])),
            ("a final line without LF", line(&[first, b"\n", second])),
            (
                "a CR ending a final line without LF is record data",
                line(&[first, b"\n", &second[..DailyTranLayout::LEN - 1], b"\r"]),
            ),
            (
                // Byte 40 is inside DALYTRAN-DESC, an alphanumeric field.
                "a CR before the end of a line is record data",
                line(&[&first[..40], b"\r", &first[41..], b"\n"]),
            ),
            (
                "an empty line is a record of spaces, which does not decode",
                line(&[first, b"\n\n", second, b"\n"]),
            ),
            (
                "a short line is padded with spaces",
                line(&[&first[..DailyTranLayout::LEN - 20], b"\n"]),
            ),
            ("a 351-byte body", line(&[first, b"\n", second, b"X\n"])),
            (
                "a 351-byte body ending the file without LF",
                line(&[second, b"X"]),
            ),
            (
                "a 352-byte body ending the file, its last byte a CR",
                line(&[second, b"X\r"]),
            ),
            ("a non-digit in DALYTRAN-AMT", non_digit_amount),
        ];
        for (case, bytes) in cases {
            let expected = decode_ascii_file::<DailyTranLayout>(&bytes)
                .map(|framed| framed.into_iter().map(|f| f.record).collect::<Vec<_>>());
            let actual = match spool_daily(&generations, bytes.as_slice()) {
                Ok(mut spool) => {
                    let records = read_back(&mut spool).await.unwrap_or_else(|status| {
                        panic!("{case}: the spool reads back: {status:?}")
                    });
                    spool.release().await;
                    Ok(records)
                }
                Err(DailyError::Record(err)) => Err(err),
                Err(other) => panic!("{case}: {other:?}"),
            };
            assert_eq!(actual, expected, "{case}");
        }
        assert_eq!(
            dir.names(),
            Vec::<String>::new(),
            "no spool file is left behind"
        );
    }

    /// A line with no `LF` in its first [`LINE_LIMIT`] bytes, while more bytes follow, fails its
    /// record at once: nothing after those bytes is read, and the stored length reported is a
    /// lower bound of the one `decode_ascii_file` measures, equal to it when the `LF` follows
    /// those bytes directly.
    #[test]
    fn a_line_without_lf_in_its_window_fails_its_record_with_a_lower_bound_of_its_length() {
        let dir = TestDir::new("over-long");
        let generations = Generations::new(dir.0.clone());
        let record = &DAILY[..DailyTranLayout::LEN];
        let line = |parts: &[&[u8]]| parts.concat();
        let wide = [b'X'; 50];
        // (case, input, record number, reported stored length, measured stored length)
        let cases = [
            (
                "a 352-byte body and LF",
                line(&[record, b"XX\n"]),
                1,
                352,
                352,
            ),
            (
                "a 351-byte body and CR LF",
                line(&[record, b"X\r\n"]),
                1,
                351,
                351,
            ),
            (
                "a 400-byte body after a record",
                line(&[&DAILY[..FRAMED], record, &wide, b"\n"]),
                2,
                352,
                400,
            ),
            (
                "a CR at the end of the window that is record data",
                line(&[record, b"X\rY\n"]),
                1,
                351,
                353,
            ),
            (
                "NUL bytes without any line end",
                vec![0; 4096],
                1,
                352,
                4096,
            ),
        ];
        for (case, bytes, number, reported, measured) in cases {
            let codec = decode_ascii_file::<DailyTranLayout>(&bytes)
                .expect_err("the line is longer than the record");
            assert_eq!(
                codec,
                over_long(number, measured),
                "{case}: the codec measure"
            );
            let mut unread = bytes.as_slice();
            let err = match spool_daily(&generations, &mut unread) {
                Err(DailyError::Record(err)) => err,
                Err(other) => panic!("{case}: {other:?}"),
                Ok(_) => panic!("{case}: the line is spooled"),
            };
            assert_eq!(err, over_long(number, reported), "{case}");
            assert_eq!(
                bytes.len() - unread.len(),
                (number - 1) * FRAMED + LINE_LIMIT,
                "{case}: the record's window is the last byte read"
            );
        }
        assert_eq!(
            dir.names(),
            Vec::<String>::new(),
            "no spool file is left behind"
        );
    }

    /// [`open_and_read`] on `path` with the context of `dir`, run by a Tokio runtime of its own
    /// on a thread of its own, which must end within `bound`. A blocking read cannot be
    /// cancelled, and a runtime waits for its blocking reads when it shuts down, so a read of an
    /// endless input that did not stop would hang a test that awaited it; here it fails the test
    /// instead, its thread being left to end with the test process.
    fn open_and_read_within(
        dir: &TestDir,
        path: &Path,
        bound: Duration,
    ) -> Result<usize, ExitStatus> {
        let ctx = dir.context();
        let owned = path.to_path_buf();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap_or_else(|e| panic!("a Tokio runtime: {e}"));
            // The receiver is gone only once the bound has passed and failed the test.
            let _ = sender.send(runtime.block_on(open_and_read(&ctx, &owned)));
        });
        receiver
            .recv_timeout(bound)
            .unwrap_or_else(|e| panic!("{} not read within {bound:?}: {e}", path.display()))
    }

    /// A character device that never ends and a huge sparse file, neither holding a line end,
    /// each fail at their first record after one window of [`LINE_LIMIT`] bytes, as a setup
    /// failure, instead of being read whole.
    #[test]
    fn inputs_without_line_ends_fail_at_their_first_record_without_being_read_whole() {
        let dir = TestDir::new("endless");
        let sparse = dir.0.join("sparse.txt");
        File::create(&sparse)
            .and_then(|file| file.set_len(1 << 30))
            .unwrap_or_else(|e| panic!("create {}: {e}", sparse.display()));
        let refused = Err(ExitStatus::setup(over_long(1, LINE_LIMIT)));
        let cases = [
            ("/dev/zero, a character device", PathBuf::from("/dev/zero")),
            ("a sparse file of 1 GiB of NUL bytes", sparse),
        ];
        for (case, path) in cases {
            let read = open_and_read_within(&dir, &path, Duration::from_secs(60));
            assert_eq!(read, refused, "{case}");
        }
        assert_eq!(dir.names(), ["sparse.txt"], "no spool file is left behind");
    }

    /// The spool is storage of the run's own: one that cannot be created is a setup failure, and
    /// one that does not read back as it was written abends in `1000-DALYTRAN-GET-NEXT` with
    /// status 30 after the records read before the failing frame, as a `READ` of `DALYTRAN`
    /// that fails part way through the file does.
    #[tokio::test]
    async fn a_spool_that_cannot_be_created_or_read_back_stops_the_run() {
        let dir = TestDir::new("spool-failures");
        let input = dir.0.join("dailytran.txt");
        fs::write(&input, DAILY).unwrap_or_else(|e| panic!("write {}: {e}", input.display()));
        let regular_file = dir.0.join("batch-file");
        fs::write(&regular_file, b"")
            .unwrap_or_else(|e| panic!("write {}: {e}", regular_file.display()));
        let settings = Settings {
            batch_dir: regular_file,
            ..Settings::default()
        };
        let no_spool = JobContext::new(settings, AnyClock::System(SystemClock));
        let not_created = open_and_read(&no_spool, &input).await;
        assert!(
            matches!(not_created, Err(ExitStatus::SetupFailure(_))),
            "a batch directory that is a regular file holds no spool: {not_created:?}"
        );
        fs::remove_file(&input).unwrap_or_else(|e| panic!("remove {}: {e}", input.display()));

        let generations = Generations::new(dir.0.clone());
        let frame = &DAILY[..FRAMED];
        let mut non_digit_amount = frame.to_vec();
        non_digit_amount[132] = b'X';
        let mut no_lf = frame.to_vec();
        no_lf[DailyTranLayout::LEN] = b'X';
        let byte_count = |len: usize| u64::try_from(len).expect("a test spool length");
        // (case, spooled bytes, spool length, records read back before the abend)
        let cases = [
            (
                "a record that does not decode",
                non_digit_amount.clone(),
                FRAMED,
                0,
            ),
            ("a frame without its LF", no_lf, FRAMED, 0),
            (
                "a spool that ends inside a frame",
                frame[..FRAMED - 1].to_vec(),
                FRAMED,
                0,
            ),
            (
                "a record that does not decode after one that does",
                [frame, &non_digit_amount].concat(),
                2 * FRAMED,
                1,
            ),
        ];
        let abend = ExitStatus::abend_from(&PostingAbend::dalytran_read(*b"30"), Some(ABCODE));
        for (case, bytes, len, before) in cases {
            let mut writer = generations
                .create_work_file()
                .unwrap_or_else(|e| panic!("{case}: the work file: {e}"));
            writer
                .write(&bytes)
                .unwrap_or_else(|e| panic!("{case}: the work file write: {e}"));
            let work = writer
                .finish()
                .unwrap_or_else(|e| panic!("{case}: the work file flush: {e}"));
            let mut spool = DailySpool {
                work: Some(work),
                position: 0,
                len: byte_count(len),
                failed: None,
            };
            if before > 0 {
                let batch = spool
                    .next_batch()
                    .await
                    .unwrap_or_else(|status| panic!("{case}: the first records: {status:?}"));
                assert_eq!(batch.len(), before, "{case}");
            }
            assert_eq!(spool.next_batch().await, Err(abend.clone()), "{case}");
            spool.release().await;
        }
        assert_eq!(
            dir.names(),
            ["batch-file"],
            "only the regular file: no spool file is left behind"
        );
    }

    #[tokio::test]
    async fn rejects_are_written_then_cataloged_or_discarded_on_the_blocking_pool() {
        let records = decode_ascii_file::<DailyTranLayout>(DAILY)
            .unwrap_or_else(|e| panic!("the fixture decodes: {e}"));
        let daily = &records[0].record;
        for catalog in [true, false] {
            let dir = TestDir::new(&format!("rejects-{catalog}"));
            let ctx = dir.context();
            let mut rejects = open_rejects(&ctx)
                .await
                .unwrap_or_else(|status| panic!("DALYREJS(+1) allocates: {status:?}"));
            write_reject(&mut rejects, daily, 1, 100, INVALID_CARD_NUMBER_FOUND)
                .await
                .unwrap_or_else(|status| panic!("the reject is written: {status:?}"));
            if !catalog {
                rejects.discard_async().await;
                assert!(
                    dir.names().is_empty(),
                    "the temporary file and the lock are removed: {:?}",
                    dir.names()
                );
                continue;
            }
            close_rejects(rejects)
                .await
                .unwrap_or_else(|status| panic!("DALYREJS(+1) catalogs: {status:?}"));
            assert_eq!(
                dir.names(),
                ["DALYREJS.G0001V00"],
                "no lock or temporary file"
            );
            let path = dir.0.join("DALYREJS.G0001V00");
            let written =
                fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let expected = format!(
                "{}0100{INVALID_CARD_NUMBER_FOUND:<76}\n",
                std::str::from_utf8(&DAILY[..350]).expect("the fixture is ASCII")
            );
            assert_eq!(written, expected.as_bytes());
        }
    }

    /// P-CBTRN02C-020: a `DALYREJS(+1)` that cannot be cataloged, here because a file already
    /// holds its name, abends in `9300-DALYREJS-CLOSE` with the `00` of `XREFFILE-STATUS` and
    /// exit 16, and adds no generation.
    #[tokio::test]
    async fn a_failed_rejects_catalog_abends_with_the_cross_reference_status_00() {
        let dir = TestDir::new("rejects-close");
        let ctx = dir.context();
        let rejects = open_rejects(&ctx)
            .await
            .unwrap_or_else(|status| panic!("DALYREJS(+1) allocates: {status:?}"));
        let taken = dir.0.join("DALYREJS.G0001V00");
        fs::write(&taken, b"taken").unwrap_or_else(|e| panic!("write {}: {e}", taken.display()));

        let status = close_rejects(rejects)
            .await
            .expect_err("a taken generation name stops the catalog");

        assert_eq!(
            status.render(),
            [
                ERROR_CLOSING_DAILY_REJECTS_FILE,
                "FILE STATUS IS: NNNN0000",
                ABENDING_PROGRAM,
                "ABEND 999",
            ]
        );
        assert_eq!(status.code(), 16);
        assert_eq!(
            dir.names(),
            ["DALYREJS.G0001V00"],
            "no lock or temporary file"
        );
        assert_eq!(
            fs::read(&taken).unwrap_or_else(|e| panic!("read {}: {e}", taken.display())),
            b"taken",
            "the file holding the name is left as it was"
        );
    }

    /// P-CBTRN02C-223: whatever stops the allocation of `DALYREJS(+1)`, `0300-DALYREJS-OPEN`
    /// abends with status 30 and exit 16. A lock file a killed job left behind puts, first, one
    /// line that names it and its holder, a line break in the lock path included.
    #[tokio::test]
    async fn a_failed_rejects_allocation_abends_with_status_30() {
        let dir = TestDir::new("rejects-open");
        let exited = exited_pid();
        let locked = dir.0.join("killed\njob");
        fs::create_dir_all(&locked).unwrap_or_else(|e| panic!("create {}: {e}", locked.display()));
        let lock = locked.join(".DALYREJS.lock");
        let lock_content = format!("{exited}\n");
        fs::write(&lock, &lock_content).unwrap_or_else(|e| panic!("write {}: {e}", lock.display()));
        let regular_file = dir.0.join("batch-file");
        fs::write(&regular_file, b"")
            .unwrap_or_else(|e| panic!("write {}: {e}", regular_file.display()));
        let in_use = GenerationError::InUse {
            dataset: "DALYREJS",
            path: lock.clone(),
            holder: LockHolder::Process {
                pid: exited,
                running: Some(false),
            },
        }
        .to_string()
        .replace('\n', " ");
        let cases = [
            ("a lock file left by a killed job", locked, Some(in_use)),
            (
                "a batch directory that is a regular file",
                regular_file,
                None,
            ),
        ];
        for (case, batch_dir, cause) in cases {
            let settings = Settings {
                batch_dir,
                ..Settings::default()
            };
            let ctx = JobContext::new(settings, AnyClock::System(SystemClock));

            let status = open_rejects(&ctx).await.expect_err(case);

            let expected: Vec<String> = cause
                .into_iter()
                .chain(
                    [
                        ERROR_OPENING_DALY_REJECTS_FILE,
                        "FILE STATUS IS: NNNN0030",
                        ABENDING_PROGRAM,
                        "ABEND 999",
                    ]
                    .map(str::to_owned),
                )
                .collect();
            assert_eq!(status.render(), expected, "{case}");
            assert_eq!(status.code(), 16, "{case}");
        }
        assert_eq!(
            fs::read_to_string(&lock).unwrap_or_else(|e| panic!("read {}: {e}", lock.display())),
            lock_content,
            "the lock is never taken over"
        );
    }
}
