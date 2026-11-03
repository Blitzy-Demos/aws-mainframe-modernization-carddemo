//! Telemetry initialisation shared by the `api`, `batch` and `mq` binaries.
//!
//! One writer, standard output in the binaries, receives two kinds of JSON line:
//!
//! - log events, written by the `tracing-subscriber` JSON formatter with the
//!   current span (`span`) and the list of entered spans (`spans`). An event
//!   inside a span also carries that span's OpenTelemetry `trace_id` and
//!   `span_id`, equal to the IDs of that span's line, so every event joins the
//!   span it was emitted in; an event outside every span carries neither;
//! - finished spans, which pass through `tracing-opentelemetry` into an
//!   OpenTelemetry SDK tracer provider whose simple span processor hands each one
//!   to [`JsonSpanExporter`], which writes it as one `"type":"span"` line.
//!
//! Standard output therefore holds JSON lines only, and nothing in this module
//! prints anywhere else. A failed shutdown at drop time is reported the same way,
//! as one more JSON log line on the same writer (see [`TelemetryGuard`]).
//!
//! # Filtering
//!
//! The filter directives (`RUST_LOG`, default `info`) gate events and spans alike:
//! a span below the filter level is neither formatted nor exported. A span meant
//! to reach the span lines under the default filter must be created at INFO level
//! or above, for example with `tracing::info_span!`.
//!
//! A second, fixed filter drops two kinds of output of lapin, the AMQP client,
//! whatever the directives say:
//!
//! - every TRACE-level span and event: there lapin writes every outgoing frame,
//!   the one carrying the broker username and password included;
//! - every span and event of `lapin::returned_messages`, at every level: there
//!   lapin debug-formats each message the broker returns at WARN, its whole body
//!   included, and a returned CP00 reply's body holds the full card number. The
//!   `mq` consumer reports the returned publish itself, without the body, as its
//!   `reply not confirmed` event inside the message span.
//!
//! lapin's other output at DEBUG level and above stays subject to the
//! directives alone.
//!
//! Every span the filters enable is exported with its attributes: the tracer
//! provider samples all spans and keeps the SDK's default span limits, whatever
//! the `OTEL_TRACES_SAMPLER` and `OTEL_SPAN_*_COUNT_LIMIT` variables say.
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

