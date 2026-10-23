//! `carddemo-batch`: the batch jobs of the `CardDemo` replacement as one command-line program,
//! one subcommand per in-scope JCL job of the README batch inventory (AAP 0.2.3, 0.3.4.3).
//!
//! | Subcommands | JCL jobs | Program |
//! |---|---|---|
//! | `acctfile`, `cardfile`, `xreffile`, `custfile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype` | `ACCTFILE` to `TRANTYPE` | `IDCAMS` `REPRO` |
//! | `dusrsecj` | `DUSRSECJ` | `IEBGENER` |
//! | `posttran` | `POSTTRAN` | `CBTRN02C` |
//! | `intcalc` | `INTCALC` | `CBACT04C` |
//! | `creastmt` | `CREASTMT` | `CBSTM03A` with `CBSTM03B` |
//! | `tranrept` | `TRANREPT` | `CBTRN03C` |
//! | `cbpaup0j` | `CBPAUP0J` | `CBPAUP0C` |
//! | `mnttrdb2` | `MNTTRDB2` | `COBTUPDT` |
//!
//! This file holds only the process lifecycle: it parses the command line, checks the
//! `cbpaup0j` SYSIN record, reads the configuration, checks the job's output directory, installs
//! telemetry, builds the async runtime, opens the `PostgreSQL` pool and runs the selected job.
//! Business rules live in `domain::batch`, every query in the `persistence` repository traits and
//! every file read and write in the job modules.
//!
//! The file uses no `tokio` or `tracing` macro: the runtime is built explicitly in `run`, and
//! events go through `emit`, whose documentation says why.
//!
//! # Schema
//!
//! The binary never applies the schema (Single Schema Executor). Run the `carddemo-migrate`
//! binary of the `runtime` crate first; against a database whose schema has not been applied, a
//! job fails at its first query and exits with a non-zero status, as listed below.
//!
//! # Standard output
//!
//! - One line per file the job wrote, in write order, each path exactly as the job built it from
//!   the configured directories: relative paths stay relative to `modern/`, the working directory
//!   of every binary, and nothing is canonicalised. A relative path whose first character is `{`
//!   is printed with a leading `./`, which names the same file, so that no path line can begin
//!   like a JSON event. The nine loads, `cbpaup0j` and `mnttrdb2` write no file and print no line.
//! - The JSON tracing events of `runtime::telemetry` share standard output (AAP 0.3.4.7), one
//!   object per line, each beginning with `{`. The job-log (`DISPLAY`) lines of the COBOL
//!   programs are among those events, emitted by the jobs themselves, never as plain lines. A
//!   consumer such as `modern/crates/batch/tests` or `scripts/e2e.sh` therefore takes the path
//!   lines as the lines that do not begin with `{`, with span export off (the default; the
//!   `CARDDEMO_OTEL_EXPORT=stdout` exporter writes multi-line text blocks), or runs with
//!   `RUST_LOG=off`, which leaves only the path lines.
//! - The outcome of a job that ran to its end is the event `job finished`, with the fields
//!   `job`, `return_code` and `written` (the number of path lines): `INFO` for return code 0
//!   and `WARN` for any other, so a run whose records were rejected or failed raises a warning.
//!   A failed job's event is `job failed`, at `ERROR`. Before either event, the job span
//!   `batch.job` records `return_code`, when the run has one, and the job's counters, such as
//!   `processed` and `rejected` for `posttran`, as JSON numbers: the event line shows them in
//!   its `span` member, and the exported span carries them as attributes, however many of the
//!   span's events the exporter drops.
//!
//! # Standard error
//!
//! A failed run writes one line, `carddemo-batch <subcommand>: <reason>`. The reason is a
//! refused `cbpaup0j` SYSIN record, a configuration error that names its variable and never its
//! value, a telemetry failure, an async runtime that cannot be built (`cannot build the async
//! runtime: <error>`, also an `ERROR` event of that message in the job span), a database
//! connection failure that names `DATABASE_URL` and gives the value-free summary of
//! `runtime::db`, the job's error, or a failure to write standard output. None of them shows a
//! record, a card number, an SSN, a password or the connection URL (Cardholder Data Redaction).
//! A job's error on its output directory, such as `allocating generation (reports under
//! CARDDEMO_VAR_DIR): not a directory`, names the setting of that directory instead of its path.
//! `posttran`, `intcalc` and `tranrept` end such a failure with their audited abend,
//! `ERROR OPENING DALY REJECTS FILE`, `ERROR OPENING TRANSACTION FILE` or
//! `ERROR OPENING REPTFILE`, and name the setting in the event that precedes the abend's lines.
//!
//! Input checks that need no database run before the pool is opened, so a refusal comes at once
//! and without a connection attempt:
//!
//! - Right after the command line is parsed, before the environment is read: the `--sysin`
//!   record of `cbpaup0j`, through the domain's total parser (CBPAUP0C-002). A refused record
//!   gives the job's static refusal text followed by what `--sysin` takes, never the record, its
//!   length or an offset into it.
//! - Right after the configuration is read, before telemetry and before any directory or file
//!   is created: the directory the job writes into, `dalyrejs`, `systran` or `reports` under
//!   `CARDDEMO_VAR_DIR` for `posttran`, `intcalc` and `tranrept`, and the statement directory
//!   for `creastmt`. A path holding a carriage return or a line feed would split its line on
//!   standard output, so it is a configuration error that names the variables, never their
//!   value. Non-ASCII text, spaces and punctuation are accepted. The jobs that write no file
//!   are not checked.
//!
//! # Exit status
//!
//! - 0: the job ran to its end with return code 0.
//! - The program's return code when the job ran to its end: 4 for `posttran` when any record was
//!   rejected (CBTRN02C-035, `CBTRN02C.cbl:229-234`), 4 for `mnttrdb2` when any record reached
//!   `9999-ABEND` (COBTUPDT-023, `COBTUPDT.cbl:232`).
//! - The return code a job's error carries for an audited abend path, such as 16 when `cbpaup0j`
//!   ends through `9999-ABEND` (CBPAUP0C-028, `CBPAUP0C.cbl:381-382`) or 12 for the user abend
//!   `U0999` of `posttran` (escalation `X-POSTTRAN-2`).
//! - 1 for every other failure: a refused SYSIN record, configuration (an output directory
//!   holding a line break included), telemetry, the async runtime build, database connection,
//!   standard output, or a job error that carries no return code.
//! - 2 for a usage error, reported by `clap` before anything else runs, so `--help` and
//!   `--version` need neither the environment nor a database.
//!
//! The job span records a return code, the job's own or its error's, as `return_code`, and a
//! non-zero one makes the `job finished` event of a job that ran to its end a `WARN` (see
//! Standard output).

