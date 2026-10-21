//! Dev-only test harness for the `CardDemo` workspace: the integration tests of `persistence`,
//! `api`, `messaging` and `batch` get their containers, fixtures, audit index and trace capture
//! here. The crate is listed only under their `[dev-dependencies]`, never as a normal dependency.
//!
//! # Public-API boundary
//!
//! `persistence` dev-depends on this crate while this crate depends on `persistence`, so Cargo
//! compiles `persistence` twice and the types of the two copies differ. Every public signature
//! here therefore uses only standard-library types, `sqlx` types such as `PgPool`, `lapin` types
//! and this crate's own structs built from them, never a `persistence`, `domain` or `copybook`
//! type.
//!
//! # Local-only runtime
//!
//! The stores run only as testcontainers on the local Docker engine and are removed when their
//! guard drops. Their credentials are the local values `carddemo`/`carddemo`.
//!
//! Every start labels its container `org.carddemo.test-support.instance=<token>` and names it
//! `carddemo-test-<image>-<token>`, with a token no other start shares. Docker creates a
//! container before testcontainers hands back the handle that removes it, and a start cut off
//! while Docker creates the container cannot learn whether Docker made it. Each start therefore
//! runs to its end on a thread and Tokio runtime of its own, which nothing outside can cut off,
//! so every request it makes gets Docker's answer. That thread hands the handle to the caller or,
//! when the start failed or the caller gave up on it, removes the containers carrying its own
//! token, and no other. A caller that gives up part-way, through a timeout, a runtime shutdown
//! or a panic, waits in the drop of the start's future until the thread has done so.
//!
//! A container stays behind, labelled with its token for removal by hand, only when the process
//! is killed or aborts while a start runs; when Docker keeps a start waiting past the bound that
//! drop waits for, which standard error reports, and the process ends before Docker answers;
//! when Docker does not answer the cleanup within its bound, which the start error or standard
//! error reports; when a request of the start fails at the transport level, for example at the
//! Docker client's 120-second answer timeout, while Docker still finishes it later, which the
//! start error reports; or when a panic inside testcontainers' start drops it while Docker is
//! creating the container and Docker finishes that create more than four seconds later.
//!
//! # Modules
//!
//! - [`audit_index`]: the audit-table index and the scanners for test tags and rule citations.
//! - [`fixtures`]: fixture paths read in place, fixture loading and redaction needles.
//! - [`postgres`]: a `postgres:16` container migrated through the `runtime` migration entrypoint.
//! - [`rabbitmq`]: a `rabbitmq:3.13-management` container with the broker definitions mounted.
//! - [`trace_capture`]: an in-memory capture of the JSON tracing output.

// Crate-specific lints can live only here: a member inheriting `[lints] workspace = true` cannot
// add entries of its own. `deny` tightens the workspace set for the total parsing in
// `audit_index` and `fixtures`; it suppresses nothing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod audit_index;
pub mod fixtures;
pub mod postgres;
pub mod rabbitmq;
pub mod trace_capture;

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Write};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use testcontainers::bollard::Docker;
use testcontainers::bollard::container::{ListContainersOptions, RemoveContainerOptions};
use testcontainers::bollard::errors::Error as DockerError;
use testcontainers::core::client::docker_client_instance;
use testcontainers::runners::AsyncRunner;
use testcontainers::{
    ContainerAsync, ContainerRequest, GenericImage, Image, ImageExt, TestcontainersError,
};
use tokio::sync::oneshot;
use tokio::time;

/// Label on every container this crate starts. Its value is the token of the start that created
/// the container, so a cleanup finds exactly that start's containers.
const INSTANCE_LABEL: &str = "org.carddemo.test-support.instance";

/// Bound on one [`sweep`]: the listings of a start's containers and every removal they make.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Further listings, [`PANIC_SETTLE_PAUSE`] apart, that the [`sweep`] after a panic inside
/// testcontainers' start makes: the one case in which a start does not run to its end, as the
/// panic drops it, possibly while Docker is creating the container. Docker finishes a create
/// whose client went away, so that container appears after the first listing; the eight rounds
/// give the create four seconds. No other sweep waits on time: every other start has run to its
/// end, so every create it made has returned.
const PANIC_SETTLE_ROUNDS: u32 = 8;
const PANIC_SETTLE_PAUSE: Duration = Duration::from_millis(500);

/// How long the Docker client testcontainers builds waits for each answer from Docker.
const DOCKER_ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
/// testcontainers' startup bound for a request that sets none.
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// Time allowed for pulling an image the engine lacks: no answer timeout bounds the download.
const PULL_ALLOWANCE: Duration = Duration::from_secs(120);

/// One testcontainers start: it creates the container, starts it, waits until it is ready and
/// returns the handle whose drop removes it.
type Starting =
    Pin<Box<dyn Future<Output = Result<ContainerAsync<GenericImage>, TestcontainersError>> + Send>>;

/// What a start hands its caller: the handle of its container, or the error with its cleanup.
type Outcome = Result<ContainerAsync<GenericImage>, HarnessError>;

/// Tells apart the starts of one process that read the same clock value.
static NEXT_START: AtomicU64 = AtomicU64::new(0);

/// The `modern/` directory, for example `/src/carddemo/modern/crates/test-support/../..` for a
/// checkout at `/src/carddemo`.
///
/// `CARGO_MANIFEST_DIR` expands when this crate compiles, so the result is the same `modern/`
/// directory whichever crate's test calls it. The path is not canonicalised; callers that need an
/// absolute path, such as Docker bind mounts, canonicalise it themselves.
#[must_use]
pub fn modern_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The repository root, one level above [`modern_dir`]. The fixtures live beneath it, at
/// `app/data/ASCII` and `app/jcl`.
#[must_use]
pub fn repo_root() -> PathBuf {
    modern_dir().join("..")
}

/// A failure of the harness itself: a container that does not start, a store that never becomes
/// ready, or a fixture that cannot be read or loaded.
///
/// The message is the `Display` text of the library error behind it (sqlx, lapin, testcontainers,
/// Tokio, the Docker client, `PersistenceError`, the domain `LoadError` and `PasswordError`, or
/// `std::io::Error`), with at most a bound that passed and a container label token, none of
/// which carries record bytes. No caller formats a card number, SSN, CVV or password into one, so
/// a failing test may print it (Cardholder Data Redaction).
#[derive(Debug)]
pub struct HarnessError {
    context: &'static str,
    message: String,
}

