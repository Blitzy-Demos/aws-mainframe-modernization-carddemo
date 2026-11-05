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
//!   processing clock, the token keys, the CORS origin and the three admission
//!   limits. All of it is built once at start-up, by [`PreparedState::new`]
//!   from the settings before any connection opens and by
//!   [`PreparedState::into_state`] once the database handle exists. It is
//!   immutable afterwards, apart from the permits the admission limits lend
//!   out, so a clone is cheap and shared across the runtime's threads.
//!
//! # Admission limits
//!
//! Three semaphores bound work whose cost would otherwise grow with the number
//! of concurrent requests. A request beyond a limit waits for a permit, in
//! arrival order; none is refused and none times out, because the API has no
//! rate limiting. No setting sizes them: each is derived from the pool or the
//! CPUs.
//!
//! - `AppState::admit_tran_id_writer` admits the CT02 adds and confirmed
//!   CB00 payments, the requests that allocate a TRAN-ID. The allocation
//!   serializes on an advisory lock held until the writer's unit of work
//!   ends (`carddemo_persistence::ids`), and a writer waiting for it holds a
//!   pooled connection. A writer therefore takes its permit before it opens
//!   its unit of work: at most `tran_id_writer_permits` of the pool's
//!   connections wait on the lock, and a burst of writers queues here, holding
//!   no connection, while the reads keep the rest of the pool.
//! - `AppState::admit_argon2` admits argon2id hashing and verification,
//!   each of which holds about 19 MiB while it runs on tokio's blocking pool,
//!   one run per CPU the process may use. Concurrent sign-ons then queue for
//!   a permit instead of each taking a blocking thread and its memory, so the
//!   process's peak memory has a fixed ceiling. `run_argon2` keeps the
//!   permit inside the blocking task until the step returns.
//! - `AppState::admit_report` admits the CR00 reports. A report holds one
//!   pooled connection and one database snapshot from the start of its run
//!   until its streamed answer is sent or abandoned, and besides them one
//!   page of records, the chunk of JSON it is filling, the one chunk waiting
//!   for its response body and what the server's bounded write buffer holds,
//!   whatever its size.
//!   It therefore takes its permit before it opens its unit of work, and the
//!   task that writes its answer keeps the permit: at most `report_permits`
//!   reports run or send at once, overlapping reports queue here holding no
//!   connection, and the other operations keep the rest of the pool. A
//!   client that reads its report slowly keeps the permit and the connection
//!   for as long as it reads. One that stops reading keeps them only until
//!   its answer has waited
//!   [`REPORT_SEND_DEADLINE`](crate::handlers::reports::REPORT_SEND_DEADLINE)
//!   for it to take a chunk: the report is then abandoned, its unit of work
//!   rolled back and its permit returned, so no client that stops reading
//!   keeps a permit for good.
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
//!
//! # Version tokens
//!
//! [`AppState::version_key`] keys the record version tokens that replace the
//! COMMAREA copy of a fetched record: `GET /accounts/{id}` and
//! `GET /cards/{id}` issue them, the matching `PUT` checks them (AAP 0.3.7).
//! The key is built from `CARDDEMO_JWT_SECRET`, the server secret the process
//! already holds, so every API process started with the same settings checks
//! the tokens any of them issued.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use axum::extract::FromRef;
use axum::http::HeaderValue;
use carddemo_domain::clock::{AnyClock, ClockError};
use carddemo_domain::rules::{VersionError, VersionKey};
use carddemo_persistence::tx::Db;
use carddemo_runtime::config::Settings;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinError;

use crate::auth::jwt::JwtKeys;
use crate::error::ApiError;

/// The fewest TRAN-ID writers [`tran_id_writer_permits`] admits at once,
/// whatever the pool size: a CT02 add and a CB00 payment on one account can
/// then both wait in the database, in the lock order
/// `carddemo_persistence::ids` documents, as on a full-sized pool.
const MIN_TRAN_ID_WRITERS: usize = 2;

/// The fewest CR00 reports [`report_permits`] admits at once, whatever the
/// pool size, so a report can always run.
const MIN_REPORTS: usize = 1;

/// The shared state of the API process.
///
/// Cloning copies a pool handle, seven reference counts and the origin
/// header; every clone sees the same connections, settings, clock, keys and
/// admission limits.
#[derive(Clone)]
pub struct AppState {
    db: Db,
    settings: Arc<Settings>,
    clock: Arc<AnyClock>,
    jwt: Arc<JwtKeys>,
    version_key: Arc<VersionKey>,
    cors_origin: HeaderValue,
    tran_id_writers: Arc<Semaphore>,
    argon2_work: Arc<Semaphore>,
    report_work: Arc<Semaphore>,
}

impl fmt::Debug for AppState {
    /// Shows every field through its own `Debug`, each of which keeps its
    /// secrets out: [`Settings`] redacts the database and broker URLs and the
    /// JWT secret, [`JwtKeys`] prints only its algorithm and lifetime,
    /// [`VersionKey`] prints `VersionKey(<redacted>)`, and [`Db`] only its
    /// pool's size and options. The three admission limits show the permits
    /// they have free.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Exhaustive destructuring: a field added to `AppState` fails to
        // compile here until it is decided whether `Debug` may show it.
        let Self {
            db,
            settings,
            clock,
            jwt,
            version_key,
            cors_origin,
            tran_id_writers,
            argon2_work,
            report_work,
        } = self;
        f.debug_struct("AppState")
            .field("db", db)
            .field("settings", settings)
            .field("clock", clock)
            .field("jwt", jwt)
            .field("version_key", version_key)
            .field("cors_origin", cors_origin)
            .field(
                "tran_id_writer_permits_free",
                &tran_id_writers.available_permits(),
            )
            .field("argon2_permits_free", &argon2_work.available_permits())
            .field("report_permits_free", &report_work.available_permits())
            .finish()
    }
}

