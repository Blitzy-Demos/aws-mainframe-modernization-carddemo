//! Typed persistence errors that carry no stored value.
//!
//! Every repository method in `repos` and every codec B conversion in `rows` returns
//! `Result<_, PersistenceError>`. Callers in `api::services`, `messaging::workers::cp00` and
//! `batch::jobs` branch on the variant to take the outcome the COBOL program takes for the same
//! file response, SQL code or DL/I status:
//!
//! - [`PersistenceError::Duplicate`], SQLSTATE `23505` (`unique_violation`): CICS `DUPKEY` and
//!   `DUPREC` on a `WRITE` (`COUSR01C.cbl:260-261`, COUSR01C-023; `COTRN02C.cbl:735-736`,
//!   COTRN02C-050; `COBIL00C.cbl:533-534`, COBIL00C-029), Db2 `SQLCODE` -803, on which COPAUS2C
//!   updates the fraud row instead of inserting it (`COPAUS2C.cbl:203-204`, COPAUS2C-008), and
//!   the IMS `II` status of CP00's detail insert, its `I004` path (`COPAUA0C.cbl:913-931`,
//!   COPAUA0C-099).
//! - [`PersistenceError::Restrict`], SQLSTATE `23503` (`foreign_key_violation`, which
//!   `PostgreSQL` raises for `ON DELETE RESTRICT`) or `23001` (`restrict_violation`): Db2
//!   `SQLCODE` -532, deleting a transaction type that still has categories through foreign key
//!   `trc_type_code` of `db2.transaction_type_category` (migration 0006, `TRNTYCAT.ddl:6-7`;
//!   `COTRTUPC.cbl:1638-1649`, COTRTUPC-033; COTRTLIC-050).
//! - [`PersistenceError::NotFound`]: CICS `NOTFND` and Db2 `SQLCODE` +100 when a `fetch_one`
//!   finds no row. A repository method whose caller branches on a missing key returns `Ok(None)`
//!   or `false` instead, so this variant marks a row the caller required.
//! - [`PersistenceError::Check`], SQLSTATE `23514`: a `BIGINT` range check of a `9(n)` or
//!   `S9(n)` column rejected a value.
//! - [`PersistenceError::Database`]: every other database error, the `WHEN OTHER` branches of
//!   the programs. Its SQLSTATE tells a lock failure (`40P01`, `55P03`, the Db2 -911 of
//!   COTRTUPC) or a serialization failure (`40001`) apart from the rest.
//! - [`PersistenceError::Unavailable`]: the database cannot be reached.
//! - [`PersistenceError::Decode`]: codec B met a stored value that does not fit its picture, or
//!   a record value it cannot store; [`DecodeKind`] names the class.
//!
//! # Redaction
//!
//! A `PostgreSQL` message or detail text can quote the key values of the failing row, such as a
//! full card number in `Key (card_num)=(…) already exists`, and the text of a column decode
//! failure can quote the value. Conversion from [`sqlx::Error`] therefore keeps only the
//! SQLSTATE and the constraint name, both schema identifiers, and never reads the message, the
//! detail or the `Display` and `Debug` output of the `sqlx` error, which is dropped rather than
//! kept as a source. No variant has a field that could hold a stored value, a bind parameter or
//! a message text, so neither `Display` nor `Debug` of this type can reproduce a card number,
//! SSN, CVV or password (Cardholder Data Redaction). Keep it that way: no variant may gain such
//! a payload.

use std::fmt;

use sqlx::error::DatabaseError;

/// SQLSTATE `unique_violation`.
const UNIQUE_VIOLATION: &str = "23505";
/// SQLSTATE `foreign_key_violation`, raised for `ON DELETE RESTRICT` and `NO ACTION` alike.
const FOREIGN_KEY_VIOLATION: &str = "23503";
/// SQLSTATE `restrict_violation`.
const RESTRICT_VIOLATION: &str = "23001";
/// SQLSTATE `check_violation`.
const CHECK_VIOLATION: &str = "23514";

