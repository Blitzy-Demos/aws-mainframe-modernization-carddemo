//! Process telemetry of the `CardDemo` binaries: JSON tracing output on standard output, optional
//! `OpenTelemetry` span export to standard output, and the span shapes every service uses.
//!
//! [`init`] installs the global subscriber once per process: an `EnvFilter` read from `RUST_LOG`
//! (unset means `info`), the JSON layer of [`json_layer`] over standard output and, when
//! `CARDDEMO_OTEL_EXPORT=stdout`, a `tracing-opentelemetry` layer whose finished spans the
//! `opentelemetry-stdout` exporter prints. Output goes to standard output only: there is no
//! collector, no network exporter and no cloud service.
//!
//! # Standard output format
//!
//! With span export off, every line of standard output is one JSON event. With
//! `CARDDEMO_OTEL_EXPORT=stdout`, the `opentelemetry-stdout` exporter also writes each finished
//! span as a multi-line, human-readable text block, interleaved with the JSON event lines; the
//! exporter has no JSON format. A consumer that parses standard output line by line as JSON
//! therefore runs with export off. Exported spans and their events carry no source-location
//! attributes, so no source or host path reaches standard output.
//!
//! # Redaction boundary
//!
//! [`request_span`], [`record_response`], [`message_span`] and [`job_span`] are the only span
//! shapes the services create, and each carries only the fields its documentation lists.
//! Everything a span or event records reaches standard output, so a caller never adds a field
//! holding a request or message body, a URI, a query string, a header or a protected value (card
//! number, CVV, SSN, password), and never interpolates one into an event message.
//!
//! # Alerts
//!
//! `COPAUA0C` drew operator attention by writing its error log record to the `CSSL` transient
//! data queue. Here an alert is an `ERROR` event with target [`ALERT_TARGET`] in the same JSON
//! stream, for example messaging's `CP00_CARD_UNPARSEABLE`.
//!
//! # Spans across `.await`
//!
//! Attach a span to a future with `tracing::Instrument::instrument`. Never hold the guard that
//! `Span::enter` returns across an `.await` (`clippy::await_holding_span_guard`): the span would
//! stay entered for whatever else the executor runs on that thread.

use std::env::VarError;
use std::error::Error;
use std::fmt;
use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{Tracer, TracerProvider};
use tracing::field::Empty;
use tracing::{Span, Subscriber, info_span};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::filter::{Builder, EnvFilter, FromEnvError, LevelFilter};
use tracing_subscriber::fmt::format::{Format, Json, JsonFields};
use tracing_subscriber::fmt::{Layer as FmtLayer, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};

use crate::config::OtelExport;

/// Target of every alert event.
///
/// An alert is emitted as
/// `tracing::error!(target: runtime::telemetry::ALERT_TARGET, alert = "…", …)`. Its field values
/// name the condition and the message identifiers only; they never hold a message body or a
/// protected value.
pub const ALERT_TARGET: &str = "carddemo::alert";

/// Route the api records when axum provides no `MatchedPath` (a request no route matched), so a
/// raw URI is never recorded in its place.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// Instrumentation scope name of the `OpenTelemetry` tracer.
const TRACER_NAME: &str = "carddemo";

/// The JSON format layer of the `CardDemo` tracing output, writing through `make_writer`.
///
/// Each event is one JSON line with its level, target, fields, the current span and the list of
/// entered spans, each span with its name and recorded fields. [`init`] uses it over
/// `std::io::stdout`; `test-support`'s trace capture uses it over an in-memory writer, so what a
/// test captures has exactly the standard output format. The concrete layer type is returned so
/// that a capture can still enable span lifecycle events on top of this format.
#[must_use]
pub fn json_layer<S, W>(make_writer: W) -> FmtLayer<S, JsonFields, Format<Json>, W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + 'static,
{
    tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_target(true)
        .with_writer(make_writer)
}

