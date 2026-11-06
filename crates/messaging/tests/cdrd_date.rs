//! CDRD (`CODATE01`) parity: the date-inquiry consumer answers each request on
//! `CARD.DEMO.REQUEST.DATE` with the reply the audited `CODATE01` rules
//! specify, byte for byte where the bytes are fixed and by format where they
//! come from the clock (AAP "Time-dependent output").
//!
//! Every test starts its own `PostgreSQL` and `RabbitMQ` containers through
//! [`common::Harness`], so a running Docker daemon is required. Every expected
//! label, offset, length and queue name is read from
//! `docs/parity/parity-oracle-audit.md` through [`audit_row`] and
//! [`audit_texts`], never from the code under test; the few literals kept here
//! are asserted against the audit, so a drifted audit fails loudly.
//!
//! Rows proven here:
//!
//! - `P-CODATE01-010`, `P-CODATE01-011`: the fixed reply and error queues;
//! - `P-CODATE01-012`: the request body is never read, the request IDs are
//!   kept, and a request longer than the get buffer is answered by the
//!   `INP MQGET ERR:` record instead of a reply;
//! - `P-CODATE01-013`, `P-CODATE01-014`, `P-CODATE01-020`, `P-CODATE01-021`:
//!   the 1000-byte reply, its labels, date and time formats and positions, and
//!   its identifier echo to the fixed reply queue;
//! - `P-CODATE01-024`: each request is settled once, as the harness's AMQP
//!   tap sees it on the consumers' connection: a replied request by one
//!   `basic.ack` after its reply's confirm reached the consumer, a request
//!   without a reply by one `basic.reject` without requeue after its error
//!   record's confirm, and neither is delivered again;
//! - `P-CODATE01-001` to `P-CODATE01-009` and `P-CODATE01-016`: the
//!   `MQ-ERR-DISPLAY` layout and its 1000-byte buffer, for the get-failure
//!   record.
//!
//! The reply-put failure rows are proven by `ack_after_confirm.rs`. Rows with
//! no counterpart in a long-running consumer, which opens its queues once at
//! start-up and closes none per request, are not reproduced and nothing is
//! asserted for them: the `MQCLOSE` paths `P-CODATE01-017` and
//! `P-CODATE01-018` (E-22), and the `MQOPEN` failure paths `P-CODATE01-022`
//! and `P-CODATE01-023` (E-22).

pub mod common;

use std::ops::Range;
use std::time::Duration;

use carddemo_messaging::error_queue::ERROR_QUEUE;
use carddemo_messaging::flows::cdrd::REPLY_QUEUE;
use carddemo_testkit::block_on;
use common::amqp_tap::Outcome;
use common::{Harness, audit_row, audit_texts};
use lapin::message::Delivery;
use lapin::types::ShortString;

/// How long a reply or an error record may take to arrive.
const REPLY_WAIT: Duration = Duration::from_secs(30);

/// How long a queue is watched to show that nothing else arrives on it.
const ABSENCE_WAIT: Duration = Duration::from_secs(3);

/// How long the message span of a settled request may take to be written.
const SPAN_WAIT: Duration = Duration::from_secs(30);

/// The AMQP `delivery_mode` of a persistent message: every reply is
/// persistent and has no expiry (AAP 0.3.3).
const PERSISTENT: u8 = 2;

/// The section F pin shared by the parity tests (`CARDDEMO_CLOCK_OVERRIDE`).
const PINNED_INSTANT: &str = "2022-07-01T12:00:00Z";

/// The pinned instant as `FORMATTIME MMDDYYYY DATESEP('-')` renders it, in
/// UTC. The section F rows record the format and the position of the date,
/// not a value for this pin, so the value is written here and checked against
/// the row's format.
const PINNED_DATE: &str = "07-01-2022";

/// The pinned instant as `FORMATTIME TIME TIMESEP` renders it, in UTC; checked
/// against the section F time format as [`PINNED_DATE`] is.
const PINNED_TIME: &str = "12:00:00";

