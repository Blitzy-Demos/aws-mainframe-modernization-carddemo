//! CDRA (`COACCT01`) parity: the account-inquiry replies the consumer puts to
//! `CARD.DEMO.REPLY.ACCT` equal, byte for byte, the replies the audited
//! `COACCT01` rules give for the seeded fixture accounts.
//!
//! Every expected reply is the section D.9 row of
//! `docs/parity/parity-oracle-audit.md` for its request, read through
//! `common::audit_texts`; nothing is built from the code under test. Each D.9
//! row is first checked against the section B rule it instantiates and the
//! fixture record it cites, so a drifted audit row fails before any request is
//! sent:
//!
//! - `P-COACCT01-020` to `P-COACCT01-025`: an `INQA` inquiry of an existing
//!   account answers `WS-ACCT-RESPONSE` (`P-COACCT01-013`), built from the
//!   record of `app/data/ASCII/acctdata.txt` at its `CVACT01Y` offsets;
//! - `P-COACCT01-026`: an `INQA` inquiry of a missing account answers the
//!   `NOTFND` text (`P-COACCT01-014`);
//! - `P-COACCT01-027` and `P-COACCT01-028`: another function, or a zero key,
//!   answers the invalid-request text (`P-COACCT01-016`) without a read
//!   (`P-COACCT01-012`).
//!
//! The live test needs a Docker daemon: `Harness::start` runs a seeded
//! `PostgreSQL`, a `RabbitMQ` broker loaded with `infra/rabbitmq` and the
//! consumers. It also proves the fixed reply queue and the identifier echo
//! (`P-COACCT01-010`, `P-COACCT01-017`), the 15-byte request layout
//! (`P-COACCT01-011`) and single settlement after a confirmed reply
//! (`P-COACCT01-032`): each request is acknowledged once, gets one persistent
//! reply without expiry and one `replied` message span, and nothing reaches
//! `CARD.DEMO.ERROR`.
//!
//! The failure rows are proven by `ack_after_confirm.rs`: the `MQ-ERR-DISPLAY`
//! layout, the read error and the `MQPUT ERR` record. The `MQOPEN` and
//! `MQCLOSE` failure paths are not reproduced (E-22): the consumer opens and
//! closes no queue per request, so `P-COACCT01-019`, `P-COACCT01-029`,
//! `P-COACCT01-030` and `P-COACCT01-031` are only checked to exist here. A key
//! holding non-digits stays at `halt` (E-28) and is never sent.

pub mod common;

use std::time::Duration;

use carddemo_messaging::error_queue::ERROR_QUEUE;
use carddemo_messaging::flows::cdra::REPLY_QUEUE;
use carddemo_testkit::{block_on, fixtures};
use common::{Harness, audit_row, audit_texts};
use lapin::message::Delivery;
use lapin::types::ShortString;

/// How long a reply may take to arrive.
const REPLY_WAIT: Duration = Duration::from_secs(30);

/// How long a queue is watched to show that nothing more arrives.
const ABSENCE_WAIT: Duration = Duration::from_secs(3);

/// How long a message span may take to be written after its reply.
const SPAN_WAIT: Duration = Duration::from_secs(30);

/// COACCT01 1000-CONTROL — `P-COACCT01-010`: the reply and error queue
/// literals [app/app-vsam-mq/cbl/COACCT01.cbl:L198]
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L294].
const QUEUE_ROW: &str = "P-COACCT01-010";

/// COACCT01 3000-GET-REQUEST — `P-COACCT01-011`: `REQUEST-MSG-COPY` is
/// `WS-FUNC X(04)` then `WS-KEY 9(11)` [app/app-vsam-mq/cbl/COACCT01.cbl:L109-112].
const REQUEST_ROW: &str = "P-COACCT01-011";

/// COACCT01 4000-PROCESS-REQUEST-REPLY — `P-COACCT01-012`: only `INQA` with a
/// key above zero reads `ACCTDAT`.
const READ_ROW: &str = "P-COACCT01-012";

