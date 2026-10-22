//! Process telemetry of the `CardDemo` binaries: JSON tracing output on standard output, optional
//! `OpenTelemetry` span export to standard output, and the span shapes every service uses.
//!
//! [`init`] installs the global subscriber once per process: the [`dependency_filter`] that
//! silences the AMQP client's and its I/O reactor's own diagnostics, an `EnvFilter` read from
//! `RUST_LOG` (unset means `info`), the JSON layer of [`json_layer`] over standard output, a
//! second layer of that format that writes only the closing of each HTTP request span and,
//! when `CARDDEMO_OTEL_EXPORT=stdout`, a `tracing-opentelemetry` layer whose finished spans the
//! `opentelemetry-stdout` exporter prints. Output goes to standard output only: there is no
//! collector, no network exporter and no cloud service.
//!
//! # Standard output format
//!
//! With span export off, every line of standard output is one JSON object: an event, or the one
//! `INFO` line of a closed HTTP request span. `tracing-subscriber`'s fmt layer writes that line in
//! the same format when the span closes, so no event of this workspace's code produces it, and it
//! is how the default `info` output records every request. Its `span` member holds the method,
//! the route template, the status and the latency of [`request_span`], and its `fields` hold only
//! `message` (`close`) and the fmt layer's `time.busy` and `time.idle` durations, never a URI, a
//! header, a body or an identity. No other span's opening or closing is written.
//!
//! With `CARDDEMO_OTEL_EXPORT=stdout`, the `opentelemetry-stdout` exporter also writes each
//! finished span as a multi-line, human-readable text block, interleaved with the JSON lines; the
//! exporter has no JSON format. A consumer that parses standard output line by line as JSON
//! therefore runs with export off. The `Resource` block of the export names the emitting binary:
//! its `service.name` is the file name stem of the running executable (`carddemo-mq`,
//! `carddemo-batch`, `carddemo-migrate`, `carddemo-api`), never a directory of its path.
//!
//! Exported spans and their events carry no source-location attributes, and a JSON event of this
//! workspace's code or of a dependency's `tracing` instrumentation names no source file or line.
//! A record a dependency writes through the `log` crate is the exception: the `tracing-log`
//! bridge gives it `log.file` and `log.line` fields naming the dependency's source file under the
//! host's cargo registry. No event or span of the [`SILENCED_TARGETS`] reaches standard output,
//! at any `RUST_LOG`, and they include the reactor crates `polling` and `async_io`, which write
//! such a record for every poll.
//!
//! # Redaction boundary
//!
//! [`request_span`], [`record_response`], [`message_span`], [`record_reply_message_id`],
//! [`job_span`] and [`record_job_outcome`] are the only span shapes the services create and fill
//! in, and each carries only the fields its documentation lists; a job span's outcome is its
//! return code and its [`JobCounter`] counts, plain numbers. Everything a span or event records
//! reaches standard output, so a caller never adds a field holding a request or message body, a
//! URI, a query string, a header or a protected value (card number, CVV, SSN, password), and
//! never interpolates one into an event message.
//!
//! Text a requester chooses, such as the AMQP `message_id`, `correlation_id` and `reply_to`
//! properties, can itself be a card number, an SSN or a password, and a `CardDemo` password is
//! any one to eight printable bytes, spaces included, so nothing in the text tells a protected
//! value from a harmless one. Such text reaches a span or an event only as a [`ScreenedText`],
//! never as itself; [`message_span`] takes its two message IDs in that form.
//!
//! A message ID must still let an operator find the log lines of one request and tie them to its
//! reply. A [`ScreenedText`] made with an [`IdKey`] therefore shows the base64url HMAC-SHA256 of
//! the text under that key, never the text: the same ID gives the same 43 characters on every
//! line. The caller supplies the key's secret; `carddemo-mq` keys it with its
//! `CARDDEMO_AMQP_URL`, so an operator who holds the broker credentials recomputes the digest of
//! an ID they already know, in a shell that exported `modern/.env`, with
//!
//! ```text
//! python3 -c 'import base64,hashlib,hmac,os,sys; print(base64.urlsafe_b64encode(hmac.new(
//!   os.environ["CARDDEMO_AMQP_URL"].encode(), sys.argv[1].encode(), hashlib.sha256).digest()
//!   ).rstrip(b"=").decode())' "$ID"
//! ```
//!
//! The digest is keyed because an unkeyed hash would not hide a card number: its issuer prefix
//! is public and its last digit a check digit, so every candidate could be hashed and compared
//! with the log. An ID this system generates, the CP00 reply's `message_id` from
//! `messaging::ids`, holds no requester text and is recorded verbatim through
//! [`record_reply_message_id`].
//!
//! # Dependency boundary
//!
//! The redaction boundary binds the code of this workspace, and a dependency's own events bypass
//! it. The AMQP client `lapin` logs, at `TRACE`, every frame it reads or writes in `Debug` form,
//! the bytes of each message body and the `PLAIN` authentication response that carries the
//! broker password included, and at `ERROR` the close text the broker composes. The reactor
//! crates `polling` and `async_io` under `lapin`'s executor log every poll at `TRACE` through the
//! `log` crate, dozens of lines a second while idle, each naming its source file under the host's
//! cargo registry, and log nothing above `TRACE` on Linux. [`init`] therefore installs
//! [`dependency_filter`] as a global filter of its own, ahead of the `RUST_LOG` filter. An event
//! or span is recorded only when both filters enable it, so no `RUST_LOG` directive (`trace`,
//! `lapin=trace`, `lapin::channels=trace`, `polling=trace`, …) re-enables one of the
//! [`SILENCED_TARGETS`], while `RUST_LOG` alone, `info` by default, governs every other target in
//! every binary.
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
use std::path::Path;
use std::time::Duration;

use jsonwebtoken::{Algorithm, EncodingKey};
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::export::trace::SpanExporter;
use opentelemetry_sdk::trace::{Tracer, TracerProvider};
use tracing::callsite::{Callsite as _, DefaultCallsite};
use tracing::field::Empty;
use tracing::level_filters::STATIC_MAX_LEVEL;
use tracing::{Metadata, Span, Subscriber, info_span};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::filter::{
    Builder, EnvFilter, FromEnvError, LevelFilter, Targets, filter_fn,
};
use tracing_subscriber::fmt::format::{FmtSpan, Format, Json, JsonFields};
use tracing_subscriber::fmt::{Layer as FmtLayer, MakeWriter};
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::{SubscriberInitExt, TryInitError};

use crate::config::OtelExport;

/// Target of every alert event.
///
/// An alert is emitted as
/// `tracing::error!(target: runtime::telemetry::ALERT_TARGET, alert = "…", …)`. Its field values
/// name the condition and give each requester-chosen message identifier only as its keyed
/// [`ScreenedText`] shows it, the base64url HMAC-SHA256 under the same [`IdKey`] as the message
/// spans, so an alert is matched to the message spans of the same request; they never hold a
/// message body, the text of a requester's property or a protected value.
pub const ALERT_TARGET: &str = "carddemo::alert";

/// Route the api records when axum provides no `MatchedPath` (a request no route matched), so a
/// raw URI is never recorded in its place.
pub const UNMATCHED_ROUTE: &str = "<unmatched>";

/// Instrumentation scope name of the `OpenTelemetry` tracer.
const TRACER_NAME: &str = "carddemo";

/// Key of the `OpenTelemetry` resource attribute that names the emitting service. The SDK's own
/// constant for it is private to the SDK crate.
const SERVICE_NAME_KEY: &str = "service.name";

/// Name of the span [`request_span`] creates, the only span whose closing standard output records
/// (see [`request_close_layer`]).
const REQUEST_SPAN: &str = "http.request";

