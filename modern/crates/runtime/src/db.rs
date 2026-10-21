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
//! # Settings from `DATABASE_URL` alone
//!
//! `sqlx` fills each setting a URL omits from the libpq environment variables (`PGHOST`,
//! `PGHOSTADDR`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGSSLMODE`, `PGAPPNAME`,
//! `PGOPTIONS` and the `PGSSLROOTCERT`, `PGSSLCERT` and `PGSSLKEY` paths) and, when the URL has
//! no password, from the pgpass file (`PGPASSFILE` or `~/.pgpass`), whose malformed lines it
//! logs verbatim. [`Config`] is the only configuration reader, so [`connect`] leaves none of
//! this to `sqlx`:
//!
//! - `DATABASE_URL` must name a host, a user and a password, in its authority or as the `host`
//!   or `hostaddr`, `user` and `password` query parameters. A URL that lacks one is refused
//!   before `sqlx` sees it, so neither those variables nor the pgpass file can supply it.
//! - What the URL may omit is set explicitly: port 5432, `sslmode` `prefer`, the user name as
//!   the database and an empty application name, the last two being the server's own
//!   defaults. This build has no TLS backend, so `prefer` connects in plain text and the
//!   `PGSSL*` certificate paths, which `sqlx` offers no way to clear, are never used.
//! - A `PGOPTIONS` that names any server option is refused, because `sqlx` sends it as server
//!   options and offers no way to clear it: server options come only from the URL's `options`
//!   parameters. One of ASCII whitespace alone names none, so it passes.
//! - The URL is refused as invalid when its scheme is not `postgres` or `postgresql`, in any
//!   ASCII case, or when it holds an ASCII control character, ends in a space or has a `.` or
//!   `..` path segment, plain or percent-encoded. `sqlx` accepts any scheme, and the URL
//!   standard reads a special one such as `http` or `file` by other rules: a `\` ends its
//!   authority, its default port is dropped and its host is rewritten. The standard may also
//!   strip the control characters and the space and resolves the dot segments. In each case
//!   `sqlx` would read other parts than the ones checked here.
//!
//! # Unreachable servers
//!
//! Before it builds the pool, [`connect`] makes one connection of its own within 30 seconds,
//! the default acquire timeout of a `sqlx` pool, resolving the host first. The pool alone would
//! retry a refused connection until that timeout and then report only that it timed out, and
//! would report an unresolvable host as an uncategorized I/O error. The single connection
//! reports the cause instead, which [`ErrorSummary`] renders as `io (connection refused)`,
//! `io (host name not resolved)` or `io (timed out)`.
//!
//! # Credential-safe diagnostics
//!
//! The URL carries the database password, so nothing here logs, traces or formats it. `sqlx`
//! logs every query parameter it does not recognize together with its value, so [`connect`]
//! refuses such a parameter before `sqlx` sees the URL. It also logs a malformed pgpass line,
//! password included, which is one reason the pgpass file is never read. Every configuration
//! error [`connect`] returns names `DATABASE_URL` and never quotes the URL or any value of the
//! environment. Callers log a `sqlx` error only through [`ErrorSummary`], which keeps an error
//! class and, for a database error, the SQLSTATE, and never the driver's or the server's text:
//! that text can quote a rejected option value or a key value.

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::time::Duration;

use crate::config::Config;
use sqlx::Connection;
use sqlx::error::DatabaseError;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions, PgSslMode};

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

/// The schemes of a `PostgreSQL` connection URL. Neither is special in the URL standard, which
/// therefore locates the authority, the port and the path of such a URL as [`UrlParts::of`]
/// does. For a special scheme, such as `http` or `file`, the standard also ends the authority at
/// a `\`, drops the scheme's default port and rewrites the host.
const POSTGRES_SCHEMES: [&str; 2] = ["postgres", "postgresql"];

/// Opening of an `options[<name>]` parameter, which sets one server setting.
const OPTIONS_PREFIX: &str = "options[";

/// Closing of an `options[<name>]` parameter.
const OPTIONS_SUFFIX: &str = "]";

/// The one byte of `isspace` in the C locale that [`u8::is_ascii_whitespace`] leaves out.
const VERTICAL_TAB: u8 = 0x0B;

/// The port that libpq and `PostgreSQL` use when none is named.
const DEFAULT_PORT: u16 = 5432;

