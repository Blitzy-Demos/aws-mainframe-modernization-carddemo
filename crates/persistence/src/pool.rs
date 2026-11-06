//! [`Db`], the `PostgreSQL` connection pool, and [`connect`], which opens it,
//! or [`connect_with_statement_timeout`], which also has the server cancel any
//! statement that runs too long.
//!
//! A process opens one pool at start-up and shares it: `api` and `mq` clone
//! the [`Db`] into every request handler and consumer, and each `batch`
//! subcommand opens one for its run. `mq` opens its pool with a statement
//! timeout, so a statement blocked behind another session's lock cannot hold
//! a consumer. Adapters hold a [`Db`] and begin their [`UnitOfWork`] through
//! [`Db::begin`], or [`Db::begin_snapshot`] for a read-only unit on one
//! snapshot, so none of them names sqlx.
//!
//! The database URL comes from the caller. `CARDDEMO_DATABASE_URL` is read by
//! the settings of `carddemo-runtime`; this crate reads no environment. The
//! URL carries the password, so nothing here logs or displays it, and the
//! [`Debug`] form of a [`Db`] shows only the pool's size and options.
//!
//! A URL refused before any connection attempt, one that does not parse or
//! one refused as below, is a [`PersistenceError::InvalidDatabaseUrl`]: its
//! text names `CARDDEMO_DATABASE_URL`, the variable every process passes the
//! URL in, and the reason, never any part of the URL.
//!
//! sqlx logs every query parameter of the URL that it does not read, at
//! `WARN` and with the parameter's value, and that value can be a credential:
//! libpq's `sslpassword`, or a misspelt `password`. Its parse error for an
//! `sslmode` value it does not know quotes that value. [`connect`] and
//! [`connect_with_statement_timeout`] therefore refuse a URL whose query holds
//! such a parameter or value before sqlx parses it, naming the parameter but
//! never its value.
//!
//! [`connect`] and [`connect_with_statement_timeout`] open the first connection
//! directly, in one attempt sqlx does not retry, beside the pool's own first
//! connection. A server that refuses the connection or turns it away therefore
//! reports its cause at once, `Connection refused (os error 111)` for example,
//! instead of the `pool timed out while waiting for an open connection` that
//! sqlx's pool returns after retrying a refused connection for its whole
//! 30-second acquire timeout. A server that never answers still fails at that
//! acquire timeout, and once the pool holds its first connection the opener
//! returns without waiting for the direct attempt, which only diagnoses.
//!
//! The pool runs on the caller's tokio runtime through sqlx's `runtime-tokio`
//! feature; this module starts none.

use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};
use sqlx::{Connection, PgPool};

use crate::PersistenceError;
use crate::tx::UnitOfWork;

/// The most connections one process keeps open. Each [`UnitOfWork`] holds one
/// connection until it commits or rolls back, so this bounds the units of
/// work a process runs at once; further [`Db::begin`] calls wait for a free
/// connection.
const MAX_CONNECTIONS: u32 = 10;

/// The URL query parameters sqlx 0.8.6 reads, spelt and cased exactly as it
/// matches them. Besides these it reads `options[NAME]`
/// ([`OPTION_PREFIX`] … [`OPTION_SUFFIX`]); it logs any other parameter.
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

/// The start of a parameter that sets one server option, `options[NAME]`.
const OPTION_PREFIX: &str = "options[";

/// The end of an `options[NAME]` parameter. sqlx ignores a parameter that has
/// the prefix but not this end, so such a parameter is refused as well.
const OPTION_SUFFIX: char = ']';

/// The two names sqlx 0.8.6 reads the SSL mode under.
const SSL_MODE_PARAMETERS: [&str; 2] = ["sslmode", "ssl-mode"];