/// The 46 text bytes of the reply at the pinned instant, which the reply built
/// from the audited labels must equal.
const PINNED_TEXT: &str = "SYSTEM DATE : 07-01-2022SYSTEM TIME : 12:00:00";

/// The date format `P-CODATE01-020` records, which [`is_mmddyyyy`] implements.
const DATE_FORMAT: &str = "^(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])-[0-9]{4}$";

/// The time format `P-CODATE01-021` records, which [`is_hhmmss`] implements.
const TIME_FORMAT: &str = "^([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]$";

/// The key of a section F row's position, `reply bytes <first>-<last>`.
const POSITION_KEY: &str = "position=reply bytes ";

/// The cells of a section A layout row: the six common columns, then Offset,
/// Length and Usage.
const LAYOUT_CELLS: usize = 9;

/// The `MQ-ERR-DISPLAY` fields in record order [CODATE01.cbl:L58-67]:
/// `MQ-ERROR-PARA`, `FILLER`, `MQ-APPL-RETURN-MESSAGE`, `FILLER`,
/// `MQ-APPL-CONDITION-CODE`, `FILLER`, `MQ-APPL-REASON-CODE`, `FILLER`,
/// `MQ-APPL-QUEUE-NAME`.
const ERROR_RECORD_ROWS: [&str; 9] = [
    "P-CODATE01-001",
    "P-CODATE01-002",
    "P-CODATE01-003",
    "P-CODATE01-004",
    "P-CODATE01-005",
    "P-CODATE01-006",
    "P-CODATE01-007",
    "P-CODATE01-008",
    "P-CODATE01-009",
];

/// The span attribute values a settled request can carry as its `outcome`.
const OUTCOMES: [&str; 2] = ["replied", "rejected"];

/// The marker of a span line in the captured telemetry.
const SPAN_LINE: &str = r#""type":"span""#;

/// The `flow` attribute of a CDRD message span.
const CDRD_FLOW: &str = r#""flow":"CDRD""#;

/// The `message_id` and `correlation_id` of each test's request, distinct so
/// that no test can match another's reply or span.
const MESSAGE_ID_A: &str = "cdrd-msg-0001";
/// See [`MESSAGE_ID_A`].
const CORRELATION_ID_A: &str = "cdrd-corr-0001";
/// See [`MESSAGE_ID_A`].
const MESSAGE_ID_B: &str = "cdrd-msg-0002";
/// See [`MESSAGE_ID_A`].
const CORRELATION_ID_B: &str = "cdrd-corr-0002";
/// See [`MESSAGE_ID_A`].
const MESSAGE_ID_C: &str = "cdrd-msg-0003";
/// See [`MESSAGE_ID_A`].
const CORRELATION_ID_C: &str = "cdrd-corr-0003";

/// The CDRD reply layout as the audit records it.
struct ReplyLayout {
    /// The reply length: `reply=1000 bytes` (`P-CODATE01-014`).
    length: usize,
    /// The label before the date (`P-CODATE01-013`, `P-CODATE01-020`).
    date_label: String,
    /// The 0-based byte range of the date (`P-CODATE01-020`).
    date: Range<usize>,
    /// The label before the time (`P-CODATE01-013`, `P-CODATE01-021`).
    time_label: String,
    /// The 0-based byte range of the time (`P-CODATE01-021`).
    time: Range<usize>,
}

