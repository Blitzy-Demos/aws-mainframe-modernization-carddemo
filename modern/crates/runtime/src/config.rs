//! Environment configuration of the `CardDemo` replacement binaries (`carddemo-api`,
//! `carddemo-mq`, `carddemo-batch`, `carddemo-migrate`).
//!
//! [`Config::from_env`] is the only Rust application code that reads the process environment for
//! application settings. It reads the variables that `modern/.env.example` lists, each with the
//! local default stated on the matching [`Config`] field. `RUST_LOG` is read by the tracing filter
//! in `runtime::telemetry`, and `SQLX_OFFLINE` only by the `sqlx` macros at build time.
//!
//! Nothing here loads a `.env` file. A process sees `modern/.env` only when its shell exported it
//! first (`set -a; . ./.env; set +a`), as every script in `modern/scripts/` does. Relative paths
//! are kept as given and resolve against `modern/`, the working directory of every binary.
//!
//! Every default names a local resource: the Compose `PostgreSQL` and `RabbitMQ` services on
//! `localhost` and directories of the local checkout. Values can be credentials, so neither the
//! [`Config`] `Debug` output nor any [`EnvError`] message ever shows the database URL, the AMQP
//! URL, the JWT secret or the text of a rejected value.
//!
//! Reply and error queue names are literals of the COBOL programs, owned by the `messaging`
//! crate, and are not configuration. Only the request queues are: in the COBOL, the trigger
//! message named them.

use std::env::{self, VarError};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

const DATABASE_URL: &str = "DATABASE_URL";
const CARDDEMO_AMQP_URL: &str = "CARDDEMO_AMQP_URL";
const CARDDEMO_QUEUE_REQUEST_DATE: &str = "CARDDEMO_QUEUE_REQUEST_DATE";
const CARDDEMO_QUEUE_REQUEST_ACCT: &str = "CARDDEMO_QUEUE_REQUEST_ACCT";
const CARDDEMO_QUEUE_REQUEST_PAUTH: &str = "CARDDEMO_QUEUE_REQUEST_PAUTH";
const CARDDEMO_MQ_POLL_MS: &str = "CARDDEMO_MQ_POLL_MS";
const CARDDEMO_API_BIND: &str = "CARDDEMO_API_BIND";
const CARDDEMO_JWT_SECRET: &str = "CARDDEMO_JWT_SECRET";
const CARDDEMO_JWT_TTL_SECONDS: &str = "CARDDEMO_JWT_TTL_SECONDS";
const CARDDEMO_VAR_DIR: &str = "CARDDEMO_VAR_DIR";
const CARDDEMO_STATEMENT_DIR: &str = "CARDDEMO_STATEMENT_DIR";
const CARDDEMO_INTRDR_DIR: &str = "CARDDEMO_INTRDR_DIR";
const CARDDEMO_FIXTURE_DIR: &str = "CARDDEMO_FIXTURE_DIR";
const CARDDEMO_OTEL_EXPORT: &str = "CARDDEMO_OTEL_EXPORT";

/// Every variable [`Config::from_env`] reads. `RUST_LOG`, the fifteenth application variable of
/// `modern/.env.example`, belongs to the tracing filter and is deliberately absent.
const VARIABLES: [&str; 14] = [
    DATABASE_URL,
    CARDDEMO_AMQP_URL,
    CARDDEMO_QUEUE_REQUEST_DATE,
    CARDDEMO_QUEUE_REQUEST_ACCT,
    CARDDEMO_QUEUE_REQUEST_PAUTH,
    CARDDEMO_MQ_POLL_MS,
    CARDDEMO_API_BIND,
    CARDDEMO_JWT_SECRET,
    CARDDEMO_JWT_TTL_SECONDS,
    CARDDEMO_VAR_DIR,
    CARDDEMO_STATEMENT_DIR,
    CARDDEMO_INTRDR_DIR,
    CARDDEMO_FIXTURE_DIR,
    CARDDEMO_OTEL_EXPORT,
];

