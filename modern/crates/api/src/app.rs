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
//!   of its own operations, wraps every route in the request trace layer and binds the state.
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
//! request that finds every place taken is refused at once, never queued: its service answers
//! the 500 it gives a computation that cannot finish. Argon2's parameters, the verification and
//! every ordinary answer are unchanged; only admission is bounded.
//!
//! # What is deliberately absent
//!
//! - No CORS layer: the Vite dev server proxies `/api`, so the browser sees a single origin.
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
use std::time::Duration;

use axum::extract::{FromRef, MatchedPath};
use axum::handler::Handler;
use axum::http::{Request, Response};
use axum::routing::{MethodRouter, get};
use axum::{Json, Router};
use domain::clock::Clock;
use persistence::PgStore;
use runtime::config::{Config, EnvError};
use runtime::telemetry::{UNMATCHED_ROUTE, record_response, request_span};
use tokio::sync::Semaphore;
use tower_http::trace::{MakeSpan, OnResponse, TraceLayer};
use tracing::Span;
use utoipa::__dev::{SchemaReferences, Tags};
use utoipa::OpenApi as _;
use utoipa::Path;
use utoipa::openapi::OpenApi;
use utoipa::openapi::path::Paths;
use utoipa_axum::PathItemExt as _;
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouter};

use crate::auth::jwt::JwtKeys;
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

/// How many argon2id computations [`PasswordWork::default`] lets wait for a worker once every
/// worker is busy. A waiting computation has not started, so it holds no argon2 memory and no
/// thread, only its request; 64 places are eight rounds of a full complement of eight workers,
/// enough to absorb an ordinary burst of sign-ons, and a request beyond them is refused at once
/// rather than kept waiting behind an unbounded queue.
pub const PASSWORD_QUEUE: usize = 64;

/// The budget every argon2id computation of the API runs under: sign-on's verification
/// (COSGN00C), the user add's hash (COUSR01C) and the user update's change test (COUSR02C). See
/// "Password work" in the module documentation for why it exists.
///
/// At most `running` computations run at once, each on a thread of Tokio's blocking pool, and at
/// most `queued` more wait for one. A computation keeps its places until it returns, so a
/// request that is dropped while its computation runs frees nothing early. A request that finds
/// every place taken is refused at once ([`PasswordWorkError::Saturated`]).
///
/// Cloning is cheap and shares the budget: each limit is one `Arc`ed semaphore.
#[derive(Clone)]
pub struct PasswordWork {
    /// One permit per computation that may run at once.
    running: Arc<Semaphore>,
    /// One permit per computation admitted, running or waiting for a `running` permit.
    admitted: Arc<Semaphore>,
    /// The `running` limit, as `Debug` and the saturation event show it.
    workers: usize,
    /// The waiting places beyond the workers, as `Debug` and the saturation event show them.
    queue: usize,
}

impl PasswordWork {
    /// A budget of `running` computations at once and `queued` more waiting for a worker.
    ///
    /// `running` is raised to 1 when it is 0, because a budget that runs nothing would keep
    /// every admitted request waiting forever. `queued` may be 0: every request that finds all
    /// workers busy is then refused. Both are capped so that their sum fits one Tokio semaphore
    /// ([`Semaphore::MAX_PERMITS`]).
    #[must_use]
    pub fn new(running: usize, queued: usize) -> Self {
        let workers = running.clamp(1, Semaphore::MAX_PERMITS);
        let admitted = workers.saturating_add(queued).min(Semaphore::MAX_PERMITS);
        Self {
            running: Arc::new(Semaphore::new(workers)),
            admitted: Arc::new(Semaphore::new(admitted)),
            workers,
            queue: admitted.saturating_sub(workers),
        }
    }

    /// Runs `work`, an argon2id computation, on Tokio's blocking pool within the budget, and
    /// returns its result.
    ///
    /// The request takes a waiting place first, without waiting for one, then waits for a
    /// worker. Both places move into the blocking task and are given back only when `work`
    /// returns or unwinds, so dropping the returned future frees a place only while `work` has
    /// not started. A refusal is logged here, once, as an `ERROR`, since its request is answered
    /// with a 500, naming the budget's limits and nothing of the request.
    ///
    /// # Errors
    ///
    /// - [`PasswordWorkError::Saturated`] at once when every place is taken; `work` never runs.
    /// - [`PasswordWorkError::Interrupted`] when the blocking task does not complete: `work`
    ///   panicked, or the runtime shut down first. Only whether it panicked is kept.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PasswordWorkError> {
        // Neither semaphore is ever closed, so only `NoPermits` refuses in practice; a closed
        // one would admit nothing, which is what a refusal answers.
        let admitted = Arc::clone(&self.admitted)
            .try_acquire_owned()
            .map_err(|_| self.refused())?;
        let running = Arc::clone(&self.running)
            .acquire_owned()
            .await
            .map_err(|_| self.refused())?;
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

    /// Logs a refusal and returns it: the one event of a saturated budget, naming its limits
    /// alone, never a user ID, a password or a hash.
    fn refused(&self) -> PasswordWorkError {
        tracing::error!(
            running = self.workers,
            queued = self.queue,
            "password work budget saturated; refusing the request"
        );
        PasswordWorkError::Saturated
    }
}

/// The budget of the `carddemo-api` process: as many workers as the host's available
/// parallelism, at least 1, and 1 when it cannot be determined, but never more than
/// [`MAX_PASSWORD_WORKERS`]; and [`PASSWORD_QUEUE`] waiting places.
impl Default for PasswordWork {
    fn default() -> Self {
        let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        Self::new(parallelism.min(MAX_PASSWORD_WORKERS), PASSWORD_QUEUE)
    }
}