impl ReplyLayout {
    /// `P-CODATE01-013`, `P-CODATE01-014`, `P-CODATE01-020`, `P-CODATE01-021`.
    ///
    /// Reads the layout from the audit and checks that its rows agree: the
    /// labels of `P-CODATE01-013` are those the section F rows place the
    /// fields after, the fields follow their labels with nothing between
    /// (`STRING … DELIMITED BY SIZE` [CODATE01.cbl:L355-360]), and the text
    /// and padding lengths are the ones `P-CODATE01-013` states.
    fn from_audit() -> Self {
        let labels = audit_texts("P-CODATE01-013");
        assert_eq!(
            labels.len(),
            2,
            "P-CODATE01-013 names two labels: {labels:?}"
        );
        let date_label = section_f_label("P-CODATE01-020", DATE_FORMAT);
        let time_label = section_f_label("P-CODATE01-021", TIME_FORMAT);
        assert_eq!([&date_label, &time_label], [&labels[0], &labels[1]]);
        let date = reply_position("P-CODATE01-020");
        let time = reply_position("P-CODATE01-021");
        assert_eq!(date.start, date_label.len(), "the date follows its label");
        assert_eq!(
            time.start - date.end,
            time_label.len(),
            "the time label follows the date, and the time its label"
        );
        let length = number_after(&audit_row("P-CODATE01-014"), "reply=", "P-CODATE01-014");
        assert_audit_says(
            "P-CODATE01-013",
            &format!(
                "{length}-byte buffer: {} text bytes then {} spaces",
                time.end,
                length - time.end
            ),
        );
        Self {
            length,
            date_label,
            date,
            time_label,
            time,
        }
    }
}

/// The `MQ-ERR-DISPLAY` record layout as the audit records it.
struct ErrorRecordLayout {
    /// `MQ-ERROR-PARA X(25)` (`P-CODATE01-001`).
    error_para: Range<usize>,
    /// The four `FILLER X(02)` fields, each `VALUE SPACES`.
    fillers: [Range<usize>; 4],
    /// `MQ-APPL-RETURN-MESSAGE X(25)` (`P-CODATE01-003`).
    return_message: Range<usize>,
    /// `MQ-APPL-CONDITION-CODE 9(02)` (`P-CODATE01-005`).
    condition_code: Range<usize>,
    /// `MQ-APPL-REASON-CODE 9(05)` (`P-CODATE01-007`).
    reason_code: Range<usize>,
    /// `MQ-APPL-QUEUE-NAME X(48)` (`P-CODATE01-009`).
    queue_name: Range<usize>,
    /// The length of the message `9000-ERROR` puts (`P-CODATE01-016`).
    buffer: usize,
}

impl ErrorRecordLayout {
    /// `P-CODATE01-001` to `P-CODATE01-009`, `P-CODATE01-016`.
    ///
    /// Reads every field's offset and length, checks that the fields tile the
    /// record from offset 0, and that `P-CODATE01-016` puts a record of that
    /// width in its buffer.
    fn from_audit() -> Self {
        let fields = ERROR_RECORD_ROWS.map(layout_field);
        assert_eq!(fields[0].start, 0, "MQ-ERR-DISPLAY starts at offset 0");
        for pair in fields.windows(2) {
            assert_eq!(
                pair[0].end, pair[1].start,
                "MQ-ERR-DISPLAY fields are contiguous"
            );
        }
        let width = fields[fields.len() - 1].end;
        let row = audit_row("P-CODATE01-016");
        let buffer = number_after(&row, &format!("X({width}) in a "), "P-CODATE01-016");
        assert_audit_says("P-CODATE01-016", &format!("{buffer}-byte buffer"));
        let [
            error_para,
            filler_1,
            return_message,
            filler_2,
            condition_code,
            filler_3,
            reason_code,
            filler_4,
            queue_name,
        ] = fields;
        Self {
            error_para,
            fillers: [filler_1, filler_2, filler_3, filler_4],
            return_message,
            condition_code,
            reason_code,
            queue_name,
            buffer,
        }
    }

