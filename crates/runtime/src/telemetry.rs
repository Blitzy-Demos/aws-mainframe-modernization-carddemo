//! Telemetry initialisation shared by the `api`, `batch` and `mq` binaries.
//!
//! One writer, standard output in the binaries, reached through a writer
//! thread, receives two kinds of JSON line:
//!
//! - log events, written by the `tracing-subscriber` JSON formatter with the
//!   current span (`span`) and the list of entered spans (`spans`). An event
//!   inside an exported span also carries that span's OpenTelemetry `trace_id`
//!   and `span_id`, equal to the IDs of that span's line, so every such event
//!   joins the span it was emitted in; an event outside every exported span
//!   carries neither (see Filtering);
//! - finished spans, which pass through `tracing-opentelemetry` into an
//!   OpenTelemetry SDK tracer provider whose simple span processor hands each one
//!   to [`JsonSpanExporter`], which writes it as one `"type":"span"` line.
//!
//! Standard output therefore holds JSON lines only, and nothing in this module
//! prints anywhere else. A failed shutdown at drop time is reported the same way,
//! as one more JSON log line on the same writer (see [`TelemetryGuard`]).
//!
//! In the binaries the formatter and the exporter do not write to standard
//! output themselves: each hands every line, whole, to a queue, and one writer
//! thread writes the queued lines to standard output in order and flushes it.
//! While the queue is open, a thread that logs or ends a span therefore never
//! waits on standard output. The queue is bounded: a slow standard output
//! alone drops no line until the queue holds 16 384 lines or 8 MiB of them.
//! Then the line being written is dropped whole, as is any line above 256 KiB,
//! and counted, and the writer thread follows with one more JSON log line, a
//! WARN event `telemetry lines dropped` whose `dropped_lines` and
//! `dropped_bytes` give the count. A batch of lines that standard output fails
//! to take is discarded, uncounted (see Lifecycle).
//!
//! # Filtering
//!
//! The filter directives (`RUST_LOG`, default `info`) decide what each of the
//! two writers writes, each through a filter of its own:
//!
//! - span lines: the spans the directives enable are exported, and no other.
//!   A span meant to reach the span lines under the default filter must be
//!   created at INFO level or above, for example with `tracing::info_span!`;
//! - event lines: the events the directives enable are written, and no other.
//!   The JSON formatter also keeps, whatever the directives say, every INFO,
//!   WARN and ERROR span of a workspace library crate (a target starting with
//!   `carddemo_`) as the context of the events inside it, for example the
//!   API's request and handler spans with their `transaction`, `user_id` and
//!   resource keys.
//!
//! Under directives that enable those spans, such as `info`, `debug` or
//! `trace`, both writers see the same spans. Under `warn` or `error` they are
//! not exported, and no INFO event is written, but a WARN or ERROR event
//! inside one still carries it as `span` and `spans`, with its fields. Such a
//! span writes no line of its own and has no OpenTelemetry IDs, so the event
//! carries no `trace_id` or `span_id`.
//!
//! A further, fixed filter ahead of both drops two kinds of output of lapin,
//! the AMQP client, whatever the directives say:
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
//! Every span the filters of the span lines enable is exported with its
//! attributes: the tracer provider samples all spans and keeps the SDK's
//! default span limits, whatever the `OTEL_TRACES_SAMPLER` and
//! `OTEL_SPAN_*_COUNT_LIMIT` variables say.
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
//!
//! Until then, the lines reach standard output through the writer thread of
//! [`init`]. Shutting the guard down, or dropping it, shuts the tracer provider
//! down, then closes the queue, and returns only once the writer thread has
//! handed every line queued until then to standard output, the span lines the
//! shutdown exports included, followed by the report of any line dropped since
//! the last one. Each is written and flushed unless standard output fails to
//! take it. A line written after that, such as the event a binary logs when the
//! shutdown fails, is written and flushed to standard output directly by the
//! thread that writes it.

use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};

