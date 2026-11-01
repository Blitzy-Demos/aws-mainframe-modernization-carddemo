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

use std::io;

use lapin::{
    Channel, Connection, ConnectionProperties,
    options::ConfirmSelectOptions,
    uri::{AMQPScheme, AMQPUri},
};

use crate::MessagingError;

/// Opens the broker connection that every consumer channel shares.
///
/// `url` is an `amqp://user:password@host:port/vhost` URI. It carries the
/// broker password, so it is never logged and never quoted in an error. The
/// connection's I/O runs as tasks on the tokio runtime that polls this future.
///
/// # Errors
///
/// Returns [`MessagingError::Amqp`] when the URL is malformed or uses the
/// `amqps` scheme, the broker is unreachable, or the broker refuses the
/// credentials or the virtual host.
///
/// # Panics
///
/// Panics when polled outside a tokio runtime, because the connection's
/// executor binds to the current runtime handle.
pub async fn connect(url: &str) -> Result<Connection, MessagingError> {
    let target = parse_plain_amqp_uri(url)?;
    let properties = ConnectionProperties::default()
        .with_executor(tokio_executor_trait::Tokio::current())
        .with_reactor(tokio_reactor_trait::Tokio);
    let connection = Connection::connect_uri(target, properties).await?;
    tracing::info!(target: "carddemo_messaging", "connected to broker");
    Ok(connection)
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
/// refuses a new channel, or the broker rejects `confirm.select`.
pub async fn open_channel(connection: &Connection) -> Result<Channel, MessagingError> {
    let channel = connection.create_channel().await?;
    channel
        .confirm_select(ConfirmSelectOptions::default())
        .await?;
    Ok(channel)
}

#[cfg(test)]
mod tests {
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
}
