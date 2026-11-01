//! Typed process settings: the one place a `CARDDEMO_*` variable is read and
//! given its default.
//!
//! The `api`, `batch` and `mq` binaries call [`Settings::from_env`] once at
//! start. A variable that is not set keeps its value from [`defaults`].
//!
//! Configuration flows one way. The repository's `.env.example` lists every
//! setting with its local default, and [`defaults`] carries the same values, so
//! a `.env` is needed only to change one. The unit tests read `.env.example`
//! and fail when a key or a value drifts between the two.
//!
//! # Variables that are not fields
//!
//! - `CARDDEMO_API_PROXY_TARGET` is read only by `web/vite.config.ts`, as the
//!   upstream of the dev server's `/api` proxy.
//! - `DATABASE_URL` is read only at compile time, by the sqlx `query!` and
//!   `query_as!` macros and by `cargo sqlx prepare`. The binaries connect with
//!   `CARDDEMO_DATABASE_URL`.
//!
//! # Queue names
//!
//! Only the request queues are settings. The COBOL consumers take their input
//! queue from the trigger message (`MQTM-QNAME`,
//! `app/app-vsam-mq/cbl/CODATE01.cbl:146`), so it is deployment configuration.
//! The reply and error queues are COBOL literals, owned by `crates/messaging`
//! and not configurable: `CARD.DEMO.REPLY.DATE` (`CODATE01.cbl:147`),
//! `CARD.DEMO.REPLY.ACCT` (`app/app-vsam-mq/cbl/COACCT01.cbl:198`) and
//! `CARD.DEMO.ERROR` (`CODATE01.cbl:243`, `COACCT01.cbl:294`). CP00 replies to
//! each request's `reply_to`.

use std::env::VarError;
use std::fmt;
use std::net::{AddrParseError, SocketAddr};
use std::num::ParseIntError;
use std::path::PathBuf;

/// Names of the environment variables [`Settings`] reads.
pub mod keys {
    /// Database connection URL the `api`, `mq` and `batch` binaries open.
    pub const DATABASE_URL: &str = "CARDDEMO_DATABASE_URL";
    /// AMQP URL of the broker the `mq` consumers connect to.
    pub const AMQP_URL: &str = "CARDDEMO_AMQP_URL";
    /// Socket address (IP literal and port) the `api` binary listens on.
    pub const HTTP_ADDR: &str = "CARDDEMO_HTTP_ADDR";
    /// HS256 secret that signs and verifies the tokens `POST /auth/login` issues.
    pub const JWT_SECRET: &str = "CARDDEMO_JWT_SECRET";
    /// Lifetime of an issued token, in whole seconds.
    pub const JWT_TTL_SECS: &str = "CARDDEMO_JWT_TTL_SECS";
    /// Request queue of the CDRD date flow (`CODATE01`).
    pub const QUEUE_DATE_REQUEST: &str = "CARDDEMO_QUEUE_DATE_REQUEST";
    /// Request queue of the CDRA account flow (`COACCT01`).
    pub const QUEUE_ACCT_REQUEST: &str = "CARDDEMO_QUEUE_ACCT_REQUEST";
    /// Request queue of the CP00 authorization flow (`COPAUA0C`).
    pub const QUEUE_PAUTH_REQUEST: &str = "CARDDEMO_QUEUE_PAUTH_REQUEST";
    /// Directory `batch creastmt` writes the statement files to.
    pub const STATEMENT_DIR: &str = "CARDDEMO_STATEMENT_DIR";
    /// Directory of the GDG generation files the batch subcommands write and read.
    pub const BATCH_DIR: &str = "CARDDEMO_BATCH_DIR";
    /// ASCII fixture directory `batch seed` decodes to load the database.
    pub const FIXTURE_DIR: &str = "CARDDEMO_FIXTURE_DIR";
    /// Browser origin the `api` admits through CORS.
    pub const CORS_ORIGIN: &str = "CARDDEMO_CORS_ORIGIN";
    /// Optional RFC 3339 instant that replaces the system clock in every
    /// time-dependent output.
    pub const CLOCK_OVERRIDE: &str = "CARDDEMO_CLOCK_OVERRIDE";
    /// Filter directives of the JSON logs, under the conventional unprefixed name.
    pub const RUST_LOG: &str = "RUST_LOG";