/// COACCT01 4000-PROCESS-REQUEST-REPLY `NORMAL` — `P-COACCT01-013`: the
/// `WS-ACCT-RESPONSE` labels [app/app-vsam-mq/cbl/COACCT01.cbl:L130-169].
const NORMAL_ROW: &str = "P-COACCT01-013";

/// COACCT01 4000-PROCESS-REQUEST-REPLY `NOTFND` — `P-COACCT01-014`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L428-435].
const NOT_FOUND_ROW: &str = "P-COACCT01-014";

/// COACCT01 4000-PROCESS-REQUEST-REPLY `ELSE` — `P-COACCT01-016`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L448-457].
const INVALID_ROW: &str = "P-COACCT01-016";

/// COACCT01 4100-PUT-REPLY — `P-COACCT01-017`: the 1000-byte reply with the
/// request's message and correlation IDs [app/app-vsam-mq/cbl/COACCT01.cbl:L462-499].
const PUT_ROW: &str = "P-COACCT01-017";

/// COACCT01 4000-MAIN-PROCESS — `P-COACCT01-032`: one settlement per request,
/// the acknowledgement after the reply's confirm.
const MAIN_ROW: &str = "P-COACCT01-032";

/// Rows of paths the consumer has no counterpart for (E-22): `MQCLOSE`
/// (`P-COACCT01-019`, `P-COACCT01-029`) and `MQOPEN` (`P-COACCT01-030`,
/// `P-COACCT01-031`). They are not reproduced, so no behaviour is asserted.
const NOT_REPRODUCED_ROWS: [&str; 4] = [
    "P-COACCT01-019",
    "P-COACCT01-029",
    "P-COACCT01-030",
    "P-COACCT01-031",
];

/// The fixture file the seed loads into `accounts`.
const ACCTDATA: &str = "acctdata.txt";

/// `CVACT01Y` `ACCT-ID 9(11)` [app/cpy/CVACT01Y.cpy:L5], which is also the
/// `WS-KEY 9(11)` width [app/app-vsam-mq/cbl/COACCT01.cbl:L111].
const KEY_LEN: usize = 11;

/// `WS-FUNC X(04)` [app/app-vsam-mq/cbl/COACCT01.cbl:L110].
const FUNC_LEN: usize = 4;

/// The `ACCOUNT-RECORD` field each `WS-ACCT-RESPONSE` value is moved from, in
/// response order, as byte ranges of the record at its `CVACT01Y` offsets
/// [app/cpy/CVACT01Y.cpy:L5-16] [app/app-vsam-mq/cbl/COACCT01.cbl:L408-427]:
/// `ACCT-ID`, `ACCT-ACTIVE-STATUS`, `ACCT-CURR-BAL`, `ACCT-CREDIT-LIMIT`,
/// `ACCT-CASH-CREDIT-LIMIT`, `ACCT-OPEN-DATE`, `ACCT-EXPIRAION-DATE`,
/// `ACCT-REISSUE-DATE`, `ACCT-CURR-CYC-CREDIT`, `ACCT-CURR-CYC-DEBIT` and
/// `ACCT-GROUP-ID`. `ACCT-ADDR-ZIP` (102..112) is not part of the response.
const RESPONSE_FIELDS: [(usize, usize); 11] = [
    (0, 11),
    (11, 12),
    (12, 24),
    (24, 36),
    (36, 48),
    (48, 58),
    (58, 68),
    (68, 78),
    (78, 90),
    (90, 102),
    (112, 122),
];

/// The `COACCT01` branch a section D.9 row exercises.
#[derive(Clone, Copy, Debug)]
enum Branch {
    /// `INQA` read `NORMAL`: the account response.
    Account,
    /// `INQA` read `NOTFND`: the not-found text.
    NotFound,
    /// Not `INQA`, or a zero key: the invalid-request text.
    Invalid,
}