// Local defaults; user, password and database are those of the Compose services and of the
// `carddemo` user in `rabbitmq/definitions.json`.
const DEFAULT_DATABASE_URL: &str = "postgres://carddemo:carddemo@localhost:5432/carddemo";
const DEFAULT_AMQP_URL: &str = "amqp://carddemo:carddemo@localhost:5672/%2f";
const DEFAULT_QUEUE_REQUEST_DATE: &str = "CARD.DEMO.REQUEST.DATE";
const DEFAULT_QUEUE_REQUEST_ACCT: &str = "CARD.DEMO.REQUEST.ACCT";
const DEFAULT_QUEUE_REQUEST_PAUTH: &str = "CARD.DEMO.REQUEST.PAUTH";
const DEFAULT_MQ_POLL_MS: u64 = 250;
/// The longest `CARDDEMO_MQ_POLL_MS` accepted: five seconds. The poll replaces the `MQGMO-WAIT`
/// of `CODATE01`, `COACCT01` and `COPAUA0C`, each of which waits at most 5000 ms for a request
/// (`CODATE01.cbl:286`, `COACCT01.cbl:337`, `COPAUA0C.cbl:242`, as
/// `modern/audit/messaging/{CODATE01,COACCT01,COPAUA0C}.md` record). It also equals the reconnect
/// backoff cap of the `messaging` crate (`broker::BACKOFF_CAP`), so no configuration leaves an
/// empty request queue unpolled for longer.
const MAX_MQ_POLL_MS: u64 = 5_000;
const DEFAULT_API_BIND: &str = "127.0.0.1:8080";
const DEFAULT_JWT_TTL_SECONDS: u64 = 3600;
const DEFAULT_VAR_DIR: &str = "./var";
const DEFAULT_FIXTURE_DIR: &str = "../app/data/ASCII";

// Subdirectories of the variable-data root, one per output kind.
const STATEMENTS_SUBDIR: &str = "statements";
const JOBS_SUBDIR: &str = "jobs";
const DALYREJS_SUBDIR: &str = "dalyrejs";
const SYSTRAN_SUBDIR: &str = "systran";
const REPORTS_SUBDIR: &str = "reports";

const OTEL_EXPORT_STDOUT: &str = "stdout";

const EXPECTED_POSITIVE_INTEGER: &str = "a positive integer of decimal digits";
/// [`EXPECTED_POSITIVE_INTEGER`] bounded by [`MAX_MQ_POLL_MS`].
const EXPECTED_MQ_POLL_MS: &str = "a positive integer of decimal digits, at most 5000";
const EXPECTED_OTEL_EXPORT: &str = "unset, empty or stdout";

/// Text the `Debug` output of [`Config`] shows in place of a credential.
const REDACTED: &str = "<redacted>";

/// Whether spans are exported through `OpenTelemetry` (`CARDDEMO_OTEL_EXPORT`), as
/// `runtime::telemetry::init` applies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OtelExport {
    /// No span export; the JSON tracing output on standard output is unaffected.
    #[default]
    Off,
    /// `OpenTelemetry` spans are written to standard output by the `opentelemetry-stdout`
    /// exporter. There is no network exporter.
    Stdout,
}

/// Application settings of one process, read once at startup.
///
/// Fields are public so that tests build a configuration with
/// `Config { database_url: url, ..Config::default() }` and no environment.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// `PostgreSQL` connection URL (`DATABASE_URL`, default
    /// `postgres://carddemo:carddemo@localhost:5432/carddemo`). It carries the password.
    pub database_url: String,
    /// `RabbitMQ` connection URL (`CARDDEMO_AMQP_URL`, default
    /// `amqp://carddemo:carddemo@localhost:5672/%2f`). It carries the password.
    pub amqp_url: String,
    /// `CDRD` (`CODATE01`) request queue (`CARDDEMO_QUEUE_REQUEST_DATE`, default
    /// `CARD.DEMO.REQUEST.DATE`).
    pub queue_request_date: String,
    /// `CDRA` (`COACCT01`) request queue (`CARDDEMO_QUEUE_REQUEST_ACCT`, default
    /// `CARD.DEMO.REQUEST.ACCT`).
    pub queue_request_acct: String,
    /// `CP00` (`COPAUA0C`) request queue (`CARDDEMO_QUEUE_REQUEST_PAUTH`, default
    /// `CARD.DEMO.REQUEST.PAUTH`). A renamed queue needs the same dead-letter arguments.
    pub queue_request_pauth: String,
    /// Milliseconds between polls of an empty request queue (`CARDDEMO_MQ_POLL_MS`, default
    /// `250`, from `1` to `5000`).
    pub mq_poll_ms: u64,
    /// Listen address of the API, `host:port` (`CARDDEMO_API_BIND`, default `127.0.0.1:8080`).
    /// Kept as given; the API resolves it when it binds.
    pub api_bind: String,
    /// HS256 signing secret of the API (`CARDDEMO_JWT_SECRET`). No default: empty when unset,
    /// and the API refuses to start then (see [`Config::require_jwt_secret`]).
    pub jwt_secret: String,
    /// Token lifetime in seconds (`CARDDEMO_JWT_TTL_SECONDS`, default `3600`, never zero).
    pub jwt_ttl_seconds: u64,
    /// Root of the local output directories (`CARDDEMO_VAR_DIR`, default `./var`).
    pub var_dir: PathBuf,
    /// Statement output directory of `creastmt` (`CARDDEMO_STATEMENT_DIR`); when unset,
    /// `statements` under the effective [`Config::var_dir`].
    pub statement_dir: PathBuf,
    /// Job-deck directory of the online report request, the counterpart of the `JOBS`
    /// transient data queue (`CARDDEMO_INTRDR_DIR`); when unset, `jobs` under the effective
    /// [`Config::var_dir`].
    pub intrdr_dir: PathBuf,
    /// ASCII fixture directory, read in place by the load subcommands (`CARDDEMO_FIXTURE_DIR`,
    /// default `../app/data/ASCII`).
    pub fixture_dir: PathBuf,
    /// `OpenTelemetry` span export (`CARDDEMO_OTEL_EXPORT`): unset or empty is
    /// [`OtelExport::Off`], exactly `stdout` is [`OtelExport::Stdout`].
    pub otel_export: OtelExport,
}

