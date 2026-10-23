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
//! 2. refuses to start when `CARDDEMO_JWT_SECRET` is empty or unset, before anything else is set
//!    up, so that neither an invalid `RUST_LOG` nor the state of the database can hide that
//!    diagnostic;
//! 3. installs the JSON tracing subscriber on standard output, with the optional span export of
//!    `CARDDEMO_OTEL_EXPORT`;
//! 4. refuses to start when `CARDDEMO_INTRDR_DIR`, the job-deck directory of endpoint 14, names
//!    something that is not a directory or a path through one, before any connection; a missing
//!    directory is no refusal, as the first submission creates it;
//! 5. builds the multi-threaded async runtime;
//! 6. opens the `PostgreSQL` request pool on `DATABASE_URL` (`runtime::db::connect_for_requests`),
//!    whose sessions keep the server's lock and statement settings, so a row lock another
//!    transaction holds makes a request wait for it, as a CICS `READ UPDATE` waits;
//! 7. builds the application state, with the system clock, over a store that fails a request's
//!    statements at once while the pool's refusal watch finds the server refusing connections;
//! 8. listens on `CARDDEMO_API_BIND` and serves until SIGTERM or SIGINT.
//!
//! The server never changes the schema: only the schema executor binary of the `runtime` crate
//! applies it (Single Schema Executor). Against a database without the schema, the server starts
//! and each operation fails at its first query. It reaches no broker, because no online
//! transaction sends a message.
//!
//! A stop signal ends the accept loop, and the requests in flight get `DRAIN_DEADLINE` (8 s) to
//! finish. When they all do, the pool is closed and the process exits 0 after the telemetry
//! guard flushes. When the deadline passes first, the requests still in flight are abandoned:
//! one `ERROR` event says so and the process exits 1, so that a supervisor can tell that stop
//! from a clean one. The pool close and the runtime shutdown that follow are bounded as well
//! (`POOL_CLOSE_DEADLINE`, `RUNTIME_SHUTDOWN_DEADLINE`), so a connection or a task that an
//! abandoned request still holds cannot keep the process alive: it ends at most about 9 s after
//! the signal.
//!
//! Any startup failure exits non-zero: a configuration or telemetry error as one line on standard
//! error, since no subscriber exists yet, and every later failure as one JSON `ERROR` event on
//! standard output. No message carries the JWT secret, the database URL or any value it holds,
//! the job-deck directory's path, or the database driver's own text.
//!
//! The file uses no `tokio` or `tracing` macro: the runtime is built explicitly, the stop signals
//! are polled by hand, and events go through `emit`, whose documentation says why.

use std::fmt;
use std::future::poll_fn;
use std::io;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use axum::Router;
use domain::clock::SystemClock;
use persistence::PgStore;
use runtime::config::Config;
use runtime::db::ErrorSummary;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{self, FieldSet, Value};
use tracing::level_filters::{LevelFilter, STATIC_MAX_LEVEL};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

/// How long the requests in flight at a stop signal get to finish before they are abandoned.
///
/// With the two bounds below it keeps a stop under the 10 s after which `scripts/e2e.sh` kills a
/// server that has not ended on SIGTERM.
const DRAIN_DEADLINE: Duration = Duration::from_secs(8);

/// How long closing the pool may take after serving. A connection that an abandoned request
/// still holds is never returned, and would otherwise hold the close for good.
const POOL_CLOSE_DEADLINE: Duration = Duration::from_millis(500);

/// How long the runtime may take to shut down after serving: the tasks of abandoned requests are
/// dropped, and a blocking task still running after it is left to the process exit.
const RUNTIME_SHUTDOWN_DEADLINE: Duration = Duration::from_millis(500);