/// The SSL modes sqlx 0.8.6 knows, matched after ASCII lower-casing. Its
/// error for any other value quotes the value.
const SSL_MODES: [&str; 6] = [
    "disable",
    "allow",
    "prefer",
    "require",
    "verify-ca",
    "verify-full",
];

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

    /// Takes a connection from the pool and begins a read-only transaction
    /// whose statements all read one snapshot: `SET TRANSACTION ISOLATION
    /// LEVEL REPEATABLE READ, READ ONLY` runs as its first statement.
    ///
    /// Every statement of the unit of work sees the database as it was
    /// committed when the first of them began, so a read spread over several
    /// statements is one consistent read, as a single `SELECT` is. Nothing can
    /// be written in it, and it blocks no writer. The TRANREPT backup reads
    /// the pages of the live transaction master this way, and so does the CR00
    /// report, whose pages and lookups between them must see one state: a
    /// `TRANBKP`, `COMBTRAN` or CT02 commit between two pages would otherwise
    /// tear the report.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Sqlx`] in every case [`Db::begin`] fails, and when
    /// the `SET TRANSACTION` fails; the begun transaction is then dropped,
    /// which rolls it back.
    pub async fn begin_snapshot(&self) -> Result<UnitOfWork, PersistenceError> {
        let mut uow = self.begin().await?;
        sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut **uow.transaction())
            .await?;
        Ok(uow)
    }

    /// Closes the pool, for a process that is stopping.
    ///
    /// Every later [`Db::begin`], on this handle or any clone, fails at once.
    /// The call waits until each connection in use returns to the pool, that
    /// is until each open [`UnitOfWork`] commits, rolls back or is dropped,
    /// and then closes every connection, so `PostgreSQL` sees each session
    /// end cleanly.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Opens the process's pool on `url`, a `postgres://` connection URL, with at
/// most `MAX_CONNECTIONS` (10) connections.
///
/// One connection is opened at once, so a wrong URL, a wrong password or an
/// unreachable server fails here, at start-up, rather than on the first
/// request. That first connection is opened directly, in one attempt sqlx does
/// not retry, so a server that refuses it or turns it away reports its cause
/// at once, such as `Connection refused`; a server that never answers fails at
/// the pool's acquire timeout of 30 seconds.
///
/// # Errors
///
/// - [`PersistenceError::InvalidDatabaseUrl`] when `url` does not parse as a
///   connection URL, with sqlx's reason, `relative URL without a base` or
///   `invalid port number` for example. A query parameter sqlx does not read,
///   or an `sslmode` value it does not know, is refused first, naming the
///   parameter but not its value (nor any of its text when it has no `=`),
///   because sqlx would log or quote that value.
/// - [`PersistenceError::Sqlx`] when the first connection cannot be opened or
///   authenticated: the error of that attempt, [`sqlx::Error::Io`] for a
///   refused or unreachable server for example, or
///   [`sqlx::Error::PoolTimedOut`] when the server does not answer within the
///   acquire timeout.
pub async fn connect(url: &str) -> Result<Db, PersistenceError> {
    let pool = open_pool(parse_url(url)?).await?;
    // The URL is left out of the event: it carries the password.
    tracing::debug!(max_connections = MAX_CONNECTIONS, "database pool connected");
    Ok(Db::from_pool(pool))
}

/// Opens the process's pool as [`connect`] does, with the server's
/// `statement_timeout` set on every connection the pool opens: `PostgreSQL`
/// cancels any statement of the process that runs longer than
/// `statement_timeout`, a lock wait included, and fails it with SQLSTATE
/// `57014`, which maps to `PortError::Unavailable`.
///
/// A statement the server cancels fails like any other statement error: a
/// write rolls back to its savepoint and the unit of work stays usable, so
/// the writes before it can still be committed. The timeout is whole
/// milliseconds, rounded down, and never below one millisecond, because `0`
/// would turn the timeout off.
///
/// # Errors
///
/// Every error [`connect`] returns, the refusal of a query parameter or an
/// `sslmode` value included, checked before sqlx parses `url`. The first
/// connection is opened directly as [`connect`] opens it, so a server that
/// refuses it or turns it away reports its cause at once here too, and one
/// that never answers fails at the pool's acquire timeout.
pub async fn connect_with_statement_timeout(
    url: &str,
    statement_timeout: Duration,
) -> Result<Db, PersistenceError> {
    let timeout_ms = statement_timeout.as_millis().max(1);
    let options = parse_url(url)?.options([("statement_timeout", timeout_ms)]);
    let pool = open_pool(options).await?;
    // The URL is left out of the event: it carries the password.
    tracing::debug!(
        max_connections = MAX_CONNECTIONS,
        statement_timeout_ms = u64::try_from(timeout_ms).unwrap_or(u64::MAX),
        "database pool connected"
    );
    Ok(Db::from_pool(pool))
}

