//! The JCL job layer: one module per `batch` subcommand, each running the steps of the job it
//! replaces in their JCL order.
//!
//! | Subcommand | Module | JCL job |
//! |---|---|---|
//! | `migrate` | `migrate` | CREADB21, schema only |
//! | `seed` | `seed` | ACCTFILE … DUSRSECJ, the CREADB21 loads, TRANEXTR |
//! | `posttran` | `posttran` | POSTTRAN (CBTRN02C) |
//! | `intcalc` | `intcalc` | INTCALC (CBACT04C) |
//! | `tranbkp` | `tranbkp` | TRANBKP (REPROC, IDCAMS DELETE/DEFINE) |
//! | `combtran` | `combtran` | COMBTRAN (SORT, IDCAMS REPRO) |
//! | `creastmt` | `creastmt` | CREASTMT (CBSTM03A, CBSTM03B) |
//! | `tranrept` | `tranrept` | TRANREPT (REPROC, SORT, CBTRN03C) |
//! | `cbpaup0j` | `cbpaup0j` | CBPAUP0J (CBPAUP0C) |
//! | `mnttrdb2` | `mnttrdb2` | MNTTRDB2 (COBTUPDT) |
//!
//! A job only orchestrates. The program logic comes from `carddemo_domain::rules`, every
//! database access runs in a `UnitOfWork` of `carddemo_persistence`, generation files go through
//! [`crate::generations`], the outcome is a [`crate::exit::ExitStatus`], and telemetry goes
//! through `tracing` inside the job span [`dispatch`] opens. The REPROC backup that TRANBKP and
//! TRANREPT share lives here, in [`backup_live_master`] and [`backup_live_master_in`], and nowhere
//! else.
//!
//! # Execution model
//!
//! Jobs run on Tokio's multi-threaded runtime, and a job future itself only awaits: database work
//! and work on Tokio's blocking pool. Every synchronous file-system, codec and CPU phase of a job
//! (generation allocation, appends, catalogs, whole-file reads, sorting, encoding, PDF rendering,
//! fixture preparation) runs through [`crate::generations::blocking`] or the asynchronous methods
//! of [`crate::generations`], inside the job span, so its events and its span records land there
//! as an inline call's would. A blocking task that the runtime cancels because it is shutting
//! down is a setup failure (exit 16), unless the generation API maps it to the I/O error of the
//! operation it cancelled. A panic in blocking work propagates to the job as an inline call's
//! would.

mod cbpaup0j;
mod combtran;
mod creastmt;
mod intcalc;
mod migrate;
mod mnttrdb2;
mod posttran;
mod seed;
mod tranbkp;
mod tranrept;

use std::fmt;

use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
use carddemo_domain::clock::AnyClock;
use carddemo_persistence::{Db, UnitOfWork};
use carddemo_runtime::config::Settings;
use tracing::field::{Empty, display};
use tracing::{Instrument, Span};

use crate::cli::Command;
use crate::exit::{ExitStatus, RECORDS_REJECTED, SUCCESS};
use crate::generations::{Dataset, Generation, Generations, PendingGeneration};

/// The condition code `REPRO` ends with when its input holds no record: `IDC3351I VSAM OPEN
/// RETURN CODE IS 160` (P-TRANBKP-002, P-TRANREPT-001, E-37).
const EMPTY_INPUT_CONDITION: u8 = 12;

/// The live records one read of the backup returns: a bound on the records the job holds at
/// once, not a COBOL value. `REPRO` copies one record at a time.
const BACKUP_PAGE_SIZE: u16 = 500;

/// What every job of one `batch` run shares: the settings, the processing clock and the
/// generation catalog.
#[derive(Debug)]
pub(crate) struct JobContext {
    /// The `CARDDEMO_*` settings of the run.
    pub(crate) settings: Settings,
    /// The processing clock: the host clock, or the `CARDDEMO_CLOCK_OVERRIDE` instant.
    pub(crate) clock: AnyClock,
    /// The GDGs under `CARDDEMO_BATCH_DIR`.
    pub(crate) generations: Generations,
}