impl Default for Config {
    /// The local defaults, with an empty JWT secret.
    fn default() -> Self {
        let var_dir = PathBuf::from(DEFAULT_VAR_DIR);
        Self {
            database_url: DEFAULT_DATABASE_URL.to_owned(),
            amqp_url: DEFAULT_AMQP_URL.to_owned(),
            queue_request_date: DEFAULT_QUEUE_REQUEST_DATE.to_owned(),
            queue_request_acct: DEFAULT_QUEUE_REQUEST_ACCT.to_owned(),
            queue_request_pauth: DEFAULT_QUEUE_REQUEST_PAUTH.to_owned(),
            mq_poll_ms: DEFAULT_MQ_POLL_MS,
            api_bind: DEFAULT_API_BIND.to_owned(),
            jwt_secret: String::new(),
            jwt_ttl_seconds: DEFAULT_JWT_TTL_SECONDS,
            statement_dir: var_dir.join(STATEMENTS_SUBDIR),
            intrdr_dir: var_dir.join(JOBS_SUBDIR),
            var_dir,
            fixture_dir: PathBuf::from(DEFAULT_FIXTURE_DIR),
            otel_export: OtelExport::Off,
        }
    }
}

impl fmt::Debug for Config {
    /// Lists every field, but shows each credential only as `<redacted>`, or as `""` when it
    /// is empty, so that an unset JWT secret stays visible without anything being disclosed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &redacted(&self.database_url))
            .field("amqp_url", &redacted(&self.amqp_url))
            .field("queue_request_date", &self.queue_request_date)
            .field("queue_request_acct", &self.queue_request_acct)
            .field("queue_request_pauth", &self.queue_request_pauth)
            .field("mq_poll_ms", &self.mq_poll_ms)
            .field("api_bind", &self.api_bind)
            .field("jwt_secret", &redacted(&self.jwt_secret))
            .field("jwt_ttl_seconds", &self.jwt_ttl_seconds)
            .field("var_dir", &self.var_dir)
            .field("statement_dir", &self.statement_dir)
            .field("intrdr_dir", &self.intrdr_dir)
            .field("fixture_dir", &self.fixture_dir)
            .field("otel_export", &self.otel_export)
            .finish()
    }
}

/// The `Debug` stand-in for a credential.
fn redacted(value: &str) -> &'static str {
    if value.is_empty() { "" } else { REDACTED }
}

impl Config {
    /// Reads the configuration from the process environment.
    ///
    /// This is the only function that touches the environment, and it only reads it. A variable
    /// that is unset or set to the empty string takes its default; every other value is parsed
    /// by [`Config::from_lookup`].
    ///
    /// # Errors
    ///
    /// - [`EnvError::NotUnicode`] when a variable this module reads holds bytes that are not
    ///   valid Unicode.
    /// - [`EnvError::Invalid`] when a value is rejected, as [`Config::from_lookup`] describes.
    pub fn from_env() -> Result<Self, EnvError> {
        let mut values: Vec<(&'static str, String)> = Vec::with_capacity(VARIABLES.len());
        for var in VARIABLES {
            match env::var(var) {
                Ok(value) => values.push((var, value)),
                Err(VarError::NotPresent) => {}
                Err(VarError::NotUnicode(_)) => return Err(EnvError::NotUnicode { var }),
            }
        }
        Self::from_lookup(|name| {
            values
                .iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| value.clone())
        })
    }