/// Opens a pool of at most [`MAX_CONNECTIONS`] connections on `options`, and
/// reports a first connection that fails by its cause.
///
/// sqlx's pool retries a connection the server refuses, or turns away as
/// starting up or full, until its acquire timeout of 30 seconds, and then
/// returns only [`sqlx::Error::PoolTimedOut`]. One direct connection attempt,
/// which nothing retries, therefore runs beside the pool's own first
/// connection, and [`first_connection`] decides between them:
///
/// - When the direct attempt fails before the pool has opened, its error is
///   returned at once: `Connection refused`, an unknown host, a failed
///   password or an unknown database.
/// - When it succeeds, its connection is closed while the pool's first
///   connection is still awaited.
/// - When the pool's first connection completes, its outcome is returned at
///   once, whatever the direct attempt or its close is still doing. For a
///   server that never answers that is the acquire timeout, so start-up ends
///   there, and not at the operating system's TCP connect timeout, which the
///   direct attempt alone would wait for because it has no deadline. A healthy
///   pool is likewise never held up by a direct attempt that stalls.
///
/// Neither error holds the URL or its password: sqlx's I/O, database and
/// timeout errors carry neither.
async fn open_pool(options: PgConnectOptions) -> Result<PgPool, PersistenceError> {
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect_with(options.clone());
    let probe = PgConnection::connect_with(&options);
    Ok(first_connection(pool, probe, PgConnection::close).await?)
}

/// Waits for `pool`, the pool's first connection, while `probe`, a direct
/// connection attempt on the same server, runs beside it, and returns the
/// pool's outcome, or the probe's error when the probe fails first.
///
/// One poll loop drives three futures until the pool's first connection
/// completes, and never polls one of them again once it has completed:
///
/// - `probe`: its error, when the pool has not completed in the same poll, is
///   returned at once. The connection it opens is handed to `close`.
/// - the future `close` returns: driven beside the pool, its outcome ignored.
///   The probe only diagnoses why a server cannot be reached; the pool holds
///   connections of its own, so a failed close says nothing about the pool
///   and must not fail start-up.
/// - `pool`: its outcome is returned the moment it completes, even in the poll
///   in which the probe fails, because a pool that opened can serve. A probe or
///   a close still pending is then dropped, which drops its socket, so neither
///   can hold start-up past the pool, whose own acquire timeout bounds the
///   whole wait.
async fn first_connection<T, C, E, Close>(
    pool: impl Future<Output = Result<T, E>>,
    probe: impl Future<Output = Result<C, E>>,
    mut close: impl FnMut(C) -> Close,
) -> Result<T, E>
where
    Close: Future,
{
    let mut pool = pin!(pool);
    // Each slot is emptied once its future completes, so none is polled again.
    let mut probe = pin!(Some(probe));
    let mut closing = pin!(None::<Close>);
    poll_fn(|cx| {
        let mut probe_error = None;
        if let Some(attempt) = probe.as_mut().as_pin_mut() {
            if let Poll::Ready(outcome) = attempt.poll(cx) {
                probe.set(None);
                match outcome {
                    Ok(connection) => closing.set(Some(close(connection))),
                    Err(error) => probe_error = Some(error),
                }
            }
        }
        if let Some(pending_close) = closing.as_mut().as_pin_mut() {
            if pending_close.poll(cx).is_ready() {
                closing.set(None);
            }
        }
        match (pool.as_mut().poll(cx), probe_error) {
            (Poll::Ready(outcome), _) => Poll::Ready(outcome),
            (Poll::Pending, Some(error)) => Poll::Ready(Err(error)),
            (Poll::Pending, None) => Poll::Pending,
        }
    })
    .await
}

/// Parses `url` into the options sqlx connects with, once
/// [`check_query_parameters`] has refused what sqlx would log or quote.
///
/// The reason of a parse failure is sqlx's: a URL syntax, UTF-8, number or IP
/// address error, none of which quotes the URL. It is taken from inside
/// sqlx's configuration error, without that error's `error with
/// configuration: ` prefix.
///
/// # Errors
///
/// [`PersistenceError::InvalidDatabaseUrl`] when [`check_query_parameters`]
/// refuses `url` or sqlx cannot parse it.
fn parse_url(url: &str) -> Result<PgConnectOptions, PersistenceError> {
    check_query_parameters(url)?;
    // The parse `PgPoolOptions::connect` would make of `url`.
    url.parse::<PgConnectOptions>()
        .map_err(|error| PersistenceError::InvalidDatabaseUrl {
            reason: match error {
                sqlx::Error::Configuration(source) => source.to_string(),
                other => other.to_string(),
            },
        })
}