    /// Every variable above, in declaration order.
    pub const ALL: [&str; 14] = [
        DATABASE_URL,
        AMQP_URL,
        HTTP_ADDR,
        JWT_SECRET,
        JWT_TTL_SECS,
        QUEUE_DATE_REQUEST,
        QUEUE_ACCT_REQUEST,
        QUEUE_PAUTH_REQUEST,
        STATEMENT_DIR,
        BATCH_DIR,
        FIXTURE_DIR,
        CORS_ORIGIN,
        CLOCK_OVERRIDE,
        RUST_LOG,
    ];
}

/// Local defaults, equal to the values `.env.example` sets.
///
/// `CARDDEMO_CLOCK_OVERRIDE` has no default: unset means the system clock.
pub mod defaults {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    /// Default of `CARDDEMO_DATABASE_URL`: the Compose `postgres` service.
    pub const DATABASE_URL: &str = "postgres://carddemo:carddemo@localhost:5432/carddemo";
    /// Default of `CARDDEMO_AMQP_URL`: the Compose `rabbitmq` service, vhost `/`.
    pub const AMQP_URL: &str = "amqp://carddemo:carddemo@localhost:5672/%2f";
    /// Default of `CARDDEMO_HTTP_ADDR`: `127.0.0.1:8080`.
    pub const HTTP_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
    /// Default of `CARDDEMO_JWT_SECRET`, fit for a development machine only.
    pub const JWT_SECRET: &str = "local-dev-secret-change-me";
    /// Default of `CARDDEMO_JWT_TTL_SECS`: one hour.
    pub const JWT_TTL_SECS: u64 = 3600;
    /// Default of `CARDDEMO_QUEUE_DATE_REQUEST`.
    pub const QUEUE_DATE_REQUEST: &str = "CARD.DEMO.REQUEST.DATE";
    /// Default of `CARDDEMO_QUEUE_ACCT_REQUEST`.
    pub const QUEUE_ACCT_REQUEST: &str = "CARD.DEMO.REQUEST.ACCT";
    /// Default of `CARDDEMO_QUEUE_PAUTH_REQUEST`.
    pub const QUEUE_PAUTH_REQUEST: &str = "CARD.DEMO.REQUEST.PAUTH";
    /// Default of `CARDDEMO_STATEMENT_DIR`, relative to the working directory.
    pub const STATEMENT_DIR: &str = "./var/statements/";
    /// Default of `CARDDEMO_BATCH_DIR`, relative to the working directory.
    pub const BATCH_DIR: &str = "./var/batch/";
    /// Default of `CARDDEMO_FIXTURE_DIR`, relative to the repository root.
    pub const FIXTURE_DIR: &str = "app/data/ASCII";
    /// Default of `CARDDEMO_CORS_ORIGIN`: the Vite dev server.
    pub const CORS_ORIGIN: &str = "http://localhost:5173";
    /// Default of `RUST_LOG`.
    pub const RUST_LOG: &str = "info";
}

/// Settings of one process, resolved from the environment over [`defaults`].
#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    /// `CARDDEMO_DATABASE_URL`: database connection URL. `Debug` never prints
    /// it.
    pub database_url: String,
    /// `CARDDEMO_AMQP_URL`: broker connection URL. `Debug` never prints it.
    pub amqp_url: String,
    /// `CARDDEMO_HTTP_ADDR`: listen address of the `api` binary.
    pub http_addr: SocketAddr,
    /// `CARDDEMO_JWT_SECRET`: token signing secret. `Debug` never prints it.
    pub jwt_secret: String,
    /// `CARDDEMO_JWT_TTL_SECS`: token lifetime in seconds.
    pub jwt_ttl_secs: u64,
    /// `CARDDEMO_QUEUE_DATE_REQUEST`: CDRD request queue.
    pub queue_date_request: String,
    /// `CARDDEMO_QUEUE_ACCT_REQUEST`: CDRA request queue.
    pub queue_acct_request: String,
    /// `CARDDEMO_QUEUE_PAUTH_REQUEST`: CP00 request queue.
    pub queue_pauth_request: String,
    /// `CARDDEMO_STATEMENT_DIR`: statement output directory.
    pub statement_dir: PathBuf,
    /// `CARDDEMO_BATCH_DIR`: generation file directory.
    pub batch_dir: PathBuf,
    /// `CARDDEMO_FIXTURE_DIR`: ASCII fixture directory.
    pub fixture_dir: PathBuf,
    /// `CARDDEMO_CORS_ORIGIN`: browser origin admitted through CORS.
    pub cors_origin: String,
    /// `CARDDEMO_CLOCK_OVERRIDE`: the raw RFC 3339 text, `None` when unset or
    /// empty.
    ///
    /// This crate never parses it. Consumers turn it into a clock with the
    /// domain clock, `carddemo_domain::clock::AnyClock::from_override`.
    pub clock_override: Option<String>,
    /// `RUST_LOG`: log filter directives.
    pub rust_log: String,
}

