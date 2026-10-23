//! The assembly of the `CardDemo` REST API: the state every handler receives, the router of the 27
//! operations under `/api/v1`, and the one `OpenAPI` document built from those same operations
//! (AAP 0.3.4.1).
//!
//! - [`AppState`] holds what the services reach: the [`PgStore`], their only path to
//!   `PostgreSQL` (R4), the [`JwtKeys`] that sign and verify the sign-on token, the injected
//!   [`Clock`], their only time source, the job-deck directory of endpoint 14
//!   (`CARDDEMO_INTRDR_DIR`), the counterpart of CORPT00C's `JOBS` transient data queue, and the
//!   [`PasswordWork`] budget every argon2id computation of sign-on, user add and user update
//!   runs under (see "Password work" below).
//!   [`check_intrdr_dir`] is the check `carddemo-api` runs on that directory before it connects,
//!   through the one service that touches the filesystem (AAP 0.5.3).
//! - [`router`] registers every handler through `operation`, which builds what
//!   `utoipa_axum::routes!` builds without the lint suppressions of that macro's expansion, and
//!   splits the result into the axum router and the document, so each operation in the document
//!   is exactly one handler and each handler is exactly one operation. [`openapi`] is that
//!   document alone. Building it opens no connection and reads no environment, so
//!   `carddemo-openapi` and `scripts/regen-api.sh` run offline, and one build always emits the
//!   same bytes: the document is the frontend's only source of payload types (Generated API Types
//!   Only).
//! - [`app`] adds [`DOCUMENT_ROUTE`] after the split, so the document is served without being one
//!   of its own operations, adds the router's own answers (see "Router-level answers" below),
//!   wraps every route and those answers in three layers, from the outermost: the request span,
//!   the response headers and the in-flight limit (each described below), and binds the state.
//!   The document is held once, behind an `Arc`, because axum clones every route for each
//!   connection it accepts.
//! - `EventSite`, `event_metadata` and `emit`, crate-private, write a tracing event out through
//!   `tracing`'s callsite API, without the lint suppression every `tracing` event macro expands
//!   to, for the events of the password-work budget and of the services that use them.
//!
//! # Router-level answers
//!
//! A request that reaches no operation still gets the API's one error body, a `ScreenError`
//! with `content-type: application/json`:
//!
//! - a path no route matches: 404 with `api::error::NO_SUCH_ROUTE`;
//! - a method the route matching the path does not serve: 405 with
//!   `api::error::METHOD_NOT_SERVED`, and the `Allow` header that axum adds, listing the
//!   route's methods (`GET` implies `HEAD`).
//!
//! Both texts are fixed and name no field: neither echoes the method, the path or the query,
//! which can carry account IDs and card numbers. Neither reaches a handler of `handlers`, an
//! extractor or the store, so a token is not checked first. They are not operations of the
//! document.
//!
//! # Response headers
//!
//! Every response, an error, a router-level answer and the document included, carries
//! `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`, set by one
//! `axum::middleware::map_response` layer over every route and fallback. A response can carry
//! the bearer token of sign-on, the full card number endpoint 8 shows an admin, or other
//! cardholder and personal data, so no browser or intermediate cache may keep any of them; and
//! every body is JSON, which a browser must never sniff as another type.
//!
//! # In-flight limit
//!
//! [`app`] works on at most [`MAX_IN_FLIGHT`] requests at once, whatever their route. A request
//! that finds every place taken waits for one, in arrival order, for at most [`ADMISSION_WAIT`],
//! before any extractor reads it or any handler runs, so a burst beyond the limit is served a
//! little later rather than refused. Only when that wait passes without a place is it answered
//! 503 with `api::error::SERVER_BUSY` and `Retry-After: 1`; the client may send it again then. A
//! client that disconnects while waiting gives up its turn. An admitted request keeps its place
//! until its response is returned, so the work and the memory of the requests in flight stay
//! bounded however many clients connect. The limit sits inside the other two layers, so a
//! refused request still has its span, with its route and its 503, and both response headers.
//! An idle connection holds no place.
//!
//! # Request spans
//!
//! Every request, an unmatched one included, runs inside the `http.request` span of
//! `runtime::telemetry::request_span`. The span records the method and the `MatchedPath` route
//! template, such as `/api/v1/accounts/{acct_id}/view`, or `runtime::telemetry::UNMATCHED_ROUTE`
//! when no route matched; `runtime::telemetry::record_response` then adds the status and the
//! latency. The URI, the query string, the headers and the body are never recorded, because path
//! segments and query values carry account IDs and card numbers (Cardholder Data Redaction, AAP
//! 0.3.4.7, 0.3.4.8).
//!
//! The response callback only records the status and the latency; it emits no event. Standard
//! output gets one `INFO` line per request when the request's span closes, written by the
//! subscriber of `runtime::telemetry`, carrying the method, the route template, the status and
//! the latency, so the default `info` output records every request. tower-http's default request
//! event (`DEBUG`) and failure event (`ERROR`, for a 5xx) are kept: neither names the URI.
//!
//! # Password work
//!
//! argon2id is deliberately CPU- and memory-hard: with argon2's default parameters, the ones
//! `domain::password::hash` writes into every stored hash, one computation holds about 19 MiB
//! and a core for its whole run. The services run each one on Tokio's blocking pool, so no
//! runtime worker stalls, but the pool alone bounds nothing: it grows to 512 threads and queues
//! the rest without limit, and a task it has started runs to its end even when its request is
//! dropped. A burst of sign-ons for a known user ID, which anyone can send, would then exhaust
//! the host's memory and starve every other blocking task: uncontrolled resource consumption.
//!
//! [`PasswordWork`] bounds that work for the whole process. A computation is admitted only while
//! fewer than `running + queued` are admitted, runs only while fewer than `running` run, and
//! keeps both places until argon2 returns, whatever happens to the request that asked for it. A
//! request that finds every place taken waits for one, in arrival order, and then for its
//! worker, both within one bounded wait ([`PASSWORD_WAIT`] in `carddemo-api`). Only a request
//! still waiting when that wait ends is refused: its service answers 503 with a `Retry-After`
//! header, the answer of a server too busy to take the request now, and the budget logs its
//! refusals at most once per [`REFUSAL_LOG_INTERVAL`], with how many there were. Argon2's
//! parameters, the verification and every ordinary answer are unchanged; only admission is
//! bounded.
//!
//! # What is deliberately absent
//!
//! - No CORS layer: the Vite dev server proxies `/api`, so the browser sees a single origin.
//! - No connection limit: an idle connection holds its socket and its clone of the router, tens
//!   of kilobytes since the document is shared, and no place of the in-flight limit, which bounds
//!   the work instead.
//! - No authentication middleware: the gate is the `SignedOnUser` or `AdminUser` extractor each
//!   handler declares, which is also what its operation's security requirement documents.
//! - No schema change and no broker: the schema is applied by `carddemo-migrate` alone, and no
//!   online transaction sends a message (Single Schema Executor, AAP 0.5.3).
//!
//! Sign-on (COSGN00C) is the one operation the token does not guard, since it issues the token,
//! and CORPT00C's job deck is the one file the API writes; both are services' concerns, reached
//! only through the handlers registered here.

use std::fmt;
use std::io;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{FromRef, MatchedPath, State};
use axum::handler::Handler;
use axum::http::{HeaderValue, Request, Response, header};
use axum::middleware::Next;
use axum::response::IntoResponse as _;
use axum::routing::{MethodRouter, get};
use axum::{Json, Router, middleware};
use domain::clock::Clock;
use persistence::PgStore;
use runtime::config::{Config, EnvError};
use runtime::telemetry::{UNMATCHED_ROUTE, event_enabled, record_response, request_span};
use serde::{Serialize, Serializer};
use tokio::sync::Semaphore;
use tower_http::trace::{MakeSpan, OnResponse, TraceLayer};
use tracing::callsite::{Callsite as _, DefaultCallsite, Identifier};
use tracing::field::{FieldSet, Value};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata, Span};
use utoipa::__dev::{SchemaReferences, Tags};
use utoipa::OpenApi as _;
use utoipa::Path;
use utoipa::openapi::OpenApi;
use utoipa::openapi::path::Paths;
use utoipa_axum::PathItemExt as _;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouter};

use crate::auth::jwt::JwtKeys;
use crate::error::{METHOD_NOT_SERVED, NO_SUCH_ROUTE, SERVER_BUSY, ScreenError};
use crate::handlers::{
    coactupc, coactvwc, coadm01c, cobil00c, cocrdlic, cocrdslc, cocrdupc, comen01c, copaus0c,
    copaus1c, corpt00c, cosgn00c, cotrn00c, cotrn01c, cotrn02c, cotrtlic, cotrtupc, cousr00c,
    cousr01c, cousr02c, cousr03c,
};
use crate::openapi::ApiDoc;
use crate::services::corpt00c::{DECK_DIR_SETTING, check_deck_dir};

/// Where [`app`] serves the `OpenAPI` document as JSON. It is added after the split, so it is not
/// an operation of the document it serves.
pub const DOCUMENT_ROUTE: &str = "/api-docs/openapi.json";

