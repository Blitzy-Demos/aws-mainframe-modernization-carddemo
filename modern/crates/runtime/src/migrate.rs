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
//!
//! The text of a [`MigrateError`] embeds the driver's and the server's text, which can quote a
//! rejected value or a key value, so callers log a migration error only through
//! [`ErrorSummary`].

use std::fmt;

use sqlx::PgPool;
use sqlx::migrate::{MigrateError, Migrator};

use crate::db;

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

/// A value-free account of a [`MigrateError`], the only form in which a caller logs one.
///
/// Its `Display` is a fixed error class, such as `execute_migration` or `version_mismatch`, then
/// the migration versions the error names and, for a failed execution, the
/// [`db::ErrorSummary`] of the `sqlx` error behind it, as in
/// `execute_migration (version 4): database (SQLSTATE 42P07)`. It never keeps the text of the
/// driver, the server or the migration source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorSummary {
    /// Stable name of the error variant.
    class: &'static str,
    /// The migration the error names.
    version: Option<i64>,
    /// The latest applied migration, named by an out-of-order version.
    latest_applied: Option<i64>,
    /// The summary of the `sqlx` error behind a failed execution.
    cause: Option<db::ErrorSummary>,
}

impl From<&MigrateError> for ErrorSummary {
    fn from(error: &MigrateError) -> Self {
        let (class, version, latest_applied, cause) = match error {
            MigrateError::Execute(cause) => ("execute", None, None, Some(cause)),
            MigrateError::ExecuteMigration(cause, version) => {
                ("execute_migration", Some(*version), None, Some(cause))
            }
            MigrateError::Source(_) => ("source", None, None, None),
            MigrateError::VersionMissing(version) => {
                ("version_missing", Some(*version), None, None)
            }
            MigrateError::VersionMismatch(version) => {
                ("version_mismatch", Some(*version), None, None)
            }
            MigrateError::VersionNotPresent(version) => {
                ("version_not_present", Some(*version), None, None)
            }
            MigrateError::VersionTooOld(version, latest) => {
                ("version_too_old", Some(*version), Some(*latest), None)
            }
            MigrateError::VersionTooNew(version, latest) => {
                ("version_too_new", Some(*version), Some(*latest), None)
            }
            MigrateError::ForceNotSupported => ("force_not_supported", None, None, None),
            MigrateError::Dirty(version) => ("dirty", Some(*version), None, None),
            // `MigrateError` is non-exhaustive and its remaining variant is deprecated; neither
            // a later variant nor that one keeps its text.
            _ => ("other", None, None, None),
        };
        Self {
            class,
            version,
            latest_applied,
            cause: cause.map(db::ErrorSummary::from),
        }
    }
}

impl fmt::Display for ErrorSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.class)?;
        match (self.version, self.latest_applied) {
            (Some(version), Some(latest)) => {
                write!(f, " (version {version}, latest applied {latest})")?;
            }
            (Some(version), None) => write!(f, " (version {version})")?,
            (None, _) => {}
        }
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use sqlx::migrate::MigrateError;

    use super::ErrorSummary;

    /// Stand-in for a value that the driver's or the server's text quotes.
    const SECRET: &str = "s3cr3t-migration-value-8K";

    #[test]
    fn summaries_keep_the_class_and_versions_and_drop_every_value() {
        let executed = MigrateError::ExecuteMigration(sqlx::Error::Protocol(SECRET.to_owned()), 4);
        assert!(executed.to_string().contains(SECRET), "{executed}");
        let cases = [
            (executed, "execute_migration (version 4): protocol"),
            (
                MigrateError::Execute(sqlx::Error::Io(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    SECRET,
                ))),
                "execute: io (connection reset)",
            ),
            (MigrateError::Source(SECRET.into()), "source"),
            (
                MigrateError::VersionMissing(5),
                "version_missing (version 5)",
            ),
            (
                MigrateError::VersionMismatch(3),
                "version_mismatch (version 3)",
            ),
            (
                MigrateError::VersionNotPresent(6),
                "version_not_present (version 6)",
            ),
            (
                MigrateError::VersionTooOld(2, 6),
                "version_too_old (version 2, latest applied 6)",
            ),
            (
                MigrateError::VersionTooNew(7, 6),
                "version_too_new (version 7, latest applied 6)",
            ),
            (MigrateError::ForceNotSupported, "force_not_supported"),
            (MigrateError::Dirty(1), "dirty (version 1)"),
        ];
        for (error, expected) in &cases {
            assert_eq!(
                ErrorSummary::from(error).to_string(),
                *expected,
                "{error:?}"
            );
        }
    }
}
