//! The eight fixture loads of `carddemo-batch`: the IDCAMS jobs that delete, define and `REPRO`
//! a VSAM cluster from an ASCII fixture, each now the replacement of one table
//! (`modern/audit/batch/LOADS.md`, LOADS-001 to LOADS-008; AAP 0.3.3.4), and the hand-off of the
//! ninth load, `dusrsecj`, to its own module (LOADS-009).
//!
//! | Subcommand | JCL (`app/jcl/`) | Fixture | Record (copybook) | `KEYS` / `RECORDSIZE` | Table | Rule |
//! |---|---|---|---|---|---|---|
//! | `acctfile` | `ACCTFILE.jcl:40-41,61` | `acctdata.txt` | `AccountRecord` (`CVACT01Y`) | `(11 0)` / 300 | `accounts` | LOADS-001 |
//! | `cardfile` | `CARDFILE.jcl:54-55,75` | `carddata.txt` | `CardRecord` (`CVACT02Y`) | `(16 0)` / 150 | `cards` | LOADS-002 |
//! | `xreffile` | `XREFFILE.jcl:43-44,64` | `cardxref.txt` | `CardXrefRecord` (`CVACT03Y`) | `(16 0)` / 50 | `card_xref` | LOADS-003 |
//! | `custfile` | `CUSTFILE.jcl:50-51,71` | `custdata.txt` | `CustomerRecord` (`CVCUS01Y`) | `(9 0)` / 500 | `customers` | LOADS-004 |
//! | `discgrp` | `DISCGRP.jcl:40-41,61` | `discgrp.txt` | `DisclosureGroupRecord` (`CVTRA02Y`) | `(16 0)` / 50 | `disclosure_groups` | LOADS-005 |
//! | `tcatbalf` | `TCATBALF.jcl:40-41,61` | `tcatbal.txt` | `TranCatBalanceRecord` (`CVTRA01Y`) | `(17 0)` / 50 | `tran_cat_balances` | LOADS-006 |
//! | `trancatg` | `TRANCATG.jcl:40-41,61` | `trancatg.txt` | `TransactionCategoryRecord` (`CVTRA04Y`) | `(6 0)` / 60 | `transaction_categories` | LOADS-007 |
//! | `trantype` | `TRANTYPE.jcl:40-41,61` | `trantype.txt` | `TransactionTypeRecord` (`CVTRA03Y`) | `(2 0)` / 60 | `transaction_types` | LOADS-008 |
//!
//! The fixtures hold 50, 50, 50, 50, 51, 50, 18 and 7 records (FIXLOAD-001 to FIXLOAD-008).
//! The input is the path `main` resolves with `LoadArgs::input_path`: `--input` when given, else
//! the job's fixture in `CARDDEMO_FIXTURE_DIR` (LOADS-010). It is read in place, only read, and
//! never copied (LOADS-011).
//!
//! # One load
//!
//! Every fixture job runs the one body of `load`:
//!
//! 1. The whole input is read; an unreadable file fails the job before the table is touched
//!    (LOADS-019).
//! 2. Codec A decodes every line into a framed record, and the keys, the first `KEYS(n 0)`
//!    bytes of each record, must rise strictly. The first line refused fails the job, naming
//!    that line and the failing field, before the transaction opens; nothing is skipped,
//!    defaulted or normalised (LOADS-016, LOADS-017, FIXFRM-020, R7).
//! 3. The framing is dropped: `LineFraming` describes the file line, not the record, and no
//!    column stores it (LOADS-030, LOADS-031).
//! 4. One `PostgreSQL` transaction deletes every row of the table and inserts every record
//!    through codec B in file order, the counterpart of `DELETE CLUSTER`, `DEFINE CLUSTER` and
//!    `REPRO` into the new, empty cluster; once it commits the table holds exactly the input set,
//!    and an empty table before the run is no error (LOADS-014, LOADS-015). A failed statement
//!    or commit drops the uncommitted transaction, which rolls it back, so every failure leaves
//!    the previous contents (LOADS-018).
//!
//! # Fixture shapes kept as they are
//!
//! - E04: every `cardxref.txt` body is 36 bytes against the 50-byte `CVACT03Y`; the missing
//!   tail is the trailing `FILLER`, which decodes as spaces and is stored as 14 spaces in
//!   `card_xref.filler`, while the body length stays framing (LOADS-030, PIC-013).
//! - E05: the `0` characters in the `FILLER` of `discgrp.txt`, `tcatbal.txt`, `trancatg.txt`
//!   and `trantype.txt` are stored byte for byte in each table's `filler` column (LOADS-032,
//!   PIC-012).
//! - E06: the mixed CRLF and LF terminators of `tcatbal.txt`, `trancatg.txt` and `trantype.txt`
//!   are framing only, so no CR reaches a column (LOADS-031, PIC-014).
//!
//! # What a load leaves alone
//!
//! - No foreign key joins the tables the loads replace, so they succeed in any order, on an
//!   empty or a loaded database, and a rerun leaves the same copybook-derived contents; only
//!   `cards.card_ref`, which the database assigns anew to every inserted card, differs
//!   (LOADS-025 to LOADS-027).
//! - A load writes only its own table (LOADS-028). `trancatg` and `trantype` fill only the VSAM
//!   data version, `transaction_categories` and `transaction_types`; schema `db2`, seeded once
//!   by its migration, is never read or written, and nothing is copied between the two
//!   versions (LOADS-029, E23).
//! - No schema work (Single Schema Executor): the tables, keys and indexes come from the
//!   migrations `carddemo-migrate` applies, a load issues no DDL, and against an unmigrated
//!   database it fails at its first statement with [`JobError::Persistence`] (LOADS-020 to
//!   LOADS-024).
//! - No output: a load writes no file, so [`JobOutcome::written`] is empty and `main` prints no
//!   path line; it reports only the count of rows written as `records`, for the event `main`
//!   logs (LOADS-038). It reads no clock and contacts nothing but `PostgreSQL` (LOADS-037).
//!
//! # Redaction
//!
//! Card numbers, CVVs and SSNs are stored in full, as the copybooks hold them, and never leave
//! the records: a failure is a `LoadError`, which names line numbers, a field, an offset and a
//! width, or a `PersistenceError`, which names an SQLSTATE and a constraint, never a value
//! (LOADS-016, LOADS-018, LOADS-039, Cardholder Data Redaction).

