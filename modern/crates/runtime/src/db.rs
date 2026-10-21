//! `PostgreSQL` connection pool of the `carddemo-api`, `carddemo-mq`, `carddemo-batch` and
//! `carddemo-migrate` binaries.
//!
//! [`connect`] opens a [`PgPool`] on [`Config::database_url`]. Its default names the local
//! `PostgreSQL` 16 service of `modern/compose.yaml`, and an integration test hands a binary its
//! testcontainers instance through `DATABASE_URL`. The `sqlx` feature set has no TLS backend, so
//! every connection is unencrypted, and a URL that requires TLS (`sslmode` `require`,
//! `verify-ca` or `verify-full`) fails to connect. This module does not restrict which host a
//! URL names.
//!
//! Connecting never applies migrations. The schema reaches the database only through the
//! `apply` function in the `migrate` module (`crate::migrate`), run by the `carddemo-migrate`
//! binary or by the test harness. A service started against an unmigrated database therefore
//! connects and then fails at its first query.
//!
//! # Credential-safe diagnostics
//!
//! The URL carries the database password, so nothing here logs, traces or formats it. `sqlx`
//! logs every query parameter it does not recognize together with its value, so [`connect`]
//! refuses such a parameter before `sqlx` sees the URL. Every configuration error [`connect`]
//! returns names `DATABASE_URL` and never quotes the URL. Callers log a `sqlx` error only through
//! [`ErrorSummary`], which keeps an error class and, for a database error, the SQLSTATE, and
//! never the driver's or the server's text: that text can quote a rejected option value or a key
//! value.

use std::error::Error as StdError;
use std::fmt;
use std::io;

use crate::config::Config;
use sqlx::error::DatabaseError;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

/// The query parameter names that the `sqlx` 0.8.6 `PostgreSQL` URL parser recognizes, besides
/// the `options[<name>]` form. That parser logs any other parameter at `WARN` together with its
/// value.
const RECOGNIZED_PARAMETERS: [&str; 18] = [
    "sslmode",
    "ssl-mode",
    "sslrootcert",
    "ssl-root-cert",
    "ssl-ca",
    "sslcert",
    "ssl-cert",
    "sslkey",
    "ssl-key",
    "statement-cache-capacity",
    "host",
    "hostaddr",
    "port",
    "dbname",
    "user",
    "password",
    "application_name",
    "options",
];

/// Opening of an `options[<name>]` parameter, which sets one server setting.
const OPTIONS_PREFIX: &str = "options[";

/// Closing of an `options[<name>]` parameter.
const OPTIONS_SUFFIX: &str = "]";

/// Opens a [`PgPool`] on [`Config::database_url`] with the default `sqlx` pool settings, and
/// establishes one connection before returning, so a wrong URL, an unreachable server or rejected
/// credentials surface at startup rather than at the first query.
///
/// The URL's query parameter names are checked before `sqlx` parses it: each must be one that
/// `sqlx` recognizes, compared exactly as written, so a percent-encoded or `+`-encoded name is
/// refused even when it decodes to a recognized one.
///
/// # Errors
///
/// - [`sqlx::Error::Configuration`] when the URL holds a query parameter that `sqlx` does not
///   recognize, or is not a valid `PostgreSQL` connection URL, for example because a recognized
///   parameter holds a value `sqlx` rejects. Its text names `DATABASE_URL` and at most the
///   position of the offending parameter, and never quotes the URL or any part of it.
/// - [`sqlx::Error::Io`] or [`sqlx::Error::PoolTimedOut`] when the server cannot be reached
///   within the pool's acquire timeout.
/// - [`sqlx::Error::Tls`] when the URL requires TLS, which this build cannot provide.
/// - [`sqlx::Error::Database`] when the server rejects the connection, for example on an
///   authentication failure or an unknown database.
pub async fn connect(config: &Config) -> Result<PgPool, sqlx::Error> {
    let options = connect_options(&config.database_url)?;
    PgPoolOptions::new().connect_with(options).await
}

/// Parses `url` into connect options after [`check_query_parameters`] has accepted it, and
/// replaces any parse error of `sqlx` with [`UrlError::Invalid`]: the `sqlx` text can quote a
/// rejected value, such as an unknown `sslmode`.
fn connect_options(url: &str) -> Result<PgConnectOptions, UrlError> {
    check_query_parameters(url)?;
    url.parse::<PgConnectOptions>()
        .map_err(|_| UrlError::Invalid)
}

