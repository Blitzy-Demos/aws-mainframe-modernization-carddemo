//! `PostgreSQL` connection pool of the `carddemo-api`, `carddemo-mq`, `carddemo-batch` and
//! `carddemo-migrate` binaries.
//!
//! [`connect`] opens a [`PgPool`] on [`Config::database_url`]: the local `PostgreSQL` 16 service
//! of `modern/compose.yaml`, or the testcontainers instance an integration test hands a binary
//! through `DATABASE_URL`. The `sqlx` feature set has no TLS backend, so only a local, unencrypted
//! connection is possible (Local-Only Runtime).
//!
//! Connecting never applies migrations. The schema reaches the database only through the
//! `apply` function in the `migrate` module (`crate::migrate`), run by the `carddemo-migrate`
//! binary or by the test harness. A service started against an unmigrated database therefore
//! connects and then fails at its first query.
//!
//! The URL carries the database password, so nothing here logs, traces or formats it.

use crate::config::Config;
use sqlx::postgres::{PgPool, PgPoolOptions};

/// Opens a [`PgPool`] on [`Config::database_url`] with the default `sqlx` pool settings, and
/// establishes one connection before returning, so a wrong URL, an unreachable server or rejected
/// credentials surface at startup rather than at the first query.
///
/// # Errors
///
/// - [`sqlx::Error::Configuration`] when the URL is not a valid `PostgreSQL` connection URL.
/// - [`sqlx::Error::Io`] or [`sqlx::Error::PoolTimedOut`] when the server cannot be reached
///   within the pool's acquire timeout.
/// - [`sqlx::Error::Database`] when the server rejects the connection, for example on an
///   authentication failure or an unknown database.
pub async fn connect(config: &Config) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().connect(&config.database_url).await
}

#[cfg(test)]
mod tests {
    use super::connect;
    use crate::config::Config;

    #[tokio::test]
    async fn rejects_invalid_url() {
        let config = Config {
            database_url: "not-a-url".to_owned(),
            ..Config::default()
        };
        match connect(&config).await {
            Err(error @ sqlx::Error::Configuration(_)) => {
                assert!(
                    !error.to_string().contains("not-a-url"),
                    "the error must not quote the URL: {error}"
                );
            }
            Err(other) => panic!("expected a configuration error, got {other}"),
            Ok(_) => panic!("an invalid URL must not yield a pool"),
        }
    }
}
