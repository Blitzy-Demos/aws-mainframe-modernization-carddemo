//! In-memory capture of the JSON tracing output, the instrument of the Cardholder Data Redaction
//! tests: they run endpoints and consumers under a capture and assert that no fixture card
//! number, SSN or submitted password appears in anything it recorded.
//!
//! A capture writes through [`runtime::telemetry::json_layer`], so each line has exactly the
//! shape of the standard output that `runtime::telemetry::init` writes. Two settings widen it so
//! that nothing escapes the scan:
//!
//! - Every level down to `TRACE` is recorded, whatever `RUST_LOG` says, for every target but the
//!   [`runtime::telemetry::SILENCED_TARGETS`] (the AMQP client `lapin`, its `amq_protocol`
//!   crates and `pinky_swear`, and the I/O reactor crates `polling` and `async_io`).
//!   [`runtime::telemetry::dependency_filter`] keeps those off here exactly as it does on
//!   standard output, so the capture records only what standard output can carry.
//! - Every span writes one line when it opens and one when it closes, each with the span's
//!   recorded fields. A span therefore shows its fields even when no event fires inside it,
//!   which is what the `OpenTelemetry` span exporter would print for it.
//!
//! The buffer is kept in memory only, and is never filtered, redacted or truncated. Alerts are
//! the lines whose target is [`runtime::telemetry::ALERT_TARGET`], selected with
//! [`TraceCapture::lines_with_target`].
//!
//! No event escapes a capture because another thread reached its callsite first. `tracing`
//! caches each callsite's interest when a thread first reaches it, and while it knows of a
//! single live dispatcher it asks only the reaching thread's default subscriber. A test thread
//! without a capture would then cache "never" for a callsite that a capture on another thread
//! records, and that capture would miss the callsite's events from then on. The first capture
//! therefore registers a dispatcher that records nothing and lives as long as the process (see
//! [`capture`]), so every capture is one of at least two live dispatchers and each callsite's
//! interest is taken from all of them.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use runtime::telemetry::{dependency_filter, json_layer};
use tracing::Dispatch;
use tracing::dispatcher::{self, DefaultGuard};
use tracing::subscriber::NoSubscriber;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt as _;

/// The captured bytes, shared by the capture and every writer its layer makes.
type Buffer = Arc<Mutex<Vec<u8>>>;

/// The dispatcher that keeps every capture one of at least two live dispatchers, registered by
/// the first [`capture`] of the process before its own and never dropped.
///
/// `tracing` takes a newly reached callsite's interest from the reaching thread's default
/// subscriber alone whenever the last dispatcher it registered was the only live one. With this
/// dispatcher alive, every capture's registration counts two or more, so a callsite's interest
/// combines every live dispatcher's: "sometimes" while any capture that records it is live,
/// never "never". Its subscriber records nothing and sets no level ceiling, so it neither adds
/// lines to a capture nor lowers the maximum level below the `TRACE` a capture enables.
static INTEREST_SENTINEL: OnceLock<Dispatch> = OnceLock::new();

/// Locks `buffer`. A poisoned lock is taken over rather than reported: the lock is held only to
/// append, copy or clear whole slices, so its bytes stay consistent, and no accessor panics.
fn lock(buffer: &Buffer) -> MutexGuard<'_, Vec<u8>> {
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The writer for one formatted line. The fmt layer hands it each line in a single `write_all`,
/// and `write` appends the whole slice, so lines from concurrent threads never interleave.
struct BufferWriter(Buffer);

impl io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Makes a [`BufferWriter`] over the shared buffer for every line the layer writes.
#[derive(Clone)]
struct BufferMakeWriter(Buffer);

impl<'a> MakeWriter<'a> for BufferMakeWriter {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BufferWriter(Arc::clone(&self.0))
    }
}

/// A running capture of the current thread's tracing output, made by [`capture`].
///
/// Dropping it ends the capture and restores the thread's previous default subscriber.
pub struct TraceCapture {
    buffer: Buffer,
    dispatch: Dispatch,
    // Held only for its drop, which restores the thread's previous default subscriber. The
    // leading underscore is how rustc exempts a field that is never read from `dead_code`.
    _guard: DefaultGuard,
}

// Hand-written so that a failing test printing the capture never prints what it recorded.
impl fmt::Debug for TraceCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraceCapture")
            .field("captured_bytes", &lock(&self.buffer).len())
            .finish_non_exhaustive()
    }
}

