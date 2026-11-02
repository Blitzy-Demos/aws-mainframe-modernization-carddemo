//! `carddemo-persistence`: the `PostgreSQL` 16 adapters of the `CardDemo` port.
//!
//! The crate implements the repository ports of `carddemo_domain::ports` and
//! the reward store port of `carddemo_domain::rewards` over one borrowed
//! `sqlx::Transaction`, and owns everything else that touches the database:
//!
//! - [`pool`]: [`Db`], the connection pool a process opens once with
//!   [`connect`];
//! - [`tx`]: [`UnitOfWork`], the database transaction an adapter opens and
//!   commits, [`PgStore`], the single port adapter bound to it, and the
//!   savepoint around each legacy write;
//! - [`repos`]: the port implementations on [`PgStore`], one file per table
//!   group;
//! - [`rewards`]: [`PgRewardStore`], the reward store adapter;
//! - [`ids`]: [`next_tran_id`], the serialized highest-key-plus-one `TRAN-ID`
//!   allocation of `COTRN02C` and `COBIL00C`;
//! - [`migrate`]: the migrations of `migrations/`, embedded at build time;
//! - [`seed`]: the fixture seed, refresh and `TRANEXTR` copy that replace the
//!   seed and load jobs.
//!
//! # Layering
//!
//! Ports and adapters. `carddemo-api`, `carddemo-batch` and
//! `carddemo-messaging` hold a [`Db`], open a [`UnitOfWork`], hand the rules
//! its [`PgStore`] and commit. This crate starts no async runtime (sqlx runs
//! on the caller's) and reads no environment: the database URL comes from the
//! caller's settings.
//!
//! # Write persistence
//!
//! Every `CardDemo` VSAM file is defined `RECOVERY(NONE)`
//! [app/csd/CARDDEMO.CSD:L1-96], so a completed write survives a later
//! failure in the same task and `SYNCPOINT ROLLBACK` does not undo it. Each
//! legacy write therefore runs under its own savepoint: a failed write rolls
//! back to that savepoint only, the writes before it stay in place, and the
//! caller commits what the parity audit says persists. [`tx`] holds the
//! mechanism and its detailed rules.
//!
//! # SQL
//!
//! Every statement is a `sqlx::query!`, `sqlx::query_as!` or
//! `sqlx::query_scalar!` invocation, checked at compile time against the
//! migrated schema, or against the committed `.sqlx/` metadata when built
//! offline. No SQL is assembled from strings. Legacy reads of the transaction
//! master go through the view `transaction_master` (E-09).
//!
//! # Errors
//!
//! [`PersistenceError`] is the error of the crate's own operations: connect,
//! migrate, seed, allocate, commit. Port methods return [`PortError`]
//! instead. The conversion between the two, and the SQLSTATE mapping behind
//! it, keep the outcomes the COBOL programs branch on (not found, duplicate
//! key, delete restricted, lock contention) and turn every other failure into
//! [`PortError::Unavailable`], whose text is for logs only.

pub mod ids;
pub mod migrate;
pub mod pool;
pub mod repos;
pub mod rewards;
pub mod seed;
pub mod tx;

pub use ids::next_tran_id;
pub use pool::{Db, connect};
pub use rewards::PgRewardStore;
pub use tx::{PgStore, UnitOfWork};

use std::path::PathBuf;

use carddemo_codec::CodecError;
use carddemo_domain::auth::PasswordError;
use carddemo_domain::ports::PortError;
use sqlx::error::DatabaseError;

/// SQLSTATE `unique_violation`: a row with the key already exists, the CICS
/// `DUPREC`/`DUPKEY` (VSAM file status 22) or Db2 `SQLCODE` -803 of the COBOL.
const UNIQUE_VIOLATION: &str = "23505";

/// SQLSTATE `foreign_key_violation`: the Db2 `SQLCODE` -532 of a delete that
/// dependent rows block, such as the `ON DELETE RESTRICT` of the categories on
/// their transaction type
/// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L6-7].
const FOREIGN_KEY_VIOLATION: &str = "23503";

/// SQLSTATE `serialization_failure`, one of the Db2 `SQLCODE` -911 outcomes.
const SERIALIZATION_FAILURE: &str = "40001";

/// SQLSTATE `deadlock_detected`, one of the Db2 `SQLCODE` -911 outcomes.
const DEADLOCK_DETECTED: &str = "40P01";

