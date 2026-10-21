//! `tranrept`: batch job TRANREPT, the transaction detail report of `CBTRN03C` (`TRANREPT.jcl`,
//! `TRANREPT.prc`; AAP 0.3.4.3). Its expected results are the `CBTRN03C-` rows of
//! `modern/audit/batch/TRANREPT.md` and, over the POSTED scenario, the `FIXRPT-` rows of
//! `modern/audit/fixtures.md`.
//!
//! # The steps it replaces
//!
//! | JCL step | What it does | Replacement |
//! |---|---|---|
//! | `STEP05R` (`REPROC`) | IDCAMS `REPRO` of the transaction KSDS into `TRANSACT.BKUP(+1)` | none: `transactions` is read in place and no backup is written (CBTRN03C-003, X-TRANREPT-2) |
//! | `STEP05R` (`SORT`) | `INCLUDE` on `TRAN-PROC-DT` inside the window, `SORT FIELDS=(TRAN-CARD-NUM,A)`, into `TRANSACT.DALY(+1)` | `TransactionRepository::find_processed_between` over the typed window, in card-number order (CBTRN03C-004, CBTRN03C-005) |
//! | `STEP10R` | `CBTRN03C` over the sorted file, `CARDXREF`, `TRANTYPE`, `TRANCATG` and `DATEPARM`, writing `TRANREPT(+1)` | `domain::batch::cbtrn03c::Tranrept` over the selected rows, writing `TRANREPT.G<nnnn>` under `Config::reports_dir` (CBTRN03C-032) |
//!
//! # Flow
//!
//! 1. `--start-date` and `--end-date` become the DATEPARM window, two typed dates (CBTRN03C-001,
//!    CBTRN03C-002; Typed Dates and Timestamps). The one window stands for both the SORT
//!    constants and the DATEPARM record (X-TRANREPT-1).
//! 2. The inputs are opened in the program's order (CBTRN03C-007): the selection of
//!    `transactions` (`TRANFILE`), the generation file (`REPTFILE`), then the reads of
//!    `card_xref` (`CARDXREF`), `transaction_types` (`TRANTYPE`) and `transaction_categories`
//!    (`TRANCATG`). Every read is read-only; the job writes no table (CBTRN03C-003).
//! 3. Each selected row goes to `Tranrept::process` in the order the repository returns it; the
//!    job neither filters nor sorts again. Rows of one card come in transaction-ID order, the
//!    proposed resolution of halt candidate X-TRANREPT-H1, which claims no parity. The loop ends
//!    at the first `Flow::Stop`, as `NEXT SENTENCE` leaves the COBOL loop (CBTRN03C-011,
//!    X-TRANREPT-9); the window being the selection's own, no selected row reaches it.
//! 4. `Tranrept::finish` returns every report record, and each is written as its 133 bytes and
//!    LF (CBTRN03C-032). The last amount counted twice at end of file and the missing account
//!    total of the last card are the domain's, reproduced as audited and never corrected here
//!    (CBTRN03C-030, CBTRN03C-031, X-TRANREPT-7; Behaviour constraint).
//! 5. A normal end returns code 0 and the generation path, which `main` prints (CBTRN03C-034).
//!
//! # Abends
//!
//! A lookup's `INVALID KEY` (CBTRN03C-014, CBTRN03C-016, CBTRN03C-018), a card number that is not
//! 16 digits (X-TRANREPT-H4), a failed query (CBTRN03C-007, CBTRN03C-010, CBTRN03C-040 to
//! CBTRN03C-042, X-TRANREPT-10) or a failed write (CBTRN03C-033) ends the job with its
//! [`JobError`]. The unfinished generation file is then removed, as `DISP=(NEW,CATLG,DELETE)`
//! deletes it at abend, so no path is printed and the next run takes the same generation number
//! (CBTRN03C-035).
//!
//! # Data versions (E23)
//!
//! The type and category descriptions come only from the VSAM data version, `transaction_types`
//! and `transaction_categories`, which the `trantype` and `trancatg` loads replace. No repository
//! of schema `db2` is used here, so a `mnttrdb2` change never reaches a report line
//! (CBTRN03C-038).
//!
//! # Job log and cardholder data
//!
//! The program's `DISPLAY` lines (CBTRN03C-006, CBTRN03C-008, CBTRN03C-012, CBTRN03C-014,
//! CBTRN03C-029, CBTRN03C-034, CBTRN03C-035) are not written: standard output carries only the
//! paths `main` prints from [`JobOutcome::written`] (AAP 0.3.4.3), and the job has no tracing of
//! its own. Report records hold no card number (CBTRN03C-023); the job's errors are the fixed
//! texts of [`JobError`] and its counts are plain numbers, so nothing the job emits carries a
//! card number, an SSN or a password (CBTRN03C-039; Cardholder Data Redaction).

use domain::batch::cbtrn03c::{DateParm, DateParmError, Flow, ReportError, ReportRecord, Tranrept};
use domain::records::{
    CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
};
use persistence::{
    CardXrefRepository, TransactionCategoryRepository, TransactionRepository,
    TransactionTypeRepository,
};

