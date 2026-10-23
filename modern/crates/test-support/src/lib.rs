//! Dev-only test harness for the `CardDemo` workspace: the integration tests of `persistence`,
//! `api`, `messaging` and `batch` get their containers, fixtures, audit index, trace capture and
//! child-process runner here. The crate is listed only under their `[dev-dependencies]`, never
//! as a normal dependency.
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
//! guard drops, or when SIGINT or SIGTERM interrupts the run. Their credentials are the local
//! values `carddemo`/`carddemo`.
//!
//! Every start labels its container `org.carddemo.test-support.instance=<token>` and names it
//! `carddemo-test-<image>-<token>`, with a token no other start shares, and labels it
//! `org.carddemo.test-support.process=<token>` with the token of its process, which every start
//! of that process shares. Docker creates a container before testcontainers hands back the
//! handle that removes it, and a start cut off while Docker creates the container cannot learn
//! whether Docker made it. Each start therefore runs to its end on a thread and Tokio runtime of
//! its own, which nothing outside can cut off, so every request it makes gets Docker's answer.
//! That thread hands the handle to the caller or, when the start failed or the caller gave up on
//! it, removes the containers carrying its own token, and no other. A caller that gives up
//! part-way, through a timeout, a runtime shutdown or a panic, waits in the drop of the start's
//! future until the thread has done so.
//!
//! # Interrupted runs
//!
//! The default action of SIGINT and SIGTERM ends a process without running a destructor, so no
//! guard would remove its container. Before its first start reaches Docker, a process therefore
//! installs a watcher for both signals on a thread and Tokio runtime of its own. The watcher first
//! obtains, on that runtime, the Docker client its cleanup uses, and a start fails without
//! contacting Docker when the watcher cannot be installed or cannot obtain that client. On the
//! first of the two signals the watcher force-removes every container labelled with the process's
//! token. From then on nothing the harness does lets a test end, as the end of the last test would
//! end the run before its cleanup: a later start waits, without contacting Docker, until the
//! process ends, and so do a start whose outcome arrives after the signal, `start_postgres`,
//! `start_rabbitmq`, and the drop of a guard or of an abandoned start. Such a drop holds its
//! thread, and with it any runtime that thread drives, so the cleanup never waits for
//! testcontainers' shared client, which a task of that runtime may keep, and sends its requests
//! from the watcher's runtime. A removal round that Docker does not finish within 10 seconds counts
//! as failed, and the next round starts. The watcher repeats its removal, a quarter of a second
//! apart, until no start is running and one more removal has succeeded after that, so that a
//! container Docker finishes creating after the signal goes too, within 30 seconds in all. It then
//! writes one line to standard error, with the signal, how many containers it removed and what it
//! could not finish, and exits with status 128 plus the signal number: 130 for SIGINT, 143 for
//! SIGTERM. A second of the two signals during that cleanup ends the process at once, with a line
//! saying that containers may remain. A signal the process inherited as ignored, as a background
//! job of a non-interactive shell inherits SIGINT, stays ignored: it does not end the process, so
//! it leaves nothing behind.
//!
//! # Residual containers
//!
//! A container stays behind, labelled with both tokens for removal by hand, only:
//!
//! - when the process ends by SIGKILL or another signal the watcher does not handle, or aborts,
//!   while a guard or a start is alive;
//! - when a start is still running when the cleanup after a signal reaches its bound, or Docker
//!   does not answer that cleanup within it, which the watcher's line reports;
//! - when Docker keeps a start waiting past the bound the drop of an abandoned start waits for,
//!   which standard error reports, and the process ends before Docker answers;
//! - when Docker does not answer a start's cleanup within its bound, which the start error or
//!   standard error reports;
//! - when a request of the start fails at the transport level, for example at the Docker
//!   client's 120-second answer timeout, while Docker still finishes it later, which the start
//!   error reports;
//! - or when a panic inside testcontainers' start drops it while Docker is creating the
//!   container and Docker finishes that create more than four seconds later.
//!
//! `docker rm -f -v $(docker ps -aq --filter label=org.carddemo.test-support.process=<token>)`
//! removes what one process left; the filter `label=org.carddemo.test-support.instance=<token>`
//! narrows it to one start.
//!
//! # Child processes
//!
//! [`run_child`] runs a child process of a test, such as `carddemo-batch`, with its output
//! captured, for at most a deadline, [`CHILD_DEADLINE`] unless the test chooses another. A child
//! still running at the deadline, or when the test gives up on the run through a panic, a
//! timeout or a runtime shutdown, is killed and waited for, so that no process outlives its run.
//!
//! # Modules
//!
//! - [`audit_index`]: the audit-table index and the scanners for test tags and rule citations.
//! - [`fixtures`]: fixture paths read in place, fixture loading and redaction needles.
//! - [`postgres`]: a `postgres:16` container migrated through the `runtime` migration entrypoint.
//! - [`rabbitmq`]: a `rabbitmq:3.13-management` container with the broker definitions mounted,
//!   and [`rabbitmq::ConfirmGate`], which withholds its publisher confirmations until a test
//!   settles them.
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
use std::ffi::c_int;
use std::fmt;
use std::fs;
use std::future::{self, Future};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{self, Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{LazyLock, OnceLock};
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
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::oneshot;
use tokio::time;

/// Label on every container this crate starts. Its value is the token of the start that created
/// the container, so a cleanup finds exactly that start's containers.
const INSTANCE_LABEL: &str = "org.carddemo.test-support.instance";

/// Label on every container this crate starts. Its value is the token of the process that
/// started it ([`process_token`]), which every start of the process shares, so the cleanup after
/// an interrupting signal ([`drain`]) finds exactly this process's containers.
const PROCESS_LABEL: &str = "org.carddemo.test-support.process";

/// Bound on one [`sweep`]: the listings of a start's containers and every removal they make.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// The signals the watcher handles ([`watch_signals`]), by name: the two whose default action
/// ends a test run, an interactive interrupt and a runner's or timeout's termination request.
const HANDLED_SIGNALS: [(&str, SignalKind); 2] = [
    ("SIGINT", SignalKind::interrupt()),
    ("SIGTERM", SignalKind::terminate()),
];
/// Bound on the cleanup after an interrupting signal ([`drain`]): every removal it makes and its
/// wait for the starts still running, three times the 10 seconds a busy engine took to finish
/// the creates an interrupt left in flight.
const INTERRUPT_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on one removal round of that cleanup, a listing and the removals it leads to. A round
/// cut off at it counts as failed and the next one starts, so that a request Docker does not
/// answer, such as the removal of a container whose kill hangs, holds the cleanup up for this
/// long rather than for the client's [`DOCKER_ANSWER_TIMEOUT`]. A third of
/// [`INTERRUPT_CLEANUP_TIMEOUT`], so that two more rounds fit after one cut off.
const INTERRUPT_ROUND_TIMEOUT: Duration = Duration::from_secs(10);
/// Pause between the removals of that cleanup while a start is still running.
const INTERRUPT_SWEEP_PAUSE: Duration = Duration::from_millis(250);
/// The process status file whose `SigIgn:` line names the signals the process ignores.
const PROCESS_STATUS: &str = "/proc/self/status";

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

/// This process's value of [`PROCESS_LABEL`], made at its first use ([`new_process_token`]).
static PROCESS_TOKEN: LazyLock<String> = LazyLock::new(new_process_token);

/// The outcome of installing the signal watcher, which the first start sets ([`watch_signals`]).
static SIGNAL_WATCH: OnceLock<Result<(), String>> = OnceLock::new();

/// Set by the first interrupting signal: from then on no start contacts Docker.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// How many starts have passed the [`STOPPING`] check and not yet ended ([`InFlight`]).
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

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

/// The deadline a test gives each child process it runs through [`run_child`], such as a run of
/// `carddemo-batch`: five minutes.
///
/// The slowest batch run the tests make, a job over the whole fixture set, takes seconds, under
/// coverage instrumentation and on a loaded machine too, so the bound ends only a child that
/// hangs, for example on a lock nobody releases or a store that never answers. It still ends
/// that child, and with it the test, its container and its scratch directory, rather than
/// leaving the run blocked until someone kills it.
pub const CHILD_DEADLINE: Duration = Duration::from_secs(300);

/// How often [`run_child`] looks whether its child has exited and its output has closed.
const CHILD_POLL: Duration = Duration::from_millis(10);

/// Runs `command` as [`Command::output`] does, for at most `deadline`, and returns the child's
/// exit status and everything it wrote.
///
/// The child's standard input is null, and its standard output and standard error are pipes,
/// each read to its end on a thread of its own, so a child that fills one while the other is
/// read never blocks. Nothing else of `command` changes: the child inherits the test's
/// environment, the coverage profile variables included, and a variable the caller set, such
/// as `LLVM_PROFILE_FILE=/dev/null` for a run under a file-size limit, reaches it as set.
///
/// The run is done once the child has exited and both pipes have closed. A run not done at
/// `deadline` first kills (`SIGKILL`) and waits for its child, unless the child has already
/// exited, so that no process is left behind, and then fails with an error of kind
/// [`io::ErrorKind::TimedOut`] that names the program and the deadline, never the arguments or
/// the output (Cardholder Data Redaction). A run whose future is dropped before it is done, by a
/// panic, an outer timeout or a runtime shutdown, kills and waits for its child in the drop the
/// same way. Neither waits for a pipe that a process the child started holds open: the thread
/// reading it ends when that process closes it.
///
/// # Errors
///
/// The error of spawning the child, of reading its status or of reading its output, or the
/// timeout above.
pub async fn run_child(mut command: Command, deadline: Duration) -> io::Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Reaper {
        child: command.spawn()?,
        program,
        killed: false,
        status: None,
    };
    let stdout = read_pipe(child.child.stdout.take(), &child.program, "standard output")?;
    let stderr = read_pipe(child.child.stderr.take(), &child.program, "standard error")?;
    let Ok(status) = time::timeout(deadline, child.finish([&stdout, &stderr])).await else {
        let message = match child.reap() {
            Ok(true) => format!(
                "{} did not finish within {deadline:?} and was killed",
                child.program
            ),
            Ok(false) => format!(
                "{} exited, but its output did not close within {deadline:?}",
                child.program
            ),
            Err(err) => format!(
                "{} did not finish within {deadline:?} and could not be killed: {err}",
                child.program
            ),
        };
        return Err(io::Error::new(io::ErrorKind::TimedOut, message));
    };
    Ok(Output {
        status: status?,
        stdout: collected(stdout, &child.program, "standard output")?,
        stderr: collected(stderr, &child.program, "standard error")?,
    })
}

