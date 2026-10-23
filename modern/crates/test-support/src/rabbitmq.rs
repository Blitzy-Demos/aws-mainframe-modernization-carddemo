//! A `rabbitmq:3.13-management` testcontainer booted from the same `rabbitmq/rabbitmq.conf` and
//! `rabbitmq/definitions.json` that `compose.yaml` mounts, and the few `lapin` helpers the
//! messaging tests need to publish requests and read replies, errors and dead letters.
//!
//! The definitions file supplies the vhost `/`, the local-only user `carddemo`/`carddemo` and the
//! seven durable queues, the `CARD.DEMO.REQUEST.PAUTH` dead-letter arguments included. There are
//! no exchanges: MQ point-to-point maps to the default exchange, so a queue name is also the
//! routing key.
//!
//! Every broker round trip the helpers make (a connection with its handshake, a connection or
//! channel close, a publication, its confirmation, a declaration, a `basic.get`) is bounded by
//! 10 seconds, or, for a `basic.get` poll, by the deadline of its [`get`] call when that is
//! later, so a broker that stops answering fails the test instead of stalling it. A channel
//! request cut off at its bound is left outstanding on the channel, so the helper closes that
//! channel before it returns the error, and every later request on it fails at once.
//!
//! The helpers never log a message body and never put one into a [`HarnessError`]; errors carry
//! only the `Display` text of the `lapin`, testcontainers, Docker or I/O error behind them, a
//! mount path, the bound that passed, or the label token of the start (Cardholder Data
//! Redaction).
//!
//! # Withheld publisher confirmations
//!
//! A message on its queue is not a confirmed publication: the broker enqueues a message before
//! its `basic.ack` reaches the publisher, so a test that reads the queue when a worker settles a
//! request cannot tell a worker that waited for `Confirmation::Ack` from one that acknowledged,
//! or wrote, as soon as it had sent the message. [`RabbitMqGuard::confirm_gate`] opens a
//! [`ConfirmGate`] for that proof: a relay on a free port of `127.0.0.1` between the clients that
//! connect to [`ConfirmGate::amqp_url`] and the broker. It copies every byte a client sends to
//! the broker unchanged, and reads what the broker sends back as AMQP 0-9-1 frames. A method
//! frame `basic.ack` or `basic.nack` from the broker is a publisher confirmation, as the broker
//! sends neither for anything else. The gate counts it as arrived and holds it, and with it
//! every later frame of that connection, until the test settles it:
//!
//! - [`ConfirmGate::release`] hands the confirmation to the client unchanged;
//! - [`ConfirmGate::refuse`] hands it on as a `basic.nack` for the same delivery tag, the
//!   negative confirmation a broker sends only on an internal error, which nothing else lets a
//!   test provoke.
//!
//! While a confirmation is held, the publication it confirms is on its queue and its publisher
//! has not been told, so whatever the publisher does in that window is done without a
//! confirmation. [`ConfirmGate::arrived`] and [`ConfirmGate::passed`] count the confirmations
//! that reached the gate and those handed on, and [`ConfirmGate::wait_for_arrivals`] waits,
//! bounded, for the next to arrive.
//!
//! Limits of the gate:
//!
//! - Only connections opened to [`ConfirmGate::amqp_url`] pass through it; every other
//!   connection reaches the broker directly and is never held.
//! - It settles confirmation frames, in the order they reach it across all its connections. One
//!   `basic.ack` with `multiple` set confirms several publications and counts as one.
//! - A broker frame whose end octet is not `0xCE`, or whose payload exceeds 1 MiB (eight times
//!   the broker's default `frame_max`, which `rabbitmq/rabbitmq.conf` keeps), ends that
//!   connection, and so does a broker the relay cannot reach within 10 seconds.
//! - Dropping the last handle closes the listener, ends a confirmation still held, which then
//!   never reaches its client, and shuts down every relayed socket, so the broker requeues every
//!   delivery those connections left unsettled.
//! - It logs nothing and keeps no frame beyond the one it forwards, so no message body reaches a
//!   log or an error (Cardholder Data Redaction).

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use lapin::message::BasicGetMessage;
use lapin::options::{BasicGetOptions, BasicPublishOptions, QueueDeclareOptions};
use lapin::publisher_confirm::Confirmation;
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties};
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::{ContainerAsync, ContainerRequest, GenericImage, ImageExt};
use tokio::task;
use tokio::time::{self, Instant};

use crate::{HarnessError, modern_dir};

const IMAGE: &str = "rabbitmq";
const TAG: &str = "3.13-management";
const AMQP_PORT: u16 = 5672;
const MANAGEMENT_PORT: u16 = 15672;

/// The broker's log line once every plugin, the definitions import included, has booted.
const STARTUP_MESSAGE: &str = "Server startup complete";
/// Covers a first pull of the image on a busy host.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(180);

/// Bound on the readiness probe that follows the startup message.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Bound on one probe attempt, so a handshake the broker never answers cannot stall the probe.
const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_RETRY: Duration = Duration::from_millis(500);
/// AMQP 0-9-1 `reply-success`, sent when the probe connection or an abandoned channel closes.
const REPLY_SUCCESS: u16 = 200;

/// Bound on one broker round trip: a connection with its handshake, a connection or channel
/// close, a publication, its confirmation, a queue declaration or a `basic.get`. A `basic.get`
/// poll in [`get`] runs instead to the `get` deadline when that is later.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between `basic.get` polls in [`get`].
const GET_POLL: Duration = Duration::from_millis(50);

/// AMQP 0-9-1 frame type of a method frame.
const METHOD_FRAME: u8 = 1;
/// The octet that ends every AMQP 0-9-1 frame.
const FRAME_END: u8 = 0xCE;
/// Octets of a frame header: the frame type (1), the channel (2) and the payload size (4).
const FRAME_HEADER_LEN: usize = 7;
/// Bound on the payload of one frame a [`ConfirmGate`] relays from the broker: eight times the
/// broker's default `frame_max` of 131,072 octets, which `rabbitmq/rabbitmq.conf` keeps.
const MAX_FRAME_PAYLOAD: usize = 1 << 20;
/// Class ID of the `basic` methods.
const BASIC_CLASS: u16 = 60;
/// Method ID of `basic.ack`. The broker sends it only as a publisher confirmation.
const BASIC_ACK: u16 = 80;
/// Method ID of `basic.nack`, whose arguments have the shape of `basic.ack`'s: the delivery tag,
/// then one octet of bits, `multiple` first. The broker sends it only as a negative publisher
/// confirmation.
const BASIC_NACK: u16 = 120;
/// How often a [`ConfirmGate`]'s listener checks for a client or for its drop, and how often
/// [`ConfirmGate::wait_for_arrivals`] checks the count.
const GATE_POLL: Duration = Duration::from_millis(10);