mod cli;
mod jobs;
mod pdf;

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use domain::batch::cbpaup0c::{DEFAULT_SYSIN, PurgeParams, SYSIN_RECORD_LEN};
use domain::batch::loads::LoadJob;
use domain::clock::SystemClock;
use persistence::PgStore;
use runtime::config::Config;
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{self, FieldSet, Value};
use tracing::metadata::Kind;
use tracing::{Event, Instrument, Level, Metadata, Span};

use crate::cli::{
    Cli, DusrsecjArgs, IntcalcArgs, Job, LoadArgs, Mnttrdb2Args, PosttranArgs, TranreptArgs,
};
use crate::jobs::{
    DALYREJS_DIR_SETTING, JobContext, JobError, JobOutcome, REPORTS_DIR_SETTING,
    STATEMENT_DIR_SETTING, SYSTRAN_DIR_SETTING,
};

/// The prefix of every standard-error line, followed by the subcommand.
const PROGRAM: &str = "carddemo-batch";

fn main() -> ExitCode {
    // First, so that `--help`, `--version` and usage errors need no environment.
    let cli = Cli::parse();
    let job = cli.job.name();
    // Before the environment and the pool: a refused value needs neither.
    let checked = match check(&cli.job) {
        Ok(checked) => checked,
        Err(refused) => {
            report(job, &refused);
            return refused.exit_code();
        }
    };
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => return fail(job, &format_args!("configuration error: {error}")),
    };
    // Before any directory or file is created, and before the pool.
    if let Err(refused) = check_output_dir(&checked, &config) {
        return fail(job, &format_args!("configuration error: {refused}"));
    }
    // Bound until the exit code is computed: dropping the guard flushes the exported spans.
    let guard = match runtime::telemetry::init(config.otel_export) {
        Ok(guard) => guard,
        Err(error) => {
            // No subscriber exists, so standard error is the only channel left.
            return fail(
                job,
                &format_args!("telemetry initialisation failed: {error}"),
            );
        }
    };
    let code = run(job, checked, &config);
    // Dropped once the runtime has shut down and every event is emitted, so the span exporter
    // flushes everything the run recorded. Its exporter is synchronous and needs no runtime.
    drop(guard);
    code
}

/// Builds the multi-threaded async runtime, runs `selected` on it inside the job span, and
/// shuts the runtime down before returning the job's exit code.
///
/// The runtime is built here, by hand, because `#[tokio::main]` expands to a block carrying a
/// lint suppression and an `expect` on the build, and this workspace admits no lint
/// suppression, generated ones included. A runtime that cannot be built is reported by
/// [`runtime_failed`] instead.
fn run(job: &'static str, selected: CheckedJob<'_>, config: &Config) -> ExitCode {
    let async_runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(async_runtime) => async_runtime,
        // In the job span, as every other failure of the job is reported.
        Err(error) => {
            return runtime::telemetry::job_span(job).in_scope(|| runtime_failed(job, &error));
        }
    };
    let code = async_runtime
        .block_on(execute(job, selected, config).instrument(runtime::telemetry::job_span(job)));
    // Shutdown waits for the runtime's threads, as the end of `#[tokio::main]` does.
    drop(async_runtime);
    code
}