/// Target of every event of this binary: its crate name, the module path a `tracing` macro would
/// have given the events.
const TARGET: &str = "carddemo_api";

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("carddemo-api: configuration error: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Checked before the subscriber is installed, so a missing secret is reported as such
    // whatever `RUST_LOG` or the database holds. The message names the variable, never a value.
    if let Err(error) = config.require_jwt_secret() {
        eprintln!("carddemo-api: configuration error: {error}");
        return ExitCode::FAILURE;
    }
    let telemetry = match runtime::telemetry::init(config.otel_export) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("carddemo-api: telemetry initialisation failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Before the runtime and any connection: a job-deck directory that can never hold a deck is
    // a configuration error. The message names the variable and the error kind, never the path.
    if let Err(error) = api::app::check_intrdr_dir(&config) {
        cannot_start(&error);
        return ExitCode::FAILURE;
    }
    let status = run(&config);
    // Dropped last, once the runtime is shut down and every event emitted, so the span exporter
    // flushes everything the process recorded.
    drop(telemetry);
    status
}

/// Builds the async runtime, serves on it until a stop, and shuts it down within
/// `RUNTIME_SHUTDOWN_DEADLINE`.
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
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_DEADLINE);
    status
}

/// Connects, listens and serves until a stop signal, then closes the pool.
async fn serve(config: &Config) -> ExitCode {
    // The request pool: a request blocked on a row another transaction holds waits until that
    // transaction ends, then reads the row it left (COACTUPC-101, COBIL00C-013). A stop stays
    // bounded all the same, by `DRAIN_DEADLINE` and `POOL_CLOSE_DEADLINE`.
    let (pool, refusals) = match runtime::db::connect_for_requests(config).await {
        Ok(opened) => opened,
        Err(error) => {
            // Logged only as the value-free summary: the driver's own text can quote the
            // connection URL, password included.
            database_failed(&ErrorSummary::from(&error));
            return ExitCode::FAILURE;
        }
    };
    // The store takes a copy of the pool's handle; this one closes the pool after serving,
    // which also ends the pool's refusal watch. While the watch finds the server refusing
    // connections, the store fails every request's statements at once, as the database
    // unavailable, instead of after the pool's acquire timeout.
    let store = PgStore::new(pool.clone())
        .failing_fast_while(move || refusals.server_refusing())
        .await;
    let state = match api::app::AppState::from_config(store, config, Arc::new(SystemClock)) {
        Ok(state) => state,
        Err(error) => {
            cannot_start(&error);
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind(config.api_bind.as_str()).await {
        Ok(listener) => listener,
        Err(error) => {
            bind_failed(error.kind(), &config.api_bind);
            return ExitCode::FAILURE;
        }
    };
    listening(listener.local_addr().ok());

    let served = serve_until_stopped(
        listener,
        api::app::app(state),
        shutdown_signal(),
        DRAIN_DEADLINE,
    )
    .await;
    // After a drain within the deadline every store copy went with the router, so no connection
    // is checked out and the close is prompt. After an abandoned drain, the connection tasks of
    // the abandoned requests may still hold one until the runtime drops them.
    if tokio::time::timeout(POOL_CLOSE_DEADLINE, pool.close())
        .await
        .is_err()
    {
        pool_close_late(POOL_CLOSE_DEADLINE);
    }
    match served {
        Served::Returned(Ok(())) => {
            stopped();
            ExitCode::SUCCESS
        }
        Served::Returned(Err(error)) => {
            stopped_serving(error.kind());
            ExitCode::FAILURE
        }
        Served::Abandoned => ExitCode::FAILURE,
    }
}

/// How [`serve_until_stopped`] ended.
enum Served {
    /// The server returned by itself: after a drain within the deadline, or with an error.
    Returned(io::Result<()>),
    /// The drain deadline passed with requests still in flight, which were abandoned.
    Abandoned,
}

/// Serves `app` on `listener` until `stop` resolves, then lets the requests in flight finish
/// within `drain_deadline`.
///
/// Until `stop` resolves, the server is polled first, so it returning by itself, with an error,
/// ends the wait at once. Once `stop` resolves, the server accepts no connection and each
/// connection closes when its request in flight has been answered. When `drain_deadline` passes
/// first, the server is dropped and the abandonment emitted as one `ERROR` event. Both futures
/// are pinned on the heap with `Box::pin`, the pin that needs no pinning macro.
async fn serve_until_stopped(
    listener: TcpListener,
    app: Router,
    stop: impl Future<Output = ()>,
    drain_deadline: Duration,
) -> Served {
    let (stop_server, server_stopped) = oneshot::channel::<()>();
    let mut serving = Box::pin(
        axum::serve(listener, app)
            // The receiver resolves when its sender is dropped, which is how the stop is sent.
            .with_graceful_shutdown(async move { server_stopped.await.unwrap_or(()) })
            .into_future(),
    );
    let mut stop = Box::pin(stop);
    let returned = poll_fn(|cx| match serving.as_mut().poll(cx) {
        Poll::Ready(result) => Poll::Ready(Some(result)),
        Poll::Pending => stop.as_mut().poll(cx).map(|()| None),
    })
    .await;
    if let Some(result) = returned {
        return Served::Returned(result);
    }
    drop(stop_server);
    match tokio::time::timeout(drain_deadline, serving).await {
        Ok(result) => Served::Returned(result),
        Err(_elapsed) => {
            drain_abandoned(drain_deadline);
            Served::Abandoned
        }
    }
}

/// Resolves at the first SIGTERM or SIGINT, after emitting which of the two arrived.
///
/// Both signal streams are polled in turn in one hand-written future, which holds them by
/// mutable reference and so needs no pinning. When a handler cannot be installed, it warns and
/// never resolves, so the server keeps serving rather than stopping at once.
async fn shutdown_signal() {
    match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(mut terminate), Ok(mut interrupt)) => {
            // A stream that reports no more signals counts as having fired, so the server never
            // waits on a signal that cannot arrive.
            let signal = poll_fn(|cx| {
                if terminate.poll_recv(cx).is_ready() {
                    Poll::Ready("SIGTERM")
                } else if interrupt.poll_recv(cx).is_ready() {
                    Poll::Ready("SIGINT")
                } else {
                    Poll::Pending
                }
            })
            .await;
            stopping(signal);
        }
        (Err(error), _) | (_, Err(error)) => {
            no_signal_handlers(error.kind());
            std::future::pending::<()>().await;
        }
    }
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
/// The target and module path are [`TARGET`]. No source file or line is given: the JSON output
/// and the span export print neither.
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

/// Emits one event of `site` with `values`, one per field the site names and in that order, a
/// `None` leaving its field out, when the current subscriber enables it.
///
/// Every `tracing` event macro expands to an attribute that allows `unused_imports`, and this
/// workspace admits no lint suppression, generated ones included. So this binary builds its
/// events from `tracing`'s callsite API and takes the steps an event macro takes: the
/// compile-time and the current maximum level first, then the callsite's cached interest, which
/// the first call registers, and, when the subscriber is only sometimes interested, its own
/// decision on the metadata. `RUST_LOG` filtering, the JSON layer and the span export therefore
/// treat these events exactly as they treat a macro's.
fn emit<const N: usize>(site: &'static EventSite<N>, values: [Option<&dyn Value>; N]) {
    let callsite = &site.0;
    let metadata = callsite.metadata();
    let level = *metadata.level();
    if level > STATIC_MAX_LEVEL || level > LevelFilter::current() {
        return;
    }
    let interest = callsite.interest();
    if interest.is_never() {
        return;
    }
    if interest.is_always()
        || tracing::dispatcher::get_default(|dispatch| dispatch.enabled(metadata))
    {
        Event::dispatch(metadata, &metadata.fields().value_set_all(&values));
    }
}

/// `ERROR` when a configuration value refuses the start, with its `error`: the job-deck
/// directory, or a setting the application state cannot be built from.
fn cannot_start(error: &dyn fmt::Display) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("cannot_start", Level::ERROR, &["message", "error"], &SITE);
    emit(
        &SITE,
        [
            Some(&"carddemo-api cannot start"),
            Some(&field::display(error)),
        ],
    );
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
            Some(&field::debug(kind)),
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

/// `ERROR` when the server cannot listen on `bind`, with the I/O error `kind`.
fn bind_failed(kind: io::ErrorKind, bind: &str) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "bind_failed",
        Level::ERROR,
        &["message", "kind", "bind"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"cannot listen on CARDDEMO_API_BIND"),
            Some(&field::debug(kind)),
            Some(&field::display(bind)),
        ],
    );
}