    /// Parses the configuration from `lookup`, which returns the value of a variable by name,
    /// or `None` when it is unset. The environment is not consulted.
    ///
    /// An empty value counts as unset. Values are never trimmed, so surrounding spaces make a
    /// number or `CARDDEMO_OTEL_EXPORT` invalid and stay part of a string or path value.
    ///
    /// # Errors
    ///
    /// [`EnvError::Invalid`], naming the variable, when:
    /// - `CARDDEMO_MQ_POLL_MS` or `CARDDEMO_JWT_TTL_SECONDS` holds anything but ASCII decimal
    ///   digits (a sign included), is zero, or exceeds the 64-bit unsigned range;
    /// - `CARDDEMO_MQ_POLL_MS` exceeds `5000`, the COBOL's five-second wait;
    /// - `CARDDEMO_OTEL_EXPORT` is anything other than exactly `stdout`.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Result<Self, EnvError> {
        let get = |var: &str| lookup(var).filter(|value| !value.is_empty());
        let text = |var: &str, default: &str| get(var).unwrap_or_else(|| default.to_owned());

        let var_dir =
            get(CARDDEMO_VAR_DIR).map_or_else(|| PathBuf::from(DEFAULT_VAR_DIR), PathBuf::from);
        let statement_dir = get(CARDDEMO_STATEMENT_DIR)
            .map_or_else(|| var_dir.join(STATEMENTS_SUBDIR), PathBuf::from);
        let intrdr_dir =
            get(CARDDEMO_INTRDR_DIR).map_or_else(|| var_dir.join(JOBS_SUBDIR), PathBuf::from);

        Ok(Self {
            database_url: text(DATABASE_URL, DEFAULT_DATABASE_URL),
            amqp_url: text(CARDDEMO_AMQP_URL, DEFAULT_AMQP_URL),
            queue_request_date: text(CARDDEMO_QUEUE_REQUEST_DATE, DEFAULT_QUEUE_REQUEST_DATE),
            queue_request_acct: text(CARDDEMO_QUEUE_REQUEST_ACCT, DEFAULT_QUEUE_REQUEST_ACCT),
            queue_request_pauth: text(CARDDEMO_QUEUE_REQUEST_PAUTH, DEFAULT_QUEUE_REQUEST_PAUTH),
            mq_poll_ms: positive_integer(
                CARDDEMO_MQ_POLL_MS,
                get(CARDDEMO_MQ_POLL_MS).as_deref(),
                DEFAULT_MQ_POLL_MS,
                MAX_MQ_POLL_MS,
                EXPECTED_MQ_POLL_MS,
            )?,
            api_bind: text(CARDDEMO_API_BIND, DEFAULT_API_BIND),
            jwt_secret: get(CARDDEMO_JWT_SECRET).unwrap_or_default(),
            jwt_ttl_seconds: positive_integer(
                CARDDEMO_JWT_TTL_SECONDS,
                get(CARDDEMO_JWT_TTL_SECONDS).as_deref(),
                DEFAULT_JWT_TTL_SECONDS,
                u64::MAX,
                EXPECTED_POSITIVE_INTEGER,
            )?,
            var_dir,
            statement_dir,
            intrdr_dir,
            fixture_dir: get(CARDDEMO_FIXTURE_DIR)
                .map_or_else(|| PathBuf::from(DEFAULT_FIXTURE_DIR), PathBuf::from),
            otel_export: otel_export(get(CARDDEMO_OTEL_EXPORT).as_deref())?,
        })
    }

    /// The JWT signing secret, which the API requires before it starts.
    ///
    /// # Errors
    ///
    /// [`EnvError::MissingJwtSecret`] when [`Config::jwt_secret`] is empty.
    pub fn require_jwt_secret(&self) -> Result<&str, EnvError> {
        if self.jwt_secret.is_empty() {
            Err(EnvError::MissingJwtSecret)
        } else {
            Ok(&self.jwt_secret)
        }
    }

    /// Pause between polls of an empty request queue.
    #[must_use]
    pub fn mq_poll_interval(&self) -> Duration {
        Duration::from_millis(self.mq_poll_ms)
    }

    /// Directory of the `posttran` reject generations (`DALYREJS`), `dalyrejs` under
    /// [`Config::var_dir`].
    #[must_use]
    pub fn dalyrejs_dir(&self) -> PathBuf {
        self.var_dir.join(DALYREJS_SUBDIR)
    }

    /// Directory of the `intcalc` system-transaction generations (`SYSTRAN`), `systran` under
    /// [`Config::var_dir`].
    #[must_use]
    pub fn systran_dir(&self) -> PathBuf {
        self.var_dir.join(SYSTRAN_SUBDIR)
    }

    /// Directory of the `tranrept` report generations (`TRANREPT`), `reports` under
    /// [`Config::var_dir`].
    #[must_use]
    pub fn reports_dir(&self) -> PathBuf {
        self.var_dir.join(REPORTS_SUBDIR)
    }
}