/// A thread reading one output pipe of a [`run_child`] child to its end.
type PipeReader = thread::JoinHandle<io::Result<Vec<u8>>>;

/// The child of a [`run_child`] run, which it kills and waits for unless a wait has already read
/// its exit status.
struct Reaper {
    /// The child process.
    child: Child,
    /// The program the child runs, which a failure names.
    program: String,
    /// Whether a kill reached the child, which is then not signalled again.
    killed: bool,
    /// The child's exit status once a wait has read it. From then on its process ID may name
    /// another process, so nothing signals it.
    status: Option<ExitStatus>,
}

impl Reaper {
    /// Waits, looking every [`CHILD_POLL`], until the child has exited and every one of `pipes`
    /// has read its pipe to the end, and returns the child's exit status.
    async fn finish(&mut self, pipes: [&PipeReader; 2]) -> io::Result<ExitStatus> {
        loop {
            if self.status.is_none() {
                self.status = self.child.try_wait()?;
            }
            if let Some(status) = self.status {
                if pipes.iter().all(|pipe| pipe.is_finished()) {
                    return Ok(status);
                }
            }
            time::sleep(CHILD_POLL).await;
        }
    }

    /// Kills the child and waits for it, unless a wait has already read its exit status, and
    /// returns whether it killed it.
    fn reap(&mut self) -> io::Result<bool> {
        if self.status.is_some() {
            return Ok(false);
        }
        if !self.killed {
            self.child.kill()?;
            self.killed = true;
        }
        self.status = Some(self.child.wait()?);
        Ok(true)
    }
}

impl Drop for Reaper {
    /// Kills the child and waits for it unless a wait has already read its exit status: its run
    /// was dropped, or failed, before the child finished. A child that cannot be killed or waited
    /// for is reported on standard error, as `drop` cannot return the failure.
    fn drop(&mut self) {
        if let Err(err) = self.reap() {
            // `eprintln!` panics when standard error cannot be written, and `drop` must not
            // panic. Standard error is the last place left to report to, so a failed write of
            // this line is dropped.
            let _ = writeln!(
                io::stderr(),
                "test-support: {} may still be running: {err}",
                self.program
            );
        }
    }
}

/// Starts a thread that reads `pipe`, the child's `stream` (standard output or standard error),
/// to its end. A `pipe` that is missing, which only a stream that is not piped leaves, fails.
fn read_pipe<R: Read + Send + 'static>(
    pipe: Option<R>,
    program: &str,
    stream: &str,
) -> io::Result<PipeReader> {
    let Some(mut pipe) = pipe else {
        return Err(io::Error::other(format!(
            "the {stream} of {program} is not piped"
        )));
    };
    thread::Builder::new()
        .name("test-support-child-output".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
}

/// What `reader`, which has finished, read from the child's `stream`, or why that failed.
fn collected(reader: PipeReader, program: &str, stream: &str) -> io::Result<Vec<u8>> {
    reader.join().unwrap_or_else(|_| {
        Err(io::Error::other(format!(
            "the thread reading the {stream} of {program} panicked"
        )))
    })
}

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

/// This process's token, its value of [`PROCESS_LABEL`].
fn process_token() -> &'static str {
    PROCESS_TOKEN.as_str()
}

/// A fresh process token: the process ID, the nanoseconds since the UNIX epoch and a randomly
/// keyed hash of the two, so that no two processes on one Docker engine share one. It starts
/// with the process ID, as every [`Instance`] token does.
fn new_process_token() -> String {
    let process = process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let salt = RandomState::new().hash_one((process, nanos));
    format!("{process:x}-{nanos:x}-{salt:016x}")
}