/// The targets whose events and spans are never recorded, whatever `RUST_LOG` says: the AMQP
/// client `lapin`, its protocol crates and `pinky_swear`, the promise crate behind every `lapin`
/// operation, and `polling` and `async_io`, the I/O reactor `lapin`'s executor runs on.
///
/// Their diagnostics pass through no redaction of this workspace. At `TRACE`, `lapin` logs every
/// frame in `Debug` form, which spells out the bytes of each message body (a CP00 request body
/// holds a full card number) and of the `PLAIN` authentication response (the broker user and
/// password); at `ERROR` and `INFO` it logs the channel and connection close text the broker
/// composes. `amq_protocol_tcp` logs the broker address, and `pinky_swear` traces the promises of
/// every broker operation.
///
/// The reactor crates log every poll at `TRACE` through the `log` crate, dozens of lines a second
/// while `carddemo-mq` is idle, and the `tracing-log` bridge gives each of those records
/// `log.file` and `log.line` fields naming the crate's source file under the host's cargo
/// registry. The versions this workspace builds log nothing above `TRACE` on Linux, so silencing
/// them hides no failure.
///
/// A target matches by string prefix, as `tracing_subscriber::filter::Targets` compares them:
/// `lapin` covers `lapin::channels` and every other module of the crate, `amq_protocol` covers
/// `amq_protocol_tcp`, `amq_protocol_uri` and `amq_protocol_types` as well, and `polling` and
/// `async_io` cover `polling::epoll`, `async_io::reactor` and `async_io::driver`.
pub const SILENCED_TARGETS: [&str; 5] = [
    "lapin",
    "amq_protocol",
    "pinky_swear",
    "polling",
    "async_io",
];

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

/// The filter that turns every target of [`SILENCED_TARGETS`] off and passes every other target
/// at every level.
///
/// [`init`] installs it as a global layer ahead of the `RUST_LOG` filter. Every global filter of
/// a subscriber must enable an event or span for any layer to record it, so the two filters
/// combine as an AND: no `RUST_LOG` directive re-enables a silenced target, and `RUST_LOG` alone
/// decides every other one. A record a dependency writes through the `log` crate is checked
/// against its own target, such as `polling::epoll`, before the `tracing-log` bridge dispatches
/// it, so the filter drops a silenced crate's `log` records as well as its `tracing` events.
/// `test-support`'s trace capture installs it too, so that a capture records exactly what
/// standard output can carry.
#[must_use]
pub fn dependency_filter() -> Targets {
    Targets::new()
        .with_default(LevelFilter::TRACE)
        .with_targets(SILENCED_TARGETS.map(|target| (target, LevelFilter::OFF)))
}

/// The layer that writes one `INFO` line in the [`json_layer`] format when a [`request_span`]
/// closes, and nothing else.
///
/// It is [`json_layer`] over `make_writer` with `FmtSpan::CLOSE`, behind a per-layer filter that
/// passes only the `http.request` span of this module. The layer therefore sees no event and no
/// other span: it writes no event, no span opening and no closing of a message or job span. The
/// line is `tracing-subscriber`'s own record of the close. Its `level` and `target` are the span's
/// (`INFO`, `runtime::telemetry`), its `fields` hold only `message` (`close`) and the fmt layer's
/// `time.busy` and `time.idle` durations, and its `span` member holds the span's recorded
/// `latency_ms`, `method`, `route` and `status` and its `name`. Its `spans` member lists only the
/// request spans entered where the span closes, and the api enters none there, so it is `[]`. The
/// line adds no other field, so it never carries a URI, a query string, a header, a body or an
/// identity:
///
/// ```text
/// {"timestamp":"…","level":"INFO","fields":{"message":"close","time.busy":"…","time.idle":"…"},
///  "target":"runtime::telemetry","span":{"latency_ms":12,"method":"GET",
///  "route":"/api/v1/accounts/{acct_id}/view","status":401,"name":"http.request"},"spans":[]}
/// ```
///
/// That is one line on standard output, wrapped here.
///
/// The per-layer filter decides for this layer alone, and only over what the global filters of
/// [`json_subscriber`] enable: a `RUST_LOG` that disables `INFO` for `runtime::telemetry`, such
/// as `warn`, disables the span, and the layer writes no line.
fn request_close_layer<S, W>(make_writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'w> MakeWriter<'w> + 'static,
{
    json_layer(make_writer)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter_fn(is_request_span))
}

/// Whether `metadata` is that of a [`request_span`]: a span named `http.request` whose target is
/// this module. An event, a span of another name, and a span of that name made in another module
/// are not.
fn is_request_span(metadata: &Metadata<'_>) -> bool {
    metadata.is_span() && metadata.name() == REQUEST_SPAN && metadata.target() == module_path!()
}

/// The subscriber [`init`] installs, short of its optional `OpenTelemetry` layer: the registry
/// with [`dependency_filter`] and then `env_filter` as global filters, the [`json_layer`] over
/// `make_writer`, which writes every event and no span opening or closing, and the
/// [`request_close_layer`] over the same writer, which writes the one line of each closed
/// [`request_span`].
///
/// Both filters sit below every layer, so they decide for the `OpenTelemetry` layer that [`init`]
/// adds on top as well. The unit tests build this same subscriber over an in-memory writer, so
/// they check the filtering and the output that [`init`] installs.
fn json_subscriber<W>(
    env_filter: EnvFilter,
    make_writer: W,
) -> impl Subscriber + for<'span> LookupSpan<'span> + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Clone + Send + Sync + 'static,
{
    tracing_subscriber::registry()
        .with(dependency_filter())
        .with(env_filter)
        .with(json_layer(make_writer.clone()))
        .with(request_close_layer(make_writer))
}

/// Installs the process-wide tracing subscriber.
///
/// The subscriber drops every event and span of the [`SILENCED_TARGETS`] through
/// [`dependency_filter`], whatever `RUST_LOG` says, filters every other target with `RUST_LOG`
/// (unset or empty means `info`), writes JSON lines to standard output through [`json_layer`],
/// one for each event and, through its request close layer, one `INFO` line for each closed
/// [`request_span`] carrying its method, route template, status and latency, and, for
/// [`OtelExport::Stdout`], also bridges spans into an `OpenTelemetry` tracer provider
/// whose simple span processor prints each finished span to standard output. [`OtelExport::Off`]
/// adds no `OpenTelemetry` layer. A binary calls it before it connects to a broker or a
/// database, so the silenced targets are off from the first frame.
///
/// With [`OtelExport::Stdout`], the `opentelemetry-stdout` exporter writes each finished span as a
/// multi-line, human-readable text block between the JSON event lines, so standard output is no
/// longer one JSON object per line. A consumer that parses standard output line by line as JSON
/// runs with [`OtelExport::Off`], where every line is one JSON object. Exported spans and their
/// events carry no source-location attributes.
///
/// The resource of the exported spans, which the exporter prints in its `Resource` block, names
/// the emitting binary: its `service.name` is the file name stem of the running executable, such
/// as `carddemo-mq` for `…/target/debug/carddemo-mq`, beside the SDK's `telemetry.sdk.*`
/// attributes. Only the stem is exported, never a directory of the path. When the executable path
/// is unavailable, or its stem is empty or not valid UTF-8, the resource is the SDK's default,
/// whose `service.name` is `unknown_service`.
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
        OtelExport::Stdout => {
            let executable = std::env::current_exe().ok();
            Some(tracer_provider(
                opentelemetry_stdout::SpanExporter::default(),
                service_resource(executable.as_deref()),
            ))
        }
    };
    let otel = provider.as_ref().map(otel_layer);

    json_subscriber(filter, std::io::stdout)
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

/// The `service.name` of the spans a process exports: the file name stem of its `executable`, so
/// `/…/target/debug/carddemo-mq` gives `carddemo-mq` and `carddemo-api.exe` gives
/// `carddemo-api`.
///
/// `None` when the path has no file name, or its stem is empty or not valid UTF-8. Only the stem is
/// returned, so no directory of the host path reaches standard output.
#[must_use]
fn service_name(executable: &Path) -> Option<&str> {
    executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
}

/// The resource of the tracer provider [`init`] builds: the SDK's default resource, its
/// `telemetry.sdk.*` attributes included, with `service.name` overridden by the [`service_name`]
/// of `executable`.
///
/// Without an executable path, or with one [`service_name`] finds no name in, it is the SDK's
/// default resource alone, whose `service.name` is the `OpenTelemetry` fallback
/// `unknown_service`.
#[must_use]
fn service_resource(executable: Option<&Path>) -> Resource {
    match executable.and_then(service_name) {
        Some(name) => {
            Resource::new_with_defaults([KeyValue::new(SERVICE_NAME_KEY, name.to_owned())])
        }
        None => Resource::default(),
    }
}

