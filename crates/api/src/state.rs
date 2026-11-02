//! [`AppState`], the one value every handler receives as `State<AppState>`.
//!
//! A CICS program gets its context in two ways. Per task, it receives the
//! `CARDDEMO-COMMAREA` [app/cpy/COCOM01Y.cpy:L19-44]: the signed-on user
//! (`CDEMO-USER-ID`, `CDEMO-USER-TYPE` [app/cpy/COCOM01Y.cpy:L25-28]), the
//! navigation fields (`CDEMO-FROM-TRANID` … `CDEMO-LAST-MAPSET`) and the keys
//! of the customer, account and card being worked on. Per region, it reaches
//! the files, the clock and the region's definitions through CICS.
//!
//! The HTTP adapter keeps the two apart in the same way:
//!
//! - Nothing per user or per request lives here. The signed-on user comes
//!   only from the verified bearer token (`crate::auth::jwt`), the resource
//!   keys come from the path, query or body, and the navigation fields are the
//!   SPA's routes.
//! - Everything per process does: the database handle, the settings, the
//!   processing clock, the token keys and the CORS origin. All of it is built
//!   once at start-up by [`AppState::new`] and is immutable afterwards, so a
//!   clone is cheap and shared across the runtime's threads without locking.
//!
//! # Time
//!
//! [`AppState::clock`] is the processing clock of every time-dependent API
//! output: the CB00 payment timestamps, the CPVD fraud-report date, the CR00
//! current date, the instant of a CT02 reward accrual and the reward-rule
//! soft-delete instant. A CT02 transaction's own timestamps are not on the
//! list: COTRN02C reads no clock and stores the entered dates
//! (P-COTRN02C-030). `CARDDEMO_CLOCK_OVERRIDE` pins the clock to one RFC 3339
//! instant; unset, it is the system clock.
//!
//! Token `iat` and `exp` never read it: `crate::auth::jwt` stamps them from
//! system time, because a pinned instant in the past would issue tokens that
//! are already expired.

use std::fmt;
use std::sync::Arc;

use axum::http::HeaderValue;
use axum::http::header::InvalidHeaderValue;
use carddemo_domain::clock::{AnyClock, ClockError};
use carddemo_persistence::Db;
use carddemo_runtime::config::Settings;

use crate::auth::jwt::JwtKeys;

/// The shared state of the API process.
///
/// Cloning copies a pool handle, three reference counts and the origin
/// header; every clone sees the same connections, settings, clock and keys.
#[derive(Clone)]
pub struct AppState {
    db: Db,
    settings: Arc<Settings>,
    clock: Arc<AnyClock>,
    jwt: Arc<JwtKeys>,
    cors_origin: HeaderValue,
}

impl fmt::Debug for AppState {
    /// Shows every field through its own `Debug`, each of which keeps its
    /// secrets out: [`Settings`] redacts the database and broker URLs and the
    /// JWT secret, [`JwtKeys`] prints only its algorithm and lifetime, and
    /// [`Db`] only its pool's size and options.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Exhaustive destructuring: a field added to `AppState` fails to
        // compile here until it is decided whether `Debug` may show it.
        let Self {
            db,
            settings,
            clock,
            jwt,
            cors_origin,
        } = self;
        f.debug_struct("AppState")
            .field("db", db)
            .field("settings", settings)
            .field("clock", clock)
            .field("jwt", jwt)
            .field("cors_origin", cors_origin)
            .finish()
    }
}