/// Starts `request` as a container this start owns, labelled and named from a fresh
/// [`Instance`] and labelled with this process's token, and returns the testcontainers handle
/// whose drop removes it.
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
/// - SIGINT or SIGTERM, from the start's first poll on, removes the container with every other
///   container of this process and ends the process ([`watch_signals`]); a start that polls
///   first after the signal, or whose outcome arrives after it, never returns, and the first
///   does not contact Docker.
///
/// A container stays behind only in the cases the crate documentation lists under "Residual
/// containers".
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
        .with_label(PROCESS_LABEL, process_token())
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
/// That poll first installs the process's signal watcher ([`watch_signals`]) and counts the
/// start in [`IN_FLIGHT`] until its thread ends. It fails without contacting Docker when the
/// watcher cannot be installed. Once a signal has stopped the run, a start that has not yet
/// counted itself in waits, without contacting Docker, for the watcher to end the process, and
/// so does a caller whose outcome arrives after the signal ([`unless_stopping`]).
async fn start_with(
    starting: Starting,
    context: &'static str,
    instance: &Instance,
    bound: Duration,
) -> Outcome {
    if let Err(failure) = watch_signals() {
        return Err(HarnessError::new(context, failure));
    }
    let Some(in_flight) = InFlight::enter() else {
        // Returning would let the test, and with it the run, end before the cleanup.
        return future::pending().await;
    };
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
            // Counted out only now, when every create the start made has returned. A spawn that
            // fails drops the closure, and the count with it.
            drop(in_flight);
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
    let outcome = pending.wait(context).await;
    // The start's thread, and with it the count, ends only once this side has let go.
    drop(pending);
    unless_stopping(outcome).await
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
        let mut removed = 0;
        let mut outcome = remove_labelled(&docker, INSTANCE_LABEL, token, &mut removed).await;
        if panicked {
            for _ in 0..PANIC_SETTLE_ROUNDS {
                if outcome.is_ok() && removed > 0 {
                    break;
                }
                time::sleep(PANIC_SETTLE_PAUSE).await;
                outcome = remove_labelled(&docker, INSTANCE_LABEL, token, &mut removed).await;
            }
        }
        outcome.map(|()| removed)
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
    /// standard error, as `drop` cannot return it. The drop never panics. Once a signal has
    /// stopped the run, it then holds the thread until the process ends ([`hold_if_stopping`]).
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
        hold_if_stopping();
    }
}

/// Force-removes, with their anonymous volumes, the containers labelled `label=token`, where
/// `label` is [`INSTANCE_LABEL`] or [`PROCESS_LABEL`], stopped and never-started ones included,
/// and adds each removal Docker confirms to `removed` as its answer arrives, so that a caller
/// that cuts the call off, or sees it fail part-way, still counts what it removed.
///
/// Only a container whose `label` carries exactly `token` is touched. One already gone (404) or
/// already being removed (409), for example by the drop of its testcontainers handle, counts as
/// done but not as removed here. [`sweep`] and [`drain`] bound the listing and the removals.
async fn remove_labelled(
    docker: &Docker,
    label: &str,
    token: &str,
    removed: &mut usize,
) -> Result<(), String> {
    let removal = async {
        let listed = docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: HashMap::from([("label".to_owned(), vec![format!("{label}={token}")])]),
                ..ListContainersOptions::default()
            }))
            .await?;
        for summary in listed {
            let owned = summary
                .labels
                .as_ref()
                .and_then(|labels| labels.get(label))
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
                Ok(()) => *removed += 1,
                Err(DockerError::DockerResponseServerError {
                    status_code: 404 | 409,
                    ..
                }) => {}
                Err(err) => return Err(err),
            }
        }
        Ok::<_, DockerError>(())
    };
    removal.await.map_err(|err| err.to_string())
}

/// Returns `outcome` at once, or, once a signal has stopped the run, never: the calling task
/// then waits for the watcher to end the process, so that its test, and with it the run, cannot
/// end before the watcher has removed this process's containers. The thread stays free for the
/// runtime's other tasks.
pub(crate) async fn unless_stopping<T>(outcome: T) -> T {
    if STOPPING.load(Ordering::SeqCst) {
        future::pending::<()>().await;
    }
    outcome
}

/// Returns at once, or, once a signal has stopped the run, holds the calling thread until the
/// watcher ends the process: the [`unless_stopping`] of a `drop`, which cannot wait otherwise.
pub(crate) fn hold_if_stopping() {
    while STOPPING.load(Ordering::SeqCst) {
        thread::park();
    }
}

/// A start counted in [`IN_FLIGHT`] from before it can contact Docker until its thread ends.
struct InFlight;

impl InFlight {
    /// Counts a start in, or returns `None`, counted out again, once a signal has stopped the
    /// run. The count comes before the check, and the watcher sets [`STOPPING`] before it reads
    /// the count, all sequentially consistent, so either the watcher sees this start or this
    /// start sees the stop.
    fn enter() -> Option<Self> {
        IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        let entered = Self;
        (!STOPPING.load(Ordering::SeqCst)).then_some(entered)
    }
}

impl Drop for InFlight {
    /// Counts the start out.
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One of [`HANDLED_SIGNALS`] as the watcher holds it.
struct Handler {
    /// The signal's name, such as `SIGINT`, which the watcher's line reports.
    name: &'static str,
    /// The signal, whose number sets the exit status.
    kind: SignalKind,
    /// The stream of its deliveries; none for a signal the process inherited as ignored, which
    /// keeps that disposition.
    stream: Option<Signal>,
}

/// Installs the process's signal watcher on its first call and returns the outcome of that
/// installation, on later calls too.
///
/// The watcher runs on a thread and current-thread Tokio runtime of its own ([`watch`]), which
/// report back before this returns, so no start contacts Docker before the watcher holds the
/// client of its cleanup and both signals are handled.
fn watch_signals() -> &'static Result<(), String> {
    SIGNAL_WATCH.get_or_init(install_watcher)
}

/// Spawns the watcher's thread and returns its report: its client and the handlers are in
/// place, or why not.
fn install_watcher() -> Result<(), String> {
    let (report, reported) = mpsc::channel();
    thread::Builder::new()
        .name("test-support-signals".to_owned())
        .spawn(move || watch(&report))
        .map_err(|err| format!("the signal watcher thread did not start: {err}"))?;
    reported
        .recv()
        .unwrap_or_else(|_| Err("the signal watcher thread ended without a report".to_owned()))
}

/// The watcher's thread: obtains the Docker client of its cleanup, installs a handler for each
/// of [`HANDLED_SIGNALS`] the process did not inherit as ignored, reports through `report`, then
/// waits for the first of them and ends the process ([`interrupt`]).
///
/// The client is testcontainers' ([`docker_client_instance`]), obtained here, on this thread's
/// runtime, before the report, and kept for the cleanup ([`drain`]). After a signal the cleanup
/// therefore never waits for testcontainers' shared client lock, which a task on a runtime a held
/// thread drives ([`hold_if_stopping`]) could keep, and the connections it opens run on this
/// runtime, which no drop holds. When the client cannot be obtained, the report carries the
/// reason, the installation fails, and so does every start, without contacting Docker. The
/// client comes before the handlers, as a handler stays registered for the life of the process
/// once installed: that failure then leaves both signals at their default action.
fn watch(report: &mpsc::Sender<Result<(), String>>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            // The installing start waits for this report.
            let _ = report.send(Err(format!(
                "the signal watcher runtime did not start: {err}"
            )));
            return;
        }
    };
    runtime.block_on(async {
        let docker = match docker_client_instance().await {
            Ok(docker) => docker,
            Err(err) => {
                let _ = report.send(Err(format!(
                    "the signal watcher's Docker client could not be obtained: {err}"
                )));
                return;
            }
        };
        // The handlers need the runtime's signal driver, so they are made inside it.
        let mut handlers = match handlers() {
            Ok(handlers) => handlers,
            Err(failure) => {
                let _ = report.send(Err(failure));
                return;
            }
        };
        let _ = report.send(Ok(()));
        let first = next_signal(&mut handlers).await;
        interrupt(&docker, first, &mut handlers).await;
    });
}