/// Section D.9: the CDRA requests and replies, in the order they are sent.
/// Each request is read from its row's input condition, so the other-function
/// case sends the audited `INQX`.
const REPLY_ROWS: [(&str, Branch); 9] = [
    ("P-COACCT01-020", Branch::Account),
    ("P-COACCT01-021", Branch::Account),
    ("P-COACCT01-022", Branch::Account),
    ("P-COACCT01-023", Branch::Account),
    ("P-COACCT01-024", Branch::Account),
    ("P-COACCT01-025", Branch::Account),
    ("P-COACCT01-026", Branch::NotFound),
    ("P-COACCT01-027", Branch::Invalid),
    ("P-COACCT01-028", Branch::Invalid),
];

/// One request of a section D.9 row and the reply the audit gives for it.
struct Case {
    /// The D.9 row.
    row: &'static str,
    /// The 15-byte request body.
    request: String,
    /// The reply body: the row's text padded to its `reply_length`.
    expected: Vec<u8>,
    /// The request's AMQP `message_id`, unique within the test.
    message_id: String,
    /// The request's AMQP `correlation_id`, unique within the test.
    correlation_id: String,
}

/// The exporter's marker of a span line, as `json_span_exporter` writes it.
const SPAN_LINE: &str = r#""type":"span""#;

/// The name of the span the consumer opens around each delivery.
const MESSAGE_SPAN: &str = r#""name":"message""#;

/// The `flow` attribute of a CDRA message span.
const CDRA_FLOW: &str = r#""flow":"CDRA""#;

/// The disposition of a request whose reply was confirmed.
const REPLIED: &str = r#""outcome":"replied""#;

/// The error-record attribute of a request that needed no error record.
const NO_ERROR_RECORD: &str = r#""error_record":"none""#;

/// The disposition of a request removed without a reply.
const REJECTED: &str = r#""outcome":"rejected""#;

/// Without Docker: the queue literals and the section B rows this file relies
/// on say what the tests assume (`P-COACCT01-010`, `P-COACCT01-011`,
/// `P-COACCT01-012`, `P-COACCT01-017`, `P-COACCT01-032`), the not-reproduced
/// rows exist (`P-COACCT01-019`, `P-COACCT01-029`, `P-COACCT01-030`,
/// `P-COACCT01-031`, E-22), and every section D.9 reply (`P-COACCT01-020` to
/// `P-COACCT01-028`) is the reply its rule (`P-COACCT01-013`,
/// `P-COACCT01-014`, `P-COACCT01-016`) gives against the fixture.
#[test]
fn audit_cdra_rows_agree_with_the_coacct01_rules_and_the_fixture() {
    assert_queue_literals();
    assert_rule_rows();
    assert_eq!(cases().len(), REPLY_ROWS.len());
}

/// Each section D.9 request (`P-COACCT01-020` to `P-COACCT01-028`), sent with
/// a decoy `reply_to`, gets exactly the audited 1000-byte reply on
/// `CARD.DEMO.REPLY.ACCT`, with its message and correlation IDs echoed, as a
/// persistent message without expiry (`P-COACCT01-010`, `P-COACCT01-017`).
/// The accounts are the seeded fixture accounts 1, 2, 10, 27, 49 and 50
/// (`P-COACCT01-013`); the missing account answers `NOTFND`
/// (`P-COACCT01-014`); another function or a zero key answers the
/// invalid-request text (`P-COACCT01-012`, `P-COACCT01-016`). Every request
/// is a 15-byte `REQUEST-MSG-COPY` (`P-COACCT01-011`), settled once after its
/// confirmed reply with one `replied` message span (`P-COACCT01-032`); nothing
/// reaches the decoy or `CARD.DEMO.ERROR`.
#[test]
fn cdra_replies_equal_the_audited_coacct01_replies() {
    assert_queue_literals();
    assert_rule_rows();
    let cases = cases();
    block_on(Box::pin(async {
        let h = Harness::start(None).await;
        let request_queue = h.settings.queue_acct_request.as_str();
        let decoy = h.temp_reply_queue().await;
        for case in &cases {
            exchange(&h, request_queue, &decoy, case).await;
        }
        assert_settled(&h, request_queue, &decoy, &cases).await;
        h.shutdown().await;
    }));
}