impl HarnessError {
    /// Wraps `err` under `context`, the harness step that failed, such as `"start postgres"`.
    pub(crate) fn new(context: &'static str, err: &dyn fmt::Display) -> Self {
        Self {
            context,
            message: err.to_string(),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.message)
    }
}

impl std::error::Error for HarnessError {}

/// The identity one container start owns: the value of its [`INSTANCE_LABEL`] and the container
/// name built from it.
struct Instance {
    token: String,
    name: String,
}

impl Instance {
    /// A fresh identity for a container of the image named `image`, such as `postgres`.
    ///
    /// The token joins the process ID, the nanoseconds since the UNIX epoch, a per-process
    /// sequence number and a randomly keyed hash of the three, so no two starts share one,
    /// whether they run in one process or in concurrent processes on one Docker engine. The name
    /// keeps only the characters a Docker container name allows, `[a-zA-Z0-9_.-]`, from `image`.
    fn new(image: &str) -> Self {
        let process = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let sequence = NEXT_START.fetch_add(1, Ordering::Relaxed);
        let salt = RandomState::new().hash_one((process, nanos, sequence));
        let token = format!("{process:x}-{nanos:x}-{sequence:x}-{salt:016x}");
        let image: String = image
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let name = format!("carddemo-test-{image}-{token}");
        Self { token, name }
    }
}

/// Starts `request` as a container this start owns, labelled and named from a fresh
/// [`Instance`], and returns the testcontainers handle whose drop removes it.
///
/// testcontainers creates the container, then starts it, and only then builds that handle, so a
/// start error or a startup timeout in between would leave the created container behind, and a
/// start cut off during the create could not even learn whether Docker made it. The start
/// therefore runs to its end on a thread and Tokio runtime of its own ([`start_with`]):
///
/// - A start that succeeds hands its handle to the caller, who alone owns removal from then on.
/// - A start error removes every container carrying this start's token, and the error appends
///   how many it removed or why the removal failed.
/// - A caller that gives up part-way, as a timeout, a runtime shutdown or a panic makes it,
///   waits in the drop of the returned future until the thread has ended the start and removed
///   its container, a handle already on its way to the caller included ([`PendingStart`]).
///
/// A container stays behind only in the cases the crate documentation lists.
pub(crate) async fn start_owned(
    request: ContainerRequest<GenericImage>,
    context: &'static str,
) -> Outcome {
    let instance = Instance::new(request.image().name());
    start_as(request, context, &instance).await
}

/// [`start_owned`] under a given identity, which the tests choose so that they can look for it.
async fn start_as(
    request: ContainerRequest<GenericImage>,
    context: &'static str,
    instance: &Instance,
) -> Outcome {
    let bound = finish_bound(request.startup_timeout());
    let starting = request
        .with_label(INSTANCE_LABEL, instance.token.as_str())
        .with_container_name(instance.name.as_str())
        .start();
    start_with(starting, context, instance, bound).await
}

/// How long the drop of a start its caller gave up on waits for the start's thread, for a
/// request whose startup bound is `startup`, testcontainers' 60 seconds when it sets none.
///
/// The thread's work takes at most four answers from Docker, each within
/// [`DOCKER_ANSWER_TIMEOUT`] (a create, a second create after pulling a missing image, the
/// removal of the container, and the removal testcontainers' drop of the handle retries when
/// that one fails), [`PULL_ALLOWANCE`] for the pull, the startup bound and one [`sweep`]: 810
/// seconds for the 180-second startup bound of both harnesses.
fn finish_bound(startup: Option<Duration>) -> Duration {
    DOCKER_ANSWER_TIMEOUT
        .saturating_mul(4)
        .saturating_add(PULL_ALLOWANCE)
        .saturating_add(startup.unwrap_or(DEFAULT_STARTUP_TIMEOUT))
        .saturating_add(CLEANUP_TIMEOUT)
}

/// Runs `starting`, a start of a container labelled with `instance`'s token, to its end on a
/// thread and current-thread Tokio runtime of its own, and returns its outcome.
///
/// Nothing outside that thread can cut the start off, so every request it makes gets Docker's
/// answer, and when it ends every create it made has returned; [`run_start`] then hands its
/// outcome over or removes what it made. The caller waits through a [`PendingStart`], on a
/// runtime of either flavor; one that gives up first waits in its drop, at most `bound`, until
/// the thread has finished.
///
/// The thread starts at the first poll: a future dropped before it was polled never started.
async fn start_with(
    starting: Starting,
    context: &'static str,
    instance: &Instance,
    bound: Duration,
) -> Outcome {
    let (outcome_sender, outcome) = oneshot::channel();
    let (returns, returned) = mpsc::channel();
    let (report, finished) = mpsc::channel();
    let token = instance.token.clone();
    thread::Builder::new()
        .name("test-support-start".to_owned())
        .spawn(move || {
            let finish = run_start(starting, context, &token, outcome_sender, &returned);
            // The receiver is gone only once the caller took the outcome, and then nothing reads
            // the report.
            let _ = report.send(finish);
        })
        .map_err(|err| {
            HarnessError::new(
                context,
                &format_args!("the start thread did not start: {err}"),
            )
        })?;
    let mut pending = PendingStart {
        token: instance.token.clone(),
        outcome,
        returns,
        finished,
        bound,
        received: false,
    };
    pending.wait(context).await
}

