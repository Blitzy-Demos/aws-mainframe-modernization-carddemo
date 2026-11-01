//! Telemetry initialisation shared by the `api`, `batch` and `mq` binaries.
//!
//! One writer, standard output in the binaries, receives two kinds of JSON line:
//!
//! - log events, written by the `tracing-subscriber` JSON formatter with the
//!   current span (`span`) and the list of entered spans (`spans`);
//! - finished spans, which pass through `tracing-opentelemetry` into an
//!   OpenTelemetry SDK tracer provider whose simple span processor hands each one
//!   to [`JsonSpanExporter`], which writes it as one `"type":"span"` line.
//!
//! Standard output therefore holds JSON lines only. Nothing in this module prints
//! anywhere else, except a failed shutdown at drop time, which goes to standard
//! error.
//!
//! # Filtering
//!
//! The filter directives (`RUST_LOG`, default `info`) gate events and spans alike:
//! a span below the filter level is neither formatted nor exported. A span meant
//! to reach the span lines under the default filter must be created at INFO level
//! or above, for example with `tracing::info_span!`.
//!
//! # Spans the consumers emit
//!
//! The code here is generic; each consumer names its own spans and fields:
//!
//! - `api`: the `tower-http` `TraceLayer` request span and, inside it, a handler
//!   span with `transaction` (for example `CAUP`), `user_id`, the resource keys
//!   and `outcome`.
//! - `batch`: a job span with `job`, its inputs, the read, posted and rejected
//!   counts and the exit code, plus one event per reject and per halt.
//! - `mq`: a message span with `flow`, `message_id`, `correlation_id`, `outcome`
//!   (`replied` or `rejected`), `error_record` (`published`, `logged` or `none`)
//!   and, for CP00 errors, the `CCPAUERY` location (`M004`, `I003`, `I004`).
//!
//! A field declared `tracing::field::Empty` and filled in later with
//! `Span::record` appears among the `attributes` of the span line.
//!
//! # Lifecycle
//!
//! Each binary calls [`init`] once at start, keeps the returned
//! [`TelemetryGuard`] alive until exit, then calls [`TelemetryGuard::shutdown`]:
//!
//! ```no_run
//! use carddemo_runtime::telemetry::{self, TelemetryError};
//!
//! let guard = telemetry::init("batch")?;
//! tracing::info_span!("job", job = "posttran").in_scope(|| tracing::info!("job started"));
//! guard.shutdown()?;
//! # Ok::<(), TelemetryError>(())
//! ```

use std::io::Write;

use opentelemetry::KeyValue;
use opentelemetry::trace::{TraceError, TracerProvider as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::TracerProvider;
use tracing_subscriber::filter::{EnvFilter, ParseError};
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};

use crate::config::{defaults, keys};
use crate::json_span_exporter::JsonSpanExporter;

/// Resource attribute naming the emitting process; the exporter writes its value
/// as the `service` key of every span line.
const SERVICE_NAME_KEY: &str = "service.name";

