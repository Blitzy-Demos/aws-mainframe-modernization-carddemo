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

use std::fmt;
use std::path::PathBuf;
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

/// A running broker container and the URLs that reach it.
///
/// Dropping the guard removes the container. Drop it inside the test's Tokio runtime, because
/// the removal runs on that runtime; never keep a guard in a `static`, which outlives it.
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
        time::timeout(
            OPERATION_TIMEOUT,
            Connection::connect(&self.amqp_url, ConnectionProperties::default()),
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
pub async fn start_rabbitmq() -> Result<RabbitMqGuard, HarnessError> {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{IMAGE, bind_source, image, request, start_rabbitmq};
    use crate::tests::{MISSING_ENTRYPOINT, failed_start_case, guarded_container_case};
    use crate::{HarnessError, modern_dir};

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
