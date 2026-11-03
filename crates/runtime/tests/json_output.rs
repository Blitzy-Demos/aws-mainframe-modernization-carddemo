//! Telemetry output contract: standard output carries JSON lines only.
//!
//! Every finished span is one `"type":"span"` line from the JSON span exporter,
//! every log event is one line from the `tracing-subscriber` JSON formatter, and
//! an event below the filter level is not written at all. The scenario emits the
//! three write-span families the binaries produce, with the names and fields the
//! binaries give them:
//!
//! - an API `request` span (`method`, `route`, `version`) holding the CAUP
//!   handler span `update_account`, inside which sqlx writes a `sqlx::query`
//!   statement event, as enabled by `sqlx::query=debug`;
//! - a batch `job` span with its reject and halt events;
//! - an MQ `message` span.
//!
//! Every event line has a string `level`, a string `target` and a `fields`
//! object. An application event also has a `fields.message`; a dependency's
//! event, such as sqlx's statement event, may carry structured fields only. An
//! event emitted inside a span carries that span's `trace_id` and `span_id`,
//! equal to the IDs of the span's line, and an event outside every span carries
//! neither.
//!
//! The subscriber comes from [`subscriber_with_writer`] over an in-memory capture
//! and is installed only for the scenario's closure, so no global state is set
//! and nothing reaches the real standard output.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use carddemo_runtime::telemetry::{TelemetryError, subscriber_with_writer};
use serde_json::Value;
use tracing_subscriber::fmt::MakeWriter;

const SERVICE: &str = "carddemo-test";

/// The binaries' default filter, `info`, plus sqlx's statement events, which
/// sqlx writes at DEBUG.
const DIRECTIVES: &str = "info,sqlx::query=debug";

/// Target of this test's own events, which stand in for the binaries' code.
const APP_TARGET: &str = module_path!();
/// Target of sqlx's statement events.
const SQLX_QUERY_TARGET: &str = "sqlx::query";

const START_EVENT: &str = "telemetry json output test start";
const CAUP_EVENT: &str = "account update committed";
const REJECT_EVENT: &str = "daily transaction rejected";
const HALT_EVENT: &str = "posting halted by the reward engine";
const DEBUG_EVENT: &str = "debug event must be filtered out";

/// The API's request span, from its `TraceLayer`.
const REQUEST_SPAN: &str = "request";
/// The CAUP handler span, named after the API's handler function.
const HANDLER_SPAN: &str = "update_account";
const JOB_SPAN: &str = "job";
const MESSAGE_SPAN: &str = "message";

/// The request span's `version`: the `Debug` text of `http::Version::HTTP_11`.
const HTTP_VERSION: &str = "HTTP/1.1";

/// sqlx's `summary` of [`SQLX_STATEMENT`]: its first four words, then ` …`.
const SQLX_SUMMARY: &str = "UPDATE accounts SET acct_active_status …";
/// sqlx's `db.statement` for a statement longer than its summary: the SQL
/// between two leading newlines and a trailing one.
const SQLX_STATEMENT: &str =
    "\n\nUPDATE accounts SET acct_active_status = $2 WHERE account_id = $1\n";

const ACCOUNT_ID: &str = "00000000001";
/// `TRAN-ID` of record 1 of `app/data/ASCII/dailytran.txt`.
const TRAN_ID: &str = "0000000000683580";
/// A CP00 reply `message_id`: 48 lowercase hexadecimal characters.
const MESSAGE_ID: &str = "17f0c2a9b3d4e5f600001a2b00000001c0ffee00deadbeef";
const CORRELATION_ID: &str = "cp00-correlation-0001";

/// The `CCPAUERY` `ERR-LOCATION` `COPAUA0C` sets when the reply put fails
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L768].
const REPLY_PUT_FAILED: &str = "M004";

/// In-memory stand-in for standard output, shared by the JSON formatter and the
/// span exporter.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn contents(&self) -> String {
        let bytes = self.0.lock().expect("capture buffer poisoned").clone();
        String::from_utf8(bytes).expect("telemetry output is UTF-8")
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

/// Stand-in for the `http::Version` the API records, `Debug`-formatted, as the
/// request span's `version`.
struct Http11;

impl fmt::Debug for Http11 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(HTTP_VERSION)
    }
}

/// The statement event sqlx writes once a query has run: structured fields
/// only, without a message, at DEBUG, its default level for statements.
fn emit_sqlx_query() {
    let elapsed = Duration::from_micros(412);
    tracing::debug!(
        target: SQLX_QUERY_TARGET,
        summary = SQLX_SUMMARY,
        db.statement = SQLX_STATEMENT,
        rows_affected = 1_u64,
        rows_returned = 0_u64,
        ?elapsed,
        elapsed_secs = elapsed.as_secs_f64(),
    );
}

