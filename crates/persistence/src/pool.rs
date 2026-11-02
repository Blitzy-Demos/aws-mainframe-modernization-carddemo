//! [`Db`], the `PostgreSQL` connection pool, and [`connect`], which opens it.
//!
//! A process opens one pool at start-up and shares it: `api` and `mq` clone
//! the [`Db`] into every request handler and consumer, and each `batch`
//! subcommand opens one for its run. Adapters hold a [`Db`] and begin their
//! [`UnitOfWork`] through [`Db::begin`], so none of them names sqlx.
//!
//! The database URL comes from the caller. `CARDDEMO_DATABASE_URL` is read by
//! the settings of `carddemo-runtime`; this crate reads no environment. The
//! URL carries the password, so nothing here logs or displays it, and the
//! [`Debug`] form of a [`Db`] shows only the pool's size and options.
//!
//! The pool runs on the caller's tokio runtime through sqlx's `runtime-tokio`
//! feature; this module starts none.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::PersistenceError;
use crate::tx::UnitOfWork;

/// The most connections one process keeps open. Each [`UnitOfWork`] holds one
/// connection until it commits or rolls back, so this bounds the units of
/// work a process runs at once; further [`Db::begin`] calls wait for a free
/// connection.
const MAX_CONNECTIONS: u32 = 10;

/// The connection pool of one process.
///
/// Cloning is cheap and every clone shares the same connections.
#[derive(Clone, Debug)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Wraps a pool the caller built itself, such as the testkit's pool on its
    /// `PostgreSQL` container.
    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for the code of this crate's callers that drives
    /// sqlx itself: the migrator, tests and the testkit.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Takes a connection from the pool and begins a transaction on it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Sqlx`] when no connection becomes free before the
    /// pool's acquire timeout, when a new connection cannot be opened, or when
    /// the `BEGIN` fails.
    pub async fn begin(&self) -> Result<UnitOfWork, PersistenceError> {
        let tx = self.pool.begin().await?;
        Ok(UnitOfWork::new(tx))
    }
}

/// Opens the process's pool on `url`, a `postgres://` connection URL, with at
/// most `MAX_CONNECTIONS` (10) connections.
///
/// One connection is opened at once, so a wrong URL, a wrong password or an
/// unreachable server fails here, at start-up, rather than on the first
/// request.
///
/// # Errors
///
/// [`PersistenceError::Sqlx`] when `url` does not parse as a connection URL,
/// or when the first connection cannot be opened or authenticated.
pub async fn connect(url: &str) -> Result<Db, PersistenceError> {
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(url)
        .await?;
    // The URL is left out of the event: it carries the password.
    tracing::debug!(max_connections = MAX_CONNECTIONS, "database pool connected");
    Ok(Db::from_pool(pool))
}
