//! `mq`: the long-running process that replaces the CICS MQ trigger monitor of
//! `CardDemo`.
//!
//! On the mainframe an IBM MQ trigger message started one CICS transaction per
//! request, bound to its program in the CSD: CDRD runs `CODATE01` and CDRA runs
//! `COACCT01` [app/app-vsam-mq/csd/CRDDEMOM.csd:L17-36], and CP00 runs
//! `COPAUA0C` [app/app-authorization-ims-db2-mq/csd/CRDDEMO2.csd:L59-60]. Each
//! program took its input queue from the trigger message (`MQTM-QNAME`)
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L146] [app/app-vsam-mq/cbl/COACCT01.cbl:L197].
//! Here the three are `RabbitMQ` consumers that `carddemo_messaging::run`
//! starts on the request queues the settings name. This binary only wires them
//! up: settings, telemetry, the processing clock, the database pool, then
//! `run`.
//!
//! # Starting
//!
//! - On the host: `cargo run -p carddemo-messaging --bin mq`, configured by the
//!   `CARDDEMO_*` variables `.env.example` lists.
//! - In Compose: the `app` profile service `mq` of `docker-compose.yml`, which
//!   compiles from the committed offline query metadata and points the
//!   database and broker URLs at the `postgres` and `rabbitmq` services.
//!
//! The process takes no arguments and checks no identity: the requests carry
//! no credential.
//!
//! # Output and exit status
//!
//! Standard output carries JSON lines only, from the shared telemetry
//! formatter of `carddemo_runtime::telemetry`: the log events and the spans.
//!
//! A failure is one line on standard error, `mq: <stage>: <error>`, as `api`
//! reports its own, and exit status 1. The stages, in start-up order, are
//! `configuration`, `telemetry`, `clock` (`CARDDEMO_CLOCK_OVERRIDE`),
//! `database` (opening the pool) and `serve` (the broker connection, the
//! consumers, and the first of them to stop). Line breaks in the error text
//! become spaces, so the report stays one line.
//!
//! Each failure is also a JSON line on standard output. One before telemetry
//! is installed (`configuration`, `telemetry`) is the ERROR event `mq start-up
//! failed` with the `stage` and the `error`. Once telemetry is installed, the
//! end of the run is the JSON event `mq stopped`, with the `stage` and the
//! `error` when the run failed. Telemetry is then shut down and flushed before
//! the process exits, and a failed shutdown is one more ERROR event,
//! `telemetry shutdown failed`, which writes nothing to standard error and
//! leaves the exit status unchanged. No database or broker URL is ever
//! written, because both carry credentials.
//!
//! A signal ends the process without settling the delivery in flight; the
//! broker redelivers it once the connection drops.

use std::fmt::Display;
use std::process::ExitCode;

use carddemo_domain::clock::AnyClock;
use carddemo_messaging::{MessagingError, STATEMENT_TIMEOUT, run};
use carddemo_persistence::tx::connect_with_statement_timeout;
use carddemo_runtime::config::Settings;
use carddemo_runtime::telemetry;

/// The process name in telemetry: the tracer name, the `service.name` resource
/// of every exported span, and the `service` field of the startup event.
const SERVICE_NAME: &str = "mq";

#[tokio::main]
async fn main() -> ExitCode {
    let settings = match Settings::from_env() {
        Ok(settings) => settings,
        Err(error) => {
            report_startup_failure("configuration", &error);
            return ExitCode::FAILURE;
        }
    };
    let guard = match telemetry::init(SERVICE_NAME) {
        Ok(guard) => guard,
        Err(error) => {
            report_startup_failure("telemetry", &error);
            return ExitCode::FAILURE;
        }
    };

    let code = match serve(&settings).await {
        Ok(()) => {
            tracing::info!("mq stopped");
            ExitCode::SUCCESS
        }
        Err((stage, error)) => {
            tracing::error!(stage, error = %error, "mq stopped");
            report_on_stderr(stage, &error);
            ExitCode::FAILURE
        }
    };
    // Shut down after the last event, so it and every pending span are
    // flushed. The global subscriber's JSON log layer outlives the provider,
    // so a failed shutdown is still a JSON line on standard output; it leaves
    // the exit status to the consumers' outcome, which is what the status
    // reports.
    if let Err(error) = guard.shutdown() {
        tracing::error!(error = %error, "telemetry shutdown failed");
    }
    code
}

