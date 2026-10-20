//! A `rabbitmq:3.13-management` testcontainer booted from the same `rabbitmq/rabbitmq.conf` and
//! `rabbitmq/definitions.json` that `compose.yaml` mounts, and the few `lapin` helpers the
//! messaging tests need to publish requests and read replies, errors and dead letters.
//!
//! The definitions file supplies the vhost `/`, the local-only user `carddemo`/`carddemo` and the
//! seven durable queues, the `CARD.DEMO.REQUEST.PAUTH` dead-letter arguments included. There are
//! no exchanges: MQ point-to-point maps to the default exchange, so a queue name is also the
//! routing key.
//!
//! The helpers never log a message body and never put one into a [`HarnessError`]; errors carry
//! only the `Display` text of the `lapin` or testcontainers error behind them (Cardholder Data
//! Redaction).

use std::fmt;
use std::path::Path;
use std::time::Duration;

use lapin::message::BasicGetMessage;
use lapin::options::{BasicGetOptions, BasicPublishOptions, QueueDeclareOptions};
use lapin::publisher_confirm::Confirmation;
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection, ConnectionProperties};
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
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
/// AMQP 0-9-1 `reply-success`, sent when the probe connection closes.
const REPLY_SUCCESS: u16 = 200;

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
    /// Returns a [`HarnessError`] when the connection or its AMQP handshake fails.
    pub async fn connect(&self) -> Result<Connection, HarnessError> {
        Connection::connect(&self.amqp_url, ConnectionProperties::default())
            .await
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
/// Returns a [`HarnessError`] when either broker file is missing or its path is not UTF-8, when
/// the container does not start or log its startup message within 180 seconds, when its ports
/// cannot be read, or when user `carddemo` cannot connect within 60 seconds of startup.
pub async fn start_rabbitmq() -> Result<RabbitMqGuard, HarnessError> {
    let modern = modern_dir();
    let conf = bind_source(&modern.join("rabbitmq/rabbitmq.conf"))?;
    let definitions = bind_source(&modern.join("rabbitmq/definitions.json"))?;

    let container = GenericImage::new(IMAGE, TAG)
        .with_exposed_port(AMQP_PORT.tcp())
        .with_exposed_port(MANAGEMENT_PORT.tcp())
        .with_wait_for(WaitFor::message_on_stdout(STARTUP_MESSAGE))
        .with_mount(
            Mount::bind_mount(conf, "/etc/rabbitmq/rabbitmq.conf")
                .with_access_mode(AccessMode::ReadOnly),
        )
        .with_mount(
            Mount::bind_mount(definitions, "/etc/rabbitmq/definitions.json")
                .with_access_mode(AccessMode::ReadOnly),
        )
        .with_startup_timeout(STARTUP_TIMEOUT)
        .start()
        .await
        .map_err(|err| HarnessError::new("start rabbitmq", &err))?;

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

/// The absolute UTF-8 form of a broker file, as a Docker bind mount requires.
fn bind_source(path: &Path) -> Result<String, HarnessError> {
    let absolute = std::fs::canonicalize(path).map_err(|err| {
        HarnessError::new(
            "resolve rabbitmq mount",
            &format_args!("{}: {err}", path.display()),
        )
    })?;
    absolute.to_str().map(str::to_owned).ok_or_else(|| {
        HarnessError::new(
            "resolve rabbitmq mount",
            &format_args!("{} is not valid UTF-8", absolute.display()),
        )
    })
}

/// Retries an AMQP connection as user `carddemo` until one succeeds or [`READY_TIMEOUT`] passes,
/// then closes the probe connection.
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
                return connection
                    .close(REPLY_SUCCESS, "probe")
                    .await
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
/// any other channel it returns once the message has been sent.
///
/// # Errors
///
/// Returns a [`HarnessError`] when the publication fails or the broker confirms it negatively.
pub async fn publish(
    channel: &Channel,
    queue: &str,
    body: &[u8],
    properties: BasicProperties,
) -> Result<(), HarnessError> {
    let confirm = channel
        .basic_publish("", queue, BasicPublishOptions::default(), body, properties)
        .await
        .map_err(|err| HarnessError::new("publish", &err))?;
    let confirmation = confirm
        .await
        .map_err(|err| HarnessError::new("publish confirmation", &err))?;
    match confirmation {
        Confirmation::Ack(_) | Confirmation::NotRequested => Ok(()),
        Confirmation::Nack(_) => Err(HarnessError::new(
            "publish confirmation",
            &"the broker refused the message",
        )),
    }
}

/// Takes the next message from `queue`, polling every 50 milliseconds until one arrives or
/// `timeout` passes. The last poll runs at the deadline, so a zero `timeout` polls once.
///
/// The message is taken with `no_ack`, so a test never leaves an unacknowledged delivery behind.
///
/// # Errors
///
/// Returns a [`HarnessError`] when a `basic.get` fails, for example because `queue` does not
/// exist, which also closes the channel.
pub async fn get(
    channel: &Channel,
    queue: &str,
    timeout: Duration,
) -> Result<Option<BasicGetMessage>, HarnessError> {
    let deadline = Instant::now() + timeout;
    loop {
        let message = channel
            .basic_get(queue, BasicGetOptions { no_ack: true })
            .await
            .map_err(|err| HarnessError::new("get", &err))?;
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
/// Returns a [`HarnessError`] when the declaration fails.
pub async fn declare_reply_queue(channel: &Channel) -> Result<String, HarnessError> {
    let queue = channel
        .queue_declare(
            "",
            QueueDeclareOptions {
                durable: false,
                exclusive: false,
                auto_delete: false,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await
        .map_err(|err| HarnessError::new("declare reply queue", &err))?;
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
/// exist.
pub async fn message_count(channel: &Channel, queue: &str) -> Result<u32, HarnessError> {
    let queue = channel
        .queue_declare(
            queue,
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await
        .map_err(|err| HarnessError::new("count queue messages", &err))?;
    Ok(queue.message_count())
}
