//! Applies the embedded `CardDemo` migrations to the `PostgreSQL` database named by
//! `DATABASE_URL`, through the embedded migrator in `runtime::migrate`.
//!
//! This binary is the only operator entrypoint for schema changes: the API, worker and batch
//! binaries never migrate, and the `sqlx` command-line tool only prepares the query cache. A
//! successful run logs one INFO event, `migrations applied`, whose `applied` field counts the
//! migrations this run applied and whose `latest_version` field names the latest version the
//! database records. A run against an already migrated database applies nothing, logs `applied`
//! 0, and still succeeds. The exit status is non-zero when the configuration, the connection or
//! a migration fails. A configuration error names its variable and never its value. A connection
//! error names `DATABASE_URL`. A connection or migration error is logged only as the value-free
//! summary that `runtime::db` or `runtime::migrate` builds: an error class with at most migration
//! versions and a SQLSTATE, never the driver's or the server's text. Neither the connection URL
//! nor any value it carries reaches the output.

use std::process::ExitCode;

use runtime::config::{Config, OtelExport};
use tracing::Instrument;

#[tokio::main]
async fn main() -> ExitCode {
    let config = Config::from_env();
    let otel_export = config.as_ref().map_or(OtelExport::Off, |c| c.otel_export);
    // Bound until `main` returns: dropping the guard flushes the exported spans.
    let _telemetry = match runtime::telemetry::init(otel_export) {
        Ok(guard) => guard,
        Err(err) => {
            // No subscriber exists yet, so standard error is the only channel left.
            eprintln!("carddemo-migrate: {err}");
            return ExitCode::FAILURE;
        }
    };
    let config = match config {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(error = %err, "invalid configuration");
            return ExitCode::FAILURE;
        }
    };
    async {
        let pool = match runtime::db::connect(&config).await {
            Ok(pool) => pool,
            Err(err) => {
                let error = runtime::db::ErrorSummary::from(&err);
                tracing::error!(%error, "database connection failed (DATABASE_URL)");
                return ExitCode::FAILURE;
            }
        };
        let outcome = runtime::migrate::apply(&pool).await;
        // Closing also ends the session that holds the migration lock after a failure.
        pool.close().await;
        match outcome {
            Ok(applied) => {
                tracing::info!(
                    applied = applied.count(),
                    latest_version = applied.latest(),
                    "migrations applied"
                );
                ExitCode::SUCCESS
            }
            Err(err) => {
                let error = runtime::migrate::ErrorSummary::from(&err);
                tracing::error!(%error, "migration failed");
                ExitCode::FAILURE
            }
        }
    }
    .instrument(runtime::telemetry::job_span("carddemo-migrate"))
    .await
}
