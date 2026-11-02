//! `api`: the `CardDemo` REST API server, the process that replaces the CICS
//! region of the online transactions.
//!
//! On the mainframe the `CARDDEMO` CSD group bound each online transaction code
//! to its program, one `DEFINE TRANSACTION` each [app/csd/CARDDEMO.CSD:L306-487],
//! and the region served them to 3270 terminals. Here every transaction is a
//! route of [`carddemo_api::app`]. This binary only wires the shared components
//! together and serves them: the settings, the telemetry, the application
//! state, the database pool, the listener, then the stop signals.
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
//! `state`, `database`, `bind <addr>`, `signals` and, once serving, `serve`.
//! Line breaks in the error text become spaces, so the report stays one line.
//! Every setting is checked by `state` before `database` makes the first
//! connection attempt, so an unreachable database never hides an invalid
//! setting. A failure after telemetry is installed is also an ERROR event on
//! standard output. No database URL or secret is ever written: neither the
//! settings errors nor the database errors carry one.
//!
//! # Stopping
//!
//! SIGINT (Ctrl-C) and SIGTERM (`kill`, `docker compose stop`) stop the server
//! gracefully. It logs `api stopping: draining in-flight requests` with the
//! signal's name, stops accepting connections, closes the idle ones and lets
//! every request in flight finish. It then closes the database pool, logs
//! `api stopped`, shuts telemetry down, which flushes every pending span, and
//! exits with status 0. A request that never finishes holds the shutdown
//! open; SIGKILL then ends the process, and `PostgreSQL` rolls back the unit
//! of work of any request still in flight when its connection closes.

use std::fmt::Display;
use std::process::ExitCode;

use carddemo_api::{PreparedState, app};
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
    // Every setting is checked, and the clock parsed, before the first
    // connection attempt, so a database failure cannot mask an invalid one.
    let prepared = match PreparedState::new(settings) {
        Ok(prepared) => prepared,
        Err(err) => return fail("state", &err),
    };
    let db = match connect(&prepared.settings().database_url).await {
        Ok(db) => db,
        Err(err) => return fail("database", &err),
    };
    // `into_state` moves the settings into the state, so the address is
    // copied out first. A handle is kept to close the pool on shutdown.
    let addr = prepared.settings().http_addr;
    let state = prepared.into_state(db.clone());
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => return fail(&format!("bind {addr}"), &err),
    };
    // Installed before `api listening`, so from then on a stop signal drains
    // the server instead of killing the process.
    #[cfg(unix)]
    let stop = match stop_signal() {
        Ok(stop) => stop,
        Err(err) => return fail("signals", &err),
    };
    #[cfg(not(unix))]
    let stop = stop_signal();
    // With port 0 the system picks the port, so the event names the address
    // the listener holds rather than the configured one.
    let bound = listener.local_addr().unwrap_or(addr);
    tracing::info!(addr = %bound, "api listening");
    let draining = async move {
        let signal = stop.await;
        tracing::info!(signal, "api stopping: draining in-flight requests");
    };
    match axum::serve(listener, app(state))
        .with_graceful_shutdown(draining)
        .await
    {
        Ok(()) => {
            db.close().await;
            tracing::info!("api stopped");
            // Returning drops `_telemetry`, which flushes the span export.
            ExitCode::SUCCESS
        }
        Err(err) => fail("serve", &err),
    }
}

/// Installs the SIGINT (Ctrl-C) and SIGTERM (`kill`, `docker compose stop`)
/// streams and returns a future that resolves to the name of the first of
/// them the process receives.
///
/// Once installed, neither signal ends the process by itself any more: the
/// server stops accepting connections, answers the requests in flight and
/// returns.
///
/// # Errors
///
/// The I/O error of a stream that cannot be installed.
#[cfg(unix)]
fn stop_signal() -> std::io::Result<impl Future<Output = &'static str> + Send + 'static> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
        }
    })
}

/// Resolves to `ctrl-c` when the process receives Ctrl-C, on a platform
/// without Unix signals.
///
/// A handler that cannot be installed is logged as an ERROR event, and the
/// future then never resolves: the server keeps serving until the process is
/// killed.
#[cfg(not(unix))]
async fn stop_signal() -> &'static str {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %err, "api cannot handle Ctrl-C: it stops only when killed");
        std::future::pending::<()>().await;
    }
    "ctrl-c"
}

/// Reports a failure at `stage` and returns exit status 1.
///
/// The standard error line is the report an operator always gets: before
/// telemetry is installed it is the only channel that keeps standard output
/// JSON-only. Afterwards the same failure is also an ERROR event among the
/// JSON lines; before that the event has no subscriber and is discarded.
/// The standard error line is [`stderr_report`]'s, so it stays one line.
fn fail(stage: &str, err: &dyn Display) -> ExitCode {
    tracing::error!(stage, error = %err, "api failed");
    eprintln!("{}", stderr_report(stage, err));
    ExitCode::FAILURE
}

/// The standard error line that reports a failure at `stage`,
/// `api: <stage>: <error>`, without its line ending.
///
/// Line breaks in the error text, such as one inside an invalid
/// `CARDDEMO_CLOCK_OVERRIDE`, become spaces, so the report is always one
/// line. The ERROR event keeps the error text as it is, since JSON escapes
/// its line breaks.
fn stderr_report(stage: &str, err: &dyn Display) -> String {
    let text = err.to_string().replace(['\r', '\n'], " ");
    format!("api: {stage}: {text}")
}

#[cfg(test)]
mod tests {
    use super::stderr_report;

    #[test]
    fn the_stderr_report_is_one_line_with_line_breaks_as_spaces() {
        for (stage, error, expected) in [
            (
                "state",
                "invalid CARDDEMO_CLOCK_OVERRIDE: CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: 2022\nbad",
                "api: state: invalid CARDDEMO_CLOCK_OVERRIDE: CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: 2022 bad",
            ),
            ("state", "2022\r\nbad", "api: state: 2022  bad"),
            ("state", "2022\rbad", "api: state: 2022 bad"),
            ("configuration", "a\n\nb\n", "api: configuration: a  b "),
            (
                "bind 127.0.0.1:8080",
                "Address already in use (os error 98)",
                "api: bind 127.0.0.1:8080: Address already in use (os error 98)",
            ),
        ] {
            let report = stderr_report(stage, &error);
            assert_eq!(report, expected, "{error:?}");
            assert!(!report.contains(['\r', '\n']), "{report:?}");
        }
    }
}