/// A setting [`PreparedState::new`] cannot turn into state.
///
/// These are operator-facing start-up failures, reported by the `api` binary
/// before it serves anything; they never reach an API response.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// `CARDDEMO_CLOCK_OVERRIDE` is set but is not an RFC 3339 instant of at
    /// most microsecond precision, without a leap second.
    #[error("invalid CARDDEMO_CLOCK_OVERRIDE: {0}")]
    Clock(#[from] ClockError),
    /// `CARDDEMO_CORS_ORIGIN` is not one serialized browser origin, the exact
    /// `Origin` header text a browser sends: `http://` or `https://`, a
    /// lower-case host and an optional non-default port, nothing else. The
    /// wildcard `*`, an empty value, a path (even a lone trailing `/`), a
    /// query, a fragment, user information, an upper-case or non-ASCII host,
    /// a malformed IP literal and an invalid or default port are all
    /// rejected, because the one-entry allow-list compares the header byte
    /// for byte and `tower-http` panics on `*`.
    ///
    /// The rejected value is never retained or reported, by `Display` or by
    /// `Debug`: a mistaken value can hold user information, a query or a
    /// fragment that carries a credential. The setting's name and the fixed
    /// reason of the rule the value breaks identify the fault, and neither
    /// holds a line break, so the report stays one line.
    #[error("invalid CARDDEMO_CORS_ORIGIN: {reason}")]
    CorsOrigin {
        /// Why the value is not a serialized origin: the fixed text of the
        /// first rule it breaks.
        reason: &'static str,
    },
    /// `CARDDEMO_JWT_TTL_SECS` is a lifetime no token can be issued with:
    /// zero, so every token is expired when issued, or so long that the
    /// expiry of a token issued now overflows a 64-bit timestamp or lies past
    /// the last instant `chrono` represents, so every sign-on would fail.
    #[error(
        "invalid CARDDEMO_JWT_TTL_SECS: {ttl_secs}: a token lifetime must be positive and give an expiry a timestamp can represent"
    )]
    JwtTtl {
        /// The rejected lifetime, in seconds.
        ttl_secs: u64,
    },
    /// `CARDDEMO_JWT_SECRET` cannot key the record version tokens: it is
    /// empty, which would leave every token computable from the record.
    #[error("invalid CARDDEMO_JWT_SECRET for the record version key: {0}")]
    VersionKey(#[from] VersionError),
}

/// The settings checked and turned into everything an [`AppState`] holds
/// apart from the database handle.
///
/// The `api` binary builds it before it opens any connection, so a setting
/// that cannot become state stops start-up before the database is reached,
/// and the database failure cannot mask it. [`PreparedState::into_state`]
/// then adds the handle without parsing anything again.
///
/// `Debug` shows every field through its own `Debug`, each of which keeps
/// its secrets out, as for [`AppState`].
#[derive(Debug)]
pub struct PreparedState {
    settings: Settings,
    clock: AnyClock,
    cors_origin: HeaderValue,
    jwt: JwtKeys,
    version_key: VersionKey,
    argon2_work: Semaphore,
}

impl PreparedState {
    /// Checks `settings` and derives the state from them:
    ///
    /// - The clock is `AnyClock::from_override` of `CARDDEMO_CLOCK_OVERRIDE`:
    ///   the system clock when unset, else the pinned instant in UTC.
    /// - The CORS origin is `CARDDEMO_CORS_ORIGIN`, checked to be one
    ///   serialized browser origin, as a header value for the router's CORS
    ///   layer.
    /// - The token keys sign and verify HS256 tokens with
    ///   `CARDDEMO_JWT_SECRET` and issue them for `CARDDEMO_JWT_TTL_SECS`
    ///   seconds, a lifetime checked to give a representable expiry.
    /// - The version key keys the record version tokens with
    ///   `CARDDEMO_JWT_SECRET`.
    /// - The argon2 admission has `argon2_permits` permits, one per CPU the
    ///   process may use. No setting sizes it.
    ///
    /// Nothing here connects or reads the environment: `settings` are
    /// already resolved.
    ///
    /// # Errors
    ///
    /// The first setting that fails, in this order: [`StateError::Clock`]
    /// when `CARDDEMO_CLOCK_OVERRIDE` is set and is not an RFC 3339 instant of
    /// at most microsecond precision,
    /// [`StateError::CorsOrigin`] when `CARDDEMO_CORS_ORIGIN` is not one
    /// serialized origin, [`StateError::JwtTtl`] when `CARDDEMO_JWT_TTL_SECS`
    /// is zero or gives an unrepresentable expiry, and
    /// [`StateError::VersionKey`] when `CARDDEMO_JWT_SECRET` is empty.
    pub fn new(settings: Settings) -> Result<Self, StateError> {
        let clock = AnyClock::from_override(settings.clock_override.as_deref())?;
        let cors_origin = cors_origin(&settings.cors_origin)?;
        let jwt = JwtKeys::new(&settings.jwt_secret, settings.jwt_ttl_secs);
        if settings.jwt_ttl_secs == 0 || !jwt.expiry_is_representable() {
            return Err(StateError::JwtTtl {
                ttl_secs: settings.jwt_ttl_secs,
            });
        }
        let version_key = VersionKey::new(settings.jwt_secret.as_bytes())?;
        Ok(Self {
            settings,
            clock,
            cors_origin,
            jwt,
            version_key,
            argon2_work: Semaphore::new(argon2_permits()),
        })
    }