use std::path::Path;

use copybook::FixedRecord;
use domain::batch::loads::{LoadError, LoadJob, check_key_sequence, decode_fixture};
use domain::records::{
    AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
    TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord,
};
use persistence::{
    AccountRepository, CardRepository, CardXrefRepository, CustomerRepository,
    DisclosureGroupRepository, PersistenceError, PgTx, TranCatBalanceRepository,
    TransactionCategoryRepository, TransactionTypeRepository,
};

use super::{JobContext, JobError, JobOutcome};

/// A copybook layout that one fixture load writes: the job that loads it and the replacement of
/// its table.
trait Table: FixedRecord {
    /// The job that loads the layout, whose `KEYS(n 0)` length the key-sequence check applies
    /// (LOADS-001 to LOADS-008, LOADS-017).
    const JOB: LoadJob;

    /// Deletes every row of the layout's table and inserts `records` in slice order, inside
    /// `tx`, returning how many rows were inserted (LOADS-014).
    fn replace(
        tx: &mut PgTx<'_>,
        records: &[Self],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

/// Implements [`Table`] for each layout by delegating to the `replace_all` of its table's
/// repository. Every repository trait names the method `replace_all`, so the call names the
/// trait to choose one.
macro_rules! tables {
    ($($record:ty => $job:ident, $repository:ident;)+) => {$(
        impl Table for $record {
            const JOB: LoadJob = LoadJob::$job;

            fn replace(
                tx: &mut PgTx<'_>,
                records: &[Self],
            ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
                $repository::replace_all(tx, records)
            }
        }
    )+};
}

tables! {
    AccountRecord => Acctfile, AccountRepository;
    CardRecord => Cardfile, CardRepository;
    CardXrefRecord => Xreffile, CardXrefRepository;
    CustomerRecord => Custfile, CustomerRepository;
    DisclosureGroupRecord => Discgrp, DisclosureGroupRepository;
    TranCatBalanceRecord => Tcatbalf, TranCatBalanceRepository;
    TransactionCategoryRecord => Trancatg, TransactionCategoryRepository;
    TransactionTypeRecord => Trantype, TransactionTypeRepository;
}

/// Runs the load `job` over the file at `input`: one of the eight fixture loads, or `dusrsecj`,
/// which has its own module (LOADS-001 to LOADS-009).
///
/// A fixture load replaces its table with the records of `input` inside one transaction and
/// returns 0 with the count of rows written as `records` and no file (LOADS-014, LOADS-038).
///
/// # Errors
///
/// - [`JobError::Io`] when `input` cannot be read (LOADS-019).
/// - [`JobError::Load`] when codec A refuses a line or a key does not rise above the previous
///   line's key, naming the line (LOADS-016, LOADS-017).
/// - [`JobError::Persistence`] when the transaction cannot be opened, the delete or an insert
///   fails, or the commit is refused (LOADS-018, LOADS-021).
/// - For `dusrsecj`, the errors of its own `run` as well.
///
/// Every failure leaves the previous contents of the table.
pub(crate) async fn run(
    ctx: &JobContext<'_>,
    job: LoadJob,
    input: &Path,
) -> Result<JobOutcome, JobError> {
    match job {
        LoadJob::Acctfile => load::<AccountRecord>(ctx, input).await,
        LoadJob::Cardfile => load::<CardRecord>(ctx, input).await,
        LoadJob::Xreffile => load::<CardXrefRecord>(ctx, input).await,
        LoadJob::Custfile => load::<CustomerRecord>(ctx, input).await,
        LoadJob::Discgrp => load::<DisclosureGroupRecord>(ctx, input).await,
        LoadJob::Tcatbalf => load::<TranCatBalanceRecord>(ctx, input).await,
        LoadJob::Trancatg => load::<TransactionCategoryRecord>(ctx, input).await,
        LoadJob::Trantype => load::<TransactionTypeRecord>(ctx, input).await,
        LoadJob::Dusrsecj => super::dusrsecj::run(ctx, input).await,
    }
}

/// The one body of every fixture load: read `input`, decode and check it, then replace the
/// table of `R` in one transaction (LOADS-014 to LOADS-019). The read and the decoding block,
/// so they run as one step on the blocking pool, awaited before the transaction opens
/// (W011-F05); their errors are those of an inline run, in the same order.
async fn load<R: Table + Send + 'static>(
    ctx: &JobContext<'_>,
    input: &Path,
) -> Result<JobOutcome, JobError> {
    let input = input.to_path_buf();
    let records = super::blocking(super::READING_INPUT, move || {
        let bytes = super::read_input(&input)?;
        decode::<R>(&bytes).map_err(JobError::from)
    })
    .await??;

    // Dropping `tx` on an early return rolls the replacement back (LOADS-018).
    let mut tx = ctx.store.begin().await?;
    let written = R::replace(&mut tx, &records).await?;
    tx.commit().await?;

    Ok(JobOutcome {
        written: Vec::new(),
        return_code: 0,
        counts: vec![("records", written)],
    })
}

/// The records of the fixture `bytes` in file order: codec A over every line, then the
/// key-sequence check of `R`'s job, then the framing dropped, since no column stores it
/// (LOADS-016, LOADS-017, LOADS-030, LOADS-031).
///
/// # Errors
///
/// The [`LoadError`] of the first line codec A refuses, or of the first record whose key does
/// not rise above the previous one.
fn decode<R: Table>(bytes: &[u8]) -> Result<Vec<R>, LoadError> {
    let framed = decode_fixture::<R>(bytes)?;
    check_key_sequence(R::JOB, &framed)?;
    Ok(framed.into_iter().map(|line| line.record).collect())
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::PathBuf;

    use domain::batch::loads::{LoadErrorKind, LoadJob};
    use domain::records::{
        AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
        TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord,
    };

    use super::{Table, decode};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The fixture `file`, read in place from `app/data/ASCII/`.
    fn fixture(file: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file);
        Ok(fs::read(path)?)
    }