use super::{JobContext, JobError, JobOutcome, OutputFile, count};
use crate::cli::TranreptArgs;

/// The generation base of the report, `AWS.M2.CARDDEMO.TRANREPT(+1)` of `TRANREPT.jcl:80`
/// (CBTRN03C-032).
const REPORT_BASE: &str = "TRANREPT";

/// Runs TRANREPT over the window of `args`: writes the next `TRANREPT` generation under
/// `Config::reports_dir` and returns its path, return code 0, and the counts `records` (rows
/// read) and `report_lines` (records written).
///
/// # Errors
///
/// - [`JobError::Abend`] for a window `DateParm::new` refuses, before any table is read
///   (CBTRN03C-002); a command line `clap` parsed never gives one.
/// - [`JobError::Persistence`] when the selection or a lookup table cannot be read
///   (CBTRN03C-007, CBTRN03C-010).
/// - [`JobError::Report`] for the abends of `CBTRN03C`: a card, type or category key missing
///   from its table, or a card number that is not 16 digits (CBTRN03C-014, CBTRN03C-016,
///   CBTRN03C-018, CBTRN03C-035).
/// - [`JobError::Io`] when the generation file cannot be allocated, created, written or synced
///   (CBTRN03C-007, CBTRN03C-033, CBTRN03C-034).
///
/// After any error the generation file is removed (CBTRN03C-035).
pub(crate) async fn run(ctx: &JobContext<'_>, args: &TranreptArgs) -> Result<JobOutcome, JobError> {
    let parm = args.date_parm().map_err(window_error)?;
    // The store is a handle on the shared pool, cloned for the `&mut self` repository calls.
    let mut store = ctx.store.clone();
    let selected = store
        .find_processed_between(parm.start(), parm.end())
        .await?;
    let mut output = OutputFile::next_generation(&ctx.config.reports_dir(), REPORT_BASE)?;
    let xrefs = CardXrefRepository::find_all(&mut store).await?;
    let types = TransactionTypeRepository::find_all(&mut store).await?;
    let categories = TransactionCategoryRepository::find_all(&mut store).await?;
    let (read, records) = report(parm, &selected, xrefs, types, categories)?;
    for record in &records {
        output.write_record(record)?;
    }
    let path = output.finish()?;
    Ok(JobOutcome {
        written: vec![path],
        return_code: 0,
        counts: vec![
            ("records", count(read)),
            ("report_lines", count(records.len())),
        ],
    })
}

/// The read loop of `CBTRN03C` over `selected`, in order (`CBTRN03C.cbl:170-206`): each row is
/// passed to `Tranrept::process` until the first `Flow::Stop`, which reads no further row
/// (CBTRN03C-011), then `Tranrept::finish` gives the report records. Returns the number of rows
/// passed, the stopping one included, and the records.
///
/// # Errors
///
/// The [`ReportError`] that abended the run (CBTRN03C-035).
fn report(
    parm: DateParm,
    selected: &[TransactionRecord],
    xrefs: Vec<CardXrefRecord>,
    types: Vec<TransactionTypeRecord>,
    categories: Vec<TransactionCategoryRecord>,
) -> Result<(usize, Vec<ReportRecord>), ReportError> {
    let mut run = Tranrept::new(parm, xrefs, types, categories);
    let mut read = 0;
    for row in selected {
        read += 1;
        if run.process(row)? == Flow::Stop {
            break;
        }
    }
    Ok((read, run.finish()?))
}