/// Text `Debug` prints in place of every value that may carry a credential:
/// the database URL, the broker URL and the token signing secret.
const REDACTED: &str = "<redacted>";

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Exhaustive destructuring: a field added to `Settings` fails to compile
        // here until it is listed below.
        let Self {
            database_url: _,
            amqp_url: _,
            http_addr,
            jwt_secret: _,
            jwt_ttl_secs,
            queue_date_request,
            queue_acct_request,
            queue_pauth_request,
            statement_dir,
            batch_dir,
            fixture_dir,
            cors_origin,
            clock_override,
            rust_log,
        } = self;
        // The URLs are redacted whole: a password can sit in the user-info part
        // or, for PostgreSQL, in a `password=` query parameter.
        f.debug_struct("Settings")
            .field("database_url", &REDACTED)
            .field("amqp_url", &REDACTED)
            .field("http_addr", http_addr)
            .field("jwt_secret", &REDACTED)
            .field("jwt_ttl_secs", jwt_ttl_secs)
            .field("queue_date_request", queue_date_request)
            .field("queue_acct_request", queue_acct_request)
            .field("queue_pauth_request", queue_pauth_request)
            .field("statement_dir", statement_dir)
            .field("batch_dir", batch_dir)
            .field("fixture_dir", fixture_dir)
            .field("cors_origin", cors_origin)
            .field("clock_override", clock_override)
            .field("rust_log", rust_log)
            .finish()
    }
}

impl Default for Settings {
    /// Every field at its [`defaults`] value, with no clock override.
    fn default() -> Self {
        Self {
            database_url: defaults::DATABASE_URL.to_owned(),
            amqp_url: defaults::AMQP_URL.to_owned(),
            http_addr: defaults::HTTP_ADDR,
            jwt_secret: defaults::JWT_SECRET.to_owned(),
            jwt_ttl_secs: defaults::JWT_TTL_SECS,
            queue_date_request: defaults::QUEUE_DATE_REQUEST.to_owned(),
            queue_acct_request: defaults::QUEUE_ACCT_REQUEST.to_owned(),
            queue_pauth_request: defaults::QUEUE_PAUTH_REQUEST.to_owned(),
            statement_dir: PathBuf::from(defaults::STATEMENT_DIR),
            batch_dir: PathBuf::from(defaults::BATCH_DIR),
            fixture_dir: PathBuf::from(defaults::FIXTURE_DIR),
            cors_origin: defaults::CORS_ORIGIN.to_owned(),
            clock_override: None,
            rust_log: defaults::RUST_LOG.to_owned(),
        }
    }
}

