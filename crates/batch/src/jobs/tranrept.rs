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

use std::collections::BTreeMap;

use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
use carddemo_domain::entities::Transaction;
use carddemo_domain::format::REPORT_RECORD_LEN;
use carddemo_domain::messages::{
    ABENDING_PROGRAM, END_OF_EXECUTION_OF_PROGRAM_CBTRN03C, ERROR_CLOSING_REPORT_FILE,
    ERROR_OPENING_TRANFILE, ERROR_READING_TRANSACTION_FILE, ERROR_WRITING_REPTFILE,
    file_status_message,
};
use carddemo_domain::numeric::{Alnum, TransactionRowId};
use carddemo_domain::rules::transaction_report::{self, ReportAbend, ReportRun, UnpinnedSortKey};
use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;

use super::{JobContext, backup_live_master, setup_failure};
use crate::cli::TranreptArgs;
use crate::exit::ExitStatus;
use crate::generations::{
    Dataset, Generation, GenerationError, Generations, PendingGeneration, blocking, read_records,
    read_records_async,
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

/// TRANREPT [app/jcl/TRANREPT.jcl] — P-TRANREPT-001 to P-TRANREPT-003: REPROC, SORT and
/// CBTRN03C over the dates of `args`.
///
/// Every step runs, since none has a `COND`. SORT and CBTRN03C end with condition code 0, so a
/// job whose steps all end normally ends with the condition code of the REPROC step: 0, or 12
/// when the live master was empty (`Backup::status`, E-37), after an empty sort output and a
/// report of zero totals. A failed step ends the job with its status and runs no later step:
///
/// - REPROC and SORT: a generation or database failure, or a record the sort cannot place, exit 16.
/// - CBTRN03C: an abend, exit 16 with its `DISPLAY` lines and `ABEND 999`, or a generation or
///   database failure outside the program, exit 16.
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
    let bkup = backup.generation.clone();
    let daly = match blocking(move || sort_step(&generations, &bkup, &start, &end)).await {
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
/// records [`select`] keeps and catalogs the generation. The number of backup records is recorded
/// in `read` and the number of sorted records in `written` on the current span.
///
/// The step is synchronous from the allocation to the catalog: [`run`] runs it on Tokio's
/// blocking pool, inside the job span, so an uncataloged `TRANSACT.DALY(+1)` is deleted there.
///
/// # Errors
///
/// - A record the sort cannot place, as [`select`] reports it: exit 16.
/// - A failure to allocate, read, write or catalog a generation is a setup failure (exit 16),
///   because the audit records no condition code for it. `TRANSACT.DALY(+1)` is then not
///   cataloged, and `TRANSACT.BKUP(+1)` stays cataloged.
fn sort_step(
    generations: &Generations,
    bkup: &Generation,
    start: &Alnum<10>,
    end: &Alnum<10>,
) -> Result<Generation, ExitStatus> {
    let mut sortout = generations
        .begin_next(Dataset::TransactDaly)
        .map_err(|err| setup_failure("TRANSACT.DALY(+1) allocation failed", err))?;
    let sortin = read_records::<TransactionLayout>(&bkup.path)
        .map_err(|err| setup_failure("TRANSACT.BKUP(+1) read failed", err))?;
    let read = sortin.len();
    Span::current().record("read", read);
    let selected = select(start, end, sortin)?;
    sortout
        .append_records::<TransactionLayout>(&selected)
        .map_err(|err| setup_failure("TRANSACT.DALY(+1) write failed", err))?;
    let daly = sortout
        .catalog()
        .map_err(|err| setup_failure("TRANSACT.DALY(+1) catalog failed", err))?;
    let written = selected.len();
    Span::current().record("written", written);
    tracing::info!(
        generation = %daly,
        read,
        written,
        "transactions in the date range sorted by card number"
    );
    Ok(daly)
}

/// The `SORTOUT` records of `sortin`: the records `transaction_report::select_for_report` keeps,
/// in the order it gives, each one the `SORTIN` record unchanged, FILLER and preserved bytes
/// included, as DFSORT copies a record whole.
///
/// # Errors
///
/// - A selected record whose card number holds a byte that is not a digit stops the job with exit
///   16 ([`unpinned_sort_key`]).
/// - A selected transaction that leads back to no `SORTIN` record, which the selection never
///   returns, is a setup failure (exit 16) rather than a record silently left out.
fn select(
    start: &Alnum<10>,
    end: &Alnum<10>,
    sortin: Vec<TransactionRecord>,
) -> Result<Vec<TransactionRecord>, ExitStatus> {
    // A backup record carries no row identity; its record number stands in for one, so every
    // transaction the selection returns leads back to the record it came from.
    let mut by_number: BTreeMap<TransactionRowId, TransactionRecord> =
        record_numbers().zip(sortin).collect();
    let transactions = by_number
        .iter()
        .map(|(&number, record)| Transaction::from_new(number, record.transaction.clone()));
    // `SORT FIELDS` has no `EQUALS`, so DFSORT leaves the order of one card's records undefined
    // (E-38, a halt escalation). The job does not stop on equal card numbers: it takes the order
    // the audit's disposition records and its report lines assume, TRAN-ID within a card, from
    // the selection the CR00 report shares, so both reports give the same lines. Only a card
    // number the zoned-decimal sort cannot place stops it.
    let selected = transaction_report::select_for_report(start, end, transactions)
        .map_err(|err| unpinned_sort_key(&err))?;
    selected
        .iter()
        .map(|tran| by_number.remove(&tran.transaction_id))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            setup_failure(
                "TRANSACT.DALY(+1) not written",
                "the sort selection returned a record TRANSACT.BKUP(+1) does not hold",
            )
        })
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
/// Allocates `TRANREPT(+1)` (`LRECL=133`), writes the report into it ([`report_program`]), and
/// catalogs it once the program has closed its files, so a failure while the files close leaves
/// no report cataloged, as the abnormal end of the step deletes it.
///
/// The allocation, the catalog and the deletion of an uncataloged `TRANREPT(+1)` run on Tokio's
/// blocking pool, inside the job span, as do the reads and appends of [`report_program`].
///
/// # Errors
///
/// - A CBTRN03C abend, exit 16 with `ABEND 999`: a failed lookup ([`lookup_abend`]), a
///   `TRANFILE` that cannot be read ([`tranfile_abend`]), or a `TRANREPT(+1)` that cannot be
///   written or closed ([`file_abend`]).
/// - A failure to allocate `TRANREPT(+1)` or to begin or end the unit of work is a setup failure
///   (exit 16): neither has a COBOL counterpart.
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
    let mut reptfile = ctx
        .generations
        .begin_next_async(Dataset::TranRept)
        .await
        .map_err(|err| setup_failure("TRANREPT(+1) allocation failed", err))?;
    let (records, lines) = match report_program(db, daly, start, end, &mut reptfile).await {
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

/// CBTRN03C `PROCEDURE DIVISION` [app/cbl/CBTRN03C.cbl:L159-217] — P-CBTRN03C-001,
/// P-CBTRN03C-002: the report of `daly` over `start` to `end`, written to `reptfile`, from the
/// opens to the closes. Returns the number of `TRANFILE` records and of report lines written.
///
/// Reads the records of `daly`, the program's `TRANFILE` and its only input of transactions,
/// opens a read-only unit of work for the `CARDXREF`, `TRANTYPE` and `TRANCATG` lookups, and
/// starts the [`ReportRun`] with the `DATEPARM` dates (CBTRN03C `0550-DATEPARM-READ`
/// [app/cbl/CBTRN03C.cbl:L220-243] — P-CBTRN03C-467). The repository holds no content for the
/// `DATEPARM` dataset [app/jcl/TRANREPT.jcl:L73-74], so `start` and `end`, the job's
/// `--start-date` and `--end-date`, stand in for its record: the same dates the `STEP05R` sort
/// selects by, as the CR00 job writes them into both `SYMNAMES` and `DATEPARM` (E-34, resolved,
/// whose disposition uses the start and end dates).
///
/// [`report_records`] reports every record and [`ReportRun::finish`] adds the end-of-input
/// totals. The unit of work is then rolled back, since it writes nothing, before the caller
/// catalogs `reptfile`.
///
/// # Errors
///
/// The errors [`report_step`] lists, except the allocation and the catalog of `TRANREPT(+1)`.
/// `reptfile` is left to the caller, which discards it.
async fn report_program(
    db: &Db,
    daly: &Generation,
    start: Alnum<10>,
    end: Alnum<10>,
    reptfile: &mut PendingGeneration,
) -> Result<(usize, usize), ExitStatus> {
    let tranfile = read_records_async::<TransactionLayout>(daly.path.clone())
        .await
        .map_err(|err| tranfile_abend(&err))?;
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure("CARDXREF, TRANTYPE and TRANCATG open failed", err))?;
    tracing::info!(
        start_date = %start.as_text(),
        end_date = %end.as_text(),
        "report date range read"
    );
    let mut run = ReportRun::new(start, end);
    let records = tranfile.len();
    let mut lines = report_records(&mut run, &mut uow, tranfile, reptfile).await?;
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
/// line it returns written to `reptfile` as it comes. Returns the number of lines written.
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
/// The abend of a failed lookup ([`lookup_abend`]) or of a failed write ([`write_lines`]).
async fn report_records(
    run: &mut ReportRun,
    uow: &mut UnitOfWork,
    tranfile: Vec<TransactionRecord>,
    reptfile: &mut PendingGeneration,
) -> Result<usize, ExitStatus> {
    let mut store = uow.store();
    let mut written = 0;
    for (number, record) in record_numbers().zip(tranfile) {
        let tran = Transaction::from_new(number, record.transaction);
        let lines = run
            .process(&mut store, &tran)
            .await
            .map_err(|abend| lookup_abend(&abend))?;
        let count = lines.len();
        write_lines(reptfile, lines).await?;
        written += count;
    }
    Ok(written)
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
    // A record file read into memory holds far fewer than `i64::MAX` records, so the numbering
    // never reaches the end of the range.
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
/// `TRANSACT.DALY(+1)` cannot be read, or `1000-TRANFILE-GET-NEXT`
/// [app/cbl/CBTRN03C.cbl:L248-272] — P-CBTRN03C-474, when one of its records does not decode.
fn tranfile_abend(err: &GenerationError) -> ExitStatus {
    let display = if matches!(err, GenerationError::Codec(_)) {
        ERROR_READING_TRANSACTION_FILE
    } else {
        ERROR_OPENING_TRANFILE
    };
    file_abend(display, "TRANSACT.DALY(+1) read failed", err)
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
        lines: vec![
            display,
            file_status_message(status),
            ABENDING_PROGRAM.to_owned(),
        ],
        abcode: Some(ABCODE),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::{fs, io};

    use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
    use carddemo_codec::{Charset, Layout};
    use carddemo_domain::format::REPORT_RECORD_LEN;
    use carddemo_domain::messages::{
        ABENDING_PROGRAM, ERROR_OPENING_TRANFILE, ERROR_READING_TRANSACTION_FILE,
        ERROR_WRITING_REPTFILE, FILE_STATUS_IS, INVALID_CARD_NUMBER, INVALID_TRANSACTION_TYPE,
    };
    use carddemo_domain::numeric::{Alnum, CardNumber, TranId, TranTypeCode, TransactionRowId};
    use carddemo_domain::ports::PortError;
    use carddemo_domain::rules::transaction_report::{LookupKey, ReportAbend, UnpinnedSortKey};

    use super::{file_abend, lookup_abend, record_numbers, select, tranfile_abend, write_lines};
    use crate::exit::ExitStatus;
    use crate::generations::{Dataset, GenerationError, Generations};

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

    fn decode(bytes: &[u8]) -> TransactionRecord {
        TransactionLayout::decode(bytes, Charset::Ascii)
            .unwrap_or_else(|e| panic!("test record does not decode: {e}"))
    }

    fn select_default_range(sortin: Vec<TransactionRecord>) -> Result<Vec<Vec<u8>>, ExitStatus> {
        let selected = select(&Alnum::move_from(START), &Alnum::move_from(END), sortin)?;
        Ok(selected
            .iter()
            .map(|record| TransactionLayout::encode(record, Charset::Ascii))
            .collect())
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
        let sortin = [
            &in_card_2_late,
            &after_range,
            &in_card_2_early,
            &before_range,
            &in_card_1,
        ]
        .iter()
        .map(|bytes| decode(bytes))
        .collect();
        assert_eq!(
            select_default_range(sortin),
            Ok(vec![in_card_1, in_card_2_early, in_card_2_late])
        );
    }

    #[test]
    fn a_non_digit_card_number_stops_the_sort_only_inside_the_range() {
        let card = "4111X00000000001";
        let cases = [("2022-07-01", true), ("2022-07-07", false)];
        for (proc_date, stops) in cases {
            let sortin = vec![decode(&record_bytes(
                "0000000000000009",
                card,
                proc_date,
                "",
            ))];
            let expected = if stops {
                let err = UnpinnedSortKey {
                    tran_id: TranId::move_from("0000000000000009"),
                };
                Err(ExitStatus::abend_from(&err, None))
            } else {
                Ok(Vec::new())
            };
            let result = select_default_range(sortin);
            assert_eq!(result, expected, "processing date {proc_date}");
            if let Err(status) = result {
                assert_eq!(status.code(), 16);
                assert!(
                    status.render().iter().all(|line| !line.contains(card)),
                    "the stop must not show the card number: {:?}",
                    status.render()
                );
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
                tranfile_abend(&GenerationError::Codec(codec)),
                abend_lines(ERROR_READING_TRANSACTION_FILE, "30"),
            ),
            (
                tranfile_abend(&GenerationError::Io {
                    path: PathBuf::from("TRANSACT.DALY.G0001V00"),
                    source: io::Error::other("unreadable"),
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

    #[test]
    fn record_numbers_count_from_one() {
        let numbers: Vec<i64> = record_numbers()
            .take(3)
            .map(TransactionRowId::get)
            .collect();
        assert_eq!(numbers, [1, 2, 3]);
    }
}