/// A running broker container and the URLs that reach it.
///
/// Dropping the guard removes the container. Drop it inside the test's Tokio runtime, because
/// the removal runs on that runtime; never keep a guard in a `static`, which outlives it. A run
/// that SIGINT or SIGTERM interrupts removes the container too, while the guard is alive, and a
/// guard dropped after the signal holds its thread until the process ends (see the [crate
/// documentation](crate)).
pub struct RabbitMqGuard {
    amqp_url: String,
    management_url: String,
    container: ContainerAsync<GenericImage>,
}

// Hand-written so that a test printing the guard never prints the credential-bearing AMQP URL.
impl fmt::Debug for RabbitMqGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RabbitMqGuard")
            .field("container_id", &self.container.id())
            .field("management_url", &self.management_url)
            .finish_non_exhaustive()
    }
}

impl Drop for RabbitMqGuard {
    /// Once a signal has stopped the run, holds the dropping thread until the process ends, so
    /// that the test cannot end the run before the cleanup (see the [crate documentation](crate)).
    /// Otherwise the container handle then drops and removes the container.
    fn drop(&mut self) {
        crate::hold_if_stopping();
    }
}

impl RabbitMqGuard {
    /// The AMQP URL of the broker, `amqp://carddemo:carddemo@{host}:{port}/%2f`, in the shape of
    /// `CARDDEMO_AMQP_URL`, so a spawned `carddemo-mq` process can be pointed at it.
    #[must_use]
    pub fn amqp_url(&self) -> &str {
        &self.amqp_url
    }

    /// The management console, `http://{host}:{port}`, for inspecting queues while debugging a
    /// test. User `carddemo`, password `carddemo`.
    #[must_use]
    pub fn management_url(&self) -> &str {
        &self.management_url
    }

    /// Opens a new connection to the broker as user `carddemo`.
    ///
    /// # Errors
    ///
    /// Returns a [`HarnessError`] when the connection or its AMQP handshake fails or does not
    /// complete within 10 seconds.
    pub async fn connect(&self) -> Result<Connection, HarnessError> {
        connect_to(&self.amqp_url).await
    }

    /// Opens a [`ConfirmGate`] in front of this broker: a relay on a free port of `127.0.0.1`
    /// that withholds every publisher confirmation the broker sends until the test settles it
    /// (see the [module documentation](crate::rabbitmq)).
    ///
    /// # Errors
    ///
    /// Returns a [`HarnessError`] when the broker's host or AMQP port cannot be read, when the
    /// host resolves to no address, or when the gate's listener or its thread cannot be set up.
    pub async fn confirm_gate(&self) -> Result<ConfirmGate, HarnessError> {
        let host = self
            .container
            .get_host()
            .await
            .map_err(|err| HarnessError::new("read rabbitmq host", &err))?;
        let port = self
            .container
            .get_host_port_ipv4(AMQP_PORT.tcp())
            .await
            .map_err(|err| HarnessError::new("read rabbitmq amqp port", &err))?;
        let authority = format!("{host}:{port}");
        // Name resolution can block, so it runs on Tokio's blocking pool (see `bind_source`).
        let broker = task::spawn_blocking(move || resolve(&authority))
            .await
            .map_err(|err| HarnessError::new("resolve rabbitmq host", &err))??;
        ConfirmGate::open(broker)
    }
}

/// Opens a connection to `amqp_url` as user `carddemo`, bounded by [`OPERATION_TIMEOUT`].
async fn connect_to(amqp_url: &str) -> Result<Connection, HarnessError> {
    time::timeout(
        OPERATION_TIMEOUT,
        Connection::connect(amqp_url, ConnectionProperties::default()),
    )
    .await
    .map_err(|_| {
        HarnessError::new(
            "connect to rabbitmq",
            &format_args!(
                "the broker did not complete the connection and its handshake within {} ms",
                OPERATION_TIMEOUT.as_millis()
            ),
        )
    })?
    .map_err(|err| HarnessError::new("connect to rabbitmq", &err))
}

/// The addresses of `authority`, the broker's `host:port`. Only the IPv4 ones are kept when
/// there are any, because the port is the one Docker published on IPv4.
fn resolve(authority: &str) -> Result<Vec<SocketAddr>, HarnessError> {
    let addresses: Vec<SocketAddr> = authority
        .to_socket_addrs()
        .map_err(|err| HarnessError::new("resolve rabbitmq host", &err))?
        .collect();
    let ipv4: Vec<SocketAddr> = addresses
        .iter()
        .copied()
        .filter(SocketAddr::is_ipv4)
        .collect();
    let chosen = if ipv4.is_empty() { addresses } else { ipv4 };
    if chosen.is_empty() {
        return Err(HarnessError::new(
            "resolve rabbitmq host",
            &"the host resolved to no address",
        ));
    }
    Ok(chosen)
}

/// Starts a `rabbitmq:3.13-management` container with `modern/rabbitmq/rabbitmq.conf` and
/// `modern/rabbitmq/definitions.json` mounted read-only under `/etc/rabbitmq/`, and returns once
/// user `carddemo` can open an AMQP connection.
///
/// That user exists only through `definitions.json`, so a successful probe also proves the
/// definitions, and with them the seven durable queues, were loaded.
///
/// # Errors
///
/// Returns a [`HarnessError`] when either broker file is missing or its path cannot be resolved
/// or is not UTF-8, when the container does not start or log its startup message within 180
/// seconds, when its ports cannot be read, when user `carddemo` cannot connect within 60 seconds
/// of startup (the attempt under way at that point may take 10 seconds more), or when the broker
/// does not confirm the close of that probe connection within 10 seconds. Every such error
/// removes the container; when Docker created it but the start failed, the error also reports
/// how many containers of this start that removal found, or why it failed.
///
/// Once SIGINT or SIGTERM has interrupted the run, it does not return: the process ends instead
/// (see the [crate documentation](crate)).
pub async fn start_rabbitmq() -> Result<RabbitMqGuard, HarnessError> {
    crate::unless_stopping(start_broker().await).await
}