/// A tracer provider whose simple span processor hands each finished span to `exporter`, with
/// `resource` as the resource of every span.
///
/// Building the provider passes `resource` on to the exporter, which is how `opentelemetry-stdout`
/// learns what to print in its `Resource` block. [`init`] builds its provider here over that
/// exporter, and the unit tests build theirs here over an in-memory one, so they check the
/// construction [`init`] uses.
#[must_use]
fn tracer_provider<E>(exporter: E, resource: Resource) -> TracerProvider
where
    E: SpanExporter + 'static,
{
    TracerProvider::builder()
        .with_simple_exporter(exporter)
        .with_resource(resource)
        .build()
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
/// When the span closes, the subscriber of [`init`] writes its one `INFO` line on standard
/// output: the span's four fields and its name, plus only the fmt layer's `time.busy` and
/// `time.idle` durations. It is the only span whose closing is written, so a request that emits
/// no event of its own still leaves its method, route template, status and latency on the `info`
/// output.
///
/// `api::app` calls it from tower-http's `TraceLayer::make_span_with` with the request method
/// string and the route template, so the api passes plain strings and this crate needs no HTTP
/// type. The request future runs inside the span through `tracing::Instrument::instrument`, as
/// `TraceLayer` does; never hold `Span::enter` across an `.await`.
#[must_use]
pub fn request_span(method: &str, matched_path: &str) -> Span {
    info_span!(
        REQUEST_SPAN,
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

/// The secret key of the message IDs that telemetry records, under which a [`ScreenedText`]
/// shows a requester's ID as its base64url HMAC-SHA256.
///
/// The digest lets an operator match every log line of one request, and recompute it for an ID
/// they already know, without the log holding the ID: an ID can be a card number, an SSN or a
/// password, and only a holder of the key can test a guess against a digest.
///
/// The caller supplies the secret. `carddemo-mq` keys its workers with its `CARDDEMO_AMQP_URL`,
/// exactly the string it connects with: anyone who holds the broker credentials can already read
/// every request and its properties from the broker, so a digest keyed by them discloses nothing
/// its key holder cannot read directly. The digest is exactly as protected as the messages it
/// identifies, which an unkeyed hash would not be, since a card number can be brute-forced.
/// `Debug` never shows the secret, and no `Display` or `Serialize` exists.
#[derive(Clone)]
pub struct IdKey(EncodingKey);

impl IdKey {
    /// The key of `secret`. Any bytes, an empty secret's included, make an HMAC key, so every
    /// secret makes one.
    #[must_use]
    pub fn new(secret: &str) -> Self {
        Self(EncodingKey::from_secret(secret.as_bytes()))
    }

    /// The base64url HMAC-SHA256 of `text` under this key, without padding: 43 characters of
    /// `A`-`Z`, `a`-`z`, `0`-`9`, `-` and `_`. `None` when signing fails, so that the caller
    /// withholds the text rather than showing it.
    fn digest(&self, text: &str) -> Option<String> {
        jsonwebtoken::crypto::sign(text.as_bytes(), &self.0, Algorithm::HS256).ok()
    }
}

impl fmt::Debug for IdKey {
    /// A fixed placeholder, never a byte of the secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdKey(<redacted>)")
    }
}

/// Text a requester chose, such as an AMQP property, in the only form a span, an event or a
/// `Debug` output may show it: its keyed digest, or one `*` for each of its bytes.
///
/// The requester chooses the text, so it can be a card number, an SSN or a password, and a
/// `CardDemo` password is any one to eight printable bytes, spaces included. Nothing in the text
/// tells such a value from a harmless one, so every non-empty text is withheld whole, never a
/// byte of it shown:
///
/// - made by [`ScreenedText::keyed`] with an [`IdKey`], it shows the base64url HMAC-SHA256 of
///   the text under that key, 43 characters that are the same for the same text on every line,
///   so the log lines of one message can be matched; an operator recomputes the digest of a
///   known ID with the `python3` line of the module documentation. The digest is keyed because
///   an unkeyed hash of a card number could be brute-forced from the log;
/// - made by [`ScreenedText::new`], it shows one `*` per byte: a diagnostic learns that the text
///   is present and how many bytes it has. Should signing ever fail, a keyed text falls back to
///   this form rather than show the text.
///
/// An empty text, which has no byte to hide, shows as empty in either form.
///
/// `Display` writes the shown form and `Debug` writes it quoted, as `str`'s `Debug` writes a
/// string; neither ever formats the text. The value only borrows the text, so the caller keeps
/// the original for the wire.
#[derive(Clone, Copy)]
pub struct ScreenedText<'a> {
    text: &'a str,
    key: Option<&'a IdKey>,
}

impl<'a> ScreenedText<'a> {
    /// Wraps `text`, which shows as one `*` per byte, never as itself.
    #[must_use]
    pub const fn new(text: &'a str) -> Self {
        Self { text, key: None }
    }

    /// Wraps `text`, which shows as its base64url HMAC-SHA256 under `key`, never as itself.
    #[must_use]
    pub const fn keyed(text: &'a str, key: &'a IdKey) -> Self {
        Self {
            text,
            key: Some(key),
        }
    }

    /// Whether the text is withheld, which every non-empty text is: it shows as its digest or
    /// its stars, never as itself.
    #[must_use]
    pub fn is_withheld(&self) -> bool {
        !self.text.is_empty()
    }

    /// The keyed digest of the text, or one `*` for each of its bytes when it was made without a
    /// key or signing fails, and nothing for an empty text.
    fn shown(&self) -> String {
        if self.text.is_empty() {
            return String::new();
        }
        self.key
            .and_then(|key| key.digest(self.text))
            .unwrap_or_else(|| "*".repeat(self.text.len()))
    }
}

impl fmt::Display for ScreenedText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.shown())
    }
}

impl fmt::Debug for ScreenedText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.shown().as_str(), f)
    }
}

/// The span of one broker message, named `mq.message`.
///
/// It records the `queue` the message was taken from and, when the message carries them, its
/// `message_id` and `correlation_id` properties, each as its [`ScreenedText`] displays it. The
/// workers pass both keyed with their [`IdKey`], so the span shows each as its base64url
/// HMAC-SHA256, which matches the span to every other line about the same message and to the
/// reply that copies the `correlation_id`. The requester chooses both and either can be a card
/// number, an SSN or a password, so the span never records their text. A fourth field,
/// `reply_message_id`, stays empty until [`record_reply_message_id`] records the ID this system
/// generated for the reply. Never the body: request and reply bodies hold card numbers and
/// account data. A worker runs its processing future inside the span through
/// `tracing::Instrument::instrument`; never hold `Span::enter` across an `.await`.
#[must_use]
pub fn message_span(
    queue: &str,
    message_id: Option<ScreenedText<'_>>,
    correlation_id: Option<ScreenedText<'_>>,
) -> Span {
    let span = info_span!(
        "mq.message",
        queue,
        message_id = Empty,
        correlation_id = Empty,
        reply_message_id = Empty
    );
    if let Some(message_id) = message_id {
        span.record("message_id", tracing::field::display(message_id));
    }
    if let Some(correlation_id) = correlation_id {
        span.record("correlation_id", tracing::field::display(correlation_id));
    }
    span
}

/// Records `message_id`, verbatim, as the `reply_message_id` of a span made by
/// [`message_span`], so that every later line of the span, and the exported span, ties the
/// request to its reply.
///
/// Only for an ID this system generates, such as the CP00 reply's `message_id` from
/// `messaging::ids`, which is made of a timestamp and a process counter and holds no requester
/// text. Never pass a requester's property here: that reaches a span only as a [`ScreenedText`].
/// On any other span it records nothing.
pub fn record_reply_message_id(span: &Span, message_id: &str) {
    span.record("reply_message_id", message_id);
}