/// How long [`connect`] waits for its own first connection, name resolution included: the
/// default acquire timeout of a `sqlx` pool, which then bounds each connection the pool opens.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Opens a [`PgPool`] on [`Config::database_url`] with the default `sqlx` pool settings, and
/// establishes one connection before returning, so a wrong URL, an unreachable server or rejected
/// credentials surface at startup rather than at the first query.
///
/// The URL's query parameter names are checked before `sqlx` parses it: each must be one that
/// `sqlx` recognizes, compared exactly as written, so a percent-encoded or `+`-encoded name is
/// refused even when it decodes to a recognized one. Every setting comes from the URL or from an
/// explicit default, never from the libpq environment or the pgpass file (see the module
/// documentation).
///
/// # Errors
///
/// - [`sqlx::Error::Configuration`] when the URL holds a query parameter that `sqlx` does not
///   recognize, names no host, no user or no password, or is not a valid `PostgreSQL`
///   connection URL, for example because its scheme is not `postgres` or `postgresql` or a
///   recognized parameter holds a value `sqlx` rejects, and when `PGOPTIONS` names a server
///   option. Its text names `DATABASE_URL` and at most the position of the offending parameter
///   or the libpq variables that are not read, and never quotes the URL, any part of it or any
///   value of the environment.
/// - [`sqlx::Error::Io`] when the server cannot be reached, at once rather than after the
///   pool's acquire timeout. [`ErrorSummary`] reports it as `io (connection refused)` when
///   nothing listens on the port, `io (host name not resolved)` when the host resolves to no
///   address, `io (timed out)` when no connection is established within 30 seconds, and by its
///   kind for any other I/O failure.
/// - [`sqlx::Error::Tls`] when the URL requires TLS, which this build cannot provide.
/// - [`sqlx::Error::Database`] when the server rejects the connection, for example on an
///   authentication failure or an unknown database.
/// - [`sqlx::Error::PoolTimedOut`] only when the server stops accepting connections between
///   that first connection and the pool's own.
pub async fn connect(config: &Config) -> Result<PgPool, sqlx::Error> {
    connect_within(config, CONNECT_TIMEOUT).await
}

/// [`connect`], with its first connection bounded by `timeout`.
///
/// The pool alone would retry a refused connection until its acquire timeout and then report
/// only [`sqlx::Error::PoolTimedOut`], and would report a host that resolves to no address as
/// an uncategorized I/O error. So one connection is made first, outside the pool, and its
/// failure is returned as it happened: an elapsed `timeout` as [`io::ErrorKind::TimedOut`] and
/// an unresolved host with [`HostNotResolved`] as its cause. The pool, with its default
/// settings, is built only once that connection has succeeded.
async fn connect_within(config: &Config, timeout: Duration) -> Result<PgPool, sqlx::Error> {
    let options = connect_options(&config.database_url)?;
    tokio::time::timeout(timeout, connect_once(&options))
        .await
        .map_err(|_| sqlx::Error::Io(io::ErrorKind::TimedOut.into()))??;
    PgPoolOptions::new().connect_with(options).await
}

/// Resolves the host that `options` name, unless they name a Unix-domain socket, then opens one
/// connection and closes it.
async fn connect_once(options: &PgConnectOptions) -> Result<(), sqlx::Error> {
    if options.get_socket().is_none() {
        // `sqlx` strips the brackets of an IPv6 address in the same way before it connects.
        let host = options.get_host().trim_matches(['[', ']']);
        let resolved = tokio::net::lookup_host((host, options.get_port())).await;
        if !resolved.is_ok_and(|mut addresses| addresses.next().is_some()) {
            return Err(sqlx::Error::Io(io::Error::other(HostNotResolved)));
        }
    }
    PgConnection::connect_with(options).await?.close().await
}

/// The cause of the I/O error [`connect`] returns when the host that the URL names resolves to
/// no address. It never names the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HostNotResolved;

impl fmt::Display for HostNotResolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("host name not resolved")
    }
}

impl StdError for HostNotResolved {}

/// Parses `url` into connect options in which no setting comes from the environment.
///
/// [`check_query_parameters`] runs first, then [`UrlParts::of`]. The URL must name a host, a
/// user and a password, and `PGOPTIONS` must name no server option ([`names_server_options`]),
/// because `sqlx` takes each of these from the libpq environment, or the password from the
/// pgpass file, when the URL omits it. Any parse error of `sqlx` becomes [`UrlError::Invalid`]:
/// the `sqlx` text can quote a rejected value, such as an unknown `sslmode`. The port, the TLS
/// mode, the database and the application name are then set explicitly wherever the URL leaves
/// them to the environment.
fn connect_options(url: &str) -> Result<PgConnectOptions, UrlError> {
    check_query_parameters(url)?;
    let parts = UrlParts::of(url)?;
    if !parts.names_host() {
        return Err(UrlError::NoHost);
    }
    if !parts.names_user() {
        return Err(UrlError::NoUser);
    }
    if !parts.names_password() {
        return Err(UrlError::NoPassword);
    }
    // Refused rather than ignored: every `PgConnectOptions` constructor reads `PGOPTIONS`, which
    // `sqlx` sends as server options, its `options` setter only appends to it, nothing clears
    // it, and `std::env::remove_var`, which could drop it, is not callable under the workspace
    // lints. Reading it here involves neither the pgpass file nor any logging.
    if PgConnectOptions::new_without_pgpass()
        .get_options()
        .is_some_and(names_server_options)
    {
        return Err(UrlError::EnvironmentOptions);
    }
    let mut options = url
        .parse::<PgConnectOptions>()
        .map_err(|_| UrlError::Invalid)?;
    if !parts.names_port() {
        options = options.port(DEFAULT_PORT);
    }
    // The `sqlx` default. Without a TLS backend it connects in plain text, so the certificate
    // paths `sqlx` takes from the environment are never used.
    if !parts.names_ssl_mode() {
        options = options.ssl_mode(PgSslMode::Prefer);
    }
    // Without a database, the server connects the session to the user's own.
    if !parts.names_database() && options.get_database().is_some() {
        let user = options.get_username().to_owned();
        options = options.database(&user);
    }
    // Without an application name, the server keeps the empty one.
    if !parts.names_application_name() && options.get_application_name().is_some() {
        options = options.application_name("");
    }
    Ok(options)
}