    /// `P-CODATE01-012` (`3000-GET-REQUEST` other failure).
    ///
    /// Asserts that `record` is the get-failure record for `input_queue`: the
    /// return message the audit quotes, the input queue as the queue name
    /// (`MOVE INPUT-QUEUE-NAME TO MQ-APPL-QUEUE-NAME` [CODATE01.cbl:L332]),
    /// `MQ-ERROR-PARA` as the spaces `INITIALIZE MQ-ERR-DISPLAY` leaves
    /// [CODATE01.cbl:L134], and spaces after the record. The condition and
    /// reason codes are checked by their `9(n)` format only: MQ codes have no
    /// AMQP counterpart (E-39).
    fn assert_get_failure(&self, record: &[u8], input_queue: &str) {
        assert_eq!(record.len(), self.buffer, "error record length");
        assert_spaces(&record[self.error_para.clone()], "MQ-ERROR-PARA");
        for filler in &self.fillers {
            assert_spaces(&record[filler.clone()], "FILLER");
        }
        let texts = audit_texts("P-CODATE01-012");
        assert_eq!(texts.len(), 1, "P-CODATE01-012 quotes one return message");
        assert_field(
            record,
            &self.return_message,
            &texts[0],
            "MQ-APPL-RETURN-MESSAGE",
        );
        assert_digits(
            &record[self.condition_code.clone()],
            "MQ-APPL-CONDITION-CODE",
        );
        assert_digits(&record[self.reason_code.clone()], "MQ-APPL-REASON-CODE");
        assert_field(record, &self.queue_name, input_queue, "MQ-APPL-QUEUE-NAME");
        assert_spaces(
            &record[self.queue_name.end..],
            "the buffer after the record",
        );
    }
}

/// `P-CODATE01-010`, `P-CODATE01-011`: the routing constants equal the queue
/// literals the audit quotes, `CARD.DEMO.REPLY.DATE` [CODATE01.cbl:L147] and
/// `CARD.DEMO.ERROR` [CODATE01.cbl:L243].
fn assert_queue_literals() {
    assert_eq!(audit_texts("P-CODATE01-010"), [REPLY_QUEUE]);
    assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.DATE");
    assert_eq!(audit_texts("P-CODATE01-011"), [ERROR_QUEUE]);
    assert_eq!(ERROR_QUEUE, "CARD.DEMO.ERROR");
}

/// The length of the get buffer, `MQ-BUFFER X(1000)` (`P-CODATE01-012`): the
/// longest request `CODATE01` can get.
fn request_buffer() -> usize {
    number_after(
        &audit_row("P-CODATE01-012"),
        "MQ-BUFFER X(",
        "P-CODATE01-012",
    )
}

/// The label of the section F row `id`, after checking that its format is
/// `format`, the one this file's matcher implements.
fn section_f_label(id: &str, format: &str) -> String {
    let texts = audit_texts(id);
    assert_eq!(
        texts.len(),
        2,
        "{id} quotes a format and a label: {texts:?}"
    );
    assert_eq!(texts[0], format, "{id} format; update its matcher with it");
    texts[1].clone()
}

/// The 0-based byte range of the section F row `id`'s
/// `position=reply bytes <first>-<last>`, which counts from 1, inclusive.
fn reply_position(id: &str) -> Range<usize> {
    let row = audit_row(id);
    let first = number_after(&row, POSITION_KEY, id);
    let last = number_after(&row, &format!("{POSITION_KEY}{first}-"), id);
    assert!(
        (1..=last).contains(&first),
        "{id}: bad position {first}-{last}"
    );
    first - 1..last
}

/// The 0-based byte range of the section A layout row `id`, from its Offset
/// and Length cells.
fn layout_field(id: &str) -> Range<usize> {
    let row = audit_row(id);
    let cells: Vec<&str> = row
        .trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    assert_eq!(cells.len(), LAYOUT_CELLS, "{id} is a layout row: {row}");
    assert_eq!(cells[0], id, "{id}: {row}");
    let offset = parse_number(cells[LAYOUT_CELLS - 3], id);
    let length = parse_number(cells[LAYOUT_CELLS - 2], id);
    offset..offset + length
}

/// The decimal number that follows the first `marker` in the audit row text
/// `row` of `id`.
fn number_after(row: &str, marker: &str, id: &str) -> usize {
    let (_, rest) = row
        .split_once(marker)
        .unwrap_or_else(|| panic!("audit row {id} holds no {marker:?}: {row}"));
    let end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    parse_number(&rest[..end], id)
}

/// `text` as a decimal number, read from the audit row `id`.
fn parse_number(text: &str, id: &str) -> usize {
    text.parse()
        .unwrap_or_else(|error| panic!("audit row {id}: {text:?} is not a number: {error}"))
}