/// The start of [`start_rabbitmq`]: the mounts, the container, its ports and the readiness
/// probe.
async fn start_broker() -> Result<RabbitMqGuard, HarnessError> {
    let modern = modern_dir();
    let conf = bind_source(modern.join("rabbitmq/rabbitmq.conf")).await?;
    let definitions = bind_source(modern.join("rabbitmq/definitions.json")).await?;

    let container =
        crate::start_owned(request(image(), conf, definitions), "start rabbitmq").await?;

    let host = container
        .get_host()
        .await
        .map_err(|err| HarnessError::new("read rabbitmq host", &err))?;
    let amqp_port = container
        .get_host_port_ipv4(AMQP_PORT.tcp())
        .await
        .map_err(|err| HarnessError::new("read rabbitmq amqp port", &err))?;
    let management_port = container
        .get_host_port_ipv4(MANAGEMENT_PORT.tcp())
        .await
        .map_err(|err| HarnessError::new("read rabbitmq management port", &err))?;

    let amqp_url = format!("amqp://carddemo:carddemo@{host}:{amqp_port}/%2f");
    let management_url = format!("http://{host}:{management_port}");
    wait_until_ready(&amqp_url).await?;

    Ok(RabbitMqGuard {
        amqp_url,
        management_url,
        container,
    })
}

/// The `rabbitmq:3.13-management` image with its ports and startup message. [`request`] adds
/// the rest; the two stay apart so that a test can change what only a `GenericImage` can, such
/// as the entrypoint.
fn image() -> GenericImage {
    GenericImage::new(IMAGE, TAG)
        .with_exposed_port(AMQP_PORT.tcp())
        .with_exposed_port(MANAGEMENT_PORT.tcp())
        .with_wait_for(WaitFor::message_on_stdout(STARTUP_MESSAGE))
}

/// The container request of [`start_rabbitmq`] for `image`: `conf` and `definitions`, the
/// absolute broker file paths of [`bind_source`], mounted read-only, and the startup bound.
fn request(
    image: GenericImage,
    conf: String,
    definitions: String,
) -> ContainerRequest<GenericImage> {
    image
        .with_mount(
            Mount::bind_mount(conf, "/etc/rabbitmq/rabbitmq.conf")
                .with_access_mode(AccessMode::ReadOnly),
        )
        .with_mount(
            Mount::bind_mount(definitions, "/etc/rabbitmq/definitions.json")
                .with_access_mode(AccessMode::ReadOnly),
        )
        .with_startup_timeout(STARTUP_TIMEOUT)
}

/// The absolute UTF-8 form of a broker file, as a Docker bind mount requires.
///
/// Resolving the path reads filesystem metadata, which can block, so it runs on Tokio's blocking
/// pool: on a current-thread runtime the caller's worker is the only thread that drives tasks and
/// timers.
async fn bind_source(path: PathBuf) -> Result<String, HarnessError> {
    let absolute = task::spawn_blocking(move || {
        std::fs::canonicalize(&path).map_err(|err| {
            HarnessError::new(
                "resolve rabbitmq mount",
                &format_args!("{}: {err}", path.display()),
            )
        })
    })
    .await
    .map_err(|err| HarnessError::new("resolve rabbitmq mount", &err))??;
    absolute.to_str().map(str::to_owned).ok_or_else(|| {
        HarnessError::new(
            "resolve rabbitmq mount",
            &format_args!("{} is not valid UTF-8", absolute.display()),
        )
    })
}

/// Retries an AMQP connection as user `carddemo` until one succeeds or [`READY_TIMEOUT`] passes,
/// then closes the probe connection, waiting at most [`OPERATION_TIMEOUT`] for the broker to
/// confirm the close. A probe whose close goes unconfirmed is dropped, already marked closing.
async fn wait_until_ready(amqp_url: &str) -> Result<(), HarnessError> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let attempt = time::timeout(
            PROBE_ATTEMPT_TIMEOUT,
            Connection::connect(amqp_url, ConnectionProperties::default()),
        )
        .await;
        let failure = match attempt {
            Ok(Ok(connection)) => {
                return time::timeout(OPERATION_TIMEOUT, connection.close(REPLY_SUCCESS, "probe"))
                    .await
                    .map_err(|_| {
                        HarnessError::new(
                            "close rabbitmq probe",
                            &format_args!(
                                "the broker did not confirm the close within {} ms",
                                OPERATION_TIMEOUT.as_millis()
                            ),
                        )
                    })?
                    .map_err(|err| HarnessError::new("close rabbitmq probe", &err));
            }
            Ok(Err(err)) => err.to_string(),
            Err(elapsed) => elapsed.to_string(),
        };
        if Instant::now() >= deadline {
            return Err(HarnessError::new("wait for rabbitmq", &failure));
        }
        time::sleep(PROBE_RETRY).await;
    }
}

/// Publishes `body` to `queue` through the default exchange, with `queue` as the routing key.
///
/// On a channel in confirm mode the call returns once the broker has confirmed the message; on
/// any other channel it returns once the message has been sent. Sending and confirmation are
/// bounded by 10 seconds each.
///
/// # Errors
///
/// Returns a [`HarnessError`] when the publication fails or the broker confirms it negatively,
/// and when sending or confirmation takes longer than 10 seconds, in which case the channel is
/// closed first, which waits up to 10 seconds more for the broker.
pub async fn publish(
    channel: &Channel,
    queue: &str,
    body: &[u8],
    properties: BasicProperties,
) -> Result<(), HarnessError> {
    let confirm = within(
        channel,
        "publish",
        Instant::now() + OPERATION_TIMEOUT,
        channel.basic_publish("", queue, BasicPublishOptions::default(), body, properties),
    )
    .await?;
    let confirmation = within(
        channel,
        "publish confirmation",
        Instant::now() + OPERATION_TIMEOUT,
        confirm,
    )
    .await?;
    match confirmation {
        Confirmation::Ack(_) | Confirmation::NotRequested => Ok(()),
        Confirmation::Nack(_) => Err(HarnessError::new(
            "publish confirmation",
            &"the broker refused the message",
        )),
    }
}