/// A setting [`AppState::new`] cannot turn into state.
///
/// These are operator-facing start-up failures, reported by the `api` binary
/// before it serves anything; they never reach an API response.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// `CARDDEMO_CLOCK_OVERRIDE` is set but is not an RFC 3339 instant.
    #[error("invalid CARDDEMO_CLOCK_OVERRIDE: {0}")]
    Clock(#[from] ClockError),
    /// `CARDDEMO_CORS_ORIGIN` holds a byte a header value cannot carry: a
    /// control character other than tab, such as a line break, or DEL.
    #[error("invalid CARDDEMO_CORS_ORIGIN: {0}")]
    CorsOrigin(#[from] InvalidHeaderValue),
}

/// Everything [`AppState::new`] derives from the settings, apart from the
/// database handle the caller supplies. Kept apart from `new` so the
/// settings checks run, and are tested, without a database.
#[derive(Debug)]
struct Derived {
    clock: AnyClock,
    cors_origin: HeaderValue,
    jwt: JwtKeys,
}

impl Derived {
    /// Parses the clock override, then the CORS origin, and builds the token
    /// keys, which cannot fail.
    fn from_settings(settings: &Settings) -> Result<Self, StateError> {
        let clock = AnyClock::from_override(settings.clock_override.as_deref())?;
        let cors_origin = HeaderValue::from_str(&settings.cors_origin)?;
        let jwt = JwtKeys::new(&settings.jwt_secret, settings.jwt_ttl_secs);
        Ok(Self {
            clock,
            cors_origin,
            jwt,
        })
    }
}

impl AppState {
    /// Builds the state from the process's database handle and settings.
    ///
    /// - The clock is `AnyClock::from_override` of `CARDDEMO_CLOCK_OVERRIDE`:
    ///   the system clock when unset, else the pinned instant in UTC.
    /// - The CORS origin is `CARDDEMO_CORS_ORIGIN` as a header value, for the
    ///   router's CORS layer.
    /// - The token keys sign and verify HS256 tokens with
    ///   `CARDDEMO_JWT_SECRET` and issue them for `CARDDEMO_JWT_TTL_SECS`
    ///   seconds.
    ///
    /// Nothing here connects or reads the environment: `db` is already open
    /// and `settings` already resolved.
    ///
    /// # Errors
    ///
    /// [`StateError::Clock`] when `CARDDEMO_CLOCK_OVERRIDE` is set and is not
    /// an RFC 3339 instant; otherwise [`StateError::CorsOrigin`] when
    /// `CARDDEMO_CORS_ORIGIN` is not a valid header value.
    pub fn new(db: Db, settings: Settings) -> Result<Self, StateError> {
        let Derived {
            clock,
            cors_origin,
            jwt,
        } = Derived::from_settings(&settings)?;
        Ok(Self {
            db,
            settings: Arc::new(settings),
            clock: Arc::new(clock),
            jwt: Arc::new(jwt),
            cors_origin,
        })
    }

    /// The connection pool. A handler opens its unit of work with
    /// `state.db().begin().await`.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The settings the process started with.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The processing clock. A handler reads the instant with
    /// `state.clock().now()` (the `Clock` trait in scope) or passes the clock
    /// on to a rule that takes `&impl Clock`.
    #[must_use]
    pub fn clock(&self) -> &AnyClock {
        &self.clock
    }

    /// The keys that issue and verify bearer tokens.
    #[must_use]
    pub fn jwt(&self) -> &JwtKeys {
        &self.jwt
    }

    /// The one browser origin the CORS layer admits.
    #[must_use]
    pub fn cors_origin(&self) -> &HeaderValue {
        &self.cors_origin
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::clock::{AnyClock, Clock, ClockError, FixedClock, SystemClock};
    use carddemo_runtime::config::{Settings, defaults};
    use chrono::{DateTime, Utc};

    use super::{AppState, Derived, StateError};
    use crate::auth::jwt::JwtKeys;

    fn assert_send_sync_clone<T: Send + Sync + Clone + 'static>() {}

    fn settings_with(clock_override: Option<&str>, cors_origin: &str) -> Settings {
        Settings {
            clock_override: clock_override.map(str::to_owned),
            cors_origin: cors_origin.to_owned(),
            ..Settings::default()
        }
    }

    fn derive(settings: &Settings) -> Result<Derived, StateError> {
        Derived::from_settings(settings)
    }

    fn instant(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("an RFC 3339 instant")
            .with_timezone(&Utc)
    }

    #[test]
    fn state_is_shareable_across_handlers_and_threads() {
        assert_send_sync_clone::<AppState>();
    }

    #[test]
    fn defaults_give_the_system_clock_the_vite_origin_and_one_hour_tokens() {
        let derived = derive(&Settings::default()).expect("the defaults are valid");
        assert_eq!(derived.clock, AnyClock::System(SystemClock));
        assert_eq!(derived.cors_origin, defaults::CORS_ORIGIN);
        assert_eq!(derived.cors_origin, "http://localhost:5173");
        assert_eq!(derived.jwt.ttl_secs(), defaults::JWT_TTL_SECS);
    }

    #[test]
    fn clock_override_pins_the_processing_clock_in_utc() {
        for (text, expected) in [
            ("2022-07-01T12:00:00Z", "2022-07-01T12:00:00Z"),
            ("2022-07-01T14:00:00+02:00", "2022-07-01T12:00:00Z"),
            (" 2022-07-18T00:00:00.5Z ", "2022-07-18T00:00:00.5Z"),
        ] {
            let derived = derive(&settings_with(Some(text), defaults::CORS_ORIGIN))
                .expect("a valid override");
            let pinned = instant(expected);
            assert_eq!(
                derived.clock,
                AnyClock::Fixed(FixedClock(pinned)),
                "{text:?}"
            );
            assert_eq!(derived.clock.now(), pinned, "{text:?}");
        }
    }

    #[test]
    fn invalid_clock_override_is_a_clock_error() {
        for text in ["not-a-time", "2022-07-01", "2022-07-01 12:00:00", "", "   "] {
            let err = derive(&settings_with(Some(text), defaults::CORS_ORIGIN))
                .expect_err("the override is rejected");
            match &err {
                StateError::Clock(ClockError::InvalidOverride(value)) => {
                    assert_eq!(value, text.trim(), "{text:?}");
                }
                StateError::CorsOrigin(_) => panic!("{text:?} gave {err:?}"),
            }
            assert!(
                err.to_string()
                    .starts_with("invalid CARDDEMO_CLOCK_OVERRIDE: "),
                "{err}"
            );
        }
    }

    #[test]
    fn origin_a_header_cannot_carry_is_a_cors_origin_error() {
        for origin in [
            "http://localhost:5173\n",
            "http://localhost:5173\r\nX-Injected: 1",
            "http://local\u{0}host",
            "http://localhost:5173\u{7f}",
        ] {
            let err = derive(&settings_with(None, origin)).expect_err("the origin is rejected");
            assert!(
                matches!(err, StateError::CorsOrigin(_)),
                "{origin:?}: {err:?}"
            );
            assert!(
                err.to_string()
                    .starts_with("invalid CARDDEMO_CORS_ORIGIN: "),
                "{err}"
            );
        }
    }

    #[test]
    fn clock_override_is_checked_before_the_origin() {
        let err = derive(&settings_with(Some("yesterday"), "http://a\n"))
            .expect_err("both settings are invalid");
        assert!(matches!(err, StateError::Clock(_)), "{err:?}");
    }

    #[test]
    fn token_keys_use_the_configured_secret_and_lifetime() {
        let settings = Settings {
            jwt_secret: "state-test-secret".to_owned(),
            jwt_ttl_secs: 60,
            ..Settings::default()
        };
        let derived = derive(&settings).expect("valid settings");
        assert_eq!(derived.jwt.ttl_secs(), 60);

        let issued = derived.jwt.issue("ADMIN001", "A").expect("issues");
        let claims = JwtKeys::new("state-test-secret", 60)
            .verify(&issued.token)
            .expect("the configured secret verifies the token");
        assert_eq!(claims.exp - claims.iat, 60);
        assert!(
            JwtKeys::new(defaults::JWT_SECRET, 60)
                .verify(&issued.token)
                .is_err(),
            "another secret must not verify the token"
        );
    }
}