use opentelemetry::KeyValue;
use opentelemetry::trace::{
    SpanId, TraceContextExt as _, TraceError, TraceId, TracerProvider as _,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{Sampler, SpanLimits, TracerProvider};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_opentelemetry::OtelData;
use tracing_subscriber::filter::{EnvFilter, FilterExt as _, ParseError, filter_fn};
use tracing_subscriber::fmt::format::{
    Format, FormatEvent, FormatFields, Json, JsonFields, Writer,
};
use tracing_subscriber::fmt::time::{FormatTime as _, SystemTime};
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::fmt::{self, FmtContext, MakeWriter};
use tracing_subscriber::layer::{Layer as _, SubscriberExt};
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
/// The predicate runs as a global filter layer, ahead of the filters of the
/// two writers, and the subscriber enables a span or event only when this
/// layer does and at least one writer's filter does. No `RUST_LOG` directive
/// and no context span ([`is_context_span`]) can therefore re-enable this
/// output, neither a target directive such as `lapin::returned_messages=warn`
/// nor a span directive such as `[outer]=trace`, which enables every event
/// inside `outer` whatever the target directives say. Targets are matched by
/// module, as [`is_in_module`] describes.
fn carries_no_sensitive_data(metadata: &Metadata<'_>) -> bool {
    let target = metadata.target();
    if is_in_module(target, LAPIN_RETURNED_MESSAGES_TARGET) {
        return false;
    }
    !is_in_module(target, LAPIN_TARGET) || *metadata.level() != Level::TRACE
}

/// The start of every target in a workspace library crate: the packages are
/// named `carddemo-*`, so their crates, and the module paths that are their
/// targets, are named `carddemo_*`.
const WORKSPACE_CRATE_PREFIX: &str = "carddemo_";

/// Whether `target` is in a workspace library crate: its crate, the first
/// segment of the module path, starts with [`WORKSPACE_CRATE_PREFIX`]. The
/// binaries' own crates (`api`, `batch`, `mq`, `openapi`) do not.
fn is_in_workspace_crate(target: &str) -> bool {
    target.starts_with(WORKSPACE_CRATE_PREFIX)
}

/// Whether a span is a context span, one the JSON formatter keeps as the
/// context of the events inside it whatever the filter directives say: a span,
/// never an event, at INFO level or above, of a workspace library crate
/// ([`is_in_workspace_crate`]).
///
/// These are the spans that hold what an operation is and whom it serves: the
/// API's request and handler spans with `transaction`, `user_id`, the
/// resource keys and `outcome`, and the message spans of `mq`. Under
/// directives above INFO, such as `warn`, a WARN or ERROR event inside one
/// still carries it as `span` and `spans`. The predicate enables no event, so
/// it adds no event line, and the JSON formatter writes no line for a span, so
/// it adds no span line either. Its result depends on the metadata alone, so
/// each callsite's interest is decided once and cached.
fn is_context_span(metadata: &Metadata<'_>) -> bool {
    metadata.is_span()
        && *metadata.level() <= Level::INFO
        && is_in_workspace_crate(metadata.target())
}

/// Failure to set up or shut down telemetry.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// The filter directives do not follow the `EnvFilter` grammar.
    ///
    /// [`init`] reads the directives from `RUST_LOG`, their only source in the
    /// binaries, so the text names that variable: `invalid RUST_LOG: ` and the
    /// parser's reason.
    #[error("invalid {variable}: {0}", variable = keys::RUST_LOG)]
    Filter(#[from] ParseError),
    /// A global default subscriber is already installed.
    #[error("failed to install the tracing subscriber: {0}")]
    Install(#[from] TryInitError),
    /// The tracer provider failed to shut down its span processor.
    #[error("failed to shut down the tracer provider: {0}")]
    Shutdown(#[from] TraceError),
    /// The thread that writes the JSON lines to standard output did not start.
    #[error("failed to start the standard output writer: {0}")]
    Writer(#[source] io::Error),
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
///
/// The guard [`init`] returns also owns the queue through which the lines
/// reach standard output, and closes it after the provider, on shutdown and
/// on drop alike, whether or not the provider shut down. Closing waits until
/// the writer thread has handed every line queued until then to standard
/// output, written and flushed unless standard output fails to take it: the
/// span lines the provider's shutdown exports, the drop-time failure line, and
/// the report of any line the bounded queue dropped since the last one. A line
/// written after that goes straight to standard output, on the thread that
/// writes it. A guard from [`subscriber_with_writer`] owns no queue: its
/// writer is the caller's.
#[must_use]
#[derive(Debug)]
pub struct TelemetryGuard {
    provider: Option<TracerProvider>,
    /// The subscriber's writer, taken by the drop-time failure report.
    writer: Option<BoxMakeWriter>,
    /// The queue the subscriber writes through, taken when it is closed.
    queue: Option<Arc<dyn CloseQueue>>,
}

impl TelemetryGuard {
    /// Shuts the tracer provider down, which shuts down its JSON span exporter,
    /// then closes the standard output queue of a guard from [`init`], which
    /// hands every line queued until then to standard output, written and
    /// flushed unless standard output fails to take it. Spans started
    /// afterwards are not exported.
    ///
    /// # Errors
    ///
    /// [`TelemetryError::Shutdown`] when the provider fails to shut down its span
    /// processor, or was already shut down through another handle. The queue is
    /// closed either way.
    pub fn shutdown(mut self) -> Result<(), TelemetryError> {
        let result = self.shut_down_provider();
        self.close_queue();
        result
    }

    /// Takes the provider and shuts it down; `Ok` when it is already gone.
    fn shut_down_provider(&mut self) -> Result<(), TelemetryError> {
        match self.provider.take() {
            Some(provider) => provider.shutdown().map_err(TelemetryError::from),
            None => Ok(()),
        }
    }

    /// Takes the queue, if any, and closes it.
    fn close_queue(&mut self) {
        if let Some(queue) = self.queue.take() {
            queue.close();
        }
    }

    /// Writes `err`, a failed drop-time shutdown, as the `telemetry shutdown
    /// failed` line on the subscriber's writer, which it takes.
    fn report_drop_failure(&mut self, err: &TelemetryError) {
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

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Err(err) = self.shut_down_provider() {
            self.report_drop_failure(&err);
        }
        // Closed last, so that the failure line, queued like every other, is
        // handed to the sink as well.
        self.close_queue();
    }
}

/// Bytes of queued lines the writer thread gathers into one write. A batch
/// stops gathering once it holds this many, so it exceeds them by less than
/// one line, and no line is ever split across two writes.
const BATCH_BYTES: usize = 64 * 1024;

/// Name of the thread that writes the queued lines.
const WRITER_THREAD_NAME: &str = "telemetry-stdout";

/// Lines a [`LineQueue`] holds at most: the capacity of its channel. A line
/// written while the queue holds this many is dropped.
const QUEUE_LINES: usize = 16_384;

/// Bytes of lines a [`LineQueue`] holds at most. A line that would take the
/// queued lines above this many bytes is dropped.
const QUEUE_BYTES: usize = 8 * 1024 * 1024;

/// Bytes of the longest line a [`LineQueue`] takes. A longer line is dropped
/// whatever the queue holds.
const MAX_LINE_BYTES: usize = 256 * 1024;

// A line the queue takes always fits into an empty queue.
const _: () = assert!(MAX_LINE_BYTES <= QUEUE_BYTES);

/// Message of the event line that reports dropped lines.
const DROP_REPORT_MESSAGE: &str = "telemetry lines dropped";

/// A queue of JSON lines and the writer thread that writes them to a sink:
/// standard output in the binaries, a capture buffer or a slow stand-in in
/// tests.
///
/// Every write through a `&LineQueue` hands its whole buffer to the thread as
/// one line, or drops it whole, and returns at once. The JSON formatter and
/// [`JsonSpanExporter`] each write a line with a single `write_all`, so every
/// line the queue takes reaches the sink whole, unless the sink fails to take
/// it, and lines never interleave. The thread writes the lines in the order
/// they were queued.
///
/// The queue is bounded, so that a sink that falls behind, such as standard
/// output into a pipe whose reader stalls, cannot grow it without limit: it
/// holds at most [`QUEUE_LINES`] lines and [`QUEUE_BYTES`] bytes of them, and
/// takes no line above [`MAX_LINE_BYTES`]. A write never waits for room. A
/// slow sink alone drops no line until the queue is full; then the line being
/// written is dropped whole, as is a line above [`MAX_LINE_BYTES`] at any
/// time, never cut short, and the lines already queued are kept. The write
/// still reports success, so that neither the JSON formatter nor the span
/// exporter prints an error outside standard output. Every dropped line is
/// counted, and the writer thread reports the count as a `telemetry lines
/// dropped` line (see [`write_lines`]).
///
/// The queue has one sender, which writers borrow under the read lock and
/// never clone, so that [`LineQueue::close`], under the write lock, can drop
/// it. Dropping the queue closes it.
struct LineQueue<S> {
    /// The sender and the writer thread, until the queue is closed.
    open: RwLock<Option<OpenQueue<S>>>,
    /// The bytes queued and the lines dropped, shared with the writer thread.
    counters: Arc<QueueCounters>,
    /// The sink the writer thread returned when the queue was closed, which
    /// every later write goes to directly.
    sink: Mutex<Option<S>>,
}

/// The two ends of an open [`LineQueue`] that its writers do not own.
struct OpenQueue<S> {
    sender: SyncSender<Vec<u8>>,
    /// The writer thread, which returns the sink once the queue is closed and
    /// drained.
    thread: JoinHandle<S>,
}

/// What the writers of a [`LineQueue`] and its writer thread share.
///
/// A writer reserves the bytes of its line before it queues the line, and the
/// writer thread releases them as it takes the line, so `queued_bytes` never
/// exceeds [`QUEUE_BYTES`]. A writer counts a dropped line's bytes before the
/// line, and the writer thread takes the line count first and the byte count
/// only when lines were dropped, so no dropped byte goes unreported.
#[derive(Debug, Default)]
struct QueueCounters {
    /// Bytes of the lines queued and not yet taken by the writer thread.
    queued_bytes: AtomicUsize,
    /// Lines dropped since the writer thread last reported.
    dropped_lines: AtomicUsize,
    /// Bytes of those lines.
    dropped_bytes: AtomicUsize,
}

impl QueueCounters {
    /// Reserves `len` bytes for a line to queue; `false`, reserving nothing,
    /// when the queued lines would then exceed [`QUEUE_BYTES`].
    fn reserve(&self, len: usize) -> bool {
        self.queued_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |queued| {
                queued
                    .checked_add(len)
                    .filter(|&total| total <= QUEUE_BYTES)
            })
            .is_ok()
    }

    /// Releases `len` reserved bytes: the line was taken, or never queued.
    fn release(&self, len: usize) {
        self.queued_bytes.fetch_sub(len, Ordering::Relaxed);
    }

    /// Counts one dropped line of `len` bytes. The first drop since the last
    /// report also queues an empty message, which holds no line, to wake the
    /// writer thread so that the report follows promptly even on an idle
    /// queue. Should that fail, the queue is full and the writer thread wakes
    /// for its lines anyway, or the thread is gone.
    fn count_drop(&self, len: usize, sender: &SyncSender<Vec<u8>>) {
        saturating_add(&self.dropped_bytes, len);
        if saturating_add(&self.dropped_lines, 1) == 0 {
            let _ = sender.try_send(Vec::new());
        }
    }

    /// The report of the lines dropped since the last one, which resets both
    /// counts, or `None` when no line was dropped.
    fn take_drop_report(&self) -> Option<Vec<u8>> {
        let lines = self.dropped_lines.swap(0, Ordering::Relaxed);
        if lines == 0 {
            return None;
        }
        let bytes = self.dropped_bytes.swap(0, Ordering::Relaxed);
        Some(drop_report(lines, bytes))
    }
}

/// Adds `n` to `counter`, stopping at `usize::MAX`, and returns the count
/// before.
fn saturating_add(counter: &AtomicUsize, n: usize) -> usize {
    match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_add(n))
    }) {
        Ok(count) | Err(count) => count,
    }
}

/// The JSON line reporting `lines` dropped lines of `bytes` bytes: a WARN
/// event line with the JSON formatter's top-level keys, its clock and
/// timestamp format, and this module as its target, for example
///
/// ```text
/// {"fields":{"dropped_bytes":4096,"dropped_lines":12,"message":"telemetry lines dropped"},"level":"WARN","target":"carddemo_runtime::telemetry","timestamp":"2026-11-05T06:55:35.323547Z"}
/// ```
///
/// with the keys in the order `serde_json` writes them. It is built here, not
/// through a subscriber of its own, so it is written whatever the filter
/// directives say and leaves every callsite's cached interest and the global
/// maximum level as they are.
fn drop_report(lines: usize, bytes: usize) -> Vec<u8> {
    let mut timestamp = String::new();
    // Formatting into a `String` cannot fail.
    let _ = SystemTime.format_time(&mut Writer::new(&mut timestamp));
    let report = serde_json::json!({
        "timestamp": timestamp,
        "level": Level::WARN.as_str(),
        "fields": {
            "message": DROP_REPORT_MESSAGE,
            "dropped_lines": lines,
            "dropped_bytes": bytes,
        },
        "target": module_path!(),
    });
    let mut line = report.to_string().into_bytes();
    line.push(b'\n');
    line
}

impl<S> LineQueue<S> {
    /// Closes the queue and waits for the writer thread to finish: every line
    /// queued until then is handed to the sink, in order, and written and
    /// flushed unless the sink fails, followed by the report of any line
    /// dropped since the last one, before this returns. A channel delivers
    /// every queued line before it reports that its sender is gone, so the
    /// thread ends only once the queue is empty.
    ///
    /// The write lock is held until the thread has returned the sink and the
    /// sink is in place, so a write that waits for the lock meanwhile goes to
    /// the sink directly, after the last queued line. Closing a closed queue does
    /// nothing.
    ///
    /// Should the writer thread have panicked, its sink is gone with it: the
    /// queue is closed without one, and every later write fails with
    /// [`io::ErrorKind::BrokenPipe`]. The writer thread itself never panics
    /// over standard output, whose write errors it drops, so only a sink that
    /// panics, such as a test stand-in, can lead there.
    fn close(&self) {
        let mut open = self.open.write().unwrap_or_else(PoisonError::into_inner);
        let Some(OpenQueue { sender, thread }) = open.take() else {
            return;
        };
        drop(sender);
        if let Ok(sink) = thread.join() {
            *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
        }
        drop(open);
    }

    /// Queues `line`, which is not empty, on the open `queue`, or drops it
    /// whole when it is above [`MAX_LINE_BYTES`] or would take the queue above
    /// [`QUEUE_BYTES`] bytes or [`QUEUE_LINES`] lines; never waits. A dropped
    /// line is counted, and its length returned as for a queued one.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::BrokenPipe`] when the writer thread is gone.
    fn enqueue(&self, queue: &OpenQueue<S>, line: &[u8]) -> io::Result<usize> {
        let len = line.len();
        // Checked before anything is copied, so a dropped line costs no allocation.
        if len <= MAX_LINE_BYTES && self.counters.reserve(len) {
            match queue.sender.try_send(line.to_vec()) {
                Ok(()) => return Ok(len),
                Err(TrySendError::Full(_)) => self.counters.release(len),
                Err(TrySendError::Disconnected(_)) => {
                    self.counters.release(len);
                    return Err(writer_stopped());
                }
            }
        }
        // While the queue is open, the writer thread ends only by panicking.
        if queue.thread.is_finished() {
            return Err(writer_stopped());
        }
        self.counters.count_drop(len, &queue.sender);
        Ok(len)
    }
}

impl<S> Drop for LineQueue<S> {
    fn drop(&mut self) {
        self.close();
    }
}

// The sink need not implement `Debug`, so only whether the queue is still open
// is shown.
impl<S> std::fmt::Debug for LineQueue<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let open = self
            .open
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("LineQueue")
            .field("open", &open)
            .finish_non_exhaustive()
    }
}

