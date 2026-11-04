//! JSON-lines span exporter for the opentelemetry SDK.
//!
//! Spans recorded through `tracing` reach this exporter by way of
//! `tracing-opentelemetry` and the SDK's simple span processor. Each finished span
//! is written as one JSON object followed by `\n`:
//!
//! ```text
//! {"type":"span","service":"api"|null,"trace_id":"<32 hex>","span_id":"<16 hex>",
//!  "parent_span_id":"<16 hex>"|null,"name":"handler",
//!  "start_time_unix_nano":<u64>,"end_time_unix_nano":<u64>,
//!  "attributes":{"<key>":<bool|integer|string>,...},
//!  "status":{"code":"unset"|"ok"|"error","description":"<text, error only>"}}
//! ```
//!
//! Log events go to the same writer through the `tracing-subscriber` JSON
//! formatter, so stdout carries only JSON lines. Events, links and dropped counts
//! are not part of the span line, because the formatter already logs every event.
//!
//! `opentelemetry-stdout` is deliberately not used: its 0.27 exporter prints
//! indented text, not JSON, and would break the JSON-only stdout contract.

use std::fmt;
use std::io::{Stdout, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use opentelemetry::trace::{SpanId, Status, TraceError};
use opentelemetry::{Key, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use serde_json::{Map, Value as Json};
use tracing_subscriber::fmt::MakeWriter;

/// Resource attribute that names the emitting process (`api`, `batch`, `mq`).
const SERVICE_NAME_KEY: &str = "service.name";

/// Span exporter that writes each finished span as one JSON line.
///
/// `W` is any [`MakeWriter`]: in the binaries, the writer `telemetry::init`
/// passes, which queues each line for a thread that writes it to standard
/// output; a capture buffer in tests. The `service` key of every line comes from
/// the `service.name` attribute of the tracer provider's resource, which the SDK
/// hands over through [`SpanExporter::set_resource`].
pub struct JsonSpanExporter<W = fn() -> Stdout> {
    writer: W,
    service_name: Option<String>,
}

impl JsonSpanExporter {
    /// Exporter that writes to the process's standard output, synchronously, on
    /// the thread that ends the span. `telemetry::init` does not use it: it hands
    /// the exporter its queued writer instead, so that a slow standard output
    /// does not hold that thread up.
    #[must_use]
    pub fn stdout() -> Self {
        let writer: fn() -> Stdout = std::io::stdout;
        Self::new(writer)
    }
}

impl<W> JsonSpanExporter<W> {
    /// Exporter that writes to `writer`. The service name stays unset until the
    /// tracer provider passes its resource to [`SpanExporter::set_resource`].
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            service_name: None,
        }
    }
}

impl<W> JsonSpanExporter<W>
where
    W: for<'a> MakeWriter<'a>,
{
    /// Serializes `span` and writes it, newline included, with a single
    /// `write_all`, then flushes the writer. The JSON formatter also writes each
    /// line with one `write_all`, so on a writer that keeps every write whole, as
    /// `Stdout` does under its lock and the queued writer of `telemetry::init`
    /// does by queueing each write as one line, a span line never interleaves
    /// with the formatter's lines.
    fn write_line(&self, span: &SpanData) -> ExportResult {
        let mut line = serde_json::to_string(&span_to_json(span, self.service_name.as_deref()))
            .map_err(|err| TraceError::from(err.to_string()))?;
        line.push('\n');
        let mut writer = self.writer.make_writer();
        writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|err| TraceError::from(err.to_string()))
    }
}

// The SDK requires `Debug` on exporters; a writer such as a function pointer or a
// test capture buffer need not implement it, so only the service name is shown.
impl<W> fmt::Debug for JsonSpanExporter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonSpanExporter")
            .field("service_name", &self.service_name)
            .finish_non_exhaustive()
    }
}

impl<W> SpanExporter for JsonSpanExporter<W>
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    /// Writes the batch synchronously, one line per span, and stops at the first
    /// serialization or I/O error. The returned future is already complete: the
    /// simple span processor drives it with a blocking executor under its own lock,
    /// so no async runtime is involved.
    fn export(&mut self, batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        let result = batch.iter().try_for_each(|span| self.write_line(span));
        Box::pin(std::future::ready(result))
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.service_name = resource
            .get(Key::from_static_str(SERVICE_NAME_KEY))
            .map(|value| value.as_str().into_owned());
    }
}