/// Refuses the first query parameter of `url` whose name `sqlx` does not recognize.
///
/// The query is located as the URL standard locates it, the text after the first `?` that
/// precedes the first `#`, and split as `sqlx` splits it: on `&`, with empty pieces skipped and
/// each name ending at its first `=`. Names are compared as written, without decoding. Positions
/// are 1-based and count the non-empty pieces only.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn check_query_parameters(url: &str) -> Result<(), UrlError> {
    let before_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let Some((_, query)) = before_fragment.split_once('?') else {
        return Ok(());
    };
    let parameters = query.split('&').filter(|parameter| !parameter.is_empty());
    for (position, parameter) in (1_usize..).zip(parameters) {
        let name = parameter
            .split_once('=')
            .map_or(parameter, |(name, _)| name);
        if !is_recognized(name) {
            return Err(UrlError::UnrecognizedParameter(position));
        }
    }
    Ok(())
}

/// Whether `sqlx` recognizes the query parameter `name`: one of [`RECOGNIZED_PARAMETERS`], or
/// `options[<name>]` with a non-empty setting name.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn is_recognized(name: &str) -> bool {
    RECOGNIZED_PARAMETERS.contains(&name)
        || name
            .strip_prefix(OPTIONS_PREFIX)
            .and_then(|rest| rest.strip_suffix(OPTIONS_SUFFIX))
            .is_some_and(|setting| !setting.is_empty())
}

/// Why `DATABASE_URL` was refused before any connection attempt.
///
/// Its text names the variable and at most a parameter's position, never a name or a value from
/// the URL: a malformed URL can put password text anywhere in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlError {
    /// The query parameter at this 1-based position is not one `sqlx` recognizes.
    UnrecognizedParameter(usize),
    /// `sqlx` cannot parse the URL, or a recognized parameter holds a value it rejects.
    Invalid,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnrecognizedParameter(position) => write!(
                f,
                "DATABASE_URL query parameter {position} is not a recognized PostgreSQL \
                 connection parameter"
            ),
            Self::Invalid => f.write_str("DATABASE_URL is not a valid PostgreSQL connection URL"),
        }
    }
}

impl StdError for UrlError {}

impl From<UrlError> for sqlx::Error {
    fn from(error: UrlError) -> Self {
        Self::Configuration(Box::new(error))
    }
}

/// A value-free account of a [`sqlx::Error`], the only form in which a caller logs one.
///
/// Its `Display` is a fixed error class, such as `database` or `pool_timed_out`, followed for a
/// database error by its SQLSTATE (`database (SQLSTATE 28P01)`), for an I/O error by its
/// [`io::ErrorKind`] and for a configuration error that [`connect`] raised by its reason, which
/// names `DATABASE_URL` only. It never keeps the driver's or the server's text, which can quote
/// the connection URL, a rejected option value or a key value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorSummary {
    /// Stable name of the error variant.
    class: &'static str,
    /// What the class may add without quoting a value.
    detail: Option<SummaryDetail>,
}

/// The value-free addition of an [`ErrorSummary`] to its class.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SummaryDetail {
    /// The SQLSTATE of a database error: five ASCII digits or uppercase letters.
    Sqlstate(String),
    /// The kind of an I/O error.
    IoKind(io::ErrorKind),
    /// The reason [`connect`] refused `DATABASE_URL`.
    Url(UrlError),
}

