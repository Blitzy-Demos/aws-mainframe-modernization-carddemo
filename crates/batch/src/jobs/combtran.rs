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
//! combined file there before its load. A blocking task the runtime cancels while shutting down
//! is a setup failure.
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
//!   decoded, and a failure to allocate, write or catalog `TRANSACT.COMBINED(+1)`. No
//!   `TRANSACT.COMBINED(+1)` is then cataloged, `STEP10` does not run and the live master is
//!   untouched. A SORTIN that cannot be read or decoded is named by its dataset, generation and
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
use std::ops::ControlFlow;
use std::sync::Arc;

use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
use carddemo_domain::numeric::TranId;
use carddemo_persistence::repos::LoadOutcome;
use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;
use tracing::field::display;

use super::{JobContext, JobCounts, count_of, setup_failure};
use crate::exit::ExitStatus;
use crate::generations::{
    Dataset, Generation, Generations, blocking, read_records, read_records_async,
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

/// One `(0)` generation of the SORTIN concatenation and its records, in file order.
struct SortIn {
    generation: Generation,
    records: Vec<TransactionRecord>,
}

/// STEP05R `PGM=SORT` [app/jcl/COMBTRAN.jcl:L22-37] — P-COMBTRAN-001, P-COMBTRAN-004,
/// P-COMBTRAN-009.
///
/// Resolves `TRANSACT.BKUP(0)` and `SYSTRAN(0)` and records their generation names, in SORTIN
/// concatenation order and separated by a comma (`TRANSACT.BKUP.G0001V00,SYSTRAN.G0001V00`), in
/// `input` on the job span before either is read, so a SORTIN that cannot be read still shows
/// the inputs. Then reads both, in that order, sorts the records by [`sort_by_tran_id`] and
/// writes them to `TRANSACT.COMBINED(+1)` ([`write_combined`]). Sets `read` in `counts` to the
/// SORTIN records read, once `TRANSACT.BKUP(0)` is read and again once `SYSTRAN(0)` is, so a
/// `SYSTRAN(0)` that cannot be read leaves the `TRANSACT.BKUP(0)` records counted. `posted` and
/// `rejected` stay 0 when the step fails, because `STEP10` then never runs.
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
/// - An equivalence halt (exit 16, E-38) when two different records share a TRAN-ID. Nothing is
///   allocated or written.
fn sort_step(generations: &Generations, counts: &JobCounts) -> Result<Generation, ExitStatus> {
    let backup = current_sortin(generations, Dataset::TransactBkup)?;
    let systran = current_sortin(generations, Dataset::SysTran)?;
    Span::current().record("input", display(format_args!("{backup},{systran}")));
    let backup = read_sortin(backup)?;
    counts.set_read(count_of(backup.records.len()));
    let systran = read_sortin(systran)?;
    counts.set_read(count_of(backup.records.len() + systran.records.len()));
    write_combined(generations, backup, systran)
}

/// STEP05R `SORT FIELDS=(TRAN-ID,A)` and `SORTOUT` [app/jcl/COMBTRAN.jcl:L28-37] —
/// P-COMBTRAN-001, P-COMBTRAN-004, P-COMBTRAN-009: the records of `backup` then `systran`, the
/// SORTIN concatenation, sorted by [`sort_by_tran_id`] and written to `TRANSACT.COMBINED(+1)`,
/// 350-byte `CVTRA05Y` records as read, FILLER included (`DCB=(*.SORTIN)`), each followed by
/// `LF`. Records the number of combined records in `written` on the job span.
///
/// # Errors
///
/// - A setup failure (exit 16) when `TRANSACT.COMBINED(+1)` cannot be allocated, written or
///   cataloged.
/// - An equivalence halt (exit 16, E-38) when two different records share a TRAN-ID. Nothing is
///   allocated or written.
fn write_combined(
    generations: &Generations,
    backup: SortIn,
    systran: SortIn,
) -> Result<Generation, ExitStatus> {
    let backup_records = backup.records.len();
    let mut records = backup.records;
    records.extend(systran.records);
    let read = records.len();
    let sorted = sort_by_tran_id(records).map_err(|conflict| {
        let halt = EqualKeyHalt {
            conflict,
            backup: &backup.generation,
            backup_records,
            systran: &systran.generation,
        };
        tracing::error!(
            tran_id = %conflict.tran_id,
            escalation = EQUAL_KEY_ESCALATION,
            "{halt}"
        );
        ExitStatus::equivalence_halt(&halt, EQUAL_KEY_ESCALATION)
    })?;
    let mut pending = generations
        .begin_next(Dataset::TransactCombined)
        .map_err(|err| setup_failure("TRANSACT.COMBINED(+1) allocation failed", err))?;
    pending
        .append_records::<TransactionLayout>(&sorted)
        .map_err(|err| setup_failure("TRANSACT.COMBINED(+1) write failed", err))?;
    let combined = pending
        .catalog()
        .map_err(|err| setup_failure("TRANSACT.COMBINED(+1) catalog failed", err))?;
    Span::current().record("written", sorted.len());
    tracing::info!(
        backup = %backup.generation,
        systran = %systran.generation,
        generation = %combined,
        records_in = read,
        records_out = sorted.len(),
        "SORT FIELDS=(TRAN-ID,A) completed"
    );
    Ok(combined)
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

/// `generation` and its records, read with the `CVTRA05Y` layout.
///
/// # Errors
///
/// A setup failure (exit 16) when the file cannot be read or decoded. Both SORTIN datasets hold
/// `CVTRA05Y` records, so the text names the one that failed by its dataset, its generation and
/// its path before the cause, for example `SYSTRAN(0) SYSTRAN.G0001V00 at
/// var/batch/SYSTRAN.G0001V00: record file decode failed: CVTRA05Y record 1 …`. The same text is
/// the `error` of the logged event.
fn read_sortin(generation: Generation) -> Result<SortIn, ExitStatus> {
    match read_records::<TransactionLayout>(&generation.path) {
        Ok(records) => Ok(SortIn {
            generation,
            records,
        }),
        Err(err) => Err(setup_failure(
            "SORTIN read failed",
            format!(
                "{}(0) {generation} at {}: {err}",
                generation.dataset.name(),
                generation.path.display()
            ),
        )),
    }
}

/// Two different records of the SORT input with one TRAN-ID, by their positions in the SORTIN
/// concatenation, counted from 0, `first` before `second`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EqualKeyConflict {
    tran_id: TranId,
    first: usize,
    second: usize,
}

/// `SORT FIELDS=(TRAN-ID,A)` with `TRAN-ID,1,16,CH` [app/jcl/COMBTRAN.jcl:L28-30] —
/// P-COMBTRAN-001, P-COMBTRAN-009: the records in ascending order of their 16 TRAN-ID bytes,
/// `CH` being the unsigned byte order.
///
/// The SORT names no `EQUALS`, and the installation default acts as `NOEQUALS` for fixed-length
/// records, so the audit gives records with equal keys no order (E-38). Equal-key records that
/// are identical give the same output in any order and are kept, in input order.
///
/// # Errors
///
/// The first [`EqualKeyConflict`] in key order: two records with one TRAN-ID that differ in any
/// byte. Their order, and with it the record `REPRO` loads, is what the COBOL leaves undefined,
/// so no order is chosen for them.
fn sort_by_tran_id(
    records: Vec<TransactionRecord>,
) -> Result<Vec<TransactionRecord>, EqualKeyConflict> {
    let mut keyed: Vec<(usize, TransactionRecord)> = records.into_iter().enumerate().collect();
    // Stable: equal keys keep their input positions in ascending order.
    keyed.sort_by_key(|(_, record)| *record.transaction.tran_id.as_bytes());
    let conflict = keyed.windows(2).find_map(|pair| match pair {
        [(first, earlier), (second, later)]
            if earlier.transaction.tran_id == later.transaction.tran_id && earlier != later =>
        {
            Some(EqualKeyConflict {
                tran_id: earlier.transaction.tran_id,
                first: *first,
                second: *second,
            })
        }
        _ => None,
    });
    match conflict {
        Some(conflict) => Err(conflict),
        None => Ok(keyed.into_iter().map(|(_, record)| record).collect()),
    }
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
/// cataloged, on the blocking pool, loads its records into the live master in file order by
/// [`load_records`] inside one unit of work, and commits. Once the load is committed, [`finish`]
/// sets `posted` in `counts` to the loaded records and `rejected` to the duplicate records, and
/// the step ends with the `REPRO` condition code:
///
/// - 0 when every record loaded, an empty file included (P-COMBTRAN-002, P-COMBTRAN-005);
/// - 8 after one to three duplicate records (P-COMBTRAN-003, P-COMBTRAN-008);
/// - 12 when the fourth duplicate record stops the load (P-COMBTRAN-003, P-COMBTRAN-006,
///   P-COMBTRAN-007).
///
/// A setup failure (exit 16) ends the step, through [`not_committed`], when the file cannot be
/// read or decoded or the unit of work cannot begin, before any record is loaded; when a record
/// cannot be loaded, after which the load is rolled back; or when the commit fails. Each leaves
/// the cataloged `TRANSACT.COMBINED(+1)` in place and the live master as the run found it, and
/// leaves `posted` 0 and `rejected` the duplicate records met before the failure.
async fn repro_step(db: &Db, combined: &Generation, counts: &JobCounts) -> ExitStatus {
    let mut tally = ReproTally::default();
    let records = match read_records_async::<TransactionLayout>(combined.path.clone()).await {
        Ok(records) => records,
        Err(err) => {
            return not_committed(
                tally,
                counts,
                setup_failure("TRANSACT.COMBINED(+1) read failed", err),
            );
        }
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
    if let Err(status) = load_records(&mut uow, &records, &mut tally).await {
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
    finish(tally, records.len(), counts)
}

/// The `REPRO` record loop of STEP10 [app/jcl/COMBTRAN.jcl:L48] — P-COMBTRAN-003: each record,
/// in file order, through `PgStore::load_combined_record`, counted in `tally`, until
/// [`ReproTally::count`] stops the load at the error limit.
///
/// A duplicate record emits one `IDC3308I DUPLICATE RECORD` warning with its record number and
/// TRAN-ID, and the stop one `IDC31467I MAXIMUM ERROR LIMIT REACHED` warning.
///
/// # Errors
///
/// A setup failure (exit 16) naming the record number and TRAN-ID of the first record whose
/// load fails. The audit records no condition code for it. `tally` then holds the records
/// counted before that one.
async fn load_records(
    uow: &mut UnitOfWork,
    records: &[TransactionRecord],
    tally: &mut ReproTally,
) -> Result<(), ExitStatus> {
    for (index, record) in records.iter().enumerate() {
        let record_number = index + 1;
        let tran_id = record.transaction.tran_id;
        let outcome = uow
            .store()
            .load_combined_record(record.transaction.clone())
            .await
            .map_err(|err| {
                setup_failure(
                    "live transaction master load failed",
                    format!(
                        "TRANSACT.COMBINED(+1) record {record_number} TRAN-ID {tran_id}: {err}"
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
            break;
        }
    }
    Ok(())
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

    use super::{ReproTally, finish, not_committed, sort_step};
    use crate::cli::Command;
    use crate::exit::ExitStatus;
    use crate::generations::{Dataset, Generation, read_records};
    use crate::jobs::tests::job_span_line;
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
        let unloaded = [r#""posted":"0""#, r#""rejected":"0""#];
        // (case, the SYSTRAN(0) record, span fragments present, span fragments absent). The
        // exporter writes an unsigned count as a decimal string.
        let cases: [(&str, String, &[&str], &[&str]); 3] = [
            // The two TRANSACT.BKUP(0) records were read before SYSTRAN(0) failed.
            (
                "SYSTRAN does not decode",
                bad_amount("0000000000000005", "INTEREST 5"),
                &[input, r#""read":"2""#],
                &[r#""written""#],
            ),
            (
                "E-38 halt",
                record("0000000000000009", "INTEREST 9"),
                &[input, r#""read":"3""#],
                &[r#""written""#],
            ),
            (
                "sorted",
                record("0000000000000005", "INTEREST 5"),
                &[input, r#""read":"3""#, r#""written":"3""#],
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
                assert!(span.contains(fragment), "{case}: {fragment} in {span}");
            }
            for fragment in absent {
                assert!(!span.contains(fragment), "{case}: no {fragment} in {span}");
            }
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