/// The JSON object written for one finished span; the module docs give its schema.
///
/// `service` becomes the `service` key, or JSON `null` when absent. A root span,
/// whose parent is [`SpanId::INVALID`], has `"parent_span_id": null`.
#[must_use]
pub fn span_to_json(span: &SpanData, service: Option<&str>) -> Json {
    let parent_span_id = if span.parent_span_id == SpanId::INVALID {
        Json::Null
    } else {
        Json::String(span.parent_span_id.to_string())
    };
    let attributes: Map<String, Json> = span
        .attributes
        .iter()
        .map(|kv| (kv.key.as_str().to_owned(), attribute_json(&kv.value)))
        .collect();

    let mut line = Map::new();
    line.insert("type".to_owned(), Json::from("span"));
    line.insert("service".to_owned(), service.map_or(Json::Null, Json::from));
    line.insert(
        "trace_id".to_owned(),
        Json::String(span.span_context.trace_id().to_string()),
    );
    line.insert(
        "span_id".to_owned(),
        Json::String(span.span_context.span_id().to_string()),
    );
    line.insert("parent_span_id".to_owned(), parent_span_id);
    line.insert("name".to_owned(), Json::from(span.name.as_ref()));
    line.insert(
        "start_time_unix_nano".to_owned(),
        Json::from(unix_nanos(span.start_time)),
    );
    line.insert(
        "end_time_unix_nano".to_owned(),
        Json::from(unix_nanos(span.end_time)),
    );
    line.insert("attributes".to_owned(), Json::Object(attributes));
    line.insert("status".to_owned(), status_json(&span.status));
    Json::Object(line)
}

/// Booleans, integers and strings keep their JSON type. Every other kind (the
/// floating-point variant, arrays, and any variant a later opentelemetry release
/// adds to the non-exhaustive enum) is written as its `Display` text, so a reader
/// never has to parse a JSON float or a nested array.
fn attribute_json(value: &Value) -> Json {
    match value {
        Value::Bool(flag) => Json::Bool(*flag),
        Value::I64(number) => Json::from(*number),
        Value::String(text) => Json::from(text.as_str()),
        other => Json::String(other.to_string()),
    }
}

fn status_json(status: &Status) -> Json {
    let mut object = Map::new();
    let code = match status {
        Status::Unset => "unset",
        Status::Ok => "ok",
        Status::Error { description } => {
            object.insert("description".to_owned(), Json::from(description.as_ref()));
            "error"
        }
    };
    object.insert("code".to_owned(), Json::from(code));
    Json::Object(object)
}

