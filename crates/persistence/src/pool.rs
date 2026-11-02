//! [`Db`], the `PostgreSQL` connection pool, and [`connect`], which opens it.
//!
//! A process opens one pool at start-up and shares it: `api` and `mq` clone
//! the [`Db`] into every request handler and consumer, and each `batch`
//! subcommand opens one for its run. Adapters hold a [`Db`] and begin their
//! [`UnitOfWork`] through [`Db::begin`], or [`Db::begin_snapshot`] for a
//! read-only unit on one snapshot, so none of them names sqlx.
//!
//! The database URL comes from the caller. `CARDDEMO_DATABASE_URL` is read by
//! the settings of `carddemo-runtime`; this crate reads no environment. The
//! URL carries the password, so nothing here logs or displays it, and the
//! [`Debug`] form of a [`Db`] shows only the pool's size and options.
//!
//! sqlx logs every query parameter of the URL that it does not read, at
//! `WARN` and with the parameter's value, and that value can be a credential:
//! libpq's `sslpassword`, or a misspelt `password`. [`connect`] therefore
//! refuses a URL whose query holds such a parameter before sqlx parses it,
//! naming the parameter but never its value.
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
    /// the pages of the live transaction master this way.
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
/// request.
///
/// # Errors
///
/// [`PersistenceError::Sqlx`] when `url` does not parse as a connection URL,
/// or when the first connection cannot be opened or authenticated. A query
/// parameter sqlx does not read is refused first, as a
/// [`sqlx::Error::Configuration`] that names the parameter but not its value
/// (nor any of its text when it has no `=`), because sqlx would log that
/// value.
pub async fn connect(url: &str) -> Result<Db, PersistenceError> {
    check_query_parameters(url)?;
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(url)
        .await?;
    // The URL is left out of the event: it carries the password.
    tracing::debug!(max_connections = MAX_CONNECTIONS, "database pool connected");
    Ok(Db::from_pool(pool))
}

/// Refuses `url` when its query holds a parameter sqlx does not read, which
/// sqlx would log at `WARN` together with its value.
///
/// The error names the parameter by its decoded name. A parameter without `=`
/// is a name alone, and that text may itself be a credential, so it is named
/// only by its position in the query.
fn check_query_parameters(url: &str) -> Result<(), PersistenceError> {
    for (position, (name, has_value)) in query_parameters(url).iter().enumerate() {
        if is_recognized(name) {
            continue;
        }
        let message = if *has_value {
            format!("unsupported connection URL query parameter {name:?} (value withheld)")
        } else {
            format!(
                "unsupported connection URL query parameter {} without \"=\" (text withheld)",
                position + 1
            )
        };
        return Err(PersistenceError::Sqlx(sqlx::Error::Configuration(
            message.into(),
        )));
    }
    Ok(())
}

/// Whether sqlx reads the query parameter `name`.
fn is_recognized(name: &str) -> bool {
    RECOGNIZED_PARAMETERS.contains(&name)
        || name
            .strip_prefix(OPTION_PREFIX)
            .is_some_and(|rest| rest.ends_with(OPTION_SUFFIX))
}

/// The query parameters of `url` as sqlx sees them, in order: each decoded
/// name, with whether a `=` follows it.
///
/// This takes the names exactly as sqlx's parser does, `url` 2.5 then the
/// `form_urlencoded` pairs of the query, so that no parameter reaches sqlx
/// under a name this check did not see.
fn query_parameters(url: &str) -> Vec<(String, bool)> {
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
            Some((name, _)) => (decode_component(name), true),
            None => (decode_component(segment), false),
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
    use std::pin::pin;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::task::{Context, Poll, Waker};

    use sqlx::postgres::PgConnectOptions;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::{PersistenceError, check_query_parameters, connect};

    /// A stand-in for a credential written into a connection URL.
    const SECRET: &str = "not-a-real-secret";

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

    /// The message of the configuration error `url` is refused with.
    fn refusal(url: &str) -> String {
        match check_query_parameters(url) {
            Err(PersistenceError::Sqlx(sqlx::Error::Configuration(message))) => message.to_string(),
            Err(other) => panic!("{url:?}: expected a configuration error, got {other}"),
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
            // sqlx matches the decoded name.
            "postgres://u:p@h/db?ssl%6Dode=disable",
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
        ];
        for (url, expected) in cases {
            let message = refusal(url);
            assert_eq!(message, expected, "{url:?}");
            assert!(!message.contains(SECRET), "{url:?}: {message}");
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
            let (poll, fields) = captured(|| {
                let mut future = pin!(connect(url));
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
            });
            assert!(
                matches!(
                    poll,
                    Poll::Ready(Err(PersistenceError::Sqlx(sqlx::Error::Configuration(_))))
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