/// The span of one batch job run, named `batch.job`.
///
/// It records the `job` (subcommand) name when it opens, and declares the job's outcome, which
/// [`record_job_outcome`] fills in once the job has run: `return_code` and one field for each
/// [`JobCounter`], named as [`JobCounter::field`] names it. A field nothing records stays empty,
/// and an empty field is neither written to standard output nor exported.
///
/// Never add the content of an input or output record or any protected value. Attach it to the
/// job's future with `tracing::Instrument::instrument`; never hold `Span::enter` across an
/// `.await`.
#[must_use]
pub fn job_span(job: &str) -> Span {
    // A span's fields are declared here, where it is created, and a later record of a name it
    // does not declare is dropped: each counter's name is the literal `JobCounter::field` gives.
    info_span!(
        "batch.job",
        job,
        return_code = Empty,
        records = Empty,
        processed = Empty,
        rejected = Empty,
        failed = Empty,
        transactions = Empty,
        accounts = Empty,
        statements = Empty,
        text_records = Empty,
        html_records = Empty,
        report_lines = Empty,
        summaries_read = Empty,
        summaries_deleted = Empty,
        details_read = Empty,
        details_deleted = Empty,
        checkpoints = Empty
    )
}

/// A counter of a batch job's outcome, recorded on its [`job_span`] under the field name
/// [`field`](Self::field) gives.
///
/// The set is closed because a span declares its fields where it is created: every variant has
/// a field of its name on every job span, so a new counter needs a variant here and a field in
/// [`job_span`]. Each counter is a plain number of records, lines, rows or commits, never
/// record content (Cardholder Data Redaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCounter {
    /// `records`: the records a job read, decoded or loaded.
    Records,
    /// `processed`: the daily transactions `posttran` processed.
    Processed,
    /// `rejected`: the daily transactions `posttran` rejected.
    Rejected,
    /// `failed`: the `mnttrdb2` records that reached `9999-ABEND`.
    Failed,
    /// `transactions`: the SYSTRAN records `intcalc` wrote.
    Transactions,
    /// `accounts`: the accounts `intcalc` rewrote.
    Accounts,
    /// `statements`: the statements `creastmt` wrote.
    Statements,
    /// `text_records`: the `STATEMNT.PS` records `creastmt` wrote.
    TextRecords,
    /// `html_records`: the `STATEMNT.HTML` records `creastmt` wrote.
    HtmlRecords,
    /// `report_lines`: the report records `tranrept` wrote.
    ReportLines,
    /// `summaries_read`: the pending authorization summaries `cbpaup0j` read.
    SummariesRead,
    /// `summaries_deleted`: the pending authorization summaries `cbpaup0j` deleted.
    SummariesDeleted,
    /// `details_read`: the pending authorization details `cbpaup0j` read.
    DetailsRead,
    /// `details_deleted`: the pending authorization details `cbpaup0j` deleted.
    DetailsDeleted,
    /// `checkpoints`: the checkpoint commits `cbpaup0j` took.
    Checkpoints,
}

impl JobCounter {
    /// Every counter, in the order [`job_span`] declares their fields.
    pub const ALL: [Self; 15] = [
        Self::Records,
        Self::Processed,
        Self::Rejected,
        Self::Failed,
        Self::Transactions,
        Self::Accounts,
        Self::Statements,
        Self::TextRecords,
        Self::HtmlRecords,
        Self::ReportLines,
        Self::SummariesRead,
        Self::SummariesDeleted,
        Self::DetailsRead,
        Self::DetailsDeleted,
        Self::Checkpoints,
    ];

    /// The name of the counter's field on the [`job_span`], as the span's JSON object and its
    /// exported attributes show it.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Records => "records",
            Self::Processed => "processed",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::Transactions => "transactions",
            Self::Accounts => "accounts",
            Self::Statements => "statements",
            Self::TextRecords => "text_records",
            Self::HtmlRecords => "html_records",
            Self::ReportLines => "report_lines",
            Self::SummariesRead => "summaries_read",
            Self::SummariesDeleted => "summaries_deleted",
            Self::DetailsRead => "details_read",
            Self::DetailsDeleted => "details_deleted",
            Self::Checkpoints => "checkpoints",
        }
    }
}

/// Records a job's outcome on its [`job_span`]: `return_code`, and each count of `counts` under
/// its [`JobCounter::field`].
///
/// The values are plain numbers, never record content or a protected value (Cardholder Data
/// Redaction). They are span fields, and not only fields of an event, because the
/// `OpenTelemetry` SDK keeps the first 128 events of a span and drops the rest: the event that
/// ends a job emitting more, as `posttran` does at `TRACE`, is not exported, while the exported
/// `batch.job` span carries its attributes however many events the job emitted. On standard
/// output, each event emitted inside the span after this call shows the outcome in its `span`
/// and `spans` members.
///
/// Each value is recorded as an `i64`, a JSON number on standard output and an integer
/// attribute of the exported span; `tracing-opentelemetry` would export a `u64` as its text. A
/// count above `i64::MAX`, which no job reaches, keeps its exact value as a `u64`, which the
/// exporter shows as text. A disabled span, and a name the span does not declare, record nothing.
pub fn record_job_outcome(span: &Span, return_code: u8, counts: &[(JobCounter, u64)]) {
    span.record("return_code", i64::from(return_code));
    for &(counter, count) in counts {
        // TRUNCATION: checked, so nothing is lost: a count above `i64::MAX` fails the conversion
        // and is recorded as its exact `u64` value instead, and no job counts 2^63 records.
        if let Ok(signed) = i64::try_from(count) {
            span.record(counter.field(), signed);
        } else {
            span.record(counter.field(), count);
        }
    }
}

/// Whether an event of `callsite` is to be dispatched now, decided as `tracing`'s event macros
/// decide it: the event's level is within the static and the current maximum level, the
/// callsite's interest, which its first use registers, is not `never`, and the interest is
/// `always` or the current dispatcher enables the event's metadata.
///
/// It serves code that writes an event's callsite out by hand, a `DefaultCallsite` static over a
/// `tracing::metadata!` static, and dispatches the event with `tracing::Event::dispatch` only when
/// this returns `true`. Such code exists because the expansion of `tracing::info!` and the other
/// event macros carries a lint suppression, an `allow(unused_imports)` attribute, and no source of
/// this workspace, hand-written or generated, holds one.
#[must_use]
pub fn event_enabled(callsite: &'static DefaultCallsite) -> bool {
    let metadata = callsite.metadata();
    let level = *metadata.level();
    if level > STATIC_MAX_LEVEL || level > LevelFilter::current() {
        return false;
    }
    let interest = callsite.interest();
    !interest.is_never()
        && (interest.is_always()
            || tracing::dispatcher::get_default(|dispatch| dispatch.enabled(metadata)))
}

#[cfg(test)]
mod tests {
    use std::env::VarError;
    use std::error::Error as _;
    use std::ffi::OsString;
    use std::io;
    use std::os::unix::ffi::OsStringExt;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use opentelemetry::Key;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
    use opentelemetry_sdk::trace::TracerProvider;
    use tracing::callsite::{Callsite as _, DefaultCallsite};
    use tracing::field::{Field, Value};
    use tracing::metadata::Kind;
    use tracing::{Event, Level, Metadata};
    use tracing_subscriber::filter::{EnvFilter, FromEnvError, LevelFilter};
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{
        ALERT_TARGET, IdKey, InitError, JobCounter, OtelExport, SERVICE_NAME_KEY, SILENCED_TARGETS,
        ScreenedText, UNMATCHED_ROUTE, dependency_filter, env_filter, event_enabled, init,
        is_request_span, job_span, json_layer, json_subscriber, message_span, otel_layer,
        record_job_outcome, record_reply_message_id, record_response, request_span, service_name,
        service_resource, tracer_provider,
    };

    /// The first card number of `app/data/ASCII/cardxref.txt`.
    const CARD_NUMBER: &str = "0500024453765740";

