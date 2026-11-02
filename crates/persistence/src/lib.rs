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
//!   savepoint around each statement of a store;
//! - [`repos`]: the port implementations on [`PgStore`], one file per table
//!   group;
//! - [`rewards`]: [`PgRewardStore`], the reward store adapter;
//! - [`ids`]: [`next_tran_id`], the serialized highest-key-plus-one `TRAN-ID`
//!   allocation of `COTRN02C` and `COBIL00C`, and the account lock order
//!   around it;
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
//! failure in the same task and `SYNCPOINT ROLLBACK` does not undo it. A
//! server-side error aborts the whole `PostgreSQL` transaction, a failed read
//! as much as a failed write, so every statement of a store runs under its own
//! savepoint: a failed statement rolls back to that savepoint only, the writes
//! before it stay in place, and the caller commits what the parity audit says
//! persists. [`tx`] holds the mechanism and its detailed rules.
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
pub use pool::{Db, connect, connect_with_statement_timeout};
pub use rewards::PgRewardStore;
pub use tx::{PgStore, UnitOfWork};

use std::borrow::Cow;
use std::panic::Location;
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
/// allocation, commit, a stored value read back from the database, or an
/// instant a column cannot store exactly.
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

    /// An instant to be written is not a whole microsecond of UTC time: it
    /// has a part finer than the microsecond a `TIMESTAMPTZ` keeps, or is a
    /// leap second. The bind would narrow it without a word, so the write is
    /// refused before its statement runs.
    #[error(
        "{table}.{column} cannot store the instant exactly: it is not a whole microsecond of UTC time"
    )]
    InexactInstant {
        /// The table the write targets.
        table: &'static str,
        /// The `TIMESTAMPTZ` column the instant was to be bound to.
        column: &'static str,
    },
}

impl From<PersistenceError> for PortError {
    /// Keeps a port outcome as it is, maps a database failure through its
    /// SQLSTATE, and reports every other failure as
    /// [`PortError::Unavailable`] with the error's text.
    ///
    /// The failure is logged once here, as [`port_error`] logs it: a database
    /// failure through that mapping, any other failure but a port outcome as
    /// one `warn` event whose text, like every [`PersistenceError`] text,
    /// names no stored value. A port outcome passes through unlogged: the
    /// statement behind it was logged where it failed, and `NotFound` is never
    /// logged.
    ///
    /// The event's code location is the caller of `from`: the call site of `?`
    /// or of a direct call. A conversion passed as a function value, as in
    /// `map_err(PortError::from)`, reports the standard-library frame that
    /// invokes it; a [`PersistenceError::Corrupt`] text names its table and
    /// column instead.
    #[track_caller]
    fn from(err: PersistenceError) -> Self {
        match err {
            PersistenceError::Port(port) => port,
            PersistenceError::Sqlx(source) => port_error(&source),
            other => {
                let text = other.to_string();
                let location = Location::caller();
                tracing::warn!(
                    outcome = OUTCOME_UNAVAILABLE,
                    error = %text,
                    code.filepath = location.file(),
                    code.lineno = location.line(),
                    "persistence error reported as unavailable"
                );
                Self::Unavailable(text)
            }
        }
    }
}

/// The `outcome` of a failure event for [`PortError::Unavailable`].
const OUTCOME_UNAVAILABLE: &str = "Unavailable";

/// Maps a sqlx failure onto the outcome the COBOL programs branch on, and logs
/// it once.
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
///
/// Every outcome but `NotFound`, which the programs branch on as a normal
/// result, is logged as one `database statement failed` event at the caller's
/// code location ([`port_error_at`] lists its fields), so the cause of a
/// failure stays in the log however the rules and the adapters turn the
/// outcome into a COBOL message.
#[must_use]
#[track_caller]
pub(crate) fn port_error(err: &sqlx::Error) -> PortError {
    port_error_at(err, Location::caller())
}