/// The parts of a connection URL that decide which settings it states, located as the URL
/// standard locates them and compared as written, without decoding.
///
/// Only presence matters: `sqlx` takes a setting from the URL when its part is non-empty or its
/// query parameter is present, and from the environment otherwise.
#[derive(Debug)]
struct UrlParts<'a> {
    /// The userinfo before its first `:`.
    user: &'a str,
    /// The userinfo after its first `:`.
    password: &'a str,
    /// The host; for a bracketed IPv6 address, the text up to its closing `]`.
    host: &'a str,
    /// The text after the host's `:`.
    port: &'a str,
    /// The path without its leading `/` characters, which `sqlx` takes as the database.
    database: &'a str,
    /// The query parameter names, as [`query_parameter_names`] yields them.
    parameters: Vec<&'a str>,
}

impl<'a> UrlParts<'a> {
    /// Locates the parts of `url`, and refuses as [`UrlError::Invalid`] a URL whose scheme is not
    /// one of [`POSTGRES_SCHEMES`], in any ASCII case, which the URL standard may read by other
    /// rules than the ones below, and one that the standard may rewrite before `sqlx` reads it:
    /// one that holds an ASCII control character or ends in a space, which the standard may
    /// strip, or has a dot path segment, which it resolves.
    ///
    /// The authority is the text after `<scheme>://` up to the first `/`, `?` or `#`, and a URL
    /// without `//` has none. Its userinfo runs to its last `@` and splits at its first `:`;
    /// the host and port follow that `@`, split at the first `:` after any bracketed address.
    #[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
    fn of(url: &'a str) -> Result<Self, UrlError> {
        if url.bytes().any(|byte| byte.is_ascii_control()) || url.ends_with(' ') {
            return Err(UrlError::Invalid);
        }
        let (scheme, rest) = url.split_once(':').ok_or(UrlError::Invalid)?;
        if !is_postgres_scheme(scheme) {
            return Err(UrlError::Invalid);
        }
        let before_query = rest.split_once(['?', '#']).map_or(rest, |(head, _)| head);
        let (authority, path) = match before_query.strip_prefix("//") {
            Some(after_slashes) => after_slashes.split_once('/').unwrap_or((after_slashes, "")),
            None => ("", before_query),
        };
        if path.split('/').any(is_dot_segment) {
            return Err(UrlError::Invalid);
        }
        let (userinfo, host_and_port) = authority.rsplit_once('@').unwrap_or(("", authority));
        let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
        let (host, port) = split_host_and_port(host_and_port);
        Ok(Self {
            user,
            password,
            host,
            port,
            database: path.trim_start_matches('/'),
            parameters: query_parameter_names(url).collect(),
        })
    }

    /// Whether the query holds a parameter named `name`.
    fn has_parameter(&self, name: &str) -> bool {
        self.parameters.contains(&name)
    }

    /// Whether the URL names a host, which `sqlx` otherwise takes from `PGHOSTADDR` or `PGHOST`.
    fn names_host(&self) -> bool {
        !self.host.is_empty() || self.has_parameter("host") || self.has_parameter("hostaddr")
    }

    /// Whether the URL names a user, which `sqlx` otherwise takes from `PGUSER` or the
    /// operating system.
    fn names_user(&self) -> bool {
        !self.user.is_empty() || self.has_parameter("user")
    }

    /// Whether the URL carries a password, which `sqlx` otherwise takes from `PGPASSWORD` or the
    /// pgpass file.
    fn names_password(&self) -> bool {
        !self.password.is_empty() || self.has_parameter("password")
    }

    /// Whether the URL names a port, which `sqlx` otherwise takes from `PGPORT`.
    fn names_port(&self) -> bool {
        !self.port.is_empty() || self.has_parameter("port")
    }

    /// Whether the URL names a database, which `sqlx` otherwise takes from `PGDATABASE`.
    fn names_database(&self) -> bool {
        !self.database.is_empty() || self.has_parameter("dbname")
    }

    /// Whether the URL sets a TLS mode, which `sqlx` otherwise takes from `PGSSLMODE`.
    fn names_ssl_mode(&self) -> bool {
        self.has_parameter("sslmode") || self.has_parameter("ssl-mode")
    }

    /// Whether the URL sets an application name, which `sqlx` otherwise takes from `PGAPPNAME`.
    fn names_application_name(&self) -> bool {
        self.has_parameter("application_name")
    }
}