/// Takes the next message from `queue`, polling every 50 milliseconds until one arrives or
/// `timeout` passes. The last poll starts at the deadline, so a zero `timeout` polls once.
///
/// Each poll waits for the broker's reply until the deadline or for 10 seconds, whichever is
/// later, so the poll that starts at the deadline still gets a full round trip. The call
/// therefore returns within `timeout` plus 10 seconds; when a poll goes unanswered, closing the
/// channel adds up to 10 seconds more.
///
/// The message is taken with `no_ack`, so a test never leaves an unacknowledged delivery behind.
///
/// # Errors
///
/// Returns a [`HarnessError`] when a `basic.get` fails, for example because `queue` does not
/// exist, which also closes the channel. A poll left unanswered past its bound is an error too,
/// never `None`, because its late reply could still carry a message; the channel is closed so
/// that the reply cannot take a message unseen.
pub async fn get(
    channel: &Channel,
    queue: &str,
    timeout: Duration,
) -> Result<Option<BasicGetMessage>, HarnessError> {
    let deadline = Instant::now() + timeout;
    loop {
        let poll_deadline = deadline.max(Instant::now() + OPERATION_TIMEOUT);
        let message = within(
            channel,
            "get",
            poll_deadline,
            channel.basic_get(queue, BasicGetOptions { no_ack: true }),
        )
        .await?;
        if message.is_some() {
            return Ok(message);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        time::sleep_until(deadline.min(now + GET_POLL)).await;
    }
}

/// Declares a server-named queue for a test's replies, for use as the `reply_to` of a CP00
/// request, and returns its name.
///
/// The queue is neither exclusive nor auto-delete on purpose: the `carddemo-mq` worker publishes
/// the reply from its own connection and may passive-declare the destination, which the broker
/// refuses (`RESOURCE_LOCKED`) for another connection's exclusive queue. The queue goes away with
/// the container.
///
/// # Errors
///
/// Returns a [`HarnessError`] when the declaration fails, and when the broker does not answer it
/// within 10 seconds, in which case the channel is closed first, which waits up to 10 seconds
/// more for the broker.
pub async fn declare_reply_queue(channel: &Channel) -> Result<String, HarnessError> {
    let queue = within(
        channel,
        "declare reply queue",
        Instant::now() + OPERATION_TIMEOUT,
        channel.queue_declare(
            "",
            QueueDeclareOptions {
                durable: false,
                exclusive: false,
                auto_delete: false,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await?;
    Ok(queue.name().as_str().to_owned())
}

/// The number of ready messages in `queue`, read with a passive declare.
///
/// A passive declare of a missing queue closes the channel (AMQP 404), so call this only on the
/// queues of `definitions.json`, such as `CARD.DEMO.DEADLETTER.PAUTH` or `CARD.DEMO.ERROR`.
///
/// # Errors
///
/// Returns a [`HarnessError`] when the declaration fails, for example because `queue` does not
/// exist, and when the broker does not answer it within 10 seconds, in which case the channel is
/// closed first, which waits up to 10 seconds more for the broker.
pub async fn message_count(channel: &Channel, queue: &str) -> Result<u32, HarnessError> {
    let queue = within(
        channel,
        "count queue messages",
        Instant::now() + OPERATION_TIMEOUT,
        channel.queue_declare(
            queue,
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        ),
    )
    .await?;
    Ok(queue.message_count())
}

/// Awaits `operation`, one request on `channel`, until `deadline`, and maps a failure to a
/// [`HarnessError`] under `context`.
///
/// A request cancelled at its deadline, whether a reply never came or a publication's frames
/// were never sent, is left outstanding on the channel: a late `basic.get-ok` would take a
/// message off its queue unseen, and every later synchronous request would wait behind it. The
/// channel is therefore closed before the error returns, waiting at most [`OPERATION_TIMEOUT`]
/// for the broker to confirm. `lapin` marks the channel closing as soon as the close is sent, so
/// every later request on it fails at once even when that confirmation never comes.
async fn within<T>(
    channel: &Channel,
    context: &'static str,
    deadline: Instant,
    operation: impl Future<Output = lapin::Result<T>>,
) -> Result<T, HarnessError> {
    let started = Instant::now();
    let Ok(outcome) = time::timeout_at(deadline, operation).await else {
        let waited = started.elapsed().as_millis();
        let close = time::timeout(
            OPERATION_TIMEOUT,
            channel.close(REPLY_SUCCESS, "harness deadline passed"),
        )
        .await;
        let note = match close {
            Ok(Ok(())) => String::new(),
            Ok(Err(err)) => format!(" (the close itself failed: {err})"),
            Err(_) => format!(
                " (the broker did not confirm the close within {} ms)",
                OPERATION_TIMEOUT.as_millis()
            ),
        };
        return Err(HarnessError::new(
            context,
            &format_args!(
                "the request did not complete within {waited} ms, so the channel was closed{note}"
            ),
        ));
    };
    outcome.map_err(|err| HarnessError::new(context, &err))
}

/// A relay in front of the broker that withholds each publisher confirmation until the test
/// settles it, opened by [`RabbitMqGuard::confirm_gate`] (see the [module
/// documentation](crate::rabbitmq) for what it proves and its limits).
///
/// A worker connected through [`ConfirmGate::amqp_url`] publishes as usual; the broker queues the
/// message and sends its confirmation, which the gate counts ([`ConfirmGate::arrived`]) and holds.
/// The test can then observe that the message is on its queue while the worker has neither
/// acknowledged nor written anything, and settles the confirmation with [`ConfirmGate::release`]
/// or [`ConfirmGate::refuse`]. Settlements apply to confirmations in the order they arrive: the
/// first settlement to the first confirmation, and so on. A settlement made before its
/// confirmation arrives lets that confirmation pass at once.
///
/// Clones share one gate, so a test double can hold one to read [`ConfirmGate::passed`] when the
/// worker calls it. The gate closes when its last clone drops.
#[derive(Clone)]
pub struct ConfirmGate {
    handle: Arc<GateHandle>,
}

/// What the clones of one [`ConfirmGate`] share; its drop closes the gate.
struct GateHandle {
    state: Arc<GateState>,
    port: u16,
    amqp_url: String,
}

impl Drop for GateHandle {
    fn drop(&mut self) {
        self.state.close();
    }
}

// Hand-written, as `RabbitMqGuard`'s, so that a test printing the gate never prints the
// credential-bearing AMQP URL.
impl fmt::Debug for ConfirmGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ledger = self.handle.state.ledger();
        f.debug_struct("ConfirmGate")
            .field("port", &self.handle.port)
            .field("arrived", &ledger.arrived)
            .field("passed", &ledger.passed)
            .finish_non_exhaustive()
    }
}

impl ConfirmGate {
    /// Binds the gate's listener on a free port of `127.0.0.1` and starts the thread that
    /// accepts its clients, each relayed to the first of `broker` that accepts the connection.
    fn open(broker: Vec<SocketAddr>) -> Result<Self, HarnessError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|err| HarnessError::new("open confirm gate", &err))?;
        // Non-blocking, so that the accepting thread notices the gate's drop between clients.
        listener
            .set_nonblocking(true)
            .map_err(|err| HarnessError::new("open confirm gate", &err))?;
        let port = listener
            .local_addr()
            .map_err(|err| HarnessError::new("open confirm gate", &err))?
            .port();
        let state = Arc::new(GateState::default());
        let accepting = Arc::clone(&state);
        thread::Builder::new()
            .name("confirm-gate".to_owned())
            .spawn(move || accept_clients(&listener, &broker, &accepting))
            .map_err(|err| HarnessError::new("open confirm gate", &err))?;
        Ok(Self {
            handle: Arc::new(GateHandle {
                state,
                port,
                amqp_url: format!("amqp://carddemo:carddemo@127.0.0.1:{port}/%2f"),
            }),
        })
    }

    /// The AMQP URL of the gate, `amqp://carddemo:carddemo@127.0.0.1:{port}/%2f`, in the shape
    /// of `CARDDEMO_AMQP_URL`: a connection to it reaches the broker through the gate.
    #[must_use]
    pub fn amqp_url(&self) -> &str {
        &self.handle.amqp_url
    }

    /// Opens a new connection to the broker through the gate as user `carddemo`.
    ///
    /// # Errors
    ///
    /// Returns a [`HarnessError`] when the connection or its AMQP handshake fails or does not
    /// complete within 10 seconds.
    pub async fn connect(&self) -> Result<Connection, HarnessError> {
        connect_to(&self.handle.amqp_url).await
    }

    /// How many publisher confirmations have reached the gate, held or handed on.
    #[must_use]
    pub fn arrived(&self) -> usize {
        self.handle.state.ledger().arrived
    }

    /// How many confirmations the gate has handed on to their client, released or refused. A
    /// confirmation is counted just before its frame is written to the client's socket, so a
    /// client that has received it always finds it counted.
    #[must_use]
    pub fn passed(&self) -> usize {
        self.handle.state.ledger().passed
    }

    /// Waits until at least `count` confirmations have reached the gate, checking every 10
    /// milliseconds without blocking the runtime.
    ///
    /// # Errors
    ///
    /// Returns a [`HarnessError`] naming how many had arrived when `timeout` passes first.
    pub async fn wait_for_arrivals(
        &self,
        count: usize,
        timeout: Duration,
    ) -> Result<(), HarnessError> {
        let deadline = Instant::now() + timeout;
        loop {
            let arrived = self.arrived();
            if arrived >= count {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(HarnessError::new(
                    "wait for a publisher confirmation",
                    &format_args!(
                        "{arrived} of {count} confirmations reached the gate within {} ms",
                        timeout.as_millis()
                    ),
                ));
            }
            time::sleep(GATE_POLL).await;
        }
    }

    /// Settles the earliest unsettled confirmation by handing it to its client unchanged.
    pub fn release(&self) {
        self.handle.state.settle(Settlement::Release);
    }

    /// Settles the earliest unsettled confirmation by handing it to its client as a `basic.nack`
    /// for the same delivery tag: the publisher sees a negative confirmation, although the broker
    /// has queued the message.
    pub fn refuse(&self) {
        self.handle.state.settle(Settlement::Refuse);
    }
}