/// `INFO` once the server listens, with the bound `address` when the listener reports one.
fn listening(address: Option<SocketAddr>) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("listening", Level::INFO, &["message", "address"], &SITE);
    // An `Option` records nothing when it is `None`, which leaves the field out of the event.
    let address = address.map(field::display);
    emit(&SITE, [Some(&"carddemo-api listening"), Some(&address)]);
}

/// `INFO` at the stop `signal`, `SIGTERM` or `SIGINT`.
fn stopping(signal: &'static str) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("stopping", Level::INFO, &["message", "signal"], &SITE);
    emit(
        &SITE,
        [
            Some(&"carddemo-api stopping: finishing the requests in flight"),
            Some(&signal),
        ],
    );
}

/// `WARN` when the stop signal handlers cannot be installed, with the I/O error `kind`.
fn no_signal_handlers(kind: io::ErrorKind) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "no_signal_handlers",
        Level::WARN,
        &["message", "kind"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(
                &"the SIGTERM and SIGINT handlers cannot be installed: carddemo-api serves \
                  without a graceful stop",
            ),
            Some(&field::debug(kind)),
        ],
    );
}

/// `ERROR` when requests are still in flight at the end of the drain `deadline`.
fn drain_abandoned(deadline: Duration) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "drain_abandoned",
        Level::ERROR,
        &["message", "deadline"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(
                &"carddemo-api stopped: the drain deadline passed and the requests still in \
                  flight were abandoned",
            ),
            Some(&field::debug(deadline)),
        ],
    );
}