/// Whether `scheme` is one of [`POSTGRES_SCHEMES`], compared ASCII case-insensitively as the URL
/// standard compares schemes.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn is_postgres_scheme(scheme: &str) -> bool {
    POSTGRES_SCHEMES
        .iter()
        .any(|known| scheme.eq_ignore_ascii_case(known))
}

/// Whether the server options string `options` names any option: whether it holds a byte other
/// than the ASCII whitespace the server skips between options, `isspace` of the C locale. Other
/// whitespace, such as U+00A0, reaches the server as an option, which it rejects.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn names_server_options(options: &str) -> bool {
    options
        .bytes()
        .any(|byte| !byte.is_ascii_whitespace() && byte != VERTICAL_TAB)
}

/// Whether the path segment `segment` is `.` or `..`, each dot written plainly or as `%2e`
/// in either case, which the URL standard removes from the path.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn is_dot_segment(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().replace("%2e", ".").as_str(),
        "." | ".."
    )
}

/// Splits the text after the userinfo into host and port. A bracketed IPv6 address keeps its
/// colons, and the port is what follows the `:` after its closing `]`.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn split_host_and_port(host_and_port: &str) -> (&str, &str) {
    if host_and_port.starts_with('[') {
        return host_and_port
            .split_once(']')
            .map_or((host_and_port, ""), |(address, after)| {
                (address, after.strip_prefix(':').unwrap_or(after))
            });
    }
    host_and_port.split_once(':').unwrap_or((host_and_port, ""))
}

/// Refuses the first query parameter of `url` whose name `sqlx` does not recognize.
///
/// The query is located as the URL standard locates it, the text after the first `?` that
/// precedes the first `#`, and split as `sqlx` splits it: on `&`, with empty pieces skipped and
/// each name ending at its first `=`. Names are compared as written, without decoding. Positions
/// are 1-based and count the non-empty pieces only.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn check_query_parameters(url: &str) -> Result<(), UrlError> {
    for (position, name) in (1_usize..).zip(query_parameter_names(url)) {
        if !is_recognized(name) {
            return Err(UrlError::UnrecognizedParameter(position));
        }
    }
    Ok(())
}

/// The query parameter names of `url` in order, located and split as
/// [`check_query_parameters`] describes, without decoding.
///
/// Once [`check_query_parameters`] has accepted `url`, each name is one `sqlx` recognizes and
/// holds no encoding, so it equals the name `sqlx` decodes.
#[deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
fn query_parameter_names(url: &str) -> impl Iterator<Item = &str> {
    let before_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let query = before_fragment
        .split_once('?')
        .map_or("", |(_, query)| query);
    query
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| {
            parameter
                .split_once('=')
                .map_or(parameter, |(name, _)| name)
        })
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