/// The [`JobError`] of a window the domain refuses (CBTRN03C-002). `JobError` has no variant of
/// its own for it, because the date options' parser already refuses every such date, so the
/// unreachable case ends the job on the abend path with a fixed text naming the option, never
/// its value.
fn window_error(error: DateParmError) -> JobError {
    JobError::Abend {
        code: None,
        reason: match error {
            DateParmError::Start => "CBTRN03C: the start date is not a valid YYYY-MM-DD date",
            DateParmError::End => "CBTRN03C: the end date is not a valid YYYY-MM-DD date",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use copybook::{PicTimestamp, PicU, PicX};
    use domain::batch::cbtrn03c::{DateParm, DateParmError};
    use domain::money::{Decimal, Money};
    use domain::records::{
        CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
    };
    use domain::sensitive::CardNumber;

    use super::{report, window_error};

    type TestResult = Result<(), Box<dyn Error>>;

    /// A synthetic card number, in no fixture file.
    const CARD: [u8; 16] = *b"4000000000000001";

    /// A processing timestamp inside the default window, layout `D` (PIC-019).
    const INSIDE: &[u8; 26] = b"2022-07-01-10.15.42.370000";

    /// A processing timestamp the day after the default window ends.
    const OUTSIDE: &[u8; 26] = b"2022-07-07-00.00.00.000000";

    /// A purchase of 183.88 on [`CARD`] with transaction ID `n`, processed at `proc_ts`.
    fn purchase(n: u64, proc_ts: &[u8; 26]) -> Result<TransactionRecord, Box<dyn Error>> {
        Ok(TransactionRecord {
            id: PicX::from_text(&format!("{n:016}"))?,
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            source: PicX::from_text("POS TERM")?,
            desc: PicX::from_text("Purchase at Abshire-Lowe")?,
            amt: Money::new(Decimal::new(18_388, 2))?,
            merchant_id: PicU::new(800_000_000)?,
            merchant_name: PicX::from_text("Abshire-Lowe")?,
            merchant_city: PicX::from_text("North Enoshaven")?,
            merchant_zip: PicX::from_text("72112")?,
            card_num: CardNumber::new(PicX::from_array(CARD)),
            orig_ts: PicTimestamp::decode(b"2022-06-10 19:27:53.000000")?,
            proc_ts: PicTimestamp::decode(proc_ts)?,
            filler: PicX::spaces(),
        })
    }

    /// The rows of the three lookup tables a run is built with.
    struct Lookups {
        xrefs: Vec<CardXrefRecord>,
        types: Vec<TransactionTypeRecord>,
        categories: Vec<TransactionCategoryRecord>,
    }

    /// One run of [`report`]: the processing timestamps of its rows, which are numbered from 1,
    /// the rows it reads, the records it returns and how the last record begins.
    struct Case {
        stamps: &'static [&'static [u8; 26]],
        read: usize,
        records: usize,
        last_prefix: &'static [u8],
    }

    /// The lookup rows of [`CARD`] (account 50) and of type `01`, category `0001`, as
    /// `cardxref.txt`, `trantype.txt:1` and `trancatg.txt:1` describe them.
    fn lookups() -> Result<Lookups, Box<dyn Error>> {
        let xref = CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(50)?,
            acct_id: PicU::new(50)?,
            filler: PicX::spaces(),
        };
        let tran_type = TransactionTypeRecord {
            tran_type: PicX::from_array(*b"01"),
            desc: PicX::from_text("Purchase")?,
            filler: PicX::from_array(*b"00000000"),
        };
        let category = TransactionCategoryRecord {
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            type_desc: PicX::from_text("Regular Sales Draft")?,
            filler: PicX::from_array(*b"0000"),
        };
        Ok(Lookups {
            xrefs: vec![xref],
            types: vec![tran_type],
            categories: vec![category],
        })
    }

    /// Rows read and records written: a run over two rows ends with the end-of-file totals,
    /// while a row outside the window ends the loop, so the row after it is never read and no
    /// total is written. The records are the header group (4), the details, then the page total,
    /// its dashes and the grand total.
    // audit: CBTRN03C-011
    // audit: CBTRN03C-030
    #[test]
    fn rows_are_read_until_the_loop_is_left() -> TestResult {
        let cases = [
            Case {
                stamps: &[INSIDE, INSIDE],
                read: 2,
                records: 9,
                last_prefix: b"Grand Total",
            },
            Case {
                stamps: &[INSIDE, OUTSIDE, INSIDE],
                read: 2,
                records: 5,
                last_prefix: b"0000000000000001 00000000050 01-Purchase",
            },
        ];
        for case in cases {
            let rows = case
                .stamps
                .iter()
                .zip(1..)
                .map(|(stamp, n)| purchase(n, stamp))
                .collect::<Result<Vec<_>, _>>()?;
            let Lookups {
                xrefs,
                types,
                categories,
            } = lookups()?;
            let (read, records) = report(DateParm::DEFAULT, &rows, xrefs, types, categories)?;
            assert_eq!(read, case.read, "{} rows", rows.len());
            assert_eq!(records.len(), case.records, "{} rows", rows.len());
            let last = records.last().ok_or("no report record")?;
            assert!(last.starts_with(case.last_prefix), "{} rows", rows.len());
        }
        Ok(())
    }

    /// A missing cross-reference ends the run with the domain's error, whose job text names the
    /// failed lookup only.
    // audit: CBTRN03C-014
    #[test]
    fn a_missing_cross_reference_abends_the_run() -> TestResult {
        let rows = [purchase(1, INSIDE)?];
        let Lookups {
            types, categories, ..
        } = lookups()?;
        let error = report(DateParm::DEFAULT, &rows, Vec::new(), types, categories)
            .err()
            .ok_or("a card with no cross-reference was reported")?;
        assert_eq!(
            super::JobError::from(error).to_string(),
            "CBTRN03C: card number not in the card cross-reference"
        );
        Ok(())
    }

    /// A refused window ends the job on the abend path with no return code of its own and a
    /// text that names the option only.
    // audit: CBTRN03C-002
    #[test]
    fn refused_windows_name_the_option_only() {
        let cases = [
            (
                DateParmError::Start,
                "ABEND: CBTRN03C: the start date is not a valid YYYY-MM-DD date",
            ),
            (
                DateParmError::End,
                "ABEND: CBTRN03C: the end date is not a valid YYYY-MM-DD date",
            ),
        ];
        for (error, text) in cases {
            let job_error = window_error(error);
            assert_eq!(job_error.to_string(), text);
            assert_eq!(job_error.exit_code(), None);
        }
    }
}