/// Parses a positive count of ASCII decimal digits no greater than `max`, or returns `default`
/// when `value` is unset. Every rejected value, out of range or malformed, reports `expected`.
fn positive_integer(
    var: &'static str,
    value: Option<&str>,
    default: u64,
    max: u64,
    expected: &'static str,
) -> Result<u64, EnvError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let invalid = EnvError::Invalid { var, expected };
    // `str::parse` alone would also accept a leading `+`.
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid);
    }
    match value.parse::<u64>() {
        Ok(number) if (1..=max).contains(&number) => Ok(number),
        Ok(_) | Err(_) => Err(invalid),
    }
}

/// Parses `CARDDEMO_OTEL_EXPORT`: unset is [`OtelExport::Off`], exactly `stdout` is
/// [`OtelExport::Stdout`], and anything else is rejected rather than guessed.
fn otel_export(value: Option<&str>) -> Result<OtelExport, EnvError> {
    match value {
        None => Ok(OtelExport::Off),
        Some(OTEL_EXPORT_STDOUT) => Ok(OtelExport::Stdout),
        Some(_) => Err(EnvError::Invalid {
            var: CARDDEMO_OTEL_EXPORT,
            expected: EXPECTED_OTEL_EXPORT,
        }),
    }
}

/// Why the configuration cannot be built. Messages name the variable and what was expected,
/// never the value, because values can be credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvError {
    /// The variable holds bytes that are not valid Unicode.
    NotUnicode {
        /// Name of the variable.
        var: &'static str,
    },
    /// The variable's value is rejected.
    Invalid {
        /// Name of the variable.
        var: &'static str,
        /// What the value must be.
        expected: &'static str,
    },
    /// `CARDDEMO_JWT_SECRET` is empty or unset, and the API cannot sign tokens without it.
    MissingJwtSecret,
}

impl fmt::Display for EnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUnicode { var } => {
                write!(f, "environment variable {var} is not valid Unicode")
            }
            Self::Invalid { var, expected } => {
                write!(
                    f,
                    "environment variable {var} is invalid: expected {expected}"
                )
            }
            Self::MissingJwtSecret => write!(
                f,
                "{CARDDEMO_JWT_SECRET} is empty or unset; the API refuses to start without it"
            ),
        }
    }
}

impl Error for EnvError {}

#[cfg(test)]
mod tests {
    use super::{
        CARDDEMO_JWT_SECRET, CARDDEMO_JWT_TTL_SECONDS, CARDDEMO_MQ_POLL_MS, CARDDEMO_OTEL_EXPORT,
        CARDDEMO_VAR_DIR, Config, EnvError, OtelExport, VARIABLES,
    };
    use std::collections::{BTreeSet, HashMap};
    use std::env::{self, VarError};
    use std::error::Error;
    use std::path::PathBuf;
    use std::time::Duration;

    /// The committed `modern/.env.example`.
    const ENV_EXAMPLE: &str = include_str!("../../../.env.example");

    fn parse(pairs: &[(&str, &str)]) -> Result<Config, EnvError> {
        let values: HashMap<&str, &str> = pairs.iter().copied().collect();
        Config::from_lookup(|name| values.get(name).map(|value| (*value).to_owned()))
    }

    fn parse_ok(pairs: &[(&str, &str)]) -> Config {
        parse(pairs).expect("configuration parses")
    }