/// `WARN` when the pool has not closed within `deadline`.
fn pool_close_late(deadline: Duration) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "pool_close_late",
        Level::WARN,
        &["message", "deadline"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(
                &"the database pool did not close within its deadline: its connections close \
                  with the process",
            ),
            Some(&field::debug(deadline)),
        ],
    );
}

/// `INFO` after a stop in which every request in flight finished.
fn stopped() {
    static SITE: EventSite<1> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("stopped", Level::INFO, &["message"], &SITE);
    emit(&SITE, [Some(&"carddemo-api stopped")]);
}

/// `ERROR` when the server returned with an I/O error of `kind`.
fn stopped_serving(kind: io::ErrorKind) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> =
        event_metadata("stopped_serving", Level::ERROR, &["message", "kind"], &SITE);
    emit(
        &SITE,
        [
            Some(&"carddemo-api stopped serving"),
            Some(&field::debug(kind)),
        ],
    );
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use axum::Router;
    use axum::routing::get;
    use runtime::config::{Config, EnvError};
    use runtime::db::ErrorSummary;
    use test_support::trace_capture;
    use tokio::net::TcpListener;
    use tokio::runtime::Runtime;
    use tokio::sync::Notify;
    use tracing::span;
    use tracing::subscriber::{Interest, Subscriber};
    use tracing::{Event, Level, Metadata};

    use super::{
        DRAIN_DEADLINE, POOL_CLOSE_DEADLINE, Served, bind_failed, cannot_start, database_failed,
        drain_abandoned, listening, no_signal_handlers, pool_close_late, runtime_failed,
        serve_until_stopped, stopped, stopped_serving, stopping,
    };

    /// A `DATABASE_URL` that `runtime::db::connect_for_requests` refuses before it connects, for a
    /// query parameter that `sqlx` does not recognize. It carries a password, which no event may
    /// show.
    const REFUSED_URL: &str =
        "postgres://carddemo:Pw-7Kq2-not-logged@127.0.0.1:1/carddemo?no_such_parameter=1";

    /// The address a listening event reports.
    const LISTEN_ADDRESS: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 20045);

    /// A current-thread runtime for the async parts of a test, built as `main` builds its own.
    fn current_thread() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime builds")
    }

    /// The summary `serve` logs when the pool cannot be opened on [`REFUSED_URL`].
    fn refused_url_summary() -> ErrorSummary {
        let config =
            Config::from_lookup(|name| (name == "DATABASE_URL").then(|| REFUSED_URL.to_owned()))
                .expect("the configuration parses");
        let error = current_thread()
            .block_on(runtime::db::connect_for_requests(&config))
            .expect_err("an unrecognized query parameter is refused");
        ErrorSummary::from(&error)
    }

    #[test]
    fn every_event_is_one_json_line_with_its_level_target_and_fields() {
        let summary = refused_url_summary().to_string();
        let summary = serde_json::to_string(&summary).expect("a string serializes");
        // The emitting call, its level, and the exact `fields` object of its line: `message`
        // first, a `None` field absent.
        let cases: [(fn(), &str, String); 13] = [
            (
                || cannot_start(&EnvError::MissingJwtSecret),
                "ERROR",
                r#"{"message":"carddemo-api cannot start","error":"CARDDEMO_JWT_SECRET is empty or unset; the API refuses to start without it"}"#.to_owned(),
            ),
            (
                || runtime_failed(io::ErrorKind::OutOfMemory),
                "ERROR",
                r#"{"message":"cannot build the async runtime","kind":"OutOfMemory"}"#.to_owned(),
            ),
            (
                || database_failed(&refused_url_summary()),
                "ERROR",
                format!(
                    r#"{{"message":"database connection failed (DATABASE_URL)","error":{summary}}}"#
                ),
            ),
            (
                || bind_failed(io::ErrorKind::AddrInUse, "127.0.0.1:20045"),
                "ERROR",
                r#"{"message":"cannot listen on CARDDEMO_API_BIND","kind":"AddrInUse","bind":"127.0.0.1:20045"}"#.to_owned(),
            ),
            (
                || listening(Some(LISTEN_ADDRESS)),
                "INFO",
                r#"{"message":"carddemo-api listening","address":"127.0.0.1:20045"}"#.to_owned(),
            ),
            (
                || listening(None),
                "INFO",
                r#"{"message":"carddemo-api listening"}"#.to_owned(),
            ),
            (
                || stopping("SIGTERM"),
                "INFO",
                r#"{"message":"carddemo-api stopping: finishing the requests in flight","signal":"SIGTERM"}"#.to_owned(),
            ),
            (
                || stopping("SIGINT"),
                "INFO",
                r#"{"message":"carddemo-api stopping: finishing the requests in flight","signal":"SIGINT"}"#.to_owned(),
            ),
            (
                || no_signal_handlers(io::ErrorKind::Unsupported),
                "WARN",
                r#"{"message":"the SIGTERM and SIGINT handlers cannot be installed: carddemo-api serves without a graceful stop","kind":"Unsupported"}"#.to_owned(),
            ),
            (
                || drain_abandoned(DRAIN_DEADLINE),
                "ERROR",
                r#"{"message":"carddemo-api stopped: the drain deadline passed and the requests still in flight were abandoned","deadline":"8s"}"#.to_owned(),
            ),
            (
                || pool_close_late(POOL_CLOSE_DEADLINE),
                "WARN",
                r#"{"message":"the database pool did not close within its deadline: its connections close with the process","deadline":"500ms"}"#.to_owned(),
            ),
            (
                stopped,
                "INFO",
                r#"{"message":"carddemo-api stopped"}"#.to_owned(),
            ),
            (
                || stopped_serving(io::ErrorKind::ConnectionReset),
                "ERROR",
                r#"{"message":"carddemo-api stopped serving","kind":"ConnectionReset"}"#.to_owned(),
            ),
        ];
        for (emit_event, level, fields) in &cases {
            let capture = trace_capture::capture();
            emit_event();
            let lines = capture.lines();
            drop(capture);
            let [line] = lines.as_slice() else {
                panic!("{fields}: expected exactly one line, got {lines:?}");
            };
            let event: serde_json::Value = serde_json::from_str(line).expect("each line is JSON");
            assert_eq!(event["level"], *level, "{line}");
            assert_eq!(event["target"], "carddemo_api", "{line}");
            assert!(line.contains(&format!(r#""fields":{fields}"#)), "{line}");
            assert!(!line.contains("Pw-7Kq2-not-logged"), "{line}");
        }
    }

    /// A subscriber that enables the events at its `ceiling` or more severe ones and records the
    /// level of each event it receives. It answers "sometimes" for every callsite, so the
    /// emitter has to ask it about each event, whatever other tests' subscribers cache.
    struct Ceiling {
        ceiling: Level,
        received: Mutex<Vec<Level>>,
    }

    impl Subscriber for Ceiling {
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::sometimes()
        }

        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() <= self.ceiling
        }

        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            panic!("the emitter opens no span");
        }

        fn record(&self, _: &span::Id, _: &span::Record<'_>) {
            panic!("the emitter records no span field");
        }

        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {
            panic!("the emitter links no span");
        }

        fn event(&self, event: &Event<'_>) {
            self.received
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(*event.metadata().level());
        }

        fn enter(&self, _: &span::Id) {
            panic!("the emitter enters no span");
        }

        fn exit(&self, _: &span::Id) {
            panic!("the emitter exits no span");
        }
    }

    #[test]
    fn an_event_less_severe_than_the_subscriber_enables_is_not_emitted() {
        // The subscriber's ceiling, and the levels it receives of one `INFO`, one `WARN` and one
        // `ERROR` event.
        let cases = [
            (Level::TRACE, vec![Level::INFO, Level::WARN, Level::ERROR]),
            (Level::INFO, vec![Level::INFO, Level::WARN, Level::ERROR]),
            (Level::WARN, vec![Level::WARN, Level::ERROR]),
            (Level::ERROR, vec![Level::ERROR]),
        ];
        for (ceiling, expected) in cases {
            let subscriber = Arc::new(Ceiling {
                ceiling,
                received: Mutex::default(),
            });
            tracing::subscriber::with_default(Arc::clone(&subscriber), || {
                stopped();
                no_signal_handlers(io::ErrorKind::Unsupported);
                stopped_serving(io::ErrorKind::ConnectionReset);
            });
            let received = subscriber
                .received
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            assert_eq!(*received, expected, "ceiling {ceiling}");
        }
    }

    #[test]
    fn a_stop_waits_for_a_request_in_flight_only_until_the_drain_deadline() {
        let drain_deadline = Duration::from_millis(400);
        // How long the request in flight at the stop takes to answer, and whether the drain
        // waits for its answer.
        let cases = [
            (Duration::from_millis(50), true),
            (Duration::from_secs(3600), false),
        ];
        for (answer_after, answered) in cases {
            current_thread().block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("an ephemeral port");
                let address = listener.local_addr().expect("a bound address");
                let in_flight = Arc::new(Notify::new());
                let arrived = Arc::clone(&in_flight);
                let app = Router::new().route(
                    "/slow",
                    get(move || {
                        let arrived = Arc::clone(&arrived);
                        async move {
                            arrived.notify_one();
                            tokio::time::sleep(answer_after).await;
                            "answered"
                        }
                    }),
                );
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .expect("a client");
                let request = tokio::spawn(client.get(format!("http://{address}/slow")).send());
                // The stop arrives while the request is in flight.
                let stop = async move { in_flight.notified().await };

                match (
                    serve_until_stopped(listener, app, stop, drain_deadline).await,
                    answered,
                ) {
                    (Served::Returned(Ok(())), true) => {
                        let response = request
                            .await
                            .expect("the request task completes")
                            .expect("the request is answered");
                        assert_eq!(
                            response.text().await.expect("a body"),
                            "answered",
                            "{answer_after:?}"
                        );
                    }
                    (Served::Abandoned, false) => {
                        assert!(!request.is_finished(), "{answer_after:?}");
                    }
                    (Served::Returned(result), _) => {
                        panic!("{answer_after:?}: the server returned {result:?}");
                    }
                    (Served::Abandoned, true) => {
                        panic!("{answer_after:?}: the drain abandoned a request it had time for");
                    }
                }
            });
        }
    }
}