use opentelemetry::KeyValue;
use opentelemetry::trace::{
    SpanId, TraceContextExt as _, TraceError, TraceId, TracerProvider as _,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{Sampler, SpanLimits, TracerProvider};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_opentelemetry::OtelData;
use tracing_subscriber::filter::{EnvFilter, ParseError, filter_fn};
use tracing_subscriber::fmt::format::{
    Format, FormatEvent, FormatFields, Json, JsonFields, Writer,
};
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::fmt::{self, FmtContext, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};

use crate::config::{keys, rust_log_directives};
use crate::json_span_exporter::JsonSpanExporter;

/// Resource attribute naming the emitting process; the exporter writes its value
/// as the `service` key of every span line.
const SERVICE_NAME_KEY: &str = "service.name";

/// Target of the lapin crate's root module; its submodules' targets continue
/// with `::`.
const LAPIN_TARGET: &str = "lapin";

/// Target of the lapin module that logs each message the broker returns.
const LAPIN_RETURNED_MESSAGES_TARGET: &str = "lapin::returned_messages";

/// Whether `target` names the module `module` or one of its submodules: it is
/// `module` itself, or `module` followed by `::`. A target such as `lapin_ext`
/// is not in `lapin`, and `lapin::returned_messages_ext` is not in
/// `lapin::returned_messages`: each names a module of its own.
fn is_in_module(target: &str, module: &str) -> bool {
    target
        .strip_prefix(module)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// Whether a span or event may reach the JSON formatter and the OpenTelemetry
/// layer: everything except lapin's TRACE level and, at every level, lapin's
/// returned-message output.
///
/// lapin writes every outgoing AMQP frame at TRACE (`will write to buffer`,
/// target `lapin::io_loop`). Among them is the `connection.start-ok` frame, whose
/// SASL PLAIN response holds the broker username and password, debug-formatted
/// as a list of bytes. Its other TRACE output debug-formats frames, protocol
/// methods and connection state as well.
///
/// When the broker returns a mandatory publish, lapin debug-formats the whole
/// returned message at WARN (`Server returned us a message`, target
/// `lapin::returned_messages`), its body included as a list of bytes, before
/// the publisher confirmation reaches the caller. A returned CP00 reply's body
/// starts with the full card number. The `mq` consumer reports a returned
/// publish itself, without the body: its `reply not confirmed` event inside the
/// message span carries the flow, the reply queue and the
/// `PublishFailure::Returned` failure (`returned unroutable`).
///
/// lapin's other output at DEBUG level and above carries neither credentials
/// nor message bodies, so it is left to the filter directives.
///
/// The predicate runs as a filter layer of its own, and the subscriber enables a
/// span or event only when every filter layer does. No `RUST_LOG` directive can
/// therefore re-enable this output, neither a target directive such as
/// `lapin::returned_messages=warn` nor a span directive such as
/// `[outer]=trace`, which enables every event inside `outer` whatever the target
/// directives say. Targets are matched by module, as [`is_in_module`] describes.
fn carries_no_sensitive_data(metadata: &Metadata<'_>) -> bool {
    let target = metadata.target();
    if is_in_module(target, LAPIN_RETURNED_MESSAGES_TARGET) {
        return false;
    }
    !is_in_module(target, LAPIN_TARGET) || *metadata.level() != Level::TRACE
}

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
/// that call shuts the provider down itself and reports a failure as one ERROR
/// event, `telemetry shutdown failed` with an `error` field, written as a JSON
/// log line to the subscriber's writer, standard output in the binaries. The
/// line comes from a JSON formatter of its own with the same settings as the
/// subscriber's, so it has the shape of every other log line, is written
/// whatever the filter directives say, and never passes through the provider
/// that failed to shut down. That subscriber has no OpenTelemetry layer and the
/// event no span, so the line carries no `trace_id` or `span_id`.
///
/// The provider is held in an `Option` and taken on the first shutdown, so a
/// guard dropped after an explicit shutdown does not shut down a second time,
/// which would fail and write a spurious error line.
#[must_use]
#[derive(Debug)]
pub struct TelemetryGuard {
    provider: Option<TracerProvider>,
    /// The subscriber's writer, taken by the drop-time failure report.
    writer: Option<BoxMakeWriter>,
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
        let Err(err) = self.shut_down_provider() else {
            return;
        };
        let Some(writer) = self.writer.take() else {
            return;
        };
        // A subscriber of its own, with the JSON formatter alone, installed for
        // this one event: the line keeps the JSON shape of standard output, and
        // no OpenTelemetry layer routes it to the provider that failed. The
        // formatter drops a failed write, as there is nothing further to report to.
        let reporter = tracing_subscriber::registry().with(json_log_layer(writer));
        tracing::subscriber::with_default(reporter, || {
            tracing::error!(error = %err, "telemetry shutdown failed");
        });
    }
}

/// The JSON formatter of the log event lines, writing to `writer`: the current
/// span (`span`) and the list of entered spans (`spans`) on every event line,
/// the OpenTelemetry `trace_id` and `span_id` of that span on an event inside
/// one (see [`JsonWithTraceIds`]), and no ANSI colour codes.
fn json_log_layer<S, W>(writer: W) -> fmt::Layer<S, JsonFields, JsonWithTraceIds, W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'a> MakeWriter<'a> + 'static,
{
    fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_ansi(false)
        .with_writer(writer)
        .map_event_format(JsonWithTraceIds)
}

/// The event formatter of the log lines: `tracing-subscriber`'s JSON formatter,
/// which writes each line, plus the OpenTelemetry IDs of the event's span.
///
/// The span is the one the JSON formatter writes as `span`: the event's explicit
/// parent, else the current span. When its OpenTelemetry data yields a valid
/// trace ID and span ID ([`span_trace_ids`]), they become the last two keys of
/// the line's object: `"trace_id"` as 32 and `"span_id"` as 16 lowercase
/// hexadecimal digits, the IDs that span's own line carries. Otherwise the JSON
/// formatter's line is written unchanged: for an event outside every span, and
/// for one under a subscriber without the OpenTelemetry layer, such as the
/// drop-time failure reporter of [`TelemetryGuard`].
#[derive(Debug)]
struct JsonWithTraceIds(Format<Json>);

impl<S, N> FormatEvent<S, N> for JsonWithTraceIds
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    /// Has the JSON formatter write the line into a buffer of its own, then
    /// writes that line with the IDs inserted before the object's closing brace.
    /// The JSON formatter ends every line with `}` and a newline; a line that
    /// does not is written unchanged, never spliced into an invalid object. When
    /// the JSON formatter fails, nothing is written, and the error makes the
    /// layer drop the event's line.
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let mut line = String::new();
        self.0.format_event(ctx, Writer::new(&mut line), event)?;
        let ids = event
            .parent()
            .and_then(|id| ctx.span(id))
            .or_else(|| ctx.lookup_current())
            .and_then(|span| span.extensions().get::<OtelData>().and_then(span_trace_ids));
        let object = line
            .strip_suffix("}\n")
            .filter(|object| object.starts_with('{'));
        if let (Some((trace_id, span_id)), Some(object)) = (ids, object) {
            let separator = if object.len() > 1 { "," } else { "" };
            writeln!(
                writer,
                "{object}{separator}\"trace_id\":\"{trace_id}\",\"span_id\":\"{span_id}\"}}"
            )
        } else {
            writer.write_str(&line)
        }
    }
}