    #[test]
    fn unset_environment_gives_the_local_defaults() {
        let config = Config::from_lookup(|_| None).expect("defaults parse");
        assert_eq!(config, Config::default());

        assert_eq!(
            config.database_url,
            "postgres://carddemo:carddemo@localhost:5432/carddemo"
        );
        assert_eq!(
            config.amqp_url,
            "amqp://carddemo:carddemo@localhost:5672/%2f"
        );
        assert_eq!(config.queue_request_date, "CARD.DEMO.REQUEST.DATE");
        assert_eq!(config.queue_request_acct, "CARD.DEMO.REQUEST.ACCT");
        assert_eq!(config.queue_request_pauth, "CARD.DEMO.REQUEST.PAUTH");
        assert_eq!(config.mq_poll_ms, 250);
        assert_eq!(config.mq_poll_interval(), Duration::from_millis(250));
        assert_eq!(config.api_bind, "127.0.0.1:8080");
        assert!(config.jwt_secret.is_empty());
        assert_eq!(config.jwt_ttl_seconds, 3600);
        assert_eq!(config.var_dir, PathBuf::from("./var"));
        assert_eq!(
            config.statement_dir,
            PathBuf::from("./var").join("statements")
        );
        assert_eq!(config.intrdr_dir, PathBuf::from("./var").join("jobs"));
        assert_eq!(config.fixture_dir, PathBuf::from("../app/data/ASCII"));
        assert_eq!(config.otel_export, OtelExport::Off);
        assert_eq!(config.dalyrejs_dir(), PathBuf::from("./var/dalyrejs"));
        assert_eq!(config.systran_dir(), PathBuf::from("./var/systran"));
        assert_eq!(config.reports_dir(), PathBuf::from("./var/reports"));
    }

    #[test]
    fn env_example_lists_exactly_these_variables_with_these_defaults() {
        let mut active: HashMap<&str, &str> = HashMap::new();
        let mut names = BTreeSet::new();
        for line in ENV_EXAMPLE.lines() {
            // A commented-out assignment (`# NAME=value`) still documents a variable.
            let (assignment, commented) = match line.strip_prefix("# ") {
                Some(rest) => (rest, true),
                None => (line, false),
            };
            let Some((name, value)) = assignment.split_once('=') else {
                continue;
            };
            let is_name = !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
            if is_name {
                names.insert(name);
                if !commented {
                    active.insert(name, value);
                }
            }
        }

        let mut expected: BTreeSet<&str> = VARIABLES.into_iter().collect();
        expected.extend(["RUST_LOG", "SQLX_OFFLINE"]);
        assert_eq!(names, expected);

        let placeholder = active.get(CARDDEMO_JWT_SECRET).copied().unwrap_or_default();
        assert!(
            !placeholder.is_empty(),
            "the example carries a local-only JWT secret"
        );
        let from_example =
            Config::from_lookup(|name| active.get(name).map(|value| (*value).to_owned()))
                .expect("the example parses");
        assert_eq!(
            from_example,
            Config {
                jwt_secret: placeholder.to_owned(),
                ..Config::default()
            }
        );
        assert_eq!(
            parse_ok(&[(CARDDEMO_OTEL_EXPORT, "stdout")]).otel_export,
            OtelExport::Stdout
        );
    }

    #[test]
    fn every_variable_can_be_overridden() {
        let config = parse_ok(&[
            ("DATABASE_URL", "postgres://u:p@db.local:6543/other"),
            ("CARDDEMO_AMQP_URL", "amqp://u:p@mq.local:5673/vh"),
            ("CARDDEMO_QUEUE_REQUEST_DATE", "Q.DATE"),
            ("CARDDEMO_QUEUE_REQUEST_ACCT", "Q.ACCT"),
            ("CARDDEMO_QUEUE_REQUEST_PAUTH", "Q.PAUTH"),
            ("CARDDEMO_MQ_POLL_MS", "40"),
            ("CARDDEMO_API_BIND", "localhost:9090"),
            ("CARDDEMO_JWT_SECRET", "s3cret"),
            ("CARDDEMO_JWT_TTL_SECONDS", "60"),
            ("CARDDEMO_VAR_DIR", "/srv/var"),
            ("CARDDEMO_STATEMENT_DIR", "/srv/stmt"),
            ("CARDDEMO_INTRDR_DIR", "/srv/jobs"),
            ("CARDDEMO_FIXTURE_DIR", "/srv/fixtures"),
            ("CARDDEMO_OTEL_EXPORT", "stdout"),
        ]);
        assert_eq!(
            config,
            Config {
                database_url: "postgres://u:p@db.local:6543/other".to_owned(),
                amqp_url: "amqp://u:p@mq.local:5673/vh".to_owned(),
                queue_request_date: "Q.DATE".to_owned(),
                queue_request_acct: "Q.ACCT".to_owned(),
                queue_request_pauth: "Q.PAUTH".to_owned(),
                mq_poll_ms: 40,
                api_bind: "localhost:9090".to_owned(),
                jwt_secret: "s3cret".to_owned(),
                jwt_ttl_seconds: 60,
                var_dir: PathBuf::from("/srv/var"),
                statement_dir: PathBuf::from("/srv/stmt"),
                intrdr_dir: PathBuf::from("/srv/jobs"),
                fixture_dir: PathBuf::from("/srv/fixtures"),
                otel_export: OtelExport::Stdout,
            }
        );
        assert_eq!(config.mq_poll_interval(), Duration::from_millis(40));
        assert_eq!(config.require_jwt_secret(), Ok("s3cret"));
    }