/// A failed repository call or codec B conversion, holding no stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceError {
    /// A statement that must return a row returned none ([`sqlx::Error::RowNotFound`]).
    NotFound,
    /// SQLSTATE `23505`: a primary key or unique index already holds the key.
    Duplicate {
        /// Name of the violated constraint or index, such as `users_pkey`.
        constraint: Option<String>,
    },
    /// SQLSTATE `23503` or `23001`: a foreign key refused the change, such as deleting a
    /// `db2.transaction_type` row that `db2.transaction_type_category` still references through
    /// `trc_type_code`.
    Restrict {
        /// Name of the foreign key, such as `trc_type_code`.
        constraint: Option<String>,
    },
    /// SQLSTATE `23514`: a check constraint, such as a `BIGINT` picture range, refused a value.
    Check {
        /// Name of the check constraint.
        constraint: Option<String>,
    },
    /// Any other database error, or a `sqlx` failure that is not a connection failure (column
    /// decode or encode, configuration, argument or driver errors).
    Database {
        /// The SQLSTATE the server reported; `None` for a failure inside `sqlx`.
        sqlstate: Option<String>,
        /// Name of the constraint the server reported, if any.
        constraint: Option<String>,
    },
    /// The database cannot be reached: the pool timed out or is closed, or the connection
    /// failed (I/O, TLS, protocol) or its worker crashed.
    Unavailable,
    /// Codec B found a stored value that does not fit its picture, or a record value that
    /// cannot be stored.
    Decode {
        /// The column the value belongs to, such as `acct_id`.
        column: &'static str,
        /// The class of the failure.
        kind: DecodeKind,
    },
}

/// The class of a codec B failure. Like [`PersistenceError`], it never holds the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeKind {
    /// An integer or decimal outside the range or scale of its `9(n)`, `S9(n)` or `S9(n)V99`
    /// picture.
    OutOfRange,
    /// A `CHAR(n)` or `VARCHAR(n)` value whose byte length does not fit the picture width.
    WidthMismatch {
        /// Width the picture declares, in bytes.
        expected: usize,
        /// Byte length of the value.
        actual: usize,
    },
    /// A non-ASCII or NUL byte. Every fixture byte is ASCII, so this is an anomaly to escalate,
    /// never a value to normalise (R7).
    InvalidByte,
    /// A `<col>_form` layout code that `copybook::TsForm::from_code` rejects.
    UnknownForm,
    /// A `TIMESTAMP(6)` value and its `<col>_form` code disagree: one is present without the
    /// other, or the layout cannot reproduce the value.
    FormMismatch,
    /// A key that must be all digits is not, such as a highest `tran_id` that is not 16 digits
    /// when `TransactionRepository::next_id` allocates the next one.
    NotNumeric,
    /// A `copybook` or `domain` constructor rejected the value; [`copybook::ErrorKind`] never
    /// holds bytes.
    Copybook(copybook::ErrorKind),
}

impl PersistenceError {
    /// A [`PersistenceError::Decode`] for `column`.
    // Public rather than crate-only: the variant it builds is public, so this grants nothing
    // new, and a crate-only constructor that no row conversion calls is dead code, which
    // `warnings = "deny"` makes a build error that no lint attribute may silence (R15).
    #[must_use]
    pub const fn decode(column: &'static str, kind: DecodeKind) -> Self {
        Self::Decode { column, kind }
    }