    /// The settings the state was derived from, for what the process needs
    /// before it has a database handle: the database URL and the listen
    /// address.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The state of the process: the derived values with `db`, the pool
    /// opened on [`Settings::database_url`], and the TRAN-ID writer admission
    /// and the report admission, whose `tran_id_writer_permits` and
    /// `report_permits` are sized from that pool's connection limit.
    #[must_use]
    pub fn into_state(self, db: Db) -> AppState {
        let Self {
            settings,
            clock,
            cors_origin,
            jwt,
            version_key,
            argon2_work,
        } = self;
        let max_connections = db.pool().options().get_max_connections();
        let writers = tran_id_writer_permits(max_connections);
        let reports = report_permits(max_connections);
        AppState {
            db,
            settings: Arc::new(settings),
            clock: Arc::new(clock),
            jwt: Arc::new(jwt),
            version_key: Arc::new(version_key),
            cors_origin,
            tran_id_writers: Arc::new(Semaphore::new(writers)),
            argon2_work: Arc::new(argon2_work),
            report_work: Arc::new(Semaphore::new(reports)),
        }
    }
}

/// The permits of the TRAN-ID writer admission for a pool of
/// `max_connections`: half the connections but one, rounded down, and never
/// fewer than [`MIN_TRAN_ID_WRITERS`]. A pool of 10 admits 4 writers.
///
/// The writers admitted at once are the most connections that wait on the
/// TRAN-ID advisory lock together, so more than half the pool stays free for
/// the reads and for the `USRSEC` read that checks every request's bearer
/// token, however many writers queue.
fn tran_id_writer_permits(max_connections: u32) -> usize {
    // A `u32` fits the `usize` of every 32- and 64-bit target.
    usize::try_from(max_connections.saturating_sub(1) / 2)
        .map_or(MIN_TRAN_ID_WRITERS, |half| half.max(MIN_TRAN_ID_WRITERS))
}

/// The permits of the argon2 admission: the CPUs the process may use, as
/// [`std::thread::available_parallelism`] counts them, or 1, with a warning,
/// when it cannot count them.
///
/// argon2id runs are CPU-bound, so more of them at once than there are CPUs
/// only add their memory, never their throughput.
fn argon2_permits() -> usize {
    match std::thread::available_parallelism() {
        Ok(cpus) => cpus.get(),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "the CPU count is unknown; argon2 work is admitted one run at a time"
            );
            1
        }
    }
}

/// The permits of the report admission for a pool of `max_connections`: a
/// quarter of the connections, rounded down, and never fewer than
/// [`MIN_REPORTS`]. A pool of 10 admits 2 reports, the testkit pool of 5
/// admits 1.
///
/// The reports admitted at once are the most connections and database
/// snapshots reports hold together. With the TRAN-ID writers, they leave 4
/// of a pool of 10 to the other reads and to the `USRSEC` read that checks
/// every request's bearer token, however many reports and writers queue.
fn report_permits(max_connections: u32) -> usize {
    // A `u32` fits the `usize` of every 32- and 64-bit target.
    usize::try_from(max_connections / 4).map_or(MIN_REPORTS, |quarter| quarter.max(MIN_REPORTS))
}

/// A permit of `permits`, the admission limit named `admission`, once one is
/// free.
///
/// # Errors
///
/// [`ApiError::unexpected`] when the semaphore is closed. No admission
/// semaphore is ever closed, so the cause is logged as the defect it would
/// be, and the request answers without internal detail.
async fn admitted(
    permits: &Arc<Semaphore>,
    admission: &'static str,
) -> Result<OwnedSemaphorePermit, ApiError> {
    Arc::clone(permits).acquire_owned().await.map_err(|error| {
        tracing::error!(admission, error = %error, "admission semaphore closed");
        ApiError::unexpected()
    })
}

/// Runs `step`, an argon2id hash or verification, on tokio's blocking pool
/// while it holds `permit`, an [`AppState::admit_argon2`] permit.
///
/// The permit moves into the blocking task and is released only once `step`
/// has returned. A blocking task runs to its end even when the request that
/// awaits it is dropped, its client gone, so a permit the request alone held
/// would be released while the step's memory is still in use.
///
/// # Errors
///
/// The [`JoinError`] of a step that panicked, or that the runtime dropped
/// unstarted while shutting down. The caller logs it and chooses its answer.
pub(crate) async fn run_argon2<T, F>(permit: OwnedSemaphorePermit, step: F) -> Result<T, JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let output = step();
        drop(permit);
        output
    })
    .await
}