    #[test]
    fn empty_values_count_as_unset() {
        let pairs: Vec<(&str, &str)> = VARIABLES.iter().map(|var| (*var, "")).collect();
        assert_eq!(parse_ok(&pairs), Config::default());
    }

    #[test]
    fn derived_directories_follow_var_dir_unless_set() {
        let moved = parse_ok(&[(CARDDEMO_VAR_DIR, "/tmp/x")]);
        assert_eq!(moved.var_dir, PathBuf::from("/tmp/x"));
        assert_eq!(moved.statement_dir, PathBuf::from("/tmp/x/statements"));
        assert_eq!(moved.intrdr_dir, PathBuf::from("/tmp/x/jobs"));
        assert_eq!(moved.dalyrejs_dir(), PathBuf::from("/tmp/x/dalyrejs"));
        assert_eq!(moved.systran_dir(), PathBuf::from("/tmp/x/systran"));
        assert_eq!(moved.reports_dir(), PathBuf::from("/tmp/x/reports"));

        let explicit = parse_ok(&[
            (CARDDEMO_VAR_DIR, "/tmp/x"),
            ("CARDDEMO_STATEMENT_DIR", "/elsewhere/statements"),
            ("CARDDEMO_INTRDR_DIR", "/elsewhere/jobs"),
        ]);
        assert_eq!(
            explicit.statement_dir,
            PathBuf::from("/elsewhere/statements")
        );
        assert_eq!(explicit.intrdr_dir, PathBuf::from("/elsewhere/jobs"));
        assert_eq!(explicit.dalyrejs_dir(), PathBuf::from("/tmp/x/dalyrejs"));
    }

