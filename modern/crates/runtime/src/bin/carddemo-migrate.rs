//! Applies the embedded `CardDemo` migrations to the `PostgreSQL` database named by
//! `DATABASE_URL`, through the embedded migrator in `runtime::migrate`.
//!
//! This binary is the only operator entrypoint for schema changes: the API, worker and batch
//! binaries never migrate, and the `sqlx` command-line tool only prepares the query cache. A
//! successful run logs one INFO event, `migrations applied`, whose `applied` field counts the
//! migrations this run applied and whose `latest_version` field names the latest version the
//! database records. A run against an already migrated database applies nothing, logs `applied`
//! 0, and still succeeds. The `NOTICE` that `PostgreSQL` sends when the migrator ensures its
//! existing bookkeeping table is not written, because the telemetry dependency filter holds back
//! every server notice below `WARN`. The exit status is non-zero when the configuration, the async
//! runtime, the connection or a migration fails. A configuration error names its variable and
//! never its value. A connection error names `DATABASE_URL`. A connection or migration error is
//! logged only as the value-free summary that `runtime::db` or `runtime::migrate` builds: an error
//! class with at most migration versions and a SQLSTATE, never the driver's or the server's text.
//! Neither the connection URL nor any value it carries reaches the output.
//!
//! Every event has target `carddemo_migrate`. The events are written without `tracing`'s event
//! macros, and the binary builds its async runtime itself rather than through `#[tokio::main]`,
//! because the expansion of each carries a lint suppression.

use std::fmt;
use std::io;
use std::process::ExitCode;

use runtime::config::{Config, OtelExport};
use runtime::telemetry::{event_enabled, with_value_set};
use tracing::Instrument;
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{self, FieldSet, Value};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

/// The target and module path of every event of this binary, `carddemo_migrate`: the target a
/// `tracing` macro here would give, so `RUST_LOG` directives select the events by it.
const TARGET: &str = module_path!();

fn main() -> ExitCode {
    let config = Config::from_env();
    let otel_export = config.as_ref().map_or(OtelExport::Off, |c| c.otel_export);
    let telemetry = match runtime::telemetry::init(otel_export) {
        Ok(guard) => guard,
        Err(err) => {
            // No subscriber exists yet, so standard error is the only channel left.
            eprintln!("carddemo-migrate: {err}");
            return ExitCode::FAILURE;
        }
    };
    let status = match config {
        Ok(config) => run(&config),
        Err(err) => {
            invalid_configuration(&err);
            ExitCode::FAILURE
        }
    };
    // Dropped last, once the async runtime is shut down and every event emitted: dropping the
    // guard flushes the exported spans.
    drop(telemetry);
    status
}

/// Builds the multi-threaded async runtime with every driver and applies the migrations on it,
/// inside the job span.
fn run(config: &Config) -> ExitCode {
    let async_runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(async_runtime) => async_runtime,
        Err(err) => {
            runtime_failed(err.kind());
            return ExitCode::FAILURE;
        }
    };
    async_runtime.block_on(
        async {
            let pool = match runtime::db::connect(config).await {
                Ok(pool) => pool,
                Err(err) => {
                    connection_failed(&runtime::db::ErrorSummary::from(&err));
                    return ExitCode::FAILURE;
                }
            };
            let outcome = runtime::migrate::apply(&pool).await;
            // Closing also ends the session that holds the migration lock after a failure.
            pool.close().await;
            match outcome {
                Ok(applied) => {
                    migrations_applied(applied.count(), applied.latest());
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    migration_failed(&runtime::migrate::ErrorSummary::from(&err));
                    ExitCode::FAILURE
                }
            }
        }
        .instrument(runtime::telemetry::job_span("carddemo-migrate")),
    )
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

/// Emits one event of `site` in the current span with `values`, one per field the site names
/// and in that order, paired with those fields by [`with_value_set`], a `None` leaving its field
/// out, when [`event_enabled`] says so.
fn emit<const N: usize>(site: &'static EventSite<N>, values: [Option<&dyn Value>; N]) {
    let callsite = &site.0;
    if !event_enabled(callsite) {
        return;
    }
    let metadata = callsite.metadata();
    with_value_set(metadata.fields(), values, |values| {
        Event::dispatch(metadata, values);
    });
}

/// `ERROR` when the configuration is refused, with its `error`, which names the variable and
/// never its value.
fn invalid_configuration(error: &dyn fmt::Display) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "invalid_configuration",
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [Some(&"invalid configuration"), Some(&field::display(error))],
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
fn connection_failed(error: &runtime::db::ErrorSummary) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "connection_failed",
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

/// `INFO` once the migrations are applied, with the count of those this run `applied` and the
/// `latest_version` the database records, left out when it records none.
fn migrations_applied(applied: usize, latest_version: Option<i64>) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "migrations_applied",
        Level::INFO,
        &["message", "applied", "latest_version"],
        &SITE,
    );
    // An `Option` records nothing when it is `None`, which leaves the field out of the event.
    emit(
        &SITE,
        [
            Some(&"migrations applied"),
            Some(&applied),
            Some(&latest_version),
        ],
    );
}

/// `ERROR` when a migration fails, with the value-free summary `error`.
fn migration_failed(error: &runtime::migrate::ErrorSummary) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "migration_failed",
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [Some(&"migration failed"), Some(&field::display(error))],
    );
}