/// The work of a start's thread: runs `starting` to its end, sends its outcome through
/// `outcome`, and returns once nothing the start made is left but what the caller took, or why
/// that failed.
///
/// - A container whose caller had already given up is removed here ([`dispose`]). One sent
///   before the caller gave up, but not yet taken, comes back through `returned` and is removed
///   the same way. After a send the thread waits on `returned` until the caller has either taken
///   the container or given it back.
/// - A failed start sends its error once [`sweep`] has removed the containers labelled with
///   `token`, with how many it removed or why that failed.
/// - A panic inside `starting` is caught, so that the thread can still sweep, and becomes the
///   error "the start panicked".
fn run_start(
    starting: Starting,
    context: &'static str,
    token: &str,
    outcome: oneshot::Sender<Outcome>,
    returned: &mpsc::Receiver<ContainerAsync<GenericImage>>,
) -> Result<(), String> {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            // `starting` was never polled, so Docker made nothing for it. The caller is gone only
            // once it gave up, and its drop then reads the report instead.
            let _ = outcome.send(Err(HarnessError::new(
                context,
                &format_args!("the start runtime did not start: {err}"),
            )));
            return Ok(());
        }
    };
    let (failure, panicked) =
        match panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(starting))) {
            Ok(Ok(container)) => {
                let container = match outcome.send(Ok(container)) {
                    Err(Ok(container)) => container,
                    _ => match returned.recv() {
                        Ok(container) => container,
                        // The caller took the container.
                        Err(_) => return Ok(()),
                    },
                };
                return runtime.block_on(dispose(container, token));
            }
            Ok(Err(err)) => (err.to_string(), false),
            Err(_) => ("the start panicked".to_owned(), true),
        };
    let label = format!("{INSTANCE_LABEL}={token}");
    let (note, finish) = match runtime.block_on(sweep(token, panicked)) {
        Ok(removed) => (
            format!("the cleanup removed {removed} container(s) labelled {label}"),
            Ok(()),
        ),
        Err(failure) => {
            let note = format!("the cleanup of the containers labelled {label} failed: {failure}");
            (note.clone(), Err(note))
        }
    };
    // The caller is gone only once it gave up, and its drop then reads `finish` instead.
    let _ = outcome.send(Err(HarnessError::new(
        context,
        &format_args!("{failure}; {note}"),
    )));
    finish
}

/// Removes `container`, which a start made but its caller never took, through its handle, and
/// falls back on a [`sweep`] of the containers labelled with `token` when that fails.
async fn dispose(container: ContainerAsync<GenericImage>, token: &str) -> Result<(), String> {
    let Err(failure) = container.rm().await else {
        return Ok(());
    };
    sweep(token, false)
        .await
        .map(|_removed| ())
        .map_err(|swept| {
            format!(
                "the removal of the container failed: {failure}; the cleanup of the containers \
                 labelled {INSTANCE_LABEL}={token} failed: {swept}"
            )
        })
}

/// Removes the containers labelled with `token` once the start that could make them has ended,
/// and returns how many it removed, within [`CLEANUP_TIMEOUT`] in all.
///
/// A start that ran to its end has had an answer to every create it made, so one listing finds
/// whatever it made. After a panic inside the start (`panicked`), which dropped it part-way, the
/// listing repeats up to [`PANIC_SETTLE_ROUNDS`] times, [`PANIC_SETTLE_PAUSE`] apart, until one
/// has removed a container: a start makes at most one, as its name is its own and testcontainers
/// creates again only after a create that found no image and so made nothing.
async fn sweep(token: &str, panicked: bool) -> Result<usize, String> {
    let swept = async {
        let docker = docker_client_instance()
            .await
            .map_err(|err| err.to_string())?;
        let mut outcome = remove_labelled(&docker, token).await;
        if panicked {
            for _ in 0..PANIC_SETTLE_ROUNDS {
                if matches!(outcome, Ok(removed) if removed > 0) {
                    break;
                }
                time::sleep(PANIC_SETTLE_PAUSE).await;
                outcome = remove_labelled(&docker, token).await;
            }
        }
        outcome
    };
    time::timeout(CLEANUP_TIMEOUT, swept)
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "Docker did not finish within {} ms",
                CLEANUP_TIMEOUT.as_millis()
            ))
        })
}

/// The caller's side of a start running on a thread of its own ([`start_with`]).
struct PendingStart {
    /// The start's label token, which a warning names.
    token: String,
    /// Where the start's outcome arrives.
    outcome: oneshot::Receiver<Outcome>,
    /// Gives the start's thread back a container that arrived after the caller gave up.
    returns: mpsc::Sender<ContainerAsync<GenericImage>>,
    /// The thread's report once it has finished: nothing the start made is left but what the
    /// caller took, or why that failed.
    finished: mpsc::Receiver<Result<(), String>>,
    /// How long the drop waits for that report ([`finish_bound`]).
    bound: Duration,
    /// Whether the outcome reached the caller, who then owns it.
    received: bool,
}

impl PendingStart {
    /// Waits for the start's outcome. Once it has arrived the caller owns it, and the drop leaves
    /// the start's thread alone.
    async fn wait(&mut self, context: &'static str) -> Outcome {
        match (&mut self.outcome).await {
            Ok(outcome) => {
                self.received = true;
                outcome
            }
            Err(_) => Err(HarnessError::new(
                context,
                &"the start thread ended without an outcome",
            )),
        }
    }
}

impl Drop for PendingStart {
    /// Runs when the caller gives up on the start before its outcome reached it: a timeout, a
    /// runtime shutdown or a panic dropped the future waiting for it.
    ///
    /// The drop closes the outcome channel, so that a later send fails and the start's thread
    /// removes the container itself, and gives a container already sent back to that thread for
    /// removal. It never drops one here: testcontainers' drop of a handle blocks on the dropping
    /// thread's runtime, which no longer drives I/O once it is shutting down, and panics outside
    /// any runtime. It then blocks until the thread reports that it has finished, at most
    /// `bound`. The thread needs nothing from the caller's runtime, so blocking even that
    /// runtime's only worker cannot hold it up. A failed removal, or a thread that did not finish
    /// within `bound` (it carries on, and removes the container once the start ends), goes to
    /// standard error, as `drop` cannot return it. The drop never panics.
    fn drop(&mut self) {
        if self.received {
            return;
        }
        self.outcome.close();
        let mut failures = Vec::new();
        if let Ok(Ok(container)) = self.outcome.try_recv() {
            if let Err(mpsc::SendError(container)) = self.returns.send(container) {
                // The thread holds the receiving end until it has received from it after a
                // successful send, so this happens only if that thread died in between. With
                // no runtime fit to remove the container, it is left to Docker, still labelled,
                // rather than dropped here.
                mem::forget(container);
                failures.push("the start thread ended before it got its container back".to_owned());
            }
        }
        match self.finished.recv_timeout(self.bound) {
            Ok(Ok(())) => {}
            Ok(Err(failure)) => failures.push(failure),
            Err(RecvTimeoutError::Timeout) => failures.push(format!(
                "the start did not finish within {} s; its thread carries on and removes the \
                 container once the start ends",
                self.bound.as_secs()
            )),
            Err(RecvTimeoutError::Disconnected) => {
                failures.push("the start thread ended without a report".to_owned());
            }
        }
        if !failures.is_empty() {
            // `eprintln!` panics when standard error cannot be written, and `drop` must not
            // panic. Standard error is the last place left to report to, so a failed write of
            // this line is dropped.
            let _ = writeln!(
                io::stderr(),
                "test-support: the containers labelled {INSTANCE_LABEL}={} of an abandoned start \
                 may remain: {}",
                self.token,
                failures.join("; ")
            );
        }
    }
}