    /// The ID key secret of the keyed cases, and a second one that must give other digests.
    const ID_KEY: &str = "local-test-id-key";
    const OTHER_ID_KEY: &str = "local-test-id-key-2";

    /// A CP00 reply `message_id` as `messaging::ids` generates it: a timestamp to the millisecond,
    /// the process ID and a counter.
    const REPLY_MESSAGE_ID: &str = "20220718193005250-4242-0";

    /// Whether `shown` has the form of a keyed digest: 43 base64url characters, no padding.
    fn is_digest(shown: &str) -> bool {
        shown.len() == 43
            && shown
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }

    /// Property values a requester may choose, none of which a diagnostic may show: a card
    /// number, whole and spaced out, an SSN with and without dashes, an 8-byte password, a
    /// password holding a space, a short password and a short ID.
    const PROBES: [&str; 8] = [
        CARD_NUMBER,
        "123-45-6789",
        "123456789",
        "PASSWORD",
        "PASS WOR",
        "SECRET",
        "M-1",
        "0500 0244 5376 5740",
    ];

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

    // Written out: `tracing::info!` and the other macros expand to an `allow(unused_imports)` lint.
    /// The callsite of the `INFO` probe event, whose one field is its `message`.
    static PROBE: DefaultCallsite = DefaultCallsite::new(&PROBE_METADATA);