/// The reasons [`StateError::CorsOrigin`] gives, one per rule of a serialized
/// origin.
mod cors_reason {
    pub const WILDCARD: &str =
        "the wildcard `*` is not an origin: the API admits exactly one concrete origin";
    pub const EMPTY: &str = "no origin is set";
    pub const SCHEME: &str = "an origin starts with `http://` or `https://`, in lower case";
    pub const PATH: &str = "an origin has no path, query or fragment, not even a trailing `/`";
    pub const USER_INFO: &str = "an origin carries no user information";
    pub const NO_HOST: &str = "the origin has no host";
    pub const HOST_CASE: &str = "a browser sends the host in lower case";
    pub const HOST_CHARACTERS: &str =
        "the host holds a character other than `a`-`z`, `0`-`9`, `-`, `.` and `_`";
    pub const IPV4: &str =
        "an IPv4 host is four decimal octets without leading zeros, as a browser sends it";
    pub const IPV6: &str =
        "an IPv6 host is a bracketed address in the compressed lower-case form a browser sends";
    pub const PORT: &str = "the port is not a number from 1 to 65535 without leading zeros";
    pub const DEFAULT_PORT: &str =
        "a browser leaves the scheme's default port (80 for http, 443 for https) out of the origin";
    pub const HEADER: &str = "a header value cannot carry it";
}

/// `value` as the header value the CORS allow-list compares, once it is
/// checked to be one serialized origin: the `Origin` header text a browser
/// sends, `scheme "://" host [":" port]` as the WHATWG URL Standard
/// serializes it.
///
/// - The scheme is `http` or `https`, in lower case.
/// - The host is a domain of lower-case ASCII letters, digits, `-`, `.` and
///   `_`; an IPv4 address in dotted-decimal form; or a bracketed IPv6 address
///   in its compressed lower-case form.
/// - A port, when present, is 1 to 65535 without leading zeros and is not
///   the scheme's default, which a browser leaves out.
///
/// The allow-list compares bytes, so any other spelling of an origin would
/// admit no browser; `*` in it would make `tower-http` panic.
///
/// # Errors
///
/// [`StateError::CorsOrigin`] with the reason of the first rule `value`
/// breaks. The error holds no part of `value`, which may carry a credential.
fn cors_origin(value: &str) -> Result<HeaderValue, StateError> {
    let reject = |reason| StateError::CorsOrigin { reason };
    check_origin(value).map_err(reject)?;
    // Every character `check_origin` accepts is visible ASCII, which a
    // header value always carries.
    HeaderValue::from_str(value).map_err(|_| reject(cors_reason::HEADER))
}

/// Checks the rules of [`cors_origin`] in order.
///
/// # Errors
///
/// The [`cors_reason`] of the first rule `value` breaks.
fn check_origin(value: &str) -> Result<(), &'static str> {
    if value == "*" {
        return Err(cors_reason::WILDCARD);
    }
    if value.is_empty() {
        return Err(cors_reason::EMPTY);
    }
    let (authority, default_port) = if let Some(rest) = value.strip_prefix("http://") {
        (rest, 80)
    } else if let Some(rest) = value.strip_prefix("https://") {
        (rest, 443)
    } else {
        return Err(cors_reason::SCHEME);
    };
    if authority.contains(['/', '?', '#']) {
        return Err(cors_reason::PATH);
    }
    if authority.contains('@') {
        return Err(cors_reason::USER_INFO);
    }
    let (host, port) = split_host_port(authority)?;
    check_host(host)?;
    match port {
        Some(port) => check_port(port, default_port),
        None => Ok(()),
    }
}

/// Splits an origin's authority into its host, brackets included for an IPv6
/// address, and the port text after the `:`, if there is one.
///
/// # Errors
///
/// [`cors_reason::IPV6`] when a `[` has no closing `]`, or text other than a
/// `:` and the port follows it.
fn split_host_port(authority: &str) -> Result<(&str, Option<&str>), &'static str> {
    let (host, rest) = if authority.starts_with('[') {
        let end = authority.find(']').ok_or(cors_reason::IPV6)?;
        authority.split_at(end + 1)
    } else {
        authority
            .find(':')
            .map_or((authority, ""), |colon| authority.split_at(colon))
    };
    match rest.strip_prefix(':') {
        Some(port) => Ok((host, Some(port))),
        None if rest.is_empty() => Ok((host, None)),
        None => Err(cors_reason::IPV6),
    }
}

/// Checks an origin's host: a bracketed IPv6 address, an IPv4 address or a
/// domain, each in the form a browser serializes.
///
/// # Errors
///
/// The [`cors_reason`] of the first rule `host` breaks.
fn check_host(host: &str) -> Result<(), &'static str> {
    if host.is_empty() {
        return Err(cors_reason::NO_HOST);
    }
    if let Some(bracketed) = host.strip_prefix('[') {
        let literal = bracketed.strip_suffix(']').ok_or(cors_reason::IPV6)?;
        return match literal.parse::<Ipv6Addr>() {
            Ok(address) if serialized_ipv6(address) == literal => Ok(()),
            _ => Err(cors_reason::IPV6),
        };
    }
    if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(cors_reason::HOST_CASE);
    }
    let domain_byte = |byte: u8| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
    };
    if !host.bytes().all(domain_byte) {
        return Err(cors_reason::HOST_CHARACTERS);
    }
    // A browser parses a host whose last label is a number as an IPv4
    // address and sends it back in dotted-decimal form.
    if ends_in_a_number(host)
        && !host
            .parse::<Ipv4Addr>()
            .is_ok_and(|address| address.to_string() == host)
    {
        return Err(cors_reason::IPV4);
    }
    Ok(())
}