/// How the test settled one confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settlement {
    Release,
    Refuse,
}

/// The state a gate shares with its relay threads.
#[derive(Default)]
struct GateState {
    ledger: Mutex<Ledger>,
    /// Signalled when a settlement is added and when the gate closes.
    settled: Condvar,
}

/// The counts, the settlements and the sockets of one gate, under [`GateState::ledger`].
#[derive(Default)]
struct Ledger {
    /// Confirmations that have reached the gate; the next one to arrive takes this as its index.
    arrived: usize,
    /// Confirmations handed on to their client.
    passed: usize,
    /// The settlements in the order the test made them, the n-th for the n-th confirmation.
    settlements: Vec<Settlement>,
    /// Set when the last handle of the gate drops.
    closed: bool,
    /// A handle to every relayed socket, for the shut down when the gate closes.
    sockets: Vec<TcpStream>,
}

impl GateState {
    /// The ledger, locked. No code panics while it holds the lock, so a poisoned lock is taken as
    /// it stands.
    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_closed(&self) -> bool {
        self.ledger().closed
    }

    /// Keeps a handle to both sockets of a new relay, or returns `false` when the gate has closed
    /// meanwhile, in which case the relay must end.
    fn keep(&self, client: TcpStream, broker: TcpStream) -> bool {
        let mut ledger = self.ledger();
        if ledger.closed {
            return false;
        }
        ledger.sockets.push(client);
        ledger.sockets.push(broker);
        true
    }