/// Asserts that the audit row `id` states `phrase`, which the caller proves.
fn assert_audit_says(id: &str, phrase: &str) {
    let row = audit_row(id);
    assert!(
        row.contains(phrase),
        "audit row {id} no longer states {phrase:?}: {row}"
    );
}

/// Whether `pair` is two ASCII digits, and their value.
fn two_digit_value(pair: &[u8]) -> Option<u8> {
    match pair {
        [tens @ b'0'..=b'9', units @ b'0'..=b'9'] => Some((tens - b'0') * 10 + (units - b'0')),
        _ => None,
    }
}

/// Whether `text` matches [`DATE_FORMAT`]: `mm-dd-yyyy` with month 01-12 and
/// day 01-31.
fn is_mmddyyyy(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[2] == b'-'
        && bytes[5] == b'-'
        && two_digit_value(&bytes[0..2]).is_some_and(|month| (1..=12).contains(&month))
        && two_digit_value(&bytes[3..5]).is_some_and(|day| (1..=31).contains(&day))
        && bytes[6..].iter().all(u8::is_ascii_digit)
}

/// Whether `text` matches [`TIME_FORMAT`]: `hh:mm:ss` with hour 00-23 and
/// minute and second 00-59.
fn is_hhmmss(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 8
        && bytes[2] == b':'
        && bytes[5] == b':'
        && two_digit_value(&bytes[0..2]).is_some_and(|hour| hour <= 23)
        && two_digit_value(&bytes[3..5]).is_some_and(|minute| minute <= 59)
        && two_digit_value(&bytes[6..8]).is_some_and(|second| second <= 59)
}

/// Asserts that every byte of `bytes` is a space.
fn assert_spaces(bytes: &[u8], what: &str) {
    assert!(
        bytes.iter().all(|&byte| byte == b' '),
        "{what} is not all spaces: {:?}",
        String::from_utf8_lossy(bytes)
    );
}

/// Asserts that every byte of `bytes` is an ASCII digit, as a `9(n)` field
/// holds.
fn assert_digits(bytes: &[u8], what: &str) {
    assert!(
        bytes.iter().all(u8::is_ascii_digit),
        "{what} is not all digits: {:?}",
        String::from_utf8_lossy(bytes)
    );
}

/// Asserts that the `range` of `record` holds `text` moved into an `X(n)`
/// field: space-padded to the field length.
fn assert_field(record: &[u8], range: &Range<usize>, text: &str, what: &str) {
    let mut expected = text.as_bytes().to_vec();
    assert!(
        expected.len() <= range.len(),
        "{text:?} does not fit {what}"
    );
    expected.resize(range.len(), b' ');
    assert_eq!(
        String::from_utf8_lossy(&record[range.clone()]),
        String::from_utf8_lossy(&expected),
        "{what}"
    );
}

/// `P-CODATE01-013`, `P-CODATE01-014`, `P-CODATE01-020`, `P-CODATE01-021`.
///
/// Asserts that `body` has the audited reply layout: the reply length, the
/// date label, a date in the section F date format, the time label, a time in
/// the section F time format, then spaces to the end. Returns the date and
/// time texts.
fn assert_date_reply_format(body: &[u8], layout: &ReplyLayout) -> (String, String) {
    assert_eq!(body.len(), layout.length, "reply length");
    let text = |range: Range<usize>| String::from_utf8_lossy(&body[range]).into_owned();
    assert_eq!(text(0..layout.date.start), layout.date_label, "date label");
    let date = text(layout.date.clone());
    assert!(
        is_mmddyyyy(&date),
        "date {date:?} does not match {DATE_FORMAT}"
    );
    assert_eq!(
        text(layout.date.end..layout.time.start),
        layout.time_label,
        "time label"
    );
    let time = text(layout.time.clone());
    assert!(
        is_hhmmss(&time),
        "time {time:?} does not match {TIME_FORMAT}"
    );
    assert_spaces(&body[layout.time.end..], "the reply after its text");
    (date, time)
}