/// Force-removes, with their anonymous volumes, the containers labelled
/// [`INSTANCE_LABEL`]`=token`, stopped and never-started ones included, and returns how many
/// this call removed.
///
/// Only a container whose label carries exactly `token` is touched. One already gone (404) or
/// already being removed (409), for example by the drop of its testcontainers handle, counts as
/// done but not as removed here. [`sweep`] bounds the listing and the removals.
async fn remove_labelled(docker: &Docker, token: &str) -> Result<usize, String> {
    let removal = async {
        let listed = docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: HashMap::from([(
                    "label".to_owned(),
                    vec![format!("{INSTANCE_LABEL}={token}")],
                )]),
                ..ListContainersOptions::default()
            }))
            .await?;
        let mut removed = 0;
        for summary in listed {
            let owned = summary
                .labels
                .as_ref()
                .and_then(|labels| labels.get(INSTANCE_LABEL))
                .is_some_and(|value| value == token);
            let Some(id) = summary.id.filter(|_| owned) else {
                continue;
            };
            let options = RemoveContainerOptions {
                force: true,
                v: true,
                ..RemoveContainerOptions::default()
            };
            match docker.remove_container(&id, Some(options)).await {
                Ok(()) => removed += 1,
                Err(DockerError::DockerResponseServerError {
                    status_code: 404 | 409,
                    ..
                }) => {}
                Err(err) => return Err(err),
            }
        }
        Ok::<_, DockerError>(removed)
    };
    removal.await.map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::mpsc::{self, TryRecvError};
    use std::task::{Context, Wake, Waker};
    use std::thread;
    use std::time::Duration;

    use testcontainers::bollard::Docker;
    use testcontainers::bollard::container::{
        Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    };
    use testcontainers::bollard::errors::Error as DockerError;
    use testcontainers::core::client::docker_client_instance;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{
        ContainerAsync, ContainerRequest, GenericImage, Image, ImageExt, TestcontainersError,
    };
    use tokio::runtime::Builder;
    use tokio::sync::oneshot;
    use tokio::time::{self, Instant};

    use super::{
        HarnessError, INSTANCE_LABEL, Instance, PANIC_SETTLE_PAUSE, PANIC_SETTLE_ROUNDS, Starting,
        finish_bound, modern_dir, repo_root, start_as, start_with,
    };

    /// An entrypoint no image has: Docker creates the container, then fails to start it.
    pub(crate) const MISSING_ENTRYPOINT: &str = "/carddemo-missing-entrypoint";

    /// How long [`cancelled_start_case`] lets each start run before it drops it: while the start
    /// reaches Docker, and while Docker creates the container, which takes hundreds of
    /// milliseconds on a busy engine.
    const CANCEL_CUTS: [Duration; 3] = [
        Duration::from_millis(10),
        Duration::from_millis(40),
        Duration::from_millis(160),
    ];
    /// Interval of an [`Observer`]'s listings and of the waits for a runtime thread; a container
    /// lives far longer between its create and its removal.
    const POLL: Duration = Duration::from_millis(50);

    /// How long [`shutdown_start_case`] runs its runtime after each start it spawns, in order:
    /// long enough for Docker to create and start the first container, then short enough that
    /// Docker is still creating the second one when the runtime drops.
    const SHUTDOWN_CUTS: [Duration; 2] = [Duration::from_millis(1500), Duration::from_millis(30)];
    /// Bound on dropping a runtime, the drops of its cut-off starts included.
    const SHUTDOWN_BOUND: Duration = Duration::from_secs(90);
    /// Wait before a case lists a start's token a second time, for a create Docker would finish
    /// late.
    const RECHECK_AFTER: Duration = Duration::from_secs(3);

    /// How long [`late_create`] waits before it creates its container: past the four seconds of
    /// [`PANIC_SETTLE_ROUNDS`] × [`PANIC_SETTLE_PAUSE`], the longest any sweep looks for a create
    /// Docker finishes late, so only a drop that waits for the start's end sees it go.
    const LATE_CREATE_DELAY: Duration = Duration::from_secs(6);
    /// How long [`late_create_case`] lets its start run before it gives up on it.
    const LATE_CUT: Duration = Duration::from_millis(50);
    /// Bound on the start of [`abandoned_start_case`] sending its outcome, a first pull included.
    const SENT_BOUND: Duration = Duration::from_secs(180);

    /// What [`cancelled_start_case`] saw of one start it dropped part-way.
    #[derive(Debug)]
    struct Watch {
        cut: Duration,
        /// Whether the start was still running when its future was dropped.
        cut_off: bool,
        /// Whether a container carried the token at any of the [`Observer`]'s listings.
        seen: bool,
        /// The containers carrying the token the moment the drop had returned.
        left: Result<Vec<String>, HarnessError>,
    }

    /// How a case gives up on a start.
    #[derive(Clone, Copy, Debug)]
    enum GiveUp {
        /// A timeout on the calling runtime drops the start's future.
        Timeout,
        /// The start runs as a task of a runtime of its own, multi-thread when `multi_thread`,
        /// which then drops, as a test returning after a timed-out start does.
        Shutdown { multi_thread: bool },
    }

    /// A task for [`run_then_shut_down`].
    type Task = Pin<Box<dyn Future<Output = ()> + Send>>;

    /// Lists the containers carrying each of a set of tokens every [`POLL`], from a thread and
    /// runtime of its own, so that it keeps listing while a test's thread blocks in the drop of
    /// a start, until [`finish`](Self::finish).
    struct Observer {
        stop: mpsc::Sender<()>,
        seen: oneshot::Receiver<Vec<bool>>,
    }

    impl Observer {
        /// Starts watching `tokens`.
        fn watch(tokens: Vec<String>) -> Result<Self, HarnessError> {
            let (stop, stopped) = mpsc::channel::<()>();
            let (report, seen) = oneshot::channel();
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| HarnessError::new("build observer runtime", &err))?;
            thread::Builder::new()
                .name("test-support-observer".to_owned())
                .spawn(move || {
                    runtime.block_on(async move {
                        let docker = docker().await;
                        let mut seen = vec![false; tokens.len()];
                        loop {
                            if let Ok(docker) = &docker {
                                for (token, seen) in tokens.iter().zip(seen.iter_mut()) {
                                    *seen |= labelled(docker, token)
                                        .await
                                        .is_ok_and(|ids| !ids.is_empty());
                                }
                            }
                            if !matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                                break;
                            }
                            time::sleep(POLL).await;
                        }
                        // The receiver is gone only once the test has stopped waiting.
                        let _ = report.send(seen);
                    });
                })
                .map_err(|err| HarnessError::new("spawn observer thread", &err))?;
            Ok(Self { stop, seen })
        }

        /// Stops the listings after one more round and returns, for each token in order,
        /// whether a container carried it at any listing.
        async fn finish(self) -> Result<Vec<bool>, HarnessError> {
            drop(self.stop);
            self.seen
                .await
                .map_err(|err| HarnessError::new("observe containers", &err))
        }
    }

    /// Wakes by sending on its channel, so a test that polls a future by hand learns when the
    /// future has something to take.
    struct WakeSignal(mpsc::Sender<()>);

    impl Wake for WakeSignal {
        fn wake(self: Arc<Self>) {
            // The receiver is gone only once the test has stopped waiting.
            let _ = self.0.send(());
        }
    }

    /// Spawns each of `tasks` on a new runtime, multi-thread when `multi_thread`, running the
    /// runtime for the task's duration after each spawn, then drops the runtime, which drops the
    /// tasks still running. Returns once that drop has returned, or the reason it was not seen
    /// to return within [`SHUTDOWN_BOUND`].
    async fn run_then_shut_down(
        tasks: Vec<(Duration, Task)>,
        multi_thread: bool,
    ) -> Result<Result<(), &'static str>, HarnessError> {
        let mut builder = if multi_thread {
            Builder::new_multi_thread()
        } else {
            Builder::new_current_thread()
        };
        let runtime = builder
            .enable_all()
            .build()
            .map_err(|err| HarnessError::new("build runtime", &err))?;
        let (dropped, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("test-support-shutdown".to_owned())
            .spawn(move || {
                for (run, task) in tasks {
                    drop(runtime.spawn(task));
                    // The timer is made inside the runtime, which it needs.
                    runtime.block_on(async move { time::sleep(run).await });
                }
                drop(runtime);
                // The receiver is gone only once the test has stopped waiting.
                let _ = dropped.send(());
            })
            .map_err(|err| HarnessError::new("spawn runtime thread", &err))?;

        let deadline = Instant::now() + SHUTDOWN_BOUND;
        Ok(loop {
            match receiver.try_recv() {
                Ok(()) => break Ok(()),
                Err(TryRecvError::Disconnected) => break Err("the runtime thread panicked"),
                Err(TryRecvError::Empty) if Instant::now() >= deadline => {
                    break Err("the runtime did not drop within its bound");
                }
                Err(TryRecvError::Empty) => time::sleep(POLL).await,
            }
        })
    }

    /// Removes each of `ids`, container IDs or names, that Docker still knows, and returns the
    /// outcome of each removal it made.
    async fn remove_leftovers<'a>(
        docker: &Docker,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<Result<(), HarnessError>> {
        let mut removed = Vec::new();
        for id in ids {
            if !matches!(exists(docker, id).await, Ok(false)) {
                removed.push(remove(docker, id).await);
            }
        }
        removed
    }

    /// Another value of `instance`'s identity, for a fake start to own.
    fn same(instance: &Instance) -> Instance {
        Instance {
            token: instance.token.clone(),
            name: instance.name.clone(),
        }
    }

    /// The `postgres:16` image as a bare request, for the containers the fake starts create.
    fn postgres_request() -> ContainerRequest<GenericImage> {
        GenericImage::new("postgres", "16").into()
    }

    /// The Docker client testcontainers is configured with.
    pub(crate) async fn docker() -> Result<Docker, HarnessError> {
        docker_client_instance()
            .await
            .map_err(|err| HarnessError::new("connect to docker", &err))
    }

    /// Whether Docker still knows container `id`.
    pub(crate) async fn exists(docker: &Docker, id: &str) -> Result<bool, HarnessError> {
        match docker.inspect_container(id, None).await {
            Ok(_) => Ok(true),
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(err) => Err(HarnessError::new("inspect container", &err)),
        }
    }

    /// Force-removes container `id`, which the calling test created or failed to see removed.
    pub(crate) async fn remove(docker: &Docker, id: &str) -> Result<(), HarnessError> {
        let options = RemoveContainerOptions {
            force: true,
            v: true,
            ..RemoveContainerOptions::default()
        };
        docker
            .remove_container(id, Some(options))
            .await
            .map_err(|err| HarnessError::new("remove container", &err))
    }

    /// The IDs of the containers labelled with `token`, stopped and never-started ones included.
    async fn labelled(docker: &Docker, token: &str) -> Result<Vec<String>, HarnessError> {
        let listed = docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: HashMap::from([(
                    "label".to_owned(),
                    vec![format!("{INSTANCE_LABEL}={token}")],
                )]),
                ..ListContainersOptions::default()
            }))
            .await
            .map_err(|err| HarnessError::new("list containers", &err))?;
        Ok(listed
            .into_iter()
            .filter_map(|summary| summary.id)
            .collect())
    }

    /// [`labelled`] for each of `tokens`, in order.
    async fn labelled_each(
        docker: &Docker,
        tokens: &[String],
    ) -> Vec<Result<Vec<String>, HarnessError>> {
        let mut listed = Vec::new();
        for token in tokens {
            listed.push(labelled(docker, token).await);
        }
        listed
    }

    /// Drops a start of `request()` after each of [`CANCEL_CUTS`], as a caller's timeout does,
    /// while an [`Observer`] lists each start's token. Checks that every start was still running
    /// when it was dropped, that it went on to create its container, and that no container
    /// carried its token the moment the drop had returned.
    ///
    /// Every observation is collected, and anything left behind removed, before the first
    /// assertion, so a failing check leaks nothing.
    pub(crate) async fn cancelled_start_case(
        request: fn() -> ContainerRequest<GenericImage>,
        context: &'static str,
    ) -> Result<(), HarnessError> {
        let docker = docker().await?;
        let starts: Vec<_> = CANCEL_CUTS
            .into_iter()
            .map(|cut| {
                let request = request();
                let instance = Instance::new(request.image().name());
                (cut, request, instance)
            })
            .collect();
        let observer = Observer::watch(
            starts
                .iter()
                .map(|(_, _, instance)| instance.token.clone())
                .collect(),
        )?;
        let mut watches = Vec::new();
        for (cut, request, instance) in starts {
            let cut_off = time::timeout(cut, start_as(request, context, &instance))
                .await
                .is_err();
            let left = labelled(&docker, &instance.token).await;
            watches.push(Watch {
                cut,
                cut_off,
                seen: false,
                left,
            });
        }
        let seen = observer.finish().await;
        for (watch, seen) in watches.iter_mut().zip(seen.as_deref().unwrap_or_default()) {
            watch.seen = *seen;
        }
        let leftovers_removed = remove_leftovers(
            &docker,
            watches
                .iter()
                .flat_map(|watch| watch.left.as_deref().unwrap_or_default())
                .map(String::as_str),
        )
        .await;

        assert!(seen.is_ok(), "{seen:?}");
        for watch in &watches {
            assert!(
                watch.cut_off,
                "the start ended within {:?}: {watch:?}",
                watch.cut
            );
            assert!(
                watch.seen,
                "the start dropped after {:?} made no container: {watch:?}",
                watch.cut
            );
            assert!(
                watch.left.as_ref().is_ok_and(Vec::is_empty),
                "{watch:?}; removal: {leftovers_removed:?}"
            );
        }
        Ok(())
    }

    /// Spawns starts of `request()` on a new runtime, multi-thread when `multi_thread`, running
    /// it for one of [`SHUTDOWN_CUTS`] after each spawn, then drops that runtime, which drops
    /// the starts still running, as a test returning after a timed-out start does. An
    /// [`Observer`] lists the starts' tokens meanwhile. Checks that the drop returns within
    /// [`SHUTDOWN_BOUND`], that every start went on to create its container, and that no
    /// container carries any of the starts' tokens the moment the drop has returned, nor
    /// [`RECHECK_AFTER`] later.
    ///
    /// Every observation is collected, and anything left behind removed, before the first
    /// assertion, so a failing check leaks nothing.
    pub(crate) async fn shutdown_start_case(
        request: fn() -> ContainerRequest<GenericImage>,
        context: &'static str,
        multi_thread: bool,
    ) -> Result<(), HarnessError> {
        let docker = docker().await?;
        let mut tokens = Vec::new();
        let mut tasks = Vec::new();
        for cut in SHUTDOWN_CUTS {
            let request = request();
            let instance = Instance::new(request.image().name());
            tokens.push(instance.token.clone());
            let task: Task = Box::pin(async move {
                // A start that ends first drops its container here, on the running runtime.
                let _outcome = start_as(request, context, &instance).await;
            });
            tasks.push((cut, task));
        }
        let observer = Observer::watch(tokens.clone())?;

        let shutdown = run_then_shut_down(tasks, multi_thread).await?;
        let at_shutdown = labelled_each(&docker, &tokens).await;
        let seen = observer.finish().await;
        time::sleep(RECHECK_AFTER).await;
        let later = labelled_each(&docker, &tokens).await;
        let leftovers_removed = remove_leftovers(
            &docker,
            at_shutdown
                .iter()
                .chain(&later)
                .flat_map(|listed| listed.as_deref().unwrap_or_default())
                .map(String::as_str),
        )
        .await;

        let flavor = if multi_thread {
            "multi-thread"
        } else {
            "current-thread"
        };
        assert!(shutdown.is_ok(), "{flavor}: {shutdown:?}");
        assert!(
            seen.as_ref()
                .is_ok_and(|seen| seen.len() == tokens.len() && seen.iter().all(|seen| *seen)),
            "{flavor}: a start made no container: {seen:?}"
        );
        assert!(
            at_shutdown
                .iter()
                .all(|listed| listed.as_ref().is_ok_and(Vec::is_empty)),
            "{flavor}: at the shutdown {at_shutdown:?}; removal: {leftovers_removed:?}"
        );
        assert!(
            later
                .iter()
                .all(|listed| listed.as_ref().is_ok_and(Vec::is_empty)),
            "{flavor}: {RECHECK_AFTER:?} later {later:?}; removal: {leftovers_removed:?}"
        );
        Ok(())
    }

    /// A fake start of `instance`'s container. After [`LATE_CREATE_DELAY`] it creates that
    /// container out of band, labelled and named from `instance` but never started, as a create
    /// Docker finishes after its client went away would leave it, tells `created` the
    /// container's ID or why the create failed, and then fails itself.
    async fn late_create(
        instance: Instance,
        created: mpsc::Sender<Result<String, HarnessError>>,
    ) -> Result<ContainerAsync<GenericImage>, TestcontainersError> {
        time::sleep(LATE_CREATE_DELAY).await;
        let made = match docker().await {
            Ok(docker) => create_unstarted(&docker, &postgres_request(), &instance).await,
            Err(err) => Err(err),
        };
        // The receiver is gone only once the test has stopped waiting; the start's sweep still
        // removes the container.
        let _ = created.send(made);
        Err(TestcontainersError::other(
            "the fake start fails after its late create",
        ))
    }

    /// A fake start that creates `instance`'s container at once, out of band and unstarted,
    /// then panics, as a fault inside testcontainers' start would.
    async fn panicking_create(
        instance: Instance,
    ) -> Result<ContainerAsync<GenericImage>, TestcontainersError> {
        let made = match docker().await {
            Ok(docker) => create_unstarted(&docker, &postgres_request(), &instance).await,
            Err(err) => Err(err),
        };
        panic!("the fake start panics after its create: {made:?}");
    }

    /// Starts [`late_create`], gives up on it after [`LATE_CUT`] as `give_up` says, and checks
    /// that the drop of the start's future returned only after the late create had happened,
    /// that no container carried the start's token or name the moment it had returned, and that
    /// none does [`RECHECK_AFTER`] after both that return and the moment the late create was
    /// due.
    ///
    /// Every observation is collected, and anything left behind removed, before the first
    /// assertion, so a failing check leaks nothing.
    async fn late_create_case(give_up: GiveUp) -> Result<(), HarnessError> {
        const CONTEXT: &str = "start late container";
        let docker = docker().await?;
        let instance = Instance::new("postgres");
        let token = instance.token.clone();
        let name = instance.name.clone();
        let (created_sender, created) = mpsc::channel();
        let starting: Starting = Box::pin(late_create(same(&instance), created_sender));
        let bound = finish_bound(None);

        let began = Instant::now();
        let cut_off = match give_up {
            GiveUp::Timeout => {
                let cut = time::timeout(LATE_CUT, start_with(starting, CONTEXT, &instance, bound));
                if cut.await.is_err() {
                    Ok(())
                } else {
                    Err("the start ended within its cut")
                }
            }
            GiveUp::Shutdown { multi_thread } => {
                let task: Task = Box::pin(async move {
                    let _outcome = start_with(starting, CONTEXT, &instance, bound).await;
                });
                run_then_shut_down(vec![(LATE_CUT, task)], multi_thread).await?
            }
        };
        let made = created.try_recv();
        let at_return = labelled(&docker, &token).await;
        let named_at_return = exists(&docker, &name).await;
        time::sleep_until((began + LATE_CREATE_DELAY).max(Instant::now()) + RECHECK_AFTER).await;
        let later = labelled(&docker, &token).await;
        let named_later = exists(&docker, &name).await;
        let leftovers_removed = remove_leftovers(
            &docker,
            [&at_return, &later]
                .into_iter()
                .flat_map(|listed| listed.as_deref().unwrap_or_default())
                .map(String::as_str)
                .chain([name.as_str()]),
        )
        .await;

        assert!(
            LATE_CREATE_DELAY > PANIC_SETTLE_PAUSE.saturating_mul(PANIC_SETTLE_ROUNDS),
            "the late create must come after every settle window"
        );
        assert!(cut_off.is_ok(), "{give_up:?}: {cut_off:?}");
        assert!(
            made.as_ref().is_ok_and(Result::is_ok),
            "{give_up:?}: the late create had not happened when the drop returned: {made:?}"
        );
        assert!(
            at_return.as_ref().is_ok_and(Vec::is_empty) && matches!(named_at_return, Ok(false)),
            "{give_up:?}: when the drop returned {at_return:?}, {named_at_return:?}; removal: \
             {leftovers_removed:?}"
        );
        assert!(
            later.as_ref().is_ok_and(Vec::is_empty) && matches!(named_later, Ok(false)),
            "{give_up:?}: {RECHECK_AFTER:?} later {later:?}, {named_later:?}; removal: \
             {leftovers_removed:?}"
        );
        Ok(())
    }

    /// Creates, without starting it, a container of `request`'s image labelled and named from
    /// `instance`, as testcontainers does before it starts one. An image the engine does not
    /// have yet is pulled first.
    async fn create_unstarted(
        docker: &Docker,
        request: &ContainerRequest<GenericImage>,
        instance: &Instance,
    ) -> Result<String, HarnessError> {
        let create = || {
            docker.create_container(
                Some(CreateContainerOptions {
                    name: instance.name.clone(),
                    platform: None,
                }),
                Config {
                    image: Some(request.descriptor()),
                    labels: Some(HashMap::from([(
                        INSTANCE_LABEL.to_owned(),
                        instance.token.clone(),
                    )])),
                    ..Config::default()
                },
            )
        };
        let created = match create().await {
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                let image = request.image();
                let _pulled = GenericImage::new(image.name(), image.tag())
                    .pull_image()
                    .await
                    .map_err(|err| HarnessError::new("pull image", &err))?;
                create().await
            }
            created => created,
        };
        created
            .map(|response| response.id)
            .map_err(|err| HarnessError::new("create container", &err))
    }

    /// Starts `request`, whose image has [`MISSING_ENTRYPOINT`], while a bystander container
    /// carries the same label under another token, then checks that the start failed under
    /// `context`, removed exactly its own container, said so, and left the bystander alone.
    ///
    /// Every observation is collected, and the bystander and anything the start left behind
    /// removed, before the first assertion, so a failing check leaks nothing.
    pub(crate) async fn failed_start_case(
        request: ContainerRequest<GenericImage>,
        context: &'static str,
    ) -> Result<(), HarnessError> {
        let docker = docker().await?;
        let instance = Instance::new(request.image().name());
        let bystander = Instance::new("bystander");
        let bystander_id = create_unstarted(&docker, &request, &bystander).await?;

        let started = start_as(request, context, &instance).await;
        let left_behind = labelled(&docker, &instance.token).await;
        let bystander_kept = exists(&docker, &bystander_id).await;
        let bystander_removed = remove(&docker, &bystander_id).await;
        let mut leftovers_removed = Vec::new();
        for id in left_behind.as_deref().unwrap_or_default() {
            leftovers_removed.push(remove(&docker, id).await);
        }

        let Err(err) = started else {
            panic!("{context}: a start with entrypoint {MISSING_ENTRYPOINT} succeeded");
        };
        let text = err.to_string();
        assert!(
            text.starts_with(&format!("{context}: failed to start a container: ")),
            "{text}"
        );
        assert!(text.contains(MISSING_ENTRYPOINT), "{text}");
        assert!(
            text.ends_with(&format!(
                "; the cleanup removed 1 container(s) labelled {INSTANCE_LABEL}={}",
                instance.token
            )),
            "{text}"
        );
        assert!(
            left_behind.as_ref().is_ok_and(Vec::is_empty),
            "{left_behind:?}; removal: {leftovers_removed:?}"
        );
        assert!(
            bystander_kept.as_ref().is_ok_and(|kept| *kept),
            "{bystander_kept:?}"
        );
        assert!(bystander_removed.is_ok(), "{bystander_removed:?}");
        Ok(())
    }

    /// Checks that container `id` of image `image`, held by `guard`, carries an instance label
    /// whose token also ends its name, and that dropping `guard` removes it. A container the drop
    /// left behind is removed before the failure.
    pub(crate) async fn guarded_container_case<G>(
        guard: G,
        id: &str,
        image: &str,
    ) -> Result<(), HarnessError> {
        let docker = docker().await?;
        let inspected = docker.inspect_container(id, None).await;
        drop(guard);
        let kept = exists(&docker, id).await;
        let leftover = if matches!(kept, Ok(false)) {
            None
        } else {
            Some(remove(&docker, id).await)
        };

        let inspected = inspected.map_err(|err| HarnessError::new("inspect container", &err))?;
        let token = inspected
            .config
            .and_then(|config| config.labels)
            .and_then(|mut labels| labels.remove(INSTANCE_LABEL));
        let Some(token) = token else {
            panic!("container {id} carries no {INSTANCE_LABEL} label");
        };
        assert_eq!(
            inspected.name,
            Some(format!("/carddemo-test-{image}-{token}"))
        );
        assert!(matches!(kept, Ok(false)), "{kept:?}; removal: {leftover:?}");
        Ok(())
    }

    /// Starts `request` through [`start_with`], with its testcontainers start held back until
    /// a first poll by hand has left the caller waiting, then lets the start run and drops the
    /// caller's future the moment the start's outcome is sent, before the caller has taken it,
    /// as a caller cut off between that send and its next poll would be. Checks that the drop
    /// gave the container back to the start's thread, which removed it before the drop
    /// returned.
    ///
    /// Every observation is collected, and anything left behind removed, before the first
    /// assertion, so a failing check leaks nothing.
    pub(crate) async fn abandoned_start_case(
        request: ContainerRequest<GenericImage>,
    ) -> Result<(), HarnessError> {
        let docker = docker().await?;
        let instance = Instance::new(request.image().name());
        let (release, held) = mpsc::channel::<()>();
        let start = request
            .with_label(INSTANCE_LABEL, instance.token.as_str())
            .with_container_name(instance.name.as_str())
            .start();
        let starting: Starting = Box::pin(async move {
            // Blocks the start's own runtime, which runs nothing else, until released.
            let _released = held.recv();
            start.await
        });
        let (notify, notified) = mpsc::channel();
        let waker = Waker::from(Arc::new(WakeSignal(notify)));
        let mut caller = Box::pin(start_with(
            starting,
            "start abandoned container",
            &instance,
            finish_bound(None),
        ));
        let first = caller.as_mut().poll(&mut Context::from_waker(&waker));
        let released = release.send(());
        // The caller's only wait is for the outcome, so the wake is the send.
        let sent = notified.recv_timeout(SENT_BOUND);
        drop(caller);
        let left = labelled(&docker, &instance.token).await;
        let named = exists(&docker, &instance.name).await;
        let leftovers_removed = remove_leftovers(
            &docker,
            left.as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .chain([instance.name.as_str()]),
        )
        .await;

        assert!(first.is_pending(), "the start ended at its first poll");
        assert!(released.is_ok(), "{released:?}");
        assert!(sent.is_ok(), "no outcome within {SENT_BOUND:?}: {sent:?}");
        assert!(
            left.as_ref().is_ok_and(Vec::is_empty) && matches!(named, Ok(false)),
            "the abandoned start left {left:?}, {named:?}; removal: {leftovers_removed:?}"
        );
        Ok(())
    }

    #[test]
    fn instances_are_unique_and_valid_container_names() {
        let first = Instance::new("postgres");
        let second = Instance::new("postgres");
        assert_ne!(first.token, second.token);
        assert_eq!(
            first.name,
            format!("carddemo-test-postgres-{}", first.token)
        );
        assert!(
            first
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
            "{}",
            first.name
        );
        let nested = Instance::new("library/postgres");
        assert_eq!(
            nested.name,
            format!("carddemo-test-library-postgres-{}", nested.token)
        );
    }

    #[test]
    fn paths_resolve_inside_the_checkout() {
        let modern = modern_dir();
        assert!(modern.join("Cargo.toml").is_file());
        assert!(modern.join("crates/test-support/Cargo.toml").is_file());
        let root = repo_root();
        assert!(root.join("app/data/ASCII/acctdata.txt").is_file());
        assert!(root.join("app/jcl/DUSRSECJ.jcl").is_file());
    }

    #[test]
    fn display_is_context_then_message() {
        let err = HarnessError::new("start postgres", &"connection refused");
        assert_eq!(err.to_string(), "start postgres: connection refused");
    }

    #[test]
    fn the_finish_bound_covers_every_wait_of_a_start() {
        assert_eq!(
            finish_bound(Some(Duration::from_secs(180))),
            Duration::from_secs(810)
        );
        assert_eq!(finish_bound(None), Duration::from_secs(690));
        assert_eq!(finish_bound(Some(Duration::MAX)), Duration::MAX);
    }

    #[tokio::test]
    async fn a_timeout_waits_for_a_late_create_on_a_current_thread_runtime()
    -> Result<(), HarnessError> {
        late_create_case(GiveUp::Timeout).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_timeout_waits_for_a_late_create_on_a_multi_thread_runtime()
    -> Result<(), HarnessError> {
        late_create_case(GiveUp::Timeout).await
    }

    #[tokio::test]
    async fn a_current_thread_runtime_shutdown_waits_for_a_late_create() -> Result<(), HarnessError>
    {
        late_create_case(GiveUp::Shutdown {
            multi_thread: false,
        })
        .await
    }

    #[tokio::test]
    async fn a_multi_thread_runtime_shutdown_waits_for_a_late_create() -> Result<(), HarnessError> {
        late_create_case(GiveUp::Shutdown { multi_thread: true }).await
    }

    /// The fake start creates its container, then panics. The panic becomes the start's error,
    /// and the container is gone when the error returns.
    #[tokio::test]
    async fn a_start_that_panics_fails_and_removes_its_container() -> Result<(), HarnessError> {
        let docker = docker().await?;
        let instance = Instance::new("postgres");
        let starting: Starting = Box::pin(panicking_create(same(&instance)));
        let started = start_with(
            starting,
            "start panicking container",
            &instance,
            finish_bound(None),
        )
        .await;
        let left = labelled(&docker, &instance.token).await;
        let named = exists(&docker, &instance.name).await;
        let leftovers_removed = remove_leftovers(
            &docker,
            left.as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .chain([instance.name.as_str()]),
        )
        .await;

        let Err(err) = started else {
            panic!("a start that panicked succeeded");
        };
        assert_eq!(
            err.to_string(),
            format!(
                "start panicking container: the start panicked; the cleanup removed 1 \
                 container(s) labelled {INSTANCE_LABEL}={}",
                instance.token
            )
        );
        assert!(
            left.as_ref().is_ok_and(Vec::is_empty) && matches!(named, Ok(false)),
            "{left:?}, {named:?}; removal: {leftovers_removed:?}"
        );
        Ok(())
    }
}