/// Starts capturing the current thread's tracing output, in the standard output JSON format with
/// every level and span lifecycle lines added (see the module documentation).
///
/// The capture becomes the thread's default subscriber, as `tracing::dispatcher::set_default`
/// makes it; no global subscriber is installed, so tests running in parallel threads of one test
/// binary each capture only their own output. It therefore sees only what runs on this thread:
///
/// - Async tests use the current-thread `#[tokio::test]` flavour, under which every task the test
///   spawns runs on the test's thread. Work on any other thread is captured only when it carries
///   [`TraceCapture::dispatch`]: a future through
///   `tracing::instrument::WithSubscriber::with_subscriber`, a closure inside
///   `tracing::dispatcher::with_default`.
/// - Drop the capture on the thread that started it, since the drop restores the default of the
///   thread it runs on.
/// - A capture started while another is active takes over the thread until it is dropped.
/// - Child processes, such as `carddemo-mq` or `carddemo-batch` run as binaries, write their own
///   standard output, which this capture does not see. Tests read the child's standard output
///   instead, which has the same JSON shape.
///
/// Before its own dispatcher, the capture makes sure a process-lifetime dispatcher that records
/// nothing is registered, so that a callsite first reached by a thread without a capture still
/// reaches this one (see the module documentation).
#[must_use = "the capture ends when it is dropped; bind it for as long as the code under test runs"]
pub fn capture() -> TraceCapture {
    INTEREST_SENTINEL.get_or_init(|| Dispatch::new(NoSubscriber::new()));
    let buffer = Buffer::default();
    let layer = json_layer(BufferMakeWriter(Arc::clone(&buffer)))
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_filter(LevelFilter::TRACE);
    let dispatch = Dispatch::new(
        tracing_subscriber::registry()
            .with(dependency_filter())
            .with(layer),
    );
    let guard = dispatcher::set_default(&dispatch);
    TraceCapture {
        buffer,
        dispatch,
        _guard: guard,
    }
}

impl TraceCapture {
    /// The dispatcher of this capture, for work that runs on another thread: attach it to a
    /// future with `tracing::instrument::WithSubscriber::with_subscriber`, or run a closure under
    /// `tracing::dispatcher::with_default`.
    #[must_use]
    pub fn dispatch(&self) -> Dispatch {
        self.dispatch.clone()
    }

