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
//! error as one line on standard error, since no subscriber exists yet; an async runtime that
//! cannot be built, a signal-handler or database error, or the broker configuration error
//! `messaging::run` returns, as one JSON `ERROR` event on standard output. A connection failure
//! names the variable that holds the URL, `DATABASE_URL` or `CARDDEMO_AMQP_URL`; no message names
//! the connection URLs or any value they carry.
//!
//! The file uses no `tokio` or `tracing` macro: the runtime is built explicitly, the stop signals
//! are polled by hand, and events go through `emit`, whose documentation says why.

use std::future::poll_fn;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use std::task::Poll;

use messaging::{StartupError, StopHandle};
use runtime::config::Config;
use runtime::db::ErrorSummary;
use tokio::signal::unix::{SignalKind, signal};
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{self, FieldSet, Value};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

/// Target and module path of every event of this binary: the module path of its crate root, the
/// target a `tracing` event macro would have given the events.
const TARGET: &str = module_path!();

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("carddemo-mq: configuration error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let telemetry = match runtime::telemetry::init(config.otel_export) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("carddemo-mq: telemetry initialisation failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = run(&config);
    // Dropped last, once the runtime is gone and every event emitted, so the span exporter
    // flushes everything the process recorded.
    drop(telemetry);
    status
}

/// Builds the multi-threaded async runtime with every driver enabled, runs [`serve`] on it, and
/// drops it, which waits for the runtime's threads to end.
fn run(config: &Config) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            runtime_failed(error.kind());
            return ExitCode::FAILURE;
        }
    };
    let status = runtime.block_on(serve(config));
    drop(runtime);
    status
}

/// Installs the stop signals, connects the database pool, runs the workers with
/// `messaging::run` until a stop or a configuration error, then closes the pool.
async fn serve(config: &Config) -> ExitCode {
    // Installed and watched before any connection, so that from here on SIGTERM and SIGINT
    // request the cooperative stop instead of ending the process.
    let signals = match stop_signals() {
        Ok(signals) => signals,
        Err(error) => {
            no_signal_handlers(error.kind());
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
    let Some(connected) = stop.unless_requested(runtime::db::connect(config)).await else {
        stopped_while_connecting(signal_watch.await.unwrap_or("a stop signal"));
        return ExitCode::SUCCESS;
    };
    let pool = match connected {
        Ok(pool) => pool,
        Err(error) => {
            signal_watch.abort();
            // Logged only as the value-free summary: the driver's own text can quote the
            // connection URL, password included.
            database_failed(&ErrorSummary::from(&error));
            return ExitCode::FAILURE;
        }
    };
    // The store takes a copy of the pool's handle; this one closes the pool after `run`.
    let store = persistence::PgStore::new(pool.clone());
    let outcome = messaging::run(config, store, Arc::new(domain::SystemClock), stop).await;
    // Every store copy went with `run`, so no connection is checked out and the close is prompt.
    pool.close().await;
    match outcome {
        Ok(()) => {
            stopped(signal_watch.await.unwrap_or("a stop signal"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            signal_watch.abort();
            stopped_on_error(&error);
            ExitCode::FAILURE
        }
    }
}

/// Installs the SIGTERM and SIGINT handlers and returns the wait for the first of the two, which
/// yields the signal's name.
///
/// Both signal streams are polled in turn, SIGTERM first, in one hand-written future that owns
/// them and so needs no pinning. A stream that reports no more signals counts as having fired,
/// so the wait never outlives a signal that can no longer arrive.
///
/// # Errors
///
/// The error of a handler that cannot be installed.
fn stop_signals() -> io::Result<impl Future<Output = &'static str>> {
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(poll_fn(move |cx| {
        if terminate.poll_recv(cx).is_ready() {
            Poll::Ready("SIGTERM")
        } else if interrupt.poll_recv(cx).is_ready() {
            Poll::Ready("SIGINT")
        } else {
            Poll::Pending
        }
    }))
}

/// The callsite of one event of this binary, whose metadata names `N` fields, `message` first.
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
/// The target and the module path are [`TARGET`]. No source file or line is given: the JSON
/// output and the span export print neither.
const fn event_metadata<const N: usize>(
    name: &'static str,
    level: Level,
    names: &'static [&'static str; N],
    site: &'static EventSite<N>,
) -> Metadata<'static> {
    Metadata::new(
        name,
        TARGET,
        level,
        None,
        None,
        Some(TARGET),
        FieldSet::new(names, Identifier(&site.0)),
        Kind::EVENT,
    )
}

/// Emits one event of `site` in the current span with `values`, one per field the site names
/// and in that order, a `None` leaving its field out, when the current subscriber enables it.
///
/// Every `tracing` event macro expands to an attribute that allows `unused_imports`, and no
/// source of this workspace, generated source included, holds a lint suppression. So this
/// binary builds its events from `tracing`'s callsite API, and the decision is
/// `runtime::telemetry::event_enabled`, which takes the steps an event macro takes: `RUST_LOG`
/// filtering, the JSON layer and the span export therefore treat these events exactly as they
/// treat a macro's.
fn emit<const N: usize>(site: &'static EventSite<N>, values: [Option<&dyn Value>; N]) {
    let callsite = &site.0;
    if !runtime::telemetry::event_enabled(callsite) {
        return;
    }
    let metadata = callsite.metadata();
    Event::dispatch(metadata, &metadata.fields().value_set_all(&values));
}

/// `ERROR` when the async runtime cannot be built, with the I/O error `kind`.
fn runtime_failed(kind: io::ErrorKind) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("runtime_failed", Level::ERROR, &["message", "kind"], &SITE);
    emit(
        &SITE,
        [
            Some(&"cannot build the async runtime"),
            Some(&field::debug(&kind)),
        ],
    );
}

/// `ERROR` when the stop signal handlers cannot be installed, with the I/O error `kind`.
fn no_signal_handlers(kind: io::ErrorKind) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "no_signal_handlers",
        Level::ERROR,
        &["message", "kind"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the SIGTERM and SIGINT handlers cannot be installed"),
            Some(&field::debug(&kind)),
        ],
    );
}

/// `INFO` when the stop `signal` came while the database connection was being opened.
fn stopped_while_connecting(signal: &'static str) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "stopped_while_connecting",
        Level::INFO,
        &["message", "signal"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"carddemo-mq stopped while connecting to the database: no worker was started"),
            Some(&signal),
        ],
    );
}

/// `ERROR` when the pool cannot be opened, with the value-free summary `error`.
fn database_failed(error: &ErrorSummary) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "database_failed",
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"database connection failed (DATABASE_URL)"),
            Some(&field::display(error)),
        ],
    );
}

/// `INFO` after a stop at `signal` in which every worker finished its request.
fn stopped(signal: &'static str) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("stopped", Level::INFO, &["message", "signal"], &SITE);
    emit(
        &SITE,
        [
            Some(
                &"carddemo-mq stopped: the workers finished their requests and the AMQP \
                  connection and the database pool are closed",
            ),
            Some(&signal),
        ],
    );
}

/// `ERROR` when `messaging::run` returned the configuration `error`.
fn stopped_on_error(error: &StartupError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "stopped_on_error",
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [Some(&"carddemo-mq stopped"), Some(&field::display(error))],
    );
}