impl JobContext {
    /// The context of a run whose generations live under `settings.batch_dir`
    /// (`CARDDEMO_BATCH_DIR`, default `./var/batch/`). Nothing is created until a job writes a
    /// generation.
    #[must_use]
    pub(crate) fn new(settings: Settings, clock: AnyClock) -> Self {
        let generations = Generations::new(settings.batch_dir.clone());
        Self {
            settings,
            clock,
            generations,
        }
    }
}

/// Runs `command` inside its job span and returns how it ended.
///
/// The span `job` is opened first and carries the subcommand name in `job` and the job's
/// inputs (`input`, `parm_date`, `start_date`, `end_date`). The `cbpaup0j` card is not
/// recorded whole: `sysin` holds the 17-byte `PRM-INFO` receiver it fills, `sysin_len` the
/// card's length in bytes and `sysin_truncated` whether the receiver left any of the card out
/// (`cbpaup0j::BoundedSysin`). Inside the span the database pool is opened and the job runs.
/// Jobs record their counts on it with
/// `tracing::Span::current().record("<field>", value)` for the fields `read`, `posted`,
/// `rejected`, `written` and `deleted`. When the job returns, `exit_code` is recorded and one
/// job-end event is emitted inside the span: `info` for exit 0, `warn` for 4, `error` for every
/// other code. The database URL carries the password and is never a span field.
///
/// The caller reports the returned status on stderr and exits with its code.
pub(crate) async fn dispatch(command: Command, ctx: &JobContext) -> ExitStatus {
    let job = job_name(&command);
    let span = job_span(job, &command);
    let status = run(command, ctx).instrument(span.clone()).await;
    let exit_code = status.code();
    span.record("exit_code", exit_code);
    span.in_scope(|| match exit_code {
        SUCCESS => tracing::info!(job, exit_code, "job ended"),
        RECORDS_REJECTED => tracing::warn!(job, exit_code, "job ended"),
        _ => tracing::error!(job, exit_code, "job ended"),
    });
    status
}

/// The subcommand name of `command`, as `cli` declares it.
const fn job_name(command: &Command) -> &'static str {
    match command {
        Command::Migrate => "migrate",
        Command::Seed(_) => "seed",
        Command::Posttran(_) => "posttran",
        Command::Intcalc(_) => "intcalc",
        Command::Tranbkp => "tranbkp",
        Command::Combtran => "combtran",
        Command::Creastmt => "creastmt",
        Command::Tranrept(_) => "tranrept",
        Command::Cbpaup0j(_) => "cbpaup0j",
        Command::Mnttrdb2(_) => "mnttrdb2",
    }
}

/// The span of one job run, with the inputs of `command` recorded and every count and the exit
/// code still empty.
fn job_span(job: &'static str, command: &Command) -> Span {
    let span = tracing::info_span!(
        "job",
        job,
        input = Empty,
        parm_date = Empty,
        start_date = Empty,
        end_date = Empty,
        sysin = Empty,
        sysin_len = Empty,
        sysin_truncated = Empty,
        read = Empty,
        posted = Empty,
        rejected = Empty,
        written = Empty,
        deleted = Empty,
        exit_code = Empty,
    );
    match command {
        Command::Posttran(args) => {
            span.record("input", display(args.input.display()));
        }
        Command::Mnttrdb2(args) => {
            span.record("input", display(args.input.display()));
        }
        Command::Intcalc(args) => {
            span.record("parm_date", args.parm_date.as_str());
        }
        Command::Tranrept(args) => {
            span.record("start_date", args.start_date.as_str());
            span.record("end_date", args.end_date.as_str());
        }
        Command::Cbpaup0j(args) => {
            let sysin = cbpaup0j::BoundedSysin::of(&args.sysin);
            span.record("sysin", display(sysin.prm_info));
            span.record("sysin_len", sysin.len);
            span.record("sysin_truncated", sysin.truncated);
        }
        Command::Migrate
        | Command::Seed(_)
        | Command::Tranbkp
        | Command::Combtran
        | Command::Creastmt => {}
    }
    span
}