    /// Decodes `R`'s own fixture and returns its record count, after checking that the layout
    /// is the one its job loads.
    fn fixture_count<R: Table>() -> Result<usize, Box<dyn Error>> {
        assert_eq!(R::WIDTH, R::JOB.record_width(), "{}", R::JOB.subcommand());
        let file = R::JOB
            .fixture_file()
            .ok_or("a fixture load has a fixture file")?;
        let records = decode::<R>(&fixture(file)?)?;
        Ok(records.len())
    }

    /// Every fixture decodes into its layout with rising keys, and each layout is tied to the
    /// job the dispatch of `run` sends to it, in `dev-up.sh` order (FIXLOAD-001 to FIXLOAD-008).
    #[test]
    fn every_layout_loads_its_own_fixture() -> TestResult {
        let counts = [
            (AccountRecord::JOB, fixture_count::<AccountRecord>()?),
            (CardRecord::JOB, fixture_count::<CardRecord>()?),
            (CardXrefRecord::JOB, fixture_count::<CardXrefRecord>()?),
            (CustomerRecord::JOB, fixture_count::<CustomerRecord>()?),
            (
                DisclosureGroupRecord::JOB,
                fixture_count::<DisclosureGroupRecord>()?,
            ),
            (
                TranCatBalanceRecord::JOB,
                fixture_count::<TranCatBalanceRecord>()?,
            ),
            (
                TransactionCategoryRecord::JOB,
                fixture_count::<TransactionCategoryRecord>()?,
            ),
            (
                TransactionTypeRecord::JOB,
                fixture_count::<TransactionTypeRecord>()?,
            ),
        ];
        let expected = [50, 50, 50, 50, 51, 50, 18, 7];
        let jobs = counts.map(|(job, _)| job);
        assert_eq!(jobs.as_slice(), LoadJob::ALL.get(..8).ok_or("nine jobs")?);
        assert_eq!(counts.map(|(_, count)| count), expected);
        Ok(())
    }