/// SQLSTATE `lock_not_available`: a lock wait that timed out or a `NOWAIT`
/// lock that was refused, the CICS lock wait or Db2 `SQLCODE` -911 timeout.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A failure of a persistence operation: connect, migrate, seed, `TRAN-ID`
/// allocation, commit, or a stored value read back from the database.
///
/// Display texts name files, tables, columns and lines, never a password, a
/// password hash or the contents of a row beyond its key. Port methods report
/// [`PortError`]; `PortError::from` converts this error into one.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// A database call failed.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Applying the embedded migrations failed.
    #[error("database migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// A fixture, JCL or control file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file that could not be read.
        path: PathBuf,
        /// The read failure.
        #[source]
        source: std::io::Error,
    },

    /// A fixture file does not decode through its copybook layout.
    #[error("cannot decode {path}: {source}")]
    Codec {
        /// The fixture file.
        path: PathBuf,
        /// The codec failure, located by layout, record, field and offset.
        #[source]
        source: CodecError,
    },

    /// A seed source outside the codec layouts does not parse: the `DUSRSECJ`
    /// `SYSUT1` in-stream rows [app/jcl/DUSRSECJ.jcl:L34-45] or a `DB2LT*.ctl`
    /// load statement.
    #[error("{path}:{line}: {reason}")]
    Fixture {
        /// The source file.
        path: PathBuf,
        /// The offending line, counted from 1.
        line: usize,
        /// What is wrong with the line.
        reason: String,
    },

    /// Hashing a seeded user password failed.
    #[error(transparent)]
    Password(#[from] PasswordError),

    /// A port call made on the crate's own behalf failed.
    #[error(transparent)]
    Port(#[from] PortError),

    /// A stored value does not fit its domain type: a `NUMERIC` outside its
    /// `Money<D>` picture, a `BIGINT` outside `RewardPoints`, a `VARCHAR`
    /// longer than its `Alnum<N>` width.
    #[error("stored {table}.{column} value does not fit its domain type: {reason}")]
    Corrupt {
        /// The table or view the value was read from.
        table: &'static str,
        /// The column holding the value.
        column: &'static str,
        /// Why the value does not fit, usually the domain constructor's error
        /// text.
        reason: String,
    },
}

impl From<PersistenceError> for PortError {
    /// Keeps a port outcome as it is, maps a database failure through its
    /// SQLSTATE, and reports every other failure as
    /// [`PortError::Unavailable`] with the error's text.
    fn from(err: PersistenceError) -> Self {
        match err {
            PersistenceError::Port(port) => port,
            PersistenceError::Sqlx(source) => port_error(&source),
            other => Self::Unavailable(other.to_string()),
        }
    }
}

/// Maps a sqlx failure onto the outcome the COBOL programs branch on.
///
/// - A query that had to return a row and found none is
///   [`PortError::NotFound`]: the CICS `NOTFND`, Db2 `SQLCODE` +100 or IMS
///   `GE` of the COBOL.
/// - A database error is mapped by its SQLSTATE: `23505` to
///   [`PortError::Duplicate`], `23503` to [`PortError::Restricted`], and
///   `40001`, `40P01` and `55P03` to [`PortError::Contention`].
/// - Anything else, a database error with another SQLSTATE, a pool timeout, an
///   I/O or decode failure, is [`PortError::Unavailable`].
///
/// The `Unavailable` text is a diagnostic for logs. It never carries a COBOL
/// message: the rules and the HTTP error mapping choose the audited message
/// shown to the user.
#[must_use]
pub(crate) fn port_error(err: &sqlx::Error) -> PortError {
    match err {
        sqlx::Error::RowNotFound => PortError::NotFound,
        sqlx::Error::Database(db) => database_port_error(db.as_ref()),
        other => PortError::Unavailable(other.to_string()),
    }
}

/// The SQLSTATE half of [`port_error`]. The text of an unmapped error keeps
/// only the code and the primary message; the server's detail line, which can
/// quote row contents, is left out.
fn database_port_error(db: &dyn DatabaseError) -> PortError {
    let Some(code) = db.code() else {
        return PortError::Unavailable(format!("database error: {}", db.message()));
    };
    match code.as_ref() {
        UNIQUE_VIOLATION => PortError::Duplicate,
        FOREIGN_KEY_VIOLATION => PortError::Restricted,
        SERIALIZATION_FAILURE | DEADLOCK_DETECTED | LOCK_NOT_AVAILABLE => PortError::Contention,
        other => PortError::Unavailable(format!("sqlstate {other}: {}", db.message())),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error;
    use std::fmt;
    use std::path::PathBuf;

    use carddemo_codec::{CodecError, CodecErrorKind};
    use carddemo_domain::auth::PasswordError;
    use sqlx::error::{DatabaseError, ErrorKind};
    use sqlx::migrate::MigrateError;

    use super::{PersistenceError, PortError, port_error};

    /// A database error with a chosen SQLSTATE, standing in for the server's.
    #[derive(Debug)]
    struct ServerError {
        code: Option<&'static str>,
        message: &'static str,
    }

    impl fmt::Display for ServerError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.message)
        }
    }

    impl Error for ServerError {}

    impl DatabaseError for ServerError {
        fn message(&self) -> &str {
            self.message
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            self.code.map(Cow::Borrowed)
        }

        fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn server_error(code: Option<&'static str>) -> sqlx::Error {
        sqlx::Error::Database(Box::new(ServerError {
            code,
            message: "relation is locked",
        }))
    }

    fn unavailable(text: &str) -> PortError {
        PortError::Unavailable(text.to_owned())
    }

    fn corrupt() -> PersistenceError {
        PersistenceError::Corrupt {
            table: "accounts",
            column: "acct_curr_bal",
            reason: "more than 10 integer digits".to_owned(),
        }
    }

    fn codec_failure() -> PersistenceError {
        PersistenceError::Codec {
            path: PathBuf::from("app/data/ASCII/acctdata.txt"),
            source: CodecError::new(
                "CVACT01Y",
                CodecErrorKind::Length {
                    expected: 300,
                    actual: 299,
                },
            )
            .at_record(3),
        }
    }

    fn read_failure() -> PersistenceError {
        PersistenceError::Io {
            path: PathBuf::from("app/jcl/DUSRSECJ.jcl"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }
    }

    #[test]
    fn sqlstates_map_to_the_outcomes_the_cobol_branches_on() {
        let cases = [
            (Some("23505"), PortError::Duplicate),
            (Some("23503"), PortError::Restricted),
            (Some("40001"), PortError::Contention),
            (Some("40P01"), PortError::Contention),
            (Some("55P03"), PortError::Contention),
            (
                Some("23514"),
                unavailable("sqlstate 23514: relation is locked"),
            ),
            (None, unavailable("database error: relation is locked")),
        ];
        for (code, expected) in cases {
            assert_eq!(port_error(&server_error(code)), expected, "{code:?}");
        }
    }

    #[test]
    fn row_not_found_is_not_found_and_other_failures_are_unavailable() {
        assert_eq!(port_error(&sqlx::Error::RowNotFound), PortError::NotFound);
        for err in [
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
            sqlx::Error::Protocol("unexpected message".to_owned()),
        ] {
            assert_eq!(port_error(&err), unavailable(&err.to_string()));
        }
    }

    #[test]
    fn conversion_keeps_port_outcomes_and_reports_the_rest_as_unavailable() {
        let corrupt_text = corrupt().to_string();
        let cases = [
            (
                PersistenceError::Port(PortError::Duplicate),
                PortError::Duplicate,
            ),
            (
                PersistenceError::Port(unavailable("pool closed")),
                unavailable("pool closed"),
            ),
            (
                PersistenceError::Sqlx(sqlx::Error::RowNotFound),
                PortError::NotFound,
            ),
            (
                PersistenceError::Sqlx(server_error(Some("23505"))),
                PortError::Duplicate,
            ),
            (
                PersistenceError::Sqlx(server_error(Some("40P01"))),
                PortError::Contention,
            ),
            (corrupt(), unavailable(&corrupt_text)),
            (
                PersistenceError::Migrate(MigrateError::VersionMissing(3)),
                unavailable(
                    "database migration failed: migration 3 was previously applied but is \
                     missing in the resolved migrations",
                ),
            ),
        ];
        for (err, expected) in cases {
            let text = err.to_string();
            assert_eq!(PortError::from(err), expected, "{text}");
        }
    }

    #[test]
    fn display_texts_name_the_place_of_the_failure() {
        let cases = [
            (
                PersistenceError::Sqlx(sqlx::Error::PoolTimedOut),
                "database error: pool timed out while waiting for an open connection",
            ),
            (
                read_failure(),
                "cannot read app/jcl/DUSRSECJ.jcl: entity not found",
            ),
            (
                codec_failure(),
                "cannot decode app/data/ASCII/acctdata.txt: CVACT01Y record 3: length 299, \
                 expected 300",
            ),
            (
                PersistenceError::Fixture {
                    path: PathBuf::from("app/jcl/DUSRSECJ.jcl"),
                    line: 36,
                    reason: "row longer than 80 bytes".to_owned(),
                },
                "app/jcl/DUSRSECJ.jcl:36: row longer than 80 bytes",
            ),
            (
                PersistenceError::Password(PasswordError::Hash("salt rejected".to_owned())),
                "password hashing failed: salt rejected",
            ),
            (
                PersistenceError::Port(PortError::Restricted),
                "delete restricted by dependent records",
            ),
            (
                corrupt(),
                "stored accounts.acct_curr_bal value does not fit its domain type: more than \
                 10 integer digits",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn wrapped_failures_stay_reachable_as_sources() {
        let io = read_failure();
        let io_source = io.source().and_then(|s| s.downcast_ref::<std::io::Error>());
        assert_eq!(
            io_source.map(std::io::Error::kind),
            Some(std::io::ErrorKind::NotFound)
        );

        let codec = codec_failure();
        let codec_source = codec.source().and_then(|s| s.downcast_ref::<CodecError>());
        assert_eq!(codec_source.and_then(|e| e.record), Some(3));

        assert!(corrupt().source().is_none());
    }
}
