//! `api`: the `CardDemo` REST API server, the process that replaces the CICS
//! region of the online transactions.
//!
//! On the mainframe the `CARDDEMO` CSD group bound each online transaction code
//! to its program, one `DEFINE TRANSACTION` each [app/csd/CARDDEMO.CSD:L306-487],
//! and the region served them to 3270 terminals. Here every transaction is a
//! route of [`carddemo_api::app`]. This binary only wires the shared components
//! together and serves them: the settings, the telemetry, the database pool,
//! the application state, then the listener.
//!
//! # Starting
//!
//! - On the host: `cargo run -p carddemo-api --bin api`, listening on
//!   `CARDDEMO_HTTP_ADDR` (by default `127.0.0.1:8080`).
//! - In Compose: the `app` profile service `api` of `docker-compose.yml`, which
//!   compiles from the committed offline query metadata and sets the listen
//!   address and the database URL to its container values.
//!
//! Every setting comes from `carddemo_runtime::config::Settings`: the
//! `CARDDEMO_*` variables and `RUST_LOG` that `.env.example` lists, each with
//! its local default. The process takes no arguments.
//!
//! The binary neither migrates nor seeds the database: `batch migrate` and
//! `batch seed` do that before the server starts.
//!
//! # Output and exit status
//!
//! Standard output carries JSON lines only, written by
//! `carddemo_runtime::telemetry`: the request and handler spans and the log
//! events. Start-up ends with the `api listening` event, whose `addr` field is
//! the address actually bound.
//!
//! A failure is one line on standard error, `api: <stage>: <error>`, and exit
//! status 1. The stages, in start-up order, are `configuration`, `telemetry`,
//! `database`, `state`, `bind <addr>` and, once serving, `serve`. A failure
//! after telemetry is installed is also an ERROR event on standard output. No
//! database URL or secret is ever written: neither the settings errors nor the
//! database errors carry one.
//!
//! The server runs until the process is stopped. There is no graceful
//! shutdown: a signal ends the process, and `PostgreSQL` rolls back the unit of
//! work of any request still in flight when its connection closes. Every span
//! that finished before then is already on standard output, because the span
//! exporter writes each one as it ends.

use std::fmt::Display;
use std::process::ExitCode;

use carddemo_api::{AppState, app};
use carddemo_persistence::connect;
use carddemo_runtime::config::Settings;
use carddemo_runtime::telemetry;
use tokio::net::TcpListener;

/// The process name in telemetry: the tracer name and the `service.name`
/// resource of every exported span.
const SERVICE_NAME: &str = "api";

#[tokio::main]
async fn main() -> ExitCode {
    let settings = match Settings::from_env() {
        Ok(settings) => settings,
        Err(err) => return fail("configuration", &err),
    };
    // Bound to a name, not to `_`, so the guard lives until `main` returns on
    // every path; dropping it then shuts the span export down.
    let _telemetry = match telemetry::init(SERVICE_NAME) {
        Ok(guard) => guard,
        Err(err) => return fail("telemetry", &err),
    };
    let db = match connect(&settings.database_url).await {
        Ok(db) => db,
        Err(err) => return fail("database", &err),
    };
    // `AppState::new` takes the settings by value, so the address is copied
    // out first.
    let addr = settings.http_addr;
    let state = match AppState::new(db, settings) {
        Ok(state) => state,
        Err(err) => return fail("state", &err),
    };
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => return fail(&format!("bind {addr}"), &err),
    };
    // With port 0 the system picks the port, so the event names the address
    // the listener holds rather than the configured one.
    let bound = listener.local_addr().unwrap_or(addr);
    tracing::info!(addr = %bound, "api listening");
    match axum::serve(listener, app(state)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => fail("serve", &err),
    }
}

/// Reports a failure at `stage` and returns exit status 1.
///
/// The standard error line is the report an operator always gets: before
/// telemetry is installed it is the only channel that keeps standard output
/// JSON-only. Afterwards the same failure is also an ERROR event among the
/// JSON lines; before that the event has no subscriber and is discarded.
fn fail(stage: &str, err: &dyn Display) -> ExitCode {
    tracing::error!(stage, error = %err, "api failed");
    eprintln!("api: {stage}: {err}");
    ExitCode::FAILURE
}