    /// A refused line and a key out of sequence each fail the decode with their line number and
    /// no record bytes, so nothing reaches the transaction (LOADS-016, LOADS-017).
    #[test]
    fn a_refused_line_or_key_fails_before_any_write() -> TestResult {
        let types = fixture("trantype.txt")?;
        let first_line_end = types
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or("trantype.txt has a terminated line")?;
        let first = types.get(..=first_line_end).ok_or("first line")?;

        let mut short = first.to_vec();
        short.truncate(10);
        short.push(b'\n');
        let mut doubled = first.to_vec();
        doubled.extend_from_slice(first);

        let short_err = decode::<TransactionTypeRecord>(&[first, short.as_slice()].concat())
            .err()
            .ok_or("a short line was accepted")?;
        assert_eq!(short_err.line(), Some(2));
        assert!(matches!(short_err.kind(), LoadErrorKind::Record(_)));

        let key_err = decode::<TransactionTypeRecord>(&doubled)
            .err()
            .ok_or("a duplicate key was accepted")?;
        assert_eq!(key_err.line(), Some(2));
        assert_eq!(
            key_err.kind(),
            LoadErrorKind::KeySequence { previous_line: 1 }
        );

        let text = format!("{short_err} {key_err}");
        assert!(!text.contains("Purchase"), "{text}");
        assert!(decode::<TransactionTypeRecord>(b"")?.is_empty());
        Ok(())
    }
}