/// Installs the process-wide tracing subscriber.
///
/// The subscriber filters with `RUST_LOG` (unset or empty means `info`), writes JSON lines to
/// standard output through [`json_layer`] and, for [`OtelExport::Stdout`], also bridges spans
/// into an `OpenTelemetry` tracer provider whose simple span processor prints each finished span
/// to standard output. [`OtelExport::Off`] adds no `OpenTelemetry` layer.
///
/// With [`OtelExport::Stdout`], the `opentelemetry-stdout` exporter writes each finished span as a
/// multi-line, human-readable text block between the JSON event lines, so standard output is no
/// longer one JSON object per line. A consumer that parses standard output line by line as JSON
/// runs with [`OtelExport::Off`], where every line is one JSON event. Exported spans and their
/// events carry no source-location attributes.
///
/// The returned [`FlushGuard`] shuts the span exporter down when dropped; bind it for the life of
/// `main`.
///
/// # Errors
///
/// - [`InitError::NotUnicode`] when `RUST_LOG` holds bytes that are not valid Unicode.
/// - [`InitError::Filter`] when `RUST_LOG` holds a directive the filter cannot parse.
/// - [`InitError::Install`] when a global subscriber is already installed in this process, for
///   example by an earlier call of this function.
pub fn init(otel_export: OtelExport) -> Result<FlushGuard, InitError> {
    let builder = EnvFilter::builder().with_default_directive(LevelFilter::INFO.into());
    let filter = env_filter(&builder, builder.try_from_env())?;

    let provider = match otel_export {
        OtelExport::Off => None,
        OtelExport::Stdout => Some(
            TracerProvider::builder()
                .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
                .build(),
        ),
    };
    let otel = provider.as_ref().map(otel_layer);

    tracing_subscriber::registry()
        .with(filter)
        .with(json_layer(std::io::stdout))
        .with(otel)
        .try_init()
        .map_err(InitError::Install)?;

    Ok(FlushGuard { provider })
}

/// The filter [`init`] installs, given `builder` and the outcome of its `try_from_env`.
///
/// An unset `RUST_LOG` gives the builder's default directive, as an empty one does. A value that
/// is not valid Unicode is an error, never read as unset. Taking the outcome as an argument keeps
/// this mapping independent of the process environment.
///
/// # Errors
///
/// - [`InitError::NotUnicode`] when `RUST_LOG` holds bytes that are not valid Unicode.
/// - [`InitError::Filter`] when `RUST_LOG` holds a directive the filter cannot parse.
fn env_filter(
    builder: &Builder,
    read: Result<EnvFilter, FromEnvError>,
) -> Result<EnvFilter, InitError> {
    read.or_else(|err| {
        match err
            .source()
            .and_then(|source| source.downcast_ref::<VarError>())
        {
            Some(VarError::NotPresent) => Ok(builder.parse_lossy("")),
            Some(VarError::NotUnicode(_)) => Err(InitError::NotUnicode),
            None => Err(InitError::Filter(err)),
        }
    })
}

/// The `tracing-opentelemetry` layer that bridges spans and their events into `provider`'s
/// tracer.
///
/// Location attributes are off, so no exported span or span event carries `code.filepath`,
/// `code.namespace` or `code.lineno`: a source path of this workspace or of the cargo registry
/// never reaches standard output. Every other layer setting keeps its default.
fn otel_layer<S>(provider: &TracerProvider) -> OpenTelemetryLayer<S, Tracer>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    tracing_opentelemetry::layer()
        .with_location(false)
        .with_tracer(provider.tracer(TRACER_NAME))
}

/// Keeps the `OpenTelemetry` span exporter alive and shuts it down, flushing it, when dropped.
///
/// Binaries bind it as `let _telemetry = runtime::telemetry::init(..)?;`, never as `let _ = ..`,
/// which drops it at once. They return `std::process::ExitCode` from `main` rather than calling
/// `std::process::exit`, so the guard drops and the exporter flushes before the process ends.
#[must_use = "dropping the guard shuts the span exporter down; keep it bound until `main` returns"]
#[derive(Debug)]
pub struct FlushGuard {
    provider: Option<TracerProvider>,
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            if let Err(err) = provider.shutdown() {
                tracing::warn!(error = %err, "OpenTelemetry span export shutdown failed");
            }
        }
    }
}

/// Why [`init`] could not install the subscriber.
#[derive(Debug)]
pub enum InitError {
    /// `RUST_LOG` holds bytes that are not valid Unicode. The message names the variable, never
    /// its value.
    NotUnicode,
    /// `RUST_LOG` holds a directive the filter cannot parse. The message carries only what the
    /// filter's own error reports.
    Filter(FromEnvError),
    /// A global subscriber is already installed in this process.
    Install(TryInitError),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUnicode => f.write_str("environment variable RUST_LOG is not valid Unicode"),
            Self::Filter(err) => write!(f, "invalid RUST_LOG filter: {err}"),
            Self::Install(err) => write!(f, "tracing subscriber already installed: {err}"),
        }
    }
}