/// `P-COACCT01-010`: the flow's queues are the COBOL literals the audit
/// quotes, reply queue first.
fn assert_queue_literals() {
    let literals = audit_texts(QUEUE_ROW);
    assert_eq!(
        literals,
        ["CARD.DEMO.REPLY.ACCT", "CARD.DEMO.ERROR"],
        "audit row {QUEUE_ROW} quotes other queue literals"
    );
    assert_eq!(REPLY_QUEUE, literals[0], "CDRA reply queue");
    assert_eq!(ERROR_QUEUE, literals[1], "error queue");
}

/// The section B rows behind the assertions of the live test say what those
/// assertions assume, and the rows of paths without a counterpart exist.
fn assert_rule_rows() {
    let statements = [
        (REQUEST_ROW, "WS-FUNC X(04), WS-KEY 9(11)"),
        (READ_ROW, "function 'INQA' and key > 0"),
        (PUT_ROW, "reply=1000 bytes to CARD.DEMO.REPLY.ACCT"),
        (PUT_ROW, "correlation id echoed"),
        (MAIN_ROW, "acked only after its reply's publisher confirm"),
        (NOT_REPRODUCED_ROWS[0], "E-22"),
        (NOT_REPRODUCED_ROWS[1], "E-22"),
        (NOT_REPRODUCED_ROWS[2], "MQOPEN"),
        (NOT_REPRODUCED_ROWS[3], "MQOPEN"),
    ];
    for (row, statement) in statements {
        let line = audit_row(row);
        assert!(
            line.contains(statement),
            "audit row {row} no longer states {statement:?}: {line}"
        );
    }
}

/// The section D.9 cases in sending order, each checked against its rule and
/// the fixture before it is returned.
///
/// # Panics
///
/// Panics, naming the row, if a row lacks its request, `account_id` or
/// `reply_length`, holds more or fewer than one exact text, or gives a reply
/// other than the one its rule gives against `acctdata.txt`.
fn cases() -> Vec<Case> {
    let records = fixture_records();
    REPLY_ROWS
        .iter()
        .zip(1_u32..)
        .map(|(&(row, branch), number)| {
            let line = audit_row(row);
            let request = quoted_value(&line, "request", row);
            assert_request_layout(&request, row);
            if !matches!(branch, Branch::Invalid) {
                assert_eq!(
                    plain_value(&line, "account_id", row),
                    &request[FUNC_LEN..],
                    "audit row {row}: account_id is not the request's key"
                );
            }
            let length: usize = plain_value(&line, "reply_length", row)
                .parse()
                .unwrap_or_else(|error| panic!("audit row {row}: reply_length: {error}"));
            let mut texts = audit_texts(row);
            assert_eq!(texts.len(), 1, "audit row {row} holds {texts:?}");
            let expected = texts.remove(0).into_bytes();
            assert_eq!(expected.len(), length, "audit row {row}: reply width");
            let mut rule = rule_reply(branch, &request, &records, row);
            assert!(
                rule.len() <= length,
                "{row}: rule reply over {length} bytes"
            );
            // REPLY-MESSAGE is X(1000) and starts as spaces, so every reply is
            // padded to it [app/app-vsam-mq/cbl/COACCT01.cbl:L107]
            // [app/app-vsam-mq/cbl/COACCT01.cbl:L391].
            rule.resize(length, b' ');
            assert_bytes_eq(
                &format!("audit row {row} against its {branch:?} rule and {ACCTDATA}"),
                &expected,
                &rule,
            );
            Case {
                row,
                request,
                expected,
                message_id: format!("cdra-msg-{number:04}"),
                correlation_id: format!("cdra-corr-{number:04}"),
            }
        })
        .collect()
}