/// Failure to set up or shut down telemetry.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// The filter directives do not follow the `EnvFilter` grammar.
    #[error("invalid log filter: {0}")]
    Filter(#[from] ParseError),
    /// A global default subscriber is already installed.
    #[error("failed to install the tracing subscriber: {0}")]
    Install(#[from] TryInitError),
    /// The tracer provider failed to shut down its span processor.
    #[error("failed to shut down the tracer provider: {0}")]
    Shutdown(#[from] TraceError),
}

/// Owns the tracer provider of one subscriber and shuts it down.
///
/// The tracer inside the OpenTelemetry layer holds its own reference to the
/// provider, and a global subscriber is never dropped, so dropping the provider
/// handle alone would never shut it down. Call [`TelemetryGuard::shutdown`]
/// before the process exits to learn about a failure. A guard dropped without
/// that call shuts the provider down itself and reports a failure on standard
/// error, never on standard output.
///
/// The provider is held in an `Option` and taken on the first shutdown, so a
/// guard dropped after an explicit shutdown does not shut down a second time,
/// which would fail and print a spurious error line.
#[must_use]
#[derive(Debug)]
pub struct TelemetryGuard {
    provider: Option<TracerProvider>,
}

impl TelemetryGuard {
    /// Shuts the tracer provider down, which shuts down its JSON span exporter.
    /// Spans started afterwards are not exported.
    ///
    /// # Errors
    ///
    /// [`TelemetryError::Shutdown`] when the provider fails to shut down its span
    /// processor, or was already shut down through another handle.
    pub fn shutdown(mut self) -> Result<(), TelemetryError> {
        self.shut_down_provider()
    }

    /// Takes the provider and shuts it down; `Ok` when it is already gone.
    fn shut_down_provider(&mut self) -> Result<(), TelemetryError> {
        match self.provider.take() {
            Some(provider) => provider.shutdown().map_err(TelemetryError::from),
            None => Ok(()),
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Err(err) = self.shut_down_provider() {
            // Standard output carries JSON lines only. A failed write to standard
            // error leaves nothing further to report to, so its result is dropped.
            let _ = writeln!(std::io::stderr(), "telemetry shutdown failed: {err}");
        }
    }
}

/// Builds the subscriber [`init`] installs, writing every JSON line to `writer`.
///
/// The layers, in order:
///
/// 1. an `EnvFilter` parsed from `directives`, which gates events and spans for
///    the layers after it;
/// 2. the `tracing-subscriber` JSON formatter, with the current span and the
///    span list on every event line and no ANSI colour codes;
/// 3. the `tracing-opentelemetry` layer, on a tracer named `service_name` from a
///    tracer provider of its own whose simple span processor feeds a
///    [`JsonSpanExporter`] on `writer`, with `service_name` as the resource's
///    `service.name`.
///
/// No global state is touched: the tracer provider is not registered globally,
/// so tests can build several subscribers side by side and install each one
/// with `tracing::subscriber::with_default`. Each subscriber's spans go only to
/// its own writer. `directives` are used as given; the environment is not read.
///
/// The returned guard shuts the provider down; see [`TelemetryGuard`].
///
/// # Errors
///
/// [`TelemetryError::Filter`] when `directives` is not a valid `EnvFilter`
/// directive list. A valid one reads like `info,carddemo_api=debug`.
pub fn subscriber_with_writer<W>(
    service_name: &str,
    directives: &str,
    writer: W,
) -> Result<
    (
        impl tracing::Subscriber + Send + Sync + 'static,
        TelemetryGuard,
    ),
    TelemetryError,
>
where
    W: for<'a> MakeWriter<'a> + Clone + Send + Sync + 'static,
{
    let filter = EnvFilter::try_new(directives)?;
    let provider = TracerProvider::builder()
        .with_simple_exporter(JsonSpanExporter::new(writer.clone()))
        .with_resource(Resource::new([KeyValue::new(
            SERVICE_NAME_KEY,
            service_name.to_owned(),
        )]))
        .build();
    let tracer = provider.tracer(service_name.to_owned());
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(true)
                .with_ansi(false)
                .with_writer(writer),
        )
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    Ok((
        subscriber,
        TelemetryGuard {
            provider: Some(provider),
        },
    ))
}

/// Installs the JSON telemetry of the process `service_name` (`api`, `batch` or
/// `mq`) as the global default subscriber, writing to standard output.
///
/// The filter directives come from `RUST_LOG`. When that variable is not set,
/// not valid Unicode, or empty, the default `info` applies, the same default the
/// settings carry.
///
/// Call it once, at the start of the process, and keep the guard alive until
/// exit, then call [`TelemetryGuard::shutdown`].
///
/// # Errors
///
/// - [`TelemetryError::Filter`] when `RUST_LOG` holds an invalid directive list.
/// - [`TelemetryError::Install`] when a global default subscriber is already
///   installed, for example by an earlier call.
pub fn init(service_name: &str) -> Result<TelemetryGuard, TelemetryError> {
    let directives = std::env::var(keys::RUST_LOG)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| defaults::RUST_LOG.to_owned());
    let (subscriber, guard) = subscriber_with_writer(service_name, &directives, std::io::stdout)?;
    subscriber.try_init()?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use serde_json::Value as Json;
    use tracing_subscriber::fmt::MakeWriter;

    use super::{TelemetryError, subscriber_with_writer};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Shared in-memory sink standing in for stdout.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        /// Every line written so far, each parsed as JSON.
        fn json_lines(&self) -> Result<Vec<Json>, Box<dyn Error>> {
            let bytes = self
                .0
                .lock()
                .map_err(|_| "capture buffer poisoned")?
                .clone();
            let text = String::from_utf8(bytes)?;
            let mut lines = Vec::new();
            for line in text.lines() {
                let value: Json = serde_json::from_str(line)
                    .map_err(|err| format!("line is not JSON ({err}): {line}"))?;
                lines.push(value);
            }
            Ok(lines)
        }
    }

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The span lines among `lines`: the objects whose `type` is `span`.
    fn span_lines(lines: &[Json]) -> Vec<&Json> {
        lines.iter().filter(|line| line["type"] == "span").collect()
    }

    #[test]
    fn spans_and_events_share_one_json_writer() -> TestResult {
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || {
            let job = tracing::info_span!("job", job = "posttran", outcome = tracing::field::Empty);
            let _entered = job.enter();
            job.record("outcome", "ok");
            tracing::info!(count = 3, "posted");
        });
        guard.shutdown()?;

        let lines = capture.json_lines()?;
        let [job_line] = span_lines(&lines)[..] else {
            return Err(format!("expected exactly one span line: {lines:?}").into());
        };
        assert_eq!(job_line["name"], "job");
        assert_eq!(job_line["service"], "test-svc");
        assert_eq!(job_line["attributes"]["job"], "posttran");
        assert_eq!(job_line["attributes"]["outcome"], "ok");

        let posted = lines
            .iter()
            .find(|line| line["fields"]["message"] == "posted")
            .ok_or_else(|| format!("no `posted` event line: {lines:?}"))?;
        assert!(posted.get("type").is_none(), "{posted}");
        assert_eq!(posted["level"], "INFO");
        assert_eq!(posted["fields"]["count"], 3);
        assert!(posted["span"].is_object(), "{posted}");
        assert_eq!(posted["span"]["name"], "job");
        assert!(posted["spans"].is_array(), "{posted}");
        Ok(())
    }

    #[test]
    fn spans_below_the_filter_level_are_not_exported() -> TestResult {
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug_span!("hidden").in_scope(|| tracing::info!("inside hidden"));
            tracing::info_span!("shown").in_scope(|| tracing::debug!("filtered out"));
        });
        guard.shutdown()?;

        let lines = capture.json_lines()?;
        let names: Vec<&str> = span_lines(&lines)
            .into_iter()
            .filter_map(|line| line["name"].as_str())
            .collect();
        assert_eq!(names, ["shown"], "{lines:?}");
        let messages: Vec<&str> = lines
            .iter()
            .filter_map(|line| line["fields"]["message"].as_str())
            .collect();
        assert_eq!(messages, ["inside hidden"], "{lines:?}");
        Ok(())
    }

    #[test]
    fn invalid_directives_are_a_filter_error() {
        for directives in ["info,=[", "=[bad", "job=loud"] {
            let result = subscriber_with_writer("x", directives, Capture::default());
            let Err(err) = result else {
                panic!("{directives:?} was accepted");
            };
            assert!(
                matches!(err, TelemetryError::Filter(_)),
                "{directives:?}: {err:?}"
            );
            assert!(err.to_string().starts_with("invalid log filter: "), "{err}");
        }
    }
}
