//! AMQP connection and publisher-confirm channels, replacing the IBM MQ
//! connection the CICS region holds for the trigger-started CDRD, CDRA and
//! CP00 transactions.
//!
//! CODATE01 and COACCT01 open their error, input and output queues on that
//! connection for every triggered task and close them in `8000-TERMINATION`.
//! Here one long-lived connection serves the whole process, and each consumer
//! gets its own channel in publisher-confirm mode, so the broker's confirmation
//! of a reply can gate the acknowledgement of its request.
//!
//! No queue is declared here. The broker definitions own every durable queue,
//! so a missing queue surfaces as a channel error or a returned mandatory
//! publish instead of being silently recreated without its definition.
//!
//! Opening the connection and its channels, setting up each consumer and
//! closing the connection all have a deadline, [`BROKER_TIMEOUT`] by default,
//! so a blackholed or half-open broker ends start-up or shutdown with
//! [`MessagingError::BrokerTimeout`] instead of hanging it.

use std::future::Future;
use std::io;
use std::time::Duration;

use lapin::{
    Channel, Connection, ConnectionProperties,
    options::ConfirmSelectOptions,
    protocol::constants::REPLY_SUCCESS,
    uri::{AMQPScheme, AMQPUri},
};

use crate::MessagingError;

/// The longest the consumers wait for the broker to answer one request: the
/// connection handshake (TCP connect included, unless the broker URL sets its
/// own `connection_timeout`), `channel.open`, `confirm.select`, `basic.qos`,
/// `basic.consume` and `connection.close`.
///
/// It is a fixed property of the adapter, not a deployment setting.
pub const BROKER_TIMEOUT: Duration = Duration::from_secs(10);

/// Waits for `request`, a broker request named `operation` (such as
/// `"channel.open"`), for at most `limit`.
///
/// The timer is tokio's: its `time` driver is compiled in through
/// `tokio-reactor-trait`, which runs lapin on tokio and enables tokio's
/// `time` feature itself.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when the request fails, and
/// [`MessagingError::BrokerTimeout`] naming `operation` and `limit` when the
/// broker has not answered within `limit`. The abandoned request is dropped.
pub(crate) async fn bounded<T>(
    operation: &'static str,
    limit: Duration,
    request: impl Future<Output = lapin::Result<T>>,
) -> Result<T, MessagingError> {
    match tokio::time::timeout(limit, request).await {
        Ok(answer) => answer.map_err(MessagingError::Amqp),
        Err(_elapsed) => Err(MessagingError::BrokerTimeout {
            operation,
            timeout: limit,
        }),
    }
}

/// Opens the broker connection that every consumer channel shares.
///
/// `url` is an `amqp://user:password@host:port/vhost` URI. It carries the
/// broker password, so it is never logged and never quoted in an error. The
/// connection's I/O runs as tasks on the tokio runtime that polls this future.
///
/// The TCP connect and the AMQP handshake together are held to the URL's
/// `connection_timeout` query parameter (milliseconds), or to
/// [`BROKER_TIMEOUT`] when the URL sets none.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when the URL is malformed or uses the
/// `amqps` scheme, the broker is unreachable, or the broker refuses the
/// credentials or the virtual host, and [`MessagingError::BrokerTimeout`]
/// when the connection is not open within that limit.
///
/// # Panics
///
/// Panics when polled outside a tokio runtime, because the connection's
/// executor binds to the current runtime handle.
pub async fn connect(url: &str) -> Result<Connection, MessagingError> {
    let (target, limit) = with_connection_timeout(parse_plain_amqp_uri(url)?);
    let properties = ConnectionProperties::default()
        .with_executor(tokio_executor_trait::Tokio::current())
        .with_reactor(tokio_reactor_trait::Tokio);
    let connection = bounded(
        "connection handshake",
        limit,
        Connection::connect_uri(target, properties),
    )
    .await?;
    tracing::info!(target: "carddemo_messaging", "connected to broker");
    Ok(connection)
}

/// Gives `uri` a `connection_timeout` of [`BROKER_TIMEOUT`] unless the URL
/// sets one, and returns it with that timeout as the handshake's limit.
///
/// lapin runs the TCP connect as a blocking call that only the URI's
/// `connection_timeout` ends, so the URI carries the same limit that bounds
/// the whole handshake, and no connect outlives it.
fn with_connection_timeout(mut uri: AMQPUri) -> (AMQPUri, Duration) {
    let default_millis = u64::try_from(BROKER_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
    let millis = *uri.query.connection_timeout.get_or_insert(default_millis);
    (uri, Duration::from_millis(millis))
}

/// Parses `url` for a plain AMQP connection.
///
/// lapin is built without TLS, and its `amqps` path then falls back to an
/// unencrypted socket, so an `amqps` URL is refused rather than sending the
/// credentials in clear text under a scheme that promises encryption. The
/// parser's own message can quote the whole URL, password included, so a
/// parse failure carries a fixed text instead.
fn parse_plain_amqp_uri(url: &str) -> Result<AMQPUri, lapin::Error> {
    let parsed = url
        .parse::<AMQPUri>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid AMQP URL"))?;
    if parsed.scheme == AMQPScheme::AMQPS {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "amqps URLs need TLS, which this build does not include",
        )
        .into());
    }
    Ok(parsed)
}