/// The trace ID and span ID of the span whose OpenTelemetry data is `otel`, or
/// `None` when either is missing or invalid.
///
/// A span whose parent context holds an active span belongs to that span's
/// trace; a root span belongs to the trace ID `tracing-opentelemetry` assigned
/// it at creation, when it also assigned every span its own span ID. These are
/// the rules by which `tracing-opentelemetry` and the SDK tracer give the
/// exported span its IDs, so the result equals the `trace_id` and `span_id` of
/// the span's line.
fn span_trace_ids(otel: &OtelData) -> Option<(TraceId, SpanId)> {
    let trace_id = if otel.parent_cx.has_active_span() {
        otel.parent_cx.span().span_context().trace_id()
    } else {
        otel.builder.trace_id?
    };
    let span_id = otel.builder.span_id?;
    (trace_id != TraceId::INVALID && span_id != SpanId::INVALID).then_some((trace_id, span_id))
}

/// Builds the subscriber [`init`] installs, writing every JSON line to `writer`.
///
/// The layers, in order:
///
/// 1. an `EnvFilter` parsed from `directives`, which gates events and spans for
///    the layers after it;
/// 2. a fixed filter that drops, for the layers after it and whatever
///    `directives` say, lapin's TRACE-level spans and events, whose outgoing
///    `connection.start-ok` frame holds the broker credentials, and every span
///    and event of `lapin::returned_messages`, whose WARN dump of a returned
///    message holds its whole body;
/// 3. the `tracing-subscriber` JSON formatter, with the current span and the
///    span list on every event line, that span's `trace_id` and `span_id` on an
///    event inside one, and no ANSI colour codes;
/// 4. the `tracing-opentelemetry` layer, on a tracer named `service_name` from a
///    tracer provider of its own whose simple span processor feeds a
///    [`JsonSpanExporter`] on `writer`, with `service_name` as the resource's
///    `service.name`. The provider samples every span (`Sampler::AlwaysOn`)
///    and keeps the SDK's default span limits (128 attributes, events and
///    links per span), both set explicitly.
///
/// No global state is touched: the tracer provider is not registered globally,
/// so tests can build several subscribers side by side and install each one
/// with `tracing::subscriber::with_default`. Each subscriber's spans go only to
/// its own writer. `directives` are used as given, and the SDK's sampler and
/// span limit variables (`OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG`,
/// `OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT`, `OTEL_SPAN_EVENT_COUNT_LIMIT`,
/// `OTEL_SPAN_LINK_COUNT_LIMIT`) have no effect.
///
/// The returned guard shuts the provider down and reports a failed drop-time
/// shutdown as a JSON line on `writer`; see [`TelemetryGuard`].
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
        .with_sampler(Sampler::AlwaysOn)
        .with_span_limits(SpanLimits::default())
        .with_simple_exporter(JsonSpanExporter::new(writer.clone()))
        .with_resource(Resource::new([KeyValue::new(
            SERVICE_NAME_KEY,
            service_name.to_owned(),
        )]))
        .build();
    let tracer = provider.tracer(service_name.to_owned());
    let guard = TelemetryGuard {
        provider: Some(provider),
        writer: Some(BoxMakeWriter::new(writer.clone())),
    };
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(filter_fn(carries_no_sensitive_data))
        .with(json_log_layer(writer))
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    Ok((subscriber, guard))
}

