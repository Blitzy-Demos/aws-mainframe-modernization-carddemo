//! Acknowledge after confirm, on a real broker: a request is acknowledged only
//! after the broker confirms its reply with a plain `Ack`, and a request
//! without a confirmed reply is rejected without requeue once its failure is
//! recorded.
//!
//! The COBOL ends each failure path with a normal `EXEC CICS RETURN`, which
//! commits: the request is consumed, the `MQ-ERR-DISPLAY` record that
//! `9000-ERROR` put is delivered, and no reply exists
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L392-403]
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L442-454]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L437-445]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L496-498]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L549]. CP00 has no error queue: it logs
//! its critical `M004` record and its task ends before any write
//! [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L767-779].
//!
//! Each test starts its own stack and breaks it for good: a deleted reply
//! queue, a dropped `accounts` table, a reply queue that never existed, a
//! requester on `RabbitMQ` direct reply-to. What the tests observe:
//!
//! - **Error record**: the body on `CARD.DEMO.ERROR`, compared field by field
//!   with the record laid out from the audit's section A rows of
//!   `MQ-ERR-DISPLAY`, each field held to its row's Offset, Length and
//!   `rust=` type.
//! - **Rejected, not acknowledged**: the request queue is empty and the
//!   request's message span records `outcome` `rejected`.
//! - **Settled**: each consumer has a prefetch of 1, so the broker hands it a
//!   later request only once the earlier one is acknowledged or rejected; a
//!   later request answered proves the earlier one settled. A consumer that
//!   acknowledged before the confirm could not reject afterwards, and would
//!   lose its channel instead.
//! - **Never redelivered**: after a settle wait, no second error record has
//!   arrived and exactly one message span exists per request.
//! - **Logged**: each request rejected with an error record is one `ERROR`
//!   event whose own fields name the request and hold the published
//!   `MQ-ERR-DISPLAY` group, never the request body.
//!
//! Every expected text comes from `docs/parity/parity-oracle-audit.md`
//! through [`audit_texts`] and [`audit_row`], never from the code under test.
//! AMQP produces no MQ or CICS codes; the codes the records carry are the
//! proposal of escalation E-39 in `docs/parity/escalations.md`.

pub mod common;

use std::ops::Range;
use std::time::Duration;

use carddemo_domain::numeric::ids::AccountId;
use carddemo_domain::numeric::{Alnum, NumericCode, NumericError};
use carddemo_domain::ports::{AuthorizationStore, Browse, Direction, PortError};
use carddemo_messaging::error_queue::ERROR_QUEUE;
use carddemo_messaging::flows::{cdra, cdrd};
use carddemo_testkit::{block_on, fixtures};
use common::audit_tables::{Citation, citations};
use common::{Harness, audit_row, audit_texts};
use futures_util::{FutureExt, StreamExt};
use lapin::options::{BasicConsumeOptions, QueueDeleteOptions};
use lapin::types::FieldTable;

/// How long an error record, a reply or a log line may take to appear.
const ARRIVAL: Duration = Duration::from_secs(30);

/// How long a queue is watched for a message that must not come: a reply, a
/// second error record, a redelivery's record.
const SETTLE: Duration = Duration::from_secs(3);

