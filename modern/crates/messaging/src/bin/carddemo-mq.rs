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
//! The process runs until SIGTERM or SIGINT stops it, or `messaging::run` reports a
//! configuration error: a `CARDDEMO_AMQP_URL` that is not a valid AMQP URL, or a request queue
//! name the workers refuse, such as a `CARDDEMO_QUEUE_REQUEST_ACCT` over 48 bytes. A broker that
//! is down or still booting when the process starts, or one that lacks a queue, does not end
//! it: `messaging::run` logs one `ERROR` event per failed attempt and retries with its backoff
//! until the broker and its queues are there, so the process needs no supervisor to outlast a
//! broker outage. A stop is cooperative: each worker finishes the request it holds, reply,
//! acknowledgement and writes included, and takes no other; the three workers are joined, the
//! AMQP connection and the database pool are closed, one `INFO` event names the signal, and the
//! process exits 0 after the telemetry guard flushes. Every broker wait is bounded, so the stop
//! takes bounded time. A stop that arrives while the database connection is still being opened
//! abandons that attempt at once: no worker has started, the `INFO` event says so, and the
//! process exits 0. Only SIGKILL, or a crash, ends the process mid-request, with the outcome E29
//! accepts.
//!
//! The process exits non-zero on any failure it does not retry: a configuration or telemetry
//! error as one line on standard error, since no subscriber exists yet; a signal-handler or
//! database error, or the broker configuration error `messaging::run` returns, as one JSON
//! `ERROR` event on standard output. A connection failure names the variable that holds the URL,
//! `DATABASE_URL` or `CARDDEMO_AMQP_URL`; no message names the connection URLs or any value they
//! carry.

use std::io;
use std::process::ExitCode;
use std::sync::Arc;

use messaging::StopHandle;
use runtime::config::Config;
use tokio::signal::unix::{SignalKind, signal};

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
    // Installed and watched before any connection, so that from here on SIGTERM and SIGINT
    // request the cooperative stop instead of ending the process.
    let signals = match stop_signals() {
        Ok(signals) => signals,
        Err(error) => {
            let kind = error.kind();
            tracing::error!(?kind, "the SIGTERM and SIGINT handlers cannot be installed");
            return ExitCode::FAILURE;
        }
    };
    let handle = StopHandle::new();
    let mut stop = handle.subscribe();
    let signal_watch = tokio::spawn(async move {
        let signal = signals.await;
        handle.request();
        signal
    });
    // Nothing is processed yet, so a stop abandons the connection attempt.
    let Some(connected) = stop.unless_requested(runtime::db::connect(&config)).await else {
        let signal = signal_watch.await.unwrap_or("a stop signal");
        tracing::info!(
            signal,
            "carddemo-mq stopped while connecting to the database: no worker was started"
        );
        return ExitCode::SUCCESS;
    };
    let pool = match connected {
        Ok(pool) => pool,
        Err(error) => {
            signal_watch.abort();
            // Logged only as the value-free summary: the driver's own text can quote the
            // connection URL, password included.
            let error = runtime::db::ErrorSummary::from(&error);
            tracing::error!(%error, "database connection failed (DATABASE_URL)");
            return ExitCode::FAILURE;
        }
    };
    // The store takes a copy of the pool's handle; this one closes the pool after `run`.
    let store = persistence::PgStore::new(pool.clone());
    let outcome = messaging::run(&config, store, Arc::new(domain::SystemClock), stop).await;
    // Every store copy went with `run`, so no connection is checked out and the close is prompt.
    pool.close().await;
    match outcome {
        Ok(()) => {
            let signal = signal_watch.await.unwrap_or("a stop signal");
            tracing::info!(
                signal,
                "carddemo-mq stopped: the workers finished their requests and the AMQP \
                 connection and the database pool are closed"
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            signal_watch.abort();
            tracing::error!(%error, "carddemo-mq stopped");
            ExitCode::FAILURE
        }
    }
}

/// Installs the SIGTERM and SIGINT handlers and returns the wait for the first of the two, which
/// yields the signal's name.
///
/// # Errors
///
/// The error of a handler that cannot be installed.
fn stop_signals() -> io::Result<impl Future<Output = &'static str>> {
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => "SIGTERM",
            _ = interrupt.recv() => "SIGINT",
        }
    })
}
