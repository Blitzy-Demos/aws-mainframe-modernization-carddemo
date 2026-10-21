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
//! Errors carry only the `Display` text of the testcontainers, Docker, `sqlx` or migration error
//! behind them, the bound that passed, or the label token of the start, none of which holds
//! record data (Cardholder Data Redaction).

use std::fmt;
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::{ContainerAsync, ContainerRequest, GenericImage, ImageExt};
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

/// Bound on applying the migrations. The six files create the schema within seconds even on a
/// busy host; the bound turns a server that stops answering after readiness into an error
/// instead of a test that never ends.
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(120);

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
/// 60 seconds of that message, when the pool cannot connect, when a migration fails, or when the
/// migrations do not finish within 120 seconds, as when the server stops answering after it
/// became ready. Each of these errors drops the pool, when one was opened, and removes the
/// container; when Docker created the container but the start failed, the error also reports
/// how many containers of this start that removal found, or why it failed.
pub async fn start_postgres() -> Result<PostgresGuard, HarnessError> {
    let container = start_container(image()).await?;
    let unmigrated = connect(container).await?;
    migrate(unmigrated, MIGRATION_TIMEOUT).await
}

/// Reads the address of `container`, waits until the server accepts connections and opens the
/// pool. The guard returned is not migrated yet, and only [`migrate`] hands one out of this
/// module. On an error the container is removed as its handle drops.
async fn connect(container: ContainerAsync<GenericImage>) -> Result<PostgresGuard, HarnessError> {
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

    Ok(PostgresGuard {
        pool,
        database_url,
        container,
    })
}

/// Applies every migration to the database of `guard` within `bound` and returns the guard.
///
/// The readiness probe is over by then, and the pool's acquire timeout bounds only taking a
/// connection, never a query on it, so only `bound` stops a server that stops answering
/// part-way. On an error the guard drops before the error returns: its pool first, then the
/// container, whose removal also ends the server session the abandoned migration left open.
async fn migrate(guard: PostgresGuard, bound: Duration) -> Result<PostgresGuard, HarnessError> {
    match time::timeout(bound, runtime::migrate::apply(&guard.pool)).await {
        Ok(Ok(())) => Ok(guard),
        Ok(Err(err)) => Err(HarnessError::new("apply migrations", &err)),
        Err(_) => Err(HarnessError::new(
            "apply migrations",
            &format_args!(
                "the migrations did not finish within {} ms",
                bound.as_millis()
            ),
        )),
    }
}

/// The `postgres:16` image with its port and ready message. [`request`] adds the rest; the two
/// stay apart so that a test can change what only a `GenericImage` can, such as the entrypoint.
fn image() -> GenericImage {
    GenericImage::new(IMAGE, TAG)
        .with_exposed_port(PORT.tcp())
        .with_wait_for(WaitFor::message_on_stderr(READY_MESSAGE))
}

/// The container request of [`start_postgres`] for `image`: the credentials and the startup
/// bound.
fn request(image: GenericImage) -> ContainerRequest<GenericImage> {
    image
        .with_env_var("POSTGRES_USER", USER)
        .with_env_var("POSTGRES_PASSWORD", PASSWORD)
        .with_env_var("POSTGRES_DB", DATABASE)
        .with_startup_timeout(STARTUP_TIMEOUT)
}

/// Starts the container of `image`, owning its removal from before Docker creates it.
async fn start_container(
    image: GenericImage,
) -> Result<ContainerAsync<GenericImage>, HarnessError> {
    crate::start_owned(request(image), "start postgres container").await
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
    use std::time::Duration;

    use tokio::time::Instant;

    use super::{
        IMAGE, address, connect, image, migrate, request, start_container, start_postgres,
    };
    use crate::HarnessError;
    use crate::tests::{
        MISSING_ENTRYPOINT, abandoned_start_case, cancelled_start_case, docker, exists,
        failed_start_case, guarded_container_case, remove, shutdown_start_case,
    };

    /// Migration bound of the frozen-server case, far below the pool's 30-second acquire
    /// timeout, so an error near it can come only from the bound.
    const FROZEN_BOUND: Duration = Duration::from_secs(3);
    /// Time allowed past [`FROZEN_BOUND`] for force-removing the paused container.
    const REMOVAL_ALLOWANCE: Duration = Duration::from_secs(15);

    #[tokio::test]
    async fn a_started_database_is_owned_and_leaves_with_its_guard() -> Result<(), HarnessError> {
        let guard = start_postgres().await?;
        let id = guard.container.id().to_owned();
        guarded_container_case(guard, &id, IMAGE).await
    }

    /// The server is paused between readiness and migration, so it accepts nothing more. The
    /// migration fails at its bound, and the container is gone when the error returns.
    #[tokio::test]
    async fn a_server_frozen_after_readiness_fails_the_migration_at_its_bound()
    -> Result<(), HarnessError> {
        let docker = docker().await?;
        let container = start_container(image()).await?;
        let id = container.id().to_owned();
        let unmigrated = connect(container).await?;
        docker
            .pause_container(&id)
            .await
            .map_err(|err| HarnessError::new("pause postgres", &err))?;

        let started = Instant::now();
        let migrated = migrate(unmigrated, FROZEN_BOUND).await;
        let elapsed = started.elapsed();
        let kept = exists(&docker, &id).await;
        let leftover = if matches!(kept, Ok(false)) {
            None
        } else {
            Some(remove(&docker, &id).await)
        };

        let Err(err) = migrated else {
            panic!("the migration of a paused server succeeded");
        };
        assert_eq!(
            err.to_string(),
            format!(
                "apply migrations: the migrations did not finish within {} ms",
                FROZEN_BOUND.as_millis()
            )
        );
        assert!(
            elapsed >= FROZEN_BOUND && elapsed < FROZEN_BOUND + REMOVAL_ALLOWANCE,
            "{elapsed:?}"
        );
        assert!(
            matches!(kept, Ok(false)),
            "the container outlived the error: {kept:?}; removal: {leftover:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_start_removes_only_its_own_container() -> Result<(), HarnessError> {
        failed_start_case(
            request(image().with_entrypoint(MISSING_ENTRYPOINT)),
            "start postgres container",
        )
        .await
    }

    #[tokio::test]
    async fn an_abandoned_start_removes_its_container_on_a_current_thread_runtime()
    -> Result<(), HarnessError> {
        abandoned_start_case(request(image())).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_abandoned_start_removes_its_container_on_a_multi_thread_runtime()
    -> Result<(), HarnessError> {
        abandoned_start_case(request(image())).await
    }

    #[tokio::test]
    async fn a_dropped_start_runs_on_and_removes_its_container_on_a_current_thread_runtime()
    -> Result<(), HarnessError> {
        cancelled_start_case(|| request(image()), "start postgres container").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_start_runs_on_and_removes_its_container_on_a_multi_thread_runtime()
    -> Result<(), HarnessError> {
        cancelled_start_case(|| request(image()), "start postgres container").await
    }

    #[tokio::test]
    async fn starts_cut_off_by_a_runtime_shutdown_leave_no_container() -> Result<(), HarnessError> {
        for multi_thread in [false, true] {
            shutdown_start_case(
                || request(image()),
                "start postgres container",
                multi_thread,
            )
            .await?;
        }
        Ok(())
    }

    #[test]
    fn address_omits_the_credentials() {
        assert_eq!(
            address("postgres://carddemo:carddemo@localhost:32768/carddemo"),
            "localhost:32768/carddemo"
        );
        assert_eq!(address("no-credentials"), "");
    }
}
