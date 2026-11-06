//! `batch`: the command line that replaces the `CardDemo` JCL jobs carrying business logic, one
//! subcommand per job, with the parameters the JCL passes as defaults.
//!
//! | Subcommand | JCL job |
//! |---|---|
//! | `migrate` | CREADB21 (DSNTEP4), the schema |
//! | `seed` | ACCTFILE, CARDFILE, CUSTFILE, XREFFILE, DISCGRP, TCATBALF, TRANTYPE, TRANCATG, DUSRSECJ and the CREADB21 loads; TRANEXTR (DSNTIAUL) with `--only tranextr` |
//! | `posttran` | POSTTRAN (CBTRN02C) |
//! | `intcalc` | INTCALC (CBACT04C) |
//! | `tranbkp` | TRANBKP (REPROC, IDCAMS DELETE/DEFINE) |
//! | `combtran` | COMBTRAN (SORT, IDCAMS REPRO) |
//! | `creastmt` | CREASTMT (CBSTM03A, CBSTM03B) |
//! | `tranrept` | TRANREPT (REPROC, SORT, CBTRN03C) |
//! | `cbpaup0j` | CBPAUP0J (CBPAUP0C) |
//! | `mnttrdb2` | MNTTRDB2 (COBTUPDT) |
//!
//! The business rules live in `carddemo_domain::rules`. A job only orchestrates: it runs the JCL
//! steps in their order, writes the generation files under `CARDDEMO_BATCH_DIR`, opens the
//! database transactions through `carddemo_persistence`, decides the exit code and emits the
//! telemetry.
//!
//! # Start-up
//!
//! The arguments are parsed first, so `--help`, `--version` (exit 0) and usage errors (exit 2)
//! need no environment, database or telemetry. Then, in order: the JSON telemetry is installed,
//! the `CARDDEMO_*` settings are resolved, and the processing clock is built from
//! `CARDDEMO_CLOCK_OVERRIDE`: the host clock, or the pinned RFC 3339 instant that every
//! time-dependent output then uses (processing timestamps, accrual times, the purge day). A
//! failure in any of the three is a setup failure, exit 16. The database is opened by the job
//! dispatch, inside the job span.
//!
//! # Output and exit status
//!
//! Standard output carries only the JSON lines of `carddemo_runtime::telemetry`: the span lines
//! and the `tracing` events, into which the COBOL `DISPLAY` output turns inside the jobs.
//! Standard error carries only the lines that report an abend, a reward-engine halt, a
//! `creastmt` capacity stop or a setup failure, plus one line should the telemetry shutdown
//! fail. The exit code is the job's condition code: 0 success, 4 records rejected, 8 the
//! `posttran` reward-engine halt, 16 an abend or a setup failure, or the IDCAMS REPRO condition
//! code of `tranbkp`, `combtran` and `tranrept`.
//!
//! Batch runs as the operating-system operator, as JCL does: no application identity is read or
//! recorded.

mod cli;
mod exit;
mod generations;
mod jobs;
mod pdf;

use std::fmt;
use std::io::Write;

use carddemo_domain::clock::AnyClock;
use carddemo_runtime::config::Settings;
use carddemo_runtime::telemetry::{self, TelemetryGuard};
use clap::Parser;

use crate::cli::Cli;
use crate::exit::ExitStatus;
use crate::jobs::JobContext;

/// The process name in telemetry: the tracer name and the `service.name` resource of every
/// exported span.
const SERVICE_NAME: &str = "batch";

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let (guard, ctx) = match setup() {
        Ok(ready) => ready,
        Err(status) => {
            status.report();
            std::process::exit(status.code());
        }
    };
    let status = jobs::dispatch(cli.command, &ctx).await;
    status.report();
    shutdown(guard);
    std::process::exit(status.code());
}

/// Installs the telemetry, then resolves the settings and the processing clock, and returns the
/// telemetry guard with the context every job runs in.
///
/// # Errors
///
/// A setup failure (exit 16) when the telemetry cannot be installed, a `CARDDEMO_*` variable is
/// unusable, or `CARDDEMO_CLOCK_OVERRIDE` is not an RFC 3339 instant. A guard taken by then is
/// dropped on the way out, which shuts the telemetry down and flushes its spans.
fn setup() -> Result<(TelemetryGuard, JobContext), ExitStatus> {
    // No subscriber exists until this succeeds, so its failure reaches standard error only.
    let guard = telemetry::init(SERVICE_NAME).map_err(ExitStatus::setup)?;
    let settings =
        Settings::from_env().map_err(|err| setup_failure("batch settings are invalid", &err))?;
    let clock = AnyClock::from_override(settings.clock_override.as_deref())
        .map_err(|err| setup_failure("processing clock override is invalid", &err))?;
    Ok((guard, JobContext::new(settings, clock)))
}

/// Logs `err` under `what` as a JSON event and turns it into the setup failure that ends the run.
/// Neither the settings nor the clock error carries a credential.
fn setup_failure(what: &'static str, err: &impl fmt::Display) -> ExitStatus {
    tracing::error!(error = %err, "{what}");
    ExitStatus::setup(err)
}

/// Shuts the telemetry down. `std::process::exit` runs no destructor, so without this call the
/// guard would never flush the exporter. A failure is reported on one standard error line and
/// leaves the exit code to the job.
fn shutdown(guard: TelemetryGuard) {
    if let Err(err) = guard.shutdown() {
        // Standard error is the last channel left: a failed write has nowhere to be reported.
        let _ = writeln!(
            std::io::stderr().lock(),
            "TELEMETRY SHUTDOWN FAILURE: {err}"
        );
    }
}