impl From<&sqlx::Error> for ErrorSummary {
    fn from(error: &sqlx::Error) -> Self {
        let (class, detail) = match error {
            sqlx::Error::Configuration(source) => (
                "configuration",
                source
                    .downcast_ref::<UrlError>()
                    .map(|reason| SummaryDetail::Url(*reason)),
            ),
            sqlx::Error::Database(database) => (
                "database",
                sqlstate(&**database).map(SummaryDetail::Sqlstate),
            ),
            sqlx::Error::Io(io) => ("io", Some(SummaryDetail::IoKind(io.kind()))),
            sqlx::Error::InvalidArgument(_) => ("invalid_argument", None),
            sqlx::Error::Tls(_) => ("tls", None),
            sqlx::Error::Protocol(_) => ("protocol", None),
            sqlx::Error::RowNotFound => ("row_not_found", None),
            sqlx::Error::TypeNotFound { .. } => ("type_not_found", None),
            sqlx::Error::ColumnIndexOutOfBounds { .. } => ("column_index_out_of_bounds", None),
            sqlx::Error::ColumnNotFound(_) => ("column_not_found", None),
            sqlx::Error::ColumnDecode { .. } => ("column_decode", None),
            sqlx::Error::Encode(_) => ("encode", None),
            sqlx::Error::Decode(_) => ("decode", None),
            sqlx::Error::AnyDriverError(_) => ("any_driver", None),
            sqlx::Error::PoolTimedOut => ("pool_timed_out", None),
            sqlx::Error::PoolClosed => ("pool_closed", None),
            sqlx::Error::WorkerCrashed => ("worker_crashed", None),
            sqlx::Error::Migrate(_) => ("migrate", None),
            sqlx::Error::InvalidSavePointStatement => ("invalid_savepoint_statement", None),
            sqlx::Error::BeginFailed => ("begin_failed", None),
            // `sqlx::Error` is non-exhaustive: a variant added later keeps its text out too.
            _ => ("other", None),
        };
        Self { class, detail }
    }
}

impl fmt::Display for ErrorSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.class)?;
        match &self.detail {
            None => Ok(()),
            Some(SummaryDetail::Sqlstate(code)) => write!(f, " (SQLSTATE {code})"),
            Some(SummaryDetail::IoKind(kind)) => write!(f, " ({kind})"),
            Some(SummaryDetail::Url(reason)) => write!(f, ": {reason}"),
        }
    }
}