impl<S: Write> Write for &LineQueue<S> {
    /// Queues `buf` as one line, or drops it whole when the bounded queue
    /// cannot take it, and returns its length either way, without waiting for
    /// the writer thread. An empty `buf` is not a line: nothing is
    /// queued and the result is 0. Once the queue is closed, `buf` is written
    /// to the sink and the sink flushed before this returns.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::BrokenPipe`] when the writer thread has panicked and
    /// taken the sink with it, and, once the queue is closed, the sink's own
    /// write or flush error.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let open = self.open.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(queue) = open.as_ref() {
            return self.enqueue(queue, buf);
        }
        drop(open);
        let mut sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        let sink = sink.as_mut().ok_or_else(writer_stopped)?;
        sink.write_all(buf)?;
        sink.flush()?;
        Ok(buf.len())
    }

    /// Returns at once: the writer thread flushes the sink after every batch
    /// it writes, and a write after close flushes it itself.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The error of a write that finds the writer thread gone without its sink.
fn writer_stopped() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the telemetry writer thread stopped",
    )
}

/// The body of the writer thread: writes every line `lines` delivers to
/// `sink`, releasing its bytes in `counters` as it takes it, and returns the
/// sink once the queue is closed and empty.
///
/// It waits for a line, gathers the lines already queued behind it into one
/// batch of up to [`BATCH_BYTES`], then writes the batch with one `write_all`
/// and flushes the sink. When lines were dropped since the last report, the
/// batch ends with a new report: one `telemetry lines dropped` WARN event
/// line ([`drop_report`]) whose `dropped_lines` and `dropped_bytes` count
/// them. A report is written whatever the filter directives say, and is never
/// itself queued, so the queue's bounds never drop it. An empty message is
/// the wake-up of [`QueueCounters::count_drop`] and adds no line. Once the
/// queue is closed and empty, a last report covers the lines dropped since the
/// last batch.
///
/// A batch the sink fails to take is discarded, report included, uncounted,
/// and the thread goes on with the next one: the JSON formatter drops a line
/// it fails to write as well, and a thread that stopped would lose every line
/// queued after the failure.
fn write_lines<S: Write>(lines: &Receiver<Vec<u8>>, counters: &QueueCounters, mut sink: S) -> S {
    let mut batch = Vec::with_capacity(BATCH_BYTES);
    while let Ok(line) = lines.recv() {
        counters.release(line.len());
        batch.extend_from_slice(&line);
        while batch.len() < BATCH_BYTES {
            let Ok(line) = lines.try_recv() else {
                break;
            };
            counters.release(line.len());
            batch.extend_from_slice(&line);
        }
        if let Some(report) = counters.take_drop_report() {
            batch.extend_from_slice(&report);
        }
        // A wake-up whose drops an earlier batch already reported leaves
        // nothing to write.
        if !batch.is_empty() {
            // A batch the sink fails to take is discarded; the next one is written.
            let _ = sink.write_all(&batch).and_then(|()| sink.flush());
        }
        batch.clear();
        batch.shrink_to(BATCH_BYTES);
    }
    if let Some(report) = counters.take_drop_report() {
        // As for a batch, a report the sink fails to take is discarded.
        let _ = sink.write_all(&report).and_then(|()| sink.flush());
    }
    sink
}

