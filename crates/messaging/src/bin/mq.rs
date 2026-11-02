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
//! up: settings, telemetry, the database pool, then `run`.
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
//! Standard output carries JSON lines only, written by
//! `carddemo_runtime::telemetry`. A failure before telemetry is installed
//! (invalid settings, telemetry setup) is one line on standard error. After
//! that, the end of the run is the JSON event `mq stopped`, and telemetry is
//! shut down and flushed before the process exits. The exit status is non-zero
//! whenever the consumers stop on an error. No database or broker URL is ever
//! written, because both carry credentials.
//!
//! A signal ends the process without settling the delivery in flight; the
//! broker redelivers it once the connection drops.

use std::process::ExitCode;

use carddemo_messaging::{MessagingError, run};
use carddemo_persistence::connect;
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
            // No subscriber is installed yet, so standard error is the only
            // channel that keeps standard output JSON-only.
            eprintln!("mq: invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    let guard = match telemetry::init(SERVICE_NAME) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("mq: telemetry initialisation failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    let code = match serve(&settings).await {
        Ok(()) => {
            tracing::info!("mq stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(error = %error, "mq stopped");
            ExitCode::FAILURE
        }
    };
    // Shut down after the last event, so it and every pending span are
    // flushed. A failed shutdown goes to standard error and leaves the exit
    // status to the consumers' outcome, which is what the status reports.
    if let Err(error) = guard.shutdown() {
        eprintln!("mq: telemetry shutdown failed: {error}");
    }
    code
}

/// Opens the database pool on `CARDDEMO_DATABASE_URL`, then runs the CDRD,
/// CDRA and CP00 consumers until the first one stops.
///
/// The startup event names the request queues only; the URLs carry
/// credentials and are never logged.
async fn serve(settings: &Settings) -> Result<(), MessagingError> {
    tracing::info!(
        service = SERVICE_NAME,
        queue_date_request = %settings.queue_date_request,
        queue_acct_request = %settings.queue_acct_request,
        queue_pauth_request = %settings.queue_pauth_request,
        "mq starting"
    );
    let db = connect(&settings.database_url)
        .await
        .map_err(MessagingError::Persistence)?;
    run(settings, db).await
}