/// [`port_error`] with the code location of the failed statement given
/// explicitly: `crate::tx::savepoint` and `crate::tx::settle` capture it where
/// a store method calls them and hand it on once the statement has run.
///
/// The event, at `warn` for [`PortError::Contention`] and
/// [`PortError::Unavailable`] and at `info` for [`PortError::Duplicate`] and
/// [`PortError::Restricted`], the expected COBOL outcomes `DUPREC`/-803 and
/// -532, has these fields:
///
/// - `sqlstate`: the SQLSTATE, when the server gave one;
/// - `outcome`: the [`PortError`] variant name;
/// - `error`: the sanitized text, for a database error its SQLSTATE and
///   primary message only, never the server's detail line, which can quote
///   row contents such as a social security number or a password hash; for
///   any other sqlx error its own text;
/// - `table` and `constraint`: the names the server reported, when it did;
/// - `code.filepath` and `code.lineno`: `location`.
#[must_use]
pub(crate) fn port_error_at(err: &sqlx::Error, location: &'static Location<'static>) -> PortError {
    match err {
        sqlx::Error::RowNotFound => PortError::NotFound,
        sqlx::Error::Database(db) => {
            let db = db.as_ref();
            let outcome = database_port_error(db);
            let failure = Failure {
                sqlstate: db.code(),
                outcome: &outcome,
                error: database_text(db),
                table: db.table(),
                constraint: db.constraint(),
                location,
            };
            failure.log();
            outcome
        }
        other => {
            let text = other.to_string();
            let outcome = PortError::Unavailable(text.clone());
            let failure = Failure {
                sqlstate: None,
                outcome: &outcome,
                error: text,
                table: None,
                constraint: None,
                location,
            };
            failure.log();
            outcome
        }
    }
}

/// The SQLSTATE half of [`port_error`]. The text of an unmapped error is
/// [`database_text`].
fn database_port_error(db: &dyn DatabaseError) -> PortError {
    match db.code().as_deref() {
        Some(UNIQUE_VIOLATION) => PortError::Duplicate,
        Some(FOREIGN_KEY_VIOLATION) => PortError::Restricted,
        Some(SERIALIZATION_FAILURE | DEADLOCK_DETECTED | LOCK_NOT_AVAILABLE) => {
            PortError::Contention
        }
        _ => PortError::Unavailable(database_text(db)),
    }
}

/// The sanitized text of a database error: its code and its primary message.
/// The server's detail line, which can quote row contents, is left out.
fn database_text(db: &dyn DatabaseError) -> String {
    match db.code() {
        Some(code) => format!("sqlstate {code}: {}", db.message()),
        None => format!("database error: {}", db.message()),
    }
}

/// One mapped statement failure, as [`port_error_at`] logs it.
struct Failure<'e> {
    /// The SQLSTATE the server gave, if any.
    sqlstate: Option<Cow<'e, str>>,
    /// The outcome the failure maps to.
    outcome: &'e PortError,
    /// The sanitized text: [`database_text`] or the sqlx error's own text.
    error: String,
    /// The table the server named, if any.
    table: Option<&'e str>,
    /// The constraint the server named, if any.
    constraint: Option<&'e str>,
    /// The code location of the failed statement.
    location: &'static Location<'static>,
}