    /// The metadata of [`PROBE`].
    static PROBE_METADATA: Metadata<'static> = tracing::metadata! {
        name: "probe",
        target: module_path!(),
        level: Level::INFO,
        fields: &["message"],
        callsite: &PROBE,
        kind: Kind::EVENT,
    };

    /// The callsite of the `WARN` probe event, whose one field is its `message`.
    static WARN_PROBE: DefaultCallsite = DefaultCallsite::new(&WARN_PROBE_METADATA);

    /// The metadata of [`WARN_PROBE`].
    static WARN_PROBE_METADATA: Metadata<'static> = tracing::metadata! {
        name: "warn probe",
        target: module_path!(),
        level: Level::WARN,
        fields: &["message"],
        callsite: &WARN_PROBE,
        kind: Kind::EVENT,
    };

    /// Emits an event of the probe `callsite` in the current span, with `message` as its message,
    /// when [`event_enabled`] says so, and returns whether it did.
    fn emit(callsite: &'static DefaultCallsite, message: &str) -> bool {
        if !event_enabled(callsite) {
            return false;
        }
        let metadata = callsite.metadata();
        let fields = metadata.fields();
        let field = fields
            .field("message")
            .expect("every probe declares `message`");
        let values: [(&Field, Option<&dyn Value>); 1] = [(&field, Some(&message))];
        Event::dispatch(metadata, &fields.value_set(&values));
        true
    }

    /// Emits the `INFO` probe event with `message` in the current span.
    fn probe(message: &str) {
        emit(&PROBE, message);
    }

    /// The callsite of a span of the request span's name and fields made in another module, this
    /// test module.
    static OTHER_MODULE_REQUEST: DefaultCallsite =
        DefaultCallsite::new(&OTHER_MODULE_REQUEST_METADATA);

    /// The metadata of [`OTHER_MODULE_REQUEST`].
    static OTHER_MODULE_REQUEST_METADATA: Metadata<'static> = tracing::metadata! {
        name: "http.request",
        target: module_path!(),
        level: Level::INFO,
        fields: &["method", "route", "status", "latency_ms"],
        callsite: &OTHER_MODULE_REQUEST,
        kind: Kind::SPAN,
    };

    /// The callsite of an event of the request span's name and target.
    static REQUEST_EVENT: DefaultCallsite = DefaultCallsite::new(&REQUEST_EVENT_METADATA);

    /// The metadata of [`REQUEST_EVENT`].
    static REQUEST_EVENT_METADATA: Metadata<'static> = tracing::metadata! {
        name: "http.request",
        target: "runtime::telemetry",
        level: Level::INFO,
        fields: &["message"],
        callsite: &REQUEST_EVENT,
        kind: Kind::EVENT,
    };

    /// In-memory span exporter standing in for `opentelemetry-stdout`: it keeps every span it is
    /// handed, as the stdout exporter would print it, and the last resource the provider hands
    /// it, which the stdout exporter prints as its `Resource` block.
    #[derive(Clone, Debug, Default)]
    struct Exported(Arc<Mutex<Vec<SpanData>>>, Arc<Mutex<Option<Resource>>>);

    impl Exported {
        fn spans(&self) -> Vec<SpanData> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn resource(&self) -> Option<Resource> {
            self.1
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

        fn set_resource(&mut self, resource: &Resource) {
            *self.1.lock().unwrap_or_else(PoisonError::into_inner) = Some(resource.clone());
        }
    }

    /// The value `resource` holds under `key`, as text.
    fn resource_attribute(resource: &Resource, key: &'static str) -> Option<String> {
        resource
            .get(Key::from_static_str(key))
            .map(|value| value.as_str().into_owned())
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
    fn a_closed_request_span_writes_one_info_line_when_rust_log_enables_it() {
        // `RUST_LOG` value, and whether it enables `INFO` for this module's spans.
        let directives = [
            ("info", true),
            ("warn,runtime::telemetry=info", true),
            ("warn", false),
            ("info,runtime::telemetry=warn", false),
        ];
        // Method, route, status and latency in milliseconds of each request.
        let requests = [
            ("GET", "/api/v1/accounts/{acct_id}/view", 401, 12),
            ("GET", "/api/v1/menus/main", 200, 1),
            ("POST", UNMATCHED_ROUTE, 404, 0),
        ];
        for (directive, enabled) in directives {
            for (method, route, status, latency) in requests {
                let captured = Captured::default();
                let subscriber = json_subscriber(EnvFilter::new(directive), captured.clone());
                tracing::subscriber::with_default(subscriber, || {
                    let span = request_span(method, route);
                    record_response(&span, status, Duration::from_millis(latency));
                    drop(span);
                });
                let output = captured.text();
                let case = format!("{directive}: {method} {route}");

                if !enabled {
                    assert_eq!(output, "", "{case}");
                    continue;
                }
                let lines: Vec<&str> = output.lines().collect();
                let [line] = lines.as_slice() else {
                    panic!("{case}: expected exactly one line, got {output}");
                };
                // The span's recorded fields, in the order the JSON format writes them, then its
                // name.
                let fields =
                    format!(r#""latency_ms":{latency},"method":"{method}","route":"{route}""#);
                let span = format!(r#"{{{fields},"status":{status},"name":"http.request"}}"#);
                assert_contains_all(
                    line,
                    &[
                        r#"{"timestamp":""#,
                        r#""level":"INFO","fields":{"message":"close","time.busy":""#,
                        r#"","time.idle":""#,
                        r#""target":"runtime::telemetry""#,
                        // No span is entered where the span closes.
                        &format!(r#""span":{span},"spans":[]}}"#),
                    ],
                );
                // The fields are the message and the two durations, and nothing else.
                let Some((_, fields)) = line.split_once(r#""fields":{"#) else {
                    panic!("{case}: no fields member in {line}");
                };
                let fields = fields.split_once('}').map_or(fields, |(object, _)| object);
                assert_eq!(fields.matches("\":").count(), 3, "{case}: {fields}");
                for withheld in ["uri", "?", "header", "body", "user"] {
                    assert!(!line.contains(withheld), "{case}: {withheld} in {line}");
                }
            }
        }
    }

    #[test]
    fn no_other_span_writes_its_opening_or_closing() {
        for directive in ["info", "trace"] {
            let captured = Captured::default();
            let subscriber = json_subscriber(EnvFilter::new(directive), captured.clone());
            tracing::subscriber::with_default(subscriber, || {
                job_span("posttran").in_scope(|| probe("job probe"));
                message_span("CARD.DEMO.REQUEST.DATE", None, None)
                    .in_scope(|| probe("message probe"));
            });
            let output = captured.text();

            // Only the two events: no line of either span's opening or closing.
            let messages: Vec<&str> = output
                .lines()
                .filter_map(|line| line.split_once(r#""message":""#))
                .map(|(_, rest)| rest.split_once('"').map_or(rest, |(message, _)| message))
                .collect();
            assert_eq!(
                messages,
                ["job probe", "message probe"],
                "{directive}: {output}"
            );
            assert_contains_all(
                &output,
                &[
                    r#""name":"batch.job""#,
                    r#""name":"mq.message""#,
                    r#""target":"runtime::telemetry::tests""#,
                ],
            );
        }
    }

    /// The request close layer's filter passes the request span of this module alone: not a span
    /// of that name made in another module, not this module's other spans, not an event.
    #[test]
    fn only_the_request_span_of_this_module_is_a_request_span() {
        // A subscriber that enables every span, so that each span has its metadata.
        tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            let request = request_span("GET", "/api/v1/menus/main");
            let job = job_span("posttran");
            let message = message_span("CARD.DEMO.REQUEST.DATE", None, None);
            let cases = [
                ("the request span", request.metadata(), true),
                (
                    "a span of its name and fields in another module",
                    Some(&OTHER_MODULE_REQUEST_METADATA),
                    false,
                ),
                ("the job span", job.metadata(), false),
                ("the message span", message.metadata(), false),
                (
                    "an event of its name and target",
                    Some(&REQUEST_EVENT_METADATA),
                    false,
                ),
            ];
            for (case, metadata, expected) in cases {
                let Some(metadata) = metadata else {
                    panic!("{case} is disabled under a subscriber that enables every span");
                };
                assert_eq!(is_request_span(metadata), expected, "{case}: {metadata:?}");
            }
            // The two written-out cases differ from the request span in target or kind alone.
            let Some(expected) = request.metadata() else {
                panic!("the request span is disabled under a subscriber that enables every span");
            };
            let other_module = &OTHER_MODULE_REQUEST_METADATA;
            assert_eq!(
                (other_module.name(), other_module.is_span()),
                (expected.name(), true)
            );
            assert_ne!(other_module.target(), expected.target());
            let event = &REQUEST_EVENT_METADATA;
            assert_eq!(
                (event.name(), event.target(), event.is_event()),
                (expected.name(), expected.target(), true)
            );
        });
    }

    #[test]
    fn screened_text_withholds_every_byte_of_a_non_empty_text() {
        for text in PROBES {
            let screened = ScreenedText::new(text);
            let stars = "*".repeat(text.len());
            assert!(screened.is_withheld(), "{text:?}");
            assert_eq!(screened.to_string(), stars, "{text:?}");
            assert_eq!(format!("{screened:?}"), format!("\"{stars}\""), "{text:?}");
        }
        let empty = ScreenedText::new("");
        assert!(!empty.is_withheld());
        assert_eq!(empty.to_string(), "");
        assert_eq!(format!("{empty:?}"), r#""""#);
    }

    #[test]
    fn message_span_records_queue_and_present_ids_only() {
        for (message_id, correlation_id) in PROBES.into_iter().zip(PROBES.into_iter().rev()) {
            let with_ids = capture(|| {
                message_span(
                    "CARD.DEMO.REQUEST.PAUTH",
                    Some(ScreenedText::new(message_id)),
                    Some(ScreenedText::new(correlation_id)),
                )
                .in_scope(|| tracing::info!("probe"));
            });
            let message_stars = format!(r#""message_id":"{}""#, "*".repeat(message_id.len()));
            let correlation_stars =
                format!(r#""correlation_id":"{}""#, "*".repeat(correlation_id.len()));
            assert_contains_all(
                &with_ids,
                &[
                    r#""name":"mq.message""#,
                    r#""queue":"CARD.DEMO.REQUEST.PAUTH""#,
                    &message_stars,
                    &correlation_stars,
                ],
            );
            for probe in PROBES {
                assert!(!with_ids.contains(probe), "{probe:?} in {with_ids}");
            }
        }

        let without_ids = capture(|| {
            message_span("CARD.DEMO.REQUEST.DATE", None, None).in_scope(|| tracing::info!("probe"));
        });
        assert_contains_all(&without_ids, &[r#""queue":"CARD.DEMO.REQUEST.DATE""#]);
        assert!(!without_ids.contains("message_id"), "{without_ids}");
        assert!(!without_ids.contains("correlation_id"), "{without_ids}");
    }

    #[test]
    fn the_keyed_digest_is_the_base64url_hmac_sha256_of_rfc_4231_test_case_2() {
        let key = IdKey::new("Jefe");
        let screened = ScreenedText::keyed("what do ya want for nothing?", &key);
        let expected = "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM";
        assert_eq!(screened.to_string(), expected);
        assert_eq!(format!("{screened:?}"), format!("\"{expected}\""));
        assert!(screened.is_withheld());
    }

    #[test]
    fn keyed_screened_text_shows_a_stable_digest_that_hides_every_probe() {
        let key = IdKey::new(ID_KEY);
        let other = IdKey::new(OTHER_ID_KEY);
        let mut digests = Vec::with_capacity(PROBES.len());
        for text in PROBES {
            let screened = ScreenedText::keyed(text, &key);
            let shown = screened.to_string();
            assert!(is_digest(&shown), "{text:?}: {shown}");
            assert!(!shown.contains(text), "{text:?} in {shown}");
            assert!(screened.is_withheld(), "{text:?}");
            assert_eq!(format!("{screened:?}"), format!("\"{shown}\""), "{text:?}");
            assert_eq!(
                ScreenedText::keyed(text, &key).to_string(),
                shown,
                "{text:?}: the same key gives the same digest"
            );
            assert_ne!(
                ScreenedText::keyed(text, &other).to_string(),
                shown,
                "{text:?}: another key gives another digest"
            );
            digests.push(shown);
        }
        digests.sort_unstable();
        digests.dedup();
        assert_eq!(digests.len(), PROBES.len(), "two probes share a digest");

        let empty = ScreenedText::keyed("", &key);
        assert!(!empty.is_withheld());
        assert_eq!(empty.to_string(), "");
    }

    #[test]
    fn an_empty_secret_still_hides_ids_and_a_key_never_shows_its_secret() {
        let empty_secret = IdKey::new("");
        for text in PROBES {
            let shown = ScreenedText::keyed(text, &empty_secret).to_string();
            assert!(is_digest(&shown), "{text:?}: {shown}");
            assert!(!shown.contains(text), "{text:?} in {shown}");
        }

        let secret = "s3cret-id-key";
        let key = IdKey::new(secret);
        let copy = key.clone();
        assert_eq!(
            ScreenedText::keyed(CARD_NUMBER, &copy).to_string(),
            ScreenedText::keyed(CARD_NUMBER, &key).to_string(),
            "a copied key gives the same digest"
        );
        for shown in [format!("{key:?}"), format!("{copy:?}"), format!("{key:#?}")] {
            assert_eq!(shown, "IdKey(<redacted>)");
            assert!(!shown.contains(secret), "{shown}");
        }
    }

    #[test]
    fn message_span_records_keyed_digests_and_the_reply_message_id_verbatim() {
        let key = IdKey::new(ID_KEY);
        for (message_id, correlation_id) in PROBES.into_iter().zip(PROBES.into_iter().rev()) {
            let output = capture(|| {
                let span = message_span(
                    "CARD.DEMO.REQUEST.PAUTH",
                    Some(ScreenedText::keyed(message_id, &key)),
                    Some(ScreenedText::keyed(correlation_id, &key)),
                );
                span.in_scope(|| tracing::info!("before the reply"));
                record_reply_message_id(&span, REPLY_MESSAGE_ID);
                span.in_scope(|| tracing::info!("after the reply"));
            });
            let lines: Vec<&str> = output.lines().collect();
            let [before, after] = lines.as_slice() else {
                panic!("expected two event lines, got {output}");
            };
            let message_digest = format!(
                r#""message_id":"{}""#,
                ScreenedText::keyed(message_id, &key)
            );
            let correlation_digest = format!(
                r#""correlation_id":"{}""#,
                ScreenedText::keyed(correlation_id, &key)
            );
            for line in [before, after] {
                assert_contains_all(
                    line,
                    &[
                        r#""name":"mq.message""#,
                        r#""queue":"CARD.DEMO.REQUEST.PAUTH""#,
                        &message_digest,
                        &correlation_digest,
                    ],
                );
            }
            assert!(!before.contains("reply_message_id"), "{before}");
            assert_contains_all(
                after,
                &[&format!(r#""reply_message_id":"{REPLY_MESSAGE_ID}""#)],
            );
            for probe in PROBES {
                assert!(!output.contains(probe), "{probe:?} in {output}");
            }
        }
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
    fn no_rust_log_directive_enables_a_silenced_dependency_target() {
        // `RUST_LOG` value, and whether it enables application `DEBUG` and `TRACE` events.
        let cases = [
            ("trace", true),
            (
                "lapin=trace,amq_protocol=trace,pinky_swear=trace,info",
                false,
            ),
            ("polling=trace,async_io=trace,info", false),
            (
                "polling::epoll=trace,async_io::reactor=trace,async_io::driver=trace,trace",
                true,
            ),
            ("info", false),
        ];
        for (directives, below_info) in cases {
            let captured = Captured::default();
            let subscriber = json_subscriber(EnvFilter::new(directives), captured.clone());
            tracing::subscriber::with_default(subscriber, || {
                tracing::error!(target: "lapin::channels", "dependency probe");
                tracing::trace!(target: "lapin::channels", "dependency probe");
                tracing::error!(target: "amq_protocol_tcp", "dependency probe");
                tracing::trace!(target: "amq_protocol_tcp", "dependency probe");
                tracing::error!(target: "pinky_swear", "dependency probe");
                tracing::trace!(target: "pinky_swear", "dependency probe");
                tracing::info!(target: "messaging::workers", "application info probe");
                tracing::debug!(target: "messaging::workers", "application debug probe");
                tracing::trace!(target: "messaging::workers", "application trace probe");
                tracing::error!(target: ALERT_TARGET, alert = "CP00_CARD_UNPARSEABLE", "alert");
            });
            let output = captured.text();

            assert!(
                !output.contains("dependency probe"),
                "{directives}: {output}"
            );
            for target in SILENCED_TARGETS {
                let member = format!(r#""target":"{target}"#);
                assert!(!output.contains(&member), "{directives}: {output}");
            }
            assert_contains_all(
                &output,
                &[
                    "application info probe",
                    r#""target":"carddemo::alert""#,
                    r#""alert":"CP00_CARD_UNPARSEABLE""#,
                ],
            );
            for probe in ["application debug probe", "application trace probe"] {
                assert_eq!(
                    output.contains(probe),
                    below_info,
                    "{directives}: {probe} in {output}"
                );
            }
        }
    }

    /// The dependency filter refuses every silenced target, and each module under one such as
    /// the reactor's `polling::epoll`, at every level, and passes an application target at every
    /// level, so only `RUST_LOG` decides the latter.
    #[test]
    fn the_dependency_filter_refuses_every_silenced_target_at_every_level() {
        let filter = dependency_filter();
        let levels = [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ];
        let modules = [
            "polling::epoll",
            "async_io::reactor",
            "async_io::driver",
            "lapin::channels",
            "amq_protocol_tcp",
        ];
        for target in SILENCED_TARGETS.into_iter().chain(modules) {
            for level in levels {
                assert!(!filter.would_enable(target, &level), "{target} at {level}");
            }
        }
        for target in ["messaging::workers", ALERT_TARGET] {
            for level in levels {
                assert!(filter.would_enable(target, &level), "{target} at {level}");
            }
        }
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
    fn exported_message_spans_carry_the_ids_as_stars_only() {
        let exported = Exported::default();
        let provider = TracerProvider::builder()
            .with_simple_exporter(exported.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(otel_layer(&provider));
        let cases: Vec<(&str, &str)> = PROBES.into_iter().zip(PROBES.into_iter().rev()).collect();
        tracing::subscriber::with_default(subscriber, || {
            for &(message_id, correlation_id) in &cases {
                message_span(
                    "CARD.DEMO.REQUEST.PAUTH",
                    Some(ScreenedText::new(message_id)),
                    Some(ScreenedText::new(correlation_id)),
                )
                .in_scope(|| tracing::info!("probe"));
            }
        });

        let spans = exported.spans();
        assert_eq!(spans.len(), cases.len(), "{spans:?}");
        for (span, &(message_id, correlation_id)) in spans.iter().zip(&cases) {
            assert_eq!(span.name, "mq.message");
            let attribute = |key: &str| {
                span.attributes
                    .iter()
                    .find(|kv| kv.key.as_str() == key)
                    .map(|kv| kv.value.as_str().into_owned())
            };
            assert_eq!(
                attribute("queue").as_deref(),
                Some("CARD.DEMO.REQUEST.PAUTH"),
                "{span:?}"
            );
            assert_eq!(
                attribute("message_id"),
                Some("*".repeat(message_id.len())),
                "{span:?}"
            );
            assert_eq!(
                attribute("correlation_id"),
                Some("*".repeat(correlation_id.len())),
                "{span:?}"
            );
            let values: Vec<String> = span
                .attributes
                .iter()
                .chain(span.events.iter().flat_map(|event| &event.attributes))
                .map(|kv| kv.value.as_str().into_owned())
                .collect();
            for probe in PROBES {
                assert!(
                    values.iter().all(|value| !value.contains(probe)),
                    "{probe:?} in {span:?}"
                );
            }
        }
    }

    #[test]
    fn exported_message_spans_carry_keyed_digests_and_the_reply_message_id_verbatim() {
        let key = IdKey::new(ID_KEY);
        let exported = Exported::default();
        let provider = TracerProvider::builder()
            .with_simple_exporter(exported.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(otel_layer(&provider));
        let cases: Vec<(&str, &str)> = PROBES.into_iter().zip(PROBES.into_iter().rev()).collect();
        tracing::subscriber::with_default(subscriber, || {
            for &(message_id, correlation_id) in &cases {
                let span = message_span(
                    "CARD.DEMO.REQUEST.PAUTH",
                    Some(ScreenedText::keyed(message_id, &key)),
                    Some(ScreenedText::keyed(correlation_id, &key)),
                );
                record_reply_message_id(&span, REPLY_MESSAGE_ID);
                span.in_scope(|| tracing::info!("probe"));
            }
        });

        let spans = exported.spans();
        assert_eq!(spans.len(), cases.len(), "{spans:?}");
        for (span, &(message_id, correlation_id)) in spans.iter().zip(&cases) {
            assert_eq!(span.name, "mq.message");
            let attribute = |name: &str| {
                span.attributes
                    .iter()
                    .find(|kv| kv.key.as_str() == name)
                    .map(|kv| kv.value.as_str().into_owned())
            };
            assert_eq!(
                attribute("message_id"),
                Some(ScreenedText::keyed(message_id, &key).to_string()),
                "{span:?}"
            );
            assert_eq!(
                attribute("correlation_id"),
                Some(ScreenedText::keyed(correlation_id, &key).to_string()),
                "{span:?}"
            );
            assert_eq!(
                attribute("reply_message_id").as_deref(),
                Some(REPLY_MESSAGE_ID),
                "{span:?}"
            );
            let values: Vec<String> = span
                .attributes
                .iter()
                .chain(span.events.iter().flat_map(|event| &event.attributes))
                .map(|kv| kv.value.as_str().into_owned())
                .collect();
            for probe in PROBES {
                assert!(
                    values.iter().all(|value| !value.contains(probe)),
                    "{probe:?} in {span:?}"
                );
            }
        }
    }

    #[test]
    fn service_name_is_the_file_name_stem_of_the_executable() {
        let not_utf8 = PathBuf::from(OsString::from_vec(
            b"/x/target/debug/carddemo-\xff".to_vec(),
        ));
        let cases = [
            (
                Path::new("/x/target/debug/carddemo-mq"),
                Some("carddemo-mq"),
            ),
            (
                Path::new("/x/target/debug/carddemo-batch"),
                Some("carddemo-batch"),
            ),
            (
                Path::new("/x/target/debug/carddemo-migrate"),
                Some("carddemo-migrate"),
            ),
            (
                Path::new("/x/target/debug/carddemo-api.exe"),
                Some("carddemo-api"),
            ),
            (Path::new("carddemo-api"), Some("carddemo-api")),
            (Path::new(""), None),
            (Path::new("/"), None),
            (Path::new("/x/target/debug/.."), None),
            (not_utf8.as_path(), None),
        ];
        for (executable, expected) in cases {
            assert_eq!(service_name(executable), expected, "{executable:?}");
        }
    }

    #[test]
    fn exported_spans_carry_the_service_name_of_the_executable() {
        // Executable path, and the `service.name` the exporter must be handed for it.
        let cases = [
            (
                Some(Path::new("/x/target/debug/carddemo-mq")),
                "carddemo-mq",
            ),
            (
                Some(Path::new("/x/target/debug/carddemo-batch")),
                "carddemo-batch",
            ),
            (
                Some(Path::new("/x/target/debug/carddemo-migrate")),
                "carddemo-migrate",
            ),
            (Some(Path::new("/")), "unknown_service"),
            (None, "unknown_service"),
        ];
        for (executable, expected) in cases {
            let exported = Exported::default();
            let provider = tracer_provider(exported.clone(), service_resource(executable));
            let subscriber = tracing_subscriber::registry().with(otel_layer(&provider));
            tracing::subscriber::with_default(subscriber, || {
                job_span("acctfile").in_scope(|| {});
            });

            assert_eq!(exported.spans().len(), 1, "{executable:?}");
            let Some(resource) = exported.resource() else {
                panic!("{executable:?}: the provider handed the exporter no resource");
            };
            assert_eq!(
                resource_attribute(&resource, SERVICE_NAME_KEY).as_deref(),
                Some(expected),
                "{executable:?}: {resource:?}"
            );
            assert_eq!(
                resource_attribute(&resource, "telemetry.sdk.language").as_deref(),
                Some("rust"),
                "{executable:?}: {resource:?}"
            );
            let directories: Vec<String> = resource
                .iter()
                .map(|(_, value)| value.as_str().into_owned())
                .filter(|value| value.contains('/'))
                .collect();
            assert!(directories.is_empty(), "{executable:?}: {resource:?}");
        }
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

    /// The outcome the job span tests record: `posttran`'s counts over the fixtures, and a count
    /// beyond `i64::MAX`.
    const OUTCOME: [(JobCounter, u64); 3] = [
        (JobCounter::Processed, 300),
        (JobCounter::Rejected, 38),
        (JobCounter::Records, u64::MAX),
    ];

    /// The counters [`OUTCOME`] leaves unrecorded.
    fn unrecorded() -> impl Iterator<Item = JobCounter> {
        JobCounter::ALL
            .into_iter()
            .filter(|counter| OUTCOME.iter().all(|(recorded, _)| recorded != counter))
    }

    #[test]
    fn job_span_declares_the_return_code_and_every_counter_field() {
        let fields = JobCounter::ALL.map(JobCounter::field);
        let distinct: std::collections::BTreeSet<&str> = fields.into_iter().collect();
        assert_eq!(distinct.len(), fields.len(), "{fields:?}");

        // A subscriber that enables every span, so that the span has its metadata.
        tracing::subscriber::with_default(tracing_subscriber::registry(), || {
            let span = job_span("posttran");
            for name in ["job", "return_code"].into_iter().chain(fields) {
                assert!(span.field(name).is_some(), "batch.job lacks {name}");
            }
            let Some(metadata) = span.metadata() else {
                panic!("the job span is disabled under a subscriber that enables every span");
            };
            let declared: Vec<&str> = metadata.fields().iter().map(|field| field.name()).collect();
            let mut expected = vec!["job", "return_code"];
            expected.extend(fields);
            assert_eq!(declared, expected);
        });
    }

    #[test]
    fn a_recorded_job_outcome_is_in_the_span_of_later_events_as_numbers() {
        let output = capture(|| {
            let span = job_span("posttran");
            record_job_outcome(&span, 4, &OUTCOME);
            span.in_scope(|| probe("job finished"));
        });
        let Some((_, after)) = output.split_once(r#""span":{"#) else {
            panic!("no span member in {output}");
        };
        let span = after.split_once('}').map_or(after, |(object, _)| object);
        assert_contains_all(
            span,
            &[
                r#""job":"posttran""#,
                r#""return_code":4"#,
                r#""processed":300"#,
                r#""rejected":38"#,
                r#""records":18446744073709551615"#,
                r#""name":"batch.job""#,
            ],
        );
        for counter in unrecorded() {
            let member = format!(r#""{}":"#, counter.field());
            assert!(!output.contains(&member), "{member} in {output}");
        }
    }

    #[test]
    fn the_exported_job_span_carries_its_outcome_however_many_events_are_dropped() {
        let exported = Exported::default();
        // The construction `init` uses, with the SDK's default span limits.
        let provider = tracer_provider(exported.clone(), Resource::default());
        let subscriber = tracing_subscriber::registry().with(otel_layer(&provider));
        tracing::subscriber::with_default(subscriber, || {
            let span = job_span("posttran");
            span.in_scope(|| {
                for _ in 0..200 {
                    probe("probe");
                }
            });
            record_job_outcome(&span, 4, &OUTCOME);
            span.in_scope(|| probe("job finished"));
        });

        let spans = exported.spans();
        let [span] = spans.as_slice() else {
            panic!("expected exactly one exported span, got {}", spans.len());
        };
        assert_eq!(span.name, "batch.job");
        // The SDK keeps the first 128 of the 201 events: the closing event is among those dropped.
        assert_eq!(
            (span.events.len(), span.events.dropped_count),
            (128, 73),
            "{:?}",
            span.attributes
        );
        assert!(
            span.events.iter().all(|event| event.name != "job finished"),
            "{:?}",
            span.attributes
        );
        let attribute = |key: &str| {
            span.attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.clone())
        };
        let expected = [
            ("job", opentelemetry::Value::from("posttran")),
            ("return_code", opentelemetry::Value::I64(4)),
            ("processed", opentelemetry::Value::I64(300)),
            ("rejected", opentelemetry::Value::I64(38)),
            ("records", opentelemetry::Value::from(u64::MAX.to_string())),
        ];
        for (key, value) in expected {
            assert_eq!(attribute(key), Some(value), "{key}: {:?}", span.attributes);
        }
        for counter in unrecorded() {
            assert_eq!(attribute(counter.field()), None, "{:?}", span.attributes);
        }
    }

    /// A written-out event is dispatched exactly when the subscriber's filters enable its level
    /// and target, and the line it writes is the one a macro-made event writes.
    #[test]
    fn event_enabled_follows_the_filters_of_the_current_subscriber() {
        // `RUST_LOG` value, and whether it enables the `INFO` and the `WARN` probe.
        let cases = [
            ("trace", true, true),
            ("info", true, true),
            ("warn", false, true),
            ("info,runtime::telemetry::tests=error", false, false),
            ("off", false, false),
        ];
        for (directive, info, warn) in cases {
            let captured = Captured::default();
            let subscriber = json_subscriber(EnvFilter::new(directive), captured.clone());
            let emitted = tracing::subscriber::with_default(subscriber, || {
                (emit(&PROBE, "info probe"), emit(&WARN_PROBE, "warn probe"))
            });
            assert_eq!(emitted, (info, warn), "{directive}");

            let output = captured.text();
            let lines = [
                (info, r#""level":"INFO","fields":{"message":"info probe"}"#),
                (warn, r#""level":"WARN","fields":{"message":"warn probe"}"#),
            ];
            for (expected, needle) in lines {
                assert_eq!(output.contains(needle), expected, "{directive}: {output}");
            }
            let count = usize::from(info) + usize::from(warn);
            assert_eq!(output.lines().count(), count, "{directive}: {output}");
            if count > 0 {
                assert_contains_all(&output, &[r#""target":"runtime::telemetry::tests""#]);
            }
        }

        // A subscriber that enables nothing.
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            assert!(!event_enabled(&PROBE));
            assert!(!event_enabled(&WARN_PROBE));
        });
    }
}