/// `ERROR-MESSAGE PIC X(1000)`, which `9000-ERROR` moves `MQ-ERR-DISPLAY` into
/// and puts with `MOVE 1000 TO MQ-BUFFER-LENGTH`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L108]
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L409-411] (P-CODATE01-016,
/// P-COACCT01-018).
const ERROR_MESSAGE_LEN: usize = 1000;

/// The length of `MQ-ERR-DISPLAY`, which its section A rows tile: "Lengths
/// sum to 113 bytes, the record length."
const ERR_DISPLAY_LEN: usize = 113;

/// The fields of `MQ-ERR-DISPLAY`, section A rows `P-<KEY>-001` to
/// `P-<KEY>-009` in PIC order [app/app-vsam-mq/cbl/CODATE01.cbl:L58-67].
const ERR_DISPLAY_FIELDS: usize = 9;

/// The section A rows of the `CODATE01` `MQ-ERR-DISPLAY`, one per field in
/// PIC order: `MQ-ERROR-PARA`, `FILLER`, `MQ-APPL-RETURN-MESSAGE`, `FILLER`,
/// `MQ-APPL-CONDITION-CODE`, `FILLER`, `MQ-APPL-REASON-CODE`, `FILLER`,
/// `MQ-APPL-QUEUE-NAME` [app/app-vsam-mq/cbl/CODATE01.cbl:L58-67].
const CODATE01_ERR_DISPLAY: [&str; ERR_DISPLAY_FIELDS] = [
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

/// The section A rows of the `COACCT01` `MQ-ERR-DISPLAY`, the same fields in
/// the same order [app/app-vsam-mq/cbl/COACCT01.cbl:L58-67].
const COACCT01_ERR_DISPLAY: [&str; ERR_DISPLAY_FIELDS] = [
    "P-COACCT01-001",
    "P-COACCT01-002",
    "P-COACCT01-003",
    "P-COACCT01-004",
    "P-COACCT01-005",
    "P-COACCT01-006",
    "P-COACCT01-007",
    "P-COACCT01-008",
    "P-COACCT01-009",
];

/// The Input condition cell of a section A `9(n)` field.
const DIGITS_CONDITION: &str = "unsigned display digits 0-9";

/// The Input condition cell of a section A `X(n)` field.
const ALNUM_CONDITION: &str = "alphanumeric copy";

/// The field name of a section A row whose item is `FILLER`.
const FILLER: &str = "FILLER";

/// The clause that gives each `MQ-ERR-DISPLAY` `FILLER` its spaces, which
/// `INITIALIZE MQ-ERR-DISPLAY` leaves as they are.
const VALUE_SPACES: &str = "VALUE SPACES";

/// The request queues' local defaults (AAP 0.6.4): the trigger queues
/// `MQTM-QNAME` would name, and so the `INPUT-QUEUE-NAME` a record carries.
const DATE_REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.DATE";
const ACCT_REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";
const PAUTH_REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.PAUTH";

/// E-39: `MQ-APPL-CONDITION-CODE` and `MQ-APPL-REASON-CODE` of the `MQPUT ERR`
/// record of a returned publish, as their `9(02)` and `9(05)` fields hold them.
const PUBLISH_FAILED_CONDITION: &str = "02";
const PUBLISH_FAILED_REASON: &str = "00000";

/// E-39: the same fields of the read-error record, where the COBOL moves the
/// read's `RESP` and `RESP2` [app/app-vsam-mq/cbl/COACCT01.cbl:L439-440].
const READ_ERROR_CONDITION: &str = "00";
const READ_ERROR_REASON: &str = "00000";

/// The decision of E-39 the four codes above are read from.
const E39_PROPOSAL: &str = "Proposed: condition code 02 and reason 00000 for a returned, nacked \
                            or unconfirmed publish; 00 and 00000 for an account read error";

/// `CCPAUERY`, the copybook whose 88-levels name the `M004` record's level and
/// subsystem codes.
const CCPAUERY: &str = "app/app-authorization-ims-db2-mq/cpy/CCPAUERY.cpy";

/// A reply queue that nothing declares: `infra/rabbitmq/definitions.json`
/// declares six queues and this is none of them. The test's channel never
/// names it, since touching a missing queue closes the channel.
const MISSING_REPLY_QUEUE: &str = "CARD.DEMO.NO.SUCH.REPLY";

/// The `RabbitMQ` direct reply-to pseudo-queue: a requester consumes it with
/// `no_ack` and names it as the `reply_to` of a request published on the
/// same channel, and the broker rewrites that `reply_to` to
/// `amq.rabbitmq.reply-to.<token>`.
const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// Account `00000000001` and its fixture card
/// [app/data/ASCII/cardxref.txt:L49].
const ACCOUNT_1: &str = "00000000001";
const ACCOUNT_1_CARD: &str = "9680294154603697";

/// The 18 comma-separated `CCPAURQY` fields of a CP00 request, of which the
/// card number is the third [app/app-authorization-ims-db2-mq/cpy/CCPAURQY.cpy:L19-36].
const CP00_REQUEST_FIELDS: usize = 18;
const CP00_CARD_FIELD: usize = 2;

/// The `{UNDEF:n}` placeholder of the one reply byte `COPAUA0C` never sets
/// (E-25), excluded from the comparison.
const UNSET_REPLY_BYTE: &str = "{UNDEF:1}";

/// Markers of a message span line and the span outcomes compared.
const SPAN_LINE: &str = "\"type\":\"span\"";
const MESSAGE_SPAN: &str = "\"name\":\"message\"";
const REPLIED: &str = "\"outcome\":\"replied\"";
const REJECTED_WITH_RECORD: [&str; 3] = [
    "\"outcome\":\"rejected\"",
    "\"error_record\":\"published\"",
    "\"location\":\"none\"",
];

/// The message of the `ERROR` event of a request rejected once its error
/// record was published and confirmed.
const REJECTED_EVENT: &str = "request rejected: MQ-ERR-DISPLAY published to the error queue";

/// The requests of the tests, each with its own `message_id`.
const CDRD_RETURNED: &str = "ACK-CDRD-RETURNED-1";
const CDRD_AFTER: &str = "ACK-CDRD-RETURNED-2";
const CDRA_READ_ERROR: &str = "ACK-CDRA-READ-ERROR-1";
const CDRA_READ_ERROR_AGAIN: &str = "ACK-CDRA-READ-ERROR-2";
const CDRA_RETURNED: &str = "ACK-CDRA-RETURNED-3";
const CP00_RETURNED: &str = "ACK-CP00-M004-1";
const CP00_ANSWERED: &str = "ACK-CP00-ANSWERED-2";
const CP00_DIRECT_REPLY_TO: &str = "ACK-CP00-DIRECT-REPLY-TO-3";

/// The message span of a CP00 request rejected after its logged `M004`.
/// `error_record` names the fate of an MQ-ERR-DISPLAY record, which only CDRD
/// and CDRA put; CP00 has no error queue, so its span records `none`, and the
/// logged CCPAUERY record shows as its `location`.
const CP00_REJECTED_M004: [&str; 3] = [
    "\"outcome\":\"rejected\"",
    "\"error_record\":\"none\"",
    "\"location\":\"M004\"",
];

/// Asserts that each audit row `ids` names is a data row of the audit, so the
/// IDs the tests cite stay in step with it.
fn assert_cited(ids: &[&str]) {
    for id in ids {
        let row = audit_row(id);
        assert!(
            row.starts_with(&format!("| {id} |")),
            "audit row {id} is not the row found: {row}"
        );
    }
}

/// The `N` exact texts of the audit row `id`'s "Expected result" cell.
fn texts<const N: usize>(id: &str) -> [String; N] {
    audit_texts(id)
        .try_into()
        .unwrap_or_else(|texts: Vec<String>| {
            panic!("audit row {id}: {N} exact text(s) expected, found {texts:?}")
        })
}

/// The request text an audit row's "Input condition" cell gives as
/// ``request=`"…"` ``.
fn audited_request(id: &str) -> String {
    let row = audit_row(id);
    let (_, rest) = row
        .split_once("request=`\"")
        .unwrap_or_else(|| panic!("audit row {id} gives no request: {row}"));
    let (request, _) = rest
        .split_once("\"`")
        .unwrap_or_else(|| panic!("audit row {id}: unterminated request text: {row}"));
    request.to_owned()
}

/// One field of `MQ-ERR-DISPLAY` as its section A row records it.
struct ErrDisplayField {
    /// The row ID.
    id: &'static str,
    /// The bytes of the record the Offset and Length cells give, 0-based.
    bytes: Range<usize>,
    /// The `rust=` label of the Expected result cell.
    rust: String,
    /// A `FILLER`, which its cited line declares [`VALUE_SPACES`].
    filler: bool,
    /// A `9(n)` field, whose Input condition is [`DIGITS_CONDITION`].
    digits: bool,
}

/// The `MQ-ERR-DISPLAY` field of the section A row `id`, after checking the
/// row against itself and against the COBOL line it cites: a `DISPLAY` field
/// whose Length is the width of its PIC, whose Input condition is the one of
/// its PIC class, and whose cited line declares its name and PIC, with
/// [`VALUE_SPACES`] for a `FILLER`.
fn layout_field(id: &'static str) -> ErrDisplayField {
    let row = audit_row(id);
    // | ID | Program | Paragraph | Field | Input | Expected | Offset | Length | Usage |
    let cells: Vec<&str> = row.split('|').map(str::trim).collect();
    assert_eq!(cells.len(), 11, "audit row {id} is not a layout row: {row}");
    assert_eq!(cells[1], id, "audit row {id}: {row}");
    assert_eq!(cells[9], "DISPLAY", "audit row {id} is not a DISPLAY field");
    let number = |cell: &str| {
        cell.parse::<usize>()
            .unwrap_or_else(|error| panic!("audit row {id}: {cell:?} is not a byte count: {error}"))
    };
    let (offset, length) = (number(cells[7]), number(cells[8]));
    let (name, pic) = cells[4]
        .rsplit_once(' ')
        .unwrap_or_else(|| panic!("audit row {id}: {:?} is not a field and its PIC", cells[4]));
    let digits = pic.starts_with('9');
    let width = pic
        .strip_prefix(if digits { "9(" } else { "X(" })
        .and_then(|rest| rest.strip_suffix(')'))
        .map_or_else(
            || panic!("audit row {id}: {pic:?} is not X(n) or 9(n)"),
            number,
        );
    assert_eq!(
        width, length,
        "audit row {id}: Length is not the width of {pic}"
    );
    let condition = if digits {
        DIGITS_CONDITION
    } else {
        ALNUM_CONDITION
    };
    assert_eq!(
        cells[5], condition,
        "audit row {id}: Input condition of {pic}"
    );
    let filler = name == FILLER;
    let line = cited_line(id, cells[3]);
    let pic_clause = format!("PIC {pic}");
    let mut declared = vec![name, pic_clause.as_str()];
    if filler {
        declared.push(VALUE_SPACES);
    }
    for clause in declared {
        assert!(
            line.contains(clause),
            "audit row {id}: its cited line does not declare {clause}: {line}"
        );
    }
    let rust = cells[6]
        .split_once("rust=")
        .and_then(|(_, rest)| rest.split_once("; sql="))
        .map_or_else(
            || panic!("audit row {id}: no rust= before sql= in {:?}", cells[6]),
            |(label, _)| label.trim().to_owned(),
        );
    ErrDisplayField {
        id,
        bytes: offset..offset + length,
        rust,
        filler,
        digits,
    }
}

/// The COBOL line the Paragraph cell of the section A row `id` cites as
/// `[<path>:L<n>]`.
fn cited_line(id: &str, paragraph: &str) -> String {
    let cited = citations(paragraph);
    let [Citation { path, start, end }] = cited.as_slice() else {
        panic!("audit row {id} cites {cited:?}, not one COBOL line");
    };
    assert_eq!(start, end, "audit row {id} cites lines {start}-{end}");
    let file = fixtures::repo_root().join(path);
    let source = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    start
        .checked_sub(1)
        .and_then(|index| source.lines().nth(index))
        .unwrap_or_else(|| panic!("audit row {id}: {path} has no line {start}"))
        .to_owned()
}

/// The message `9000-ERROR` puts, with the section A fields that lay out its
/// `MQ-ERR-DISPLAY`.
struct ExpectedRecord {
    /// The fields, in PIC order.
    fields: [ErrDisplayField; ERR_DISPLAY_FIELDS],
    /// The 1000 bytes of the message.
    bytes: Vec<u8>,
}

/// The message `9000-ERROR` puts for the `MQ-ERR-DISPLAY` whose section A
/// rows are `rows`, holding `values`, one per field in PIC order: each moved
/// into its field, the rest of the 1000-byte buffer spaces.
///
/// An empty value leaves the field's spaces, as `INITIALIZE` leaves
/// `MQ-ERROR-PARA` and `VALUE SPACES` each `FILLER`, so every `FILLER` value is
/// empty. Numeric values are the full width of display digits of their `9(n)`
/// field.
fn expected_error_message(
    rows: [&'static str; ERR_DISPLAY_FIELDS],
    values: [&str; ERR_DISPLAY_FIELDS],
) -> ExpectedRecord {
    let fields = rows.map(layout_field);
    let mut message = vec![b' '; ERROR_MESSAGE_LEN];
    let mut end = 0;
    for (field, value) in fields.iter().zip(values) {
        let ErrDisplayField { id, bytes, .. } = field;
        let length = bytes.len();
        assert_eq!(
            bytes.start, end,
            "audit row {id} does not start where the field before ends"
        );
        assert!(
            value.len() <= length,
            "{value:?} does not fit the {length} bytes of audit row {id}"
        );
        assert!(
            !field.filler || value.is_empty(),
            "{value:?} given for the FILLER of audit row {id}"
        );
        assert!(
            !field.digits
                || (value.len() == length && value.bytes().all(|byte| byte.is_ascii_digit())),
            "{value:?} is not the {length} display digits of audit row {id}"
        );
        message[bytes.start..bytes.start + value.len()].copy_from_slice(value.as_bytes());
        end = bytes.end;
    }
    assert_eq!(
        end, ERR_DISPLAY_LEN,
        "the MQ-ERR-DISPLAY rows {rows:?} do not sum to {ERR_DISPLAY_LEN} bytes"
    );
    ExpectedRecord {
        fields,
        bytes: message,
    }
}

/// Asserts that `published`, the bytes of `field` in a published record, are
/// a value of the type its `rust=` label names, as `carddemo_domain::numeric`
/// binds the label: `Alnum<n>` for an `X(n)` text, and the `NumericCode` of
/// the PIC width for `MqConditionCode` and `MqReasonCode`. The value shown
/// again must be the same bytes, so a `9(n)` field holds exactly n display
/// digits. `raw`, the label of a `FILLER`, names no type.
fn assert_rust_type(field: &ErrDisplayField, published: &[u8]) {
    let ErrDisplayField { id, rust, .. } = field;
    let text = std::str::from_utf8(published)
        .unwrap_or_else(|error| panic!("audit row {id}: {published:?} is not text: {error}"));
    let shown: Result<String, NumericError> = match rust.as_str() {
        "raw" => Ok(text.to_owned()),
        "Alnum<25>" => Alnum::<25>::from_bytes(published).map(|item| item.as_text().into_owned()),
        "Alnum<48>" => Alnum::<48>::from_bytes(published).map(|item| item.as_text().into_owned()),
        "MqConditionCode" => text.parse::<NumericCode<2>>().map(|code| code.to_string()),
        "MqReasonCode" => text.parse::<NumericCode<5>>().map(|code| code.to_string()),
        other => panic!("audit row {id}: no type is bound to the rust= label {other:?}"),
    };
    let shown =
        shown.unwrap_or_else(|error| panic!("audit row {id}: {text:?} is not a {rust}: {error}"));
    assert_eq!(shown, text, "audit row {id}: the {rust} read from {text:?}");
}

/// Asserts that `body`, the error message of the request `message_id`, is
/// `expected`: each `MQ-ERR-DISPLAY` field at the bytes its section A row
/// gives, holding the expected value as a value of the row's `rust=` type,
/// then the 113-byte record compared as text for a readable diff, then every
/// byte of the 1000.
fn assert_error_message(body: &[u8], expected: &ExpectedRecord, message_id: &str) {
    assert_eq!(
        body.len(),
        ERROR_MESSAGE_LEN,
        "error message length for {message_id}"
    );
    for field in &expected.fields {
        let ErrDisplayField { id, bytes, .. } = field;
        let published = &body[bytes.clone()];
        assert_eq!(
            String::from_utf8_lossy(published),
            String::from_utf8_lossy(&expected.bytes[bytes.clone()]),
            "field {id} of the MQ-ERR-DISPLAY for {message_id}"
        );
        assert_rust_type(field, published);
    }
    assert_eq!(
        String::from_utf8_lossy(&body[..ERR_DISPLAY_LEN]),
        String::from_utf8_lossy(&expected.bytes[..ERR_DISPLAY_LEN]),
        "MQ-ERR-DISPLAY for {message_id}"
    );
    assert_eq!(
        body, expected.bytes,
        "the error message of {message_id} after its record"
    );
}

/// Asserts that the rows citing E-39 still cite it and that the escalation
/// log still proposes the codes these tests expect: AMQP has no MQ condition
/// or reason codes and no CICS `RESP`, so they are not the COBOL's own.
fn assert_proposed_codes() {
    for id in ["P-CODATE01-015", "P-CODATE01-019", "P-COACCT01-017"] {
        assert!(
            audit_row(id).contains("E-39"),
            "audit row {id} no longer cites E-39"
        );
    }
    let path = fixtures::parity_doc("escalations.md");
    let log = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let row = log
        .lines()
        .find(|line| line.starts_with("| E-39 |"))
        .unwrap_or_else(|| panic!("no E-39 row in {}", path.display()));
    assert!(
        row.contains(E39_PROPOSAL),
        "E-39 no longer proposes the codes expected here: {row}"
    );
    assert!(
        row.contains("[app/app-vsam-mq/cbl/COACCT01.cbl:L437-445]"),
        "E-39 no longer covers the COACCT01 read error: {row}"
    );
}

/// The value of the 88-level `name` in [`CCPAUERY`], as its `VALUE 'x'`
/// clause gives it.
fn ccpauery_value(name: &str) -> String {
    let path = fixtures::repo_root().join(CCPAUERY);
    let copybook = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let level = format!("88 {name} ");
    let line = copybook
        .lines()
        .find(|line| line.contains(&level))
        .unwrap_or_else(|| panic!("{CCPAUERY} declares no {name}"));
    line.split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("{CCPAUERY}: {name} has no quoted VALUE: {line}"))
        .to_owned()
}

/// Deletes `queue`, which must be empty, on the test's channel. Nothing
/// declares it again: the definitions file declares it at broker boot and no
/// consumer declares a queue. No helper names it afterwards, since touching a
/// missing queue closes the channel.
async fn delete_queue(h: &Harness, queue: &str) {
    let held = h
        .channel
        .queue_delete(queue, QueueDeleteOptions::default())
        .await
        .unwrap_or_else(|error| panic!("cannot delete {queue}: {error}"));
    assert_eq!(held, 0, "{queue} held {held} message(s) when deleted");
}

/// Every captured message span of `flow` for the request `message_id`.
fn message_spans(h: &Harness, flow: &str, message_id: &str) -> Vec<String> {
    let flow_field = format!("\"flow\":\"{flow}\"");
    let id_field = format!("\"message_id\":\"{message_id}\"");
    let needles = [
        SPAN_LINE,
        MESSAGE_SPAN,
        flow_field.as_str(),
        id_field.as_str(),
    ];
    h.logs()
        .into_iter()
        .filter(|line| needles.iter().all(|needle| line.contains(needle)))
        .collect()
}

/// Waits for the message span of `flow` for `message_id`, then asserts that
/// it is the only one, so the request was handled once, and that it records
/// each of `fields`.
async fn assert_single_span(h: &Harness, flow: &str, message_id: &str, fields: &[&str]) {
    let flow_field = format!("\"flow\":\"{flow}\"");
    let id_field = format!("\"message_id\":\"{message_id}\"");
    let first = h
        .wait_for_log(&[SPAN_LINE, MESSAGE_SPAN, &flow_field, &id_field], ARRIVAL)
        .await;
    for field in fields {
        assert!(
            first.contains(field),
            "{flow} span of {message_id} lacks {field}: {first}"
        );
    }
    let spans = message_spans(h, flow, message_id);
    assert_eq!(
        spans.len(),
        1,
        "{flow} request {message_id} handled more than once: {spans:#?}"
    );
}

/// Asserts that no message span of `flow` records a reply.
fn assert_never_replied(h: &Harness, flow: &str) {
    let flow_field = format!("\"flow\":\"{flow}\"");
    let replied: Vec<String> = h
        .logs()
        .into_iter()
        .filter(|line| {
            [SPAN_LINE, MESSAGE_SPAN, flow_field.as_str(), REPLIED]
                .iter()
                .all(|needle| line.contains(needle))
        })
        .collect();
    assert!(replied.is_empty(), "{flow} replied: {replied:#?}");
}

/// The `fields` object of the JSON log event `line`: the event's own fields,
/// without those of the spans it was logged in, which the formatter writes
/// after it, from `target` on.
fn event_fields(line: &str) -> &str {
    line.split_once("\"fields\":{")
        .and_then(|(_, rest)| rest.split_once("},\"target\":"))
        .map_or_else(
            || panic!("not a JSON log event: {line}"),
            |(fields, _)| fields,
        )
}

/// Asserts that `flow` logged one `ERROR` event for the request `message_id`
/// rejected once its record was published, whose own fields name the
/// request's IDs and the error queue and hold the `MQ-ERR-DISPLAY` group of
/// `expected`, and that `body`, the request body, appears nowhere in its line
/// but inside that group.
async fn assert_rejection_logged(
    h: &Harness,
    flow: &str,
    expected: &ExpectedRecord,
    message_id: &str,
    correlation_id: &str,
    body: &str,
) {
    let id_field = format!("\"message_id\":\"{message_id}\"");
    let line = h
        .wait_for_log(&["\"level\":\"ERROR\"", REJECTED_EVENT, &id_field], ARRIVAL)
        .await;
    let display = format!(
        "\"mq_err_display\":\"{}\"",
        String::from_utf8_lossy(&expected.bytes[..ERR_DISPLAY_LEN])
    );
    let fields = event_fields(&line);
    for field in [
        format!("\"flow\":\"{flow}\""),
        id_field.clone(),
        format!("\"correlation_id\":\"{correlation_id}\""),
        format!("\"queue\":\"{ERROR_QUEUE}\""),
        display.clone(),
    ] {
        assert!(
            fields.contains(&field),
            "the rejection event of {message_id} lacks {field}: {line}"
        );
    }
    assert!(
        !line.replace(&display, "").contains(body),
        "the rejection event of {message_id} carries its body: {line}"
    );
    let events = h
        .logs()
        .into_iter()
        .filter(|line| line.contains(REJECTED_EVENT) && line.contains(&id_field))
        .count();
    assert_eq!(events, 1, "rejection events of {message_id}");
}

/// Asserts that `flow` logged the `reply not confirmed` warning of the
/// request `message_id`, whose reply was returned, and that the warning's own
/// fields name the request by its IDs.
async fn assert_reply_not_confirmed(
    h: &Harness,
    flow: &str,
    message_id: &str,
    correlation_id: &str,
) {
    let id_field = format!("\"message_id\":\"{message_id}\"");
    let line = h
        .wait_for_log(
            &[
                "\"level\":\"WARN\"",
                "\"message\":\"reply not confirmed\"",
                &id_field,
            ],
            ARRIVAL,
        )
        .await;
    let fields = event_fields(&line);
    for field in [
        format!("\"flow\":\"{flow}\""),
        id_field.clone(),
        format!("\"correlation_id\":\"{correlation_id}\""),
        "\"failure\":\"returned unroutable\"".to_owned(),
    ] {
        assert!(
            fields.contains(&field),
            "the reply-not-confirmed warning of {message_id} lacks {field}: {line}"
        );
    }
}

/// The request queue the settings name for `queue`, which must be the local
/// default `expected`.
fn request_queue(queue: &str, expected: &str) -> String {
    assert_eq!(queue, expected, "request queue setting");
    queue.to_owned()
}

/// CODATE01 4100-PUT-REPLY `WHEN OTHER`: the message `9000-ERROR` puts for a
/// reply put that failed. `MQ-ERROR-PARA` keeps the spaces of the
/// `INITIALIZE`, the return message is `MQPUT ERR`, the codes are those E-39
/// proposes for a returned publish, and the queue is `REPLY-QUEUE-NAME`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L396-402].
fn cdrd_put_failure_message() -> ExpectedRecord {
    assert!(
        audit_row("P-CODATE01-022").contains("MQ-ERROR-PARA spaces (from the INITIALIZE)"),
        "P-CODATE01-022 no longer records the INITIALIZEd MQ-ERROR-PARA"
    );
    let [return_message] = texts::<1>("P-CODATE01-015");
    let [reply_queue] = texts::<1>("P-CODATE01-010");
    expected_error_message(
        CODATE01_ERR_DISPLAY,
        [
            "",
            "",
            &return_message,
            "",
            PUBLISH_FAILED_CONDITION,
            "",
            PUBLISH_FAILED_REASON,
            "",
            &reply_queue,
        ],
    )
}

/// CDRD: a reply the broker returns as unroutable is no reply. The request
/// gets the `MQPUT ERR` record on `CARD.DEMO.ERROR` and is rejected without
/// requeue, never acknowledged and never redelivered.
///
/// Proves `P-CODATE01-014` (the reply goes to the fixed queue, the request's
/// reply-to unused), `P-CODATE01-015` (the `MQPUT ERR` record, then
/// termination: request consumed, record delivered, no reply),
/// `P-CODATE01-016` (`9000-ERROR`'s 1000-byte message), `P-CODATE01-024` (the
/// request consumed after a put failure; acknowledged only after a confirm),
/// the layout rows `P-CODATE01-001` to `P-CODATE01-009`, `P-CODATE01-022`
/// (`MQ-ERROR-PARA` left spaces) and `P-CODATE01-019` (E-39 codes). The
/// `MQCLOSE` paths of `P-CODATE01-017` have no counterpart (E-22,
/// `P-CODATE01-018`).
#[test]
fn cdrd_returned_reply_publishes_error_record_and_rejects() {
    block_on(async {
        assert_cited(&[
            "P-CODATE01-010",
            "P-CODATE01-011",
            "P-CODATE01-014",
            "P-CODATE01-015",
            "P-CODATE01-016",
            "P-CODATE01-017",
            "P-CODATE01-019",
            "P-CODATE01-022",
            "P-CODATE01-024",
        ]);
        assert!(
            audit_row("P-CODATE01-018").contains("E-22"),
            "P-CODATE01-018 no longer links E-22"
        );
        assert_proposed_codes();
        assert_eq!(texts::<1>("P-CODATE01-010"), ["CARD.DEMO.REPLY.DATE"]);
        assert_eq!(cdrd::REPLY_QUEUE, "CARD.DEMO.REPLY.DATE");
        assert_eq!(texts::<1>("P-CODATE01-011"), [ERROR_QUEUE]);
        let expected = cdrd_put_failure_message();

        let h = Harness::start(None).await;
        let queue = request_queue(&h.settings.queue_date_request, DATE_REQUEST_QUEUE);
        let decoy = h.temp_reply_queue().await;
        delete_queue(&h, cdrd::REPLY_QUEUE).await;

        h.publish(
            &queue,
            b"DATE",
            CDRD_RETURNED,
            "ACK-CDRD-CORREL-1",
            Some(&decoy),
        )
        .await;
        let record = h.next_message(ERROR_QUEUE, ARRIVAL).await;
        assert_error_message(&record.data, &expected, CDRD_RETURNED);
        assert_eq!(
            h.queue_depth(&queue).await,
            0,
            "{CDRD_RETURNED} left on {queue}"
        );
        // The reply-to is not used: nothing reaches it, not even a redirected reply.
        h.assert_no_message(&decoy, SETTLE).await;
        h.assert_no_message(ERROR_QUEUE, SETTLE).await;
        assert_eq!(
            h.queue_depth(&queue).await,
            0,
            "{CDRD_RETURNED} redelivered to {queue}"
        );

        // Prefetch 1: this request is delivered only once the first is settled.
        h.publish(&queue, b"DATE", CDRD_AFTER, "ACK-CDRD-CORREL-2", None)
            .await;
        let record = h.next_message(ERROR_QUEUE, ARRIVAL).await;
        assert_error_message(&record.data, &expected, CDRD_AFTER);
        h.assert_no_message(ERROR_QUEUE, SETTLE).await;

        for message_id in [CDRD_RETURNED, CDRD_AFTER] {
            assert_single_span(&h, "CDRD", message_id, &REJECTED_WITH_RECORD).await;
        }
        for (message_id, correlation_id) in [
            (CDRD_RETURNED, "ACK-CDRD-CORREL-1"),
            (CDRD_AFTER, "ACK-CDRD-CORREL-2"),
        ] {
            assert_reply_not_confirmed(&h, "CDRD", message_id, correlation_id).await;
            assert_rejection_logged(&h, "CDRD", &expected, message_id, correlation_id, "DATE")
                .await;
        }
        assert_never_replied(&h, "CDRD");
        h.shutdown().await;
    });
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY `WHEN OTHER`: the message `9000-ERROR`
/// puts for an account read that failed with a `RESP` other than `NOTFND`.
/// `MQ-ERROR-PARA` keeps the spaces of the `INITIALIZE`, the return message is
/// `'ERROR WHILE READING ACCTFILE'` cut to `X(25)`, the codes are those E-39
/// proposes for a read error, and the queue is `INPUT-QUEUE-NAME`, the queue
/// the request was put to [app/app-vsam-mq/cbl/COACCT01.cbl:L439-443].
fn cdra_read_error_message(input_queue: &str) -> ExpectedRecord {
    assert!(
        audit_row("P-COACCT01-030").contains("MQ-ERROR-PARA spaces (from the INITIALIZE)"),
        "P-COACCT01-030 no longer records the INITIALIZEd MQ-ERROR-PARA"
    );
    assert!(
        audit_row("P-COACCT01-015").contains("queue=input queue"),
        "P-COACCT01-015 no longer names the input queue"
    );
    let [return_message] = texts::<1>("P-COACCT01-015");
    expected_error_message(
        COACCT01_ERR_DISPLAY,
        [
            "",
            "",
            &return_message,
            "",
            READ_ERROR_CONDITION,
            "",
            READ_ERROR_REASON,
            "",
            input_queue,
        ],
    )
}

/// COACCT01 4100-PUT-REPLY `WHEN OTHER`: the message `9000-ERROR` puts for a
/// reply put that failed, as for CDRD, naming `CARD.DEMO.REPLY.ACCT`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L492-498].
fn cdra_put_failure_message() -> ExpectedRecord {
    let [return_message] = texts::<1>("P-COACCT01-017");
    let [reply_queue, _error_queue] = texts::<2>("P-COACCT01-010");
    expected_error_message(
        COACCT01_ERR_DISPLAY,
        [
            "",
            "",
            &return_message,
            "",
            PUBLISH_FAILED_CONDITION,
            "",
            PUBLISH_FAILED_REASON,
            "",
            &reply_queue,
        ],
    )
}

/// Two inquiries whose account read fails: each gets its read-error record and
/// no reply, the second although it names a reply-to.
async fn cdra_read_errors(h: &Harness, queue: &str) {
    let expected = cdra_read_error_message(queue);
    let inquiry = audited_request("P-COACCT01-020");
    h.publish(
        queue,
        inquiry.as_bytes(),
        CDRA_READ_ERROR,
        "ACK-CDRA-CORREL-1",
        None,
    )
    .await;
    let record = h.next_message(ERROR_QUEUE, ARRIVAL).await;
    assert_error_message(&record.data, &expected, CDRA_READ_ERROR);
    h.assert_no_message(cdra::REPLY_QUEUE, SETTLE).await;
    assert_eq!(
        h.queue_depth(queue).await,
        0,
        "{CDRA_READ_ERROR} left on {queue}"
    );
    h.assert_no_message(ERROR_QUEUE, SETTLE).await;

    // Prefetch 1: this request is delivered only once the first is settled.
    let decoy = h.temp_reply_queue().await;
    h.publish(
        queue,
        inquiry.as_bytes(),
        CDRA_READ_ERROR_AGAIN,
        "ACK-CDRA-CORREL-2",
        Some(&decoy),
    )
    .await;
    let record = h.next_message(ERROR_QUEUE, ARRIVAL).await;
    assert_error_message(&record.data, &expected, CDRA_READ_ERROR_AGAIN);
    h.assert_no_message(&decoy, SETTLE).await;
    h.assert_no_message(cdra::REPLY_QUEUE, SETTLE).await;
    h.assert_no_message(ERROR_QUEUE, SETTLE).await;
    assert_eq!(
        h.queue_depth(queue).await,
        0,
        "a CDRA request redelivered to {queue}"
    );
    for message_id in [CDRA_READ_ERROR, CDRA_READ_ERROR_AGAIN] {
        assert_single_span(h, "CDRA", message_id, &REJECTED_WITH_RECORD).await;
    }
    for (message_id, correlation_id) in [
        (CDRA_READ_ERROR, "ACK-CDRA-CORREL-1"),
        (CDRA_READ_ERROR_AGAIN, "ACK-CDRA-CORREL-2"),
    ] {
        assert_rejection_logged(h, "CDRA", &expected, message_id, correlation_id, &inquiry).await;
    }
}

/// A request that reads no account, here an unknown function, reaches
/// `4100-PUT-REPLY`; with the reply queue deleted its reply is returned and it
/// gets the `MQPUT ERR` record instead.
async fn cdra_returned_reply(h: &Harness, queue: &str) {
    let expected = cdra_put_failure_message();
    delete_queue(h, cdra::REPLY_QUEUE).await;
    let request = audited_request("P-COACCT01-027");
    h.publish(
        queue,
        request.as_bytes(),
        CDRA_RETURNED,
        "ACK-CDRA-CORREL-3",
        None,
    )
    .await;
    let record = h.next_message(ERROR_QUEUE, ARRIVAL).await;
    assert_error_message(&record.data, &expected, CDRA_RETURNED);
    h.assert_no_message(ERROR_QUEUE, SETTLE).await;
    assert_eq!(
        h.queue_depth(queue).await,
        0,
        "{CDRA_RETURNED} left on {queue}"
    );
    assert_single_span(h, "CDRA", CDRA_RETURNED, &REJECTED_WITH_RECORD).await;
    assert_reply_not_confirmed(h, "CDRA", CDRA_RETURNED, "ACK-CDRA-CORREL-3").await;
    assert_rejection_logged(
        h,
        "CDRA",
        &expected,
        CDRA_RETURNED,
        "ACK-CDRA-CORREL-3",
        &request,
    )
    .await;
}

/// CDRA: an account read that fails with anything but `NOTFND` gets no reply.
/// Its `ERROR WHILE READING ACCTF` record goes to `CARD.DEMO.ERROR` and the
/// request is rejected without requeue, never redelivered. A reply the broker
/// returns afterwards gets the `MQPUT ERR` record the same way.
///
/// The read error is a dropped `accounts` table, which fails the read when it
/// executes; a revoked grant would not, since the stack's database user is a
/// superuser. The requests are those of `P-COACCT01-020` and `P-COACCT01-027`.
///
/// Proves `P-COACCT01-012` (an `INQA` inquiry above zero reads the account),
/// `P-COACCT01-015` (the `WHEN OTHER` read error: the record, then
/// termination with no reply), `P-COACCT01-016` (an unknown function is
/// answered without a read), `P-COACCT01-017` (the `MQPUT ERR` record),
/// `P-COACCT01-018` (`9000-ERROR`'s 1000-byte message), `P-COACCT01-032` (the
/// request consumed after a read or put failure; acknowledged only after a
/// confirm), the layout rows of [`COACCT01_ERR_DISPLAY`] (each field of both
/// records at its row's Offset and Length, of its Input condition's class and
/// its `rust=` type, each `FILLER` spaces), `P-COACCT01-010` (the fixed reply
/// and error queues) and `P-COACCT01-030` (`MQ-ERROR-PARA` left spaces). The
/// `MQCLOSE` paths of `P-COACCT01-019` have no counterpart (E-22,
/// `P-COACCT01-029`).
#[test]
fn cdra_read_error_publishes_error_record_without_reply() {
    block_on(async {
        assert_cited(&[
            "P-COACCT01-010",
            "P-COACCT01-012",
            "P-COACCT01-015",
            "P-COACCT01-016",
            "P-COACCT01-017",
            "P-COACCT01-018",
            "P-COACCT01-019",
            "P-COACCT01-020",
            "P-COACCT01-027",
            "P-COACCT01-030",
            "P-COACCT01-032",
        ]);
        assert!(
            audit_row("P-COACCT01-029").contains("E-22"),
            "P-COACCT01-029 no longer links E-22"
        );
        assert_proposed_codes();
        assert_eq!(
            texts::<2>("P-COACCT01-010"),
            ["CARD.DEMO.REPLY.ACCT", ERROR_QUEUE]
        );
        assert_eq!(cdra::REPLY_QUEUE, "CARD.DEMO.REPLY.ACCT");

        let h = Harness::start(None).await;
        let queue = request_queue(&h.settings.queue_acct_request, ACCT_REQUEST_QUEUE);
        // Before any CDRA request, so no read has run on the table.
        h.stack.exec_sql("DROP TABLE accounts CASCADE").await;
        cdra_read_errors(&h, &queue).await;
        cdra_returned_reply(&h, &queue).await;
        assert_never_replied(&h, "CDRA");
        h.shutdown().await;
    });
}

/// The audited approve request of `P-COPAUA0C-021` with the card of account
/// `00000000001` in place of its own: a request that would be approved, whose
/// account no other request of the test touches.
fn account_1_request(approved: &str) -> String {
    let mut fields: Vec<&str> = approved.split(',').collect();
    assert_eq!(
        fields.len(),
        CP00_REQUEST_FIELDS,
        "P-COPAUA0C-021 request: {approved}"
    );
    fields[CP00_CARD_FIELD] = ACCOUNT_1_CARD;
    fields.join(",")
}

/// COPAUA0C 7100-SEND-RESPONSE: waits for the `M004` record of the request
/// `message_id` and asserts its `CCPAUERY` fields: critical, MQ subsystem,
/// the `FAILED TO PUT ON REPLY MQ` message, application `CP00`, program
/// `COPAUA0C` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L767-777].
/// The record's own fields name the request by `message_id` and
/// `correlation_id`, outside its message span.
async fn assert_m004_logged(h: &Harness, message_id: &str, correlation_id: &str) {
    assert!(
        audit_row("P-COPAUA0C-012").contains("M004 critical"),
        "P-COPAUA0C-012 no longer records the critical M004 record"
    );
    assert!(
        audit_row("P-COPAUA0C-015").contains("application 'CP00', program 'COPAUA0C'"),
        "P-COPAUA0C-015 no longer records the application and program"
    );
    let [message] = texts::<1>("P-COPAUA0C-012");
    let message_field = format!("\"err_message\":\"{message}\"");
    let id_field = format!("\"message_id\":\"{message_id}\"");
    let location = "\"err_location\":\"M004\"";
    let line = h
        .wait_for_log(&[location, &message_field, &id_field], ARRIVAL)
        .await;
    let critical = ccpauery_value("ERR-CRITICAL");
    let mq = ccpauery_value("ERR-MQ");
    for field in [
        format!("\"err_level\":\"{critical}\""),
        format!("\"err_subsystem\":\"{mq}\""),
        "\"err_application\":\"CP00\"".to_owned(),
        "\"err_program\":\"COPAUA0C\"".to_owned(),
    ] {
        assert!(
            line.contains(&field),
            "the M004 record lacks {field}: {line}"
        );
    }
    let fields = event_fields(&line);
    for field in [id_field, format!("\"correlation_id\":\"{correlation_id}\"")] {
        assert!(
            fields.contains(&field),
            "the M004 record does not itself name {field}: {line}"
        );
    }
}

/// Asserts that account `00000000001` has no pending authorization: no
/// summary (`GE`) and no detail.
async fn assert_no_pending_authorization(h: &Harness) {
    let account: AccountId = ACCOUNT_1
        .parse()
        .unwrap_or_else(|error| panic!("account {ACCOUNT_1}: {error:?}"));
    let mut uow =
        h.db.begin()
            .await
            .expect("begin a read of the authorizations");
    let (summary, details) = {
        let mut store = uow.store();
        let summary = store.read_summary(account).await;
        let browse = Browse {
            start_key: None,
            direction: Direction::Forward,
            limit: 100,
        };
        let details = store.details_for_account(account, browse).await;
        (summary, details)
    };
    uow.rollback()
        .await
        .expect("end the read of the authorizations");
    assert!(
        matches!(summary, Err(PortError::NotFound)),
        "a summary was written for {ACCOUNT_1}: {summary:?}"
    );
    assert!(
        matches!(&details, Ok(rows) if rows.is_empty()),
        "details were written for {ACCOUNT_1}: {details:?}"
    );
}

/// Asserts that `body` is the audited reply of `P-COPAUA0C-021`: 64 bytes,
/// the first 63 as audited; the last one `COPAUA0C` never sets (E-25).
fn assert_audited_reply(body: &[u8]) {
    let [reply] = texts::<1>("P-COPAUA0C-021");
    let set = reply
        .strip_suffix(UNSET_REPLY_BYTE)
        .unwrap_or_else(|| panic!("P-COPAUA0C-021 reply does not end unset: {reply:?}"));
    assert_eq!(body.len(), set.len() + 1, "reply length");
    assert_eq!(
        String::from_utf8_lossy(&body[..set.len()]),
        set,
        "reply text"
    );
}

/// CP00: a reply the broker returns, because the request's reply-to names no
/// queue, is no reply. The critical `M004` record is logged, the request is
/// rejected without requeue, never redelivered, and nothing is written: the
/// pending-authorization writes run only after an acknowledgement.
///
/// A later approve request, answered to a queue that exists, shows the
/// consumer settled the first one (prefetch 1) and goes on serving.
///
/// Proves `P-COPAUA0C-002` (the request consumed at get time),
/// `P-COPAUA0C-012` (`7100-SEND-RESPONSE`: `M004` critical, the task ends, no
/// database write), `P-COPAUA0C-015` (the `CCPAUERY` record logged as JSON),
/// `P-COPAUA0C-035` (the writes follow the reply's confirm and the request's
/// acknowledgement) and `P-COPAUA0C-021` (the approve reply).
#[test]
fn cp00_failed_reply_logs_m004_and_writes_nothing() {
    block_on(async {
        assert_cited(&[
            "P-COPAUA0C-002",
            "P-COPAUA0C-012",
            "P-COPAUA0C-015",
            "P-COPAUA0C-021",
            "P-COPAUA0C-035",
        ]);
        let approved = audited_request("P-COPAUA0C-021");
        let failing = account_1_request(&approved);

        let h = Harness::start(None).await;
        let queue = request_queue(&h.settings.queue_pauth_request, PAUTH_REQUEST_QUEUE);
        let correlation = "ACK-CP00-CORREL-1";
        let reply_to = Some(MISSING_REPLY_QUEUE);
        h.publish(
            &queue,
            failing.as_bytes(),
            CP00_RETURNED,
            correlation,
            reply_to,
        )
        .await;
        assert_reply_not_confirmed(&h, "CP00", CP00_RETURNED, correlation).await;
        assert_m004_logged(&h, CP00_RETURNED, correlation).await;
        assert_eq!(
            h.queue_depth(&queue).await,
            0,
            "{CP00_RETURNED} left on {queue}"
        );
        assert_no_pending_authorization(&h).await;

        let decoy = h.temp_reply_queue().await;
        h.publish(
            &queue,
            approved.as_bytes(),
            CP00_ANSWERED,
            "ACK-CP00-CORREL-2",
            Some(&decoy),
        )
        .await;
        let reply = h.next_message(&decoy, ARRIVAL).await;
        assert_audited_reply(&reply.data);
        h.assert_no_message(&decoy, SETTLE).await;
        assert_eq!(
            h.queue_depth(&queue).await,
            0,
            "a CP00 request redelivered to {queue}"
        );

        assert_single_span(&h, "CP00", CP00_RETURNED, &CP00_REJECTED_M004).await;
        let replied = [REPLIED, "\"location\":\"none\""];
        assert_single_span(&h, "CP00", CP00_ANSWERED, &replied).await;
        assert_no_pending_authorization(&h).await;
        let write_failures: Vec<String> = h
            .logs()
            .into_iter()
            .filter(|line| {
                ["\"err_location\":\"I003\"", "\"err_location\":\"I004\""]
                    .iter()
                    .any(|needle| line.contains(needle))
            })
            .collect();
        assert!(
            write_failures.is_empty(),
            "write failures logged: {write_failures:#?}"
        );
        h.shutdown().await;
    });
}

/// CP00: a requester on `RabbitMQ` direct reply-to gets no reply. The broker
/// would deliver a mandatory reply to that pseudo-queue and still return it
/// as unroutable, so the reply would reach the requester while the request
/// went unwritten. CP00 publishes nothing to it: a warning names the
/// pseudo-queue, the critical `M004` record is logged, the request is
/// rejected without requeue, never redelivered, and nothing is written.
///
/// The requester consumes [`DIRECT_REPLY_TO`] with `no_ack` on the test's
/// channel and publishes the request on that channel, as the broker requires,
/// so the request reaches CP00 with the `reply_to` the broker rewrote.
///
/// Proves `P-COPAUA0C-002` (the request consumed at get time),
/// `P-COPAUA0C-012` (`7100-SEND-RESPONSE`: `M004` critical, no reply, no
/// database write), `P-COPAUA0C-015` (the `CCPAUERY` record logged as JSON)
/// and `P-COPAUA0C-035` (no write without a confirmed reply).
#[test]
fn cp00_direct_reply_to_logs_m004_without_a_reply_or_a_write() {
    block_on(async {
        assert_cited(&[
            "P-COPAUA0C-002",
            "P-COPAUA0C-012",
            "P-COPAUA0C-015",
            "P-COPAUA0C-021",
            "P-COPAUA0C-035",
        ]);
        let approvable = account_1_request(&audited_request("P-COPAUA0C-021"));

        let h = Harness::start(None).await;
        let queue = request_queue(&h.settings.queue_pauth_request, PAUTH_REQUEST_QUEUE);
        let mut replies = h
            .channel
            .basic_consume(
                DIRECT_REPLY_TO,
                "ack-cp00-direct-reply-to",
                BasicConsumeOptions {
                    no_ack: true,
                    ..BasicConsumeOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("cannot consume {DIRECT_REPLY_TO}: {error}"));
        h.publish(
            &queue,
            approvable.as_bytes(),
            CP00_DIRECT_REPLY_TO,
            "ACK-CP00-CORREL-3",
            Some(DIRECT_REPLY_TO),
        )
        .await;

        let id_field = format!("\"message_id\":\"{CP00_DIRECT_REPLY_TO}\"");
        let rewritten = format!("\"reply_to\":\"{DIRECT_REPLY_TO}.");
        let warning = h
            .wait_for_log(&["\"level\":\"WARN\"", &id_field, &rewritten], ARRIVAL)
            .await;
        assert!(
            warning.contains("RabbitMQ direct reply-to is not supported"),
            "the direct reply-to warning: {warning}"
        );
        assert!(
            !warning.contains(ACCOUNT_1_CARD),
            "the direct reply-to warning carries the card: {warning}"
        );
        assert_m004_logged(&h, CP00_DIRECT_REPLY_TO, "ACK-CP00-CORREL-3").await;

        // CP00 has no error queue, and nothing reaches it; the wait also gives
        // a reply time to reach the requester.
        h.assert_no_message(ERROR_QUEUE, SETTLE).await;
        let reply = replies.next().now_or_never();
        assert!(
            reply.is_none(),
            "the direct reply-to requester got a reply: {reply:?}"
        );
        assert_eq!(
            h.queue_depth(&queue).await,
            0,
            "{CP00_DIRECT_REPLY_TO} left on or redelivered to {queue}"
        );
        assert_single_span(&h, "CP00", CP00_DIRECT_REPLY_TO, &CP00_REJECTED_M004).await;
        assert_no_pending_authorization(&h).await;
        assert_never_replied(&h, "CP00");
        h.shutdown().await;
    });
}