/// The job the subcommand selected, with every option that `clap` keeps as typed already parsed
/// by the job's own parser: what [`dispatch`] runs.
enum CheckedJob<'a> {
    /// One of the eight `IDCAMS` fixture loads.
    Load(LoadJob, &'a LoadArgs),
    /// `dusrsecj`.
    Dusrsecj(&'a DusrsecjArgs),
    /// `posttran`.
    Posttran(&'a PosttranArgs),
    /// `intcalc`.
    Intcalc(&'a IntcalcArgs),
    /// `creastmt`.
    Creastmt,
    /// `tranrept`.
    Tranrept(&'a TranreptArgs),
    /// `cbpaup0j`, with the run parameters of its accepted `--sysin` record (CBPAUP0C-002).
    Cbpaup0j(PurgeParams),
    /// `mnttrdb2`.
    Mnttrdb2(&'a Mnttrdb2Args),
}

/// Checks the command line of `job` before the environment is read or the pool opened: today
/// only the `--sysin` record of `cbpaup0j`, which `clap` keeps as typed so that its usage error
/// never repeats it (`cli`). The match names every variant and has no wildcard arm, so a new
/// subcommand needs a decision here.
///
/// # Errors
///
/// [`SysinRefused`] when the domain refuses the `--sysin` record (CBPAUP0C-002).
fn check(job: &Job) -> Result<CheckedJob<'_>, SysinRefused> {
    Ok(match job {
        Job::Acctfile(args) => CheckedJob::Load(LoadJob::Acctfile, args),
        Job::Cardfile(args) => CheckedJob::Load(LoadJob::Cardfile, args),
        Job::Xreffile(args) => CheckedJob::Load(LoadJob::Xreffile, args),
        Job::Custfile(args) => CheckedJob::Load(LoadJob::Custfile, args),
        Job::Discgrp(args) => CheckedJob::Load(LoadJob::Discgrp, args),
        Job::Tcatbalf(args) => CheckedJob::Load(LoadJob::Tcatbalf, args),
        Job::Trancatg(args) => CheckedJob::Load(LoadJob::Trancatg, args),
        Job::Trantype(args) => CheckedJob::Load(LoadJob::Trantype, args),
        Job::Dusrsecj(args) => CheckedJob::Dusrsecj(args),
        Job::Posttran(args) => CheckedJob::Posttran(args),
        Job::Intcalc(args) => CheckedJob::Intcalc(args),
        Job::Creastmt => CheckedJob::Creastmt,
        Job::Tranrept(args) => CheckedJob::Tranrept(args),
        Job::Cbpaup0j(args) => {
            CheckedJob::Cbpaup0j(jobs::cbpaup0j::params(args).map_err(SysinRefused)?)
        }
        Job::Mnttrdb2(args) => CheckedJob::Mnttrdb2(args),
    })
}

/// A `--sysin` record the domain refuses (CBPAUP0C-002), as the job reports it.
struct SysinRefused(JobError);

impl SysinRefused {
    /// The exit status of the refusal: the return code its error carries, else the generic
    /// failure status.
    fn exit_code(&self) -> ExitCode {
        self.0.exit_code().map_or(ExitCode::FAILURE, ExitCode::from)
    }
}

impl fmt::Display for SysinRefused {
    /// The job's static refusal text, then what `--sysin` takes. Neither part shows the record,
    /// its length or an offset into it, as the record may hold a protected value (Cardholder
    /// Data Redaction).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}; --sysin takes one record of at most {SYSIN_RECORD_LEN} printable ASCII \
             characters (default {DEFAULT_SYSIN}); see {PROGRAM} cbpaup0j --help",
            self.0
        )
    }
}

/// The refusal of a directory under `CARDDEMO_VAR_DIR` whose path holds a line break.
const VAR_DIR_LINE_BREAK: &str = "CARDDEMO_VAR_DIR holds a carriage return or a line feed, which \
                                  would split the line of a written path on standard output";

/// The refusal of a statement directory whose path holds a line break. The directory is
/// `CARDDEMO_STATEMENT_DIR` when it is set and `statements` under `CARDDEMO_VAR_DIR` otherwise
/// (`runtime::config`), so the text names both.
const STATEMENT_DIR_LINE_BREAK: &str = "CARDDEMO_STATEMENT_DIR, or CARDDEMO_VAR_DIR when \
                                        CARDDEMO_STATEMENT_DIR is unset, holds a carriage return \
                                        or a line feed, which would split the line of a written \
                                        path on standard output";

/// Refuses a run whose job writes into a directory whose path holds a carriage return or a line
/// feed. Each written path is printed as one line of standard output (AAP 0.3.4.3), and a line
/// break inside it would split that path over two lines. The file names the jobs add are fixed
/// ASCII, so the directory is the only part that can hold one; every other character, non-ASCII
/// text, spaces and punctuation included, is accepted. The match names every variant and has no
/// wildcard arm, so a new subcommand needs a decision here.
///
/// # Errors
///
/// The static text that names the variables setting the directory and why it is refused, never
/// their value.
fn check_output_dir(job: &CheckedJob<'_>, config: &Config) -> Result<(), &'static str> {
    let (line_break, refusal) = match job {
        CheckedJob::Posttran(_) => (has_line_break(&config.dalyrejs_dir()), VAR_DIR_LINE_BREAK),
        CheckedJob::Intcalc(_) => (has_line_break(&config.systran_dir()), VAR_DIR_LINE_BREAK),
        CheckedJob::Tranrept(_) => (has_line_break(&config.reports_dir()), VAR_DIR_LINE_BREAK),
        CheckedJob::Creastmt => (
            has_line_break(&config.statement_dir),
            STATEMENT_DIR_LINE_BREAK,
        ),
        // These jobs write no file, so they print no path.
        CheckedJob::Load(..)
        | CheckedJob::Dusrsecj(_)
        | CheckedJob::Cbpaup0j(_)
        | CheckedJob::Mnttrdb2(_) => return Ok(()),
    };
    if line_break { Err(refusal) } else { Ok(()) }
}

/// Whether the encoded bytes of `path` hold a carriage return or a line feed.
fn has_line_break(path: &Path) -> bool {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|&byte| byte == b'\r' || byte == b'\n')
}

/// Opens the pool, runs `selected` and reports its result, inside the job span. A pool that
/// cannot be opened is reported as a connection failure, which `intcalc` precedes with its
/// first line and follows with its first open's failure (CBACT04C-002, CBACT04C-034).
async fn execute(job: &'static str, selected: CheckedJob<'_>, config: &Config) -> ExitCode {
    let pool = match runtime::db::connect(config).await {
        Ok(pool) => pool,
        Err(error) => {
            // The summary keeps an error class and a SQLSTATE at most: the driver's own text can
            // quote the connection URL.
            let error = runtime::db::ErrorSummary::from(&error);
            // CBACT04C-002, CBACT04C-034: the connection stands for INTCALC's input opens, which
            // follow its first line, so its failure is the first open's: the first line, this
            // event carrying the cause, then that open's abend lines. Every other job shows none.
            let intcalc = matches!(selected, CheckedJob::Intcalc(_));
            if intcalc {
                jobs::intcalc::show_start();
            }
            connection_failed(job, &error);
            if intcalc {
                jobs::intcalc::show_failed_connection();
            }
            return fail(
                job,
                &format_args!("cannot connect to the database (DATABASE_URL): {error}"),
            );
        }
    };
    let store = PgStore::new(pool);
    let clock = SystemClock;
    let ctx = JobContext {
        store: &store,
        config,
        clock: &clock,
    };
    let setting = output_setting(&selected);
    let result = dispatch(selected, &ctx)
        .await
        .map_err(|error| match setting {
            Some(setting) => error.naming_output_dir(setting),
            None => error,
        });
    // Every job has committed or rolled back its work by now; closing ends the sessions cleanly
    // instead of dropping their connections when the runtime shuts down.
    store.pool().close().await;
    finish(job, result)
}

/// The setting that names the directory a job writes into, in the words its output-directory
/// errors use (`JobError::naming_output_dir`): the variable and, under `CARDDEMO_VAR_DIR`, the
/// subdirectory, as `runtime::config` builds the directory. Never its value. `None` for the jobs
/// that write no file. The match names every variant and has no wildcard arm, so a new
/// subcommand needs a decision here.
fn output_setting(job: &CheckedJob<'_>) -> Option<&'static str> {
    match job {
        CheckedJob::Posttran(_) => Some(DALYREJS_DIR_SETTING),
        CheckedJob::Intcalc(_) => Some(SYSTRAN_DIR_SETTING),
        CheckedJob::Tranrept(_) => Some(REPORTS_DIR_SETTING),
        CheckedJob::Creastmt => Some(STATEMENT_DIR_SETTING),
        CheckedJob::Load(..)
        | CheckedJob::Dusrsecj(_)
        | CheckedJob::Cbpaup0j(_)
        | CheckedJob::Mnttrdb2(_) => None,
    }
}

/// Runs the job the subcommand selected. The match names every variant and has no wildcard
/// arm, so a subcommand without a job fails to compile here.
async fn dispatch(job: CheckedJob<'_>, ctx: &JobContext<'_>) -> Result<JobOutcome, JobError> {
    match job {
        CheckedJob::Load(load, args) => run_load(ctx, load, args).await,
        CheckedJob::Dusrsecj(args) => jobs::dusrsecj::run(ctx, &args.input).await,
        CheckedJob::Posttran(args) => jobs::posttran::run(ctx, &args.dalytran).await,
        CheckedJob::Intcalc(args) => jobs::intcalc::run(ctx, args).await,
        CheckedJob::Creastmt => jobs::creastmt::run(ctx).await,
        CheckedJob::Tranrept(args) => jobs::tranrept::run(ctx, args).await,
        CheckedJob::Cbpaup0j(params) => jobs::cbpaup0j::run(ctx, params).await,
        CheckedJob::Mnttrdb2(args) => jobs::mnttrdb2::run(ctx, &args.input).await,
    }
}

/// Runs fixture load `job` over `--input`, or over the job's fixture file in the configured
/// fixture directory.
async fn run_load(
    ctx: &JobContext<'_>,
    job: LoadJob,
    args: &LoadArgs,
) -> Result<JobOutcome, JobError> {
    let input = args.input_path(&ctx.config.fixture_dir, job);
    jobs::loads::run(ctx, job, &input).await
}

/// Reports the job's result: the written paths on standard output and the outcome on the job
/// span and as a tracing event, or the error on standard error and as a tracing event, and
/// returns the exit code.
///
/// The return code, and for a job that ran to its end its counters, are recorded on the current
/// span, the job span, before the event, as numbers. The outcome event `job finished` is `INFO`
/// for return code 0 and `WARN` for any other, so a run whose records were rejected or failed
/// raises a warning.
fn finish(job: &'static str, result: Result<JobOutcome, JobError>) -> ExitCode {
    let span = Span::current();
    match result {
        Ok(outcome) => {
            if print_paths(&outcome.written).is_err() {
                return stdout_failed(job);
            }
            runtime::telemetry::record_job_outcome(&span, outcome.return_code, &outcome.counts);
            job_finished(job, outcome.return_code, outcome.written.len());
            ExitCode::from(outcome.return_code)
        }
        Err(error) => {
            let code = error.exit_code();
            if let Some(return_code) = code {
                runtime::telemetry::record_job_outcome(&span, return_code, &[]);
            }
            job_failed(job, &error);
            report(job, &error);
            code.map_or(ExitCode::FAILURE, ExitCode::from)
        }
    }
}

/// The callsite of one event of this binary, whose metadata names `N` fields, `message` first.
/// It is written out in place of a `tracing` event macro, whose expansion carries a lint
/// suppression (see [`emit`]).
///
/// [`event_metadata`] and [`emit`] take the same `N`, so an event always carries exactly one
/// value, or `None`, per field it declares.
struct EventSite<const N: usize>(DefaultCallsite);

impl<const N: usize> EventSite<N> {
    /// The callsite of the event that `metadata` describes.
    const fn new(metadata: &'static Metadata<'static>) -> Self {
        Self(DefaultCallsite::new(metadata))
    }
}

/// The metadata of the event `name` at `level`, with the field `names`, for the callsite `site`.
///
/// The target and the module path are this crate's root module, `carddemo_batch`, which a
/// `tracing` macro in this file would have given the event, so `RUST_LOG` directives select it
/// as before. No source file or line is given: the JSON output and the span export print
/// neither.
const fn event_metadata<const N: usize>(
    name: &'static str,
    level: Level,
    names: &'static [&'static str; N],
    site: &'static EventSite<N>,
) -> Metadata<'static> {
    Metadata::new(
        name,
        module_path!(),
        level,
        None,
        None,
        Some(module_path!()),
        FieldSet::new(names, Identifier(&site.0)),
        Kind::EVENT,
    )
}

/// Emits one event of `site` in the current span, with `values`, one per field the site names
/// and in that order, a `None` leaving its field out, when the current subscriber enables it.
///
/// Every `tracing` event macro expands to an attribute that allows `unused_imports`, and this
/// workspace admits no lint suppression, generated ones included. So each event of this file has
/// its callsite written out, an [`EventSite`] over its [`event_metadata`], and this function
/// takes the steps an event macro takes: it decides through `runtime::telemetry::event_enabled`
/// and dispatches with `Event::dispatch`. `RUST_LOG` filtering, the JSON line and the span
/// export therefore treat these events exactly as they treat a macro's.
fn emit<const N: usize>(site: &'static EventSite<N>, values: [Option<&dyn Value>; N]) {
    let callsite = &site.0;
    if !runtime::telemetry::event_enabled(callsite) {
        return;
    }
    let metadata = callsite.metadata();
    // The site's field names and `values` have the same length `N`, so each field gets its value.
    Event::dispatch(metadata, &metadata.fields().value_set_all(&values));
}

/// The message of the outcome event of a job that ran to its end, and the name of its callsites.
const JOB_FINISHED: &str = "job finished";

/// The fields of the [`JOB_FINISHED`] event, in the order [`job_finished`] gives their values.
const JOB_FINISHED_FIELDS: [&str; 4] = ["message", "job", "return_code", "written"];

/// The `INFO` callsite of the [`JOB_FINISHED`] event, for return code 0.
static JOB_FINISHED_INFO: EventSite<4> = EventSite::new(&JOB_FINISHED_INFO_METADATA);

/// The metadata of [`JOB_FINISHED_INFO`].
static JOB_FINISHED_INFO_METADATA: Metadata<'static> = event_metadata(
    JOB_FINISHED,
    Level::INFO,
    &JOB_FINISHED_FIELDS,
    &JOB_FINISHED_INFO,
);