/// What every handler receives through `axum::extract::State`, and its extractors through
/// [`FromRef`].
///
/// Cloning is cheap: the store clones a pool handle, the keys and the clock an `Arc`, and the
/// password budget two, so every clone shares one budget.
#[derive(Clone)]
pub struct AppState {
    /// The executor of every repository trait, the services' only path to `PostgreSQL` (R4). A
    /// service calls the traits on it, or on the transaction `store.begin()` opens.
    pub store: PgStore,
    /// The HS256 keys that sign the sign-on token and verify every bearer token.
    pub jwt: JwtKeys,
    /// The only time source of the services: `SystemClock` in `carddemo-api`, `FixedClock` in
    /// tests (AAP 0.5.6).
    pub clock: Arc<dyn Clock>,
    /// `CARDDEMO_INTRDR_DIR`: where endpoint 14 writes each job deck as the next `TRNRPT00`
    /// generation.
    pub intrdr_dir: PathBuf,
    /// The budget every argon2id computation runs under: sign-on's verification, the user add's
    /// hash and the user update's change test (see "Password work" in the module
    /// documentation).
    pub password_work: PasswordWork,
}

impl AppState {
    /// The state over `store`, signing and verifying tokens with `jwt`, reading time from
    /// `clock`, writing job decks under `intrdr_dir` and running password work under the
    /// process's default budget, [`PasswordWork::default`].
    #[must_use]
    pub fn new(store: PgStore, jwt: JwtKeys, clock: Arc<dyn Clock>, intrdr_dir: PathBuf) -> Self {
        Self {
            store,
            jwt,
            clock,
            intrdr_dir,
            password_work: PasswordWork::default(),
        }
    }

    /// The same state with `password_work` in place of its budget, for a caller that sizes the
    /// budget itself, such as a test that must fill it.
    #[must_use]
    pub fn with_password_work(self, password_work: PasswordWork) -> Self {
        Self {
            password_work,
            ..self
        }
    }

    /// The state of the `carddemo-api` process: keys from `config.jwt_secret` with lifetime
    /// `config.jwt_ttl_seconds`, and job decks under `config.intrdr_dir`. The environment is not
    /// read again.
    ///
    /// # Errors
    ///
    /// [`EnvError::MissingJwtSecret`] when `CARDDEMO_JWT_SECRET` is empty or unset: the API
    /// cannot sign a token, so it refuses to start (AAP 0.5.6).
    pub fn from_config(
        store: PgStore,
        config: &Config,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, EnvError> {
        let secret = config.require_jwt_secret()?;
        Ok(Self::new(
            store,
            JwtKeys::new(secret, config.jwt_ttl_seconds),
            clock,
            config.intrdr_dir.clone(),
        ))
    }
}

/// Why `carddemo-api` refuses its job-deck directory at startup ([`check_intrdr_dir`]):
/// `CARDDEMO_INTRDR_DIR` names something that is not a directory, or a path through one, so
/// endpoint 14 could write no deck there. Its text names the setting and the I/O error kind,
/// never the path (Cardholder Data Redaction keeps the server's directory layout out of every
/// message, as it keeps it out of the response of endpoint 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntrdrDirError {
    /// What the check found, such as `io::ErrorKind::NotADirectory`.
    kind: io::ErrorKind,
}

impl IntrdrDirError {
    /// What the check found.
    #[must_use]
    pub fn kind(&self) -> io::ErrorKind {
        self.kind
    }
}

impl fmt::Display for IntrdrDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{DECK_DIR_SETTING}, cannot hold job decks: {}",
            self.kind
        )
    }
}

impl std::error::Error for IntrdrDirError {}

/// Checks, before the server starts, that the job-deck directory of `config`
/// (`CARDDEMO_INTRDR_DIR`) can hold the decks of endpoint 14: an existing directory, or no entry
/// at all, which the first submission creates (CORPT00C-036). Reads the filesystem once and
/// creates nothing.
///
/// # Errors
///
/// [`IntrdrDirError`] when the directory exists and is not one, a dangling link included, or a
/// path through something that is not a directory leads to it. Any other failure to read it is
/// left to the submissions, each of which answers it with `Unable to Write TDQ (JOBS)...`
/// (CORPT00C-037).
pub fn check_intrdr_dir(config: &Config) -> Result<(), IntrdrDirError> {
    check_deck_dir(&config.intrdr_dir).map_err(|kind| IntrdrDirError { kind })
}

/// Names every field but shows only the job-deck directory and the password budget's limits: the
/// keys are secret, and neither the store nor the clock has anything a diagnostic needs.
impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("store", &format_args!(".."))
            .field("jwt", &format_args!(".."))
            .field("clock", &format_args!(".."))
            .field("intrdr_dir", &self.intrdr_dir)
            .field("password_work", &self.password_work)
            .finish()
    }
}

/// The most argon2id computations [`PasswordWork::default`] runs at once, however many cores the
/// host has. Each one holds about 19 MiB and a core until it returns, so eight bound the
/// budget's argon2 memory to about 152 MiB and leave the other cores, and the rest of Tokio's
/// blocking pool, to every other request.
pub const MAX_PASSWORD_WORKERS: usize = 8;

/// How many argon2id computations [`PasswordWork::default`] admits to wait for a worker once
/// every worker is busy. A waiting computation has not started, so it holds no argon2 memory and
/// no thread, only its request. With the eight workers of [`MAX_PASSWORD_WORKERS`], 248 places
/// make 256 admitted at once, so a burst of 200 concurrent sign-ons is admitted without any of
/// them waiting for a place.
///
/// The bound stays because it keeps an admitted request's own wait short: at most 31
/// verifications per worker are queued ahead of it, about 0.6 s at the 18 ms one takes in a
/// release build, well inside [`PASSWORD_WAIT`]. A request beyond the places waits for one,
/// which costs the budget nothing, and is refused only when its wait ends first.
pub const PASSWORD_QUEUE: usize = 248;

/// How long [`PasswordWork::default`] lets a request wait, from when it asks, for an admission
/// place and then a worker before it is refused.
///
/// Three seconds absorb a backlog of more than a thousand verifications on eight workers, far
/// beyond an ordinary burst of sign-ons, and keep a request's wait in the budget and its own
/// argon2 run within the 8 s that `carddemo-api` gives the requests in flight at a stop. The
/// wait starts only once [`app`]'s in-flight limit has admitted the request, so at most
/// [`MAX_IN_FLIGHT`] computations ever ask the budget at once. A refused request is answered 503
/// with `Retry-After` of this many seconds (`crate::error::RETRY_AFTER_SECONDS`).
pub const PASSWORD_WAIT: Duration = Duration::from_secs(3);

/// The least time between two refusal events of one budget. A saturated budget refuses requests
/// in bursts; one `ERROR` per interval, counting the refusals it covers, keeps the log readable
/// and its volume bounded however long the saturation lasts.
pub const REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// The budget every argon2id computation of the API runs under: sign-on's verification
/// (COSGN00C), the user add's hash (COUSR01C) and the user update's change test (COUSR02C). See
/// "Password work" in the module documentation for why it exists.
///
/// At most `running` computations run at once, each on a thread of Tokio's blocking pool, and at
/// most `queued` more are admitted to wait for one. A request waits for an admission place and
/// then for its worker, each in arrival order, for at most the budget's wait in all,
/// [`PASSWORD_WAIT`] unless [`with_wait`](Self::with_wait) sets another, and is refused when the
/// wait ends first (`PasswordWorkError::Saturated`). A computation keeps its places until it
/// returns, so a request that is dropped while its computation runs frees nothing early; one
/// dropped while it waits gives back the place it holds.
///
/// Cloning is cheap and shares the budget: each limit is one `Arc`ed semaphore, and the refusal
/// log is shared as well.
#[derive(Clone)]
pub struct PasswordWork {
    /// One permit per computation that may run at once.
    running: Arc<Semaphore>,
    /// One permit per computation admitted, running or waiting for a `running` permit.
    admitted: Arc<Semaphore>,
    /// The refusals not yet logged, and when the next refusal event may be emitted.
    refusals: Arc<RefusalLog>,
    /// The `running` limit, as `Debug` and the saturation event show it.
    workers: usize,
    /// The waiting places beyond the workers, as `Debug` and the saturation event show them.
    queue: usize,
    /// How long a request waits for its places before it is refused.
    wait: Duration,
}

impl PasswordWork {
    /// A budget of `running` computations at once and `queued` more admitted to wait for a
    /// worker, whose requests wait at most [`PASSWORD_WAIT`] for their places.
    ///
    /// `running` is raised to 1 when it is 0, because a budget that runs nothing would keep
    /// every admitted request waiting until it is refused. `queued` may be 0: every request that
    /// finds all workers busy then waits for an admission place. Both are capped so that their
    /// sum fits one Tokio semaphore ([`Semaphore::MAX_PERMITS`]).
    #[must_use]
    pub fn new(running: usize, queued: usize) -> Self {
        let workers = running.clamp(1, Semaphore::MAX_PERMITS);
        let admitted = workers.saturating_add(queued).min(Semaphore::MAX_PERMITS);
        Self {
            running: Arc::new(Semaphore::new(workers)),
            admitted: Arc::new(Semaphore::new(admitted)),
            refusals: Arc::new(RefusalLog::new()),
            workers,
            queue: admitted.saturating_sub(workers),
            wait: PASSWORD_WAIT,
        }
    }