    /// Everything captured so far, as UTF-8 with any invalid byte replaced by `U+FFFD`.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&lock(&self.buffer)).into_owned()
    }

    /// The captured lines, in the order they were written, without empty lines. Each line is one
    /// JSON object: an event or a span opening or closing.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Whether `needle` occurs anywhere in the captured text, field values and span fields
    /// included.
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }

    /// The captured lines whose `target` member is `target`, such as
    /// [`runtime::telemetry::ALERT_TARGET`] for the alert events.
    ///
    /// The match is textual on `"target":"<target>"`, which the JSON formatter writes without
    /// spaces. A field value quoting that text cannot match, because its quotes are escaped.
    #[must_use]
    pub fn lines_with_target(&self, target: &str) -> Vec<String> {
        let member = format!(r#""target":"{target}""#);
        self.lines()
            .into_iter()
            .filter(|line| line.contains(&member))
            .collect()
    }

    /// Discards everything captured so far, so that a test can isolate the output of one request
    /// or message. The capture keeps running.
    pub fn clear(&self) {
        lock(&self.buffer).clear();
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use runtime::telemetry::{
        ALERT_TARGET, ScreenedText, message_span, record_response, request_span,
    };
    use tracing::instrument::WithSubscriber as _;
    use tracing::level_filters::LevelFilter;

    use super::capture;

    #[test]
    fn spans_write_their_fields_when_they_open_and_when_they_close() {
        let capture = capture();
        let span = request_span("GET", "/api/v1/cards");
        record_response(&span, 200, Duration::from_millis(3));
        drop(span);

        let lines = capture.lines();
        let [opened, closed] = lines.as_slice() else {
            panic!("expected one open and one close line, got {lines:?}");
        };
        for expected in [
            r#""message":"new""#,
            r#""name":"http.request""#,
            r#""method":"GET""#,
            r#""route":"/api/v1/cards""#,
        ] {
            assert!(
                opened.contains(expected),
                "{expected} missing from {opened}"
            );
        }
        assert!(!opened.contains(r#""status""#), "{opened}");
        for expected in [
            r#""message":"close""#,
            r#""route":"/api/v1/cards""#,
            r#""status":200"#,
            r#""latency_ms":3"#,
        ] {
            assert!(
                closed.contains(expected),
                "{expected} missing from {closed}"
            );
        }
    }

    #[test]
    fn a_span_opened_before_its_fields_are_recorded_shows_them_on_close() {
        let capture = capture();
        drop(message_span(
            "CARD.DEMO.REQUEST.PAUTH",
            Some(ScreenedText::new("m-7")),
            Some(ScreenedText::new("c-7")),
        ));

        let lines = capture.lines();
        let [_, closed] = lines.as_slice() else {
            panic!("expected one open and one close line, got {lines:?}");
        };
        for expected in [
            r#""queue":"CARD.DEMO.REQUEST.PAUTH""#,
            r#""message_id":"***""#,
            r#""correlation_id":"***""#,
        ] {
            assert!(
                closed.contains(expected),
                "{expected} missing from {closed}"
            );
        }
    }

    #[test]
    fn every_level_is_recorded() {
        let capture = capture();
        tracing::trace!(depth = "deepest", "trace probe");
        tracing::debug!("debug probe");

        let lines = capture.lines();
        let [trace, debug] = lines.as_slice() else {
            panic!("expected two event lines, got {lines:?}");
        };
        assert!(trace.contains(r#""level":"TRACE""#), "{trace}");
        assert!(trace.contains(r#""depth":"deepest""#), "{trace}");
        assert!(debug.contains(r#""level":"DEBUG""#), "{debug}");
    }

    #[test]
    fn silenced_dependency_targets_are_not_recorded() {
        let capture = capture();
        tracing::error!(target: "lapin::channels", "dependency probe");
        tracing::trace!(target: "messaging::workers", "application probe");

        let lines = capture.lines();
        let [line] = lines.as_slice() else {
            panic!("expected the application event only, got {lines:?}");
        };
        assert!(line.contains("application probe"), "{line}");
        assert!(!capture.contains("dependency probe"), "{}", capture.text());
    }

    #[test]
    fn lines_with_target_selects_the_alert_events_only() {
        let capture = capture();
        tracing::info!("routine");
        tracing::error!(target: ALERT_TARGET, alert = "CP00_CARD_UNPARSEABLE", "alert probe");
        tracing::info!(note = r#""target":"carddemo::alert""#, "quoting the member");

        let alerts = capture.lines_with_target(ALERT_TARGET);
        let [alert] = alerts.as_slice() else {
            panic!("expected one alert line, got {alerts:?}");
        };
        assert!(alert.contains(r#""level":"ERROR""#), "{alert}");
        assert!(
            alert.contains(r#""alert":"CP00_CARD_UNPARSEABLE""#),
            "{alert}"
        );
        assert_eq!(capture.lines().len(), 3);
        assert!(capture.lines_with_target("carddemo::absent").is_empty());
    }

    #[test]
    fn clear_discards_the_output_so_far_and_keeps_capturing() {
        let capture = capture();
        tracing::info!(queue = "CARD.DEMO.REQUEST.ACCT", "before clear");
        assert!(capture.contains("CARD.DEMO.REQUEST.ACCT"));

        capture.clear();
        assert!(capture.text().is_empty());
        assert!(capture.lines().is_empty());
        assert!(!capture.contains("CARD.DEMO.REQUEST.ACCT"));

        tracing::info!("after clear");
        let lines = capture.lines();
        let [line] = lines.as_slice() else {
            panic!("expected one line after clear, got {lines:?}");
        };
        assert!(line.contains("after clear"), "{line}");
    }

    #[test]
    fn other_threads_are_captured_only_when_they_carry_the_dispatch() {
        let capture = capture();
        let outside = thread::spawn(|| tracing::info!("outside probe"));
        assert!(outside.join().is_ok());

        let dispatch = capture.dispatch();
        let carried = thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || tracing::info!("carried probe"));
        });
        assert!(carried.join().is_ok());

        assert!(!capture.contains("outside probe"), "{}", capture.text());
        assert!(capture.contains("carried probe"), "{}", capture.text());
    }

    // The one callsite of the test below: no other code reaches it, so its interest is first
    // cached by whichever thread of that test reaches it first.
    fn first_reached_without_a_capture() {
        tracing::warn!("first reached without a capture");
    }

    #[test]
    fn a_callsite_first_reached_by_a_thread_without_a_capture_is_still_captured() {
        let capture = capture();
        assert_eq!(LevelFilter::current(), LevelFilter::TRACE);

        let uncaptured = thread::spawn(first_reached_without_a_capture);
        assert!(uncaptured.join().is_ok());
        first_reached_without_a_capture();

        let lines = capture.lines();
        let [line] = lines.as_slice() else {
            panic!("expected this thread's event only, got {lines:?}");
        };
        assert!(line.contains(r#""level":"WARN""#), "{line}");
        assert!(line.contains("first reached without a capture"), "{line}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn futures_on_worker_threads_are_captured_with_the_dispatch() {
        let capture = capture();
        let task = async { tracing::info!("worker probe") }.with_subscriber(capture.dispatch());
        assert!(tokio::spawn(task).await.is_ok());
        assert!(capture.contains("worker probe"), "{}", capture.text());
    }

    #[tokio::test]
    async fn current_thread_tasks_are_captured() {
        let capture = capture();
        assert!(
            tokio::spawn(async { tracing::info!("task probe") })
                .await
                .is_ok()
        );
        assert!(capture.contains("task probe"), "{}", capture.text());
    }

    #[test]
    fn dropping_a_capture_restores_the_previous_default() {
        let outer = capture();
        {
            let inner = capture();
            tracing::info!("inner probe");
            assert!(inner.contains("inner probe"));
        }
        tracing::info!("outer probe");
        assert!(outer.contains("outer probe"), "{}", outer.text());
        assert!(!outer.contains("inner probe"), "{}", outer.text());
    }

    #[test]
    fn debug_never_prints_the_captured_text() {
        let capture = capture();
        tracing::info!(value = "captured-secret-probe", "probe");
        let debug = format!("{capture:?}");
        assert!(debug.starts_with("TraceCapture {"), "{debug}");
        assert!(debug.contains("captured_bytes"), "{debug}");
        assert!(!debug.contains("captured-secret-probe"), "{debug}");
    }
}