/// Opens the database pool, then runs the job `command` names on it.
async fn run(command: Command, ctx: &JobContext) -> ExitStatus {
    let db = match connect(ctx).await {
        Ok(db) => db,
        Err(status) => return status,
    };
    match command {
        Command::Migrate => migrate::run(ctx, &db).await,
        Command::Seed(args) => seed::run(ctx, &db, &args).await,
        Command::Posttran(args) => posttran::run(ctx, &db, &args).await,
        Command::Intcalc(args) => intcalc::run(ctx, &db, &args).await,
        Command::Tranbkp => tranbkp::run(ctx, &db).await,
        Command::Combtran => combtran::run(ctx, &db).await,
        Command::Creastmt => creastmt::run(ctx, &db).await,
        Command::Tranrept(args) => tranrept::run(ctx, &db, &args).await,
        Command::Cbpaup0j(args) => cbpaup0j::run(ctx, &db, &args).await,
        Command::Mnttrdb2(args) => mnttrdb2::run(ctx, &db, &args).await,
    }
}

/// Opens the pool on `CARDDEMO_DATABASE_URL`. A JCL job has no counterpart of a connection
/// failure, so it is a setup failure (exit 16).
async fn connect(ctx: &JobContext) -> Result<Db, ExitStatus> {
    carddemo_persistence::connect(&ctx.settings.database_url)
        .await
        .map_err(|err| setup_failure("database connection failed", err))
}

/// `TRANSACT.BKUP(+1)` as the REPROC step left it. The step returns this rather than the bare
/// [`Generation`] because its condition code depends on the record count, and only the calling
/// job, which runs the later steps, can end with it.
#[derive(Debug)]
pub(super) struct Backup {
    /// The cataloged generation.
    pub(super) generation: Generation,
    /// The number of records `REPRO` copied into it.
    pub(super) records: usize,
}

impl Backup {
    /// The condition code the `REPRO` step ends with, as the exit status it gives: 0 when the
    /// live master held records (P-TRANBKP-001), 12 when it was empty (P-TRANBKP-002,
    /// P-TRANREPT-001, E-37). After a 12, TRANBKP still runs `STEP05` and bypasses `STEP10`
    /// (`COND=(4,LT)`), and the TRANREPT steps, which have no `COND`, all run; both jobs end
    /// with this code.
    pub(super) fn status(&self) -> ExitStatus {
        if self.records == 0 {
            ExitStatus::ReproCondition(EMPTY_INPUT_CONDITION)
        } else {
            ExitStatus::Success
        }
    }
}

/// REPROC `PRC001` `IDCAMS REPRO` [app/proc/REPROC.prc:L21-28] with
/// `FILEIN=AWS.M2.CARDDEMO.TRANSACT.VSAM.KSDS` and `FILEOUT=AWS.M2.CARDDEMO.TRANSACT.BKUP(+1)`
/// `DISP=(NEW,CATLG,DELETE)`, `DCB=(LRECL=350,RECFM=FB)`: TRANBKP `STEP05R`
/// [app/jcl/TRANBKP.jcl:L23-33] (P-TRANBKP-001, P-TRANBKP-002) and TRANREPT `STEP05R`
/// [app/jcl/TRANREPT.jcl:L23-33], the `STEP01R` of [app/proc/TRANREPT.prc:L21-31]
/// (P-TRANREPT-001).
///
/// Allocates `TRANSACT.BKUP(+1)`, reads every record of the live transaction master in TRAN-ID
/// order, [`BACKUP_PAGE_SIZE`] records at a time, writes each as a 350-byte `CVTRA05Y` record
/// with FILLER spaces (P-CVTRA05Y-018) followed by `LF`, and catalogs the generation. An empty
/// live master gives an empty generation, cataloged all the same, and [`Backup::status`] reports
/// its condition code 12. The number of records is recorded in `written` on the current span.
///
/// The pages are read in one read-only unit of work on a single snapshot (`Db::begin_snapshot`),
/// so together they are the master as one moment left it, as one read of the whole file was,
/// while online writers go on. The unit of work writes nothing and is rolled back before the
/// catalog. TRANREPT backs up this way; TRANBKP, which empties the master after the backup, runs
/// [`backup_live_master_in`] inside its own unit of work instead.
///
/// The allocation, every append and the catalog run on Tokio's blocking pool, inside the current
/// span. The generation is allocated, and its lock taken, before the first page is read.
///
/// # Errors
///
/// A failure to allocate, read, write or catalog is a setup failure (exit 16): the audit records
/// no condition code for it. The generation is then discarded on the blocking pool, not
/// cataloged, and the previous one stays `(0)`.
pub(super) async fn backup_live_master(ctx: &JobContext, db: &Db) -> Result<Backup, ExitStatus> {
    let mut uow = db
        .begin_snapshot()
        .await
        .map_err(|err| setup_failure("live transaction master read failed", err))?;
    let (pending, records) = write_backup(ctx, &mut uow).await?;
    if let Err(err) = uow.rollback().await {
        let status = setup_failure("live transaction master read failed", err);
        pending.discard_async().await;
        return Err(status);
    }
    catalog_backup(pending, records).await
}