    /// The same budget, sharing its places and its refusal log, whose requests wait at most
    /// `wait` for their places: for a caller that sizes the wait itself, such as a test that
    /// must see a refusal without waiting [`PASSWORD_WAIT`], or one that must never see one.
    #[must_use]
    pub fn with_wait(self, wait: Duration) -> Self {
        Self { wait, ..self }
    }

    /// Runs `work`, an argon2id computation, on Tokio's blocking pool within the budget, and
    /// returns its result.
    ///
    /// The request waits for an admission place, then for a worker, each in arrival order, both
    /// within the budget's wait from this call. Both places move into the blocking task and are
    /// given back only when `work` returns or unwinds, so dropping the returned future frees a
    /// place only while `work` has not started. A request still waiting when the wait ends is
    /// refused, since its request is then answered with a 503, and the refusal is logged here as
    /// an `ERROR` at most once per [`REFUSAL_LOG_INTERVAL`], with the number of refusals the event
    /// covers and the budget's limits, and nothing of the request.
    ///
    /// # Errors
    ///
    /// - [`PasswordWorkError::Saturated`] when the wait ends before both places are taken;
    ///   `work` never runs.
    /// - [`PasswordWorkError::Interrupted`] when the blocking task does not complete: `work`
    ///   panicked, or the runtime shut down first. Only whether it panicked is kept.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PasswordWorkError> {
        // One wait bounds both places: when it ends while the request waits for its worker, the
        // admission place it took goes back as the inner future is dropped.
        let places = tokio::time::timeout(self.wait, async {
            let admitted = Arc::clone(&self.admitted).acquire_owned().await.ok()?;
            let running = Arc::clone(&self.running).acquire_owned().await.ok()?;
            Some((admitted, running))
        })
        .await;
        // Neither semaphore is ever closed, so only the end of the wait refuses in practice; a
        // closed one would admit nothing, which is what a refusal answers.
        let Ok(Some((admitted, running))) = places else {
            return Err(self.refused());
        };
        tokio::task::spawn_blocking(move || {
            let output = work();
            drop((admitted, running));
            output
        })
        .await
        .map_err(|error| PasswordWorkError::Interrupted {
            panicked: error.is_panic(),
        })
    }

    /// Counts a refusal and returns it. The refusal is logged as the one event of a saturated
    /// budget, an `ERROR`, when no refusal event of the budget is younger than
    /// [`REFUSAL_LOG_INTERVAL`]; the event's `refused` counts every refusal since the previous
    /// event, this one included. It names the budget's limits besides, and never a user ID, a
    /// password or a hash.
    fn refused(&self) -> PasswordWorkError {
        static SITE: EventSite<4> = EventSite::new(&METADATA);
        static METADATA: Metadata<'static> = event_metadata(
            "password_work_refused",
            module_path!(),
            Level::ERROR,
            &["message", "running", "queued", "refused"],
            &SITE,
        );
        if let Some(refused) = self.refusals.refuse(Instant::now()) {
            emit(
                &SITE,
                [
                    Some(&"password work budget saturated; refusing the request"),
                    Some(&self.workers),
                    Some(&self.queue),
                    Some(&refused),
                ],
            );
        }
        PasswordWorkError::Saturated
    }
}

/// The budget of the `carddemo-api` process: as many workers as the host's available
/// parallelism, at least 1, and 1 when it cannot be determined, but never more than
/// [`MAX_PASSWORD_WORKERS`]; [`PASSWORD_QUEUE`] waiting places; and the wait [`PASSWORD_WAIT`].
impl Default for PasswordWork {
    fn default() -> Self {
        let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        Self::new(parallelism.min(MAX_PASSWORD_WORKERS), PASSWORD_QUEUE)
    }
}

/// Shows the two limits and the wait, and leaves out the semaphores and the refusal log, whose
/// momentary state is not a setting.
impl fmt::Debug for PasswordWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordWork")
            .field("running", &self.workers)
            .field("queued", &self.queue)
            .field("wait", &self.wait)
            .finish_non_exhaustive()
    }
}

/// The refusals of one [`PasswordWork`] budget that no event has counted yet, and when the next
/// refusal event may be emitted: the rate limit of [`PasswordWork::refused`]. It holds two
/// atomics and the instant it was made, so it takes no lock.
struct RefusalLog {
    /// When the log was made, the origin of `next_event_ms`.
    origin: Instant,
    /// The milliseconds after `origin` from which a refusal is logged again: 0 at first, so the
    /// budget's first refusal is logged at once.
    next_event_ms: AtomicU64,
    /// The refusals no event has counted yet.
    unlogged: AtomicU64,
}

impl RefusalLog {
    /// A log that has counted no refusal and logs the next one.
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            next_event_ms: AtomicU64::new(0),
            unlogged: AtomicU64::new(0),
        }
    }

    /// Counts one refusal made at `now`, and answers whether to log an event for it: `Some` with
    /// the number of refusals the event covers, every one since the previous event, this one
    /// included, when that event is at least [`REFUSAL_LOG_INTERVAL`] old or there is none; and
    /// `None` otherwise, the refusal then being counted by the next event.
    ///
    /// Of refusals that find an event due at the same moment, one logs it; no refusal is counted
    /// twice or lost, since every count is added before the swap that takes it.
    fn refuse(&self, now: Instant) -> Option<u64> {
        self.unlogged.fetch_add(1, Ordering::Relaxed);
        let at = whole_millis(now.saturating_duration_since(self.origin));
        let due = self.next_event_ms.load(Ordering::Relaxed);
        if at < due {
            return None;
        }
        let next = at.saturating_add(whole_millis(REFUSAL_LOG_INTERVAL));
        self.next_event_ms
            .compare_exchange(due, next, Ordering::Relaxed, Ordering::Relaxed)
            .ok()?;
        Some(self.unlogged.swap(0, Ordering::Relaxed))
    }
}

