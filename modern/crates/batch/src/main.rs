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
//! This file holds only the process lifecycle: it parses the command line, reads the
//! configuration, installs telemetry, opens the `PostgreSQL` pool and runs the selected job.
//! Business rules live in `domain::batch`, every query in the `persistence` repository traits and
//! every file read and write in the job modules.
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
//!
//! # Standard error
//!
//! A failed run writes one line, `carddemo-batch <subcommand>: <reason>`. The reason is a
//! configuration error that names its variable and never its value, a telemetry failure, a
//! database connection failure as the value-free summary of `runtime::db`, the job's error, or
//! a failure to write standard output. None of them shows a record, a card number, an SSN, a
//! password or the connection URL (Cardholder Data Redaction).
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
//! - 1 for every other failure: configuration, telemetry, database connection, standard output,
//!   or a job error that carries no return code.
//! - 2 for a usage error, reported by `clap` before anything else runs, so `--help` and
//!   `--version` need neither the environment nor a database.

mod cli;
mod jobs;
mod pdf;

use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use domain::batch::loads::LoadJob;
use domain::clock::SystemClock;
use persistence::PgStore;
use runtime::config::Config;
use tracing::Instrument;

use crate::cli::{Cli, Job, LoadArgs};
use crate::jobs::{JobContext, JobError, JobOutcome};

/// The prefix of every standard-error line, followed by the subcommand.
const PROGRAM: &str = "carddemo-batch";

#[tokio::main]
async fn main() -> ExitCode {
    // First, so that `--help`, `--version` and usage errors need no environment.
    let cli = Cli::parse();
    let job = cli.job.name();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => return fail(job, &format_args!("configuration error: {error}")),
    };
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
    let code = execute(job, &cli.job, &config)
        .instrument(runtime::telemetry::job_span(job))
        .await;
    drop(guard);
    code
}

/// Opens the pool, runs `selected` and reports its result, inside the job span.
async fn execute(job: &'static str, selected: &Job, config: &Config) -> ExitCode {
    let pool = match runtime::db::connect(config).await {
        Ok(pool) => pool,
        Err(error) => {
            // The summary keeps an error class and a SQLSTATE at most: the driver's own text can
            // quote the connection URL.
            let error = runtime::db::ErrorSummary::from(&error);
            tracing::error!(job, error = %error, "database connection failed");
            return fail(
                job,
                &format_args!("cannot connect to the database: {error}"),
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
    let result = dispatch(selected, &ctx).await;
    // Every job has committed or rolled back its work by now; closing ends the sessions cleanly
    // instead of dropping their connections when the runtime shuts down.
    store.pool().close().await;
    finish(job, result)
}

/// Runs the job the subcommand selected. The match names every variant and has no wildcard
/// arm, so a subcommand without a job fails to compile here.
async fn dispatch(job: &Job, ctx: &JobContext<'_>) -> Result<JobOutcome, JobError> {
    match job {
        Job::Acctfile(args) => run_load(ctx, LoadJob::Acctfile, args).await,
        Job::Cardfile(args) => run_load(ctx, LoadJob::Cardfile, args).await,
        Job::Xreffile(args) => run_load(ctx, LoadJob::Xreffile, args).await,
        Job::Custfile(args) => run_load(ctx, LoadJob::Custfile, args).await,
        Job::Discgrp(args) => run_load(ctx, LoadJob::Discgrp, args).await,
        Job::Tcatbalf(args) => run_load(ctx, LoadJob::Tcatbalf, args).await,
        Job::Trancatg(args) => run_load(ctx, LoadJob::Trancatg, args).await,
        Job::Trantype(args) => run_load(ctx, LoadJob::Trantype, args).await,
        Job::Dusrsecj(args) => jobs::dusrsecj::run(ctx, &args.input).await,
        Job::Posttran(args) => jobs::posttran::run(ctx, &args.dalytran).await,
        Job::Intcalc(args) => jobs::intcalc::run(ctx, args).await,
        Job::Creastmt => jobs::creastmt::run(ctx).await,
        Job::Tranrept(args) => jobs::tranrept::run(ctx, args).await,
        Job::Cbpaup0j(args) => jobs::cbpaup0j::run(ctx, args).await,
        Job::Mnttrdb2(args) => jobs::mnttrdb2::run(ctx, &args.input).await,
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

/// Reports the job's result: the written paths on standard output and the outcome as a tracing
/// event, or the error on standard error and as a tracing event, and returns the exit code.
fn finish(job: &'static str, result: Result<JobOutcome, JobError>) -> ExitCode {
    match result {
        Ok(outcome) => {
            if print_paths(&outcome.written).is_err() {
                tracing::error!(job, "cannot write to standard output");
                return fail(job, &"cannot write to standard output");
            }
            tracing::info!(
                job,
                return_code = outcome.return_code,
                counts = ?outcome.counts,
                written = outcome.written.len(),
                "job finished"
            );
            ExitCode::from(outcome.return_code)
        }
        Err(error) => {
            tracing::error!(job, error = %error, "job failed");
            report(job, &error);
            error.exit_code().map_or(ExitCode::FAILURE, ExitCode::from)
        }
    }
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
    use std::io::{self, Write};
    use std::path::PathBuf;
    use std::process::ExitCode;

    use super::{finish, write_paths};
    use crate::jobs::{JobError, JobOutcome};

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
            counts: vec![("records", 0)],
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
}