impl Settings {
    /// Resolves the settings from `lookup`, which returns the value of a
    /// variable or `None` when it is not set.
    ///
    /// A value of `None` keeps the field's default. `CARDDEMO_DATABASE_URL`,
    /// `CARDDEMO_AMQP_URL` and `CARDDEMO_JWT_SECRET` reject a blank value
    /// (empty or only whitespace) and take any other value as given. The other
    /// text and path fields take a value as given; an empty
    /// `CARDDEMO_CLOCK_OVERRIDE` counts as unset.
    ///
    /// ```
    /// use carddemo_runtime::config::{Settings, keys};
    ///
    /// let settings =
    ///     Settings::from_lookup(|key| (key == keys::JWT_TTL_SECS).then(|| "60".to_owned()))?;
    /// assert_eq!(settings.jwt_ttl_secs, 60);
    /// # Ok::<(), carddemo_runtime::config::ConfigError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// - [`ConfigError::Blank`] when `CARDDEMO_DATABASE_URL`,
    ///   `CARDDEMO_AMQP_URL` or `CARDDEMO_JWT_SECRET` is set to an empty or
    ///   whitespace-only value.
    /// - [`ConfigError::InvalidNumber`] when `CARDDEMO_JWT_TTL_SECS` is not an
    ///   unsigned integer.
    /// - [`ConfigError::InvalidAddress`] when `CARDDEMO_HTTP_ADDR` is not an IP
    ///   socket address such as `0.0.0.0:8080`.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(lookup: F) -> Result<Self, ConfigError> {
        Self::resolve(move |key| Ok(lookup(key)))
    }

    /// Resolves the settings from the process environment, with the rules of
    /// [`Settings::from_lookup`].
    ///
    /// Tests call [`Settings::from_lookup`] instead: changing the process
    /// environment (`std::env::set_var`) needs an operation the workspace lint
    /// policy forbids.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::NotUnicode`] when a variable is set to a value that is
    ///   not valid Unicode.
    /// - [`ConfigError::Blank`] when `CARDDEMO_DATABASE_URL`,
    ///   `CARDDEMO_AMQP_URL` or `CARDDEMO_JWT_SECRET` is set to an empty or
    ///   whitespace-only value.
    /// - [`ConfigError::InvalidNumber`] when `CARDDEMO_JWT_TTL_SECS` is not an
    ///   unsigned integer.
    /// - [`ConfigError::InvalidAddress`] when `CARDDEMO_HTTP_ADDR` is not an IP
    ///   socket address.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::resolve(|key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(VarError::NotUnicode(_)) => Err(ConfigError::NotUnicode { key }),
        })
    }

    /// Applies every variable `lookup` finds over [`Settings::default`]. The
    /// parse rules live here only, shared by both public constructors.
    fn resolve(
        lookup: impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
    ) -> Result<Self, ConfigError> {
        let mut settings = Self::default();
        if let Some(value) = lookup_non_blank(&lookup, keys::DATABASE_URL)? {
            settings.database_url = value;
        }
        if let Some(value) = lookup_non_blank(&lookup, keys::AMQP_URL)? {
            settings.amqp_url = value;
        }
        if let Some(value) = lookup(keys::HTTP_ADDR)? {
            settings.http_addr = value
                .parse()
                .map_err(|source| ConfigError::InvalidAddress {
                    key: keys::HTTP_ADDR,
                    value,
                    source,
                })?;
        }
        if let Some(value) = lookup_non_blank(&lookup, keys::JWT_SECRET)? {
            settings.jwt_secret = value;
        }
        if let Some(value) = lookup(keys::JWT_TTL_SECS)? {
            settings.jwt_ttl_secs = value.parse().map_err(|source| ConfigError::InvalidNumber {
                key: keys::JWT_TTL_SECS,
                value,
                source,
            })?;
        }
        if let Some(value) = lookup(keys::QUEUE_DATE_REQUEST)? {
            settings.queue_date_request = value;
        }
        if let Some(value) = lookup(keys::QUEUE_ACCT_REQUEST)? {
            settings.queue_acct_request = value;
        }
        if let Some(value) = lookup(keys::QUEUE_PAUTH_REQUEST)? {
            settings.queue_pauth_request = value;
        }
        if let Some(value) = lookup(keys::STATEMENT_DIR)? {
            settings.statement_dir = PathBuf::from(value);
        }
        if let Some(value) = lookup(keys::BATCH_DIR)? {
            settings.batch_dir = PathBuf::from(value);
        }
        if let Some(value) = lookup(keys::FIXTURE_DIR)? {
            settings.fixture_dir = PathBuf::from(value);
        }
        if let Some(value) = lookup(keys::CORS_ORIGIN)? {
            settings.cors_origin = value;
        }
        settings.clock_override = lookup(keys::CLOCK_OVERRIDE)?.filter(|value| !value.is_empty());
        if let Some(value) = lookup(keys::RUST_LOG)? {
            settings.rust_log = value;
        }
        Ok(settings)
    }
}