    #[test]
    fn numeric_variables_accept_only_positive_decimal_digits() {
        /// One numeric variable: the text every rejected value reports, the values rejected
        /// beyond the malformed ones, and the values accepted with the number each parses to.
        struct Case {
            var: &'static str,
            expected: &'static str,
            out_of_range: &'static [&'static str],
            accepted: &'static [(&'static str, u64)],
        }

        let malformed = [
            "abc",
            "+250",
            "-1",
            " 250",
            "250 ",
            "2.5",
            "0",
            "000",
            "1234567890123456789012345",
        ];
        let cases = [
            Case {
                var: CARDDEMO_MQ_POLL_MS,
                expected: "a positive integer of decimal digits, at most 5000",
                out_of_range: &["5001", "999999999999", "18446744073709551615"],
                accepted: &[("1", 1), ("250", 250), ("007", 7), ("5000", 5000)],
            },
            Case {
                var: CARDDEMO_JWT_TTL_SECONDS,
                expected: "a positive integer of decimal digits",
                out_of_range: &[],
                accepted: &[
                    ("1", 1),
                    ("250", 250),
                    ("007", 7),
                    ("18446744073709551615", u64::MAX),
                ],
            },
        ];
        for Case {
            var,
            expected,
            out_of_range,
            accepted,
        } in cases
        {
            for rejected in malformed.iter().chain(out_of_range).copied() {
                assert_eq!(
                    parse(&[(var, rejected)]),
                    Err(EnvError::Invalid { var, expected }),
                    "{var}={rejected:?}"
                );
            }
            for &(value, number) in accepted {
                let config = parse_ok(&[(var, value)]);
                let parsed = if var == CARDDEMO_MQ_POLL_MS {
                    config.mq_poll_ms
                } else {
                    config.jwt_ttl_seconds
                };
                assert_eq!(parsed, number, "{var}={value:?}");
            }
        }
        assert_eq!(
            parse_ok(&[(CARDDEMO_MQ_POLL_MS, "5000")]).mq_poll_interval(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn otel_export_is_off_or_exactly_stdout() {
        assert_eq!(parse_ok(&[]).otel_export, OtelExport::Off);
        assert_eq!(
            parse_ok(&[(CARDDEMO_OTEL_EXPORT, "")]).otel_export,
            OtelExport::Off
        );
        assert_eq!(
            parse_ok(&[(CARDDEMO_OTEL_EXPORT, "stdout")]).otel_export,
            OtelExport::Stdout
        );
        for rejected in ["STDOUT", "otlp", " stdout", "off"] {
            assert_eq!(
                parse(&[(CARDDEMO_OTEL_EXPORT, rejected)]),
                Err(EnvError::Invalid {
                    var: CARDDEMO_OTEL_EXPORT,
                    expected: "unset, empty or stdout"
                })
            );
        }
    }

    #[test]
    fn jwt_secret_is_required_only_when_asked_for() {
        assert_eq!(
            Config::default().require_jwt_secret(),
            Err(EnvError::MissingJwtSecret)
        );
        let config = parse_ok(&[(CARDDEMO_JWT_SECRET, "local-secret")]);
        assert_eq!(config.require_jwt_secret(), Ok("local-secret"));
    }

    #[test]
    fn debug_output_never_shows_credentials() {
        let config = parse_ok(&[
            (
                "DATABASE_URL",
                "postgres://carddemo:carddemo@localhost:5432/carddemo",
            ),
            (
                "CARDDEMO_AMQP_URL",
                "amqp://mquser:amqp-pass-42@localhost:5672/%2f",
            ),
            (CARDDEMO_JWT_SECRET, "jwt-secret-99"),
        ]);
        let shown = format!("{config:?}");
        assert!(shown.contains("<redacted>"), "{shown}");
        for secret in [
            "carddemo:carddemo",
            "amqp-pass-42",
            "mquser",
            "jwt-secret-99",
        ] {
            assert!(!shown.contains(secret), "{secret} leaked in {shown}");
        }
        assert!(
            shown.contains("queue_request_pauth: \"CARD.DEMO.REQUEST.PAUTH\""),
            "{shown}"
        );
        assert!(shown.contains("otel_export: Off"), "{shown}");

        let unset_secret = format!("{:?}", Config::default());
        assert!(unset_secret.contains("jwt_secret: \"\""), "{unset_secret}");
        assert!(
            unset_secret.contains("database_url: \"<redacted>\""),
            "{unset_secret}"
        );
    }

    #[test]
    fn error_messages_name_the_variable_and_never_the_value() {
        let error = parse(&[(CARDDEMO_MQ_POLL_MS, "p4ssw0rd")]).expect_err("invalid value");
        let message = error.to_string();
        assert_eq!(
            message,
            "environment variable CARDDEMO_MQ_POLL_MS is invalid: \
             expected a positive integer of decimal digits, at most 5000"
        );
        assert!(!message.contains("p4ssw0rd"));

        let above_bound =
            parse(&[(CARDDEMO_MQ_POLL_MS, "999999999999")]).expect_err("above the bound");
        assert_eq!(
            above_bound,
            EnvError::Invalid {
                var: CARDDEMO_MQ_POLL_MS,
                expected: "a positive integer of decimal digits, at most 5000"
            }
        );
        assert_eq!(above_bound.to_string(), message);
        assert!(!above_bound.to_string().contains("999999999999"));

        let not_unicode = EnvError::NotUnicode {
            var: "DATABASE_URL",
        };
        assert_eq!(
            not_unicode.to_string(),
            "environment variable DATABASE_URL is not valid Unicode"
        );
        assert_eq!(
            EnvError::MissingJwtSecret.to_string(),
            "CARDDEMO_JWT_SECRET is empty or unset; the API refuses to start without it"
        );
        for variant in [not_unicode, error, EnvError::MissingJwtSecret] {
            assert!(!variant.to_string().is_empty());
            assert!(variant.source().is_none());
        }
    }

    #[test]
    fn from_env_agrees_with_from_lookup_over_the_same_environment() {
        let not_unicode = VARIABLES
            .into_iter()
            .find(|var| matches!(env::var(var), Err(VarError::NotUnicode(_))));
        let expected = match not_unicode {
            Some(var) => Err(EnvError::NotUnicode { var }),
            None => Config::from_lookup(|name| env::var(name).ok()),
        };
        assert_eq!(Config::from_env(), expected);
    }
}