/// API family: the request span and, inside it, the CAUP handler span. The
/// handler records the account key it validated, runs its statement and, once
/// the write has committed, records `outcome` before the span closes.
fn emit_api_request() {
    let request = tracing::info_span!(
        REQUEST_SPAN,
        method = "PUT",
        route = "/accounts/{id}",
        version = ?Http11,
    );
    request.in_scope(|| {
        let handler = tracing::info_span!(
            HANDLER_SPAN,
            transaction = "CAUP",
            user_id = "ADMIN001",
            account_id = tracing::field::Empty,
            outcome = tracing::field::Empty,
        );
        handler.in_scope(|| {
            handler.record("account_id", ACCOUNT_ID);
            emit_sqlx_query();
            tracing::info!("{CAUP_EVENT}");
        });
        handler.record("outcome", "committed");
    });
}

/// Batch family: a POSTTRAN job span with its counts and the rejected-records
/// exit code 4, one reject event (reason 102, 'OVERLIMIT TRANSACTION'
/// [app/cbl/CBTRN02C.cbl:L410-411]) and one halt event.
fn emit_batch_job() {
    let job = tracing::info_span!(
        JOB_SPAN,
        job = "posttran",
        read = 300,
        posted = 262,
        rejected = 38,
        exit_code = 4
    );
    job.in_scope(|| {
        tracing::warn!(reason = 102, tran_id = TRAN_ID, "{REJECT_EVENT}");
        tracing::error!(record = 1, tran_id = TRAN_ID, "{HALT_EVENT}");
    });
}

/// Messaging family: a CP00 message span whose outcome, error record and
/// `CCPAUERY` location are recorded after the reply was not confirmed.
fn emit_mq_message() {
    let message = tracing::info_span!(
        MESSAGE_SPAN,
        flow = "CP00",
        message_id = MESSAGE_ID,
        correlation_id = CORRELATION_ID,
        outcome = tracing::field::Empty,
        error_record = tracing::field::Empty,
        location = tracing::field::Empty,
    );
    message.record("outcome", "rejected");
    message.record("error_record", "logged");
    message.record("location", REPLY_PUT_FAILED);
}

/// Parses every line of `output`. The output must end with a newline, and every
/// line before it, a blank one included, must be a JSON object.
fn parse_lines(output: &str) -> Vec<Value> {
    let body = output
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("output does not end with a newline: {output}"));
    body.split('\n')
        .map(|line| {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line is not JSON ({e}): {line}"));
            assert!(value.is_object(), "line is not a JSON object: {line}");
            value
        })
        .collect()
}

/// `true` when `text` is exactly `len` lowercase hexadecimal digits.
fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A Unix-nanosecond timestamp written as a JSON unsigned number or as a decimal
/// string.
fn nanos(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or_else(|| panic!("not a Unix-nanosecond count: {value}"))
}

/// The string value of attribute `key` of a span line.
fn attr_str<'a>(line: &'a Value, key: &str) -> Option<&'a str> {
    line["attributes"][key].as_str()
}

fn find_span<'a>(exported: &[&'a Value], name: &str) -> &'a Value {
    exported
        .iter()
        .copied()
        .find(|line| line["name"] == name)
        .unwrap_or_else(|| panic!("no span line named {name}: {exported:?}"))
}

fn find_event<'a>(events: &[&'a Value], message: &str) -> &'a Value {
    events
        .iter()
        .copied()
        .find(|line| line["fields"]["message"] == message)
        .unwrap_or_else(|| panic!("no event line with message {message:?}: {events:?}"))
}

/// The keys and value types every span line carries.
fn assert_span_shape(line: &Value) {
    assert_eq!(line["service"], SERVICE, "service of span line {line}");
    assert!(line["name"].is_string(), "span line without a name: {line}");

    let trace_id = line["trace_id"].as_str().unwrap_or_default();
    assert!(
        is_lower_hex(trace_id, 32) && trace_id.bytes().any(|b| b != b'0'),
        "trace_id is not 32 hex digits or is all zeros: {line}"
    );
    let own_id = line["span_id"].as_str().unwrap_or_default();
    assert!(
        is_lower_hex(own_id, 16),
        "span_id is not 16 hex digits: {line}"
    );
    let parent = line
        .get("parent_span_id")
        .unwrap_or_else(|| panic!("span line without parent_span_id: {line}"));
    assert!(
        parent.is_null() || parent.as_str().is_some_and(|id| is_lower_hex(id, 16)),
        "parent_span_id is neither null nor 16 hex digits: {line}"
    );

    let start = nanos(&line["start_time_unix_nano"]);
    let end = nanos(&line["end_time_unix_nano"]);
    assert!(0 < start && start <= end, "span times out of order: {line}");

    assert!(
        line["attributes"].is_object(),
        "attributes is not an object: {line}"
    );
    let code = line["status"]["code"].as_str().unwrap_or_default();
    assert!(
        ["unset", "ok", "error"].contains(&code),
        "status code is not unset, ok or error: {line}"
    );
}