/// `P-CODATE01-012`, `P-CODATE01-014`.
///
/// Asserts that `reply` echoes the request's `message_id` and
/// `correlation_id`, saved from `MQMD-MSGID` and `MQMD-CORRELID` by the get
/// and moved back into the reply's descriptor [CODATE01.cbl:L313-321]
/// [CODATE01.cbl:L373-374], and that it is persistent with no expiry.
fn assert_reply_properties(reply: &Delivery, message_id: &str, correlation_id: &str) {
    assert_audit_says(
        "P-CODATE01-012",
        "request body ignored; message id, correlation id and reply-to saved",
    );
    assert_audit_says(
        "P-CODATE01-014",
        "message id = request message id, correlation id = request correlation id",
    );
    let properties = &reply.properties;
    assert_eq!(
        properties.message_id().as_ref().map(ShortString::as_str),
        Some(message_id),
        "reply message_id"
    );
    assert_eq!(
        properties
            .correlation_id()
            .as_ref()
            .map(ShortString::as_str),
        Some(correlation_id),
        "reply correlation_id"
    );
    assert_eq!(
        *properties.delivery_mode(),
        Some(PERSISTENT),
        "reply delivery_mode"
    );
    assert_eq!(properties.expiration(), &None, "reply expiration");
}

/// The whole reply at [`PINNED_INSTANT`]: the audited labels with
/// [`PINNED_DATE`] and [`PINNED_TIME`], space-filled to the audited length.
fn pinned_reply(layout: &ReplyLayout) -> String {
    assert!(
        is_mmddyyyy(PINNED_DATE),
        "{PINNED_DATE} does not match {DATE_FORMAT}"
    );
    assert!(
        is_hhmmss(PINNED_TIME),
        "{PINNED_TIME} does not match {TIME_FORMAT}"
    );
    let text = format!(
        "{}{PINNED_DATE}{}{PINNED_TIME}",
        layout.date_label, layout.time_label
    );
    assert_eq!(text, PINNED_TEXT, "the reply text built from the audit");
    assert_eq!(
        text.len(),
        layout.time.end,
        "the pinned text ends where the time does"
    );
    format!("{text:<width$}", width = layout.length)
}