/// The URL Standard's "ends in a number" test: whether the last label of
/// `host`, a trailing empty label aside, is decimal digits or `0x` and hex
/// digits, which makes a browser parse the host as an IPv4 address.
fn ends_in_a_number(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let last = host.rsplit_once('.').map_or(host, |(_, last)| last);
    !last.is_empty()
        && (last.bytes().all(|byte| byte.is_ascii_digit())
            || last
                .strip_prefix("0x")
                .is_some_and(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit())))
}

/// `address` as the URL Standard serializes an IPv6 host, without the
/// brackets: lower-case hex pieces without leading zeros, and the first
/// longest run of two or more zero pieces compressed to `::`.
fn serialized_ipv6(address: Ipv6Addr) -> String {
    let pieces = address.segments();
    let mut compress: Option<(usize, usize)> = None;
    let mut start = 0;
    while start < pieces.len() {
        let run = pieces[start..]
            .iter()
            .take_while(|piece| **piece == 0)
            .count();
        if run >= 2 && compress.is_none_or(|(_, longest)| run > longest) {
            compress = Some((start, run));
        }
        start += run.max(1);
    }
    let hex = |pieces: &[u16]| {
        pieces
            .iter()
            .map(|piece| format!("{piece:x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    match compress {
        Some((start, run)) => format!("{}::{}", hex(&pieces[..start]), hex(&pieces[start + run..])),
        None => hex(&pieces),
    }
}

/// Checks an origin's port text: 1 to 65535 in decimal without leading
/// zeros, and not `default_port`, the scheme's default.
///
/// # Errors
///
/// [`cors_reason::PORT`] or [`cors_reason::DEFAULT_PORT`].
fn check_port(port: &str, default_port: u16) -> Result<(), &'static str> {
    if port.starts_with('0') || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(cors_reason::PORT);
    }
    // An empty port, or one above 65535, fails to parse.
    match port.parse::<u16>() {
        Ok(number) if number == default_port => Err(cors_reason::DEFAULT_PORT),
        Ok(_) => Ok(()),
        Err(_) => Err(cors_reason::PORT),
    }
}

impl AppState {
    /// Builds the state from a database handle that is already open and the
    /// settings: [`PreparedState::new`] of `settings`, then
    /// [`PreparedState::into_state`] with `db`.
    ///
    /// For a caller that has the handle first, such as a test on its own
    /// database container. The `api` binary prepares the state before it
    /// connects instead, so an invalid setting is reported before any
    /// connection attempt.
    ///
    /// # Errors
    ///
    /// Any error of [`PreparedState::new`].
    pub fn new(db: Db, settings: Settings) -> Result<Self, StateError> {
        Ok(PreparedState::new(settings)?.into_state(db))
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

    /// The key of the record version tokens: a handler passes it to the view
    /// rule that issues a token and to the update rule that checks one.
    #[must_use]
    pub fn version_key(&self) -> &VersionKey {
        &self.version_key
    }

    /// The one browser origin the CORS layer admits.
    #[must_use]
    pub fn cors_origin(&self) -> &HeaderValue {
        &self.cors_origin
    }

    /// Waits for a TRAN-ID writer permit, the admission of the module
    /// documentation. A CT02 add or a confirmed CB00 payment takes it before
    /// it opens its unit of work and holds it until that unit has committed
    /// or rolled back, so a writer queued here holds no pooled connection.
    ///
    /// # Errors
    ///
    /// [`ApiError::unexpected`] when the semaphore is closed, which nothing
    /// does; the cause is logged.
    pub(crate) async fn admit_tran_id_writer(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        admitted(&self.tran_id_writers, "tran_id_writers").await
    }

    /// Waits for an argon2 permit, the admission of the module documentation.
    /// The holder passes it to [`run_argon2`] with the argon2id step it
    /// admits.
    ///
    /// # Errors
    ///
    /// [`ApiError::unexpected`] when the semaphore is closed, which nothing
    /// does; the cause is logged.
    pub(crate) async fn admit_argon2(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        admitted(&self.argon2_work, "argon2_work").await
    }

    /// Waits for a report permit, the admission of the module documentation.
    /// A CR00 report takes it before it opens its read-only unit of work, and
    /// the task that writes its answer holds it until the answer is sent or
    /// abandoned, so a report queued here holds no pooled connection and no
    /// snapshot.
    ///
    /// # Errors
    ///
    /// [`ApiError::unexpected`] when the semaphore is closed, which nothing
    /// does; the cause is logged.
    pub(crate) async fn admit_report(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        admitted(&self.report_work, "report_work").await
    }
}

impl FromRef<AppState> for Arc<JwtKeys> {
    /// The token keys alone, for an extractor that needs nothing else of the
    /// state: `crate::auth::extract::BearerUser`, the only input of the
    /// public `POST /auth/logout`, which can then reach no database, setting
    /// or clock.
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.jwt)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use axum::http::StatusCode;
    use carddemo_domain::clock::{AnyClock, Clock, ClockError, FixedClock, SystemClock};
    use carddemo_domain::rules::{VersionError, VersionKey, version_token};
    use carddemo_runtime::config::{Settings, defaults};
    use chrono::{DateTime, Utc};
    use tokio::sync::{Semaphore, oneshot};