/// A [`Handler`] for each of [`HANDLED_SIGNALS`], with no stream for a signal that
/// [`PROCESS_STATUS`] shows as ignored. A status file that cannot be read shows none as ignored.
fn handlers() -> Result<[Handler; 2], String> {
    let status = fs::read_to_string(PROCESS_STATUS).unwrap_or_default();
    let [first, second] = HANDLED_SIGNALS.map(|(name, kind)| -> Result<Handler, String> {
        let stream = if ignored_signal(&status, kind.as_raw_value()) {
            None
        } else {
            Some(signal(kind).map_err(|err| format!("the {name} handler did not install: {err}"))?)
        };
        Ok(Handler { name, kind, stream })
    });
    Ok([first?, second?])
}

/// Whether `status`, the text of [`PROCESS_STATUS`], shows signal `number` as ignored: its
/// `SigIgn:` line is a hexadecimal mask whose bit `number - 1` stands for signal `number`. A text
/// without that line, or whose mask is not hexadecimal, shows no signal as ignored.
fn ignored_signal(status: &str, number: c_int) -> bool {
    let Some(mask) = status
        .lines()
        .find_map(|line| line.strip_prefix("SigIgn:"))
        .map(str::trim)
    else {
        return false;
    };
    let Some(bit) = number
        .checked_sub(1)
        .and_then(|bit| u32::try_from(bit).ok())
    else {
        return false;
    };
    mask.chars().all(|c| c.is_ascii_hexdigit())
        && u64::from_str_radix(mask, 16)
            .ok()
            .and_then(|mask| mask.checked_shr(bit))
            .is_some_and(|mask| mask & 1 == 1)
}

/// Waits for the next delivery of either handled signal and returns its name and kind.
async fn next_signal([first, second]: &mut [Handler; 2]) -> (&'static str, SignalKind) {
    tokio::select! {
        delivered = arrival(first) => delivered,
        delivered = arrival(second) => delivered,
    }
}

/// Waits for the next delivery of `handler`'s signal and returns its name and kind. It never
/// returns for a signal the process inherited as ignored, nor once the stream has ended.
async fn arrival(handler: &mut Handler) -> (&'static str, SignalKind) {
    if let Some(stream) = handler.stream.as_mut() {
        if stream.recv().await.is_some() {
            return (handler.name, handler.kind);
        }
    }
    future::pending().await
}