impl Error for InitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NotUnicode => None,
            Self::Filter(err) => Some(err),
            Self::Install(err) => Some(err),
        }
    }
}

/// The span of one HTTP request, named `http.request`.
///
/// It records exactly four fields: `method`, `route` (the `MatchedPath` route template such as
/// `/api/v1/accounts/{acct_id}/view`, or [`UNMATCHED_ROUTE`]), and `status` and `latency_ms`,
/// which [`record_response`] fills in. Never add the URI, the query string, a header or the body:
/// path parameters and query values can hold card numbers and other protected values.
///
/// `api::app` calls it from tower-http's `TraceLayer::make_span_with` with the request method
/// string and the route template, so the api passes plain strings and this crate needs no HTTP
/// type. The request future runs inside the span through `tracing::Instrument::instrument`, as
/// `TraceLayer` does; never hold `Span::enter` across an `.await`.
#[must_use]
pub fn request_span(method: &str, matched_path: &str) -> Span {
    info_span!(
        "http.request",
        method,
        route = matched_path,
        status = Empty,
        latency_ms = Empty
    )
}

/// Records the response status and the latency, in whole milliseconds, on a span made by
/// [`request_span`].
///
/// Called from tower-http's `TraceLayer::on_response`, so the api performs no conversion itself.
pub fn record_response(span: &Span, status: u16, latency: Duration) {
    span.record("status", status);
    // TRUNCATION: `Duration::as_millis` is 128-bit; a latency beyond the 64-bit millisecond range
    // (about 584 million years) saturates at `u64::MAX` instead of wrapping.
    let latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX);
    span.record("latency_ms", latency_ms);
}

/// The span of one broker message, named `mq.message`.
///
/// It records the `queue` the message was taken from and, when the message carries them, its
/// `message_id` and `correlation_id` properties. Never the body: request and reply bodies hold
/// card numbers and account data. A worker runs its processing future inside the span through
/// `tracing::Instrument::instrument`; never hold `Span::enter` across an `.await`.
#[must_use]
pub fn message_span(queue: &str, message_id: Option<&str>, correlation_id: Option<&str>) -> Span {
    let span = info_span!(
        "mq.message",
        queue,
        message_id = Empty,
        correlation_id = Empty
    );
    if let Some(message_id) = message_id {
        span.record("message_id", message_id);
    }
    if let Some(correlation_id) = correlation_id {
        span.record("correlation_id", correlation_id);
    }
    span
}

/// The span of one batch job run, named `batch.job`, recording only the `job` (subcommand) name.
///
/// Never add the content of an input or output record or any protected value. Attach it to the
/// job's future with `tracing::Instrument::instrument`; never hold `Span::enter` across an
/// `.await`.
#[must_use]
pub fn job_span(job: &str) -> Span {
    info_span!("batch.job", job)
}

#[cfg(test)]
mod tests {
    use std::env::VarError;
    use std::error::Error as _;
    use std::ffi::OsString;
    use std::io;
    use std::os::unix::ffi::OsStringExt;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
    use opentelemetry_sdk::trace::TracerProvider;
    use tracing_subscriber::filter::{EnvFilter, FromEnvError, LevelFilter};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{
        ALERT_TARGET, InitError, OtelExport, UNMATCHED_ROUTE, env_filter, init, job_span,
        json_layer, message_span, otel_layer, record_response, request_span,
    };

    /// In-memory writer standing in for standard output.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        fn text(&self) -> String {
            let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Runs `body` under a thread-local subscriber made of [`json_layer`] alone and returns the
    /// JSON it wrote. Span fields appear only on events emitted inside the span.
    fn capture(body: impl FnOnce()) -> String {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(json_layer(captured.clone()));
        tracing::subscriber::with_default(subscriber, body);
        captured.text()
    }

    fn assert_contains_all(output: &str, expected: &[&str]) {
        for needle in expected {
            assert!(output.contains(needle), "{needle} missing from {output}");
        }
    }

    /// In-memory span exporter standing in for `opentelemetry-stdout`: it keeps every span it is
    /// handed, as the stdout exporter would print it.
    #[derive(Clone, Debug, Default)]
    struct Exported(Arc<Mutex<Vec<SpanData>>>);