/// A [`LineQueue`] of any sink, as [`TelemetryGuard`] holds it to close it.
trait CloseQueue: std::fmt::Debug + Send + Sync {
    /// Closes the queue; see [`LineQueue::close`].
    fn close(&self);
}

impl<S: Send> CloseQueue for LineQueue<S> {
    fn close(&self) {
        LineQueue::close(self);
    }
}

/// The [`MakeWriter`] of a [`LineQueue`]: every writer it makes queues its
/// lines for the queue's writer thread. Its clones share the one queue.
struct QueuedWriter<S>(Arc<LineQueue<S>>);

impl<S: Write + Send + 'static> QueuedWriter<S> {
    /// Starts the writer thread over `sink` and returns the writer of its
    /// queue.
    ///
    /// # Errors
    ///
    /// The error of a thread that cannot be started.
    fn start(sink: S) -> io::Result<Self> {
        // Bounded to `QUEUE_LINES` lines; a write never waits for room, but
        // drops its line when there is none (see `LineQueue::enqueue`).
        let (sender, lines) = mpsc::sync_channel(QUEUE_LINES);
        let counters = Arc::new(QueueCounters::default());
        let thread_counters = Arc::clone(&counters);
        let thread = thread::Builder::new()
            .name(WRITER_THREAD_NAME.to_owned())
            .spawn(move || write_lines(&lines, &thread_counters, sink))?;
        Ok(Self(Arc::new(LineQueue {
            open: RwLock::new(Some(OpenQueue { sender, thread })),
            counters,
            sink: Mutex::new(None),
        })))
    }
}

impl<S> Clone for QueuedWriter<S> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<'a, S: Write + 'a> MakeWriter<'a> for QueuedWriter<S> {
    type Writer = &'a LineQueue<S>;

    fn make_writer(&'a self) -> Self::Writer {
        &self.0
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
/// formatter's line is written unchanged: for an event outside every span, for
/// one inside a context span the OpenTelemetry layer's filter did not enable
/// (see [`subscriber_with_writer`]), and for one under a subscriber without the
/// OpenTelemetry layer, such as the drop-time failure reporter of
/// [`TelemetryGuard`].
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
/// Each line is written to `writer` on the thread that emits the event or ends
/// the span, a span line under the simple span processor's lock, so a slow
/// `writer` holds that thread up. [`init`] passes a writer that never waits:
/// it queues each line for a writer thread of its own, or drops and counts it
/// when the bounded queue is full or the line is above `MAX_LINE_BYTES`.
///
/// The layers, in order:
///
/// 1. a fixed global filter that drops, for both layers after it and whatever
///    `directives` say, lapin's TRACE-level spans and events, whose outgoing
///    `connection.start-ok` frame holds the broker credentials, and every span
///    and event of `lapin::returned_messages`, whose WARN dump of a returned
///    message holds its whole body;
/// 2. the `tracing-subscriber` JSON formatter, with the current span and the
///    span list on every event line, that span's `trace_id` and `span_id` on an
///    event inside one the OpenTelemetry layer exports, and no ANSI colour
///    codes. Its own filter is an `EnvFilter` parsed from `directives` or the
///    context-span rule: it writes the events the directives enable, and it
///    keeps every INFO, WARN and ERROR span whose target starts with
///    `carddemo_` as the context of the events inside it, so that under `warn`
///    or `error` such an event still carries the span's fields;
/// 3. the `tracing-opentelemetry` layer, on a tracer named `service_name` from a
///    tracer provider of its own whose simple span processor feeds a
///    [`JsonSpanExporter`] on `writer`, with `service_name` as the resource's
///    `service.name`. Its own filter is a second `EnvFilter` parsed from
///    `directives`, so the span lines are the spans the directives enable. The
///    provider samples every span (`Sampler::AlwaysOn`) and keeps the SDK's
///    default span limits (128 attributes, events and links per span), both
///    set explicitly.
///
/// A span the directives enable reaches both layers, so its events carry its
/// IDs, as under `info`, `debug` and `trace`. A context span the directives
/// do not enable reaches the JSON formatter alone: it has no span line and no
/// OpenTelemetry IDs, and the events inside it carry no `trace_id` or
/// `span_id`.
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
    // `EnvFilter` is not `Clone`, so each layer's filter parses the directives.
    let event_filter = EnvFilter::try_new(directives)?
        .or(filter_fn(is_context_span).with_max_level_hint(Level::INFO));
    let span_filter = EnvFilter::try_new(directives)?;
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
        queue: None,
    };
    let subscriber = tracing_subscriber::registry()
        .with(filter_fn(carries_no_sensitive_data))
        .with(json_log_layer(writer).with_filter(event_filter))
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(span_filter),
        );
    Ok((subscriber, guard))
}

/// Builds the subscriber of [`subscriber_with_writer`] over a bounded
/// [`LineQueue`] whose writer thread writes the queued lines to `sink`, and
/// gives the returned guard the queue to close at shutdown or drop.
///
/// # Errors
///
/// - [`TelemetryError::Writer`] when the writer thread cannot be started.
/// - [`TelemetryError::Filter`] as for [`subscriber_with_writer`]; the queue,
///   which holds no line yet, is then closed.
fn subscriber_with_queue<S>(
    service_name: &str,
    directives: &str,
    sink: S,
) -> Result<
    (
        impl tracing::Subscriber + Send + Sync + 'static,
        TelemetryGuard,
    ),
    TelemetryError,