/// Ends the process after `first`, the first signal delivered: sets [`STOPPING`], removes this
/// process's containers through `docker`, the watcher's client ([`drain`]), writes one line to
/// standard error and exits with 128 plus the signal's number. A second signal during the
/// removals ends it at once, with that signal's status.
async fn interrupt(
    docker: &Docker,
    first: (&'static str, SignalKind),
    handlers: &mut [Handler; 2],
) {
    let (name, kind) = first;
    STOPPING.store(true, Ordering::SeqCst);
    let (line, kind) = tokio::select! {
        drained = drain(docker, process_token()) => (format!("{name} received; {drained}"), kind),
        (again, again_kind) = next_signal(handlers) => (
            format!(
                "{again} received during the cleanup after {name}; containers labelled \
                 {PROCESS_LABEL}={} may remain",
                process_token()
            ),
            again_kind,
        ),
    };
    let status = 128_i32.saturating_add(kind.as_raw_value());
    // Standard error is the only place left to report to, and the process ends next, so a failed
    // write of this line is dropped.
    let _ = writeln!(
        io::stderr(),
        "test-support: {line}; exiting with status {status}"
    );
    process::exit(status)
}

/// The cleanup after an interrupting signal, once [`STOPPING`] is set: removes, through
/// `docker`, the client the watcher obtained when it installed ([`watch`]), the containers
/// labelled [`PROCESS_LABEL`]`=token`, then again every [`INTERRUPT_SWEEP_PAUSE`] until a
/// removal round has succeeded after the last running start ([`IN_FLIGHT`]) ended, within
/// [`INTERRUPT_CLEANUP_TIMEOUT`] in all. Returns what to report: how many containers it removed
/// and what it could not finish.
///
/// A start that has ended has had an answer to every create it made, so that removal finds
/// every container a start of this process made, the ones Docker created after the signal
/// included.
///
/// Each round is bounded by [`INTERRUPT_ROUND_TIMEOUT`]: a round cut off there counts as failed,
/// and the next one starts over a new connection, as the Docker client keeps no idle ones. The
/// count covers every removal Docker confirmed, a cut-off round's included ([`remove_labelled`]);
/// a removal whose answer the cut cost is not counted, although Docker carries it out.
async fn drain(docker: &Docker, token: &str) -> String {
    let label = format!("{PROCESS_LABEL}={token}");
    let mut removed = 0;
    let mut last: Result<(), String> = Ok(());
    let drained = time::timeout(INTERRUPT_CLEANUP_TIMEOUT, async {
        loop {
            // Read before the removal, so that a zero means the removal runs after every start
            // has ended.
            let running = IN_FLIGHT.load(Ordering::SeqCst);
            let round = remove_labelled(docker, PROCESS_LABEL, token, &mut removed);
            last = time::timeout(INTERRUPT_ROUND_TIMEOUT, round)
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "a removal round did not finish within {} s",
                        INTERRUPT_ROUND_TIMEOUT.as_secs()
                    ))
                });
            if last.is_ok() && running == 0 {
                return;
            }
            time::sleep(INTERRUPT_SWEEP_PAUSE).await;
        }
    })
    .await;
    let mut notes = vec![format!("removed {removed} container(s) labelled {label}")];
    if drained.is_err() {
        notes.push(format!(
            "the cleanup did not finish within {} s, with {} start(s) still running",
            INTERRUPT_CLEANUP_TIMEOUT.as_secs(),
            IN_FLIGHT.load(Ordering::SeqCst)
        ));
        if let Err(failure) = last {
            notes.push(format!("the last removal failed: {failure}"));
        }
        notes.push(format!("containers labelled {label} may remain"));
    }
    notes.join("; ")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::fs;
    use std::io::{self, BufRead, BufReader, Read};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::process::{self, Child, ChildStderr, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, TryRecvError};
    use std::task::{Context, Wake, Waker};
    use std::thread;
    use std::time::Duration;

    use testcontainers::bollard::API_DEFAULT_VERSION;
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
    use tokio::runtime::{Builder, Handle};
    use tokio::signal::unix::SignalKind;
    use tokio::sync::oneshot;
    use tokio::time::{self, Instant};

    use super::{
        CHILD_DEADLINE, DOCKER_ANSWER_TIMEOUT, HANDLED_SIGNALS, HarnessError, INSTANCE_LABEL,
        INTERRUPT_CLEANUP_TIMEOUT, INTERRUPT_ROUND_TIMEOUT, Instance, PANIC_SETTLE_PAUSE,
        PANIC_SETTLE_ROUNDS, PROCESS_LABEL, PROCESS_STATUS, STOPPING, Starting, drain,
        finish_bound, ignored_signal, modern_dir, process_token, repo_root, run_child, start_as,
        start_with,
    };
    use crate::postgres::start_postgres;

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

    /// Environment variable that turns [`an_interrupt_child_holds_containers_until_its_signal`]
    /// from a no-op into the child process of [`interrupted_run_case`].
    const INTERRUPT_CHILD: &str = "CARDDEMO_TEST_SUPPORT_INTERRUPT_CHILD";
    /// That child test's name in this test binary.
    const INTERRUPT_CHILD_TEST: &str =
        "tests::an_interrupt_child_holds_containers_until_its_signal";
    /// Precedes the process token the child prints once its containers are in place.
    const CHILD_READY: &str = "test-support interrupt child ready: ";
    /// Bound on the child getting there, a first pull of the image included.
    const CHILD_READY_BOUND: Duration = Duration::from_secs(180);
    /// How long a child must outlive a signal it inherited as ignored.
    const IGNORED_GRACE: Duration = Duration::from_secs(2);
    /// Bound on the child exiting after its signal: the watcher's cleanup bound, and as much
    /// again for the child's runtime and its last removal.
    const CHILD_EXIT_BOUND: Duration = INTERRUPT_CLEANUP_TIMEOUT.saturating_mul(2);
    /// The longest a case keeps its child, and so the bound on [`start_after_stop`]'s wait for
    /// the signal.
    const CHILD_HOLD_BOUND: Duration = CHILD_READY_BOUND
        .saturating_add(IGNORED_GRACE)
        .saturating_add(CHILD_EXIT_BOUND);
    /// Bound on reading a child's standard error once it has exited.
    const OUTPUT_BOUND: Duration = Duration::from_secs(10);

    /// How long a [`run_child`] case lets its child run before it cuts the run off: ample time
    /// for the child's shell to write a process ID.
    const CHILD_CUT: Duration = Duration::from_millis(500);
    /// How long the `sleep` of a [`run_child`] case runs: far past [`CHILD_CUT`].
    const CHILD_SLEEP: Duration = Duration::from_secs(30);
    /// How many zero bytes the child of the [`run_child`] capture case writes to standard error:
    /// past any pipe buffer.
    const CHILD_ZEROS: usize = 200_000;

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
        labelled_by(docker, INSTANCE_LABEL, token).await
    }

    /// The IDs of the containers labelled `label=token`, stopped and never-started ones
    /// included.
    async fn labelled_by(
        docker: &Docker,
        label: &str,
        token: &str,
    ) -> Result<Vec<String>, HarnessError> {
        let listed = docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: HashMap::from([("label".to_owned(), vec![format!("{label}={token}")])]),
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
    /// `instance` and labelled with this process's token, as testcontainers does before it
    /// starts one for [`start_as`], so that an interrupted run removes it too. An image the
    /// engine does not have yet is pulled first.
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
                    labels: Some(HashMap::from([
                        (INSTANCE_LABEL.to_owned(), instance.token.clone()),
                        (PROCESS_LABEL.to_owned(), process_token().to_owned()),
                    ])),
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
    /// whose token also ends its name and the process label of this process, and that dropping
    /// `guard` removes it. A container the drop left behind is removed before the failure.
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
        let mut labels = inspected
            .config
            .and_then(|config| config.labels)
            .unwrap_or_default();
        let Some(token) = labels.remove(INSTANCE_LABEL) else {
            panic!("container {id} carries no {INSTANCE_LABEL} label");
        };
        assert_eq!(
            inspected.name,
            Some(format!("/carddemo-test-{image}-{token}"))
        );
        assert_eq!(
            labels.remove(PROCESS_LABEL).as_deref(),
            Some(process_token()),
            "container {id}: {PROCESS_LABEL}"
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
            .with_label(PROCESS_LABEL, process_token())
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

    /// `postgres:16` running `sleep infinity` in place of the server: a container that runs
    /// until it is removed, and that a start hands over once Docker has started it, as nothing
    /// waits for readiness.
    fn sleeping_request() -> ContainerRequest<GenericImage> {
        GenericImage::new("postgres", "16")
            .with_entrypoint("sleep")
            .with_cmd(["infinity"])
    }

    /// A fake start admitted before the signal whose container Docker creates after it. It tells
    /// `began` at its first poll, waits until a signal has stopped the run, at most
    /// [`CHILD_HOLD_BOUND`], and then starts [`sleeping_request`] as `instance`, labelled as
    /// [`start_as`] labels a start.
    async fn start_after_stop(
        instance: Instance,
        began: oneshot::Sender<()>,
    ) -> Result<ContainerAsync<GenericImage>, TestcontainersError> {
        // The receiver is gone only once the child has stopped waiting for it.
        let _ = began.send(());
        if time::timeout(CHILD_HOLD_BOUND, stopped()).await.is_err() {
            return Err(TestcontainersError::other("no signal stopped the run"));
        }
        sleeping_request()
            .with_label(INSTANCE_LABEL, instance.token.as_str())
            .with_label(PROCESS_LABEL, process_token())
            .with_container_name(instance.name.as_str())
            .start()
            .await
    }

    /// Returns once a signal has stopped the run.
    async fn stopped() {
        while !STOPPING.load(Ordering::SeqCst) {
            time::sleep(POLL).await;
        }
    }

    /// `signal`, a name of [`HANDLED_SIGNALS`], without its `SIG` prefix, as `kill -s` and the
    /// shell's `trap` take it.
    fn short_name(signal: &str) -> &str {
        signal.strip_prefix("SIG").unwrap_or(signal)
    }

    /// Sends `signal`, a name of [`HANDLED_SIGNALS`], to process `pid` through `kill`.
    fn send_signal(pid: u32, signal: &str) -> Result<(), HarnessError> {
        let name = short_name(signal);
        let status = Command::new("kill")
            .args(["-s", name, pid.to_string().as_str()])
            .status()
            .map_err(|err| HarnessError::new("send signal", &err))?;
        if status.success() {
            Ok(())
        } else {
            Err(HarnessError::new(
                "send signal",
                &format_args!("kill -s {name} {pid} exited with {status}"),
            ))
        }
    }

    /// Starts this test binary as the child of [`interrupted_run_case`], through `sh` with
    /// `trap ''` for `trapped` first when it names a signal, so that the child inherits that
    /// signal as ignored. Returns the child, the receiver of the process token its ready line
    /// reports, and the receiver of its standard error once that has closed.
    ///
    /// The child's standard input stays open while the returned [`Child`] lives: a case that
    /// ends early drops it, and the child, seeing its input close, ends itself by SIGTERM.
    fn spawn_child(
        trapped: Option<&str>,
    ) -> Result<(Child, mpsc::Receiver<String>, mpsc::Receiver<String>), HarnessError> {
        let binary =
            env::current_exe().map_err(|err| HarnessError::new("find the test binary", &err))?;
        let mut command = match trapped {
            Some(signal) => {
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg(format!(
                        "trap '' {}; exec \"$0\" \"$@\"",
                        short_name(signal)
                    ))
                    .arg(&binary);
                command
            }
            None => Command::new(&binary),
        };
        let mut child = command
            .args([
                "--exact",
                INTERRUPT_CHILD_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(INTERRUPT_CHILD, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| HarnessError::new("spawn the interrupt child", &err))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(HarnessError::new(
                "spawn the interrupt child",
                &"its output is not piped",
            ));
        };
        let (ready_sender, ready) = mpsc::channel();
        thread::Builder::new()
            .name("test-support-child-stdout".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    // The test harness opens the line with the test's name, which a single test
                    // thread prints before the test's own output.
                    if let Some((_, token)) = line.split_once(CHILD_READY) {
                        // The receiver is gone only once the case has stopped waiting.
                        let _ = ready_sender.send(token.to_owned());
                    }
                }
            })
            .map_err(|err| HarnessError::new("spawn the child's stdout reader", &err))?;
        let (errors_sender, errors) = mpsc::channel();
        thread::Builder::new()
            .name("test-support-child-stderr".to_owned())
            .spawn(move || {
                // The receiver is gone only once the case has stopped waiting.
                let _ = errors_sender.send(read_lossy(stderr));
            })
            .map_err(|err| HarnessError::new("spawn the child's stderr reader", &err))?;
        Ok((child, ready, errors))
    }

    /// Everything `stderr` yields until it closes, as text; a read error ends it early, and the
    /// text read so far is what the case reports.
    fn read_lossy(mut stderr: ChildStderr) -> String {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Waits up to [`CHILD_EXIT_BOUND`] for `child` to exit and returns its exit code, none when
    /// a signal ended it. A child still running at the bound, or whose status cannot be read,
    /// is killed, and the error says so.
    async fn wait_exit(child: &mut Child) -> Result<Option<i32>, String> {
        let deadline = Instant::now() + CHILD_EXIT_BOUND;
        loop {
            let failure = match child.try_wait() {
                Ok(Some(status)) => return Ok(status.code()),
                Ok(None) if Instant::now() < deadline => {
                    time::sleep(POLL).await;
                    continue;
                }
                Ok(None) => format!("the child did not exit within {CHILD_EXIT_BOUND:?}"),
                Err(err) => format!("the child's status cannot be read: {err}"),
            };
            let killed = child.kill().and_then(|()| child.wait());
            return Err(format!("{failure}; killed: {killed:?}"));
        }
    }

    /// How [`observe_child`] interrupts its child.
    #[derive(Clone, Copy, Debug)]
    struct Plan {
        /// A signal the child starts with ignored, as `trap ''` in its shell leaves it.
        trapped: Option<&'static str>,
        /// The signal sent once the child is ready.
        first: &'static str,
        /// Whether the child ignores `first`, and so must outlive it.
        ignored: bool,
        /// The signal sent [`IGNORED_GRACE`] after an ignored `first`, when the child handles
        /// one.
        ender: Option<&'static str>,
    }

    /// What [`observe_child`] saw.
    #[derive(Debug)]
    struct Interrupted {
        /// The process token of the child's ready line.
        token: Result<String, String>,
        /// The outcome of each signal sent, in order.
        sent: Vec<Result<(), HarnessError>>,
        /// Whether the child still ran [`IGNORED_GRACE`] after a signal it ignores.
        alive_after_ignored: Option<bool>,
        /// The child's exit code, or why it was not seen to exit.
        exit: Result<Option<i32>, String>,
        /// The child's standard error.
        stderr: Result<String, String>,
        /// The containers carrying the child's process token once it had exited.
        left: Result<Vec<String>, HarnessError>,
        /// The same, [`RECHECK_AFTER`] later.
        later: Result<Vec<String>, HarnessError>,
    }

    /// Runs the child of [`interrupted_run_case`] as `plan` says, waits for it to exit, and
    /// lists and then removes the containers carrying its process token. Returns what it saw and
    /// the outcome of each removal it made. A child that never reports ready gets SIGTERM, so
    /// that a watcher it installed still removes what it made.
    async fn observe_child(
        docker: &Docker,
        plan: Plan,
    ) -> Result<(Interrupted, Vec<Result<(), HarnessError>>), HarnessError> {
        let (mut child, ready, errors) = spawn_child(plan.trapped)?;
        let token = ready
            .recv_timeout(CHILD_READY_BOUND)
            .map_err(|err| format!("no ready line within {CHILD_READY_BOUND:?}: {err}"));
        let mut sent = Vec::new();
        let mut alive_after_ignored = None;
        if token.is_ok() {
            sent.push(send_signal(child.id(), plan.first));
            if plan.ignored {
                time::sleep(IGNORED_GRACE).await;
                alive_after_ignored = Some(matches!(child.try_wait(), Ok(None)));
                if let Some(ender) = plan.ender {
                    sent.push(send_signal(child.id(), ender));
                }
            }
        } else {
            sent.push(send_signal(child.id(), "SIGTERM"));
        }
        let exit = wait_exit(&mut child).await;
        let stderr = errors
            .recv_timeout(OUTPUT_BOUND)
            .map_err(|err| format!("no standard error within {OUTPUT_BOUND:?}: {err}"));
        let (left, later) = if let Ok(token) = &token {
            let left = labelled_by(docker, PROCESS_LABEL, token).await;
            time::sleep(RECHECK_AFTER).await;
            (left, labelled_by(docker, PROCESS_LABEL, token).await)
        } else {
            let unknown = || HarnessError::new("list the child's containers", &"no process token");
            (Err(unknown()), Err(unknown()))
        };
        let removed = remove_leftovers(
            docker,
            [&left, &later]
                .into_iter()
                .flat_map(|listed| listed.as_deref().unwrap_or_default())
                .map(String::as_str),
        )
        .await;
        let seen = Interrupted {
            token,
            sent,
            alive_after_ignored,
            exit,
            stderr,
            left,
            later,
        };
        Ok((seen, removed))
    }

    /// Interrupts a child process holding two containers, a migrated database and one whose start
    /// was admitted before the signal but whose create comes after it, with `signal`, which the
    /// child starts with ignored when `trapped`. Checks that the child exits with 128 plus the
    /// signal's number, that its standard error reports both containers removed, and that no
    /// container carries its process token once it has exited, nor [`RECHECK_AFTER`] later.
    ///
    /// A signal the child ignores, through `trapped` or as one this process ignores and the
    /// child inherits, must leave it running for [`IGNORED_GRACE`]; the other handled signal
    /// then ends it, and the checks apply to that one. When the child ignores both, only the
    /// check that it outlives the signal applies, and [`wait_exit`] kills it.
    ///
    /// Every observation is collected, and anything left behind removed, before the first
    /// assertion, so a failing check leaks nothing.
    async fn interrupted_run_case(
        signal: (&'static str, SignalKind),
        trapped: bool,
    ) -> Result<(), HarnessError> {
        let docker = docker().await?;
        let status = fs::read_to_string(PROCESS_STATUS).unwrap_or_default();
        let inherited = |kind: SignalKind| ignored_signal(&status, kind.as_raw_value());
        let (first, kind) = signal;
        let ignored = trapped || inherited(kind);
        let ender = if ignored {
            HANDLED_SIGNALS
                .into_iter()
                .find(|&(_, other)| other != kind && !inherited(other))
        } else {
            Some(signal)
        };
        let plan = Plan {
            trapped: trapped.then_some(first),
            first,
            ignored,
            ender: ender.filter(|_| ignored).map(|(name, _)| name),
        };
        let (seen, removed) = observe_child(&docker, plan).await?;

        assert!(seen.token.is_ok(), "{plan:?}: {seen:?}");
        assert!(seen.sent.iter().all(Result::is_ok), "{plan:?}: {seen:?}");
        if ignored {
            assert_eq!(seen.alive_after_ignored, Some(true), "{plan:?}: {seen:?}");
        }
        let (Some((ender, ender_kind)), Ok(token)) = (ender, &seen.token) else {
            return Ok(());
        };
        let code = 128_i32.saturating_add(ender_kind.as_raw_value());
        let line = format!(
            "test-support: {ender} received; removed 2 container(s) labelled \
             {PROCESS_LABEL}={token}; exiting with status {code}"
        );
        assert_eq!(seen.exit, Ok(Some(code)), "{plan:?}: {seen:?}");
        assert!(
            seen.stderr
                .as_ref()
                .is_ok_and(|text| text.lines().any(|reported| reported == line)),
            "{plan:?}: expected {line:?}: {seen:?}"
        );
        assert!(
            seen.left.as_ref().is_ok_and(Vec::is_empty)
                && seen.later.as_ref().is_ok_and(Vec::is_empty),
            "{plan:?}: {seen:?}; removal: {removed:?}"
        );
        Ok(())
    }

    /// `/bin/sh` writing its process ID to `pid_file`, then running `sleep` for [`CHILD_SLEEP`]
    /// in its place, under the same ID: a child that outlives every cut a [`run_child`] case
    /// makes.
    fn sleeper(pid_file: &Path) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "echo $$ > \"$0\" && exec sleep {}",
                CHILD_SLEEP.as_secs()
            ))
            .arg(pid_file);
        command
    }

    /// A path in the temporary directory, named after this process and `case`, for a child of a
    /// [`run_child`] case to write a process ID to.
    fn pid_file(case: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "carddemo-test-support-{}-{case}.pid",
            process::id()
        ))
    }

    /// The process ID a child wrote to `pid_file`, which this removes.
    fn recorded_pid(pid_file: &Path) -> Result<u32, HarnessError> {
        let written = fs::read_to_string(pid_file);
        // Best effort: a file left behind holds only a process ID.
        let _ = fs::remove_file(pid_file);
        written
            .map_err(|err| HarnessError::new("read the child's process ID", &err))?
            .trim()
            .parse()
            .map_err(|err| HarnessError::new("read the child's process ID", &err))
    }

    /// Whether a process with ID `pid` exists, through `kill -0` in `/bin/sh`, as on any POSIX
    /// system. It finds every process nobody has waited for yet, a killed one included, so
    /// `false` for a child means it ended and was reaped. It signals nothing, as an ID freed by
    /// a reaped child may already name an unrelated process.
    fn process_exists(pid: u32) -> Result<bool, HarnessError> {
        Command::new("/bin/sh")
            .arg("-c")
            .arg("kill -0 \"$0\" 2> /dev/null")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .map_err(|err| HarnessError::new("look for the child", &err))
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

    /// [`run_child`] returns the child's exit status and both outputs byte for byte, with
    /// standard input null, which `cat` reads to its end at once, and the environment as the
    /// caller left it: inherited, as `PATH` shows, with the variables the caller set. It reads
    /// both pipes at the same time: the child writes [`CHILD_ZEROS`] bytes to standard error,
    /// past any pipe buffer, while its standard output is still open.
    #[tokio::test]
    async fn a_child_run_returns_its_status_and_both_outputs() -> Result<(), HarnessError> {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "cat; printf '%s|%s' \"$CARDDEMO_TEST_SUPPORT_CHILD\" \"$PATH\"; \
                 head -c {CHILD_ZEROS} /dev/zero >&2; printf err >&2; exit 3"
            ))
            .env("CARDDEMO_TEST_SUPPORT_CHILD", "as set");
        let output = run_child(command, CHILD_DEADLINE)
            .await
            .map_err(|err| HarnessError::new("run the child", &err))?;

        assert_eq!(output.status.code(), Some(3), "{:?}", output.status);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("as set|{}", env::var("PATH").unwrap_or_default())
        );
        assert!(
            output.stderr.strip_suffix(b"err").is_some_and(
                |zeros| zeros.len() == CHILD_ZEROS && zeros.iter().all(|&byte| byte == 0)
            ),
            "{} bytes of standard error",
            output.stderr.len()
        );
        Ok(())
    }

    /// A child still running at its deadline is killed and waited for before its run fails with
    /// [`io::ErrorKind::TimedOut`], naming the program and the deadline: once the error has
    /// returned, no process has the child's ID.
    #[tokio::test]
    async fn a_child_past_its_deadline_is_killed_and_reaped() -> Result<(), HarnessError> {
        let pid_file = pid_file("deadline");
        let began = Instant::now();
        let run = run_child(sleeper(&pid_file), CHILD_CUT).await;
        let took = began.elapsed();
        let exists = recorded_pid(&pid_file).and_then(process_exists);

        let Err(err) = run else {
            panic!("a child past its deadline finished: {run:?}");
        };
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert_eq!(
            err.to_string(),
            "/bin/sh did not finish within 500ms and was killed"
        );
        assert!(took >= CHILD_CUT && took < CHILD_SLEEP, "{took:?}");
        assert!(matches!(exists, Ok(false)), "{exists:?}");
        Ok(())
    }

    /// A run whose future is dropped while its child runs, here by an outer timeout, kills and
    /// waits for the child in the drop: once the drop has returned, no process has the child's
    /// ID.
    #[tokio::test]
    async fn a_dropped_run_kills_and_reaps_its_child() -> Result<(), HarnessError> {
        let pid_file = pid_file("dropped");
        let cut = time::timeout(CHILD_CUT, run_child(sleeper(&pid_file), CHILD_DEADLINE)).await;
        let exists = recorded_pid(&pid_file).and_then(process_exists);

        assert!(cut.is_err(), "the run finished before the cut: {cut:?}");
        assert!(matches!(exists, Ok(false)), "{exists:?}");
        Ok(())
    }

    /// A child that exits while a process it started holds its output open ends its run at the
    /// deadline with [`io::ErrorKind::TimedOut`], rather than waiting for that process, which
    /// the run leaves alone and the case then ends.
    #[tokio::test]
    async fn an_output_held_open_past_the_deadline_ends_the_run() -> Result<(), HarnessError> {
        let pid_file = pid_file("held-open");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "sleep {} & echo $! > \"$0\"",
                CHILD_SLEEP.as_secs()
            ))
            .arg(&pid_file);
        let began = Instant::now();
        let run = run_child(command, CHILD_CUT).await;
        let took = began.elapsed();
        // The holder still runs, as the run left it alone, so its ID is still its own.
        let ended = recorded_pid(&pid_file).map(|pid| send_signal(pid, "SIGTERM"));

        let Err(err) = run else {
            panic!("a run whose output stayed open finished: {run:?}");
        };
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert_eq!(
            err.to_string(),
            "/bin/sh exited, but its output did not close within 500ms"
        );
        assert!(took >= CHILD_CUT && took < CHILD_SLEEP, "{took:?}");
        assert!(matches!(ended, Ok(Ok(()))), "{ended:?}");
        Ok(())
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

    #[test]
    fn ignored_signals_are_read_from_the_status_mask() {
        for (status, number, ignored) in [
            (
                "Name:\tx\nSigIgn:\t0000000000000002\nSigCgt:\t0000000000004000\n",
                2,
                true,
            ),
            ("SigIgn:\t0000000000000002\n", 15, false),
            ("SigIgn:\t0000000000004002\n", 15, true),
            ("SigIgn:\t8000000000000000\n", 64, true),
            ("SigIgn:\tffffffffffffffff\n", 0, false),
            ("SigIgn:\tffffffffffffffff\n", 65, false),
            ("Name:\tx\nSigCgt:\t0000000000000002\n", 2, false),
            ("", 2, false),
            ("SigIgn:\t00000000000000zz\n", 2, false),
            ("SigIgn:\t+2\n", 2, false),
            ("SigIgn:\t\n", 2, false),
            ("SigIgn:\t10000000000000000\n", 1, false),
        ] {
            assert_eq!(
                ignored_signal(status, number),
                ignored,
                "{status:?}, signal {number}"
            );
        }
    }

    /// The child process of [`interrupted_run_case`]; a no-op unless [`INTERRUPT_CHILD`] is set.
    ///
    /// It holds a migrated database ([`start_postgres`]) and a start whose container Docker
    /// creates only after the signal ([`start_after_stop`]), prints [`CHILD_READY`] with this
    /// process's token, and waits for the signal. After it, the held-back start's outcome, a new
    /// start and the drop of the guard must each wait for the watcher to end the process: the
    /// first that returns instead fails this test, whose end then ends the run with the test
    /// harness's status rather than the watcher's. A case that ends first closes the child's
    /// standard input, and the child then sends itself SIGTERM, so that it leaves nothing behind
    /// either.
    #[tokio::test]
    async fn an_interrupt_child_holds_containers_until_its_signal() -> Result<(), HarnessError> {
        const CONTEXT: &str = "start sleeping container";
        if env::var_os(INTERRUPT_CHILD).is_none() {
            return Ok(());
        }
        let guard = start_postgres().await?;
        let (began_sender, began) = oneshot::channel();
        let held_back = Instance::new("postgres");
        let in_flight = tokio::spawn(async move {
            let starting: Starting = Box::pin(start_after_stop(same(&held_back), began_sender));
            start_with(starting, CONTEXT, &held_back, finish_bound(None)).await
        });
        began
            .await
            .map_err(|err| HarnessError::new("begin the held-back start", &err))?;
        println!("{CHILD_READY}{}", process_token());

        let (closed_sender, closed) = oneshot::channel();
        thread::Builder::new()
            .name("test-support-child-stdin".to_owned())
            .spawn(move || {
                // Any end of the input, a read error included, means the case is gone.
                let _ = io::copy(&mut io::stdin(), &mut io::sink());
                let _ = closed_sender.send(());
            })
            .map_err(|err| HarnessError::new("spawn the child's stdin reader", &err))?;
        tokio::select! {
            // The sender drops unsent only if its thread panicked; either way the case is gone.
            _ = closed => send_signal(process::id(), "SIGTERM")?,
            () = stopped() => {}
        }
        stopped().await;

        let runtime = Handle::current();
        let (dropped_sender, dropped) = oneshot::channel();
        thread::Builder::new()
            .name("test-support-child-drop".to_owned())
            .spawn(move || {
                // The container handle's own drop needs the runtime, should the guard's return.
                let _entered = runtime.enter();
                drop(guard);
                let _ = dropped_sender.send(());
            })
            .map_err(|err| HarnessError::new("spawn the child's drop thread", &err))?;
        let late = Instance::new("postgres");
        let went_on = tokio::select! {
            outcome = in_flight => format!(
                "the held-back start returned, ok: {}",
                matches!(outcome, Ok(Ok(_)))
            ),
            started = start_as(sleeping_request(), CONTEXT, &late) => {
                format!("a start after the signal returned, ok: {}", started.is_ok())
            }
            _ = dropped => "the drop of the guard returned".to_owned(),
        };
        Err(HarnessError::new("interrupt child", &went_on))
    }

    /// SIGTERM, as a runner's timeout sends it, and SIGINT, as Ctrl-C sends it, each remove every
    /// container of the process, one created after the signal included, and end it with 143 and
    /// 130; a SIGINT the process inherited as ignored leaves it running.
    #[tokio::test]
    async fn an_interrupted_run_leaves_no_container_of_its_process() -> Result<(), HarnessError> {
        let [interrupt, terminate] = HANDLED_SIGNALS;
        for (signal, trapped) in [(terminate, false), (interrupt, false), (interrupt, true)] {
            interrupted_run_case(signal, trapped).await?;
        }
        Ok(())
    }

    /// Against an endpoint that accepts every connection and never answers, the cleanup after a
    /// signal cuts each removal round off at [`INTERRUPT_ROUND_TIMEOUT`], starts the next over a
    /// connection of its own, and ends at [`INTERRUPT_CLEANUP_TIMEOUT`] with a line naming the
    /// cut, rather than waiting out the client's [`DOCKER_ANSWER_TIMEOUT`] on its first request.
    #[tokio::test]
    async fn a_cleanup_round_without_an_answer_is_cut_off_and_the_next_starts()
    -> Result<(), HarnessError> {
        const TOKEN: &str = "unanswered";
        let listener = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .map_err(|err| HarnessError::new("bind the silent endpoint", &err))?;
        let address = listener
            .local_addr()
            .map_err(|err| HarnessError::new("bind the silent endpoint", &err))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let endpoint = thread::Builder::new()
            .name("test-support-silent-endpoint".to_owned())
            .spawn(move || {
                // Every connection stays open, unread and unanswered, until the case ends.
                let mut held = Vec::new();
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => held.push(stream),
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                        Err(_) => break,
                    }
                }
                held.len()
            })
            .map_err(|err| HarnessError::new("spawn the silent endpoint", &err))?;
        let docker = Docker::connect_with_http(
            &format!("tcp://{address}"),
            DOCKER_ANSWER_TIMEOUT.as_secs(),
            API_DEFAULT_VERSION,
        )
        .map_err(|err| HarnessError::new("connect to the silent endpoint", &err))?;
        let began = Instant::now();
        let line = drain(&docker, TOKEN).await;
        let took = began.elapsed();
        stop.store(true, Ordering::SeqCst);
        let connections = endpoint.join();

        let label = format!("{PROCESS_LABEL}={TOKEN}");
        let opening = format!(
            "removed 0 container(s) labelled {label}; the cleanup did not finish within {} s, \
             with ",
            INTERRUPT_CLEANUP_TIMEOUT.as_secs()
        );
        let closing = format!(
            "; the last removal failed: a removal round did not finish within {} s; containers \
             labelled {label} may remain",
            INTERRUPT_ROUND_TIMEOUT.as_secs()
        );
        assert!(
            line.starts_with(&opening) && line.ends_with(&closing),
            "{line:?}"
        );
        assert!(
            took >= INTERRUPT_CLEANUP_TIMEOUT
                && took < INTERRUPT_CLEANUP_TIMEOUT.saturating_add(INTERRUPT_ROUND_TIMEOUT),
            "{took:?}"
        );
        assert!(
            matches!(connections, Ok(rounds) if rounds >= 2),
            "{connections:?}"
        );
        Ok(())
    }
}