/// The SQLSTATE of `error`, kept only when it has the five-character SQLSTATE shape, so that a
/// malformed code from a server cannot carry text into a log.
fn sqlstate(error: &dyn DatabaseError) -> Option<String> {
    error
        .code()
        .filter(|code| {
            code.len() == 5
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
        })
        .map(std::borrow::Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error as StdError;
    use std::fmt;
    use std::io;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;

    use sqlx::error::{DatabaseError, ErrorKind as DatabaseErrorKind};
    use sqlx::migrate::MigrateError;
    use sqlx::postgres::{PgConnectOptions, PgPool};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{ErrorSummary, RECOGNIZED_PARAMETERS, UrlError, check_query_parameters, connect};
    use crate::config::Config;
    use crate::telemetry::json_layer;

    /// Stand-in for a credential that a mistyped `DATABASE_URL` carries.
    const SECRET: &str = "s3cr3t-db-credential-7Q";
    /// A second, distinct credential, for URLs that carry two.
    const OTHER_SECRET: &str = "0ther-db-credential-4Z";
    /// Field value of the probe event that shows a capture records what is emitted under it.
    const CAPTURE_PROBE: &str = "capture-probe-91X";
    /// The message `sqlx` logs, with the value, for a query parameter it does not recognize.
    const SQLX_UNRECOGNIZED: &str = "ignoring unrecognized connect parameter";

    /// In-memory writer standing in for standard output.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        fn text(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Runs [`connect`] on `url` and returns its result and everything it logged.
    ///
    /// The capture is a subscriber made of [`json_layer`] alone, with no level filter, installed
    /// on this thread only; the current-thread runtime of `#[tokio::test]` polls every task of
    /// the test, those `sqlx` spawns included, on this thread. A probe event emitted first must
    /// appear in the output, so that an empty capture cannot pass.
    async fn connect_captured(url: &str) -> (Result<PgPool, sqlx::Error>, String) {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(json_layer(captured.clone()));
        let guard = tracing::subscriber::set_default(subscriber);
        tracing::warn!(probe = CAPTURE_PROBE, "capture probe");
        let config = Config {
            database_url: url.to_owned(),
            ..Config::default()
        };
        let result = connect(&config).await;
        drop(guard);
        let output = captured.text();
        assert!(
            output.contains(CAPTURE_PROBE),
            "the capture recorded nothing: {output}"
        );
        (result, output)
    }

    fn assert_free_of(text: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }

    /// A database error as a server reports it, with a message that quotes a value.
    #[derive(Debug)]
    struct ServerError {
        code: &'static str,
    }

    impl fmt::Display for ServerError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "invalid value for parameter: \"{SECRET}\"")
        }
    }

    impl StdError for ServerError {}

    impl DatabaseError for ServerError {
        fn message(&self) -> &str {
            SECRET
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
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

        fn kind(&self) -> DatabaseErrorKind {
            DatabaseErrorKind::Other
        }
    }

    #[tokio::test]
    async fn refuses_a_url_before_sqlx_can_quote_its_values() {
        let refused = [
            // An unrecognized parameter, which `sqlx` would log with its value.
            (
                format!("?application_name=carddemo&token={SECRET}"),
                UrlError::UnrecognizedParameter(2),
            ),
            (
                format!("?passwrod={SECRET}"),
                UrlError::UnrecognizedParameter(1),
            ),
            // Recognized parameters with values `sqlx` rejects, quoting an unknown `sslmode`.
            (format!("?sslmode={SECRET}"), UrlError::Invalid),
            (
                format!("?statement-cache-capacity={SECRET}"),
                UrlError::Invalid,
            ),
        ];
        for (query, reason) in refused {
            let url = format!("postgres://carddemo:{OTHER_SECRET}@127.0.0.1:5432/carddemo{query}");
            let (result, output) = connect_captured(&url).await;
            assert!(!output.contains(SQLX_UNRECOGNIZED), "{output}");
            assert_free_of(&output, &[SECRET, OTHER_SECRET]);
            let error = match result {
                Err(error @ sqlx::Error::Configuration(_)) => error,
                Err(other) => panic!("expected a configuration error for {query}, got {other}"),
                Ok(_) => panic!("{query} must not yield a pool"),
            };
            assert_eq!(
                error.to_string(),
                format!("error with configuration: {reason}")
            );
            let summary = ErrorSummary::from(&error).to_string();
            assert_eq!(summary, format!("configuration: {reason}"));
            for text in [error.to_string(), format!("{error:?}"), summary] {
                assert_free_of(&text, &[SECRET, OTHER_SECRET]);
            }
        }
    }

    #[tokio::test]
    async fn a_failed_connection_setup_logs_and_returns_no_credential() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local listener");
        let port = listener.local_addr().expect("listener address").port();
        // The peer accepts and closes without answering, so `sqlx` fails on the first read. A
        // refused connection would instead be retried until the pool's acquire timeout.
        let peer = thread::spawn(move || listener.accept().map(drop));
        let url = format!(
            "postgres://carddemo:{SECRET}@127.0.0.1:{port}/carddemo\
             ?application_name=carddemo&password={OTHER_SECRET}"
        );
        let (result, output) = connect_captured(&url).await;
        let error = match result {
            Err(error @ sqlx::Error::Io(_)) => error,
            Err(other) => panic!("expected an I/O error, got {other}"),
            Ok(_) => panic!("a peer that closes at once must not yield a pool"),
        };
        peer.join()
            .expect("the peer thread ends")
            .expect("the peer accepted the connection");
        let summary = ErrorSummary::from(&error).to_string();
        assert!(summary.starts_with("io ("), "{summary}");
        for text in [error.to_string(), format!("{error:?}"), summary, output] {
            assert_free_of(&text, &[SECRET, OTHER_SECRET]);
        }
    }

    #[test]
    fn query_parameter_names_are_checked_as_written() {
        let all_recognized = "?sslmode=disable&ssl-mode=disable&sslrootcert=ca.pem\
            &ssl-root-cert=ca.pem&ssl-ca=ca.pem&sslcert=client.pem&ssl-cert=client.pem\
            &sslkey=client.key&ssl-key=client.key&statement-cache-capacity=100&host=localhost\
            &hostaddr=127.0.0.1&port=5432&dbname=carddemo&user=carddemo&password=carddemo\
            &application_name=carddemo&options=-c%20search_path%3Dpublic\
            &options[search_path]=public";
        let cases = [
            ("", Ok(())),
            ("?", Ok(())),
            (all_recognized, Ok(())),
            ("?&&application_name=carddemo&", Ok(())),
            ("?application_name", Ok(())),
            ("?application_name=a=b", Ok(())),
            ("#?token=x", Ok(())),
            ("?application_name=carddemo#&token=x", Ok(())),
            ("?token=x", Err(UrlError::UnrecognizedParameter(1))),
            (
                "?user=carddemo&token=x",
                Err(UrlError::UnrecognizedParameter(2)),
            ),
            ("?&&token", Err(UrlError::UnrecognizedParameter(1))),
            ("?pass%77ord=x", Err(UrlError::UnrecognizedParameter(1))),
            (
                "?application+name=x",
                Err(UrlError::UnrecognizedParameter(1)),
            ),
            (
                "?options%5Bsearch_path%5D=x",
                Err(UrlError::UnrecognizedParameter(1)),
            ),
            ("?=x", Err(UrlError::UnrecognizedParameter(1))),
            ("?Password=x", Err(UrlError::UnrecognizedParameter(1))),
            ("?options[]=x", Err(UrlError::UnrecognizedParameter(1))),
            (
                "?options[search_path=x",
                Err(UrlError::UnrecognizedParameter(1)),
            ),
        ];
        for (query, expected) in cases {
            let url = format!("postgres://carddemo@localhost/carddemo{query}");
            assert_eq!(check_query_parameters(&url), expected, "{url}");
        }
    }

    #[test]
    fn every_accepted_name_is_one_sqlx_parses_without_logging() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(json_layer(captured.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let names = RECOGNIZED_PARAMETERS
                .into_iter()
                .chain(["options[search_path]"]);
            for name in names {
                let value = match name {
                    "sslmode" | "ssl-mode" => "disable",
                    "statement-cache-capacity" | "port" => "5432",
                    "hostaddr" => "127.0.0.1",
                    _ => "carddemo",
                };
                let url = format!("postgres://localhost/carddemo?{name}={value}");
                assert_eq!(check_query_parameters(&url), Ok(()), "{url}");
                assert!(url.parse::<PgConnectOptions>().is_ok(), "{url}");
            }
            // Control: `sqlx` logs an unrecognized parameter with its value into this capture.
            let unrecognized = format!("postgres://localhost/carddemo?token={CAPTURE_PROBE}");
            assert!(unrecognized.parse::<PgConnectOptions>().is_ok());
        });
        let output = captured.text();
        assert_eq!(output.matches(SQLX_UNRECOGNIZED).count(), 1, "{output}");
        assert!(output.contains(CAPTURE_PROBE), "{output}");
    }

    #[test]
    fn summaries_keep_the_class_and_drop_every_value() {
        let cases = [
            (
                sqlx::Error::Configuration(io::Error::other(SECRET).into()),
                "configuration",
            ),
            (
                UrlError::UnrecognizedParameter(3).into(),
                "configuration: DATABASE_URL query parameter 3 is not a recognized PostgreSQL \
                 connection parameter",
            ),
            (
                UrlError::Invalid.into(),
                "configuration: DATABASE_URL is not a valid PostgreSQL connection URL",
            ),
            (
                sqlx::Error::Database(Box::new(ServerError { code: "28P01" })),
                "database (SQLSTATE 28P01)",
            ),
            // A code without the SQLSTATE shape is dropped rather than logged.
            (
                sqlx::Error::Database(Box::new(ServerError { code: SECRET })),
                "database",
            ),
            (
                sqlx::Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, SECRET)),
                "io (unexpected end of file)",
            ),
            (sqlx::Error::Tls(SECRET.into()), "tls"),
            (sqlx::Error::Protocol(SECRET.to_owned()), "protocol"),
            (
                sqlx::Error::InvalidArgument(SECRET.to_owned()),
                "invalid_argument",
            ),
            (
                sqlx::Error::TypeNotFound {
                    type_name: SECRET.to_owned(),
                },
                "type_not_found",
            ),
            (
                sqlx::Error::ColumnNotFound(SECRET.to_owned()),
                "column_not_found",
            ),
            (sqlx::Error::Decode(SECRET.into()), "decode"),
            (sqlx::Error::PoolTimedOut, "pool_timed_out"),
            (sqlx::Error::PoolClosed, "pool_closed"),
            (sqlx::Error::WorkerCrashed, "worker_crashed"),
            (
                sqlx::Error::Migrate(Box::new(MigrateError::Source(SECRET.into()))),
                "migrate",
            ),
        ];
        for (error, expected) in &cases {
            assert_eq!(
                ErrorSummary::from(error).to_string(),
                *expected,
                "{error:?}"
            );
        }
    }

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
