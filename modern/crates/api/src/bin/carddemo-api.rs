//! `carddemo-api`: the HTTP server of the `CardDemo` REST API, in place of the CICS region that
//! served the online transactions from the CC00 sign-on (`COSGN00C`) onward.
//!
//! This file wires process plumbing only. The routes, the `OpenAPI` document at
//! `/api-docs/openapi.json`, the per-request spans and the state are `api::app`; the server adds
//! no route, middleware or request logging of its own.
//!
//! Startup, in order:
//! 1. reads the configuration through `runtime::config::Config`, the only reader of the
//!    environment;
//! 2. installs the JSON tracing subscriber on standard output, with the optional span export of
//!    `CARDDEMO_OTEL_EXPORT`;
//! 3. refuses to start when `CARDDEMO_JWT_SECRET` is empty or unset, before any connection;
//! 4. opens the `PostgreSQL` pool on `DATABASE_URL`;
//! 5. listens on `CARDDEMO_API_BIND` and serves until SIGTERM or SIGINT.
//!
//! The server never changes the schema: only the schema executor binary of the `runtime` crate
//! applies it (Single Schema Executor). Against a database without the schema, the server starts
//! and each operation fails at its first query. It reaches no broker, because no online
//! transaction sends a message.
//!
//! A stop signal ends the accept loop: the requests in flight finish, the pool is closed, and the
//! process exits 0 after the telemetry guard flushes. Any startup failure exits non-zero: a
//! configuration or telemetry error as one line on standard error, since no subscriber exists
//! yet, and every later failure as one JSON `ERROR` event on standard output. No message carries
//! the JWT secret, the database URL or any value it holds, or the database driver's own text.

use std::process::ExitCode;
use std::sync::Arc;

use domain::clock::SystemClock;
use persistence::PgStore;
use runtime::config::Config;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("carddemo-api: configuration error: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Bound until `main` returns: dropping the guard flushes the exported spans after the last
    // event.
    let _telemetry = match runtime::telemetry::init(config.otel_export) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("carddemo-api: telemetry initialisation failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Checked before connecting, so a missing secret is reported as such whatever the state of
    // the database. The message names the variable, never a value.
    if let Err(error) = config.require_jwt_secret() {
        tracing::error!(%error, "carddemo-api cannot start");
        return ExitCode::FAILURE;
    }
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
    // The store takes a copy of the pool's handle; this one closes the pool after serving.
    let store = PgStore::new(pool.clone());
    let state = match api::app::AppState::from_config(store, &config, Arc::new(SystemClock)) {
        Ok(state) => state,
        Err(error) => {
            tracing::error!(%error, "carddemo-api cannot start");
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind(config.api_bind.as_str()).await {
        Ok(listener) => listener,
        Err(error) => {
            let kind = error.kind();
            tracing::error!(?kind, bind = %config.api_bind, "cannot listen on CARDDEMO_API_BIND");
            return ExitCode::FAILURE;
        }
    };
    // A field that is `None` is left out of the event.
    let address = listener.local_addr().ok().map(tracing::field::display);
    tracing::info!(address, "carddemo-api listening");

    let served = axum::serve(listener, api::app::app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await;
    // Every store copy went with the router, so no connection is checked out and the close is
    // prompt.
    pool.close().await;
    match served {
        Ok(()) => {
            tracing::info!("carddemo-api stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let kind = error.kind();
            tracing::error!(?kind, "carddemo-api stopped serving");
            ExitCode::FAILURE
        }
    }
}

/// Resolves at the first SIGTERM or SIGINT, after which `axum::serve` accepts no connection and
/// lets the requests in flight finish.
///
/// When a handler cannot be installed, it warns and never resolves, so the server keeps serving
/// rather than stopping at once.
async fn shutdown_signal() {
    match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(mut terminate), Ok(mut interrupt)) => {
            let signal = tokio::select! {
                _ = terminate.recv() => "SIGTERM",
                _ = interrupt.recv() => "SIGINT",
            };
            tracing::info!(
                signal,
                "carddemo-api stopping: finishing the requests in flight"
            );
        }
        (Err(error), _) | (_, Err(error)) => {
            let kind = error.kind();
            tracing::warn!(
                ?kind,
                "the SIGTERM and SIGINT handlers cannot be installed: carddemo-api serves \
                 without a graceful stop"
            );
            std::future::pending::<()>().await;
        }
    }
}