/// Why the connection settings were refused before any connection attempt.
///
/// Its text names `DATABASE_URL`, at most a parameter's position and the libpq variables that
/// are not read, never a name or a value from the URL or the environment: a malformed URL can
/// put password text anywhere in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlError {
    /// The query parameter at this 1-based position is not one `sqlx` recognizes.
    UnrecognizedParameter(usize),
    /// `sqlx` cannot parse the URL, a recognized parameter holds a value it rejects, the scheme
    /// is not `postgres` or `postgresql`, or the URL standard would rewrite the URL before
    /// `sqlx` reads it.
    Invalid,
    /// The URL names no host, which `sqlx` would take from `PGHOSTADDR` or `PGHOST`.
    NoHost,
    /// The URL names no user, which `sqlx` would take from `PGUSER` or the operating system.
    NoUser,
    /// The URL carries no password, which `sqlx` would take from `PGPASSWORD` or the pgpass
    /// file.
    NoPassword,
    /// `PGOPTIONS` names a server option, which `sqlx` would send.
    EnvironmentOptions,
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
            Self::NoHost => {
                f.write_str("DATABASE_URL names no host: PGHOST and PGHOSTADDR are not read")
            }
            Self::NoUser => f.write_str("DATABASE_URL names no user: PGUSER is not read"),
            Self::NoPassword => f.write_str(
                "DATABASE_URL carries no password: PGPASSWORD and the pgpass file are not read",
            ),
            Self::EnvironmentOptions => {
                f.write_str("PGOPTIONS is not blank: server options come only from DATABASE_URL")
            }
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
/// [`io::ErrorKind`] (`io (connection refused)`), or by `host name not resolved` when
/// [`connect`] found no address for the host, and for a configuration error that [`connect`]
/// raised by its reason, which names variables and never their values. It never keeps the
/// driver's or the server's text, which can quote the connection URL, a rejected option value or
/// a key value.
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
    /// An I/O error caused by [`HostNotResolved`].
    HostNotResolved,
    /// The reason [`connect`] refused the connection settings.
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
            sqlx::Error::Io(io) => {
                let detail = match io.get_ref() {
                    Some(cause) if cause.is::<HostNotResolved>() => SummaryDetail::HostNotResolved,
                    _ => SummaryDetail::IoKind(io.kind()),
                };
                ("io", Some(detail))
            }
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
            Some(SummaryDetail::HostNotResolved) => write!(f, " ({HostNotResolved})"),
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
    use std::fs;
    use std::io;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::{Duration, Instant};

    use sqlx::error::{DatabaseError, ErrorKind as DatabaseErrorKind};
    use sqlx::migrate::MigrateError;
    use sqlx::postgres::{PgConnectOptions, PgPool, PgSslMode};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{
        CONNECT_TIMEOUT, ErrorSummary, HostNotResolved, RECOGNIZED_PARAMETERS, UrlError,
        check_query_parameters, connect, connect_options, connect_within, names_server_options,
    };
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
    /// Target of every event `sqlx` logs while it reads a pgpass file, a malformed line included.
    const PGPASS_TARGET: &str = "sqlx_postgres::options::pgpass";
    /// Stand-in for a password that `PGPASSWORD` carries.
    const ENV_SECRET: &str = "3nv-pgpassword-8K";
    /// Stand-in for the password on a malformed pgpass line, which `sqlx` logs verbatim.
    const PGPASS_SECRET: &str = "pgp4ss-line-secret-2M";
    /// The variable that gives [`libpq_environment_child`] its mode; unset, the test returns at
    /// once.
    const CHILD_MODE: &str = "CARDDEMO_RUNTIME_DB_TEST_CHILD_MODE";
    /// The path under which the test harness names [`libpq_environment_child`].
    const CHILD_TEST: &str = "db::tests::libpq_environment_child";
    /// What [`libpq_environment_child`] prints, followed by its mode, once its checks pass.
    const CHILD_PASSED: &str = "libpq environment child passed in mode";
    /// The libpq environment of every child run, each value distinct from the tested URLs' own.
    /// `PGPASSWORD`, which keeps `sqlx` from reading the pgpass file, is added per mode.
    const LIBPQ_ENVIRONMENT: [(&str, &str); 7] = [
        ("PGHOST", "env-host.invalid"),
        ("PGHOSTADDR", "192.0.2.10"),
        ("PGPORT", "6999"),
        ("PGUSER", "env-user"),
        ("PGDATABASE", "env-db"),
        ("PGSSLMODE", "require"),
        ("PGAPPNAME", "env-app"),
    ];
    /// The password variable of the child runs that set it.
    const PGPASSWORD: (&str, &str) = ("PGPASSWORD", ENV_SECRET);
    /// A `PGOPTIONS` of the ASCII whitespace that the server skips between options, which
    /// therefore names none.
    const BLANK_PGOPTIONS: &str = " \t\x0B ";

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

    /// Runs [`connect`] on `url` and returns its result and everything it logged, as
    /// [`logged_while`] records it.
    async fn connect_captured(url: &str) -> (Result<PgPool, sqlx::Error>, String) {
        let config = Config {
            database_url: url.to_owned(),
            ..Config::default()
        };
        logged_while(connect(&config)).await
    }

    /// Awaits `future` and returns its output and everything logged meanwhile.
    ///
    /// The capture is a subscriber made of [`json_layer`] alone, with no level filter, installed
    /// on this thread only; the current-thread runtime of `#[tokio::test]` polls every task of
    /// the test, those `sqlx` spawns included, on this thread. A probe event emitted first must
    /// appear in the output, so that an empty capture cannot pass.
    async fn logged_while<T>(future: impl Future<Output = T>) -> (T, String) {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(json_layer(captured.clone()));
        let guard = tracing::subscriber::set_default(subscriber);
        tracing::warn!(probe = CAPTURE_PROBE, "capture probe");
        let value = future.await;
        drop(guard);
        let output = captured.text();
        assert!(
            output.contains(CAPTURE_PROBE),
            "the capture recorded nothing: {output}"
        );
        (value, output)
    }

    fn assert_free_of(text: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }

    /// Asserts that [`connect`] refuses `url` for `reason` before any connection attempt,
    /// without reading a pgpass file, and with none of `secrets` in its log, its error, the
    /// error's `Debug` or its summary.
    async fn assert_refused(url: &str, reason: UrlError, secrets: &[&str]) {
        let (result, output) = connect_captured(url).await;
        assert!(!output.contains(PGPASS_TARGET), "{output}");
        let error = match result {
            Err(error @ sqlx::Error::Configuration(_)) => error,
            Err(other) => panic!("expected the configuration error {reason}, got {other}"),
            Ok(_) => panic!("a URL refused for {reason} must not yield a pool"),
        };
        assert_eq!(
            error.to_string(),
            format!("error with configuration: {reason}")
        );
        let summary = ErrorSummary::from(&error).to_string();
        assert_eq!(summary, format!("configuration: {reason}"));
        for text in [error.to_string(), format!("{error:?}"), summary, output] {
            assert_free_of(&text, secrets);
        }
    }

    /// An `http` URL whose text after the `\` looks like a password to a reader of the
    /// `postgres` scheme. `sqlx` reads it by the rules of `http`, in which `\` ends the authority,
    /// so it finds no password and reads the pgpass file.
    fn pgpass_bypass_url() -> String {
        format!("http://127.0.0.1\\:{SECRET}@ignored/db?host=127.0.0.1&port=1&user=carddemo")
    }

    /// The settings `sqlx` connects with, as its getters report them. A server gives a session
    /// without a database the user's own, and one without an application name the empty one, so
    /// both are reported as the server applies them.
    #[derive(Debug, PartialEq, Eq)]
    struct Settings<'a> {
        host: &'a str,
        port: u16,
        user: &'a str,
        database: &'a str,
        ssl_mode: &'static str,
        application_name: &'a str,
    }

    impl<'a> Settings<'a> {
        fn of(options: &'a PgConnectOptions) -> Self {
            Self {
                host: options.get_host(),
                port: options.get_port(),
                user: options.get_username(),
                database: options.get_database().unwrap_or(options.get_username()),
                ssl_mode: match options.get_ssl_mode() {
                    PgSslMode::Disable => "disable",
                    PgSslMode::Allow => "allow",
                    PgSslMode::Prefer => "prefer",
                    PgSslMode::Require => "require",
                    PgSslMode::VerifyCa => "verify-ca",
                    PgSslMode::VerifyFull => "verify-full",
                },
                application_name: options.get_application_name().unwrap_or_default(),
            }
        }
    }

    /// Asserts, for URLs that state some settings in the authority, some as query parameters and
    /// omit others, that every setting comes from the URL or from the explicit default, whatever
    /// the libpq environment holds.
    fn assert_settings_come_from_the_url() {
        let settings = |host, port, user, database, ssl_mode, application_name| Settings {
            host,
            port,
            user,
            database,
            ssl_mode,
            application_name,
        };
        let cases = [
            (
                "postgres://carddemo:carddemo@localhost:5432/carddemo",
                settings("localhost", 5432, "carddemo", "carddemo", "prefer", ""),
            ),
            // An empty port names none.
            (
                "postgres://carddemo:pw@127.0.0.1:/carddemo",
                settings("127.0.0.1", 5432, "carddemo", "carddemo", "prefer", ""),
            ),
            (
                "postgres://u:pw@db.local:6543/other?sslmode=disable&application_name=carddemo",
                settings("db.local", 6543, "u", "other", "disable", "carddemo"),
            ),
            (
                "postgres://127.0.0.1/carddemo?user=carddemo&password=pw",
                settings("127.0.0.1", 5432, "carddemo", "carddemo", "prefer", ""),
            ),
            (
                "postgres:///carddemo?host=db.local&port=6543&user=u&password=pw&ssl-mode=allow",
                settings("db.local", 6543, "u", "carddemo", "allow", ""),
            ),
            (
                "postgres://?hostaddr=127.0.0.1&dbname=other&user=u&password=pw",
                settings("127.0.0.1", 5432, "u", "other", "prefer", ""),
            ),
            // No database: the server's default, the user's own.
            (
                "postgres://u:pw@[::1]:6543",
                settings("[::1]", 6543, "u", "u", "prefer", ""),
            ),
            // Either scheme, in any ASCII case.
            (
                "POSTGRES://u:pw@db.local:6543/other",
                settings("db.local", 6543, "u", "other", "prefer", ""),
            ),
            (
                "postgresql://u:pw@db.local/other",
                settings("db.local", 5432, "u", "other", "prefer", ""),
            ),
        ];
        for (url, expected) in cases {
            let options = connect_options(url).unwrap_or_else(|reason| panic!("{url}: {reason}"));
            assert_eq!(Settings::of(&options), expected, "{url}");
            // `Debug` shows every option, the password and the certificate paths included.
            let every_option = format!("{options:?}");
            for (variable, value) in LIBPQ_ENVIRONMENT.into_iter().chain([PGPASSWORD]) {
                assert!(!every_option.contains(value), "{url}: {variable} was read");
            }
        }
    }

    /// A directory under [`std::env::temp_dir`], removed with its contents when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("carddemo-runtime-db-{name}-{}", std::process::id()));
            fs::create_dir_all(&path).expect("create a temporary directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Best effort: what a failed removal leaves behind is test data only.
            let _ = fs::remove_dir_all(&self.0);
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
        // The peer accepts and closes without answering, so the first connection fails on its
        // first read. A refused, unresolved or silent server is the subject of
        // `an_unreachable_server_is_reported_by_its_cause`.
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
            (
                sqlx::Error::Io(io::Error::other(HostNotResolved)),
                "io (host name not resolved)",
            ),
            // Only that cause renders so; another error of kind `Other` keeps the kind alone.
            (
                sqlx::Error::Io(io::Error::other(SECRET)),
                "io (other error)",
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

    #[tokio::test]
    async fn refuses_a_url_that_leaves_a_setting_to_the_environment() {
        let refused = [
            // No host, user or password, which `sqlx` would take from the libpq environment,
            // and the password from the pgpass file.
            ("postgres:///carddemo".to_owned(), UrlError::NoHost),
            (
                format!("postgres://carddemo:{SECRET}@/carddemo"),
                UrlError::NoHost,
            ),
            (
                format!("postgres:carddemo?user=carddemo&password={SECRET}"),
                UrlError::NoHost,
            ),
            (
                format!("postgres://:{SECRET}@127.0.0.1/carddemo"),
                UrlError::NoUser,
            ),
            (
                format!("postgres://127.0.0.1/carddemo?password={SECRET}"),
                UrlError::NoUser,
            ),
            (
                "postgres://carddemo@127.0.0.1:5432/carddemo".to_owned(),
                UrlError::NoPassword,
            ),
            (
                "postgres://carddemo:@127.0.0.1:5432/carddemo".to_owned(),
                UrlError::NoPassword,
            ),
            (
                "postgres://127.0.0.1/carddemo?user=carddemo".to_owned(),
                UrlError::NoPassword,
            ),
            // Parts the URL standard removes before `sqlx` reads them: a password or a database
            // that `sqlx` would then take from the environment.
            (
                "postgres://carddemo:\t@127.0.0.1/carddemo".to_owned(),
                UrlError::Invalid,
            ),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1/\n"),
                UrlError::Invalid,
            ),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1/ "),
                UrlError::Invalid,
            ),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1/%2E%2e"),
                UrlError::Invalid,
            ),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1/carddemo/.."),
                UrlError::Invalid,
            ),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1/./carddemo"),
                UrlError::Invalid,
            ),
            // No scheme.
            (
                format!("1postgres://carddemo:{SECRET}@127.0.0.1/carddemo"),
                UrlError::Invalid,
            ),
            (
                format!("postgres//carddemo:{SECRET}@127.0.0.1/carddemo"),
                UrlError::Invalid,
            ),
            // A scheme other than `postgres` or `postgresql`. In this `http` URL, `\` ends the
            // authority, so `sqlx` would find no password and read the pgpass file.
            (pgpass_bypass_url(), UrlError::Invalid),
            // The `http` default port is dropped, so `sqlx` would take the port from `PGPORT`.
            (
                format!("http://carddemo:{SECRET}@127.0.0.1:80/carddemo"),
                UrlError::Invalid,
            ),
            (
                format!("https://carddemo:{SECRET}@127.0.0.1/carddemo"),
                UrlError::Invalid,
            ),
            (
                format!("file://carddemo:{SECRET}@127.0.0.1/carddemo"),
                UrlError::Invalid,
            ),
            (
                format!("mysql://carddemo:{SECRET}@127.0.0.1/carddemo"),
                UrlError::Invalid,
            ),
        ];
        for (url, reason) in refused {
            assert_refused(&url, reason, &[SECRET]).await;
        }
    }

    #[test]
    fn every_setting_comes_from_the_url_or_an_explicit_default() {
        assert_settings_come_from_the_url();
    }

    #[test]
    fn only_whitespace_the_server_skips_names_no_server_option() {
        let cases = [
            ("", false),
            (BLANK_PGOPTIONS, false),
            (" \t\n\x0B\x0C\r", false),
            ("-c search_path=env", true),
            (" -c search_path=env ", true),
            // Whitespace outside ASCII, which the server reads as an option and rejects.
            ("\u{a0}", true),
            (" \u{3000} ", true),
        ];
        for (options, expected) in cases {
            assert_eq!(names_server_options(options), expected, "{options:?}");
        }
    }

    /// Runs [`libpq_environment_child`] in a child process of this test executable, once per
    /// mode, under a libpq environment and a pgpass file whose every value differs from the
    /// tested URLs' own, and asserts that it ran and passed. Only the child reads that
    /// environment, so no other test depends on it.
    #[test]
    fn the_libpq_environment_and_the_pgpass_file_change_nothing() {
        let directory = TempDir::new("pgpass");
        let pgpass = directory.path().join("pgpass");
        // Malformed: no field follows the user name, so `sqlx` logs the whole line on reading it.
        fs::write(&pgpass, format!("*:*:*:carddemo {PGPASS_SECRET}\n"))
            .expect("write the pgpass file");
        // `sqlx` ignores a pgpass file that others may read.
        fs::set_permissions(&pgpass, fs::Permissions::from_mode(0o600))
            .expect("restrict the pgpass file");
        let executable = std::env::current_exe().expect("the test executable");
        let modes: [(&str, &[(&str, &str)]); 4] = [
            // Without `PGPASSWORD`, `sqlx` reads the pgpass file for a URL without a password.
            ("pgpass", &[]),
            ("pgpassword", &[PGPASSWORD]),
            (
                "pgoptions",
                &[PGPASSWORD, ("PGOPTIONS", "-c search_path=env")],
            ),
            (
                "pgoptions-blank",
                &[PGPASSWORD, ("PGOPTIONS", BLANK_PGOPTIONS)],
            ),
        ];
        for (mode, variables) in modes {
            let output = Command::new(&executable)
                .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
                .envs(LIBPQ_ENVIRONMENT)
                .env("PGPASSFILE", &pgpass)
                .env_remove("PGPASSWORD")
                .env_remove("PGOPTIONS")
                .envs(variables.iter().copied())
                .env(CHILD_MODE, mode)
                .output()
                .expect("run the child test");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains(&format!("{CHILD_PASSED} {mode}")),
                "the {mode} child failed: {stdout}{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The child of [`the_libpq_environment_and_the_pgpass_file_change_nothing`]: asserts in
    /// the environment it is given that the connection settings take nothing from it. Without
    /// [`CHILD_MODE`] it returns at once.
    #[tokio::test]
    async fn libpq_environment_child() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        let secrets = [SECRET, ENV_SECRET, PGPASS_SECRET];
        let without_password = "postgres://carddemo@127.0.0.1:5432/carddemo";
        // Control: `sqlx` alone takes this environment, and the password from `PGPASSWORD` or,
        // without it, from the pgpass file, whose malformed line it logs verbatim into this
        // capture.
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(json_layer(captured.clone()));
        let parsed = tracing::subscriber::with_default(subscriber, || {
            without_password.parse::<PgConnectOptions>()
        });
        let options = parsed.expect("sqlx parses a URL without a password");
        assert_eq!(options.get_application_name(), Some("env-app"));
        let pgpass_logged = captured.text().contains(PGPASS_SECRET);
        let pgpassword_taken = format!("{options:?}").contains(ENV_SECRET);
        assert_eq!(
            (pgpass_logged, pgpassword_taken),
            (mode == "pgpass", mode != "pgpass"),
            "control of mode {mode}"
        );

        match mode.as_str() {
            "pgpass" | "pgpassword" => {
                assert_settings_come_from_the_url();
                assert_refused("postgres:///carddemo", UrlError::NoHost, &secrets).await;
                let without_user = format!("postgres://:{SECRET}@127.0.0.1/carddemo");
                assert_refused(&without_user, UrlError::NoUser, &secrets).await;
                assert_refused(without_password, UrlError::NoPassword, &secrets).await;
                if mode == "pgpass" {
                    // Control: `sqlx` alone reads the pgpass file for this URL and logs its
                    // malformed line into this capture.
                    let bypass = pgpass_bypass_url();
                    let captured = Captured::default();
                    let subscriber =
                        tracing_subscriber::registry().with(json_layer(captured.clone()));
                    let parsed = tracing::subscriber::with_default(subscriber, || {
                        bypass.parse::<PgConnectOptions>()
                    });
                    assert!(parsed.is_ok(), "sqlx parses the http URL");
                    assert!(
                        captured.text().contains(PGPASS_SECRET),
                        "control of the http URL"
                    );
                    assert_refused(&bypass, UrlError::Invalid, &secrets).await;
                }
            }
            "pgoptions" => {
                let url = format!("postgres://carddemo:{SECRET}@127.0.0.1/carddemo");
                assert_refused(&url, UrlError::EnvironmentOptions, &secrets).await;
            }
            "pgoptions-blank" => {
                let url = format!("postgres://carddemo:{SECRET}@127.0.0.1/carddemo");
                let options = connect_options(&url).unwrap_or_else(|reason| panic!("{reason}"));
                // Only the whitespace, which the server skips, reaches it as server options.
                assert_eq!(options.get_options(), Some(BLANK_PGOPTIONS));
            }
            other => panic!("unknown child mode {other}"),
        }
        println!("{CHILD_PASSED} {mode}");
    }

    #[tokio::test]
    async fn an_unreachable_server_is_reported_by_its_cause() {
        // Nothing listens on the port of a dropped listener, so a connection to it is refused.
        let refused_port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free local port")
            .port();
        // This peer accepts and never answers, reading until the client goes away.
        let silent = TcpListener::bind("127.0.0.1:0").expect("bind a local listener");
        let silent_port = silent.local_addr().expect("listener address").port();
        let peer = thread::spawn(move || {
            silent
                .accept()
                .and_then(|(mut stream, _)| io::copy(&mut stream, &mut io::sink()))
        });
        let cases = [
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1:{refused_port}/carddemo"),
                CONNECT_TIMEOUT,
                "io (connection refused)",
            ),
            (
                format!("postgres://carddemo:{SECRET}@no-such-host.invalid/carddemo"),
                CONNECT_TIMEOUT,
                "io (host name not resolved)",
            ),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1:{silent_port}/carddemo"),
                Duration::from_millis(200),
                "io (timed out)",
            ),
        ];
        for (url, timeout, expected) in cases {
            let config = Config {
                database_url: url,
                ..Config::default()
            };
            let started = Instant::now();
            let (result, output) = logged_while(connect_within(&config, timeout)).await;
            let elapsed = started.elapsed();
            let error = match result {
                Err(error @ sqlx::Error::Io(_)) => error,
                Err(other) => panic!("expected {expected}, got {other}"),
                Ok(_) => panic!("an unreachable server must not yield a pool ({expected})"),
            };
            let summary = ErrorSummary::from(&error).to_string();
            assert_eq!(summary, expected);
            // Reported by the first connection, not after the pool's acquire timeout.
            assert!(elapsed < CONNECT_TIMEOUT / 2, "{expected} took {elapsed:?}");
            for text in [error.to_string(), format!("{error:?}"), summary, output] {
                assert_free_of(&text, &[SECRET]);
            }
        }
        peer.join()
            .expect("the peer thread ends")
            .expect("the peer read until the client went away");
    }
}