/// The REPROC step of [`backup_live_master`] inside the caller's unit of work `uow`: TRANBKP
/// `STEP05R` (P-TRANBKP-001, P-TRANBKP-002). TRANBKP holds the live master still in `uow` first
/// (`PgStore::exclude_master_writers`) and empties it in the same unit after this returns, so
/// every record it takes out of the master is in the backup this catalogs.
///
/// Writes and catalogs `TRANSACT.BKUP(+1)` as [`backup_live_master`] does, and leaves `uow` open
/// for the caller to commit or roll back.
///
/// # Errors
///
/// As [`backup_live_master`]: a failure to allocate, read, write or catalog is a setup failure
/// (exit 16), the generation is then not cataloged and the previous one stays `(0)`.
pub(super) async fn backup_live_master_in(
    ctx: &JobContext,
    uow: &mut UnitOfWork,
) -> Result<Backup, ExitStatus> {
    let (pending, records) = write_backup(ctx, uow).await?;
    catalog_backup(pending, records).await
}

/// REPROC `PRC001` `REPRO INFILE(FILEIN) OUTFILE(FILEOUT)` [app/proc/REPROC.prc:L21-28] —
/// P-TRANBKP-001, P-TRANREPT-001: the copy itself, in the caller's unit of work `uow`. Allocates
/// `TRANSACT.BKUP(+1)` and appends every live record to it in TRAN-ID order, one page of
/// [`BACKUP_PAGE_SIZE`] records at a time, so the job holds one page however large the master
/// grows. A page shorter than [`BACKUP_PAGE_SIZE`] is the end of the master. Returns the
/// generation, still uncataloged, and the number of records written to it. The allocation and
/// every append run on Tokio's blocking pool, inside the current span.
///
/// # Errors
///
/// A setup failure (exit 16) when the generation cannot be allocated or written or a page cannot
/// be read. The pending generation is then discarded on the blocking pool, which deletes it.
async fn write_backup(
    ctx: &JobContext,
    uow: &mut UnitOfWork,
) -> Result<(PendingGeneration, usize), ExitStatus> {
    let mut pending = ctx
        .generations
        .begin_next_async(Dataset::TransactBkup)
        .await
        .map_err(|err| setup_failure("TRANSACT.BKUP(+1) allocation failed", err))?;
    match copy_live_master(uow, &mut pending).await {
        Ok(records) => Ok((pending, records)),
        Err(status) => {
            pending.discard_async().await;
            Err(status)
        }
    }
}

/// The page loop of [`write_backup`]: reads the live master in TRAN-ID order, one page of
/// [`BACKUP_PAGE_SIZE`] records at a time, appends each page to `pending` on the blocking pool,
/// and returns the number of records appended.
///
/// # Errors
///
/// A setup failure (exit 16) when a page cannot be read or appended; `pending` is left to the
/// caller to discard.
async fn copy_live_master(
    uow: &mut UnitOfWork,
    pending: &mut PendingGeneration,
) -> Result<usize, ExitStatus> {
    let mut records = 0;
    let mut after = None;
    loop {
        let page = uow
            .store()
            .live_master_after(after, BACKUP_PAGE_SIZE)
            .await
            .map_err(|err| setup_failure("live transaction master read failed", err))?;
        let page_records: Vec<TransactionRecord> = page
            .iter()
            .map(|row| TransactionRecord::new(row.to_new()))
            .collect();
        let appended = page_records.len();
        pending
            .append_records_async::<TransactionLayout>(page_records)
            .await
            .map_err(|err| setup_failure("TRANSACT.BKUP(+1) write failed", err))?;
        records += appended;
        match page.last() {
            Some(last) if page.len() == usize::from(BACKUP_PAGE_SIZE) => {
                after = Some(last.tran_id);
            }
            _ => return Ok(records),
        }
    }
}