/// The `WARN` callsite of the [`JOB_FINISHED`] event, for any other return code.
static JOB_FINISHED_WARN: EventSite<4> = EventSite::new(&JOB_FINISHED_WARN_METADATA);

/// The metadata of [`JOB_FINISHED_WARN`].
static JOB_FINISHED_WARN_METADATA: Metadata<'static> = event_metadata(
    JOB_FINISHED,
    Level::WARN,
    &JOB_FINISHED_FIELDS,
    &JOB_FINISHED_WARN,
);

/// Emits the [`JOB_FINISHED`] event of a job that ran to its end, in the current span, with the
/// fields `job`, `return_code` and `written`: `INFO` for return code 0 and `WARN` for any other.
///
/// It goes through [`emit`], so the event is the line a macro would write: target
/// `carddemo_batch` and fields `{"message":"job finished","job":…,"return_code":…,"written":…}`.
fn job_finished(job: &str, return_code: u8, written: usize) {
    // An event's level is fixed at its callsite, so each level has its own callsite.
    let site = if return_code == 0 {
        &JOB_FINISHED_INFO
    } else {
        &JOB_FINISHED_WARN
    };
    emit(
        site,
        [
            Some(&JOB_FINISHED),
            Some(&job),
            Some(&return_code),
            Some(&written),
        ],
    );
}

