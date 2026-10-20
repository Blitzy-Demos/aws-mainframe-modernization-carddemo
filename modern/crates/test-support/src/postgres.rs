//! A `postgres:16` testcontainer whose schema comes from the single schema executor.
//!
//! [`start_postgres`] boots the image with the local-only credentials of `compose.yaml` and
//! `.env.example` (user, password and database `carddemo`) and migrates it through
//! `runtime::migrate::apply`, the migration entrypoint whose only callers are the
//! `carddemo-migrate` binary and this module. Every test database is therefore built from
//! `crates/persistence/migrations` exactly as the local stack is, the `db2` schema and its
//! `CREADB21` seed rows of migration `0006` included. This module issues no SQL of its own: the
//! readiness probe uses the connection ping alone.
//!
//! Errors carry only the `Display` text of the testcontainers, `sqlx` or migration error behind
//! them, which holds no record data (Cardholder Data Redaction).

use std::fmt;
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::time::{self, Instant};

use crate::HarnessError;

const IMAGE: &str = "postgres";
const TAG: &str = "16";
const PORT: u16 = 5432;

/// `POSTGRES_USER`, `POSTGRES_PASSWORD` and `POSTGRES_DB` of `compose.yaml`: local-only values.
const USER: &str = "carddemo";
const PASSWORD: &str = "carddemo";
const DATABASE: &str = "carddemo";

/// Logged by every server start. On a first initialisation the image logs it twice: once for the
/// temporary server that runs the init scripts on the Unix socket only, then for the real server,
/// so the probe below, not this message, decides readiness.
const READY_MESSAGE: &str = "database system is ready to accept connections";
/// Covers a first pull of the image on a busy host.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(180);

/// Bound on the readiness probe that follows the startup message.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Bound on one probe attempt, so a handshake the server never answers cannot stall the probe.
const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_RETRY: Duration = Duration::from_millis(500);

/// Serves the API tests' concurrent requests. Spawned binaries, such as the two `carddemo-mq`
/// processes of the credit-limit test, open their own pools from [`PostgresGuard::database_url`].
const MAX_CONNECTIONS: u32 = 16;

/// A running, migrated `PostgreSQL` container, a pool on it and the URL that reaches it.
///
/// Dropping the guard drops its pool handle and then removes the container: the pool is declared
/// first because fields drop in declaration order. The removal runs on the
/// test's Tokio runtime, so drop the guard inside that runtime and never keep one in a `static`
/// or `OnceLock`, which outlives it.
///
/// An integration test that spawns a binary (`carddemo-mq`, `carddemo-batch`) sets that
/// process's `DATABASE_URL` to [`database_url`](Self::database_url) and runs it from the
/// `modern/` directory, [`modern_dir`](crate::modern_dir).
pub struct PostgresGuard {
    pool: PgPool,
    database_url: String,
    container: ContainerAsync<GenericImage>,
}

// Hand-written so that the container handle need not be `Debug` and a test printing the guard
// prints the server address only, never the credential part of the URL.
impl fmt::Debug for PostgresGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresGuard")
            .field("container_id", &self.container.id())
            .field("address", &address(&self.database_url))
            .finish_non_exhaustive()
    }
}

impl PostgresGuard {
    /// A pool of up to 16 connections on the migrated database.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The database URL, `postgres://carddemo:carddemo@{host}:{port}/carddemo`, in the shape of
    /// `DATABASE_URL`, so a spawned binary can be pointed at this container.
    #[must_use]
    pub fn database_url(&self) -> &str {
        &self.database_url
    }
}

/// Starts a `postgres:16` container with user, password and database `carddemo`, waits until it
/// accepts TCP connections, opens a pool of up to 16 connections and applies every migration of
/// `crates/persistence/migrations` through `runtime::migrate::apply`.
///
/// # Errors
///
/// Returns a [`HarnessError`] when the container does not start or log its ready message within
/// 180 seconds, when its host or mapped port cannot be read, when no connection succeeds within
/// 60 seconds of that message, when the pool cannot connect, or when a migration fails.
pub async fn start_postgres() -> Result<PostgresGuard, HarnessError> {
    let container = GenericImage::new(IMAGE, TAG)
        .with_exposed_port(PORT.tcp())
        .with_wait_for(WaitFor::message_on_stderr(READY_MESSAGE))
        .with_env_var("POSTGRES_USER", USER)
        .with_env_var("POSTGRES_PASSWORD", PASSWORD)
        .with_env_var("POSTGRES_DB", DATABASE)
        .with_startup_timeout(STARTUP_TIMEOUT)
        .start()
        .await
        .map_err(|err| HarnessError::new("start postgres container", &err))?;

    let host = container
        .get_host()
        .await
        .map_err(|err| HarnessError::new("read postgres host", &err))?;
    let port = container
        .get_host_port_ipv4(PORT.tcp())
        .await
        .map_err(|err| HarnessError::new("read postgres port", &err))?;
    let database_url = format!("postgres://{USER}:{PASSWORD}@{host}:{port}/{DATABASE}");

    wait_until_ready(&database_url).await?;

    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(&database_url)
        .await
        .map_err(|err| HarnessError::new("connect postgres pool", &err))?;
    runtime::migrate::apply(&pool)
        .await
        .map_err(|err| HarnessError::new("apply migrations", &err))?;

    Ok(PostgresGuard {
        pool,
        database_url,
        container,
    })
}

/// Retries a connection and a ping until both succeed or [`READY_TIMEOUT`] passes. The failure
/// reported after the bound is the last attempt's.
async fn wait_until_ready(database_url: &str) -> Result<(), HarnessError> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let failure = match time::timeout(PROBE_ATTEMPT_TIMEOUT, probe(database_url)).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(err)) => err.to_string(),
            Err(elapsed) => elapsed.to_string(),
        };
        if Instant::now() >= deadline {
            return Err(HarnessError::new("postgres readiness", &failure));
        }
        time::sleep(PROBE_RETRY).await;
    }
}

/// One readiness attempt: connect over TCP, ping, and close the connection cleanly.
async fn probe(database_url: &str) -> Result<(), sqlx::Error> {
    let mut connection = PgConnection::connect(database_url).await?;
    connection.ping().await?;
    connection.close().await
}

/// The part of a `postgres://user:password@host:port/database` URL after the credentials.
fn address(database_url: &str) -> &str {
    database_url
        .rsplit_once('@')
        .map_or("", |(_, address)| address)
}

#[cfg(test)]
mod tests {
    use super::address;

    #[test]
    fn address_omits_the_credentials() {
        assert_eq!(
            address("postgres://carddemo:carddemo@localhost:32768/carddemo"),
            "localhost:32768/carddemo"
        );
        assert_eq!(address("no-credentials"), "");
    }
}