/// REPROC `FILEOUT` `DISP=(NEW,CATLG,DELETE)` [app/jcl/TRANBKP.jcl:L23-33] — P-TRANBKP-001,
/// P-TRANBKP-002, P-TRANREPT-001: catalogs `pending`, the backup [`write_backup`] filled with
/// `records` records, at the normal end of the step, on Tokio's blocking pool inside the current
/// span, records `written` on the current span and reports the step's condition code.
///
/// # Errors
///
/// A setup failure (exit 16) when the catalog fails: the generation is then not cataloged and the
/// previous one stays `(0)`.
async fn catalog_backup(pending: PendingGeneration, records: usize) -> Result<Backup, ExitStatus> {
    let generation = pending
        .catalog_async()
        .await
        .map_err(|err| setup_failure("TRANSACT.BKUP(+1) catalog failed", err))?;
    let backup = Backup {
        generation,
        records,
    };
    Span::current().record("written", backup.records);
    let condition_code = backup.status().code();
    if backup.records == 0 {
        tracing::warn!(
            generation = %backup.generation,
            records = backup.records,
            condition_code,
            "IDC3351I VSAM OPEN RETURN CODE IS 160"
        );
    } else {
        tracing::info!(
            generation = %backup.generation,
            records = backup.records,
            condition_code,
            "live transaction master backed up"
        );
    }
    Ok(backup)
}

/// Logs `err` under `what` and turns it into the setup failure that ends the job.
fn setup_failure(what: &'static str, err: impl fmt::Display) -> ExitStatus {
    tracing::error!(error = %err, "{what}");
    ExitStatus::setup(err)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use carddemo_domain::clock::{AnyClock, SystemClock};
    use carddemo_runtime::config::Settings;
    use clap::{CommandFactory, Parser};

    use super::{Backup, JobContext, job_name};
    use crate::cli::Cli;
    use crate::exit::ExitStatus;
    use crate::generations::{Dataset, Generation};

    #[test]
    fn job_names_are_the_subcommand_names() {
        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|sub| sub.get_name().to_owned())
            .collect();
        assert_eq!(names.len(), 10, "{names:?}");
        for name in &names {
            let mut args = vec!["batch", name.as_str()];
            if name == "mnttrdb2" {
                args.extend(["--input", "trantype.upd"]);
            }
            let cli = Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
            assert_eq!(job_name(&cli.command), name);
        }
    }

    #[test]
    fn backup_status_is_the_repro_condition_code() {
        let cases = [
            (0, ExitStatus::ReproCondition(12)),
            (1, ExitStatus::Success),
        ];
        for (records, expected) in cases {
            let backup = Backup {
                generation: Generation {
                    dataset: Dataset::TransactBkup,
                    number: 1,
                    path: PathBuf::from("TRANSACT.BKUP.G0001V00"),
                },
                records,
            };
            assert_eq!(backup.status(), expected, "{records} record(s)");
        }
    }

    #[test]
    fn context_catalogs_generations_under_the_batch_dir() {
        let dir = std::env::temp_dir().join(format!("carddemo-jobs-ctx-{}", std::process::id()));
        let settings = Settings {
            batch_dir: dir.clone(),
            ..Settings::default()
        };
        let ctx = JobContext::new(settings, AnyClock::System(SystemClock));
        let generation = ctx
            .generations
            .begin_next(Dataset::TransactBkup)
            .and_then(crate::generations::PendingGeneration::catalog)
            .unwrap_or_else(|e| panic!("catalog under {}: {e}", dir.display()));
        let expected = dir.join("TRANSACT.BKUP.G0001V00");
        let cataloged = fs::metadata(&expected).map(|meta| meta.len());
        fs::remove_dir_all(&dir).unwrap_or_else(|e| panic!("remove {}: {e}", dir.display()));
        assert_eq!(generation.path, expected);
        assert_eq!(cataloged.ok(), Some(0));
    }
}