    /// Whether a required row was missing (CICS `NOTFND`, Db2 +100).
    #[must_use]
    pub const fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }

    /// Whether the key already existed (CICS `DUPKEY`/`DUPREC`, Db2 -803, IMS `II`).
    #[must_use]
    pub const fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }

    /// Whether a foreign key refused the change (Db2 -532).
    #[must_use]
    pub const fn is_restrict(&self) -> bool {
        matches!(self, Self::Restrict { .. })
    }

    /// The SQLSTATE behind the error, where it is known exactly: the stored code of
    /// [`PersistenceError::Database`], `23505` for [`PersistenceError::Duplicate`] and `23514`
    /// for [`PersistenceError::Check`].
    #[must_use]
    pub fn sqlstate(&self) -> Option<&str> {
        match self {
            Self::Duplicate { .. } => Some(UNIQUE_VIOLATION),
            Self::Check { .. } => Some(CHECK_VIOLATION),
            Self::Database { sqlstate, .. } => sqlstate.as_deref(),
            // `Restrict` stands for two codes and does not record which one was raised.
            Self::NotFound | Self::Restrict { .. } | Self::Unavailable | Self::Decode { .. } => {
                None
            }
        }
    }

    /// The name of the constraint the server reported, if any.
    #[must_use]
    pub fn constraint(&self) -> Option<&str> {
        match self {
            Self::Duplicate { constraint }
            | Self::Restrict { constraint }
            | Self::Check { constraint }
            | Self::Database { constraint, .. } => constraint.as_deref(),
            Self::NotFound | Self::Unavailable | Self::Decode { .. } => None,
        }
    }

    /// Classifies a server error by its SQLSTATE, copying only the code and the constraint name.
    fn from_database(database: &dyn DatabaseError) -> Self {
        let constraint = database.constraint().map(str::to_owned);
        let code = database.code();
        match code.as_deref() {
            Some(UNIQUE_VIOLATION) => Self::Duplicate { constraint },
            Some(FOREIGN_KEY_VIOLATION | RESTRICT_VIOLATION) => Self::Restrict { constraint },
            Some(CHECK_VIOLATION) => Self::Check { constraint },
            other => Self::Database {
                sqlstate: other.map(str::to_owned),
                constraint,
            },
        }
    }
}

impl From<sqlx::Error> for PersistenceError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(database) => Self::from_database(&*database),
            sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::Protocol(_)
            | sqlx::Error::WorkerCrashed => Self::Unavailable,
            // Column decode and encode, configuration, argument and driver failures, and every
            // variant a feature or a later release adds (`sqlx::Error` is non-exhaustive). Their
            // texts can quote values, so nothing of them is kept.
            _ => Self::Database {
                sqlstate: None,
                constraint: None,
            },
        }
    }
}

impl From<copybook::ErrorKind> for DecodeKind {
    /// Wraps the class unchanged, so `?` and `map_err` keep the exact `copybook` class.
    fn from(kind: copybook::ErrorKind) -> Self {
        Self::Copybook(kind)
    }
}

impl fmt::Display for DecodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange => f.write_str("out of range"),
            Self::WidthMismatch { expected, actual } => {
                write!(
                    f,
                    "width mismatch: expected {expected} bytes, found {actual}"
                )
            }
            Self::InvalidByte => f.write_str("non-ASCII or NUL byte"),
            Self::UnknownForm => f.write_str("unknown timestamp layout code"),
            Self::FormMismatch => f.write_str("timestamp value and layout code disagree"),
            Self::NotNumeric => f.write_str("not all digits"),
            Self::Copybook(kind) => kind.fmt(f),
        }
    }
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("row not found"),
            Self::Duplicate { constraint } => {
                with_constraint(f, "duplicate key", constraint.as_deref())
            }
            Self::Restrict { constraint } => {
                with_constraint(f, "restricted by foreign key", constraint.as_deref())
            }
            Self::Check { constraint } => {
                with_constraint(f, "check constraint violated", constraint.as_deref())
            }
            Self::Database {
                sqlstate,
                constraint,
            } => match (sqlstate, constraint) {
                (Some(code), Some(name)) => {
                    write!(f, "database error (SQLSTATE {code}, constraint {name})")
                }
                (Some(code), None) => write!(f, "database error (SQLSTATE {code})"),
                (None, Some(name)) => write!(f, "database error (constraint {name})"),
                (None, None) => f.write_str("database error"),
            },
            Self::Unavailable => f.write_str("database unavailable"),
            Self::Decode { column, kind } => {
                write!(
                    f,
                    "stored value in column {column} does not fit its picture: {kind}"
                )
            }
        }
    }
}

