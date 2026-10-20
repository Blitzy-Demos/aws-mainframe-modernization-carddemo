//! The single schema executor of the `CardDemo` replacement.
//!
//! This module is the only place in the workspace that invokes `sqlx::migrate!`. At build time
//! it embeds every file of `crates/persistence/migrations`, the only source of the schema's shape:
//!
//! - `0001_reference_and_master.sql`: the VSAM data version of the master and reference files;
//! - `0002_transactions.sql`: the transaction file;
//! - `0003_users.sql`: the user security file, with argon2id hashes in place of passwords;
//! - `0004_authorizations.sql`: the IMS pending-authorization hierarchy and the Db2 fraud table;
//! - `0005_cssl_error_log.sql`: the `CSSL` error log of the `CP00` worker;
//! - `0006_db2_transaction_types.sql`: the Db2 data version of the transaction types and
//!   categories. It performs the create-and-seed of `CREADB21`
//!   (`app/app-transaction-type-db2/jcl/CREADB21.jcl:52-84`) once per database, because a
//!   migration recorded as applied never runs again.
//!
//! [`apply`] has exactly two callers: the `carddemo-migrate` binary, the operator's only path to
//! the schema, and the `postgres` module of `test-support`, which migrates every test container.
//! The API, worker and batch binaries never call it; started against an unmigrated database, they
//! fail at their first query. `sqlx-cli` is used only for `prepare`, never for `migrate run`.
//!
//! The entrypoint lives in `runtime` because this crate has no query macros: it compiles against
//! an empty database, so a fresh checkout creates its schema before `persistence`, whose macros
//! need that schema or the committed `.sqlx/` metadata, is ever built. `build.rs` reruns the build
//! whenever the migrations directory changes, so the embedded set always matches it.
//!
//! Migrations are expand-only in this release: they create and never drop, rename or retype, so
//! the previous release's queries still run against the migrated schema. Step 6 of
//! `scripts/check.sh` scans the migrations directory to enforce it. A schema change is a new
//! numbered file, never an edit of an applied one.

use sqlx::PgPool;
use sqlx::migrate::{MigrateError, Migrator};

/// Every file of `crates/persistence/migrations`, embedded at build time. The macro resolves the
/// path against this package's root, `crates/runtime`.
static MIGRATOR: Migrator = sqlx::migrate!("../persistence/migrations");

/// Applies every pending migration, in version order, to the database behind `pool`.
///
/// sqlx records each applied migration in its bookkeeping table `_sqlx_migrations`. A migration
/// already recorded there is skipped, so a second run against a migrated database changes
/// nothing. Each pending migration runs in one transaction with its bookkeeping row, so a file
/// that fails leaves none of its statements applied.
///
/// The run holds a `PostgreSQL` advisory lock keyed on the database name, so concurrent runs
/// against one database wait for each other; test containers each have their own database and
/// never wait. The lock is session-level and sqlx releases it only on success: after an error it
/// stays held by the pooled connection until that connection closes. Both callers end on an
/// error, which closes it.
///
/// # Errors
///
/// Returns the [`MigrateError`] of the first failure:
/// - a connection failure, or a migration file whose SQL fails ([`MigrateError::Execute`],
///   [`MigrateError::ExecuteMigration`]);
/// - a checksum mismatch: an applied migration whose embedded file has changed since it ran
///   ([`MigrateError::VersionMismatch`]);
/// - an applied migration missing from the embedded set ([`MigrateError::VersionMissing`]).
pub async fn apply(pool: &PgPool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}