fn assert_attributes(line: &Value, expected: &[(&str, &str)]) {
    for &(key, value) in expected {
        assert_eq!(
            attr_str(line, key),
            Some(value),
            "attribute {key} of span line {line}"
        );
    }
}

/// Exactly the four scenario spans, well formed, correctly parented and carrying
/// their attributes, `Span::record` values included.
fn assert_span_lines(exported: &[&Value]) {
    let mut names: Vec<&str> = exported
        .iter()
        .filter_map(|line| line["name"].as_str())
        .collect();
    names.sort_unstable();
    let mut expected = [REQUEST_SPAN, HANDLER_SPAN, JOB_SPAN, MESSAGE_SPAN];
    expected.sort_unstable();
    assert_eq!(names, expected, "span lines: {exported:?}");
    for &line in exported {
        assert_span_shape(line);
    }

    let request = find_span(exported, REQUEST_SPAN);
    let handler = find_span(exported, HANDLER_SPAN);
    let job = find_span(exported, JOB_SPAN);
    let message = find_span(exported, MESSAGE_SPAN);

    assert_eq!(handler["parent_span_id"], request["span_id"], "{handler}");
    assert_eq!(handler["trace_id"], request["trace_id"], "{handler}");
    for root in [request, job, message] {
        assert!(
            root["parent_span_id"].is_null(),
            "root span has a parent: {root}"
        );
    }
    assert_ne!(job["trace_id"], request["trace_id"], "{job}");
    assert_ne!(message["trace_id"], request["trace_id"], "{message}");
    assert_ne!(message["trace_id"], job["trace_id"], "{message}");

    assert_attributes(
        request,
        &[
            ("method", "PUT"),
            ("route", "/accounts/{id}"),
            ("version", HTTP_VERSION),
        ],
    );
    assert_attributes(
        handler,
        &[
            ("transaction", "CAUP"),
            ("user_id", "ADMIN001"),
            ("account_id", ACCOUNT_ID),
            ("outcome", "committed"),
        ],
    );
    assert_attributes(job, &[("job", "posttran")]);
    assert_attributes(
        message,
        &[
            ("flow", "CP00"),
            ("message_id", MESSAGE_ID),
            ("correlation_id", CORRELATION_ID),
            ("outcome", "rejected"),
            ("error_record", "logged"),
            ("location", REPLY_PUT_FAILED),
        ],
    );
}