impl Failure<'_> {
    /// Emits the one `database statement failed` event of the failure: `warn`
    /// for contention and unavailability, `info` for a duplicate key or a
    /// restricted delete, none for `NotFound`.
    fn log(&self) {
        let outcome = match self.outcome {
            PortError::NotFound => return,
            PortError::Duplicate => "Duplicate",
            PortError::Restricted => "Restricted",
            PortError::Contention => "Contention",
            PortError::Unavailable(_) => OUTCOME_UNAVAILABLE,
        };
        let sqlstate = self.sqlstate.as_deref();
        let file = self.location.file();
        let line = self.location.line();
        if matches!(self.outcome, PortError::Duplicate | PortError::Restricted) {
            tracing::info!(
                sqlstate,
                outcome,
                error = %self.error,
                table = self.table,
                constraint = self.constraint,
                code.filepath = file,
                code.lineno = line,
                "database statement failed"
            );
        } else {
            tracing::warn!(
                sqlstate,
                outcome,
                error = %self.error,
                table = self.table,
                constraint = self.constraint,
                code.filepath = file,
                code.lineno = line,
                "database statement failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::fmt;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, PoisonError};

    use carddemo_codec::{CodecError, CodecErrorKind};
    use carddemo_domain::auth::PasswordError;
    use sqlx::error::{DatabaseError, ErrorKind};
    use sqlx::migrate::MigrateError;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::Interest;
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::{PersistenceError, PortError, port_error};

    /// A database error with a chosen SQLSTATE, standing in for the server's.
    ///
    /// `detail` stands for the server's `DETAIL` line, which can quote row
    /// contents: the error's `Display` shows it, its primary `message` does
    /// not.
    #[derive(Debug)]
    struct ServerError {
        code: Option<&'static str>,
        message: &'static str,
        detail: Option<&'static str>,
        table: Option<&'static str>,
        constraint: Option<&'static str>,
    }

    impl fmt::Display for ServerError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.message)?;
            match self.detail {
                Some(detail) => write!(f, "\nDETAIL: {detail}"),
                None => Ok(()),
            }
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

        fn table(&self) -> Option<&str> {
            self.table
        }

        fn constraint(&self) -> Option<&str> {
            self.constraint
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
            detail: None,
            table: None,
            constraint: None,
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

    fn inexact_instant() -> PersistenceError {
        PersistenceError::InexactInstant {
            table: "reward_accruals",
            column: "created_at",
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
        let inexact_text = inexact_instant().to_string();
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
            (inexact_instant(), unavailable(&inexact_text)),
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
            (
                inexact_instant(),
                "reward_accruals.created_at cannot store the instant exactly: it is not a \
                 whole microsecond of UTC time",
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

    /// One event a [`Capture`] saw: its level and its fields, each as text.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Captured {
        level: Level,
        fields: BTreeMap<&'static str, String>,
    }

    /// A subscriber that keeps every event it is given. The crate depends on
    /// `tracing` but not on `tracing-subscriber`, so the tests write their own.
    #[derive(Clone, Default)]
    struct Capture {
        events: Arc<Mutex<Vec<Captured>>>,
    }

    /// The fields of one event, each recorded as its text.
    #[derive(Default)]
    struct FieldText(BTreeMap<&'static str, String>);

    impl Visit for FieldText {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name(), value.to_owned());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0.insert(field.name(), format!("{value:?}"));
        }
    }

    impl Subscriber for Capture {
        // Asked again for every event, so a callsite another test thread
        // registered without this subscriber still reaches it.
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::sometimes()
        }

        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _: &Id, _: &Record<'_>) {}

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut fields = FieldText::default();
            event.record(&mut fields);
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Captured {
                    level: *event.metadata().level(),
                    fields: fields.0,
                });
        }

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }

    /// Runs `f` with a fresh [`Capture`] as the thread's subscriber and
    /// returns its result with every event it logged.
    fn capturing<T>(f: impl FnOnce() -> T) -> (T, Vec<Captured>) {
        let capture = Capture::default();
        let value = tracing::subscriber::with_default(capture.clone(), f);
        let events = capture
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        (value, events)
    }

    /// The row a detail line quotes in [`reported_error`]: it must never reach
    /// a log line.
    const QUOTED_ROW: &str = "Key (cust_ssn)=(123456789) already exists.";

    /// A server error with `code`, a primary message, a detail line quoting a
    /// row, and the table and constraint names, as `PostgreSQL` reports them.
    fn reported_error(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(ServerError {
            code: Some(code),
            message: "statement failed on the server",
            detail: Some(QUOTED_ROW),
            table: Some("customers"),
            constraint: Some("customers_pkey"),
        }))
    }

    /// The fields of one failure event, the message included.
    fn event_fields(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs
            .iter()
            .map(|(name, value)| (*name, (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_server_error_is_logged_once_with_its_sqlstate_and_never_its_detail() {
        let cases = [
            ("42P01", Level::WARN, "Unavailable"),
            ("57014", Level::WARN, "Unavailable"),
            ("23505", Level::INFO, "Duplicate"),
            ("23503", Level::INFO, "Restricted"),
            ("40001", Level::WARN, "Contention"),
            ("40P01", Level::WARN, "Contention"),
            ("55P03", Level::WARN, "Contention"),
        ];
        for (code, level, outcome) in cases {
            let err = reported_error(code);
            let ((mapped, events), line) = (capturing(|| port_error(&err)), line!());
            let text = format!("sqlstate {code}: statement failed on the server");
            if outcome == "Unavailable" {
                assert_eq!(mapped, unavailable(&text), "{code}");
            }
            let lineno = line.to_string();
            let expected = Captured {
                level,
                fields: event_fields(&[
                    ("message", "database statement failed"),
                    ("sqlstate", code),
                    ("outcome", outcome),
                    ("error", &text),
                    ("table", "customers"),
                    ("constraint", "customers_pkey"),
                    ("code.filepath", file!()),
                    ("code.lineno", &lineno),
                ]),
            };
            assert_eq!(events, [expected], "{code}");
            assert!(
                events[0]
                    .fields
                    .values()
                    .all(|value| !value.contains("123456789")),
                "{code}: the detail line reached the event: {events:?}"
            );
        }
    }

    #[test]
    fn other_sqlx_failures_are_logged_with_their_text_and_not_found_is_not() {
        let cases = [
            (sqlx::Error::PoolTimedOut, Some(Level::WARN)),
            (
                sqlx::Error::Protocol("unexpected message".to_owned()),
                Some(Level::WARN),
            ),
            (server_error(None), Some(Level::WARN)),
            (sqlx::Error::RowNotFound, None),
        ];
        for (err, level) in cases {
            let ((mapped, events), line) = (capturing(|| port_error(&err)), line!());
            let Some(level) = level else {
                assert_eq!(mapped, PortError::NotFound);
                assert_eq!(events, [], "{err}");
                continue;
            };
            let PortError::Unavailable(text) = mapped else {
                panic!("{err} mapped to {mapped:?}, not Unavailable");
            };
            let lineno = line.to_string();
            let expected = Captured {
                level,
                fields: event_fields(&[
                    ("message", "database statement failed"),
                    ("outcome", "Unavailable"),
                    ("error", &text),
                    ("code.filepath", file!()),
                    ("code.lineno", &lineno),
                ]),
            };
            assert_eq!(events, [expected], "{err}");
        }
    }

    #[test]
    fn a_conversion_logs_each_failure_once_and_a_port_outcome_not_at_all() {
        let unavailable_event = Some((Level::WARN, "persistence error reported as unavailable"));
        let statement_event = |level| Some((level, "database statement failed"));
        let cases = [
            (corrupt(), unavailable_event),
            (codec_failure(), unavailable_event),
            (
                PersistenceError::Migrate(MigrateError::VersionMissing(3)),
                unavailable_event,
            ),
            (
                PersistenceError::Sqlx(reported_error("42P01")),
                statement_event(Level::WARN),
            ),
            (
                PersistenceError::Sqlx(reported_error("23505")),
                statement_event(Level::INFO),
            ),
            (PersistenceError::Sqlx(sqlx::Error::RowNotFound), None),
            (PersistenceError::Port(PortError::Duplicate), None),
            (PersistenceError::Port(unavailable("pool closed")), None),
        ];
        for (err, expected) in cases {
            let text = err.to_string();
            let ((mapped, events), line) = (capturing(|| PortError::from(err)), line!());
            let Some((level, message)) = expected else {
                assert_eq!(events, [], "{text}");
                continue;
            };
            assert_eq!(events.len(), 1, "{text}: {events:?}");
            let event = &events[0];
            assert_eq!(event.level, level, "{text}");
            let field = |name: &str| event.fields.get(name).map(String::as_str);
            assert_eq!(field("message"), Some(message), "{text}");
            assert_eq!(field("code.filepath"), Some(file!()), "{text}");
            assert_eq!(
                field("code.lineno"),
                Some(line.to_string().as_str()),
                "{text}"
            );
            if message == "persistence error reported as unavailable" {
                assert_eq!(mapped, unavailable(&text), "{text}");
                assert_eq!(field("outcome"), Some("Unavailable"), "{text}");
                assert_eq!(field("error"), Some(text.as_str()), "{text}");
            }
        }
    }
}