/// Looks up `key`, a setting that must not be blank once it is set: a
/// connection URL or the token signing secret.
///
/// An unset variable gives `None`, so its field keeps the default. Any other
/// non-blank value is returned as given.
///
/// # Errors
///
/// - [`ConfigError::Blank`] when the value is empty or only whitespace. The
///   error names `key` alone, never the value.
/// - Any error `lookup` returns for `key`.
fn lookup_non_blank(
    lookup: &impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
    key: &'static str,
) -> Result<Option<String>, ConfigError> {
    match lookup(key)? {
        Some(value) if value.trim().is_empty() => Err(ConfigError::Blank { key }),
        value => Ok(value),
    }
}

/// A setting that is present but unusable.
///
/// [`ConfigError::InvalidNumber`] and [`ConfigError::InvalidAddress`] name the
/// variable and echo the rejected value. [`ConfigError::Blank`] and
/// [`ConfigError::NotUnicode`] name only the variable. The database URL, the
/// broker URL and `CARDDEMO_JWT_SECRET` are never parsed, only checked for
/// blankness, so no error can carry a credential.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A numeric setting is not an unsigned integer.
    #[error("invalid value for {key}: {value:?}")]
    InvalidNumber {
        /// Variable name.
        key: &'static str,
        /// Rejected value.
        value: String,
        /// Integer parse failure.
        #[source]
        source: ParseIntError,
    },
    /// An address setting is not an IP socket address.
    #[error("invalid value for {key}: {value:?}")]
    InvalidAddress {
        /// Variable name.
        key: &'static str,
        /// Rejected value.
        value: String,
        /// Address parse failure.
        #[source]
        source: AddrParseError,
    },
    /// `CARDDEMO_DATABASE_URL`, `CARDDEMO_AMQP_URL` or `CARDDEMO_JWT_SECRET`
    /// is set to an empty or whitespace-only value.
    #[error("{key} is set but blank")]
    Blank {
        /// Variable name.
        key: &'static str,
    },
    /// A variable is set to a value that is not valid Unicode.
    #[error("{key} is not valid Unicode")]
    NotUnicode {
        /// Variable name.
        key: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::error::Error;
    use std::ffi::OsStr;
    use std::net::SocketAddr;
    use std::path::PathBuf;

    use super::{ConfigError, Settings, defaults, keys};

    /// The repository root's `.env.example`; `CARGO_MANIFEST_DIR` is `crates/runtime`.
    const ENV_EXAMPLE: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env.example"));

    /// Set in `.env.example` for `web/vite.config.ts`, never read by `Settings`.
    const API_PROXY_TARGET: &str = "CARDDEMO_API_PROXY_TARGET";

    /// The active `KEY=value` lines of `.env.example`.
    fn env_example() -> BTreeMap<&'static str, &'static str> {
        let mut entries = BTreeMap::new();
        for line in ENV_EXAMPLE.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                panic!(".env.example line is not KEY=value: {line:?}");
            };
            let key = key.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'))
                .unwrap_or(value);
            assert!(
                entries.insert(key, value).is_none(),
                ".env.example sets {key} more than once"
            );
        }
        entries
    }

    /// A lookup over fixed `(key, value)` pairs.
    fn lookup_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_equal_env_example() {
        let env = env_example();
        let settings = Settings::default();
        let http_addr = settings.http_addr.to_string();
        let jwt_ttl_secs = settings.jwt_ttl_secs.to_string();
        let text = [
            (keys::DATABASE_URL, settings.database_url.as_str()),
            (keys::AMQP_URL, settings.amqp_url.as_str()),
            (keys::HTTP_ADDR, http_addr.as_str()),
            (keys::JWT_SECRET, settings.jwt_secret.as_str()),
            (keys::JWT_TTL_SECS, jwt_ttl_secs.as_str()),
            (
                keys::QUEUE_DATE_REQUEST,
                settings.queue_date_request.as_str(),
            ),
            (
                keys::QUEUE_ACCT_REQUEST,
                settings.queue_acct_request.as_str(),
            ),
            (
                keys::QUEUE_PAUTH_REQUEST,
                settings.queue_pauth_request.as_str(),
            ),
            (keys::CORS_ORIGIN, settings.cors_origin.as_str()),
            (keys::RUST_LOG, settings.rust_log.as_str()),
        ];
        // Byte comparison: `Path` equality would ignore the trailing `/` of
        // `./var/statements/`.
        let paths = [
            (keys::STATEMENT_DIR, &settings.statement_dir),
            (keys::BATCH_DIR, &settings.batch_dir),
            (keys::FIXTURE_DIR, &settings.fixture_dir),
        ];
        for (key, default) in text {
            assert_eq!(
                env.get(key).copied(),
                Some(default),
                "{key}: the default differs from .env.example"
            );
        }
        for (key, default) in paths {
            let Some(value) = env.get(key) else {
                panic!("{key}: not set in .env.example");
            };
            assert_eq!(
                default.as_os_str(),
                OsStr::new(value),
                "{key}: the default differs from .env.example"
            );
        }

        let compared: BTreeSet<&str> = text
            .iter()
            .map(|(key, _)| *key)
            .chain(paths.iter().map(|(key, _)| *key))
            .collect();
        let expected: BTreeSet<&str> = keys::ALL
            .into_iter()
            .filter(|key| *key != keys::CLOCK_OVERRIDE)
            .collect();
        assert_eq!(compared, expected, "every defaulted key is compared");
    }

    #[test]
    fn clock_override_is_documented_but_unset() {
        assert!(!env_example().contains_key(keys::CLOCK_OVERRIDE));
        assert!(
            ENV_EXAMPLE
                .lines()
                .any(|line| line.trim_start().starts_with("# CARDDEMO_CLOCK_OVERRIDE=")),
            ".env.example documents CARDDEMO_CLOCK_OVERRIDE as a commented-out line"
        );
        assert!(Settings::default().clock_override.is_none());
    }

    #[test]
    fn keys_match_env_example_in_both_directions() {
        let env = env_example();
        let unique: BTreeSet<&str> = keys::ALL.into_iter().collect();
        assert_eq!(unique.len(), keys::ALL.len(), "keys::ALL has no duplicates");

        for key in env
            .keys()
            .filter(|key| key.starts_with("CARDDEMO_") && **key != API_PROXY_TARGET)
        {
            assert!(
                keys::ALL.contains(key),
                "{key} is set in .env.example but missing from keys::ALL"
            );
        }
        for key in keys::ALL
            .into_iter()
            .filter(|key| *key != keys::CLOCK_OVERRIDE)
        {
            assert!(
                env.contains_key(key),
                "{key} is in keys::ALL but not set in .env.example"
            );
        }
    }

    #[test]
    fn sqlx_database_url_equals_the_runtime_default() {
        assert_eq!(
            env_example().get("DATABASE_URL").copied(),
            Some(defaults::DATABASE_URL)
        );
    }

    #[test]
    fn empty_lookup_yields_defaults() -> Result<(), ConfigError> {
        assert_eq!(Settings::from_lookup(|_| None)?, Settings::default());
        Ok(())
    }

    #[test]
    fn every_key_overrides_its_own_field() -> Result<(), Box<dyn Error>> {
        let settings = Settings::from_lookup(lookup_from(&[
            (keys::DATABASE_URL, "postgres://u:p@db:5432/x"),
            (keys::AMQP_URL, "amqp://u:p@mq:5672/%2f"),
            (keys::HTTP_ADDR, "0.0.0.0:8080"),
            (keys::JWT_SECRET, "jwt-test-value"),
            (keys::JWT_TTL_SECS, "60"),
            (keys::QUEUE_DATE_REQUEST, "D"),
            (keys::QUEUE_ACCT_REQUEST, "A"),
            (keys::QUEUE_PAUTH_REQUEST, "X"),
            (keys::STATEMENT_DIR, "/tmp/s/"),
            (keys::BATCH_DIR, "/tmp/b/"),
            (keys::FIXTURE_DIR, "/tmp/f"),
            (keys::CORS_ORIGIN, "http://localhost:20084"),
            (keys::CLOCK_OVERRIDE, "2022-07-01T12:00:00Z"),
            (keys::RUST_LOG, "debug"),
        ]))?;
        let expected = Settings {
            database_url: "postgres://u:p@db:5432/x".to_owned(),
            amqp_url: "amqp://u:p@mq:5672/%2f".to_owned(),
            http_addr: "0.0.0.0:8080".parse::<SocketAddr>()?,
            jwt_secret: "jwt-test-value".to_owned(),
            jwt_ttl_secs: 60,
            queue_date_request: "D".to_owned(),
            queue_acct_request: "A".to_owned(),
            queue_pauth_request: "X".to_owned(),
            statement_dir: PathBuf::from("/tmp/s/"),
            batch_dir: PathBuf::from("/tmp/b/"),
            fixture_dir: PathBuf::from("/tmp/f"),
            cors_origin: "http://localhost:20084".to_owned(),
            clock_override: Some("2022-07-01T12:00:00Z".to_owned()),
            rust_log: "debug".to_owned(),
        };
        assert_eq!(settings, expected);
        assert_eq!(settings.statement_dir.as_os_str(), OsStr::new("/tmp/s/"));
        Ok(())
    }

    #[test]
    fn non_numeric_ttl_is_rejected_with_its_key_and_value() {
        let Err(err) = Settings::from_lookup(lookup_from(&[(keys::JWT_TTL_SECS, "abc")])) else {
            panic!("\"abc\" was accepted as a token lifetime");
        };
        assert!(
            matches!(
                &err,
                ConfigError::InvalidNumber { key, value, .. }
                    if *key == keys::JWT_TTL_SECS && value == "abc"
            ),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "invalid value for CARDDEMO_JWT_TTL_SECS: \"abc\""
        );
        assert!(err.source().is_some(), "the parse failure is the source");
    }

    #[test]
    fn unparsable_address_is_rejected_with_its_key_and_value() {
        let result = Settings::from_lookup(lookup_from(&[(keys::HTTP_ADDR, "nope")]));
        assert!(
            matches!(
                &result,
                Err(ConfigError::InvalidAddress { key, value, .. })
                    if *key == keys::HTTP_ADDR && value == "nope"
            ),
            "{result:?}"
        );
    }

    #[test]
    fn blank_connection_urls_and_jwt_secret_are_rejected_naming_only_the_key() {
        for key in [keys::DATABASE_URL, keys::AMQP_URL, keys::JWT_SECRET] {
            for blank in ["", "   ", "\t"] {
                let Err(err) = Settings::from_lookup(lookup_from(&[(key, blank)])) else {
                    panic!("{key}={blank:?} was accepted");
                };
                assert!(
                    matches!(&err, ConfigError::Blank { key: rejected } if *rejected == key),
                    "{key}={blank:?}: {err:?}"
                );
                let message = err.to_string();
                assert_eq!(message, format!("{key} is set but blank"));
                // Neither the quoted value (which also covers "") nor the raw
                // value appears in the message or the error's `Debug`.
                let quoted = format!("{blank:?}");
                let debug = format!("{err:?}");
                assert!(!message.contains(&quoted), "{key}: {message}");
                assert!(!debug.contains(&quoted), "{key}: {debug}");
                assert!(
                    blank.is_empty() || !message.contains(blank),
                    "{key}: {message}"
                );
            }
        }
    }

    #[test]
    fn empty_clock_override_counts_as_unset() -> Result<(), ConfigError> {
        let settings = Settings::from_lookup(lookup_from(&[(keys::CLOCK_OVERRIDE, "")]))?;
        assert_eq!(settings.clock_override, None);
        Ok(())
    }

    #[test]
    fn from_env_reads_the_process_environment() -> Result<(), ConfigError> {
        let expected = Settings::from_lookup(|key| std::env::var(key).ok())?;
        assert_eq!(Settings::from_env()?, expected);
        Ok(())
    }

    #[test]
    fn debug_redacts_the_connection_urls_and_the_jwt_secret() {
        let settings = Settings {
            database_url: "postgres://u:db-pass-marker@db:5432/x".to_owned(),
            amqp_url: "amqp://u:mq-pass-marker@mq:5672/%2f".to_owned(),
            jwt_secret: "jwt-test-value".to_owned(),
            ..Settings::default()
        };
        let shown = format!("{settings:?}");
        for marker in ["db-pass-marker", "mq-pass-marker", "jwt-test-value"] {
            assert!(!shown.contains(marker), "{marker} is shown: {shown}");
        }
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains(defaults::QUEUE_PAUTH_REQUEST), "{shown}");
    }
}