/// Installs the JSON telemetry of the process `service_name` (`api`, `batch` or
/// `mq`) as the global default subscriber, writing to standard output.
///
/// The filter directives come from `RUST_LOG`. When that variable is not set,
/// not valid Unicode, or empty, the default `info` applies. The rule is
/// [`rust_log_directives`], the one the settings use as well, so
/// `Settings::rust_log` names the filter in effect.
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
    let directives = rust_log_directives(std::env::var(keys::RUST_LOG).ok());
    let (subscriber, guard) = subscriber_with_writer(service_name, &directives, std::io::stdout)?;
    subscriber.try_init()?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::error::Error;
    use std::io::{self, Write};
    use std::process::Command;
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

    /// `true` when `id` is a string of exactly `len` lowercase hexadecimal digits.
    fn is_lower_hex(id: &Json, len: usize) -> bool {
        id.as_str().is_some_and(|text| {
            text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
    }

    #[test]
    fn events_carry_the_ids_of_their_span_line() -> TestResult {
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("before any span");
            let outer = tracing::info_span!("outer");
            outer.in_scope(|| {
                tracing::info_span!("inner").in_scope(|| tracing::info!("in inner"));
                tracing::info!("in outer");
            });
            tracing::info!(parent: &outer, "explicit parent outer");
            drop(outer);
            tracing::info!("after every span");
        });
        guard.shutdown()?;

        let lines = capture.json_lines()?;
        let spans = span_lines(&lines);
        let span = |name: &str| {
            spans
                .iter()
                .copied()
                .find(|line| line["name"] == name)
                .ok_or_else(|| format!("no `{name}` span line: {lines:?}"))
        };
        let event = |message: &str| {
            lines
                .iter()
                .find(|line| line["fields"]["message"] == message)
                .ok_or_else(|| format!("no `{message}` event line: {lines:?}"))
        };
        let (outer, inner) = (span("outer")?, span("inner")?);
        assert_eq!(inner["parent_span_id"], outer["span_id"], "{inner}");

        for (message, owner) in [
            ("in inner", inner),
            ("in outer", outer),
            ("explicit parent outer", outer),
        ] {
            let line = event(message)?;
            assert!(is_lower_hex(&line["trace_id"], 32), "{line}");
            assert!(is_lower_hex(&line["span_id"], 16), "{line}");
            assert_eq!(line["trace_id"], owner["trace_id"], "{line}");
            assert_eq!(line["span_id"], owner["span_id"], "{line}");
            assert_eq!(line["span"]["name"], owner["name"], "{line}");
        }
        assert_eq!(
            event("in inner")?["trace_id"],
            outer["trace_id"],
            "nested spans share one trace"
        );
        for message in ["before any span", "after every span"] {
            let line = event(message)?;
            assert!(line.get("trace_id").is_none(), "{line}");
            assert!(line.get("span_id").is_none(), "{line}");
        }
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

    /// Path of [`every_span_is_exported_with_its_attributes`] in this test binary,
    /// which `ambient_sdk_variables_drop_no_span` runs in a child process.
    const EXPORT_TEST: &str = "telemetry::tests::every_span_is_exported_with_its_attributes";

    #[test]
    fn every_span_is_exported_with_its_attributes() -> TestResult {
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("write", transaction = "CAUP").in_scope(|| {});
        });
        guard.shutdown()?;

        let lines = capture.json_lines()?;
        let [write_line] = span_lines(&lines)[..] else {
            return Err(format!("expected exactly one span line: {lines:?}").into());
        };
        assert_eq!(write_line["name"], "write");
        assert_eq!(write_line["attributes"]["transaction"], "CAUP");
        Ok(())
    }

    /// The SDK's default configuration reads its sampler and span limits from
    /// the environment. The process environment cannot be changed from a test,
    /// so the export test runs in a child process with each setting that would
    /// drop the span or its attribute.
    #[test]
    fn ambient_sdk_variables_drop_no_span() -> TestResult {
        let cases: [&[(&str, &str)]; 4] = [
            &[("OTEL_TRACES_SAMPLER", "always_off")],
            &[
                ("OTEL_TRACES_SAMPLER", "traceidratio"),
                ("OTEL_TRACES_SAMPLER_ARG", "0"),
            ],
            &[("OTEL_TRACES_SAMPLER", "parentbased_always_off")],
            &[("OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT", "0")],
        ];
        for vars in cases {
            let output = Command::new(env::current_exe()?)
                .args([EXPORT_TEST, "--exact", "--test-threads=1"])
                .envs(vars.iter().copied())
                .output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{vars:?}: {stdout}{stderr}");
            assert!(stdout.contains("1 passed"), "{vars:?}: {stdout}");
        }
        Ok(())
    }

    #[test]
    fn a_failed_shutdown_at_drop_is_one_json_error_line() -> TestResult {
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("job").in_scope(|| tracing::info!("posted"));
        });
        // Shut the provider down through another handle, so that the guard's own
        // shutdown at drop time fails.
        guard
            .provider
            .clone()
            .ok_or("guard without a provider")?
            .shutdown()?;
        drop(guard);

        let lines = capture.json_lines()?;
        let [.., last] = &lines[..] else {
            return Err("telemetry wrote nothing".into());
        };
        assert_eq!(last["level"], "ERROR", "{last}");
        assert_eq!(
            last["fields"]["message"], "telemetry shutdown failed",
            "{last}"
        );
        let error = last["fields"]["error"].as_str().unwrap_or_default();
        assert!(
            error.starts_with("failed to shut down the tracer provider: "),
            "{last}"
        );
        assert!(last.get("trace_id").is_none(), "{last}");
        assert!(last.get("span_id").is_none(), "{last}");
        let errors = lines.iter().filter(|line| line["level"] == "ERROR").count();
        assert_eq!(errors, 1, "{lines:?}");
        Ok(())
    }

    #[test]
    fn an_explicit_shutdown_leaves_nothing_for_drop_to_write() -> TestResult {
        let capture = Capture::default();
        let (_subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        guard.shutdown()?;
        assert!(capture.json_lines()?.is_empty(), "drop wrote a line");

        // A failing explicit shutdown is the caller's to report; drop adds nothing.
        let (_subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        guard
            .provider
            .clone()
            .ok_or("guard without a provider")?
            .shutdown()?;
        let result = guard.shutdown();
        assert!(
            matches!(result, Err(TelemetryError::Shutdown(_))),
            "{result:?}"
        );
        assert!(capture.json_lines()?.is_empty(), "drop wrote a line");
        Ok(())
    }

    /// Stand-in for the SASL PLAIN response lapin debug-formats into its
    /// `connection.start-ok` frame line.
    const SASL_MARKER: &str = "SASL-PLAIN-PASSWORD-MARKER";

    /// Stand-in for the full card number that starts the body of a returned
    /// CP00 reply, which lapin debug-formats into its returned-message line.
    const PAN_MARKER: &str = "0500024453765740";

    /// Control: a lapin DEBUG event, target `lapin::channels`.
    const CHANNELS_DEBUG: &str = "create channel";
    /// Control: a lapin DEBUG event inside a lapin TRACE span.
    const IO_LOOP_DEBUG: &str = "io loop debug control";
    /// Control: a TRACE event of the crate `lapin_ext`, which is not lapin.
    const NEIGHBOUR_TRACE: &str = "neighbour crate trace control";
    /// Control: an application TRACE event.
    const APP_TRACE: &str = "application trace control";
    /// Control: a WARN event of the lapin module `lapin::returned_messages_ext`,
    /// which is not the returned-message module.
    const RETURNED_NEIGHBOUR_WARN: &str = "neighbour module warn control";

    /// Emits, inside the INFO span `outer`, lapin's TRACE output carrying
    /// [`SASL_MARKER`], its returned-message output carrying [`PAN_MARKER`] as
    /// an event, a submodule event and a span, and the control events, which
    /// the filter directives alone decide on.
    fn emit_lapin_output() {
        // Stand-in for the `BasicReturnMessage` lapin debug-formats: the
        // returned reply's routing key and body.
        let message = ("QA.NOPE", PAN_MARKER);
        tracing::info_span!("outer").in_scope(|| {
            tracing::warn!(
                target: "lapin::returned_messages",
                ?message,
                "Server returned us a message"
            );
            tracing::error!(
                target: "lapin::returned_messages::delivery",
                body = PAN_MARKER,
                "returned delivery"
            );
            tracing::warn_span!(
                target: "lapin::returned_messages",
                "returned_message",
                body = PAN_MARKER
            )
            .in_scope(|| {});
            tracing::warn!(
                target: "lapin::returned_messages_ext",
                "{RETURNED_NEIGHBOUR_WARN}"
            );
            tracing::trace!(
                target: "lapin::io_loop",
                next_msg = SASL_MARKER,
                "will write to buffer"
            );
            tracing::trace!(
                target: "lapin",
                next_msg = SASL_MARKER,
                "will write to buffer"
            );
            tracing::trace_span!(target: "lapin::io_loop", "io_loop", frame = SASL_MARKER)
                .in_scope(|| tracing::debug!(target: "lapin::io_loop", "{IO_LOOP_DEBUG}"));
            tracing::debug!(target: "lapin::channels", "{CHANNELS_DEBUG}");
            tracing::trace!(target: "lapin_ext", "{NEIGHBOUR_TRACE}");
            tracing::trace!("{APP_TRACE}");
        });
    }

    #[test]
    fn sensitive_lapin_output_is_never_written() -> TestResult {
        // Each directive list enables lapin's TRACE output, its returned-message
        // output, or both; the messages are the controls it also enables, which
        // must still be written.
        let cases: [(&str, &[&str]); 6] = [
            (
                "trace",
                &[
                    CHANNELS_DEBUG,
                    IO_LOOP_DEBUG,
                    NEIGHBOUR_TRACE,
                    APP_TRACE,
                    RETURNED_NEIGHBOUR_WARN,
                ],
            ),
            // A target directive matches by prefix, so `lapin` covers `lapin_ext`.
            (
                "lapin=trace",
                &[
                    CHANNELS_DEBUG,
                    IO_LOOP_DEBUG,
                    NEIGHBOUR_TRACE,
                    RETURNED_NEIGHBOUR_WARN,
                ],
            ),
            ("lapin::io_loop=trace", &[IO_LOOP_DEBUG]),
            (
                "info,[outer]=trace",
                &[
                    CHANNELS_DEBUG,
                    IO_LOOP_DEBUG,
                    NEIGHBOUR_TRACE,
                    APP_TRACE,
                    RETURNED_NEIGHBOUR_WARN,
                ],
            ),
            // The default, which enables lapin's returned-message WARN.
            ("info", &[RETURNED_NEIGHBOUR_WARN]),
            // By prefix again, this covers `lapin::returned_messages_ext`.
            ("lapin::returned_messages=warn", &[RETURNED_NEIGHBOUR_WARN]),
        ];
        for (directives, expected) in cases {
            let capture = Capture::default();
            let (subscriber, guard) = subscriber_with_writer("mq", directives, capture.clone())?;
            tracing::subscriber::with_default(subscriber, emit_lapin_output);
            guard.shutdown()?;

            let lines = capture.json_lines()?;
            for line in &lines {
                let text = line.to_string();
                assert!(!text.contains(SASL_MARKER), "{directives:?}: {text}");
                assert!(!text.contains(PAN_MARKER), "{directives:?}: {text}");
                let target = line["target"].as_str().unwrap_or_default();
                assert!(
                    !(line["level"] == "TRACE"
                        && (target == "lapin" || target.starts_with("lapin::"))),
                    "{directives:?}: {text}"
                );
                assert!(
                    !(target == "lapin::returned_messages"
                        || target.starts_with("lapin::returned_messages::")),
                    "{directives:?}: {text}"
                );
            }
            let mut messages: Vec<&str> = lines
                .iter()
                .filter_map(|line| line["fields"]["message"].as_str())
                .collect();
            messages.sort_unstable();
            let mut expected = expected.to_vec();
            expected.sort_unstable();
            assert_eq!(messages, expected, "{directives:?}: {lines:?}");
        }
        Ok(())
    }
}