    /// Counts a confirmation that has just arrived and blocks its relay thread until the
    /// settlement of its index exists, then counts it handed on and returns it. `None` when the
    /// gate closes first: the confirmation is never handed on.
    fn hold(&self) -> Option<Settlement> {
        let mut ledger = self.ledger();
        let index = ledger.arrived;
        ledger.arrived += 1;
        loop {
            if ledger.closed {
                return None;
            }
            if let Some(&settlement) = ledger.settlements.get(index) {
                ledger.passed += 1;
                return Some(settlement);
            }
            ledger = self
                .settled
                .wait(ledger)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn settle(&self, settlement: Settlement) {
        self.ledger().settlements.push(settlement);
        self.settled.notify_all();
    }

    /// Closes the gate: its listener stops at its next check, every held confirmation is
    /// dropped, and every relayed socket is shut down, which ends every relay thread.
    fn close(&self) {
        let mut ledger = self.ledger();
        ledger.closed = true;
        for socket in ledger.sockets.drain(..) {
            shut_down(&socket);
        }
        drop(ledger);
        self.settled.notify_all();
    }
}

/// The gate's listener thread: accepts clients until the gate closes, and relays each on
/// threads of its own.
fn accept_clients(listener: &TcpListener, broker: &[SocketAddr], state: &Arc<GateState>) {
    while !state.is_closed() {
        match listener.accept() {
            Ok((client, _)) => {
                let broker = broker.to_vec();
                let relaying = Arc::clone(state);
                // A thread that cannot be spawned drops the client's socket with its closure,
                // which ends that connection; the listener goes on.
                let _ = thread::Builder::new()
                    .name("confirm-gate-relay".to_owned())
                    .spawn(move || relay(client, &broker, &relaying));
            }
            // No client is waiting, or one went away during its accept.
            Err(_) => thread::sleep(GATE_POLL),
        }
    }
}

/// Relays one client connection to the first of `broker` that accepts within 10 seconds: the
/// client's bytes go to the broker verbatim on a thread of its own, and the broker's frames go
/// to the client on this one through [`forward_frames`]. When either direction ends, both
/// sockets are shut down, which ends the other.
fn relay(client: TcpStream, broker: &[SocketAddr], state: &GateState) {
    let Some(upstream) = broker
        .iter()
        .find_map(|address| TcpStream::connect_timeout(address, OPERATION_TIMEOUT).ok())
    else {
        shut_down(&client);
        return;
    };
    let Ok(handles) = relay_handles(&client, &upstream) else {
        shut_down(&client);
        shut_down(&upstream);
        return;
    };
    let [
        mut client_writer,
        mut broker_reader,
        kept_client,
        kept_broker,
    ] = handles;
    if !state.keep(kept_client, kept_broker) {
        shut_down(&client);
        shut_down(&upstream);
        return;
    }
    let (mut client_reader, mut broker_writer) = (client, upstream);
    let copying = thread::Builder::new()
        .name("confirm-gate-copy".to_owned())
        .spawn(move || {
            // Ends at the client's end of stream or at the first failed read or write.
            let _ = io::copy(&mut client_reader, &mut broker_writer);
            shut_down(&client_reader);
            shut_down(&broker_writer);
        });
    if copying.is_ok() {
        // Ends at the broker's end of stream, at a failed read or write, at a malformed frame or
        // when the gate closes while it holds a confirmation.
        let _ = forward_frames(&mut broker_reader, &mut client_writer, state);
    }
    shut_down(&broker_reader);
    shut_down(&client_writer);
}

/// The four handles a relay needs besides the two sockets themselves: the client's writer and
/// the broker's reader for [`forward_frames`], and one handle to each socket for the gate to
/// shut down when it closes. The client's socket is made blocking, whatever the listener's mode
/// left it in, and both sockets send each write at once.
fn relay_handles(client: &TcpStream, broker: &TcpStream) -> io::Result<[TcpStream; 4]> {
    client.set_nonblocking(false)?;
    client.set_nodelay(true)?;
    broker.set_nodelay(true)?;
    Ok([
        client.try_clone()?,
        broker.try_clone()?,
        client.try_clone()?,
        broker.try_clone()?,
    ])
}

/// Shuts `socket` down in both directions; a socket already shut down or reset needs nothing
/// more, so the outcome is not kept.
fn shut_down(socket: &TcpStream) {
    let _ = socket.shutdown(Shutdown::Both);
}

/// Forwards the broker's frames to the client one whole frame at a time. A confirmation is held
/// through [`GateState::hold`] until its settlement, and turned into a `basic.nack` when the
/// settlement refuses it; nothing after it is read until then.
///
/// Returns `Ok(())` at the broker's end of stream and when the gate closes while a confirmation
/// is held, and the error of a read, a write or a malformed frame ([`next_frame`]) otherwise.
fn forward_frames(
    broker: &mut impl Read,
    client: &mut impl Write,
    state: &GateState,
) -> io::Result<()> {
    while let Some(mut frame) = next_frame(broker)? {
        if frame.is_confirmation() {
            match state.hold() {
                Some(Settlement::Release) => {}
                Some(Settlement::Refuse) => frame.refuse(),
                None => return Ok(()),
            }
        }
        client.write_all(&frame.bytes)?;
    }
    Ok(())
}

/// One whole AMQP 0-9-1 frame as the broker sent it: the header, the payload and the frame end.
struct Frame {
    bytes: Vec<u8>,
}

impl Frame {
    /// The class and method IDs of a method frame, `None` for any other frame.
    fn method(&self) -> Option<(u16, u16)> {
        let (&[kind, ..], rest) = self.bytes.split_first_chunk::<FRAME_HEADER_LEN>()?;
        if kind != METHOD_FRAME {
            return None;
        }
        let (_, payload) = rest.split_last()?;
        let &[class_high, class_low, method_high, method_low] = payload.first_chunk::<4>()?;
        Some((
            u16::from_be_bytes([class_high, class_low]),
            u16::from_be_bytes([method_high, method_low]),
        ))
    }

    /// Whether the frame is a publisher confirmation: the method `basic.ack` or `basic.nack`.
    fn is_confirmation(&self) -> bool {
        matches!(self.method(), Some((BASIC_CLASS, BASIC_ACK | BASIC_NACK)))
    }

    /// Makes the method a `basic.nack`. Only the method ID changes, because `basic.nack` carries
    /// the delivery tag and the `multiple` bit where `basic.ack` does, so the frame keeps its
    /// length; a `basic.nack` stays as it is.
    fn refuse(&mut self) {
        let method = self
            .bytes
            .split_first_chunk_mut::<FRAME_HEADER_LEN>()
            .and_then(|(_, payload)| payload.first_chunk_mut::<4>());
        if let Some([_, _, method_high, method_low]) = method {
            let [high, low] = BASIC_NACK.to_be_bytes();
            *method_high = high;
            *method_low = low;
        }
    }
}

/// The next frame of `source`, or `None` at its end of stream before a frame starts.
///
/// # Errors
///
/// [`io::ErrorKind::UnexpectedEof`] when the stream ends inside a frame,
/// [`io::ErrorKind::InvalidData`] when the payload size exceeds [`MAX_FRAME_PAYLOAD`] or the
/// frame does not end with [`FRAME_END`], and the error of any failed read.
fn next_frame(source: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut kind = [0; 1];
    loop {
        match source.read(&mut kind) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let mut rest = [0; FRAME_HEADER_LEN - 1];
    source.read_exact(&mut rest)?;
    let [kind] = kind;
    let [channel_high, channel_low, size_0, size_1, size_2, size_3] = rest;
    let size = usize::try_from(u32::from_be_bytes([size_0, size_1, size_2, size_3]))
        .ok()
        .filter(|size| *size <= MAX_FRAME_PAYLOAD)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "a frame payload exceeds the confirm gate's bound",
            )
        })?;
    let header = [
        kind,
        channel_high,
        channel_low,
        size_0,
        size_1,
        size_2,
        size_3,
    ];
    let mut bytes = Vec::with_capacity(FRAME_HEADER_LEN + size + 1);
    bytes.extend_from_slice(&header);
    bytes.resize(FRAME_HEADER_LEN + size + 1, 0);
    if let Some(body) = bytes.get_mut(FRAME_HEADER_LEN..) {
        source.read_exact(body)?;
    }
    if bytes.last() != Some(&FRAME_END) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a frame does not end with its frame-end octet",
        ));
    }
    Ok(Some(Frame { bytes }))
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor};
    use std::path::Path;
    use std::pin::pin;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use lapin::BasicProperties;
    use lapin::options::{BasicPublishOptions, ConfirmSelectOptions};
    use lapin::publisher_confirm::Confirmation;
    use tokio::time;

    use super::{
        BASIC_ACK, BASIC_CLASS, BASIC_NACK, ConfirmGate, FRAME_END, GateState, IMAGE,
        MAX_FRAME_PAYLOAD, METHOD_FRAME, OPERATION_TIMEOUT, bind_source, forward_frames, image,
        message_count, next_frame, request, start_rabbitmq,
    };
    use crate::tests::{MISSING_ENTRYPOINT, failed_start_case, guarded_container_case};
    use crate::{HarnessError, modern_dir};

    /// Frame type of a content header frame.
    const HEADER_FRAME: u8 = 2;
    /// Frame type of a heartbeat frame.
    const HEARTBEAT_FRAME: u8 = 8;
    /// A queue of `definitions.json` with no dead-letter arguments, for the gate's publications.
    const GATE_QUEUE: &str = "CARD.DEMO.ERROR";
    /// How long a held confirmation is watched for reaching its publisher all the same.
    const HELD_FOR: Duration = Duration::from_millis(300);

    /// How a test settles a held confirmation: [`ConfirmGate::release`] or [`ConfirmGate::refuse`].
    type Settle = fn(&ConfirmGate);

    /// One frame of `kind` on channel 1 with `payload`, as the broker writes it.
    fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        let mut bytes = vec![kind, 0, 1];
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes.push(FRAME_END);
        bytes
    }

    /// A method frame of `class` and `method` with the arguments of `basic.ack`: delivery tag 7,
    /// then a bits octet with `multiple` clear.
    fn method_frame(class: u16, method: u16) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&class.to_be_bytes());
        payload.extend_from_slice(&method.to_be_bytes());
        payload.extend_from_slice(&7_u64.to_be_bytes());
        payload.push(0);
        frame(METHOD_FRAME, &payload)
    }

    #[test]
    fn frames_are_read_whole_and_only_basic_ack_and_nack_are_confirmations() {
        let mut header_payload = Vec::new();
        for id in [BASIC_CLASS, 0, BASIC_ACK, 0] {
            header_payload.extend_from_slice(&id.to_be_bytes());
        }
        let cases = [
            ("basic.ack", method_frame(BASIC_CLASS, BASIC_ACK), true),
            ("basic.nack", method_frame(BASIC_CLASS, BASIC_NACK), true),
            ("basic.deliver", method_frame(BASIC_CLASS, 60), false),
            ("connection.close-ok", method_frame(10, 51), false),
            (
                "a content header that starts like basic.ack",
                frame(HEADER_FRAME, &header_payload),
                false,
            ),
            ("a heartbeat", frame(HEARTBEAT_FRAME, &[]), false),
        ];
        for (name, bytes, confirmation) in cases {
            let mut source = Cursor::new(bytes.clone());
            let read = next_frame(&mut source);
            let Ok(Some(read)) = read else {
                panic!("{name}: {:?}", read.err());
            };
            assert_eq!(read.bytes, bytes, "{name}: the frame is kept whole");
            assert_eq!(read.is_confirmation(), confirmation, "{name}");
            assert!(
                matches!(next_frame(&mut source), Ok(None)),
                "{name}: the stream ends after the frame"
            );
        }
    }

    #[test]
    fn a_refused_ack_becomes_the_nack_of_the_same_tag_and_a_nack_stays_one() {
        for method in [BASIC_ACK, BASIC_NACK] {
            let Ok(Some(mut held)) =
                next_frame(&mut Cursor::new(method_frame(BASIC_CLASS, method)))
            else {
                panic!("method {method} did not parse");
            };
            held.refuse();
            assert_eq!(
                held.bytes,
                method_frame(BASIC_CLASS, BASIC_NACK),
                "method {method}"
            );
        }
    }

    #[test]
    fn a_malformed_or_cut_frame_is_an_error_and_an_empty_stream_ends_cleanly() {
        assert!(
            matches!(next_frame(&mut Cursor::new(Vec::new())), Ok(None)),
            "an empty stream"
        );
        let ack = method_frame(BASIC_CLASS, BASIC_ACK);
        let mut wrong_end = ack.clone();
        if let Some(end) = wrong_end.last_mut() {
            *end = 0;
        }
        let oversize = u32::try_from(MAX_FRAME_PAYLOAD + 1).unwrap_or(u32::MAX);
        let mut too_large = vec![METHOD_FRAME, 0, 1];
        too_large.extend_from_slice(&oversize.to_be_bytes());
        let cases = [
            (
                "a cut header",
                ack.get(..4).unwrap_or_default().to_vec(),
                io::ErrorKind::UnexpectedEof,
            ),
            (
                "a cut payload",
                ack.get(..ack.len() - 2).unwrap_or_default().to_vec(),
                io::ErrorKind::UnexpectedEof,
            ),
            ("a wrong frame end", wrong_end, io::ErrorKind::InvalidData),
            ("an oversize payload", too_large, io::ErrorKind::InvalidData),
        ];
        for (name, bytes, kind) in cases {
            let read = next_frame(&mut Cursor::new(bytes));
            assert!(
                matches!(&read, Err(error) if error.kind() == kind),
                "{name}: {:?}",
                read.map(|parsed| parsed.map(|parsed| parsed.bytes.len()))
            );
        }
    }

    #[test]
    fn a_gate_that_closes_drops_its_held_confirmation_and_ends_the_relay() {
        let state = Arc::new(GateState::default());
        let heartbeat = frame(HEARTBEAT_FRAME, &[]);
        let mut stream = heartbeat.clone();
        stream.extend_from_slice(&method_frame(BASIC_CLASS, BASIC_ACK));
        stream.extend_from_slice(&heartbeat);
        let relaying = Arc::clone(&state);
        let relay = thread::spawn(move || {
            let mut client = Vec::new();
            let outcome = forward_frames(&mut Cursor::new(stream), &mut client, &relaying);
            (outcome.map_err(|error| error.kind()), client)
        });
        for _ in 0..500 {
            if state.ledger().arrived == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            state.ledger().arrived,
            1,
            "the confirmation reached the gate"
        );
        state.close();
        let Ok((outcome, client)) = relay.join() else {
            panic!("the relay thread panicked");
        };
        assert_eq!(outcome, Ok(()), "a close ends the relay without an error");
        assert_eq!(
            client, heartbeat,
            "only the frame before the held confirmation reached the client"
        );
        assert_eq!(state.ledger().passed, 0, "nothing was handed on");
    }

    /// Maps the outcome of a `lapin` call bounded by [`OPERATION_TIMEOUT`] to a [`HarnessError`]
    /// under `context`.
    async fn bounded<T>(
        context: &'static str,
        operation: impl Future<Output = lapin::Result<T>>,
    ) -> Result<T, HarnessError> {
        time::timeout(OPERATION_TIMEOUT, operation)
            .await
            .map_err(|err| HarnessError::new(context, &err))?
            .map_err(|err| HarnessError::new(context, &err))
    }

    #[tokio::test]
    async fn a_gate_withholds_each_confirmation_until_it_is_released_or_refused()
    -> Result<(), HarnessError> {
        let guard = start_rabbitmq().await?;
        let gate = guard.confirm_gate().await?;
        assert!(
            !format!("{gate:?}").contains("carddemo@"),
            "the gate's Debug shows no credential"
        );
        let direct = guard.connect().await?;
        let observer = bounded("open the observer channel", direct.create_channel()).await?;
        let gated = gate.connect().await?;
        let channel = bounded("open a gated channel", gated.create_channel()).await?;
        let select = channel.confirm_select(ConfirmSelectOptions::default());
        bounded("select confirms", select).await?;

        let cases: [(Settle, Confirmation); 2] = [
            (ConfirmGate::release, Confirmation::Ack(None)),
            (ConfirmGate::refuse, Confirmation::Nack(None)),
        ];
        for (index, (settle, expected)) in cases.into_iter().enumerate() {
            let publish = channel.basic_publish(
                "",
                GATE_QUEUE,
                BasicPublishOptions::default(),
                b"gate",
                BasicProperties::default(),
            );
            let mut confirmation = pin!(bounded("publish through the gate", publish).await?);
            gate.wait_for_arrivals(index + 1, OPERATION_TIMEOUT).await?;
            assert_eq!(
                (gate.arrived(), gate.passed()),
                (index + 1, index),
                "case {index}: the confirmation is held"
            );
            let early = time::timeout(HELD_FOR, &mut confirmation).await;
            assert!(
                early.is_err(),
                "case {index}: the publisher learned of a held confirmation"
            );
            // Queued while its confirmation is held: queue presence is not confirmation.
            let queued = message_count(&observer, GATE_QUEUE).await?;
            assert_eq!(
                usize::try_from(queued).ok(),
                Some(index + 1),
                "case {index}: the broker queued the message before confirming it"
            );
            settle(&gate);
            let received = bounded("await the confirmation", confirmation).await?;
            assert_eq!(received, expected, "case {index}: what the publisher sees");
            assert_eq!(gate.passed(), index + 1, "case {index}: handed on");
        }
        bounded(
            "close the gated connection",
            gated.close(200, "gate test done"),
        )
        .await?;
        bounded(
            "close the observer connection",
            direct.close(200, "gate test done"),
        )
        .await
    }

    #[tokio::test]
    async fn a_started_broker_is_owned_and_leaves_with_its_guard() -> Result<(), HarnessError> {
        let guard = start_rabbitmq().await?;
        let id = guard.container.id().to_owned();
        guarded_container_case(guard, &id, IMAGE).await
    }

    #[tokio::test]
    async fn a_failed_start_removes_only_its_own_container() -> Result<(), HarnessError> {
        let modern = modern_dir();
        let conf = bind_source(modern.join("rabbitmq/rabbitmq.conf")).await?;
        let definitions = bind_source(modern.join("rabbitmq/definitions.json")).await?;
        failed_start_case(
            request(
                image().with_entrypoint(MISSING_ENTRYPOINT),
                conf,
                definitions,
            ),
            "start rabbitmq",
        )
        .await
    }

    // `#[tokio::test]` runs on a current-thread runtime, where blocking the worker would stall
    // every other task and timer.
    #[tokio::test]
    async fn broker_files_resolve_to_absolute_utf8_paths() {
        for file in ["rabbitmq/rabbitmq.conf", "rabbitmq/definitions.json"] {
            let resolved = bind_source(modern_dir().join(file)).await;
            assert!(
                resolved
                    .as_deref()
                    .is_ok_and(|path| Path::new(path).is_absolute() && path.ends_with(file)),
                "{file}: {resolved:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_missing_broker_file_is_reported_with_its_path() {
        let Err(err) = bind_source(modern_dir().join("rabbitmq/missing.conf")).await else {
            panic!("a missing broker file resolved");
        };
        let text = err.to_string();
        assert!(
            text.starts_with("resolve rabbitmq mount: ") && text.contains("rabbitmq/missing.conf"),
            "{text}"
        );
    }
}