/// Refuses `url` when its query holds a parameter sqlx does not read, which
/// sqlx would log at `WARN` together with its value, or an `sslmode` (or
/// `ssl-mode`) value sqlx does not know, which its parse error would quote.
///
/// The error names the parameter by its decoded name. A parameter without `=`
/// is a name alone, and that text may itself be a credential, so it is named
/// only by its position in the query. An `sslmode` without `=` has the empty
/// value, as sqlx reads it, and is refused as unknown.
fn check_query_parameters(url: &str) -> Result<(), PersistenceError> {
    for (position, (name, value)) in query_parameters(url).iter().enumerate() {
        let reason = if is_recognized(name) {
            if !SSL_MODE_PARAMETERS.contains(&name.as_str())
                || is_ssl_mode(value.as_deref().unwrap_or_default())
            {
                continue;
            }
            format!(
                "unsupported value of connection URL query parameter {name:?} (value withheld); \
                 expected one of {}",
                SSL_MODES.join(", ")
            )
        } else if value.is_some() {
            format!("unsupported connection URL query parameter {name:?} (value withheld)")
        } else {
            format!(
                "unsupported connection URL query parameter {} without \"=\" (text withheld)",
                position + 1
            )
        };
        return Err(PersistenceError::InvalidDatabaseUrl { reason });
    }
    Ok(())
}

/// Whether sqlx reads `value`, decoded, as an SSL mode.
fn is_ssl_mode(value: &str) -> bool {
    SSL_MODES.contains(&value.to_ascii_lowercase().as_str())
}

/// Whether sqlx reads the query parameter `name`.
fn is_recognized(name: &str) -> bool {
    RECOGNIZED_PARAMETERS.contains(&name)
        || name
            .strip_prefix(OPTION_PREFIX)
            .is_some_and(|rest| rest.ends_with(OPTION_SUFFIX))
}

/// The query parameters of `url` as sqlx sees them, in order: each decoded
/// name, with its decoded value when a `=` follows it.
///
/// This takes the names and values exactly as sqlx's parser does, `url` 2.5
/// then the `form_urlencoded` pairs of the query, so that no parameter reaches
/// sqlx under a name or with a value this check did not see.
fn query_parameters(url: &str) -> Vec<(String, Option<String>)> {
    // C0 controls and spaces at either end are trimmed, and tabs and line
    // breaks are dropped wherever they stand.
    let url: String = url
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    // The query follows the first `?` when no `#` comes before it, and runs
    // to the next `#`, where the fragment starts.
    let after_query_mark = match url.find(['?', '#']) {
        Some(at) if url[at..].starts_with('?') => &url[at + 1..],
        _ => return Vec::new(),
    };
    let query = after_query_mark.split('#').next().unwrap_or_default();
    query
        .split('&')
        .filter(|segment| !segment.is_empty())
        .map(|segment| match segment.split_once('=') {
            Some((name, value)) => (decode_component(name), Some(decode_component(value))),
            None => (decode_component(segment), None),
        })
        .collect()
}