    impl Exported {
        fn spans(&self) -> Vec<SpanData> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl SpanExporter for Exported {
        fn export(
            &mut self,
            batch: Vec<SpanData>,
        ) -> Pin<Box<dyn Future<Output = ExportResult> + Send + 'static>> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(batch);
            Box::pin(std::future::ready(Ok(())))
        }
    }

    #[test]
    fn request_span_records_method_route_status_and_latency_only() {
        let output = capture(|| {
            let span = request_span("GET", "/api/v1/accounts/{acct_id}/view");
            record_response(&span, 200, Duration::from_millis(12));
            span.in_scope(|| tracing::info!("probe"));
        });
        assert_contains_all(
            &output,
            &[
                r#""name":"http.request""#,
                r#""method":"GET""#,
                r#""route":"/api/v1/accounts/{acct_id}/view""#,
                r#""status":200"#,
                r#""latency_ms":12"#,
                r#""message":"probe""#,
            ],
        );
        assert!(!output.contains("uri"), "{output}");
        assert!(!output.contains('?'), "{output}");
    }

    #[test]
    fn request_span_before_the_response_has_no_status_or_latency() {
        let output = capture(|| {
            request_span("POST", UNMATCHED_ROUTE).in_scope(|| tracing::info!("probe"));
        });
        assert_contains_all(&output, &[r#""method":"POST""#, r#""route":"<unmatched>""#]);
        assert!(!output.contains(r#""status""#), "{output}");
        assert!(!output.contains(r#""latency_ms""#), "{output}");
    }

    #[test]
    fn latency_beyond_the_millisecond_range_saturates() {
        let output = capture(|| {
            let span = request_span("PUT", "/api/v1/cards/{card_ref}");
            record_response(&span, 500, Duration::MAX);
            span.in_scope(|| tracing::info!("probe"));
        });
        assert_contains_all(
            &output,
            &[r#""status":500"#, r#""latency_ms":18446744073709551615"#],
        );
    }

    #[test]
    fn message_span_records_queue_and_present_ids_only() {
        let with_ids = capture(|| {
            message_span("CARD.DEMO.REQUEST.PAUTH", Some("m-1"), Some("c-1"))
                .in_scope(|| tracing::info!("probe"));
        });
        assert_contains_all(
            &with_ids,
            &[
                r#""name":"mq.message""#,
                r#""queue":"CARD.DEMO.REQUEST.PAUTH""#,
                r#""message_id":"m-1""#,
                r#""correlation_id":"c-1""#,
            ],
        );

        let without_ids = capture(|| {
            message_span("CARD.DEMO.REQUEST.DATE", None, None).in_scope(|| tracing::info!("probe"));
        });
        assert_contains_all(&without_ids, &[r#""queue":"CARD.DEMO.REQUEST.DATE""#]);
        assert!(!without_ids.contains("message_id"), "{without_ids}");
        assert!(!without_ids.contains("correlation_id"), "{without_ids}");
    }

    #[test]
    fn job_span_records_the_job_name() {
        let output = capture(|| job_span("posttran").in_scope(|| tracing::info!("probe")));
        assert_contains_all(&output, &[r#""name":"batch.job""#, r#""job":"posttran""#]);
    }

    #[test]
    fn alerts_are_error_events_with_the_alert_target() {
        let output = capture(|| {
            tracing::error!(target: ALERT_TARGET, alert = "CP00_CARD_UNPARSEABLE", "probe");
        });
        assert_contains_all(
            &output,
            &[
                r#""target":"carddemo::alert""#,
                r#""level":"ERROR""#,
                r#""alert":"CP00_CARD_UNPARSEABLE""#,
            ],
        );
    }

    #[test]
    fn exported_spans_carry_their_fields_but_no_source_location() {
        let exported = Exported::default();
        let provider = TracerProvider::builder()
            .with_simple_exporter(exported.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(otel_layer(&provider));
        tracing::subscriber::with_default(subscriber, || {
            job_span("posttran").in_scope(|| tracing::info!(step = "load", "probe"));
        });

        let spans = exported.spans();
        let [span] = spans.as_slice() else {
            panic!("expected exactly one exported span, got {spans:?}");
        };
        assert_eq!(span.name, "batch.job");
        assert!(
            span.attributes
                .iter()
                .any(|kv| kv.key.as_str() == "job" && kv.value.as_str() == "posttran"),
            "{span:?}"
        );
        let [event] = &*span.events else {
            panic!("expected exactly one span event, got {span:?}");
        };
        assert_eq!(event.name, "probe");
        assert!(
            event
                .attributes
                .iter()
                .any(|kv| kv.key.as_str() == "step" && kv.value.as_str() == "load"),
            "{event:?}"
        );

        let location_keys: Vec<&str> = span
            .attributes
            .iter()
            .chain(&event.attributes)
            .map(|kv| kv.key.as_str())
            .filter(|key| key.starts_with("code."))
            .collect();
        assert!(location_keys.is_empty(), "{location_keys:?} in {span:?}");
    }

    #[test]
    fn filter_errors_carry_only_the_inner_message() {
        let not_present = InitError::Filter(FromEnvError::from(VarError::NotPresent));
        assert_eq!(
            not_present.to_string(),
            "invalid RUST_LOG filter: environment variable not found"
        );
        assert!(not_present.source().is_some());

        let parse = EnvFilter::builder()
            .parse("runtime=loud")
            .expect_err("`loud` is not a level");
        let invalid = InitError::Filter(FromEnvError::from(parse));
        assert!(
            invalid.to_string().starts_with("invalid RUST_LOG filter: "),
            "{invalid}"
        );
        assert!(invalid.source().is_some());
        assert!(format!("{invalid:?}").starts_with("Filter("));
    }

    #[test]
    fn rust_log_outcomes_give_the_default_filter_or_a_value_free_error() {
        /// What `env_filter` must make of one failed `try_from_env`.
        #[derive(Debug)]
        enum Expected {
            DefaultInfo,
            NotUnicode,
            Filter,
        }

        let builder = EnvFilter::builder().with_default_directive(LevelFilter::INFO.into());
        let unparsable = builder
            .parse("runtime=loud")
            .expect_err("`loud` is not a level");
        let not_unicode = OsString::from_vec(b"trace\xffX".to_vec());
        let cases = [
            (
                "unset",
                FromEnvError::from(VarError::NotPresent),
                Expected::DefaultInfo,
            ),
            (
                "not valid Unicode",
                FromEnvError::from(VarError::NotUnicode(not_unicode)),
                Expected::NotUnicode,
            ),
            (
                "unparsable directive",
                FromEnvError::from(unparsable),
                Expected::Filter,
            ),
        ];
        for (case, read, expected) in cases {
            match (env_filter(&builder, Err(read)), expected) {
                (Ok(filter), Expected::DefaultInfo) => {
                    assert_eq!(filter.max_level_hint(), Some(LevelFilter::INFO), "{case}");
                }
                (Err(err @ InitError::NotUnicode), Expected::NotUnicode) => {
                    let display = err.to_string();
                    assert_eq!(
                        display, "environment variable RUST_LOG is not valid Unicode",
                        "{case}"
                    );
                    assert!(err.source().is_none(), "{case}");
                    for text in [display, format!("{err:?}")] {
                        for value in ["trace", "\\x", "\u{fffd}"] {
                            assert!(!text.contains(value), "{case}: {text}");
                        }
                    }
                }
                (Err(err @ InitError::Filter(_)), Expected::Filter) => {
                    assert!(
                        err.to_string().starts_with("invalid RUST_LOG filter: "),
                        "{case}: {err}"
                    );
                }
                (outcome, expected) => panic!("{case}: expected {expected:?}, got {outcome:?}"),
            }
        }
    }

    // The only test that calls `init`: a global subscriber can be installed once per process.
    #[test]
    fn init_installs_the_global_subscriber_once() {
        match init(OtelExport::Stdout) {
            Ok(guard) => {
                // A closed span passes through the `OpenTelemetry` layer and the stdout exporter.
                job_span("telemetry-init").in_scope(|| {});

                let second = init(OtelExport::Off).expect_err("a second install must fail");
                assert!(matches!(second, InitError::Install(_)), "{second:?}");
                assert!(
                    second
                        .to_string()
                        .starts_with("tracing subscriber already installed: "),
                    "{second}"
                );
                assert!(second.source().is_some());

                assert!(format!("{guard:?}").contains("provider: Some"), "{guard:?}");
                drop(guard);
            }
            // An invalid or non-Unicode `RUST_LOG` in the test environment stops `init` before it
            // installs anything, which are the documented `Filter` and `NotUnicode` outcomes.
            Err(InitError::Filter(err)) => assert!(!err.to_string().is_empty()),
            Err(err @ InitError::NotUnicode) => assert!(err.source().is_none()),
            Err(other) => panic!("first install must succeed, got {other:?}"),
        }
    }
}