/// `P-COACCT01-011`: a request is `WS-FUNC X(04)` then `WS-KEY 9(11)`, 15
/// ASCII bytes, and every key sent here is display digits: a key holding other
/// bytes is at `halt` (E-28).
fn assert_request_layout(request: &str, row: &str) {
    assert_eq!(
        request.len(),
        FUNC_LEN + KEY_LEN,
        "audit row {row}: request {request:?} is not 15 bytes"
    );
    assert!(
        request.is_ascii(),
        "audit row {row}: request {request:?} is not ASCII"
    );
    assert!(
        request[FUNC_LEN..]
            .bytes()
            .all(|byte| byte.is_ascii_digit()),
        "audit row {row}: the key of {request:?} is not display digits"
    );
}

/// The reply the section B rule of `branch` gives for `request` against the
/// fixture `records`, before its padding to 1000 bytes, checked against the
/// length the rule row states. `row` is the D.9 row being checked.
fn rule_reply(branch: Branch, request: &str, records: &[Vec<u8>], row: &str) -> Vec<u8> {
    let (func, key) = request.split_at(FUNC_LEN);
    let found = records
        .iter()
        .position(|record| record.get(..KEY_LEN) == Some(key.as_bytes()));
    let (rule, reply) = match branch {
        Branch::Account => {
            let index =
                found.unwrap_or_else(|| panic!("audit row {row}: account {key} not in {ACCTDATA}"));
            assert_eq!(
                index + 1,
                cited_fixture_line(&audit_row(row), row),
                "audit row {row}: account {key} is not on the {ACCTDATA} line it cites"
            );
            (NORMAL_ROW, account_response(&records[index], row))
        }
        Branch::NotFound => {
            assert!(
                found.is_none(),
                "audit row {row}: account {key} is in {ACCTDATA}, so it is found"
            );
            let texts = rule_texts(NOT_FOUND_ROW, 2);
            let reply = [texts[0].as_str(), texts[1].as_str(), key].concat();
            (NOT_FOUND_ROW, reply.into_bytes())
        }
        Branch::Invalid => {
            assert!(
                func != "INQA" || key.bytes().all(|byte| byte == b'0'),
                "audit row {row}: {request:?} is an INQA inquiry with a key above zero"
            );
            let texts = rule_texts(INVALID_ROW, 3);
            let reply = [
                texts[0].as_str(),
                texts[1].as_str(),
                key,
                texts[2].as_str(),
                func,
            ]
            .concat();
            (INVALID_ROW, reply.into_bytes())
        }
    };
    assert_eq!(
        reply.len(),
        stated_length(rule),
        "the {rule} reply for {request:?} has another length than the row states"
    );
    reply
}

/// `P-COACCT01-013`: the `WS-ACCT-RESPONSE` of the fixture `record`, each
/// audited label followed by the record field moved after it.
fn account_response(record: &[u8], row: &str) -> Vec<u8> {
    let labels = rule_texts(NORMAL_ROW, RESPONSE_FIELDS.len());
    let mut reply = Vec::new();
    for (label, &(start, end)) in labels.iter().zip(&RESPONSE_FIELDS) {
        let field = record.get(start..end).unwrap_or_else(|| {
            panic!("audit row {row}: the {ACCTDATA} record ends before offset {end}")
        });
        reply.extend_from_slice(label.as_bytes());
        reply.extend_from_slice(field);
    }
    reply
}

/// The exact texts of the rule row `row`, which must hold `count` of them.
fn rule_texts(row: &str, count: usize) -> Vec<String> {
    let texts = audit_texts(row);
    assert_eq!(
        texts.len(),
        count,
        "audit row {row} holds {texts:?}, not {count} texts"
    );
    texts
}