/// The message of the event of a pool that cannot be opened, and the name of its callsite.
const CONNECTION_FAILED: &str = "database connection failed (DATABASE_URL)";

/// Emits the `ERROR` event [`CONNECTION_FAILED`] in the current span, with the fields `job` and
/// `error`, the value-free summary of the connection error as text.
fn connection_failed(job: &str, error: &runtime::db::ErrorSummary) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        CONNECTION_FAILED,
        Level::ERROR,
        &["message", "job", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&CONNECTION_FAILED),
            Some(&job),
            Some(&field::display(error)),
        ],
    );
}

/// The message of the event of written paths that cannot be printed, the reason of its
/// standard-error line, and the name of its callsite.
const STDOUT_FAILED: &str = "cannot write to standard output";

/// Reports written paths that cannot be printed: the `ERROR` event [`STDOUT_FAILED`] in the
/// current span, with the field `job`, then the standard-error line; returns the generic failure
/// status.
fn stdout_failed(job: &str) -> ExitCode {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata(STDOUT_FAILED, Level::ERROR, &["message", "job"], &SITE);
    emit(&SITE, [Some(&STDOUT_FAILED), Some(&job)]);
    fail(job, &STDOUT_FAILED)
}

/// The message of the event of a failed job, and the name of its callsite.
const JOB_FAILED: &str = "job failed";

/// Emits the `ERROR` event [`JOB_FAILED`] in the current span, with the fields `job` and `error`,
/// the job's error as text.
fn job_failed(job: &str, error: &JobError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        JOB_FAILED,
        Level::ERROR,
        &["message", "job", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [Some(&JOB_FAILED), Some(&job), Some(&field::display(error))],
    );
}

/// The message of the event of an async runtime that cannot be built, the start of the reason of
/// its standard-error line, and the name of its callsite.
const RUNTIME_FAILED: &str = "cannot build the async runtime";

/// Reports an async runtime that cannot be built: the `ERROR` event [`RUNTIME_FAILED`] in the
/// current span, with the fields `job` and `error`, the build's I/O error as text, then the
/// standard-error line `carddemo-batch <job>: cannot build the async runtime: <error>`; returns
/// the generic failure status.
fn runtime_failed(job: &str, error: &io::Error) -> ExitCode {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        RUNTIME_FAILED,
        Level::ERROR,
        &["message", "job", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&RUNTIME_FAILED),
            Some(&job),
            Some(&field::display(error)),
        ],
    );
    fail(job, &format_args!("{RUNTIME_FAILED}: {error}"))
}

/// Writes each path on its own line of standard output, then flushes it.
fn print_paths(paths: &[PathBuf]) -> io::Result<()> {
    write_paths(&mut io::stdout().lock(), paths)
}

/// Writes each path on its own line of `out`, in order, then flushes `out`. A path whose first
/// character is `{` gets a leading `./`, so that no path line can be read as a JSON event.
fn write_paths(out: &mut impl Write, paths: &[PathBuf]) -> io::Result<()> {
    for path in paths {
        if path.as_os_str().as_encoded_bytes().first() == Some(&b'{') {
            out.write_all(b"./")?;
        }
        writeln!(out, "{}", path.display())?;
    }
    out.flush()
}

/// [`report`]s `message`, then returns the generic failure status.
fn fail(job: &str, message: &dyn fmt::Display) -> ExitCode {
    report(job, message);
    ExitCode::FAILURE
}