>
where
    S: Write + Send + 'static,
{
    let writer = QueuedWriter::start(sink).map_err(TelemetryError::Writer)?;
    let (subscriber, mut guard) = subscriber_with_writer(service_name, directives, writer.clone())?;
    guard.queue = Some(writer.0);
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
/// Standard output is written by a thread of its own, named
/// `telemetry-stdout`: the JSON formatter and the span exporter only queue
/// each line for it, so a slow standard output, such as a pipe whose reader
/// stalls, holds up no thread that logs or ends a span. The queue is bounded
/// to 16 384 lines and 8 MiB, and takes no line above 256 KiB: a slow
/// standard output alone drops no line until the queue is full, and every
/// dropped line is counted and reported as a `telemetry lines dropped` WARN
/// line with `dropped_lines` and `dropped_bytes`. The guard closes the
/// queue, which hands every queued line to standard output, written and
/// flushed unless standard output fails to take it, when it shuts down or is
/// dropped.
///
/// Call it once, at the start of the process, and keep the guard alive until
/// exit, then call [`TelemetryGuard::shutdown`].
///
/// # Errors
///
/// - [`TelemetryError::Writer`] when the standard output writer thread cannot
///   be started.
/// - [`TelemetryError::Filter`] when `RUST_LOG` holds an invalid directive list.
/// - [`TelemetryError::Install`] when a global default subscriber is already
///   installed, for example by an earlier call.
pub fn init(service_name: &str) -> Result<TelemetryGuard, TelemetryError> {
    let directives = rust_log_directives(std::env::var(keys::RUST_LOG).ok());
    let (subscriber, guard) = subscriber_with_queue(service_name, &directives, io::stdout())?;
    subscriber.try_init()?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::error::Error;
    use std::io::{self, Write};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::Value as Json;
    use tracing_subscriber::fmt::MakeWriter;

    use super::{
        BATCH_BYTES, DROP_REPORT_MESSAGE, MAX_LINE_BYTES, QUEUE_BYTES, QUEUE_LINES, QueuedWriter,
        TelemetryError, keys, subscriber_with_queue, subscriber_with_writer,
    };

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
    fn events_above_info_keep_the_context_of_their_application_spans() -> TestResult {
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "warn", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || {
            // Neither another crate's INFO span nor an application DEBUG span
            // is a context span.
            let foreign = tracing::info_span!(target: "tower_http::trace", "foreign");
            let _foreign = foreign.enter();
            let request = tracing::info_span!("request", route = "/accounts/{id}");
            let _request = request.enter();
            let handler = tracing::info_span!(
                "update_account",
                transaction = "CAUP",
                account_id = "00000000001",
                outcome = tracing::field::Empty,
            );
            let _handler = handler.enter();
            handler.record("outcome", "failed");
            tracing::debug_span!("detail").in_scope(|| {
                tracing::info!("below the directives");
                tracing::error!(code = "UNEXPECTED", "unexpected failure answered");
            });
        });
        guard.shutdown()?;

        let lines = capture.json_lines()?;
        assert!(span_lines(&lines).is_empty(), "{lines:?}");
        let [event] = &lines[..] else {
            return Err(format!("expected exactly the ERROR event line: {lines:?}").into());
        };
        assert_eq!(event["level"], "ERROR", "{event}");
        assert_eq!(
            event["fields"],
            serde_json::json!({"message": "unexpected failure answered", "code": "UNEXPECTED"}),
            "{event}"
        );
        let handler = serde_json::json!({
            "name": "update_account",
            "transaction": "CAUP",
            "account_id": "00000000001",
            "outcome": "failed",
        });
        assert_eq!(event["span"], handler, "{event}");
        assert_eq!(
            event["spans"],
            serde_json::json!([{"name": "request", "route": "/accounts/{id}"}, handler]),
            "{event}"
        );
        assert!(event.get("trace_id").is_none(), "{event}");
        assert!(event.get("span_id").is_none(), "{event}");
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
            let prefix = format!("invalid {}: ", keys::RUST_LOG);
            assert!(err.to_string().starts_with(&prefix), "{err}");
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

    /// The longest a gated [`RecordingSink`] waits for its gate, so that a test
    /// that never opens it cannot hang.
    const GATE_TIMEOUT: Duration = Duration::from_secs(10);

    /// One call a [`RecordingSink`] received.
    #[derive(Debug, PartialEq)]
    enum SinkCall {
        /// A write of this many bytes.
        Write(usize),
        Flush,
    }

    /// What a [`RecordingSink`] received, in order.
    #[derive(Default)]
    struct Recording {
        bytes: Vec<u8>,
        calls: Vec<SinkCall>,
    }

    /// A sink standing in for standard output, which records every write and
    /// flush. With a gate, its first write waits until the gate's sender is
    /// dropped, or for [`GATE_TIMEOUT`].
    struct RecordingSink {
        recording: Arc<Mutex<Recording>>,
        gate: Option<Receiver<()>>,
    }

    impl RecordingSink {
        fn record(&self, call: SinkCall, buf: &[u8]) -> io::Result<()> {
            let mut recording = self
                .recording
                .lock()
                .map_err(|_| io::Error::other("recording poisoned"))?;
            recording.bytes.extend_from_slice(buf);
            recording.calls.push(call);
            Ok(())
        }
    }

    impl Write for RecordingSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(gate) = self.gate.take() {
                // Opened by the sender's drop, or given up on after the timeout.
                let _ = gate.recv_timeout(GATE_TIMEOUT);
            }
            self.record(SinkCall::Write(buf.len()), buf)?;
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.record(SinkCall::Flush, &[])
        }
    }

    /// A queue over a new [`RecordingSink`], with what the sink records.
    fn recording_queue(
        gate: Option<Receiver<()>>,
    ) -> io::Result<(QueuedWriter<RecordingSink>, Arc<Mutex<Recording>>)> {
        let recording = Arc::new(Mutex::new(Recording::default()));
        let sink = RecordingSink {
            recording: Arc::clone(&recording),
            gate,
        };
        Ok((QueuedWriter::start(sink)?, recording))
    }

    fn lock(recording: &Mutex<Recording>) -> Result<MutexGuard<'_, Recording>, Box<dyn Error>> {
        recording
            .lock()
            .map_err(|_| Box::<dyn Error>::from("recording poisoned"))
    }

    /// Writes `line` as the JSON formatter and the span exporter do: with one
    /// `write_all` on a writer from `make_writer`.
    fn write_line<S: Write + 'static>(writer: &QueuedWriter<S>, line: &[u8]) -> io::Result<()> {
        writer.make_writer().write_all(line)
    }

    /// Line `n` of a test: `{"n":"<n as 5 digits>","pad":"x…x"}` and a
    /// newline, `len` bytes long for `n` below 100 000.
    fn numbered_line(n: usize, len: usize) -> String {
        let pad = "x".repeat(len - 23);
        format!("{{\"n\":\"{n:05}\",\"pad\":\"{pad}\"}}\n")
    }

    /// Checks that `bytes` hold exactly the lines of `writers` writers, each of
    /// which wrote `lines` [`numbered_line`]s of `line_len` bytes, writer `w`
    /// numbering its lines from `w * lines` up: every line whole, and each
    /// writer's lines in the order it wrote them.
    fn assert_whole_lines_in_order(
        bytes: &[u8],
        writers: usize,
        lines: usize,
        line_len: usize,
    ) -> TestResult {
        let text = std::str::from_utf8(bytes)?;
        let mut next = vec![0; writers];
        for line in text.split_inclusive('\n') {
            let value: Json = serde_json::from_str(line)?;
            let n: usize = value["n"]
                .as_str()
                .ok_or_else(|| format!("no line number: {line}"))?
                .parse()?;
            assert_eq!(line, numbered_line(n, line_len), "a torn line");
            let writer_no = n / lines;
            assert_eq!(
                n % lines,
                next[writer_no],
                "writer {writer_no} out of order"
            );
            next[writer_no] += 1;
        }
        assert_eq!(next, vec![lines; writers]);
        Ok(())
    }

    /// A [`RecordingSink`] that reports through `entered` that its first write
    /// has begun, so that a test knows which lines the writer thread holds in
    /// its first batch while the sink's gate keeps it waiting.
    struct EnteredSink {
        sink: RecordingSink,
        entered: Option<mpsc::Sender<()>>,
    }

    impl Write for EnteredSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                // The test may have stopped waiting; the write goes on either way.
                let _ = entered.send(());
            }
            self.sink.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.sink.flush()
        }
    }

    /// `true` when `text` is a timestamp as the JSON formatter writes it, such
    /// as `2026-11-05T06:55:35.323547Z`.
    fn is_formatter_timestamp(text: &str) -> bool {
        text.len() == 27
            && text.bytes().enumerate().all(|(i, b)| match i {
                4 | 7 => b == b'-',
                10 => b == b'T',
                13 | 16 => b == b':',
                19 => b == b'.',
                26 => b == b'Z',
                _ => b.is_ascii_digit(),
            })
    }

    /// The sorted keys of the JSON object `value`; none when it is no object.
    fn sorted_keys(value: &Json) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        keys
    }

    /// Checks that `report` is a drop report line, a WARN event of this
    /// module with the formatter's keys and timestamp, and returns its
    /// `dropped_lines` and `dropped_bytes`, the first never 0.
    fn drop_report_counts(report: &Json) -> Result<(usize, usize), Box<dyn Error>> {
        assert_eq!(
            sorted_keys(report),
            ["fields", "level", "target", "timestamp"],
            "{report}"
        );
        assert_eq!(report["level"], "WARN", "{report}");
        assert_eq!(report["target"], "carddemo_runtime::telemetry", "{report}");
        let timestamp = report["timestamp"].as_str().unwrap_or_default();
        assert!(is_formatter_timestamp(timestamp), "{report}");
        let fields = &report["fields"];
        assert_eq!(
            sorted_keys(fields),
            ["dropped_bytes", "dropped_lines", "message"],
            "{report}"
        );
        assert_eq!(fields["message"], DROP_REPORT_MESSAGE, "{report}");
        let count = |key: &str| {
            fields[key]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| format!("no `{key}` count: {report}"))
        };
        let lines = count("dropped_lines")?;
        assert!(lines > 0, "a report of no dropped line: {report}");
        Ok((lines, count("dropped_bytes")?))
    }

    /// What a queue delivered: its whole data lines and the sums of its drop
    /// reports.
    #[derive(Debug, Default, PartialEq)]
    struct Delivery {
        lines: usize,
        bytes: usize,
        reports: usize,
        dropped_lines: usize,
        dropped_bytes: usize,
    }

    /// Reads `bytes` as JSON lines, each either a drop report or a
    /// [`numbered_line`] of `line_len` bytes. Line `n` belongs to producer
    /// `n / lines_each`, and each producer's lines must arrive in increasing
    /// order, with gaps where lines were dropped.
    fn read_delivery(
        bytes: &[u8],
        lines_each: usize,
        line_len: usize,
    ) -> Result<Delivery, Box<dyn Error>> {
        let text = std::str::from_utf8(bytes)?;
        let mut last: Vec<Option<usize>> = Vec::new();
        let mut delivery = Delivery::default();
        for line in text.split_inclusive('\n') {
            let value: Json = serde_json::from_str(line)
                .map_err(|err| format!("line is not JSON ({err}): {line}"))?;
            if value["fields"]["message"] == DROP_REPORT_MESSAGE {
                let (lines, bytes) = drop_report_counts(&value)?;
                delivery.reports += 1;
                delivery.dropped_lines += lines;
                delivery.dropped_bytes += bytes;
                continue;
            }
            let n: usize = value["n"]
                .as_str()
                .ok_or_else(|| format!("neither a data line nor a report: {line}"))?
                .parse()?;
            assert_eq!(line, numbered_line(n, line_len), "a torn line");
            let producer = n / lines_each;
            if last.len() <= producer {
                last.resize(producer + 1, None);
            }
            if let Some(before) = last[producer] {
                assert!(n > before, "producer {producer}: {n} after {before}");
            }
            last[producer] = Some(n);
            delivery.lines += 1;
            delivery.bytes += line.len();
        }
        Ok(delivery)
    }

    /// Lines a producer writes between two exchanges with the heartbeat thread.
    const LINES_PER_HEARTBEAT: usize = 500;

    /// One case of [`writes_return_and_the_queue_stays_bounded_while_the_sink_is_stalled`]:
    /// the bound that fills, and the lines written while the sink is stalled,
    /// between all producers, with their length.
    struct StallCase {
        bound: &'static str,
        lines: usize,
        line_len: usize,
    }

    const STALL_CASES: [StallCase; 2] = [
        // `QUEUE_LINES` short lines hold far less than `QUEUE_BYTES`.
        StallCase {
            bound: "lines",
            lines: 3 * QUEUE_LINES,
            line_len: 32,
        },
        // Fewer than `QUEUE_LINES` lines of 4 KiB, three times `QUEUE_BYTES`.
        StallCase {
            bound: "bytes",
            lines: 3 * QUEUE_BYTES / 4_096,
            line_len: 4_096,
        },
    ];

    /// While the sink is stalled, holding the writer thread with its first
    /// batch, producers write many times what the queue holds, with the
    /// production limits, and every write returns before the sink takes a
    /// byte. Between writes, each producer also exchanges a ping with a
    /// heartbeat thread, as a tokio worker runs other tasks between the events
    /// and spans of one task; those exchanges all complete while the queue is
    /// full. The queue holds exactly as many lines as its bound allows, and
    /// counts every other line as dropped. Once the sink is released, every
    /// line it receives is JSON: each queued line whole and in its producer's
    /// order, then drop reports that account for every missing line and byte.
    #[test]
    fn writes_return_and_the_queue_stays_bounded_while_the_sink_is_stalled() -> TestResult {
        for case in &STALL_CASES {
            for producers in [1, 4] {
                stall(case, producers)
                    .map_err(|err| format!("{} bound, {producers} producers: {err}", case.bound))?;
            }
        }
        Ok(())
    }

    /// Has `producers` threads write `lines_each` [`numbered_line`]s of
    /// `line_len` bytes each to `writer`, producer `p` numbering its lines from
    /// `p * lines_each` up, and returns once every write has. After every
    /// [`LINES_PER_HEARTBEAT`] lines, a producer also exchanges a ping with a
    /// heartbeat thread; the result is the number of exchanges.
    ///
    /// # Errors
    ///
    /// The first failed write, or an exchange not answered within
    /// [`GATE_TIMEOUT`].
    fn produce_with_heartbeats(
        writer: &QueuedWriter<EnteredSink>,
        producers: usize,
        lines_each: usize,
        line_len: usize,
    ) -> Result<usize, Box<dyn Error>> {
        let heartbeats = AtomicUsize::new(0);
        // Each message is the sender on which the heartbeat thread answers.
        let (to_heartbeat, heartbeat_inbox) = mpsc::channel::<mpsc::Sender<()>>();
        let produce = |producer: usize, to_heartbeat: mpsc::Sender<mpsc::Sender<()>>| {
            let (answer, answers) = mpsc::channel();
            for n in 0..lines_each {
                let line = numbered_line(producer * lines_each + n, line_len);
                write_line(writer, line.as_bytes())?;
                if (n + 1) % LINES_PER_HEARTBEAT == 0 {
                    to_heartbeat
                        .send(answer.clone())
                        .map_err(io::Error::other)?;
                    answers
                        .recv_timeout(GATE_TIMEOUT)
                        .map_err(io::Error::other)?;
                }
            }
            io::Result::Ok(())
        };
        thread::scope(|scope| -> TestResult {
            let heartbeat = scope.spawn(|| {
                for answer in heartbeat_inbox {
                    heartbeats.fetch_add(1, Ordering::Relaxed);
                    // A producer that stopped waiting reports that itself.
                    let _ = answer.send(());
                }
            });
            let handles: Vec<_> = (0..producers)
                .map(|producer| {
                    let to_heartbeat = to_heartbeat.clone();
                    scope.spawn(move || produce(producer, to_heartbeat))
                })
                .collect();
            drop(to_heartbeat);
            for handle in handles {
                handle.join().map_err(|_| "a producer panicked")??;
            }
            heartbeat
                .join()
                .map_err(|_| "the heartbeat thread panicked")?;
            Ok(())
        })?;
        Ok(heartbeats.into_inner())
    }

    /// One run of [`writes_return_and_the_queue_stays_bounded_while_the_sink_is_stalled`].
    fn stall(case: &StallCase, producers: usize) -> TestResult {
        let line_len = case.line_len;
        let lines_each = case.lines / producers;
        let written = producers * lines_each;
        // The queue fills to its bound behind the first batch and stays there.
        let queued = QUEUE_LINES.min(QUEUE_BYTES / line_len);
        assert!(queued < written, "the case fills no bound");

        let (open_gate, gate) = mpsc::channel();
        let (entered, first_write) = mpsc::channel();
        let recording = Arc::new(Mutex::new(Recording::default()));
        let writer = QueuedWriter::start(EnteredSink {
            sink: RecordingSink {
                recording: Arc::clone(&recording),
                gate: Some(gate),
            },
            entered: Some(entered),
        })?;
        // The first batch is this one line, numbered after every producer's:
        // the writer thread holds it at the gate before any producer starts.
        let first = numbered_line(written, line_len);
        write_line(&writer, first.as_bytes())?;
        first_write.recv_timeout(GATE_TIMEOUT)?;

        let heartbeats = produce_with_heartbeats(&writer, producers, lines_each, line_len)?;
        // Every exchange with the heartbeat thread completed, and the writer
        // thread is still held at the gate with its first batch, so every
        // write returned without the sink taking a byte.
        assert_eq!(heartbeats, producers * (lines_each / LINES_PER_HEARTBEAT));
        assert!(
            lock(&recording)?.calls.is_empty(),
            "a write waited on the sink"
        );
        let counters = &writer.0.counters;
        let queued_bytes = counters.queued_bytes.load(Ordering::Relaxed);
        assert_eq!(queued_bytes, queued * line_len);
        assert!(queued_bytes <= QUEUE_BYTES, "{queued_bytes} bytes queued");
        let dropped = written - queued;
        assert_eq!(counters.dropped_lines.load(Ordering::Relaxed), dropped);
        assert_eq!(
            counters.dropped_bytes.load(Ordering::Relaxed),
            dropped * line_len
        );

        drop(open_gate);
        writer.0.close();
        for counter in [
            &counters.queued_bytes,
            &counters.dropped_lines,
            &counters.dropped_bytes,
        ] {
            assert_eq!(counter.load(Ordering::Relaxed), 0, "a count left over");
        }
        let recording = lock(&recording)?;
        assert!(
            recording.bytes.starts_with(first.as_bytes()),
            "the first batch"
        );
        let delivery = read_delivery(&recording.bytes, lines_each, line_len)?;
        // Every line the queue took arrived, and no more than it can hold
        // behind the first batch.
        assert!(delivery.lines <= 1 + QUEUE_LINES, "{delivery:?}");
        assert!(delivery.bytes <= first.len() + QUEUE_BYTES, "{delivery:?}");
        assert!(delivery.reports >= 1, "{delivery:?}");
        assert_eq!(
            delivery,
            Delivery {
                lines: 1 + queued,
                bytes: (1 + queued) * line_len,
                reports: delivery.reports,
                dropped_lines: 1 + written - delivery.lines,
                dropped_bytes: (1 + written) * line_len - delivery.bytes,
            }
        );
        Ok(())
    }

    /// Waits until `recording` holds at least `calls` calls, for at most
    /// [`GATE_TIMEOUT`].
    fn wait_for_calls(recording: &Mutex<Recording>, calls: usize) -> TestResult {
        let start = Instant::now();
        while lock(recording)?.calls.len() < calls {
            if start.elapsed() > GATE_TIMEOUT {
                return Err(format!("the sink did not receive {calls} calls").into());
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// A line of [`MAX_LINE_BYTES`] reaches the sink, and a longer one is
    /// dropped whole. Its report follows at once, though the queue is idle,
    /// and has the keys and timestamp of the JSON formatter's event lines.
    #[test]
    fn a_line_above_the_largest_is_dropped_whole_and_reported_at_once() -> TestResult {
        let (writer, recording) = recording_queue(None)?;
        let largest = numbered_line(0, MAX_LINE_BYTES);
        write_line(&writer, largest.as_bytes())?;
        // Written and flushed: the writer thread waits on an empty queue.
        wait_for_calls(&recording, 2)?;
        write_line(&writer, numbered_line(1, MAX_LINE_BYTES + 1).as_bytes())?;
        // Only the wake-up of the first drop has the report written before
        // the queue is closed.
        wait_for_calls(&recording, 4)?;
        writer.0.close();

        let recording = lock(&recording)?;
        let (written, report) = recording
            .bytes
            .split_at_checked(largest.len())
            .ok_or("the largest line did not arrive")?;
        assert_eq!(written, largest.as_bytes());
        assert_eq!(
            recording.calls,
            [
                SinkCall::Write(largest.len()),
                SinkCall::Flush,
                SinkCall::Write(report.len()),
                SinkCall::Flush
            ]
        );
        let text = std::str::from_utf8(report)?;
        let report: Json = serde_json::from_str(
            text.strip_suffix('\n')
                .filter(|line| !line.contains('\n'))
                .ok_or_else(|| format!("not one line: {text}"))?,
        )?;
        assert_eq!(drop_report_counts(&report)?, (1, MAX_LINE_BYTES + 1));

        // An event line outside every span has the report's keys and the same
        // timestamp shape.
        let capture = Capture::default();
        let (subscriber, guard) = subscriber_with_writer("test-svc", "info", capture.clone())?;
        tracing::subscriber::with_default(subscriber, || tracing::warn!("an event line"));
        guard.shutdown()?;
        let lines = capture.json_lines()?;
        let [event] = &lines[..] else {
            return Err(format!("expected exactly one event line: {lines:?}").into());
        };
        assert_eq!(sorted_keys(event), sorted_keys(&report), "{event}");
        let timestamp = event["timestamp"].as_str().unwrap_or_default();
        assert!(is_formatter_timestamp(timestamp), "{event}");
        Ok(())
    }

    #[test]
    fn close_writes_every_queued_line_in_order_one_flushed_batch_at_a_time() -> TestResult {
        const LINE_LEN: usize = 100;
        let (open_gate, gate) = mpsc::channel();
        let (writer, recording) = recording_queue(Some(gate))?;
        let lines: Vec<String> = (0..4_000).map(|n| numbered_line(n, LINE_LEN)).collect();
        for line in &lines {
            write_line(&writer, line.as_bytes())?;
        }
        drop(open_gate);
        writer.0.close();

        let recording = lock(&recording)?;
        let expected = lines.concat();
        assert_eq!(recording.bytes, expected.as_bytes());
        // Every batch is one write and one flush, and stops gathering lines
        // once it holds `BATCH_BYTES`. Every line but the first batch's was
        // queued when the gate opened, so the batches after it are full.
        let mut batches = 0;
        for pair in recording.calls.chunks(2) {
            let [SinkCall::Write(len), SinkCall::Flush] = pair else {
                return Err(format!("not a write and its flush: {pair:?}").into());
            };
            assert!(*len < BATCH_BYTES + LINE_LEN, "batch of {len} bytes");
            batches += 1;
        }
        assert!(batches >= 2, "{batches} batches");
        assert!(
            batches <= 2 + expected.len().div_ceil(BATCH_BYTES),
            "{batches} batches: lines were not gathered"
        );
        Ok(())
    }

    #[test]
    fn a_write_after_close_reaches_the_sink_before_it_returns() -> TestResult {
        let (writer, recording) = recording_queue(None)?;
        assert_eq!(writer.make_writer().write(b"")?, 0, "an empty write");
        write_line(&writer, b"before\n")?;
        writer.0.close();
        writer.0.close();
        assert_eq!(writer.make_writer().write(b"")?, 0, "an empty write");
        write_line(&writer, b"after\n")?;

        let recording = lock(&recording)?;
        assert_eq!(recording.bytes, b"before\nafter\n");
        assert_eq!(
            recording.calls,
            [
                SinkCall::Write(7),
                SinkCall::Flush,
                SinkCall::Write(6),
                SinkCall::Flush
            ]
        );
        Ok(())
    }

    #[test]
    fn concurrent_writers_keep_every_line_whole() -> TestResult {
        const THREADS: usize = 8;
        const LINES: usize = 500;
        const LINE_LEN: usize = 300;
        let (writer, recording) = recording_queue(None)?;
        let writes_of = |writer_no: usize| -> io::Result<()> {
            for n in 0..LINES {
                write_line(
                    &writer,
                    numbered_line(writer_no * LINES + n, LINE_LEN).as_bytes(),
                )?;
            }
            Ok(())
        };
        thread::scope(|scope| -> TestResult {
            let handles: Vec<_> = (0..THREADS)
                .map(|writer_no| scope.spawn(move || writes_of(writer_no)))
                .collect();
            for handle in handles {
                handle.join().map_err(|_| "a writing thread panicked")??;
            }
            Ok(())
        })?;
        writer.0.close();

        assert_whole_lines_in_order(&lock(&recording)?.bytes, THREADS, LINES, LINE_LEN)
    }

    /// A [`RecordingSink`] whose first write fails, unrecorded, and reports the
    /// failure through `failed`.
    struct FailingOnceSink {
        sink: RecordingSink,
        failed: Option<mpsc::Sender<()>>,
    }

    impl Write for FailingOnceSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(failed) = self.failed.take() {
                // The test may have stopped waiting; the failure stands either way.
                let _ = failed.send(());
                return Err(io::Error::other("failing test sink"));
            }
            self.sink.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.sink.flush()
        }
    }

    #[test]
    fn a_failed_batch_is_dropped_and_the_next_one_written() -> TestResult {
        let recording = Arc::new(Mutex::new(Recording::default()));
        let (failed, first_failed) = mpsc::channel();
        let writer = QueuedWriter::start(FailingOnceSink {
            sink: RecordingSink {
                recording: Arc::clone(&recording),
                gate: None,
            },
            failed: Some(failed),
        })?;
        write_line(&writer, b"dropped\n")?;
        first_failed.recv_timeout(GATE_TIMEOUT)?;
        write_line(&writer, b"written\n")?;
        writer.0.close();

        let recording = lock(&recording)?;
        assert_eq!(recording.bytes, b"written\n");
        assert_eq!(recording.calls, [SinkCall::Write(8), SinkCall::Flush]);
        Ok(())
    }

    /// A sink whose every write panics.
    struct PanickingSink;

    impl Write for PanickingSink {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            panic!("panicking test sink");
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_panicked_writer_thread_fails_later_writes_and_close_still_returns() -> TestResult {
        let writer = QueuedWriter::start(PanickingSink)?;
        write_line(&writer, b"taken by the panicking sink\n")?;
        writer.0.close();
        let Err(err) = write_line(&writer, b"after close\n") else {
            return Err("a write without a sink succeeded".into());
        };
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe, "{err}");
        Ok(())
    }

    #[test]
    fn a_line_the_queue_would_drop_fails_once_the_writer_thread_has_panicked() -> TestResult {
        let writer = QueuedWriter::start(PanickingSink)?;
        write_line(&writer, b"taken by the panicking sink\n")?;
        let start = Instant::now();
        loop {
            let open = writer.0.open.read().map_err(|_| "queue lock poisoned")?;
            let queue = open.as_ref().ok_or("the queue closed itself")?;
            if queue.thread.is_finished() {
                break;
            }
            drop(open);
            if start.elapsed() > GATE_TIMEOUT {
                return Err("the writer thread did not panic".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
        // Above `MAX_LINE_BYTES`, so the queue would drop it without sending it.
        let Err(err) = write_line(&writer, numbered_line(0, MAX_LINE_BYTES + 1).as_bytes()) else {
            return Err("a write to a panicked writer thread succeeded".into());
        };
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe, "{err}");
        writer.0.close();
        Ok(())
    }

    /// How a test ends a guard that owns a queue.
    #[derive(Clone, Copy, Debug)]
    enum Ending {
        Shutdown,
        Drop,
        /// Drop after the provider was shut down through another handle, so
        /// that the guard writes the `telemetry shutdown failed` line.
        FailedDrop,
    }

    #[test]
    fn a_queued_guard_writes_every_line_before_it_is_gone() -> TestResult {
        const SPANS: usize = 200;
        for ending in [Ending::Shutdown, Ending::Drop, Ending::FailedDrop] {
            let capture = Capture::default();
            let (subscriber, guard) = subscriber_with_queue("test-svc", "info", capture.clone())?;
            tracing::subscriber::with_default(subscriber, || {
                for n in 0..SPANS {
                    tracing::info_span!("job", n).in_scope(|| tracing::info!(n, "posted"));
                }
            });
            match ending {
                Ending::Shutdown => guard.shutdown()?,
                Ending::Drop => drop(guard),
                Ending::FailedDrop => {
                    guard
                        .provider
                        .clone()
                        .ok_or("guard without a provider")?
                        .shutdown()?;
                    drop(guard);
                }
            }

            let lines = capture.json_lines()?;
            let spans = span_lines(&lines);
            assert_eq!(spans.len(), SPANS, "{ending:?}");
            assert!(spans.iter().all(|line| line["name"] == "job"), "{ending:?}");
            let posted = lines
                .iter()
                .filter(|line| line["fields"]["message"] == "posted")
                .count();
            assert_eq!(posted, SPANS, "{ending:?}");
            let failure = lines
                .last()
                .filter(|line| line["fields"]["message"] == "telemetry shutdown failed");
            let failures = usize::from(matches!(ending, Ending::FailedDrop));
            assert_eq!(usize::from(failure.is_some()), failures, "{ending:?}");
            assert_eq!(lines.len(), 2 * SPANS + failures, "{ending:?}: {lines:?}");
        }
        Ok(())
    }
}