/// Opens a channel on `connection` and puts it in publisher-confirm mode.
///
/// Each consumer takes its own channel, so its prefetch, deliveries and
/// confirmations never interleave with another flow's. Every publish on the
/// returned channel resolves to a broker `Confirmation`.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when the connection is closed, the broker
/// refuses a new channel, or the broker rejects `confirm.select`, and
/// [`MessagingError::BrokerTimeout`] when the broker does not answer
/// `channel.open` or `confirm.select` within [`BROKER_TIMEOUT`].
pub async fn open_channel(connection: &Connection) -> Result<Channel, MessagingError> {
    let channel = bounded("channel.open", BROKER_TIMEOUT, connection.create_channel()).await?;
    bounded(
        "confirm.select",
        BROKER_TIMEOUT,
        channel.confirm_select(ConfirmSelectOptions::default()),
    )
    .await?;
    Ok(channel)
}

/// Closes `connection` with reply code 200 and waits for the broker's
/// `connection.close-ok`, for at most [`BROKER_TIMEOUT`]. Every channel on it
/// closes with it, so the broker redelivers each request still unsettled.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when the connection is no longer open or
/// the close fails, and [`MessagingError::BrokerTimeout`] when the broker has
/// not confirmed the close within [`BROKER_TIMEOUT`].
pub async fn close(connection: &Connection) -> Result<(), MessagingError> {
    bounded(
        "connection.close",
        BROKER_TIMEOUT,
        connection.close(REPLY_SUCCESS, "shutdown"),
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::future;
    use std::pin::Pin;

    use lapin::ChannelState;

    use super::*;

    /// Stands in for a password, so a test can tell whether an error quotes it.
    const MARKER: &str = "fake-pw-marker";

    #[test]
    fn plain_amqp_url_parses() {
        let parsed = parse_plain_amqp_uri(&format!("amqp://tester:{MARKER}@localhost:5672/%2f"))
            .expect("plain amqp URL");
        assert_eq!(parsed.scheme, AMQPScheme::AMQP);
        assert_eq!(parsed.authority.userinfo.password, MARKER);
        assert_eq!(parsed.authority.port, 5672);
        assert_eq!(parsed.vhost, "/");
    }

    #[test]
    fn refused_urls_never_quote_the_url() {
        for (url, kind) in [
            (
                format!("amqps://tester:{MARKER}@localhost:5671/%2f"),
                io::ErrorKind::Unsupported,
            ),
            (
                format!("amqp:tester:{MARKER}@localhost"),
                io::ErrorKind::InvalidInput,
            ),
            (
                format!("http://tester:{MARKER}@localhost/"),
                io::ErrorKind::InvalidInput,
            ),
            (format!("tester:{MARKER}"), io::ErrorKind::InvalidInput),
        ] {
            let Err(lapin::Error::IOError(error)) = parse_plain_amqp_uri(&url) else {
                panic!("{url} must be refused with an I/O error");
            };
            assert_eq!(error.kind(), kind, "{url}");
            assert!(!error.to_string().contains(MARKER), "{url}: {error}");
        }
    }

    #[test]
    fn the_handshake_limit_is_the_url_timeout_or_the_broker_timeout() {
        for (url, millis, limit) in [
            (
                format!("amqp://tester:{MARKER}@localhost:5672/%2f"),
                10_000,
                Duration::from_secs(10),
            ),
            (
                format!("amqp://tester:{MARKER}@localhost:5672/%2f?connection_timeout=2500"),
                2_500,
                Duration::from_millis(2_500),
            ),
        ] {
            let parsed = parse_plain_amqp_uri(&url).expect("plain amqp URL");
            let (target, handshake_limit) = with_connection_timeout(parsed);
            assert_eq!(target.query.connection_timeout, Some(millis), "{url}");
            assert_eq!(handshake_limit, limit, "{url}");
        }
    }

    #[tokio::test]
    async fn a_broker_request_ends_as_its_answer_or_its_deadline() {
        /// A broker request as `bounded` receives it.
        type Request = Pin<Box<dyn Future<Output = lapin::Result<u8>> + Send>>;
        /// Whether `bounded` ended the request as the case expects.
        type Expected = fn(&Result<u8, MessagingError>) -> bool;
        const LIMIT: Duration = Duration::from_millis(10);

        let cases: [(&'static str, Request, Expected); 3] = [
            ("basic.qos", Box::pin(future::ready(Ok(7))), |outcome| {
                matches!(outcome, Ok(7))
            }),
            (
                "channel.open",
                Box::pin(future::ready(Err(lapin::Error::InvalidChannelState(
                    ChannelState::Closed,
                )))),
                |outcome| {
                    matches!(
                        outcome,
                        Err(MessagingError::Amqp(lapin::Error::InvalidChannelState(
                            ChannelState::Closed
                        )))
                    )
                },
            ),
            (
                "connection handshake",
                Box::pin(future::pending()),
                |outcome| {
                    matches!(
                        outcome,
                        Err(MessagingError::BrokerTimeout {
                            operation: "connection handshake",
                            timeout,
                        }) if *timeout == LIMIT
                    )
                },
            ),
        ];
        for (operation, request, expected) in cases {
            let outcome = bounded(operation, LIMIT, request).await;
            assert!(expected(&outcome), "{operation}: {outcome:?}");
        }
    }
}