/// Writes the one standard-error line of a failed run, `carddemo-batch <job>: <message>`.
fn report(job: &str, message: &dyn fmt::Display) {
    // Ignored: when standard error itself cannot be written, the exit status is the only report
    // left, and the caller returns it either way.
    let _ = writeln!(io::stderr().lock(), "{PROGRAM} {job}: {message}");
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::process::ExitCode;

    use clap::Parser;
    use domain::batch::cbpaup0c::{DEFAULT_SYSIN, PurgeParams};
    use runtime::config::Config;
    use runtime::telemetry::{JobCounter, job_span};
    use test_support::trace_capture;

    use super::{
        CheckedJob, PROGRAM, STATEMENT_DIR_LINE_BREAK, VAR_DIR_LINE_BREAK, check, check_output_dir,
        finish, output_setting, runtime_failed, stdout_failed, write_paths,
    };
    use crate::cli::{Cli, Job};
    use crate::jobs::{JobError, JobOutcome};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The job `clap` parses from the subcommand line `args`.
    fn parse(args: &[&str]) -> Result<Job, clap::Error> {
        Cli::try_parse_from(std::iter::once(PROGRAM).chain(args.iter().copied())).map(|cli| cli.job)
    }

    /// A refused `--sysin` record is reported by the check that runs before the environment is
    /// read: the job's static text, then what `--sysin` takes, never the record, its length or
    /// its offset, and exit status 1 (CBPAUP0C-002).
    // audit: CBPAUP0C-002
    #[test]
    fn a_refused_sysin_is_reported_with_guidance_and_without_the_record() -> TestResult {
        let long = "A".repeat(81);
        let guidance = "; --sysin takes one record of at most 80 printable ASCII characters \
                        (default 00,00001,00001,Y); see carddemo-batch cbpaup0j --help";
        let cases = [
            (
                long.as_str(),
                "ABEND: SYSIN record refused: longer than the 80-byte card",
                "81",
            ),
            (
                "00,\t",
                "ABEND: SYSIN record refused: a byte outside printable ASCII",
                "byte 3",
            ),
        ];
        for (sysin, reason, hidden) in cases {
            let job = parse(&["cbpaup0j", "--sysin", sysin])?;
            let Err(refused) = check(&job) else {
                return Err(format!("accepted {sysin:?}").into());
            };
            let shown = refused.to_string();
            assert_eq!(shown, format!("{reason}{guidance}"));
            assert!(!shown.contains(sysin), "{shown}");
            assert!(!shown.contains(hidden), "{shown}");
            assert_eq!(refused.exit_code(), ExitCode::FAILURE);
        }
        Ok(())
    }

    /// An accepted `--sysin` record, the default card included, reaches the job as the run
    /// parameters the domain parses from it (CBPAUP0C-002).
    // audit: CBPAUP0C-002
    #[test]
    fn an_accepted_sysin_reaches_the_job_as_its_run_parameters() -> TestResult {
        let cases: [(&[&str], &str); 3] = [
            (&["cbpaup0j"], DEFAULT_SYSIN),
            (&["cbpaup0j", "--sysin", "05,00010"], "05,00010"),
            (&["cbpaup0j", "--sysin", ""], ""),
        ];
        for (args, sysin) in cases {
            let job = parse(args)?;
            let Ok(CheckedJob::Cbpaup0j(params)) = check(&job) else {
                return Err(format!("{args:?} did not check as cbpaup0j").into());
            };
            assert_eq!(params, PurgeParams::parse(sysin.as_bytes())?, "{args:?}");
        }
        Ok(())
    }

    const VAR_DIR: &str = "CARDDEMO_VAR_DIR";
    const STATEMENT_DIR: &str = "CARDDEMO_STATEMENT_DIR";

    /// The three jobs that write under `CARDDEMO_VAR_DIR`.
    const UNDER_VAR_DIR: [&[&str]; 3] = [&["posttran"], &["intcalc"], &["tranrept"]];

    /// The 11 jobs that write no file.
    const WRITING_NO_FILE: [&[&str]; 11] = [
        &["acctfile"],
        &["cardfile"],
        &["xreffile"],
        &["custfile"],
        &["discgrp"],
        &["tcatbalf"],
        &["trancatg"],
        &["trantype"],
        &["dusrsecj"],
        &["cbpaup0j"],
        &["mnttrdb2", "--input", "maint.txt"],
    ];

    /// A directory with non-ASCII text, spaces and punctuation, but no line break.
    const ORDINARY: &str = "/srv/vär dir (x)/ü-ß,#;!'~&";

    /// The variables a test sets, by name; every other variable is unset.
    type Env = Vec<(&'static str, String)>;

    /// The result of the output directory check for the job of `args` under the configuration
    /// whose only set variables are `env`.
    fn output_check(
        args: &[&str],
        env: &[(&str, String)],
    ) -> Result<Result<(), &'static str>, Box<dyn Error>> {
        let job = parse(args)?;
        let Ok(checked) = check(&job) else {
            return Err(format!("{args:?} did not check").into());
        };
        let config = Config::from_lookup(|name| {
            env.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| value.clone())
        })?;
        Ok(check_output_dir(&checked, &config))
    }

    /// A carriage return or a line feed in the directory a job writes into is refused before any
    /// file exists, with text that names the variables and not the value (AAP 0.3.4.3).
    #[test]
    fn a_line_break_in_the_directory_a_job_writes_into_is_refused() -> TestResult {
        for line_break in ['\n', '\r'] {
            let broken = format!("var{line_break}x");
            let mut cases: Vec<(&[&str], &str, &str)> = UNDER_VAR_DIR
                .into_iter()
                .map(|args| (args, VAR_DIR, VAR_DIR_LINE_BREAK))
                .collect();
            // The statement directory is `CARDDEMO_STATEMENT_DIR`, or defaults under
            // `CARDDEMO_VAR_DIR`.
            cases.push((&["creastmt"], STATEMENT_DIR, STATEMENT_DIR_LINE_BREAK));
            cases.push((&["creastmt"], VAR_DIR, STATEMENT_DIR_LINE_BREAK));
            for (args, var, refusal) in cases {
                let outcome = output_check(args, &[(var, broken.clone())])?;
                assert_eq!(outcome, Err(refusal), "{args:?} {var} {broken:?}");
                assert!(refusal.contains(var), "{refusal}");
                assert!(!refusal.contains(&broken), "{refusal}");
            }
        }
        Ok(())
    }

    /// Every other directory passes the check, a line break in a directory the job does not
    /// write into included, and so does every job that writes no file, whatever the directories
    /// hold.
    #[test]
    fn every_other_directory_and_every_job_writing_no_file_passes() -> TestResult {
        let ordinary = ORDINARY.to_owned();
        let mut cases: Vec<(&[&str], Env)> = Vec::new();
        for line_break in ['\n', '\r'] {
            let broken = format!("var{line_break}x");
            for args in UNDER_VAR_DIR {
                cases.push((args, vec![(STATEMENT_DIR, broken.clone())]));
            }
            // An explicit statement directory is the whole of `creastmt`'s directory.
            cases.push((
                &["creastmt"],
                vec![(VAR_DIR, broken.clone()), (STATEMENT_DIR, ordinary.clone())],
            ));
            for args in WRITING_NO_FILE {
                cases.push((
                    args,
                    vec![(VAR_DIR, broken.clone()), (STATEMENT_DIR, broken.clone())],
                ));
            }
        }
        for args in UNDER_VAR_DIR.into_iter().chain([&["creastmt"][..]]) {
            cases.push((args, Vec::new()));
            cases.push((args, vec![(VAR_DIR, ordinary.clone())]));
            cases.push((args, vec![(STATEMENT_DIR, ordinary.clone())]));
        }
        for (args, env) in cases {
            assert_eq!(output_check(args, &env)?, Ok(()), "{args:?} {env:?}");
        }
        Ok(())
    }

    /// A failure of a step on the output directory names the setting of the job's directory and
    /// the true cause, never a value, and keeps the generic failure status; the jobs that write
    /// no file have no such setting, and every other error is reported as it was.
    #[test]
    fn an_output_directory_failure_names_the_setting_of_the_jobs_directory() -> TestResult {
        let settings: [(&[&str], &str); 4] = [
            (&["posttran"], "dalyrejs under CARDDEMO_VAR_DIR"),
            (&["intcalc"], "systran under CARDDEMO_VAR_DIR"),
            (&["tranrept"], "reports under CARDDEMO_VAR_DIR"),
            (
                &["creastmt"],
                "CARDDEMO_STATEMENT_DIR, or statements under CARDDEMO_VAR_DIR when \
                 CARDDEMO_STATEMENT_DIR is unset",
            ),
        ];
        for (args, expected) in settings {
            let job = parse(args)?;
            let checked = check(&job).map_err(|refused| format!("{args:?}: {refused}"))?;
            let setting = output_setting(&checked).ok_or("a writing job has no setting")?;
            assert_eq!(setting, expected, "{args:?}");
            for context in [
                "allocating generation",
                "creating output directory",
                "locking output directory",
                "creating output file",
            ] {
                let error = JobError::Io {
                    context,
                    kind: io::ErrorKind::NotADirectory,
                }
                .naming_output_dir(setting);
                assert_eq!(
                    error.to_string(),
                    format!("{context} ({expected}): not a directory")
                );
                assert_eq!(finish(args[0], Err(error)), ExitCode::FAILURE);
            }
            let unchanged = JobError::Io {
                context: "writing output",
                kind: io::ErrorKind::StorageFull,
            }
            .naming_output_dir(setting);
            assert_eq!(unchanged.to_string(), "writing output: no storage space");
            let limit = JobError::GenerationLimit { base: "TRANREPT" }.naming_output_dir(setting);
            assert_eq!(
                limit.to_string(),
                "allocating generation: generation limit 9999 reached for TRANREPT"
            );
        }
        for args in WRITING_NO_FILE {
            let job = parse(args)?;
            let checked = check(&job).map_err(|refused| format!("{args:?}: {refused}"))?;
            assert_eq!(output_setting(&checked), None, "{args:?}");
        }
        Ok(())
    }

    /// A writer that refuses every write and every flush.
    struct Refusing;

    impl Write for Refusing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    fn outcome(return_code: u8) -> JobOutcome {
        JobOutcome {
            written: Vec::new(),
            return_code,
            counts: vec![(JobCounter::Records, 0)],
        }
    }

    #[test]
    fn paths_are_written_in_order_one_per_line_as_given() -> io::Result<()> {
        let paths = [
            "./var/dalyrejs/DALYREJS.G0001",
            "var/statements/STATEMNT.PS",
            "/abs/STATEMNT.HTML",
            "{dir}/STATEMNT.PDF",
            "./{dir}/STATEMNT.PDF",
        ]
        .map(PathBuf::from);
        let mut out = Vec::new();
        write_paths(&mut out, &paths)?;
        assert_eq!(
            String::from_utf8_lossy(&out),
            "./var/dalyrejs/DALYREJS.G0001\nvar/statements/STATEMNT.PS\n/abs/STATEMNT.HTML\n\
             ./{dir}/STATEMNT.PDF\n./{dir}/STATEMNT.PDF\n"
        );
        assert!(
            out.split(|&b| b == b'\n')
                .all(|line| line.first() != Some(&b'{'))
        );
        Ok(())
    }

    #[test]
    fn no_path_writes_nothing() -> io::Result<()> {
        let mut out = Vec::new();
        write_paths(&mut out, &[])?;
        assert!(out.is_empty());
        Ok(())
    }

    #[test]
    fn a_refused_write_or_flush_is_an_error() {
        assert!(write_paths(&mut Refusing, &[PathBuf::from("var/x")]).is_err());
        assert!(write_paths(&mut Refusing, &[]).is_err());
    }

    #[test]
    fn a_finished_job_exits_with_its_return_code() {
        for code in [0, 4] {
            assert_eq!(finish("posttran", Ok(outcome(code))), ExitCode::from(code));
        }
        assert_eq!(finish("acctfile", Ok(outcome(0))), ExitCode::SUCCESS);
    }

    #[test]
    fn a_failed_job_exits_with_its_abend_code_or_failure() {
        let abend = JobError::Abend {
            code: Some(16),
            reason: "CBPAUP0C ABENDING ...",
        };
        assert_eq!(finish("cbpaup0j", Err(abend)), ExitCode::from(16));
        let uncoded = JobError::Abend {
            code: None,
            reason: "ABENDING PROGRAM",
        };
        assert_eq!(finish("intcalc", Err(uncoded)), ExitCode::FAILURE);
        assert_eq!(
            finish("dusrsecj", Err(JobError::Password)),
            ExitCode::FAILURE
        );
    }

    /// The object of member `name` in the JSON event `line`, such as `"span"` for the current
    /// span's fields and name, without its braces. Neither member holds a nested object.
    fn member<'a>(line: &'a str, name: &str) -> Option<&'a str> {
        let (_, after) = line.split_once(&format!(r#""{name}":{{"#))?;
        after.split_once('}').map(|(object, _)| object)
    }

    /// The one captured event of `capture` whose message is `message`.
    fn sole_event(capture: &trace_capture::TraceCapture, message: &str) -> String {
        let needle = format!(r#""message":"{message}""#);
        let events: Vec<String> = capture
            .lines()
            .into_iter()
            .filter(|line| line.contains(&needle))
            .collect();
        let [event] = events.as_slice() else {
            panic!("expected one {message:?} event, got {events:?}");
        };
        event.clone()
    }

    fn assert_members(object: &str, expected: &[&str]) {
        for needle in expected {
            assert!(object.contains(needle), "{needle} missing from {object}");
        }
    }

    /// The outcome event of a job that ran to its end is a `WARN` for a non-zero return code
    /// and an `INFO` for 0, and either way the job span holds the return code and every counter
    /// as a number; no counter is left as a `Debug` string.
    #[test]
    fn the_job_finished_event_warns_on_a_non_zero_return_code_and_the_span_holds_the_outcome() {
        for (return_code, level) in [(4, "WARN"), (0, "INFO")] {
            let capture = trace_capture::capture();
            let outcome = JobOutcome {
                written: Vec::new(),
                return_code,
                counts: vec![(JobCounter::Processed, 300), (JobCounter::Rejected, 38)],
            };
            let code = job_span("posttran").in_scope(|| finish("posttran", Ok(outcome)));
            assert_eq!(code, ExitCode::from(return_code));

            let event = sole_event(&capture, "job finished");
            let return_code_member = format!(r#""return_code":{return_code}"#);
            assert!(event.contains(&format!(r#""level":"{level}""#)), "{event}");
            let Some(fields) = member(&event, "fields") else {
                panic!("no fields member in {event}");
            };
            assert_members(
                fields,
                &[r#""job":"posttran""#, &return_code_member, r#""written":0"#],
            );
            let Some(span) = member(&event, "span") else {
                panic!("no span member in {event}");
            };
            assert_members(
                span,
                &[
                    r#""name":"batch.job""#,
                    r#""job":"posttran""#,
                    &return_code_member,
                    r#""processed":300"#,
                    r#""rejected":38"#,
                ],
            );
            assert!(!capture.contains(r#""counts":"#), "{}", capture.text());
        }
    }

    /// A failed job records the return code its error carries on the job span before its
    /// `ERROR` event, and an error without one records none.
    #[test]
    fn a_failed_job_records_its_error_return_code_on_the_job_span() {
        for code in [Some(16), None] {
            let capture = trace_capture::capture();
            let error = JobError::Abend {
                code,
                reason: "CBPAUP0C ABENDING ...",
            };
            let exit = job_span("cbpaup0j").in_scope(|| finish("cbpaup0j", Err(error)));
            assert_eq!(exit, code.map_or(ExitCode::FAILURE, ExitCode::from));

            let event = sole_event(&capture, "job failed");
            assert!(event.contains(r#""level":"ERROR""#), "{event}");
            let Some(span) = member(&event, "span") else {
                panic!("no span member in {event}");
            };
            assert_members(span, &[r#""name":"batch.job""#, r#""job":"cbpaup0j""#]);
            match code {
                Some(code) => assert_members(span, &[&format!(r#""return_code":{code}"#)]),
                None => assert!(!span.contains("return_code"), "{span}"),
            }
            assert!(!capture.contains("job finished"), "{}", capture.text());
        }
    }

    /// A failure path of `main`: it reports in the current span and returns the exit status.
    type Failure<'a> = &'a dyn Fn() -> ExitCode;

    /// Each failure event of `main` is one `ERROR` event of target `carddemo_batch` in the job
    /// span, whose fields are exactly the message, the job as text and, where the failure has
    /// one, the error as its `Display` text, in that order, and the run exits with status 1.
    #[test]
    fn a_failure_event_is_one_error_in_the_job_span_with_the_job_and_the_error() {
        let abend = || JobError::Abend {
            code: None,
            reason: "ABENDING PROGRAM",
        };
        let failed_job = || finish("intcalc", Err(abend()));
        // What building the runtime's I/O driver meets when the process may open no more file
        // descriptors: `EMFILE`, error 24 on Linux.
        let no_descriptor = io::Error::from_raw_os_error(24);
        let cases: [(&str, Failure<'_>, &str, Option<String>); 3] = [
            (
                "posttran",
                &|| runtime_failed("posttran", &no_descriptor),
                "cannot build the async runtime",
                Some(no_descriptor.to_string()),
            ),
            (
                "tranrept",
                &|| stdout_failed("tranrept"),
                "cannot write to standard output",
                None,
            ),
            (
                "intcalc",
                &failed_job,
                "job failed",
                Some(abend().to_string()),
            ),
        ];
        for (job, fails, message, error) in cases {
            let capture = trace_capture::capture();
            let code = job_span(job).in_scope(fails);
            assert_eq!(code, ExitCode::FAILURE, "{message}");

            let event = sole_event(&capture, message);
            assert!(event.contains(r#""level":"ERROR""#), "{event}");
            assert!(event.contains(r#""target":"carddemo_batch""#), "{event}");
            let Some(fields) = member(&event, "fields") else {
                panic!("no fields member in {event}");
            };
            let expected = match error {
                Some(error) => format!(r#""message":"{message}","job":"{job}","error":"{error}""#),
                None => format!(r#""message":"{message}","job":"{job}""#),
            };
            assert_eq!(fields, expected, "{event}");
            let Some(span) = member(&event, "span") else {
                panic!("no span member in {event}");
            };
            assert_members(
                span,
                &[r#""name":"batch.job""#, &format!(r#""job":"{job}""#)],
            );
        }
    }
}