/// The byte length the rule row `row` states as `(<n> bytes)`.
fn stated_length(row: &str) -> usize {
    let line = audit_row(row);
    line.match_indices(" bytes)")
        .find_map(|(at, _)| {
            let open = line[..at].rfind('(')?;
            line[open + 1..at].parse().ok()
        })
        .unwrap_or_else(|| panic!("audit row {row} states no (<n> bytes) length: {line}"))
}

/// The records of `acctdata.txt` in file order, without their line
/// terminators, read as stored: the fixture the seed loads into `accounts`.
fn fixture_records() -> Vec<Vec<u8>> {
    let path = fixtures::ascii_dir().join(ACCTDATA);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    bytes
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line).to_vec())
        .collect()
}

/// The `acctdata.txt` line the audit row `line` cites, `[…acctdata.txt:L<n>]`.
fn cited_fixture_line(line: &str, row: &str) -> usize {
    let marker = format!("{ACCTDATA}:L");
    let start = line
        .find(&marker)
        .unwrap_or_else(|| panic!("audit row {row} cites no {ACCTDATA} line"))
        + marker.len();
    let digits: String = line[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("audit row {row}: {ACCTDATA} line {digits:?}: {error}"))
}

/// The text of the `` key=`"…"` `` code span in the audit row `line`.
fn quoted_value(line: &str, key: &str, row: &str) -> String {
    let opener = format!("{key}=`\"");
    let start = line
        .find(&opener)
        .unwrap_or_else(|| panic!("audit row {row} holds no {key} code span"))
        + opener.len();
    let length = line[start..]
        .find("\"`")
        .unwrap_or_else(|| panic!("audit row {row}: the {key} code span is not closed"));
    let value = &line[start..start + length];
    assert!(
        !value.contains('\\'),
        "audit row {row}: the {key} value {value:?} holds an escape"
    );
    value.to_owned()
}

/// The plain `key=value` value in the audit row `line`: the text after a
/// `key=` that starts a word, up to the next `;`, space or `|`.
fn plain_value<'a>(line: &'a str, key: &str, row: &str) -> &'a str {
    let needle = format!("{key}=");
    let (at, _) = line
        .match_indices(&needle)
        .find(|&(at, _)| line[..at].ends_with([' ', '|', ';']))
        .unwrap_or_else(|| panic!("audit row {row} holds no {needle} value"));
    let rest = &line[at + needle.len()..];
    let end = rest.find([';', ' ', '|']).unwrap_or(rest.len());
    &rest[..end]
}

/// Asserts `actual == expected`, reporting the first differing offset and the
/// bytes from there on both sides.
fn assert_bytes_eq(context: &str, actual: &[u8], expected: &[u8]) {
    if actual == expected {
        return;
    }
    let (actual_len, expected_len) = (actual.len(), expected.len());
    let offset = actual
        .iter()
        .zip(expected)
        .position(|(left, right)| left != right)
        .unwrap_or(actual_len.min(expected_len));
    let window = |bytes: &[u8]| {
        let end = (offset + 40).min(bytes.len());
        String::from_utf8_lossy(&bytes[offset.min(end)..end]).into_owned()
    };
    let (found, wanted) = (window(actual), window(expected));
    panic!(
        "{context}: first difference at offset {offset} (lengths {actual_len} and \
         {expected_len}): {found:?} instead of {wanted:?}"
    );
}

/// Sends `case`'s request to `request_queue` with `decoy` as its ignored
/// `reply_to`, then takes its reply from `CARD.DEMO.REPLY.ACCT` and checks the
/// reply and its message span. One request is in flight at a time.
async fn exchange(h: &Harness, request_queue: &str, decoy: &str, case: &Case) {
    h.publish(
        request_queue,
        case.request.as_bytes(),
        &case.message_id,
        &case.correlation_id,
        Some(decoy),
    )
    .await;
    let reply = h.next_message(REPLY_QUEUE, REPLY_WAIT).await;
    assert_reply(case, &reply);
    assert_replied_span(h, case).await;
}