/// Shows the two limits and leaves out the semaphores, whose momentary state is not a setting.
impl fmt::Debug for PasswordWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordWork")
            .field("running", &self.workers)
            .field("queued", &self.queue)
            .finish_non_exhaustive()
    }
}

/// Why [`PasswordWork::run`] returned no result. Neither variant carries a value of the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasswordWorkError {
    /// Every worker was busy and every waiting place taken, so the computation was refused at
    /// once and never ran.
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
    use std::future::poll_fn;
    use std::pin::pin;
    use std::sync::mpsc;
    use std::task::Poll;
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

    /// Returns once every place of `work` is taken, which a probe computation refused as
    /// saturated shows.
    ///
    /// Each probe is polled exactly once and dropped at once when it is admitted instead, so it
    /// never keeps a place from the request the caller waits for, even across the pause between
    /// probes.
    pub(crate) async fn until_saturated(work: &PasswordWork) -> Result<(), Box<dyn StdError>> {
        loop {
            let answered = {
                let mut probe = pin!(work.run(|| ()));
                poll_fn(|cx| Poll::Ready(probe.as_mut().poll(cx))).await
            };
            match answered {
                Poll::Ready(Err(PasswordWorkError::Saturated)) => return Ok(()),
                Poll::Ready(other) => {
                    return Err(format!("a probe was not refused: {other:?}").into());
                }
                Poll::Pending => tokio::time::sleep(Duration::from_millis(1)).await,
            }
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

/// The complete API: the operations of [`router`], the document at [`DOCUMENT_ROUTE`], one
/// `http.request` span per request (see the module documentation) and `state`.
#[must_use = "the API answers no request until the router is served"]
pub fn app(state: AppState) -> Router {
    let (router, document) = router();
    router
        .route(DOCUMENT_ROUTE, get(move || async move { Json(document) }))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(RouteSpan)
                .on_response(ResponseFields),
        )
        .with_state(state)
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::pin::pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use axum::body::{self, Body};
    use axum::extract::FromRef;
    use axum::http::{Request, StatusCode, header};
    use chrono::NaiveDate;
    use domain::clock::{Clock, FixedClock};
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use persistence::PgStore;
    use runtime::config::{Config, EnvError};
    use serde_json::Value;
    use test_support::postgres::{PostgresGuard, start_postgres};
    use test_support::trace_capture;
    use tokio::sync::{Semaphore, oneshot};
    use tower::ServiceExt as _;

    use super::worker_hold::WorkerHold;
    use super::{
        AppState, DOCUMENT_ROUTE, MAX_PASSWORD_WORKERS, PASSWORD_QUEUE, PasswordWork,
        PasswordWorkError, app, openapi,
    };
    use crate::auth::jwt::JwtKeys;

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
        let response = app(state.clone()).oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, bytes.to_vec()))
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

    #[test]
    fn password_budgets_keep_valid_limits() {
        let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let host_workers = parallelism.min(MAX_PASSWORD_WORKERS);
        assert!((1..=MAX_PASSWORD_WORKERS).contains(&host_workers));
        let cases = [
            // A budget that ran nothing would keep every request waiting: one worker at least.
            (PasswordWork::new(0, 0), 1, 0),
            (PasswordWork::new(2, 3), 2, 3),
            // Capped so that every place fits one semaphore.
            (
                PasswordWork::new(usize::MAX, usize::MAX),
                Semaphore::MAX_PERMITS,
                0,
            ),
            (PasswordWork::default(), host_workers, PASSWORD_QUEUE),
        ];
        for (work, running, queued) in cases {
            assert_eq!(
                format!("{work:?}"),
                format!("PasswordWork {{ running: {running}, queued: {queued}, .. }}")
            );
        }
    }

    #[test]
    fn a_full_password_budget_refuses_at_once_and_logs_no_value() -> TestResult {
        block_on(async {
            let work = PasswordWork::new(1, 0);
            let shared = work.clone();
            let hold = WorkerHold::start(&work).await?;
            let traces = trace_capture::capture();

            // The refusal is the answer to the first poll: a request never waits for a place, and
            // a clone draws on the same places.
            let refused = tokio::time::timeout(Duration::ZERO, shared.run(|| 1)).await;
            assert_eq!(refused, Ok(Err(PasswordWorkError::Saturated)));
            let lines = traces.lines();
            let [line] = lines.as_slice() else {
                return Err(format!("{} lines", lines.len()).into());
            };
            for part in [
                r#""level":"ERROR""#,
                "password work budget saturated",
                r#""running":1"#,
                r#""queued":0"#,
            ] {
                assert!(line.contains(part), "{part} missing from {line}");
            }

            // Once the computation returns, its places serve the next request.
            assert!(hold.release().await?);
            assert_eq!(shared.run(|| 2).await, Ok(2));
            Ok(())
        })
    }

    #[test]
    fn an_abandoned_computation_keeps_its_places_until_it_returns() -> TestResult {
        block_on(async {
            let work = PasswordWork::new(1, 1);
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
            // ... and the abandoned one's admission is still taken too: a third is refused.
            assert_eq!(work.run(|| false).await, Err(PasswordWorkError::Saturated));

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
                r#"AppState { store: .., jwt: .., clock: .., intrdr_dir: "var/test-jobs", password_work: PasswordWork { running: 2, queued: 3, .. } }"#
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

            let (status, bytes) = send(&state, get(DOCUMENT_ROUTE, None)?).await?;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes)?,
                serde_json::to_value(openapi())?
            );
            assert!(traces.contains(r#""route":"/api-docs/openapi.json""#));

            // No token: the extractor answers 401 before the path or the store is read.
            let (status, _) = send(&state, get("/api/v1/accounts/00000000050/view", None)?).await?;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
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
}