/// Decodes one `application/x-www-form-urlencoded` component: `+` becomes a
/// space, a valid `%XX` escape becomes its byte (a `%` without two hex digits
/// stays as it is), and bytes that are not UTF-8 become U+FFFD.
fn decode_component(raw: &str) -> String {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut rest = raw.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        let escaped = match (byte, tail) {
            (b'%', [high, low, ..]) => hex_digit(*high).zip(hex_digit(*low)),
            _ => None,
        };
        if let Some((high, low)) = escaped {
            decoded.push(high * 16 + low);
            rest = &tail[2..];
        } else {
            decoded.push(if byte == b'+' { b' ' } else { byte });
            rest = tail;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// The value of one hexadecimal digit, in either case.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::future::Future;
    use std::io::ErrorKind;
    use std::net::{Ipv4Addr, TcpListener};
    use std::pin::{Pin, pin};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use sqlx::postgres::PgConnectOptions;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::{
        Db, PersistenceError, check_query_parameters, connect, connect_with_statement_timeout,
        first_connection,
    };

    /// A stand-in for a credential written into a connection URL.
    const SECRET: &str = "not-a-real-secret";

    /// The start of the text of every URL refused before a connection
    /// attempt. The `mq` start-up test checks that the binary's report names
    /// the settings' key for the URL the same way.
    const INVALID_URL: &str = "invalid CARDDEMO_DATABASE_URL: ";

    /// How long a refused first connection may take to be reported: well
    /// under the pool's 30-second acquire timeout, which sqlx's retries of a
    /// refused connection would run out.
    const PROMPT: Duration = Duration::from_secs(10);

    /// The field values a visitor was shown, each as `name=value` text.
    struct FieldText(Vec<String>);

    impl Visit for FieldText {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }

    /// A subscriber that keeps the text of every field of every span and
    /// event, at every level.
    #[derive(Clone, Default)]
    struct Capture {
        fields: Arc<Mutex<Vec<String>>>,
    }

    impl Capture {
        fn keep(&self, record: impl FnOnce(&mut FieldText)) {
            let mut text = FieldText(Vec::new());
            record(&mut text);
            self.fields
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(text.0);
        }

        fn fields(&self) -> Vec<String> {
            self.fields
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            self.keep(|text| span.record(text));
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, values: &Record<'_>) {
            self.keep(|text| values.record(text));
        }

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            self.keep(|text| event.record(text));
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    /// Runs `body` with a [`Capture`] as the thread's subscriber, returning
    /// its result and the field text of everything it traced.
    fn captured<T>(body: impl FnOnce() -> T) -> (T, Vec<String>) {
        let capture = Capture::default();
        let result = tracing::subscriber::with_default(capture.clone(), body);
        (result, capture.fields())
    }

    /// The reason `url` is refused with, checked to follow the name of
    /// `CARDDEMO_DATABASE_URL` in the error's text.
    fn refusal(url: &str) -> String {
        match check_query_parameters(url) {
            Err(error @ PersistenceError::InvalidDatabaseUrl { .. }) => {
                let text = error.to_string();
                match text.strip_prefix(INVALID_URL) {
                    Some(reason) => reason.to_owned(),
                    None => panic!("{url:?}: the error does not name the variable: {text}"),
                }
            }
            Err(other) => panic!("{url:?}: expected an invalid URL error, got {other}"),
            Ok(()) => panic!("{url:?}: accepted"),
        }
    }

    #[test]
    fn parameters_sqlx_reads_are_accepted() {
        let cases = [
            // The local default and the Compose `app` profile URL.
            "postgres://carddemo:carddemo@localhost:5432/carddemo",
            "postgres://carddemo:carddemo@postgres:5432/carddemo",
            // Every parameter sqlx 0.8.6 reads.
            "postgres://u:p@h/db?sslmode=disable&ssl-mode=disable&sslrootcert=/ca.pem\
             &ssl-root-cert=/ca.pem&ssl-ca=/ca.pem&sslcert=/c.pem&ssl-cert=/c.pem\
             &sslkey=/k.pem&ssl-key=/k.pem&statement-cache-capacity=100&host=h\
             &hostaddr=127.0.0.1&port=5432&dbname=db&user=u&password=p\
             &application_name=carddemo&options=-c%20search_path%3Dpublic",
            "postgres://u:p@h/db?options[search_path]=public",
            "postgres://u:p@h/db?options%5Bsearch_path%5D=public",
            // sqlx matches the decoded name, and the SSL mode in any case.
            "postgres://u:p@h/db?ssl%6Dode=disable",
            "postgres://u:p@h/db?sslmode=Verify-Full&ssl-mode=REQUIRE&sslmode=%61llow",
            // The URL parser drops tabs and line breaks inside the name.
            "postgres://u:p@h/db?ssl\tmo\r\nde=disable",
            // A name without `=` that sqlx reads gets an empty value.
            "postgres://u:p@h/db?application_name",
            // Empty segments, an empty query and the trimmed ends.
            "postgres://u:p@h/db?&&sslmode=disable&",
            "postgres://u:p@h/db?",
            "\t postgres://u:p@h/db?port=5432 \n",
            // The fragment is not part of the query.
            "postgres://u:p@h/db?sslmode=disable#sslpassword=not-a-real-secret",
            "postgres://u:p@h/db#?sslpassword=not-a-real-secret",
        ];
        for url in cases {
            if let Err(err) = check_query_parameters(url) {
                panic!("{url:?} refused: {err}");
            }
            if let Err(err) = url.parse::<PgConnectOptions>() {
                panic!("{url:?} does not parse in sqlx: {err}");
            }
        }
    }

    #[test]
    fn parameters_sqlx_would_log_are_refused_without_their_value() {
        let cases = [
            (
                "postgres://u:p@h/db?sslpassword=not-a-real-secret",
                r#"unsupported connection URL query parameter "sslpassword" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?pasword=not-a-real-secret",
                r#"unsupported connection URL query parameter "pasword" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?SSLMODE=not-a-real-secret",
                r#"unsupported connection URL query parameter "SSLMODE" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?sslmode=disable&sslpassword=not-a-real-secret",
                r#"unsupported connection URL query parameter "sslpassword" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?ssl%70assword=not-a-real-secret",
                r#"unsupported connection URL query parameter "sslpassword" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?ssl+password=not-a-real-secret",
                r#"unsupported connection URL query parameter "ssl password" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?ssl%0Apassword=not-a-real-secret",
                r#"unsupported connection URL query parameter "ssl\npassword" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?%FF=not-a-real-secret",
                "unsupported connection URL query parameter \"\u{FFFD}\" (value withheld)",
            ),
            (
                "postgres://u:p@h/db?=not-a-real-secret",
                r#"unsupported connection URL query parameter "" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?options[x=not-a-real-secret",
                r#"unsupported connection URL query parameter "options[x" (value withheld)"#,
            ),
            (
                "postgres://u:p@h/db?not-a-real-secret",
                r#"unsupported connection URL query parameter 1 without "=" (text withheld)"#,
            ),
            (
                "postgres://u:p@h/db?sslmode=disable&&not-a-real-secret%3Dx#y",
                r#"unsupported connection URL query parameter 2 without "=" (text withheld)"#,
            ),
            // sqlx's error for an SSL mode it does not know quotes the value.
            (
                "postgres://u:p@h/db?sslmode=not-a-real-secret",
                r#"unsupported value of connection URL query parameter "sslmode" (value withheld); expected one of disable, allow, prefer, require, verify-ca, verify-full"#,
            ),
            (
                "postgres://u:p@h/db?sslmode=disable&ssl-mode=not-a-real-secret",
                r#"unsupported value of connection URL query parameter "ssl-mode" (value withheld); expected one of disable, allow, prefer, require, verify-ca, verify-full"#,
            ),
            (
                "postgres://u:p@h/db?ssl%6Dode=dis+able",
                r#"unsupported value of connection URL query parameter "sslmode" (value withheld); expected one of disable, allow, prefer, require, verify-ca, verify-full"#,
            ),
            (
                "postgres://u:p@h/db?sslmode",
                r#"unsupported value of connection URL query parameter "sslmode" (value withheld); expected one of disable, allow, prefer, require, verify-ca, verify-full"#,
            ),
        ];
        for (url, expected) in cases {
            let message = refusal(url);
            assert_eq!(message, expected, "{url:?}");
            assert!(!message.contains(SECRET), "{url:?}: {message}");
            if expected.starts_with("unsupported value") {
                // sqlx refuses the value as well; the check only withholds it.
                assert!(url.parse::<PgConnectOptions>().is_err(), "{url:?}");
            }
        }
    }

    #[test]
    fn connect_refuses_before_sqlx_parses_so_no_event_holds_the_secret() {
        // sqlx's own parser logs the value of a parameter it does not read,
        // and the capture sees it.
        let logged = "postgres://u:p@h/db?sslpassword=not-a-real-secret";
        let (parsed, fields) = captured(|| logged.parse::<PgConnectOptions>());
        assert!(parsed.is_ok(), "{logged:?}");
        assert!(
            fields.iter().any(|field| field.contains(SECRET)),
            "{fields:?}"
        );

        for url in [logged, "postgres://u:p@h/db?not-a-real-secret"] {
            let openers = [
                captured(|| poll_once(connect(url))),
                captured(|| {
                    poll_once(connect_with_statement_timeout(url, Duration::from_secs(25)))
                }),
            ];
            for (poll, fields) in openers {
                assert!(
                    matches!(
                        poll,
                        Poll::Ready(Err(PersistenceError::InvalidDatabaseUrl { .. }))
                    ),
                    "{url:?}: {poll:?}"
                );
                assert!(
                    !fields.iter().any(|field| field.contains(SECRET)),
                    "{url:?}: {fields:?}"
                );
            }
        }
    }

    #[test]
    fn a_url_sqlx_cannot_parse_names_the_variable_and_the_reason_but_not_the_url() {
        let cases = [
            ("notaurl".to_owned(), "relative URL without a base"),
            (
                format!("postgres://carddemo:{SECRET}@127.0.0.1:notaport/carddemo"),
                "invalid port number",
            ),
            (
                format!("postgres://carddemo:{SECRET}@[::1/carddemo"),
                "invalid IPv6 address",
            ),
            (
                format!("postgres://carddemo:%FF{SECRET}@127.0.0.1/carddemo"),
                "invalid utf-8 sequence of 1 bytes from index 0",
            ),
            (
                format!("postgres://carddemo:carddemo@127.0.0.1/carddemo?port={SECRET}"),
                "invalid digit found in string",
            ),
            (
                format!("postgres://carddemo:carddemo@127.0.0.1/carddemo?hostaddr={SECRET}"),
                "invalid IP address syntax",
            ),
        ];
        for (url, reason) in &cases {
            let openers = [
                poll_once(connect(url)),
                poll_once(connect_with_statement_timeout(url, Duration::from_secs(25))),
            ];
            for poll in openers {
                let Poll::Ready(Err(error)) = poll else {
                    panic!("{url:?}: expected an immediate refusal, got {poll:?}");
                };
                assert!(
                    matches!(error, PersistenceError::InvalidDatabaseUrl { .. }),
                    "{url:?}: {error:?}"
                );
                let text = error.to_string();
                assert_eq!(text, format!("{INVALID_URL}{reason}"), "{url:?}");
                for withheld in [SECRET, url.as_str()] {
                    assert!(!text.contains(withheld), "{url:?}: {text}");
                    assert!(
                        !format!("{error:?}").contains(withheld),
                        "{url:?}: {error:?}"
                    );
                }
            }
        }
    }

    /// A loopback port nothing listens on: bound to take a free port, then
    /// released, so a connection to it is refused.
    fn refused_port() -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap_or_else(|error| panic!("cannot bind a loopback port: {error}"));
        listener
            .local_addr()
            .unwrap_or_else(|error| panic!("cannot read the bound loopback port: {error}"))
            .port()
    }

    /// The URL of a server that refuses every connection, with [`SECRET`] as
    /// its password.
    fn refused_url() -> String {
        format!(
            "postgres://carddemo:{SECRET}@127.0.0.1:{}/carddemo",
            refused_port()
        )
    }

    #[test]
    fn a_refused_first_connection_reports_its_cause_at_once_without_the_secret() {
        /// One of the two openers, run on a URL.
        type Opener = fn(String) -> Pin<Box<dyn Future<Output = Result<Db, PersistenceError>>>>;
        let openers: [(&str, Opener); 2] = [
            ("connect", |url| {
                Box::pin(async move { connect(&url).await })
            }),
            ("connect_with_statement_timeout", |url| {
                Box::pin(async move {
                    connect_with_statement_timeout(&url, Duration::from_secs(25)).await
                })
            }),
        ];
        for (opener, open) in openers {
            let url = refused_url();
            let started = Instant::now();
            let outcome = carddemo_testkit::block_on(open(url.clone()));
            let elapsed = started.elapsed();
            let error = match outcome {
                Err(error) => error,
                Ok(db) => panic!("{opener}: connected to a refused port: {db:?}"),
            };
            let text = error.to_string();
            match &error {
                PersistenceError::Sqlx(sqlx::Error::Io(cause)) => {
                    assert_eq!(
                        cause.kind(),
                        ErrorKind::ConnectionRefused,
                        "{opener}: {text}"
                    );
                    assert!(text.contains(&cause.to_string()), "{opener}: {text}");
                }
                other => panic!("{opener}: expected the refused connection, got {other:?}"),
            }
            assert!(elapsed < PROMPT, "{opener}: took {elapsed:?}: {text}");
            for withheld in [SECRET, url.as_str()] {
                assert!(!text.contains(withheld), "{opener}: {text}");
                assert!(
                    !format!("{error:?}").contains(withheld),
                    "{opener}: {error:?}"
                );
            }
        }
    }

    /// The most polls a race case may take. Its scripted futures are ready
    /// within a few polls, so a race still pending after this many waits on a
    /// future that never completes.
    const MAX_POLLS: usize = 16;

    /// A stand-in for one future of the start-up race, polled by hand: pending
    /// for its first `pending_polls` polls and then ready with `output`, or
    /// pending for ever when `pending_polls` is `None`. A poll after it is
    /// ready panics, so a race that polls a completed future again fails.
    struct Scripted<T> {
        pending_polls: Option<usize>,
        output: Option<T>,
    }

    impl<T> Scripted<T> {
        fn after(pending_polls: usize, output: T) -> Self {
            Self {
                pending_polls: Some(pending_polls),
                output: Some(output),
            }
        }

        fn never() -> Self {
            Self {
                pending_polls: None,
                output: None,
            }
        }
    }

    impl<T: Unpin> Future for Scripted<T> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
            let this = self.get_mut();
            match &mut this.pending_polls {
                None => Poll::Pending,
                Some(0) => Poll::Ready(
                    this.output
                        .take()
                        .unwrap_or_else(|| panic!("polled again after it was ready")),
                ),
                Some(left) => {
                    *left -= 1;
                    Poll::Pending
                }
            }
        }
    }

    #[test]
    fn the_pool_outcome_ends_the_start_up_race_and_a_probe_failing_first_is_reported() {
        /// A scripted future with a value or an error.
        type Script<T> = Scripted<Result<T, &'static str>>;
        /// One race: its three futures, the outcome, the poll it completes
        /// on, and the probe connections handed to the close.
        struct Case {
            name: &'static str,
            probe: Script<&'static str>,
            close: Script<()>,
            pool: Script<&'static str>,
            outcome: Result<&'static str, &'static str>,
            polls: usize,
            closed: &'static [&'static str],
        }
        let cases = [
            Case {
                name: "a probe that stalls does not hold up the pool",
                probe: Scripted::never(),
                close: Scripted::never(),
                pool: Scripted::after(0, Ok("pool")),
                outcome: Ok("pool"),
                polls: 1,
                closed: &[],
            },
            Case {
                name: "a probe failing before the pool opens is the outcome",
                probe: Scripted::after(0, Err("connection refused")),
                close: Scripted::never(),
                pool: Scripted::never(),
                outcome: Err("connection refused"),
                polls: 1,
                closed: &[],
            },
            Case {
                name: "a close that stalls does not hold up the pool",
                probe: Scripted::after(0, Ok("probe")),
                close: Scripted::never(),
                pool: Scripted::after(2, Ok("pool")),
                outcome: Ok("pool"),
                polls: 3,
                closed: &["probe"],
            },
            Case {
                name: "a pool failing while the probe waits is the outcome",
                probe: Scripted::never(),
                close: Scripted::never(),
                pool: Scripted::after(0, Err("pool timed out")),
                outcome: Err("pool timed out"),
                polls: 1,
                closed: &[],
            },
            Case {
                name: "a probe closed, even by a failed close, leaves the pool's later outcome",
                probe: Scripted::after(0, Ok("probe")),
                close: Scripted::after(1, Err("close failed")),
                pool: Scripted::after(3, Ok("pool")),
                outcome: Ok("pool"),
                polls: 4,
                closed: &["probe"],
            },
            Case {
                name: "a pool opening in the poll the probe fails in stands",
                probe: Scripted::after(1, Err("too many clients")),
                close: Scripted::never(),
                pool: Scripted::after(1, Ok("pool")),
                outcome: Ok("pool"),
                polls: 2,
                closed: &[],
            },
        ];
        for case in cases {
            let mut closed = Vec::new();
            let mut close = Some(case.close);
            let finished = {
                let mut race = pin!(first_connection(case.pool, case.probe, |connection| {
                    closed.push(connection);
                    close
                        .take()
                        .unwrap_or_else(|| panic!("{}: closed twice", case.name))
                }));
                (1..=MAX_POLLS).find_map(|poll| match poll_once(race.as_mut()) {
                    Poll::Ready(outcome) => Some((outcome, poll)),
                    Poll::Pending => None,
                })
            };
            assert_eq!(finished, Some((case.outcome, case.polls)), "{}", case.name);
            assert_eq!(closed, case.closed, "{}", case.name);
        }
    }

    /// Polls `future` once, with a waker that does nothing.
    fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
        let mut future = pin!(future);
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }
}
