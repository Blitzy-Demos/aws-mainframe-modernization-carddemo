//! `carddemo-mq`: the CDRD (`CODATE01`), CDRA (`COACCT01`) and CP00 (`COPAUA0C`) workers in one
//! long-running process, in place of the CICS tasks that an MQ trigger started for each program.
//!
//! Everything comes from the environment through `runtime::config::Config`: the request queues
//! (`CARDDEMO_QUEUE_REQUEST_DATE`, `CARDDEMO_QUEUE_REQUEST_ACCT`, `CARDDEMO_QUEUE_REQUEST_PAUTH`),
//! `DATABASE_URL`, `CARDDEMO_AMQP_URL` and `CARDDEMO_MQ_POLL_MS`. The process reads no file and
//! never changes the schema: run the `carddemo-migrate` binary first, or the workers fail at
//! their first query. This file only wires configuration, telemetry, the database pool and the
//! clock; the workers, their reconnects and their failure paths are `messaging::run`.
//!
//! The process runs until it is stopped or `messaging::run` reports a fatal startup error. It
//! exits non-zero on any failure: a configuration or telemetry error as one line on standard
//! error, since no subscriber exists yet; a database or broker error as one JSON `ERROR` event on
//! standard output. No message names the connection URLs or any value they carry.

use std::process::ExitCode;
use std::sync::Arc;

use runtime::config::Config;

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("carddemo-mq: configuration error: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Bound until `main` returns: dropping the guard flushes the exported spans after the last
    // event.
    let _telemetry = match runtime::telemetry::init(config.otel_export) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("carddemo-mq: telemetry initialisation failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let pool = match runtime::db::connect(&config).await {
        Ok(pool) => pool,
        Err(error) => {
            // Logged only as the value-free summary: the driver's own text can quote the
            // connection URL, password included.
            let error = runtime::db::ErrorSummary::from(&error);
            tracing::error!(%error, "database connection failed");
            return ExitCode::FAILURE;
        }
    };
    let store = persistence::PgStore::new(pool);
    let Err(error) = messaging::run(&config, store, Arc::new(domain::SystemClock)).await;
    tracing::error!(%error, "carddemo-mq stopped");
    ExitCode::FAILURE
}