/// Nanoseconds since the Unix epoch: 0 for an earlier instant, `u64::MAX` when the
/// count does not fit.
fn unix_nanos(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::{self, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    use futures_util::FutureExt;
    use opentelemetry::trace::{
        Span, SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState, Tracer,
        TracerProvider,
    };
    use opentelemetry::{Array, InstrumentationScope, KeyValue, StringValue, Value};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
    use opentelemetry_sdk::trace::{Sampler, SpanEvents, SpanLimits, SpanLinks};
    use serde_json::{Value as Json, json};
    use tracing_subscriber::fmt::MakeWriter;

    use super::{JsonSpanExporter, span_to_json, unix_nanos};

    type TestResult = Result<(), Box<dyn Error>>;

    const TRACE_ID: [u8; 16] = [
        0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e, 0x0e, 0x47,
        0x36,
    ];
    const TRACE_HEX: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
    // Leading zero bytes prove the identifiers are zero-padded to full width.
    const SPAN_ID: [u8; 8] = [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7];
    const SPAN_HEX: &str = "00f067aa0ba902b7";
    const PARENT_ID: [u8; 8] = [0x53, 0x99, 0x5c, 0x3f, 0x42, 0xcd, 0x8a, 0xd8];
    const PARENT_HEX: &str = "53995c3f42cd8ad8";

    /// Shared in-memory sink standing in for stdout.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> Result<String, Box<dyn Error>> {
            let bytes = self
                .0
                .lock()
                .map_err(|_| "capture buffer poisoned")?
                .clone();
            Ok(String::from_utf8(bytes)?)
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

    /// Sink that accepts its first write and fails every later one, counting attempts.
    #[derive(Clone, Default)]
    struct FullAfterOneWrite {
        lines: Capture,
        attempts: Arc<AtomicUsize>,
    }

    impl Write for FullAfterOneWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                self.lines.write(buf)
            } else {
                Err(io::Error::other("device full"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for FullAfterOneWrite {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn span(parent_span_id: SpanId, status: Status, attributes: Vec<KeyValue>) -> SpanData {
        SpanData {
            span_context: SpanContext::new(
                TraceId::from_bytes(TRACE_ID),
                SpanId::from_bytes(SPAN_ID),
                TraceFlags::SAMPLED,
                false,
                TraceState::default(),
            ),
            parent_span_id,
            span_kind: SpanKind::Internal,
            name: "handler".into(),
            start_time: UNIX_EPOCH + Duration::from_nanos(1_500),
            end_time: UNIX_EPOCH + Duration::from_nanos(2_500),
            attributes,
            dropped_attributes_count: 0,
            events: SpanEvents::default(),
            links: SpanLinks::default(),
            status,
            instrumentation_scope: InstrumentationScope::builder("t").build(),
        }
    }

    fn root_span() -> SpanData {
        span(SpanId::INVALID, Status::Unset, Vec::new())
    }

    /// Resolves the exporter's future, which must already be complete.
    fn run<W>(
        exporter: &mut JsonSpanExporter<W>,
        batch: Vec<SpanData>,
    ) -> Result<ExportResult, Box<dyn Error>>
    where
        W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
    {
        exporter
            .export(batch)
            .now_or_never()
            .ok_or_else(|| "export future was still pending".into())
    }

    fn is_lower_hex(text: &str) -> bool {
        text.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }

    #[test]
    fn root_span_line_has_the_schema_keys_and_a_null_parent() {
        let line = span_to_json(&root_span(), None);

        let mut keys: Vec<&str> = line
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "attributes",
                "end_time_unix_nano",
                "name",
                "parent_span_id",
                "service",
                "span_id",
                "start_time_unix_nano",
                "status",
                "trace_id",
                "type",
            ]
        );
        assert_eq!(line["type"], "span");
        assert_eq!(line["service"], Json::Null);
        assert_eq!(line["parent_span_id"], Json::Null);
        assert_eq!(line["name"], "handler");
        assert_eq!(line["attributes"], json!({}));

        let trace_id = line["trace_id"].as_str().unwrap_or_default();
        let span_id = line["span_id"].as_str().unwrap_or_default();
        assert_eq!(trace_id, TRACE_HEX);
        assert_eq!(span_id, SPAN_HEX);
        assert!(trace_id.len() == 32 && is_lower_hex(trace_id), "{trace_id}");
        assert!(span_id.len() == 16 && is_lower_hex(span_id), "{span_id}");
    }

    #[test]
    fn child_span_line_carries_the_parent_id() {
        let child = span(SpanId::from_bytes(PARENT_ID), Status::Unset, Vec::new());
        let line = span_to_json(&child, Some("api"));

        assert_eq!(line["parent_span_id"], PARENT_HEX);
        assert_eq!(line["service"], "api");
    }

    #[test]
    fn status_maps_to_its_code() {
        let cases = [
            (Status::Unset, json!({ "code": "unset" })),
            (Status::Ok, json!({ "code": "ok" })),
            (
                Status::error("boom"),
                json!({ "code": "error", "description": "boom" }),
            ),
        ];
        for (status, expected) in cases {
            let line = span_to_json(&span(SpanId::INVALID, status, Vec::new()), None);
            assert_eq!(line["status"], expected);
        }
    }

    #[test]
    fn attribute_kinds_map_to_json() {
        let attributes = vec![
            KeyValue::new("committed", true),
            KeyValue::new("count", 3_i64),
            KeyValue::new("delta", -7_i64),
            KeyValue::new("transaction", "CAUP"),
            KeyValue::new("ratio", 0.5),
            KeyValue::new("codes", Value::Array(Array::from(vec![1_i64, 2]))),
            KeyValue::new(
                "tags",
                Value::Array(Array::from(vec![
                    StringValue::from("a"),
                    StringValue::from("b"),
                ])),
            ),
        ];
        let line = span_to_json(&span(SpanId::INVALID, Status::Unset, attributes), None);

        assert_eq!(
            line["attributes"],
            json!({
                "committed": true,
                "count": 3,
                "delta": -7,
                "transaction": "CAUP",
                "ratio": "0.5",
                "codes": "[1,2]",
                "tags": "[\"a\",\"b\"]",
            })
        );
    }

    #[test]
    fn times_are_unix_nanoseconds_clamped_to_u64() -> TestResult {
        let line = span_to_json(&root_span(), None);
        assert_eq!(line["start_time_unix_nano"], 1_500_u64);
        assert_eq!(line["end_time_unix_nano"], 2_500_u64);

        let before_epoch = UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .ok_or("an instant before the epoch is not representable")?;
        assert_eq!(unix_nanos(before_epoch), 0);

        // 2e10 seconds is 2e19 nanoseconds, beyond `u64::MAX` (about 1.8e19).
        let beyond_u64 = UNIX_EPOCH
            .checked_add(Duration::from_secs(20_000_000_000))
            .ok_or("an instant past the u64 nanosecond range is not representable")?;
        assert_eq!(unix_nanos(beyond_u64), u64::MAX);
        Ok(())
    }

    #[test]
    fn service_comes_from_the_resource() -> TestResult {
        assert_eq!(
            format!("{:?}", JsonSpanExporter::stdout()),
            "JsonSpanExporter { service_name: None, .. }"
        );

        let capture = Capture::default();
        let mut exporter = JsonSpanExporter::new(capture.clone());
        run(&mut exporter, vec![root_span()])??;

        exporter.set_resource(&Resource::new([KeyValue::new("service.name", "api")]));
        assert_eq!(
            format!("{exporter:?}"),
            "JsonSpanExporter { service_name: Some(\"api\"), .. }"
        );
        run(&mut exporter, vec![root_span()])??;

        exporter.set_resource(&Resource::empty());
        run(&mut exporter, vec![root_span()])??;

        let mut services = Vec::new();
        for line in capture.text()?.lines() {
            let value: Json = serde_json::from_str(line)?;
            services.push(value["service"].clone());
        }
        assert_eq!(services, [Json::Null, json!("api"), Json::Null]);
        Ok(())
    }

    #[test]
    fn export_writes_one_json_line_per_span() -> TestResult {
        let capture = Capture::default();
        let mut exporter = JsonSpanExporter::new(capture.clone());
        let child = span(SpanId::from_bytes(PARENT_ID), Status::Ok, Vec::new());
        run(&mut exporter, vec![root_span(), child])??;

        let text = capture.text()?;
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        assert_eq!(lines.len(), 2, "{text}");
        for line in lines {
            assert!(line.ends_with('\n'), "{line}");
            let value: Json = serde_json::from_str(line)?;
            assert!(value.is_object(), "{line}");
            assert_eq!(value["type"], "span", "{line}");
        }
        Ok(())
    }

    #[test]
    fn export_stops_at_the_first_write_error() -> TestResult {
        let sink = FullAfterOneWrite::default();
        let mut exporter = JsonSpanExporter::new(sink.clone());

        let Err(err) = run(&mut exporter, vec![root_span(), root_span(), root_span()])? else {
            return Err("export succeeded on a full device".into());
        };
        assert_eq!(err.to_string(), "device full");
        assert_eq!(sink.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(sink.lines.text()?.lines().count(), 1);
        Ok(())
    }

    #[test]
    fn sdk_provider_feeds_the_exporter_through_the_simple_processor() -> TestResult {
        let capture = Capture::default();
        // An explicit sampler and span limits keep the SDK from reading OTEL_TRACES_SAMPLER and
        // OTEL_SPAN_*_COUNT_LIMIT from the environment, like `telemetry::subscriber_with_writer`.
        let provider = opentelemetry_sdk::trace::TracerProvider::builder()
            .with_sampler(Sampler::AlwaysOn)
            .with_span_limits(SpanLimits::default())
            .with_simple_exporter(JsonSpanExporter::new(capture.clone()))
            .with_resource(Resource::new([KeyValue::new("service.name", "batch")]))
            .build();
        let mut job = provider.tracer("json_span_exporter").start("job");
        job.set_attribute(KeyValue::new("job", "posttran"));
        job.end();
        provider.shutdown()?;

        let text = capture.text()?;
        assert_eq!(text.lines().count(), 1, "{text}");
        let line: Json = serde_json::from_str(&text)?;
        assert_eq!(line["service"], "batch");
        assert_eq!(line["name"], "job");
        assert_eq!(line["parent_span_id"], Json::Null);
        assert_eq!(line["attributes"]["job"], "posttran");
        assert_eq!(line["status"], json!({ "code": "unset" }));
        Ok(())
    }
}