/// Writes `text`, followed by the constraint name in parentheses when there is one.
fn with_constraint(
    f: &mut fmt::Formatter<'_>,
    text: &str,
    constraint: Option<&str>,
) -> fmt::Result {
    match constraint {
        Some(name) => write!(f, "{text} (constraint {name})"),
        None => f.write_str(text),
    }
}

impl std::error::Error for PersistenceError {}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error as StdError;
    use std::fmt;
    use std::io;

    use copybook::{ErrorKind, TsForm};
    use sqlx::error::{DatabaseError, ErrorKind as SqlxErrorKind};

    use super::{DecodeKind, PersistenceError};

    /// The first `cardxref.txt` card number.
    const CARD_NUMBER: &str = "0500024453765740";
    /// What a server message or detail, or a `sqlx` decode text, can say about a failing row.
    const DETAIL: &str =
        "duplicate key: Key (card_num)=(0500024453765740) already exists; SSN 123-45-6789";

    /// A server error whose every text quotes [`DETAIL`].
    #[derive(Debug)]
    struct ServerError {
        code: Option<&'static str>,
        constraint: Option<&'static str>,
    }

    impl fmt::Display for ServerError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(DETAIL)
        }
    }

    impl StdError for ServerError {}

    impl DatabaseError for ServerError {
        fn message(&self) -> &str {
            DETAIL
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            self.code.map(Cow::Borrowed)
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn constraint(&self) -> Option<&str> {
            self.constraint
        }

        fn kind(&self) -> SqlxErrorKind {
            SqlxErrorKind::Other
        }
    }

    fn server(code: Option<&'static str>, constraint: Option<&'static str>) -> sqlx::Error {
        sqlx::Error::Database(Box::new(ServerError { code, constraint }))
    }

    /// Every `sqlx` failure class the conversion distinguishes, each quoting [`DETAIL`] wherever
    /// it can carry text, with the variant it must become.
    fn conversions() -> Vec<(sqlx::Error, PersistenceError)> {
        let unavailable = PersistenceError::Unavailable;
        let internal = PersistenceError::Database {
            sqlstate: None,
            constraint: None,
        };
        vec![
            (sqlx::Error::RowNotFound, PersistenceError::NotFound),
            (
                server(Some("23505"), Some("users_pkey")),
                PersistenceError::Duplicate {
                    constraint: Some("users_pkey".to_owned()),
                },
            ),
            (
                server(Some("23505"), None),
                PersistenceError::Duplicate { constraint: None },
            ),
            (
                server(Some("23503"), Some("trc_type_code")),
                PersistenceError::Restrict {
                    constraint: Some("trc_type_code".to_owned()),
                },
            ),
            (
                server(Some("23001"), Some("trc_type_code")),
                PersistenceError::Restrict {
                    constraint: Some("trc_type_code".to_owned()),
                },
            ),
            (
                server(Some("23514"), Some("accounts_acct_id_check")),
                PersistenceError::Check {
                    constraint: Some("accounts_acct_id_check".to_owned()),
                },
            ),
            (
                server(Some("23502"), Some("users_pkey")),
                PersistenceError::Database {
                    sqlstate: Some("23502".to_owned()),
                    constraint: Some("users_pkey".to_owned()),
                },
            ),
            (
                server(Some("40P01"), None),
                PersistenceError::Database {
                    sqlstate: Some("40P01".to_owned()),
                    constraint: None,
                },
            ),
            (
                server(Some("55P03"), None),
                PersistenceError::Database {
                    sqlstate: Some("55P03".to_owned()),
                    constraint: None,
                },
            ),
            (
                server(Some("40001"), None),
                PersistenceError::Database {
                    sqlstate: Some("40001".to_owned()),
                    constraint: None,
                },
            ),
            (server(None, None), internal.clone()),
            (sqlx::Error::PoolTimedOut, unavailable.clone()),
            (sqlx::Error::PoolClosed, unavailable.clone()),
            (sqlx::Error::WorkerCrashed, unavailable.clone()),
            (
                sqlx::Error::Io(io::Error::new(io::ErrorKind::ConnectionRefused, DETAIL)),
                unavailable.clone(),
            ),
            (sqlx::Error::Tls(DETAIL.into()), unavailable.clone()),
            (sqlx::Error::Protocol(DETAIL.to_owned()), unavailable),
            (sqlx::Error::Configuration(DETAIL.into()), internal.clone()),
            (
                sqlx::Error::InvalidArgument(DETAIL.to_owned()),
                internal.clone(),
            ),
            (
                sqlx::Error::TypeNotFound {
                    type_name: DETAIL.to_owned(),
                },
                internal.clone(),
            ),
            (
                sqlx::Error::ColumnNotFound(DETAIL.to_owned()),
                internal.clone(),
            ),
            (
                sqlx::Error::ColumnDecode {
                    index: DETAIL.to_owned(),
                    source: DETAIL.into(),
                },
                internal.clone(),
            ),
            (sqlx::Error::Encode(DETAIL.into()), internal.clone()),
            (sqlx::Error::Decode(DETAIL.into()), internal.clone()),
            (sqlx::Error::BeginFailed, internal),
        ]
    }

    #[test]
    fn sqlx_errors_convert_by_sqlstate_and_class() {
        for (error, expected) in conversions() {
            assert_eq!(PersistenceError::from(error), expected);
        }
    }

    /// A server or `sqlx` text that quotes a card number or SSN never reaches the converted
    /// error, which has no source to reach it through either (Cardholder Data Redaction).
    #[test]
    fn no_rendering_reproduces_the_failing_row() {
        for (error, _) in conversions() {
            let converted = PersistenceError::from(error);
            for text in [converted.to_string(), format!("{converted:?}")] {
                assert!(!text.contains(CARD_NUMBER), "{text}");
                assert!(!text.contains("6789"), "{text}");
                assert!(!text.contains("Key ("), "{text}");
                assert!(!text.contains("already exists"), "{text}");
            }
            let boxed: Box<dyn StdError> = Box::new(converted);
            assert!(boxed.source().is_none());
        }
    }

    #[test]
    fn display_is_fixed_text_with_only_sqlstate_constraint_and_column() {
        let cases = [
            (PersistenceError::NotFound, "row not found"),
            (
                PersistenceError::Duplicate {
                    constraint: Some("users_pkey".to_owned()),
                },
                "duplicate key (constraint users_pkey)",
            ),
            (
                PersistenceError::Duplicate { constraint: None },
                "duplicate key",
            ),
            (
                PersistenceError::Restrict {
                    constraint: Some("trc_type_code".to_owned()),
                },
                "restricted by foreign key (constraint trc_type_code)",
            ),
            (
                PersistenceError::Restrict { constraint: None },
                "restricted by foreign key",
            ),
            (
                PersistenceError::Check {
                    constraint: Some("accounts_acct_id_check".to_owned()),
                },
                "check constraint violated (constraint accounts_acct_id_check)",
            ),
            (
                PersistenceError::Check { constraint: None },
                "check constraint violated",
            ),
            (
                PersistenceError::Database {
                    sqlstate: Some("40P01".to_owned()),
                    constraint: Some("xtran_type".to_owned()),
                },
                "database error (SQLSTATE 40P01, constraint xtran_type)",
            ),
            (
                PersistenceError::Database {
                    sqlstate: Some("40001".to_owned()),
                    constraint: None,
                },
                "database error (SQLSTATE 40001)",
            ),
            (
                PersistenceError::Database {
                    sqlstate: None,
                    constraint: Some("xtran_type".to_owned()),
                },
                "database error (constraint xtran_type)",
            ),
            (
                PersistenceError::Database {
                    sqlstate: None,
                    constraint: None,
                },
                "database error",
            ),
            (PersistenceError::Unavailable, "database unavailable"),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn decode_names_the_column_and_the_class() {
        let cases = [
            (
                DecodeKind::OutOfRange,
                "stored value in column acct_id does not fit its picture: out of range",
            ),
            (
                DecodeKind::WidthMismatch {
                    expected: 16,
                    actual: 15,
                },
                "stored value in column acct_id does not fit its picture: \
                 width mismatch: expected 16 bytes, found 15",
            ),
            (
                DecodeKind::InvalidByte,
                "stored value in column acct_id does not fit its picture: non-ASCII or NUL byte",
            ),
            (
                DecodeKind::UnknownForm,
                "stored value in column acct_id does not fit its picture: \
                 unknown timestamp layout code",
            ),
            (
                DecodeKind::FormMismatch,
                "stored value in column acct_id does not fit its picture: \
                 timestamp value and layout code disagree",
            ),
            (
                DecodeKind::NotNumeric,
                "stored value in column acct_id does not fit its picture: not all digits",
            ),
            (
                DecodeKind::Copybook(ErrorKind::NonDigit),
                "stored value in column acct_id does not fit its picture: \
                 non-digit byte in a numeric field",
            ),
        ];
        for (kind, text) in cases {
            let error = PersistenceError::decode("acct_id", kind);
            assert_eq!(
                error,
                PersistenceError::Decode {
                    column: "acct_id",
                    kind
                }
            );
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn copybook_classes_convert_unchanged() {
        let rejected = TsForm::from_code('X').map_err(DecodeKind::from);
        assert_eq!(
            rejected,
            Err(DecodeKind::Copybook(ErrorKind::UnknownFormCode))
        );
        assert_eq!(
            DecodeKind::from(ErrorKind::OutOfRange),
            DecodeKind::Copybook(ErrorKind::OutOfRange)
        );
    }

    #[test]
    fn predicates_and_accessors_report_only_what_is_known() {
        let cases = [
            (PersistenceError::NotFound, (true, false, false), None, None),
            (
                PersistenceError::Duplicate {
                    constraint: Some("users_pkey".to_owned()),
                },
                (false, true, false),
                Some("23505"),
                Some("users_pkey"),
            ),
            (
                PersistenceError::Restrict {
                    constraint: Some("trc_type_code".to_owned()),
                },
                (false, false, true),
                None,
                Some("trc_type_code"),
            ),
            (
                PersistenceError::Check { constraint: None },
                (false, false, false),
                Some("23514"),
                None,
            ),
            (
                PersistenceError::Database {
                    sqlstate: Some("55P03".to_owned()),
                    constraint: Some("xtran_type".to_owned()),
                },
                (false, false, false),
                Some("55P03"),
                Some("xtran_type"),
            ),
            (
                PersistenceError::Unavailable,
                (false, false, false),
                None,
                None,
            ),
            (
                PersistenceError::decode("tran_id", DecodeKind::NotNumeric),
                (false, false, false),
                None,
                None,
            ),
        ];
        for (error, flags, sqlstate, constraint) in cases {
            assert_eq!(
                (
                    error.is_not_found(),
                    error.is_duplicate(),
                    error.is_restrict()
                ),
                flags,
                "{error:?}"
            );
            assert_eq!(error.sqlstate(), sqlstate, "{error:?}");
            assert_eq!(error.constraint(), constraint, "{error:?}");
        }
    }
}