/// The JSON spelling of the string span attribute `key` with `value`, as the
/// captured span lines write it.
fn attribute(key: &str, value: &str) -> String {
    format!(r#""{key}":"{value}""#)
}

/// Waits for the CDRD message span of `message_id` and asserts that it
/// carries `correlation_id`, `outcome` and `error_record`.
async fn assert_settled(
    h: &Harness,
    message_id: &str,
    correlation_id: &str,
    outcome: &str,
    error_record: &str,
) {
    let id = attribute("message_id", message_id);
    let span = h
        .wait_for_log(&[SPAN_LINE, CDRD_FLOW, &id], SPAN_WAIT)
        .await;
    for expected in [
        attribute("correlation_id", correlation_id),
        attribute("outcome", outcome),
        attribute("error_record", error_record),
    ] {
        assert!(
            span.contains(&expected),
            "the span lacks {expected}: {span}"
        );
    }
}

/// `P-CODATE01-024`.
///
/// Asserts, once the absence checks have waited, that the request
/// `message_id` has exactly one message span, so the consumer handled it
/// once, and that no captured line holds an outcome other than `outcome`.
/// How it was settled, and that it was never delivered again, is what the
/// tap's journal shows ([`Harness::assert_acked_after_confirm`],
/// [`Harness::assert_rejected`], [`Harness::assert_delivered_once`]).
fn assert_settled_once(h: &Harness, message_id: &str, outcome: &str) {
    let id = attribute("message_id", message_id);
    let lines = h.logs();
    let spans = lines
        .iter()
        .filter(|line| {
            [SPAN_LINE, CDRD_FLOW, id.as_str()]
                .iter()
                .all(|needle| line.contains(needle))
        })
        .count();
    assert_eq!(spans, 1, "{message_id} was handled {spans} time(s)");
    for other in OUTCOMES.into_iter().filter(|&other| other != outcome) {
        let other = attribute("outcome", other);
        assert!(
            !lines.iter().any(|line| line.contains(&other)),
            "a captured line holds {other}"
        );
    }
}

/// Waits for the `ERROR` event of the CDRD request `message_id` rejected once
/// its record was published, and asserts that it holds `display`, the
/// `MQ-ERR-DISPLAY` group of the published record, with its return message,
/// and nothing of `body`, the request body.
async fn assert_rejection_logged(h: &Harness, message_id: &str, display: &[u8], body: &[u8]) {
    let id = attribute("message_id", message_id);
    let line = h
        .wait_for_log(
            &[
                r#""level":"ERROR""#,
                CDRD_FLOW,
                &id,
                "request rejected: MQ-ERR-DISPLAY published to the error queue",
            ],
            SPAN_WAIT,
        )
        .await;
    let display = attribute("mq_err_display", &String::from_utf8_lossy(display));
    assert!(
        line.contains(&display),
        "the rejection event lacks {display}: {line}"
    );
    for text in audit_texts("P-CODATE01-012") {
        assert!(
            line.contains(&text),
            "the rejection event lacks the return message {text}: {line}"
        );
    }
    assert!(
        !line.contains(&*String::from_utf8_lossy(body)),
        "the rejection event carries the request body: {line}"
    );
}

/// `P-CODATE01-010`, `P-CODATE01-011`, `P-CODATE01-012`, `P-CODATE01-013`,
/// `P-CODATE01-014`, `P-CODATE01-020`, `P-CODATE01-021`, `P-CODATE01-024`.
///
/// On the system clock, a request with a `reply_to` of its own gets the
/// audited 1000-byte reply on the fixed reply queue: the two labels, a date
/// and a time in their section F formats and positions, then spaces. The
/// reply echoes the request's IDs, is persistent and does not expire. The
/// `reply_to` queue stays empty, because `CODATE01` never reads the saved
/// `REPLYTOQ`, and so does the error queue. The request leaves its queue and
/// is settled once, by a `basic.ack` after its reply's confirm, and is never
/// delivered again.
#[test]
fn cdrd_reply_has_audited_format_with_system_clock() {
    block_on(async {
        assert_queue_literals();
        let layout = ReplyLayout::from_audit();
        let h = Harness::start(None).await;
        let request_queue = &h.settings.queue_date_request;
        let decoy = h.temp_reply_queue().await;
        h.publish(
            request_queue,
            b"DATE",
            MESSAGE_ID_A,
            CORRELATION_ID_A,
            Some(&decoy),
        )
        .await;
        let reply = h.next_message(REPLY_QUEUE, REPLY_WAIT).await;
        assert_date_reply_format(&reply.data, &layout);
        assert_reply_properties(&reply, MESSAGE_ID_A, CORRELATION_ID_A);
        assert_audit_says("P-CODATE01-014", "(the request reply-to is not used)");
        assert_settled(&h, MESSAGE_ID_A, CORRELATION_ID_A, "replied", "none").await;
        h.assert_acked_after_confirm("CDRD", MESSAGE_ID_A, REPLY_QUEUE)
            .await;
        h.assert_no_message(&decoy, ABSENCE_WAIT).await;
        tokio::join!(
            h.assert_no_message(ERROR_QUEUE, ABSENCE_WAIT),
            h.assert_delivered_once("CDRD", MESSAGE_ID_A, ABSENCE_WAIT),
        );
        assert_eq!(
            h.queue_depth(request_queue).await,
            0,
            "request left on its queue"
        );
        assert_settled_once(&h, MESSAGE_ID_A, "replied");
        h.shutdown().await;
    });
}

/// `P-CODATE01-012`, `P-CODATE01-013`, `P-CODATE01-014`, `P-CODATE01-020`,
/// `P-CODATE01-021`, `P-CODATE01-024`.
///
/// With the clock pinned to [`PINNED_INSTANT`], a request that fills the
/// whole get buffer, whose content `CODATE01` never reads, gets exactly
/// `SYSTEM DATE : 07-01-2022SYSTEM TIME : 12:00:00` followed by 954 spaces,
/// built from the audited labels and lengths, with the request's IDs echoed.
/// Nothing reaches the error queue, and the request is settled once, by a
/// `basic.ack` after its reply's confirm, and is never delivered again.
#[test]
fn cdrd_reply_uses_pinned_clock() {
    block_on(async {
        let layout = ReplyLayout::from_audit();
        let expected = pinned_reply(&layout);
        let h = Harness::start(Some(PINNED_INSTANT)).await;
        let request_queue = &h.settings.queue_date_request;
        let body = vec![b'X'; request_buffer()];
        h.publish(request_queue, &body, MESSAGE_ID_B, CORRELATION_ID_B, None)
            .await;
        let reply = h.next_message(REPLY_QUEUE, REPLY_WAIT).await;
        let (date, time) = assert_date_reply_format(&reply.data, &layout);
        assert_eq!([date.as_str(), time.as_str()], [PINNED_DATE, PINNED_TIME]);
        assert_eq!(
            String::from_utf8_lossy(&reply.data),
            expected,
            "pinned reply"
        );
        assert_reply_properties(&reply, MESSAGE_ID_B, CORRELATION_ID_B);
        assert_settled(&h, MESSAGE_ID_B, CORRELATION_ID_B, "replied", "none").await;
        h.assert_acked_after_confirm("CDRD", MESSAGE_ID_B, REPLY_QUEUE)
            .await;
        tokio::join!(
            h.assert_no_message(ERROR_QUEUE, ABSENCE_WAIT),
            h.assert_delivered_once("CDRD", MESSAGE_ID_B, ABSENCE_WAIT),
        );
        assert_eq!(
            h.queue_depth(request_queue).await,
            0,
            "request left on its queue"
        );
        assert_settled_once(&h, MESSAGE_ID_B, "replied");
        h.shutdown().await;
    });
}

/// `P-CODATE01-001` to `P-CODATE01-009`, `P-CODATE01-011`, `P-CODATE01-012`,
/// `P-CODATE01-016`.
///
/// A request one byte longer than the 1000-byte get buffer fails its get
/// [CODATE01.cbl:L291-335]: no reply is put, and the `MQ-ERR-DISPLAY` record
/// with return message `INP MQGET ERR:` and the request queue's name, in its
/// 1000-byte buffer, goes to the error queue, once. The request is rejected
/// without requeue, because no path requeues (AAP 0.3.3): its only publish is
/// the record, confirmed, then one `basic.reject` settles it, and it is never
/// delivered again. The rejection is an `ERROR` event that holds the record's
/// `MQ-ERR-DISPLAY`, not the body.
#[test]
fn cdrd_oversize_request_gets_get_failure_record_and_no_reply() {
    block_on(async {
        assert_queue_literals();
        let layout = ErrorRecordLayout::from_audit();
        let h = Harness::start(None).await;
        let request_queue = &h.settings.queue_date_request;
        let body = vec![b'X'; request_buffer() + 1];
        h.publish(request_queue, &body, MESSAGE_ID_C, CORRELATION_ID_C, None)
            .await;
        let record = h.next_message(ERROR_QUEUE, REPLY_WAIT).await;
        layout.assert_get_failure(&record.data, request_queue);
        assert_settled(&h, MESSAGE_ID_C, CORRELATION_ID_C, "rejected", "published").await;
        h.assert_rejected("CDRD", MESSAGE_ID_C, &[(ERROR_QUEUE, Outcome::Confirmed)])
            .await;
        let display = &record.data[..layout.queue_name.end];
        assert_rejection_logged(&h, MESSAGE_ID_C, display, &body).await;
        h.assert_no_message(REPLY_QUEUE, ABSENCE_WAIT).await;
        tokio::join!(
            h.assert_no_message(ERROR_QUEUE, ABSENCE_WAIT),
            h.assert_delivered_once("CDRD", MESSAGE_ID_C, ABSENCE_WAIT),
        );
        assert_eq!(
            h.queue_depth(request_queue).await,
            0,
            "request left on its queue"
        );
        assert_settled_once(&h, MESSAGE_ID_C, "rejected");
        h.shutdown().await;
    });
}