/// The reply is the case's audited body, carries the request's message and
/// correlation IDs [app/app-vsam-mq/cbl/COACCT01.cbl:L469-470], and is
/// persistent without expiry.
fn assert_reply(case: &Case, reply: &Delivery) {
    let Case { row, request, .. } = case;
    assert_bytes_eq(
        &format!("the reply to {request:?} ({row})"),
        &reply.data,
        &case.expected,
    );
    let properties = &reply.properties;
    assert_eq!(
        text(properties.message_id().as_ref()),
        Some(case.message_id.as_str()),
        "message_id of the reply to {request:?} ({row})"
    );
    assert_eq!(
        text(properties.correlation_id().as_ref()),
        Some(case.correlation_id.as_str()),
        "correlation_id of the reply to {request:?} ({row})"
    );
    assert_eq!(
        *properties.delivery_mode(),
        Some(2),
        "delivery_mode of the reply to {request:?} ({row})"
    );
    assert_eq!(
        text(properties.expiration().as_ref()),
        None,
        "expiration of the reply to {request:?} ({row})"
    );
}

/// The text of an AMQP short-string property.
fn text(value: Option<&ShortString>) -> Option<&str> {
    value.map(ShortString::as_str)
}

/// The `message_id` attribute of `case`'s message span, quoted as the
/// exporter writes it, so no ID matches as a prefix of another.
fn message_id_field(case: &Case) -> String {
    format!("\"message_id\":\"{}\"", case.message_id)
}

/// The CDRA message span of `case` is written with the request's correlation
/// ID, the `replied` outcome and no error record.
async fn assert_replied_span(h: &Harness, case: &Case) {
    let message_id = message_id_field(case);
    let span = h
        .wait_for_log(
            &[SPAN_LINE, MESSAGE_SPAN, CDRA_FLOW, &message_id],
            SPAN_WAIT,
        )
        .await;
    let correlation_id = format!("\"correlation_id\":\"{}\"", case.correlation_id);
    for field in [correlation_id.as_str(), REPLIED, NO_ERROR_RECORD] {
        assert!(
            span.contains(field),
            "the message span of {} lacks {field}: {span}",
            case.row
        );
    }
}

/// After every case: no second reply, nothing on the decoy or the error
/// queue, no request left on `request_queue`, and exactly one CDRA message
/// span per request (no redelivery), none of them rejected.
async fn assert_settled(h: &Harness, request_queue: &str, decoy: &str, cases: &[Case]) {
    h.assert_no_message(REPLY_QUEUE, ABSENCE_WAIT).await;
    h.assert_no_message(decoy, ABSENCE_WAIT).await;
    h.assert_no_message(ERROR_QUEUE, ABSENCE_WAIT).await;
    assert_eq!(
        h.queue_depth(request_queue).await,
        0,
        "requests left on {request_queue}"
    );
    let logs = h.logs();
    let cdra_spans: Vec<&String> = logs
        .iter()
        .filter(|line| {
            [SPAN_LINE, MESSAGE_SPAN, CDRA_FLOW]
                .iter()
                .all(|needle| line.contains(needle))
        })
        .collect();
    for case in cases {
        let message_id = message_id_field(case);
        let spans = cdra_spans
            .iter()
            .filter(|line| line.contains(&message_id))
            .count();
        assert_eq!(
            spans, 1,
            "CDRA message spans of {} ({})",
            case.message_id, case.row
        );
    }
    assert_eq!(
        cdra_spans.len(),
        cases.len(),
        "CDRA message spans: {cdra_spans:#?}"
    );
    let rejected: Vec<&String> = logs.iter().filter(|line| line.contains(REJECTED)).collect();
    assert!(rejected.is_empty(), "rejected requests: {rejected:#?}");
}