/// `duration` in whole milliseconds.
fn whole_millis(duration: Duration) -> u64 {
    // TRUNCATION: checked; a duration beyond `u64::MAX` milliseconds, some 584 million years,
    // which no budget lives, saturates at `u64::MAX`.
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Why [`PasswordWork::run`] returned no result. Neither variant carries a value of the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasswordWorkError {
    /// No admission place and worker came free within the budget's wait, so the computation was
    /// refused and never ran.
    Saturated,
    /// The computation's blocking task did not complete. Only whether it panicked is kept, never
    /// the panic's payload, which can carry any value the computation held.
    Interrupted {
        /// Whether the computation panicked, rather than being cancelled by a shutting-down
        /// runtime.
        panicked: bool,
    },
}

/// Test support for the services that run password work: a computation that holds one worker
/// of a [`PasswordWork`] until the test lets it go, so that a test can fill a budget, and a wait
/// for a budget whose every place is taken.
#[cfg(test)]
pub(crate) mod worker_hold {
    use std::error::Error as StdError;
    use std::sync::mpsc;
    use std::time::Duration;

    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    use super::{PasswordWork, PasswordWorkError};

    /// A computation that holds a worker and a place of its budget until
    /// [`release`](Self::release), or until it is dropped.
    pub(crate) struct WorkerHold {
        /// Lets the computation return.
        release: mpsc::Sender<()>,
        /// The request that runs it: `Ok(true)` when the computation was released by a send.
        task: JoinHandle<Result<bool, PasswordWorkError>>,
    }

    impl WorkerHold {
        /// Starts the computation under `work` and returns once it is running.
        pub(crate) async fn start(work: &PasswordWork) -> Result<Self, Box<dyn StdError>> {
            let (started, has_started) = oneshot::channel();
            let (release, released) = mpsc::channel();
            let work = work.clone();
            let task = tokio::spawn(async move {
                work.run(move || started.send(()).is_ok() && released.recv().is_ok())
                    .await
            });
            has_started.await?;
            Ok(Self { release, task })
        }

        /// Lets the computation return, and answers what its request received.
        pub(crate) async fn release(self) -> Result<bool, Box<dyn StdError>> {
            self.release.send(())?;
            self.task
                .await?
                .map_err(|error| format!("the held computation failed: {error:?}").into())
        }
    }

    /// Whether every admission place of `work` is taken, by computations that run and by
    /// requests that wait for a worker, so that a request asking now would wait for a place.
    ///
    /// It reads the free places without taking one, so it never keeps a place from the request
    /// the caller waits for.
    pub(crate) fn is_saturated(work: &PasswordWork) -> bool {
        work.admitted.available_permits() == 0
    }

    /// Returns once [`is_saturated`] holds for `work`.
    pub(crate) async fn until_saturated(work: &PasswordWork) {
        while !is_saturated(work) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

/// The keys the `SignedOnUser` and `AdminUser` extractors verify a bearer token with.
impl FromRef<AppState> for JwtKeys {
    fn from_ref(state: &AppState) -> Self {
        state.jwt.clone()
    }
}

/// The router of the 27 operations, still awaiting its state, and the `OpenAPI` document they
/// make: the base document of [`ApiDoc`] with one path item per route and every schema the
/// handlers declare.
///
/// Each handler is registered by one `operation` call under the path and methods its
/// `#[utoipa::path]` declares, so 27 calls make the 27 operations. The handlers of one path are
/// registered one after the other: `OpenApiRouter::routes` merges their operations into the
/// path's one path item, and axum merges their method routers into the path's one method router,
/// which makes 22 paths. No route is added outside `operation` before the split, so the router
/// serves exactly the document's operations. The static segments `/cards/view`, `/cards/edit` and
/// `/transaction-types/list-actions` take precedence over the `{param}` segment beside them.
#[must_use = "the router serves nothing and the document is emitted nowhere until both are used"]
pub fn router() -> (Router<AppState>, OpenApi) {
    OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        // 1: CC00 / COSGN00C.
        .routes(operation(cosgn00c::SignonOperation, cosgn00c::signon))
        // 2: CM00 / COMEN01C.
        .routes(operation(comen01c::MainMenuOperation, comen01c::main_menu))
        // 3: CA00 / COADM01C.
        .routes(operation(
            coadm01c::AdminMenuOperation,
            coadm01c::admin_menu,
        ))
        // 4: CAVW / COACTVWC.
        .routes(operation(coactvwc::ViewOperation, coactvwc::view))
        // 5 and 6: CAUP / COACTUPC, fetch and save, on one path.
        .routes(operation(coactupc::FetchOperation, coactupc::fetch))
        .routes(operation(coactupc::SaveOperation, coactupc::save))
        // 7: CCLI / COCRDLIC.
        .routes(operation(cocrdlic::ListOperation, cocrdlic::list))
        // 8: CCDL / COCRDSLC.
        .routes(operation(cocrdslc::ViewOperation, cocrdslc::view))
        // 9 and 10: CCUP / COCRDUPC, fetch and save, on two paths.
        .routes(operation(cocrdupc::FetchOperation, cocrdupc::fetch))
        .routes(operation(cocrdupc::SaveOperation, cocrdupc::save))
        // 11 and 13: CT00 / COTRN00C's list and CT02 / COTRN02C's add share a path.
        .routes(operation(cotrn00c::ListOperation, cotrn00c::list))
        .routes(operation(cotrn02c::AddOperation, cotrn02c::add))
        // 12: CT01 / COTRN01C.
        .routes(operation(cotrn01c::ViewOperation, cotrn01c::view))
        // 14: CR00 / CORPT00C.
        .routes(operation(corpt00c::SubmitOperation, corpt00c::submit))
        // 15: CB00 / COBIL00C.
        .routes(operation(cobil00c::PayOperation, cobil00c::pay))
        // 16 and 17: CU00 / COUSR00C's list and CU01 / COUSR01C's add share a path.
        .routes(operation(cousr00c::ListOperation, cousr00c::list))
        .routes(operation(cousr01c::AddOperation, cousr01c::add))
        // 18 to 20: the user read COUSR02C and COUSR03C share, COUSR02C's update and COUSR03C's
        // delete, on one path.
        .routes(operation(cousr02c::FetchOperation, cousr02c::fetch))
        .routes(operation(cousr02c::UpdateOperation, cousr02c::update))
        .routes(operation(cousr03c::DeleteOperation, cousr03c::delete))
        // 21: CPVS / COPAUS0C.
        .routes(operation(copaus0c::SummaryOperation, copaus0c::summary))
        // 22 and 23: CPVD / COPAUS1C, the detail and the fraud toggle COPAUS2C performs.
        .routes(operation(copaus1c::DetailOperation, copaus1c::detail))
        .routes(operation(
            copaus1c::ToggleFraudOperation,
            copaus1c::toggle_fraud,
        ))
        // 24 and 25: CTLI / COTRTLIC, paging and the ENTER and F10 workflow.
        .routes(operation(cotrtlic::ListOperation, cotrtlic::list))
        .routes(operation(
            cotrtlic::ListActionsOperation,
            cotrtlic::list_actions,
        ))
        // 26 and 27: CTTU / COTRTUPC, fetch and the save and delete actions.
        .routes(operation(cotrtupc::FetchOperation, cotrtupc::fetch))
        .routes(operation(cotrtupc::ActionOperation, cotrtupc::action))
        .split_for_parts()
}

/// One operation of [`router`]: `handler`, served at the path and methods that its
/// `#[utoipa::path(impl_for = O)]` declares, with the operation and every schema the operation
/// references, ready for `OpenApiRouter::routes`. `O` is passed as a value only to name the
/// operation's type at the call site, and `handler` is a handler function, which is `Copy`.
///
/// This builds what `utoipa_axum::routes!` builds for one handler, from the same metadata: the
/// `utoipa::Path`, `utoipa::__dev::Tags` and `utoipa::__dev::SchemaReferences` implementations
/// that `#[utoipa::path]` generates on `O` and that `routes!` reads, the declared tags appended
/// to the operation's own as `routes!` appends them, and one `MethodRouter::on` per declared
/// method. The macro itself is not used, because its expansion carries lint-suppression
/// attributes and R15 admits none in generated source; a function needs none (escalation E46).
pub(crate) fn operation<O, H, T>(_: O, handler: H) -> UtoipaMethodRouter<AppState>
where
    O: Path + SchemaReferences + Tags<'static>,
    H: Handler<T, AppState> + Copy,
    T: 'static,
{
    let mut schemas = Vec::new();
    O::schemas(&mut schemas);
    let mut documented = O::operation();
    let tags = O::tags();
    if !tags.is_empty() {
        documented
            .tags
            .get_or_insert_with(Vec::new)
            .extend(tags.into_iter().map(String::from));
    }
    let methods = O::methods();
    let method_router = methods.iter().fold(MethodRouter::new(), |router, method| {
        router.on(method.to_method_filter(), handler)
    });
    let mut paths = Paths::new();
    paths.add_path_operation(O::path(), methods, documented);
    (schemas, paths, method_router)
}

/// The `OpenAPI` document of the 27 operations, as `carddemo-openapi` prints it and
/// [`DOCUMENT_ROUTE`] serves it. Building it opens no connection and reads no environment.
#[must_use]
pub fn openapi() -> OpenApi {
    router().1
}

/// The complete API: the operations of [`router`], the document at [`DOCUMENT_ROUTE`], the
/// router-level 404 and 405, one `http.request` span per request, the two response headers, the
/// limit of [`MAX_IN_FLIGHT`] requests at once (see the module documentation) and `state`.
#[must_use = "the API answers no request until the router is served"]
pub fn app(state: AppState) -> Router {
    let (router, document) = router();
    let document = SharedDocument(Arc::new(document));
    let router = router
        .route(DOCUMENT_ROUTE, get(move || async move { Json(document) }))
        // After the last route, so that it reaches every method router, and before any layer,
        // since it rewrites only method routers that no layer has wrapped yet.
        .method_not_allowed_fallback(method_not_served)
        .fallback(no_such_route);
    layered(router, MAX_IN_FLIGHT, ADMISSION_WAIT).with_state(state)
}

/// The most requests [`app`] works on at once; a request beyond them waits up to
/// [`ADMISSION_WAIT`] for a place, and is answered 503 with [`SERVER_BUSY`] only when none frees
/// in that time (see "In-flight limit" in the module documentation).
///
/// Far more than the API can serve concurrently anyway: every operation that reads the store
/// waits for one of the request pool's 10 connections, and every password computation for one
/// of the [`MAX_PASSWORD_WORKERS`] workers of [`PasswordWork::default`], so an ordinary load
/// never meets it.
/// And few enough that what the admitted requests hold stays bounded: each buffers at most
/// axum's default body limit of 2 MiB, so their bodies hold at most 512 MiB together.
pub const MAX_IN_FLIGHT: usize = 256;

/// How long a request beyond [`MAX_IN_FLIGHT`] waits for a place before it is answered 503 with
/// [`SERVER_BUSY`] and `Retry-After: 1`.
///
/// The same 5 seconds a request waits inside the store for one of the request pool's
/// connections (`runtime::db::ACQUIRE_TIMEOUT`). Under the 8-second drain deadline of
/// `carddemo-api`, so a request still waiting at a stop signal is admitted or refused before the
/// deadline passes. And far under the frontend's 30-second request deadline, so the client
/// receives the 503 long before it would give up waiting.
pub const ADMISSION_WAIT: Duration = Duration::from_secs(5);

/// `router`, its routes and its fallbacks alike, behind the layers every request passes, from
/// the outermost: the request span, the two response headers and the limit of `in_flight`
/// requests at once, raised to 1 when it is 0 and capped at [`Semaphore::MAX_PERMITS`], for
/// which a request beyond it waits at most `admission_wait`.
///
/// The limit is innermost, so a refused request still has its span, recording its route and its
/// 503, and still gets both headers. One semaphore serves every route and every connection of
/// the returned router.
fn layered<S>(router: Router<S>, in_flight: usize, admission_wait: Duration) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let admission = Admission {
        permits: Arc::new(Semaphore::new(in_flight.clamp(1, Semaphore::MAX_PERMITS))),
        wait: admission_wait,
    };
    router
        .layer(middleware::from_fn_with_state(admission, admit))
        .layer(middleware::map_response(no_store_nosniff))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(RouteSpan)
                .on_response(ResponseFields),
        )
}

/// What [`admit`] reads: the permits of the in-flight limit and how long a request waits for one.
#[derive(Clone)]
struct Admission {
    /// One permit per request in flight. The semaphore is fair, so waiting requests are admitted
    /// in the order they arrived.
    permits: Arc<Semaphore>,
    /// How long a request waits for a permit before it is answered 503.
    wait: Duration,
}

/// The `Retry-After` value of the in-flight limit's 503, in seconds. A place frees whenever a
/// request in flight is answered, so a client refused after the whole admission wait needs to
/// stay away only briefly before it sends the request again.
const RETRY_IN_ONE_SECOND: HeaderValue = HeaderValue::from_static("1");

/// Runs `request` once it holds one of the limit's permits, before any handler or extractor
/// reads it. A request that finds none free waits for one, in arrival order, for at most the
/// admission's `wait`; when the wait passes, or the semaphore is closed, it is answered 503 with
/// [`SERVER_BUSY`] and `Retry-After: 1` instead. The permit is held until the inner service
/// returns its response, and given back when this future ends or is dropped. A client that
/// disconnects while waiting drops the wait, and its turn with it.
async fn admit(
    State(admission): State<Admission>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // The semaphore is never closed, so only a wait that passes refuses in practice; a closed one
    // admits nothing, at once, which is what the refusal answers.
    let acquired = tokio::time::timeout(admission.wait, admission.permits.acquire()).await;
    let Ok(Ok(permit)) = acquired else {
        let mut refused = ScreenError::service_unavailable(SERVER_BUSY).into_response();
        refused
            .headers_mut()
            .insert(header::RETRY_AFTER, RETRY_IN_ONE_SECOND);
        return refused;
    };
    let response = next.run(request).await;
    drop(permit);
    response
}

/// The `OpenAPI` document as [`DOCUMENT_ROUTE`] serves it: one copy for the whole process, shared
/// by reference count.
///
/// axum clones every route's service for each connection it accepts, and a handler for each
/// request it serves, so a route that owned the document would copy its whole tree each time,
/// about 1.5 MB: a thousand keep-alive connections would hold about 1.5 GB for nothing. Cloning
/// this clones an `Arc`. It serialises exactly as the document does, so the served bytes are
/// unchanged.
#[derive(Clone)]
struct SharedDocument(Arc<OpenApi>);

/// The document's own serialisation, unchanged.
impl Serialize for SharedDocument {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// The `Cache-Control` value of every response: no cache, the browser's included, may keep it.
const NO_STORE: HeaderValue = HeaderValue::from_static("no-store");

/// The `X-Content-Type-Options` value of every response: the browser takes the declared content
/// type as it is and never sniffs another.
const NOSNIFF: HeaderValue = HeaderValue::from_static("nosniff");

/// Adds `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` to `response`, replacing
/// any value a handler set. Any response can carry a bearer token (sign-on), cardholder data or
/// personal data, so none may be stored by a cache, and every body is JSON that must never be
/// read as anything else.
async fn no_store_nosniff<B>(mut response: Response<B>) -> Response<B> {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, NO_STORE);
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, NOSNIFF);
    response
}