/// Whether `target` is the application's: this test's own target, standing in
/// for the binaries' code, or one of its modules. Any other target, such as
/// `sqlx::query`, is a dependency's.
fn is_application_target(target: &str) -> bool {
    target
        .strip_prefix(APP_TARGET)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// `true` when event line `line` carries the `trace_id` and `span_id` of span
/// line `span`, both as strings.
fn joins(line: &Value, span: &Value) -> bool {
    line["trace_id"].is_string()
        && line["span_id"].is_string()
        && line["trace_id"] == span["trace_id"]
        && line["span_id"] == span["span_id"]
}

/// An event line emitted inside a span carries the `trace_id` and `span_id` of
/// the span line its `span` names; one emitted outside every span carries
/// neither key.
fn assert_joins_its_span(line: &Value, exported: &[&Value]) {
    if let Some(span) = line.get("span") {
        let name = span["name"]
            .as_str()
            .unwrap_or_else(|| panic!("event span without a name: {line}"));
        let owner = find_span(exported, name);
        assert!(
            joins(line, owner),
            "event line does not carry the IDs of its span line {owner}: {line}"
        );
    } else {
        assert!(
            line.get("trace_id").is_none() && line.get("span_id").is_none(),
            "event line outside every span carries a trace or span ID: {line}"
        );
    }
}

/// Every event line has a string `level`, a string `target` and a `fields`
/// object, an application event line a `fields.message` as well, and each joins
/// its span line by ID. The scenario's events are present at their levels, with
/// the context of the span they were emitted in, and sqlx's statement event has
/// structured fields and no message.
fn assert_event_lines(events: &[&Value], exported: &[&Value]) {
    for &line in events {
        assert!(
            line["level"].is_string(),
            "event line without a level: {line}"
        );
        let target = line["target"]
            .as_str()
            .unwrap_or_else(|| panic!("event line without a target: {line}"));
        assert!(
            line["fields"].is_object(),
            "event line without a fields object: {line}"
        );
        if is_application_target(target) {
            assert!(
                line["fields"]["message"].is_string(),
                "application event line without fields.message: {line}"
            );
        }
        assert_joins_its_span(line, exported);
    }
    let handler = find_span(exported, HANDLER_SPAN);
    let job = find_span(exported, JOB_SPAN);

    let start = find_event(events, START_EVENT);
    assert_eq!(start["level"], "INFO", "{start}");
    assert_eq!(start["target"], APP_TARGET, "{start}");
    assert!(start.get("span").is_none(), "{start}");
    assert!(start.get("trace_id").is_none(), "{start}");
    assert!(start.get("span_id").is_none(), "{start}");

    let caup = find_event(events, CAUP_EVENT);
    assert_eq!(caup["level"], "INFO", "{caup}");
    assert_eq!(caup["span"]["name"], HANDLER_SPAN, "{caup}");
    assert_eq!(caup["span"]["transaction"], "CAUP", "{caup}");
    let entered: Vec<&str> = caup["spans"]
        .as_array()
        .unwrap_or_else(|| panic!("CAUP event line without a span list: {caup}"))
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert_eq!(entered, [REQUEST_SPAN, HANDLER_SPAN], "{caup}");
    assert!(joins(caup, handler), "{caup}");

    let query = events
        .iter()
        .copied()
        .find(|line| line["target"] == SQLX_QUERY_TARGET)
        .unwrap_or_else(|| panic!("no {SQLX_QUERY_TARGET} event line: {events:?}"));
    assert_eq!(query["level"], "DEBUG", "{query}");
    assert!(query["fields"].get("message").is_none(), "{query}");
    assert_eq!(query["fields"]["summary"], SQLX_SUMMARY, "{query}");
    assert_eq!(query["fields"]["db.statement"], SQLX_STATEMENT, "{query}");
    assert_eq!(query["fields"]["rows_affected"], 1, "{query}");
    assert_eq!(query["fields"]["rows_returned"], 0, "{query}");
    assert!(query["fields"]["elapsed"].is_string(), "{query}");
    assert!(query["fields"]["elapsed_secs"].is_number(), "{query}");
    assert_eq!(query["span"]["name"], HANDLER_SPAN, "{query}");
    assert!(joins(query, handler), "{query}");

    let reject = find_event(events, REJECT_EVENT);
    assert_eq!(reject["level"], "WARN", "{reject}");
    assert_eq!(reject["fields"]["reason"], 102, "{reject}");
    assert_eq!(reject["fields"]["tran_id"], TRAN_ID, "{reject}");
    assert_eq!(reject["span"]["name"], JOB_SPAN, "{reject}");
    assert!(joins(reject, job), "{reject}");

    let halt = find_event(events, HALT_EVENT);
    assert_eq!(halt["level"], "ERROR", "{halt}");
    assert_eq!(halt["fields"]["tran_id"], TRAN_ID, "{halt}");
    assert_eq!(halt["span"]["name"], JOB_SPAN, "{halt}");
    assert!(joins(halt, job), "{halt}");
}

#[test]
fn json_lines_only() {
    let capture = Capture::default();
    let (subscriber, guard) =
        subscriber_with_writer(SERVICE, DIRECTIVES, capture.clone()).expect("valid directives");
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("{START_EVENT}");
        emit_api_request();
        emit_batch_job();
        emit_mq_message();
        tracing::debug!("{DEBUG_EVENT}");
    });
    guard.shutdown().expect("telemetry shutdown");

    let output = capture.contents();
    assert!(!output.is_empty(), "telemetry wrote nothing");
    assert!(
        !output.contains(DEBUG_EVENT),
        "an event below the filter level was written: {output}"
    );

    let lines = parse_lines(&output);
    let exported: Vec<&Value> = lines.iter().filter(|line| line["type"] == "span").collect();
    let events: Vec<&Value> = lines
        .iter()
        .filter(|line| line.get("type").is_none())
        .collect();
    assert_eq!(
        exported.len() + events.len(),
        lines.len(),
        "a line is neither a span line nor an event line: {output}"
    );
    assert_span_lines(&exported);
    assert_event_lines(&events, &exported);
}

#[test]
fn invalid_directives_are_rejected() {
    let capture = Capture::default();
    let result = subscriber_with_writer(SERVICE, "=[bad", capture.clone());
    let Err(err) = result else {
        panic!("invalid directives were accepted");
    };
    assert!(
        matches!(err, TelemetryError::Filter(_)),
        "unexpected error: {err:?}"
    );
    assert!(
        capture.contents().is_empty(),
        "a rejected setup wrote output"
    );
}