/// Reports a start-up failure that happens before [`telemetry::init`] has
/// installed the process telemetry: one ERROR event, `mq start-up failed`,
/// with the failed `stage` and the `error`, written as a JSON line on standard
/// output by the shared telemetry formatter, like every other event of the
/// process, and the standard error line `mq: <stage>: <error>` of
/// [`report_on_stderr`].
///
/// The subscriber is scoped to this one event, and dropping its guard shuts
/// its tracer provider down. `error` is a settings or telemetry error, neither
/// of which ever holds a database or broker URL.
fn report_startup_failure(stage: &'static str, error: &dyn Display) {
    report_on_stderr(stage, error);
    // The fixed directive `info` always parses, so this subscriber is always
    // built. Were it not, the standard error line would be the whole report.
    let Ok((subscriber, guard)) =
        telemetry::subscriber_with_writer(SERVICE_NAME, "info", std::io::stdout)
    else {
        return;
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::error!(stage, error = %error, "mq start-up failed");
    });
    drop(guard);
}

/// Writes the one standard error line that reports a failure at `stage`,
/// `mq: <stage>: <error>`, the form `api` reports its own failures in.
///
/// Standard output stays JSON-only, so this line is the plain-text report an
/// operator reads. Line breaks in the error text, such as one inside an
/// invalid `CARDDEMO_CLOCK_OVERRIDE`, become spaces, so the report is always
/// one line. No error that reaches it holds a database or broker URL.
fn report_on_stderr(stage: &str, error: &dyn Display) {
    let text = error.to_string().replace(['\r', '\n'], " ");
    eprintln!("mq: {stage}: {text}");
}

/// Parses the processing clock from `CARDDEMO_CLOCK_OVERRIDE`, opens the
/// database pool on `CARDDEMO_DATABASE_URL`, then runs the CDRD, CDRA and
/// CP00 consumers on that clock until the first one stops.
///
/// The pool's connections carry [`STATEMENT_TIMEOUT`], so the server cancels
/// a statement blocked behind another session's lock before the flow's own
/// deadline gives up on it.
///
/// The clock is parsed first, and only here, so an invalid override is
/// reported before any connection attempt and an unreachable database cannot
/// mask it. The startup event names the request queues only; the URLs carry
/// credentials and are never logged.
///
/// # Errors
///
/// The failed stage with its error: `clock` and [`MessagingError::Clock`]
/// when the override is not an RFC 3339 instant, `database` and
/// [`MessagingError::Persistence`] when the pool cannot be opened, and
/// `serve` with any error of `run`, from the broker connection to the first
/// consumer that stops.
async fn serve(settings: &Settings) -> Result<(), (&'static str, MessagingError)> {
    tracing::info!(
        service = SERVICE_NAME,
        queue_date_request = %settings.queue_date_request,
        queue_acct_request = %settings.queue_acct_request,
        queue_pauth_request = %settings.queue_pauth_request,
        "mq starting"
    );
    let clock = AnyClock::from_override(settings.clock_override.as_deref())
        .map_err(|error| ("clock", MessagingError::Clock(error)))?;
    let db = connect_with_statement_timeout(&settings.database_url, STATEMENT_TIMEOUT)
        .await
        .map_err(|error| ("database", MessagingError::Persistence(error)))?;
    run(settings, clock, db)
        .await
        .map_err(|error| ("serve", error))
}