    use super::{
        AppState, PreparedState, StateError, admitted, cors_reason, report_permits, run_argon2,
        tran_id_writer_permits,
    };
    use crate::auth::jwt::{JwtKeys, UserType};
    use crate::error::ApiError;

    fn assert_send_sync_clone<T: Send + Sync + Clone + 'static>() {}

    fn settings_with(clock_override: Option<&str>, cors_origin: &str) -> Settings {
        Settings {
            clock_override: clock_override.map(str::to_owned),
            cors_origin: cors_origin.to_owned(),
            ..Settings::default()
        }
    }

    fn derive(settings: &Settings) -> Result<PreparedState, StateError> {
        PreparedState::new(settings.clone())
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
        assert_eq!(derived.settings(), &Settings::default());
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
                StateError::Clock(ClockError::InexactOverride { .. })
                | StateError::CorsOrigin { .. }
                | StateError::JwtTtl { .. }
                | StateError::VersionKey(_) => panic!("{text:?} gave {err:?}"),
            }
            assert!(
                err.to_string()
                    .starts_with("invalid CARDDEMO_CLOCK_OVERRIDE: "),
                "{err}"
            );
        }
    }

    #[test]
    fn value_that_is_not_one_serialized_origin_is_a_cors_origin_error() {
        for (origin, expected) in [
            ("*", cors_reason::WILDCARD),
            ("", cors_reason::EMPTY),
            (" ", cors_reason::SCHEME),
            ("null", cors_reason::SCHEME),
            ("localhost:5173", cors_reason::SCHEME),
            ("ftp://localhost", cors_reason::SCHEME),
            ("HTTP://localhost:5173", cors_reason::SCHEME),
            ("http:/localhost:5173", cors_reason::SCHEME),
            (" http://localhost:5173", cors_reason::SCHEME),
            ("http://", cors_reason::NO_HOST),
            ("http://:5173", cors_reason::NO_HOST),
            ("http://::1:5173", cors_reason::NO_HOST),
            ("http://localhost:5173/", cors_reason::PATH),
            ("http://localhost:5173/app", cors_reason::PATH),
            ("http://localhost:5173?debug=1", cors_reason::PATH),
            ("http://localhost:5173#top", cors_reason::PATH),
            ("http://user@localhost:5173", cors_reason::USER_INFO),
            ("http://user:pass@localhost:5173", cors_reason::USER_INFO),
            ("http://Localhost:5173", cors_reason::HOST_CASE),
            ("https://CARDDEMO.EXAMPLE", cors_reason::HOST_CASE),
            ("http://exämple.com", cors_reason::HOST_CHARACTERS),
            ("http://local host:5173", cors_reason::HOST_CHARACTERS),
            ("http://local%68ost:5173", cors_reason::HOST_CHARACTERS),
            ("http://127.1:5173", cors_reason::IPV4),
            ("http://127.0.0.01:5173", cors_reason::IPV4),
            ("http://0x7f.0.0.1:5173", cors_reason::IPV4),
            ("http://127.0.0.1.:5173", cors_reason::IPV4),
            ("http://[::1:5173", cors_reason::IPV6),
            ("http://[]:5173", cors_reason::IPV6),
            ("http://[::g]:5173", cors_reason::IPV6),
            ("http://[::A]:5173", cors_reason::IPV6),
            ("http://[0:0:0:0:0:0:0:1]:5173", cors_reason::IPV6),
            ("http://[1::2:0:0:0:3]:5173", cors_reason::IPV6),
            ("http://[::1]5173", cors_reason::IPV6),
            ("http://localhost:", cors_reason::PORT),
            ("http://localhost:0", cors_reason::PORT),
            ("http://localhost:65536", cors_reason::PORT),
            ("http://localhost:05173", cors_reason::PORT),
            ("http://localhost:+5173", cors_reason::PORT),
            ("http://localhost:5173:1", cors_reason::PORT),
            ("http://localhost:5173 ", cors_reason::PORT),
            ("http://localhost:80", cors_reason::DEFAULT_PORT),
            ("https://localhost:443", cors_reason::DEFAULT_PORT),
            // Control characters a header value cannot carry.
            ("http://localhost:5173\n", cors_reason::PORT),
            ("http://localhost:5173\r\nX-Injected: 1", cors_reason::PORT),
            ("http://local\u{0}host", cors_reason::HOST_CHARACTERS),
            ("http://localhost:5173\u{7f}", cors_reason::PORT),
        ] {
            let err = derive(&settings_with(None, origin)).expect_err("the origin is rejected");
            match &err {
                StateError::CorsOrigin { reason } => {
                    assert_eq!(*reason, expected, "{origin:?}");
                }
                StateError::Clock(_) | StateError::JwtTtl { .. } | StateError::VersionKey(_) => {
                    panic!("{origin:?} gave {err:?}")
                }
            }
            let text = err.to_string();
            assert_eq!(
                text,
                format!("invalid CARDDEMO_CORS_ORIGIN: {expected}"),
                "{origin:?}"
            );
            assert!(!text.contains(['\n', '\r']), "{text:?}");
        }
    }

    #[test]
    fn rejected_origin_never_reaches_the_error_text_or_debug() {
        // The credential-bearing origins are assembled here, so no source
        // line holds a URL with a password in it.
        let user = "carddemo";
        let password = "example-password-7f3a";
        let query_secret = "example-query-secret-7f3a";
        let fragment_secret = "example-fragment-secret-7f3a";
        let path_secret = "example-path-secret-7f3a";
        let secrets = [password, query_secret, fragment_secret, path_secret];
        for (origin, expected) in [
            (
                format!("http://{user}:{password}@localhost:5173"),
                cors_reason::USER_INFO,
            ),
            (
                format!("http://{password}@localhost:5173"),
                cors_reason::USER_INFO,
            ),
            (
                format!("http://localhost:5173?token={query_secret}"),
                cors_reason::PATH,
            ),
            (
                format!("http://localhost:5173#access_token={fragment_secret}"),
                cors_reason::PATH,
            ),
            (
                format!("http://localhost:5173/{path_secret}"),
                cors_reason::PATH,
            ),
            (
                format!("http://{user}:{password}@localhost:5173/?token={query_secret}"),
                cors_reason::PATH,
            ),
        ] {
            let err = derive(&settings_with(None, &origin)).expect_err("the origin is rejected");
            assert!(
                matches!(err, StateError::CorsOrigin { reason } if reason == expected),
                "{err:?}"
            );
            let text = err.to_string();
            assert_eq!(text, format!("invalid CARDDEMO_CORS_ORIGIN: {expected}"));
            let debug = format!("{err:?}");
            for report in [&text, &debug] {
                assert!(!report.contains(&origin), "{report:?} holds the origin");
                for secret in secrets {
                    assert!(!report.contains(secret), "{report:?} holds {secret:?}");
                }
            }
        }
    }

    #[test]
    fn serialized_origin_is_admitted_as_given() {
        for origin in [
            defaults::CORS_ORIGIN,
            "http://localhost:20074",
            "http://127.0.0.1:20073",
            "http://[::1]:5173",
            "http://[2001:db8::8:800:200c:417a]",
            "http://[1:0:0:2::3]:5173",
            "https://carddemo.example",
            "https://app.example.com:8443",
            "http://my_host.local:8080",
            "http://localhost:443",
            "https://localhost:80",
            "http://localhost:65535",
        ] {
            let derived = derive(&settings_with(None, origin))
                .unwrap_or_else(|err| panic!("{origin:?} was rejected: {err}"));
            assert_eq!(derived.cors_origin, origin);
        }
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

        let issued = derived
            .jwt
            .issue("ADMIN001", UserType::Admin)
            .expect("issues");
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

    #[test]
    fn lifetime_no_token_can_be_issued_with_is_a_jwt_ttl_error() {
        let beyond_i64 = u64::try_from(i64::MAX).expect("i64::MAX fits u64");
        for ttl_secs in [0, 10_u64.pow(16), beyond_i64, u64::MAX] {
            let settings = Settings {
                jwt_ttl_secs: ttl_secs,
                ..Settings::default()
            };
            let err = derive(&settings).expect_err("the lifetime is rejected");
            assert!(
                matches!(err, StateError::JwtTtl { ttl_secs: rejected } if rejected == ttl_secs),
                "{ttl_secs}: {err:?}"
            );
            assert!(
                err.to_string()
                    .starts_with(&format!("invalid CARDDEMO_JWT_TTL_SECS: {ttl_secs}: ")),
                "{err}"
            );
        }
    }

    #[test]
    fn settings_are_checked_in_order_clock_origin_lifetime_secret() {
        let all_invalid = Settings {
            clock_override: Some("yesterday".to_owned()),
            cors_origin: "*".to_owned(),
            jwt_ttl_secs: u64::MAX,
            jwt_secret: String::new(),
            ..Settings::default()
        };
        let err = derive(&all_invalid).expect_err("every setting is invalid");
        assert!(matches!(err, StateError::Clock(_)), "{err:?}");

        let settings = Settings {
            clock_override: None,
            ..all_invalid
        };
        let err = derive(&settings).expect_err("three settings are invalid");
        assert!(matches!(err, StateError::CorsOrigin { .. }), "{err:?}");

        let settings = Settings {
            cors_origin: defaults::CORS_ORIGIN.to_owned(),
            ..settings
        };
        let err = derive(&settings).expect_err("two settings are invalid");
        assert!(matches!(err, StateError::JwtTtl { .. }), "{err:?}");

        let settings = Settings {
            jwt_ttl_secs: defaults::JWT_TTL_SECS,
            ..settings
        };
        let err = derive(&settings).expect_err("the secret is invalid");
        assert!(matches!(err, StateError::VersionKey(_)), "{err:?}");
    }

    #[test]
    fn version_key_is_the_configured_secret() {
        let parts: [&[u8]; 2] = [b"747", b"ANIYA VON"];
        let token_under = |secret: &str| {
            let settings = Settings {
                jwt_secret: secret.to_owned(),
                ..Settings::default()
            };
            let derived = derive(&settings).expect("valid settings");
            version_token(&derived.version_key, &parts).expect("a token")
        };
        let configured = VersionKey::new(b"state-test-secret").expect("a secret");
        assert_eq!(
            token_under("state-test-secret"),
            version_token(&configured, &parts).expect("a token")
        );
        assert_ne!(
            token_under("state-test-secret"),
            token_under(defaults::JWT_SECRET),
            "another secret must issue another token"
        );
    }

    #[test]
    fn empty_secret_is_a_version_key_error() {
        let settings = Settings {
            jwt_secret: String::new(),
            ..Settings::default()
        };
        let err = derive(&settings).expect_err("an empty secret keys nothing");
        assert!(
            matches!(err, StateError::VersionKey(VersionError::EmptyKey)),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("invalid CARDDEMO_JWT_SECRET for the record version key: "),
            "{err}"
        );
    }

    #[test]
    fn tran_id_writers_are_half_the_pool_but_one_and_never_fewer_than_two() {
        // (pool connections, writers admitted at once): the testkit pool
        // holds 5 connections and the `api` pool 10.
        for (max_connections, permits) in [(1, 2), (2, 2), (3, 2), (5, 2), (10, 4), (11, 5)] {
            assert_eq!(
                tran_id_writer_permits(max_connections),
                permits,
                "a pool of {max_connections}"
            );
        }
    }

    #[test]
    fn reports_are_a_quarter_of_the_pool_and_never_fewer_than_one() {
        // (pool connections, reports admitted at once): the testkit pool
        // holds 5 connections and the `api` pool 10.
        for (max_connections, permits) in [(0, 1), (1, 1), (4, 1), (5, 1), (8, 2), (10, 2), (12, 3)]
        {
            assert_eq!(
                report_permits(max_connections),
                permits,
                "a pool of {max_connections}"
            );
        }
    }

    #[test]
    fn argon2_work_is_admitted_once_per_cpu_the_process_may_use() {
        let derived = derive(&Settings::default()).expect("the defaults are valid");
        let cpus = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        assert_eq!(derived.argon2_work.available_permits(), cpus);
    }

    /// How long the first admitted argon2 step waits for a second one to
    /// start, so the test fails instead of hanging when none can.
    const PARTNER_DEADLINE: Duration = Duration::from_secs(10);

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn argon2_steps_run_no_more_at_once_than_the_admission_has_permits() {
        const PERMITS: usize = 2;
        const STEPS: usize = 6;
        let admission = Arc::new(Semaphore::new(PERMITS));
        let running = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let requests: Vec<_> = (0..STEPS)
            .map(|step| {
                let admission = Arc::clone(&admission);
                let (running, started, most) = (
                    Arc::clone(&running),
                    Arc::clone(&started),
                    Arc::clone(&most),
                );
                tokio::spawn(async move {
                    let permit = admitted(&admission, "test")
                        .await
                        .expect("the admission is open");
                    run_argon2(permit, move || {
                        most.fetch_max(
                            running.fetch_add(1, Ordering::SeqCst) + 1,
                            Ordering::SeqCst,
                        );
                        started.fetch_add(1, Ordering::SeqCst);
                        // The first step waits for a second, so the test sees
                        // the steps the permits admit together; every later
                        // step finds two started already.
                        let until = Instant::now() + PARTNER_DEADLINE;
                        while started.load(Ordering::SeqCst) < PERMITS && Instant::now() < until {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        std::thread::sleep(Duration::from_millis(20));
                        running.fetch_sub(1, Ordering::SeqCst);
                        step
                    })
                    .await
                    .expect("the step completes")
                })
            })
            .collect();
        let mut finished = Vec::with_capacity(STEPS);
        for request in requests {
            finished.push(request.await.expect("the request task completes"));
        }
        assert_eq!(finished, (0..STEPS).collect::<Vec<_>>(), "every step ran");
        assert_eq!(
            most.load(Ordering::SeqCst),
            PERMITS,
            "the most steps that ran at once"
        );
        assert_eq!(
            admission.available_permits(),
            PERMITS,
            "every permit is back"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_argon2_step_keeps_its_permit_when_the_request_awaiting_it_is_dropped() {
        let admission = Arc::new(Semaphore::new(1));
        let (started, step_started) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let permit = admitted(&admission, "test")
            .await
            .expect("the admission is open");
        let request = tokio::spawn(run_argon2(permit, move || {
            started.send(()).expect("the test awaits the start");
            released.recv().expect("the test releases the step");
        }));
        step_started.await.expect("the step starts");

        // The client is gone: the request future is dropped while its step
        // still runs, as a dropped connection drops a handler.
        request.abort();
        let aborted = request.await.expect_err("the request was dropped");
        assert!(aborted.is_cancelled(), "{aborted}");
        assert_eq!(
            admission.available_permits(),
            0,
            "the running step still holds its permit"
        );

        release.send(()).expect("the step waits for its release");
        let again = Arc::clone(&admission)
            .acquire_owned()
            .await
            .expect("the permit returns once the step has returned");
        drop(again);
        assert_eq!(admission.available_permits(), 1);
    }

    #[tokio::test]
    async fn a_closed_admission_answers_the_unexpected_error_without_panicking() {
        let admission = Arc::new(Semaphore::new(1));
        admission.close();
        let error = admitted(&admission, "test")
            .await
            .expect_err("a closed semaphore admits nothing");
        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.body(), ApiError::unexpected().body());
    }
}