/// The answer to a request whose path no route matches: 404, [`NO_SUCH_ROUTE`] and no field. It
/// reads nothing of the request, so the body cannot echo its path or query.
async fn no_such_route() -> ScreenError {
    ScreenError::not_found(NO_SUCH_ROUTE, None)
}

/// The answer to a method that the route matching the path does not serve: 405,
/// [`METHOD_NOT_SERVED`] and no field. It sets no `Allow` header, so axum adds the one listing
/// the route's methods.
async fn method_not_served() -> ScreenError {
    ScreenError::method_not_allowed(METHOD_NOT_SERVED)
}

/// Makes the `http.request` span of a request from its method and its `MatchedPath` route
/// template alone. tower-http's `DefaultMakeSpan` is not used, because it records the URI.
#[derive(Debug, Clone, Copy, Default)]
struct RouteSpan;

impl<B> MakeSpan<B> for RouteSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or(UNMATCHED_ROUTE, MatchedPath::as_str);
        request_span(request.method().as_str(), route)
    }
}

/// Records the status and the latency of a response on its request's span, and nothing else: no
/// header and no body. It emits no event: when the span closes, the subscriber of
/// `runtime::telemetry` writes the request's one `INFO` line on standard output, carrying the
/// method, the route template and these two fields.
#[derive(Debug, Clone, Copy, Default)]
struct ResponseFields;

impl<B> OnResponse<B> for ResponseFields {
    fn on_response(self, response: &Response<B>, latency: Duration, span: &Span) {
        record_response(span, response.status().as_u16(), latency);
    }
}

/// The callsite of one event of this crate written without a `tracing` macro, whose metadata
/// names `N` fields, `message` first.
///
/// Every `tracing` event macro expands to an attribute that allows `unused_imports`, and no
/// source of this workspace, generated source included, holds a lint suppression. Such an event
/// is therefore written out as two statics, an `EventSite` and the [`event_metadata`] it holds,
/// and emitted with [`emit`]. All three take the same `N`, so an event always carries exactly
/// one value, or `None`, per field it declares.
pub(crate) struct EventSite<const N: usize>(DefaultCallsite);

impl<const N: usize> EventSite<N> {
    /// The callsite of the event that `metadata` describes.
    pub(crate) const fn new(metadata: &'static Metadata<'static>) -> Self {
        Self(DefaultCallsite::new(metadata))
    }
}

/// The metadata of the event `name` at `level`, with the field `names`, for the callsite `site`
/// in the module `module_path`.
///
/// The target and the module path are both `module_path`, which a caller passes as
/// `module_path!()`: the target a `tracing` macro gives an event, so `RUST_LOG` directives select
/// the event exactly as they would select a macro's. No source file or line is given: the JSON
/// output and the span export print neither.
pub(crate) const fn event_metadata<const N: usize>(
    name: &'static str,
    module_path: &'static str,
    level: Level,
    names: &'static [&'static str; N],
    site: &'static EventSite<N>,
) -> Metadata<'static> {
    Metadata::new(
        name,
        module_path,
        level,
        None,
        None,
        Some(module_path),
        FieldSet::new(names, Identifier(&site.0)),
        Kind::EVENT,
    )
}

