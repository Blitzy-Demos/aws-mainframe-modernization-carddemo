//! The schema migrator: the SQL files of the workspace `migrations/`
//! directory, embedded into the crate at compile time.
//!
//! It replaces the DDL step of `CREADB21`, the `CRCRDDB` step that runs
//! `DSNTIAD` over the `DB2CREAT` statements
//! [app/app-transaction-type-db2/jcl/CREADB21.jcl:L52-59], and the IDCAMS
//! `DEFINE` steps of the VSAM file jobs, whose roles the migration headers
//! name. `batch migrate` calls [`run`] with its [`Db`]; the testkit calls
//! [`run_on`] with the pool of each fresh test container.
//!
//! Only the schema is applied here. The rows `CREADB21` loads next through
//! `DSNTEP4` (`DB2LTTYP`, `DB2LTCAT`
//! [app/app-transaction-type-db2/jcl/CREADB21.jcl:L64-84]), the fixture files
//! and the reward seed rules are loaded by [`crate::seed`].
//!
//! # Versions
//!
//! Each file's numeric prefix is its sqlx version: `0001_reference_data.sql`
//! is version 1, through `0005_rewards.sql` as version 5. sqlx records every
//! applied version with the checksum of its file in the `_sqlx_migrations`
//! table, applies each pending file in its own database transaction, and
//! holds a `PostgreSQL` advisory lock for the whole run, so two migrators on
//! one database apply each version once.

use sqlx::PgPool;
use sqlx::migrate::Migrator;

use crate::PersistenceError;
use crate::pool::Db;

/// The migrations of the workspace `migrations/` directory, read at compile
/// time.
///
/// The path is relative to this crate's manifest directory,
/// `crates/persistence`. The macro fails the build when the directory is
/// missing, and `build.rs` makes Cargo rebuild the crate whenever a file is
/// added to, edited in or removed from it, so the embedded set always equals
/// the directory.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Applies every embedded migration that `pool`'s database has not applied
/// yet, in version order.
///
/// Idempotent: versions already recorded with a matching checksum are
/// skipped, so a second run on a migrated database changes nothing.
///
/// # Errors
///
/// [`PersistenceError::Migrate`] when a connection or the advisory lock
/// cannot be acquired, when a migration's SQL fails (its transaction rolls
/// back and the versions before it stay applied), when an applied version's
/// file has since been edited (checksum mismatch), when an applied version no
/// longer exists in the directory, or when an earlier run left a version
/// recorded as failed.
pub async fn run_on(pool: &PgPool) -> Result<(), PersistenceError> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(PersistenceError::Migrate)?;
    tracing::info!(
        migrations = MIGRATOR.iter().count(),
        "database migrations applied"
    );
    Ok(())
}

/// Applies the embedded migrations to the database of `db`, as [`run_on`]
/// does with its pool.
///
/// # Errors
///
/// [`PersistenceError::Migrate`] in every case [`run_on`] lists.
pub async fn run(db: &Db) -> Result<(), PersistenceError> {
    run_on(db.pool()).await
}