/// Emits one event of `site` in the current span with `values`, one per field the site names
/// and in that order, a `None` leaving its field out, when the current subscriber enables it.
///
/// The decision is `runtime::telemetry::event_enabled`, which takes the steps an event macro
/// takes, so the level filters, the JSON layer and the span export treat the event exactly as
/// they treat a macro's.
pub(crate) fn emit<const N: usize>(site: &'static EventSite<N>, values: [Option<&dyn Value>; N]) {
    let callsite = &site.0;
    if !event_enabled(callsite) {
        return;
    }
    let metadata = callsite.metadata();
    Event::dispatch(metadata, &metadata.fields().value_set_all(&values));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::pin::pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    use axum::body::{self, Body};
    use axum::extract::FromRef;
    use axum::http::{HeaderMap, Method, Request, StatusCode, header};
    use axum::{Router, middleware, routing};
    use chrono::NaiveDate;
    use domain::clock::{Clock, FixedClock};
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use persistence::PgStore;
    use runtime::config::{Config, EnvError};
    use runtime::telemetry::UNMATCHED_ROUTE;
    use serde_json::{Value, json};
    use test_support::postgres::{PostgresGuard, start_postgres};
    use test_support::trace_capture;
    use tokio::sync::{Notify, Semaphore, oneshot};
    use tower::ServiceExt as _;

    use super::worker_hold::WorkerHold;
    use super::{
        Admission, AppState, DOCUMENT_ROUTE, MAX_PASSWORD_WORKERS, PASSWORD_QUEUE, PASSWORD_WAIT,
        PasswordWork, PasswordWorkError, REFUSAL_LOG_INTERVAL, RefusalLog, admit, app, layered,
        openapi,
    };
    use crate::auth::jwt::JwtKeys;
    use crate::error::{METHOD_NOT_SERVED, NO_SUCH_ROUTE, SERVER_BUSY};

    type TestResult = Result<(), Box<dyn StdError>>;

    const SECRET: &str = "app-unit-test-secret";

    /// The 27 operations of AAP 0.3.4.1 in endpoint order: method, path and the operation ID
    /// `<module>_<handler>` of the handler registered for them.
    const OPERATIONS: [(&str, &str, &str); 27] = [
        ("post", "/api/v1/signon", "cosgn00c_signon"),
        ("get", "/api/v1/menus/main", "comen01c_main_menu"),
        ("get", "/api/v1/menus/admin", "coadm01c_admin_menu"),
        ("get", "/api/v1/accounts/{acct_id}/view", "coactvwc_view"),
        ("get", "/api/v1/accounts/{acct_id}", "coactupc_fetch"),
        ("put", "/api/v1/accounts/{acct_id}", "coactupc_save"),
        ("get", "/api/v1/cards", "cocrdlic_list"),
        ("get", "/api/v1/cards/view", "cocrdslc_view"),
        ("get", "/api/v1/cards/edit", "cocrdupc_fetch"),
        ("put", "/api/v1/cards/{card_ref}", "cocrdupc_save"),
        ("get", "/api/v1/transactions", "cotrn00c_list"),
        ("get", "/api/v1/transactions/{tran_id}", "cotrn01c_view"),
        ("post", "/api/v1/transactions", "cotrn02c_add"),
        ("post", "/api/v1/reports/transactions", "corpt00c_submit"),
        ("post", "/api/v1/bill-payments", "cobil00c_pay"),
        ("get", "/api/v1/users", "cousr00c_list"),
        ("post", "/api/v1/users", "cousr01c_add"),
        ("get", "/api/v1/users/{user_id}", "cousr02c_fetch"),
        ("put", "/api/v1/users/{user_id}", "cousr02c_update"),
        ("delete", "/api/v1/users/{user_id}", "cousr03c_delete"),
        (
            "get",
            "/api/v1/accounts/{acct_id}/authorizations",
            "copaus0c_summary",
        ),
        (
            "get",
            "/api/v1/authorizations/{acct_id}/{auth_key}",
            "copaus1c_detail",
        ),
        (
            "post",
            "/api/v1/authorizations/{acct_id}/{auth_key}/fraud",
            "copaus1c_toggle_fraud",
        ),
        ("get", "/api/v1/transaction-types", "cotrtlic_list"),
        (
            "post",
            "/api/v1/transaction-types/list-actions",
            "cotrtlic_list_actions",
        ),
        (
            "get",
            "/api/v1/transaction-types/{type_code}",
            "cotrtupc_fetch",
        ),
        (
            "post",
            "/api/v1/transaction-types/{type_code}/actions",
            "cotrtupc_action",
        ),
    ];

    /// The keys of a path item that name an operation.
    const METHODS: [&str; 8] = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    /// Every operation of the document: method, path, operation ID and whether it declares a
    /// security requirement.
    fn operations(doc: &Value) -> Result<Vec<(String, String, String, bool)>, String> {
        let paths = doc
            .get("paths")
            .and_then(Value::as_object)
            .ok_or("the document has no paths object")?;
        let mut found = Vec::new();
        for (path, item) in paths {
            let item = item
                .as_object()
                .ok_or_else(|| format!("{path} is not an object"))?;
            for (method, operation) in item {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                let id = operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{method} {path} has no operation ID"))?;
                found.push((
                    method.clone(),
                    path.clone(),
                    id.to_owned(),
                    operation.get("security").is_some(),
                ));
            }
        }
        Ok(found)
    }

    #[test]
    fn the_document_holds_exactly_the_27_operations_on_22_paths() -> TestResult {
        let doc = openapi();
        assert_eq!(doc.paths.paths.len(), 22);
        assert!(!doc.paths.paths.contains_key(DOCUMENT_ROUTE));

        let found = operations(&serde_json::to_value(&doc)?)?;
        assert_eq!(found.len(), 27);
        let found_set: BTreeSet<(&str, &str, &str)> = found
            .iter()
            .map(|(method, path, id, _)| (method.as_str(), path.as_str(), id.as_str()))
            .collect();
        let expected: BTreeSet<(&str, &str, &str)> = OPERATIONS.into_iter().collect();
        assert_eq!(found_set, expected);

        // Sign-on issues the token, so it alone declares no bearer requirement.
        for (method, path, _, secured) in &found {
            assert_eq!(*secured, path != "/api/v1/signon", "{method} {path}");
        }
        Ok(())
    }

    #[test]
    fn the_document_is_the_same_bytes_on_every_build() -> TestResult {
        let first = openapi().to_pretty_json()?;
        assert_eq!(first, openapi().to_pretty_json()?);
        assert_eq!(
            serde_json::from_str::<Value>(&first)?.pointer("/info/title"),
            Some(&Value::from("CardDemo API"))
        );
        Ok(())
    }

    /// A state over a closed pool, so that a request that reached the store would fail; every
    /// request below is answered before any query.
    async fn closed_store() -> Result<(PgStore, PostgresGuard), Box<dyn StdError>> {
        let guard = start_postgres().await?;
        guard.pool().close().await;
        Ok((PgStore::new(guard.pool().clone()), guard))
    }

    /// The fixed instant of the test clock.
    fn clock() -> Result<Arc<dyn Clock>, Box<dyn StdError>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_opt(10, 30, 0))
            .ok_or("an invalid test instant")?;
        Ok(Arc::new(FixedClock::new(at)))
    }

    /// A bearer token for `USER0001`, a user of type `U`, signed with `keys`.
    fn user_token(keys: &JwtKeys) -> Result<String, Box<dyn StdError>> {
        Ok(keys.issue(&PicX::from_text("USER0001")?, &UserType::USER)?)
    }

    /// The status of `request` sent to `app(state)`, with the body read to its end so that the
    /// request's span closes.
    async fn send(
        state: &AppState,
        request: Request<Body>,
    ) -> Result<(StatusCode, Vec<u8>), Box<dyn StdError>> {
        let (status, _, bytes) = exchange(app(state.clone()), request).await?;
        Ok((status, bytes))
    }

    /// The status, the headers and the whole body of `router`'s response to `request`; the body
    /// is read to its end, so the request's span closes.
    async fn exchange(
        router: Router,
        request: Request<Body>,
    ) -> Result<(StatusCode, HeaderMap, Vec<u8>), Box<dyn StdError>> {
        let response = router.oneshot(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, headers, bytes.to_vec()))
    }

    /// The value of the header `name` in `headers`, when it is present and visible ASCII.
    fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
        headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// `headers`, those of the response to `case`, forbid caching and content sniffing, each
    /// with exactly one value.
    fn assert_protected(headers: &HeaderMap, case: &str) {
        for (name, value) in [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ] {
            let values: Vec<_> = headers.get_all(&name).iter().collect();
            assert_eq!(values, [value], "{case}: {name}");
        }
    }

    /// A `GET` of `uri`, with `token` as bearer token when one is given.
    fn get(uri: &str, token: Option<&str>) -> Result<Request<Body>, axum::http::Error> {
        let builder = Request::get(uri);
        let builder = match token {
            Some(token) => builder.header(header::AUTHORIZATION, format!("Bearer {token}")),
            None => builder,
        };
        builder.body(Body::empty())
    }

    /// How long a test budget lets a request wait when the test expects it refused.
    const SHORT_WAIT: Duration = Duration::from_millis(100);

    /// How long a test budget lets a request wait when the test expects it admitted: longer than
    /// any test runs.
    const LONG_WAIT: Duration = Duration::from_secs(60);

    /// How long a test keeps a computation's places taken before it releases them.
    const HELD_FOR: Duration = Duration::from_millis(50);

    #[test]
    fn password_budgets_keep_valid_limits() {
        let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let host_workers = parallelism.min(MAX_PASSWORD_WORKERS);
        assert!((1..=MAX_PASSWORD_WORKERS).contains(&host_workers));
        let cases = [
            // A budget that ran nothing would keep every request waiting: one worker at least.
            (PasswordWork::new(0, 0), 1, 0, PASSWORD_WAIT),
            (PasswordWork::new(2, 3), 2, 3, PASSWORD_WAIT),
            // Capped so that every place fits one semaphore.
            (
                PasswordWork::new(usize::MAX, usize::MAX),
                Semaphore::MAX_PERMITS,
                0,
                PASSWORD_WAIT,
            ),
            (
                PasswordWork::default(),
                host_workers,
                PASSWORD_QUEUE,
                PASSWORD_WAIT,
            ),
            // The wait is the caller's, and the limits stay.
            (
                PasswordWork::new(2, 3).with_wait(SHORT_WAIT),
                2,
                3,
                SHORT_WAIT,
            ),
        ];
        for (work, running, queued, wait) in cases {
            assert_eq!(
                format!("{work:?}"),
                format!(
                    "PasswordWork {{ running: {running}, queued: {queued}, wait: {wait:?}, .. }}"
                )
            );
        }
    }

    /// A request that finds every place taken waits: for an admission place when the budget
    /// has no waiting place, and for the worker when it has one. It runs once the held
    /// computation returns within its wait, and nothing is logged.
    #[test]
    fn a_request_runs_once_its_places_free_within_its_wait() -> TestResult {
        block_on(async {
            for queued in [0, 1] {
                let work = PasswordWork::new(1, queued).with_wait(LONG_WAIT);
                let hold = WorkerHold::start(&work).await?;
                let traces = trace_capture::capture();
                let started = Instant::now();
                let waiting = tokio::spawn({
                    let work = work.clone();
                    async move { work.run(|| 7).await }
                });
                tokio::time::sleep(HELD_FOR).await;
                assert!(!waiting.is_finished(), "queued {queued}: it did not wait");

                assert!(hold.release().await?);
                assert_eq!(waiting.await?, Ok(7), "queued {queued}");
                assert!(started.elapsed() >= HELD_FOR, "queued {queued}");
                assert!(
                    traces.lines().is_empty(),
                    "queued {queued}: {}",
                    traces.text()
                );
            }
            Ok(())
        })
    }

    /// Requests still waiting when their wait ends are refused then, never at once, whether
    /// they wait for an admission place or for the worker. Three refusals log one `ERROR` with
    /// the budget's limits and its count, and nothing of a request; a refusal gives back the
    /// place it held, so the next request runs once the held computation returns.
    #[test]
    fn requests_still_waiting_when_their_wait_ends_are_refused() -> TestResult {
        block_on(async {
            for queued in [0, 1] {
                let work = PasswordWork::new(1, queued).with_wait(SHORT_WAIT);
                let hold = WorkerHold::start(&work).await?;
                let traces = trace_capture::capture();
                let started = Instant::now();
                let requests: Vec<_> = (0..3)
                    .map(|_| {
                        let work = work.clone();
                        tokio::spawn(async move { work.run(|| 1).await })
                    })
                    .collect();
                for request in requests {
                    assert_eq!(
                        request.await?,
                        Err(PasswordWorkError::Saturated),
                        "queued {queued}"
                    );
                }
                let waited = started.elapsed();
                assert!(
                    (SHORT_WAIT..LONG_WAIT).contains(&waited),
                    "queued {queued}: refused after {waited:?}"
                );

                let lines = traces.lines();
                let [line] = lines.as_slice() else {
                    return Err(format!("queued {queued}: {} lines", lines.len()).into());
                };
                let event: Value = serde_json::from_str(line)?;
                assert_eq!(event.get("level"), Some(&json!("ERROR")), "{line}");
                assert_eq!(event.get("target"), Some(&json!("api::app")), "{line}");
                assert_eq!(
                    event.get("fields"),
                    Some(&json!({
                        "message": "password work budget saturated; refusing the request",
                        "running": 1,
                        "queued": queued,
                        "refused": 1,
                    })),
                    "{line}"
                );

                assert!(hold.release().await?);
                assert_eq!(work.run(|| 2).await, Ok(2), "queued {queued}");
            }
            Ok(())
        })
    }

    /// A budget logs its first refusal at once and the next one no sooner than
    /// [`REFUSAL_LOG_INTERVAL`] after it, counting every refusal since the previous event; after
    /// a quiet spell, the next refusal is logged at once.
    #[test]
    fn refusals_are_logged_at_most_once_per_interval_with_their_count() {
        let log = RefusalLog::new();
        let tick = Duration::from_millis(1);
        let cases = [
            (Duration::ZERO, Some(1)),
            (tick, None),
            (REFUSAL_LOG_INTERVAL / 2, None),
            (REFUSAL_LOG_INTERVAL - tick, None),
            (REFUSAL_LOG_INTERVAL, Some(4)),
            (REFUSAL_LOG_INTERVAL + tick, None),
            (REFUSAL_LOG_INTERVAL * 5, Some(2)),
        ];
        for (offset, expected) in cases {
            assert_eq!(log.refuse(log.origin + offset), expected, "{offset:?}");
        }
    }

    #[test]
    fn an_abandoned_computation_keeps_its_places_until_it_returns() -> TestResult {
        block_on(async {
            let work = PasswordWork::new(1, 1).with_wait(LONG_WAIT);
            let finished = Arc::new(AtomicBool::new(false));
            let (started, has_started) = oneshot::channel();
            let (release, released) = mpsc::channel::<()>();
            let request = tokio::spawn({
                let work = work.clone();
                let finished = Arc::clone(&finished);
                async move {
                    work.run(move || {
                        let sent = started.send(()).is_ok() && released.recv().is_ok();
                        finished.store(true, Ordering::SeqCst);
                        sent
                    })
                    .await
                }
            });
            has_started.await?;
            // The request is dropped while its computation runs, as when a client disconnects.
            request.abort();
            assert!(request.await.is_err_and(|error| error.is_cancelled()));

            // The next computation takes the waiting place and waits for the worker the abandoned
            // one still holds ...
            let mut waiting = pin!(work.run({
                let finished = Arc::clone(&finished);
                move || finished.load(Ordering::SeqCst)
            }));
            let first_poll = tokio::time::timeout(Duration::ZERO, waiting.as_mut()).await;
            assert!(
                first_poll.is_err(),
                "the waiting computation finished at once"
            );
            // ... and the abandoned one's admission is still taken too: a third, with a short
            // wait of its own on the same places, finds none and is refused.
            let impatient = work.clone().with_wait(SHORT_WAIT);
            assert_eq!(
                impatient.run(|| false).await,
                Err(PasswordWorkError::Saturated)
            );

            // Released, the abandoned computation returns, and only then does the waiting one run.
            release.send(())?;
            assert_eq!(waiting.await, Ok(true));
            Ok(())
        })
    }

    #[test]
    fn a_panicking_computation_is_interrupted_and_gives_its_places_back() -> TestResult {
        block_on(async {
            let work = PasswordWork::new(1, 0);
            let interrupted = work
                .run(|| -> u8 { panic!("a stand-in for a failed computation") })
                .await;
            assert_eq!(
                interrupted,
                Err(PasswordWorkError::Interrupted { panicked: true })
            );
            assert_eq!(work.run(|| 5).await, Ok(5));
            Ok(())
        })
    }

    /// Runs `test` to completion on a current-thread runtime with I/O and time enabled, the
    /// runtime `#[tokio::test]` builds. It is built here because that attribute's expansion
    /// carries lint-suppression attributes, which R15 admits nowhere (escalation E46). The test
    /// runs on the calling thread, so a trace capture the test starts sees all of it.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    #[test]
    fn the_state_comes_from_the_configuration_and_shows_no_secret() -> TestResult {
        block_on(async {
            let (store, _guard) = closed_store().await?;

            let unset = AppState::from_config(store.clone(), &Config::default(), clock()?);
            assert_eq!(unset.err(), Some(EnvError::MissingJwtSecret));

            let config = Config {
                jwt_secret: SECRET.to_owned(),
                jwt_ttl_seconds: 120,
                intrdr_dir: PathBuf::from("var/test-jobs"),
                ..Config::default()
            };
            let state = AppState::from_config(store, &config, clock()?)?;
            assert_eq!(state.intrdr_dir, PathBuf::from("var/test-jobs"));
            assert_eq!(JwtKeys::from_ref(&state).ttl_seconds(), 120);

            let shown = format!("{state:?}");
            assert_eq!(
                shown,
                format!(
                    r#"AppState {{ store: .., jwt: .., clock: .., intrdr_dir: "var/test-jobs", password_work: {:?} }}"#,
                    PasswordWork::default()
                )
            );
            assert!(!shown.contains(SECRET));

            // The extractors verify with the configured secret: a token of these keys is
            // accepted, one of another secret is refused before the handler runs.
            let token = user_token(&JwtKeys::new(SECRET, 60))?;
            let (status, _) = send(&state, get("/api/v1/menus/main", Some(&token))?).await?;
            assert_eq!(status, StatusCode::OK);
            let foreign = user_token(&JwtKeys::new("another-secret", 60))?;
            let (status, _) = send(&state, get("/api/v1/menus/main", Some(&foreign))?).await?;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            // A `U` token reaches no admin operation.
            let (status, _) = send(&state, get("/api/v1/menus/admin", Some(&token))?).await?;
            assert_eq!(status, StatusCode::FORBIDDEN);

            // A budget sized by the caller replaces the default one and nothing else.
            let resized = state.with_password_work(PasswordWork::new(2, 3));
            assert_eq!(
                format!("{resized:?}"),
                r#"AppState { store: .., jwt: .., clock: .., intrdr_dir: "var/test-jobs", password_work: PasswordWork { running: 2, queued: 3, wait: 3s, .. } }"#
            );
            Ok(())
        })
    }

    #[test]
    fn the_document_is_served_and_spans_carry_route_templates_only() -> TestResult {
        block_on(async {
            let (store, _guard) = closed_store().await?;
            let state = AppState::new(
                store,
                JwtKeys::new(SECRET, 3600),
                clock()?,
                PathBuf::from("var/jobs"),
            );

            let traces = trace_capture::capture();

            // The document's compact JSON, byte for byte, on every request and every connection.
            let router = app(state.clone());
            for _ in 0..2 {
                let (status, headers, bytes) =
                    exchange(router.clone(), get(DOCUMENT_ROUTE, None)?).await?;
                assert_eq!(status, StatusCode::OK);
                assert_eq!(bytes, serde_json::to_vec(&openapi())?);
                assert_eq!(
                    header_text(&headers, header::CONTENT_TYPE),
                    Some("application/json")
                );
                assert_protected(&headers, DOCUMENT_ROUTE);
            }
            assert!(traces.contains(r#""route":"/api-docs/openapi.json""#));

            // No token: the extractor answers 401 before the path or the store is read.
            let (status, headers, _) = exchange(
                app(state.clone()),
                get("/api/v1/accounts/00000000050/view", None)?,
            )
            .await?;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_protected(&headers, "a request without a token");
            assert!(traces.contains(r#""route":"/api/v1/accounts/{acct_id}/view""#));
            assert!(traces.contains(r#""method":"GET""#));
            assert!(traces.contains(r#""status":401"#));
            assert!(traces.contains(r#""latency_ms":"#));

            let (status, _) = send(
                &state,
                get(
                    "/api/v1/no-such-route/0500024453765740?card=0500024453765740",
                    None,
                )?,
            )
            .await?;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(traces.contains(r#""route":"<unmatched>""#));
            assert!(traces.contains(r#""status":404"#));

            let text = traces.text();
            for withheld in ["00000000050", "0500024453765740", "no-such-route", "card="] {
                assert!(
                    !text.contains(withheld),
                    "a span or event recorded {withheld}"
                );
            }
            Ok(())
        })
    }

    /// The admission wait of the in-flight tests: short, so they finish quickly, and long enough
    /// to tell a request that waited from one answered at once.
    const IN_FLIGHT_WAIT: Duration = Duration::from_millis(200);

    #[test]
    fn a_request_beyond_the_in_flight_limit_waits_then_is_refused_and_traced() -> TestResult {
        block_on(async {
            // A route whose handler counts its calls and returns only when released.
            let calls = Arc::new(AtomicUsize::new(0));
            let release = Arc::new(Notify::new());
            let held = {
                let calls = Arc::clone(&calls);
                let release = Arc::clone(&release);
                move || {
                    let release = Arc::clone(&release);
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        release.notified().await;
                        StatusCode::OK
                    }
                }
            };
            let router = layered(
                Router::new().route("/held/{key}", routing::get(held)),
                1,
                IN_FLIGHT_WAIT,
            );
            let traces = trace_capture::capture();

            // The first request takes the only permit and waits in its handler ...
            let mut first = pin!(exchange(
                router.clone(),
                get("/held/0500024453765740", None)?
            ));
            let first_poll = tokio::time::timeout(Duration::ZERO, first.as_mut()).await;
            assert!(first_poll.is_err(), "the held request finished at once");
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // ... so the next waits the whole admission wait for a place, which never frees, and
            // is then refused before its handler runs.
            let sent = Instant::now();
            let (status, headers, bytes) =
                exchange(router.clone(), get("/held/00000000050", None)?).await?;
            let waited = sent.elapsed();
            assert!(waited >= IN_FLIGHT_WAIT, "refused after {waited:?}");
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                header_text(&headers, header::CONTENT_TYPE),
                Some("application/json")
            );
            assert_eq!(header_text(&headers, header::RETRY_AFTER), Some("1"));
            assert_protected(&headers, "the refused request");
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                json!({ "message": SERVER_BUSY })
            );
            for recorded in [r#""route":"/held/{key}""#, r#""status":503"#] {
                assert!(traces.contains(recorded), "no {recorded}");
            }

            // A third request, sent while the first still holds the permit, queues for it
            // rather than being refused ...
            let mut third = pin!(exchange(router, get("/held/1", None)?));
            let third_poll = tokio::time::timeout(Duration::ZERO, third.as_mut()).await;
            assert!(
                third_poll.is_err(),
                "the queued request was answered at once"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // ... and when the first is released and answered within the wait, its permit
            // admits the third, which is served.
            release.notify_one();
            let (status, headers, _) = first.await?;
            assert_eq!(status, StatusCode::OK);
            assert_protected(&headers, "the held request");
            assert_eq!(header_text(&headers, header::RETRY_AFTER), None);
            release.notify_one();
            let (status, headers, _) = third.await?;
            assert_eq!(status, StatusCode::OK);
            assert_protected(&headers, "the queued request");
            assert_eq!(header_text(&headers, header::RETRY_AFTER), None);
            assert_eq!(calls.load(Ordering::SeqCst), 2);

            let text = traces.text();
            for withheld in ["0500024453765740", "00000000050"] {
                assert!(!text.contains(withheld), "a span recorded {withheld}");
            }
            Ok(())
        })
    }

    #[test]
    fn a_closed_in_flight_limit_refuses_at_once() -> TestResult {
        block_on(async {
            // A closed semaphore admits nothing, so its refusal answers the first poll, however
            // long the admission wait.
            let permits = Arc::new(Semaphore::new(1));
            permits.close();
            let admission = Admission {
                permits,
                wait: Duration::from_secs(3600),
            };
            let router = Router::new()
                .route("/", routing::get(|| async { StatusCode::OK }))
                .layer(middleware::from_fn_with_state(admission, admit));
            let (status, headers, bytes) =
                tokio::time::timeout(Duration::ZERO, exchange(router, get("/", None)?)).await??;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(header_text(&headers, header::RETRY_AFTER), Some("1"));
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                json!({ "message": SERVER_BUSY })
            );
            Ok(())
        })
    }

    #[test]
    fn in_flight_limits_admit_at_least_one_request() -> TestResult {
        block_on(async {
            // A limit of 0 is raised to 1, so the router still serves.
            let ok = routing::get(|| async { StatusCode::OK });
            let router = layered(Router::new().route("/", ok), 0, IN_FLIGHT_WAIT);
            let (status, headers, _) = exchange(router, get("/", None)?).await?;
            assert_eq!(status, StatusCode::OK);
            assert_protected(&headers, "a limit of 0");
            Ok(())
        })
    }

    #[test]
    fn unmatched_paths_and_unserved_methods_answer_screen_errors() -> TestResult {
        block_on(async {
            let (store, _guard) = closed_store().await?;
            let keys = JwtKeys::new(SECRET, 3600);
            let token = user_token(&keys)?;
            let state = AppState::new(store, keys, clock()?, PathBuf::from("var/jobs"));
            let traces = trace_capture::capture();

            // Method, URI, status, message, the `Allow` header axum lists for the matched route
            // (none for an unmatched path) and the route the request span records.
            let cases = [
                (
                    Method::GET,
                    "/api/v1/signon",
                    StatusCode::METHOD_NOT_ALLOWED,
                    METHOD_NOT_SERVED,
                    Some("POST"),
                    "/api/v1/signon",
                ),
                (
                    Method::PATCH,
                    "/api/v1/accounts/00000000050",
                    StatusCode::METHOD_NOT_ALLOWED,
                    METHOD_NOT_SERVED,
                    Some("GET,HEAD,PUT"),
                    "/api/v1/accounts/{acct_id}",
                ),
                (
                    Method::GET,
                    "/api/v1/reports/transactions",
                    StatusCode::METHOD_NOT_ALLOWED,
                    METHOD_NOT_SERVED,
                    Some("POST"),
                    "/api/v1/reports/transactions",
                ),
                (
                    Method::POST,
                    DOCUMENT_ROUTE,
                    StatusCode::METHOD_NOT_ALLOWED,
                    METHOD_NOT_SERVED,
                    Some("GET,HEAD"),
                    DOCUMENT_ROUTE,
                ),
                (
                    Method::GET,
                    "/api/v1/nope?card_num=0500024453765740",
                    StatusCode::NOT_FOUND,
                    NO_SUCH_ROUTE,
                    None,
                    UNMATCHED_ROUTE,
                ),
                (
                    Method::GET,
                    "/api/v1/users/",
                    StatusCode::NOT_FOUND,
                    NO_SUCH_ROUTE,
                    None,
                    UNMATCHED_ROUTE,
                ),
            ];
            for (method, uri, status, message, allow, route) in cases {
                traces.clear();
                let request = Request::builder()
                    .method(method.clone())
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())?;
                let (answered, headers, bytes) = exchange(app(state.clone()), request).await?;
                let case = format!("{method} {uri}");
                assert_eq!(answered, status, "{case}");
                assert_eq!(
                    header_text(&headers, header::CONTENT_TYPE),
                    Some("application/json"),
                    "{case}"
                );
                assert_eq!(header_text(&headers, header::ALLOW), allow, "{case}");
                assert_protected(&headers, &case);
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes)?,
                    json!({ "message": message }),
                    "{case}"
                );

                for recorded in [
                    format!(r#""route":"{route}""#),
                    format!(r#""method":"{method}""#),
                    format!(r#""status":{}"#, status.as_u16()),
                ] {
                    assert!(traces.contains(&recorded), "{case}: no {recorded}");
                }
                let text = traces.text();
                for withheld in ["00000000050", "0500024453765740", "nope", "/users/"] {
                    assert!(!text.contains(withheld), "{case}: recorded {withheld}");
                }
            }
            Ok(())
        })
    }
}
