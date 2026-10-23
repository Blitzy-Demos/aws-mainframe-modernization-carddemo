//! Cardholder Data Redaction for the three `RabbitMQ` consumers, CDRD, CDRA and CP00 (AAP
//! 0.3.4.8, E32): the messaging half of the proof whose HTTP half is `api/tests/redaction.rs`.
//!
//! Each test runs its worker in process against a `rabbitmq:3.13-management` testcontainer
//! booted from `rabbitmq/definitions.json` and, for CDRA and CP00, a fixture-loaded `postgres:16`
//! testcontainer, both from `test_support`, under `test_support::trace_capture`. It sends the
//! ordinary requests of its flow and the probes its audit row lists, then searches every body the
//! worker published, every `cssl_error_log` row as its encoded 122-byte record and the captured
//! trace for each protected value:
//!
//! - the 50 fixture card numbers, and the card number of the CP00 request no fixture holds;
//! - the 50 fixture SSNs, each with and without dashes;
//! - the fixture user passwords and the eight-digit numeric password the probes send.
//!
//! No CVV is a needle: three digits occur in any output. A CVV could show only as a field named
//! for it, so the test also finds no captured trace line with a JSON member, at any depth, whose
//! name holds `cvv` in any ASCII case, and no published body or encoded `cssl_error_log` record,
//! fixed-format text rather than JSON, holding a `cvv` label. It holds every CDRD and CDRA body to
//! its audited layout with spaces after the written bytes, and every CP00 reply to the six commas
//! of its written fields, so that no output carries a field its audit does not list.
//!
//! The workers key their spans and alert with the `CARDDEMO_AMQP_URL` they were opened with, and
//! CDRD and CP00 requests carry a fixture card number, a dashed fixture SSN and the fixture
//! password as their `message_id` and `correlation_id`: the trace must show each only as its
//! digest keyed by that URL, which the scan checks is present, and never as its text, and it
//! must never hold the URL.
//!
//! Request queues and `CARD.DEMO.DEADLETTER.PAUTH` are never searched: the test published those
//! bodies itself, and the broker, not a worker, moves a dead-lettered request.
//!
//! A failed search names the output, the kind of value and its index among the needles, never
//! the value, and no assertion prints a body or a field that could hold one. The CP00 reply taken
//! whole is HALTED(H6) (COPAUA0C-065): only fields inside its 63 written bytes are read, and
//! neither its length nor the whole body is asserted.

use std::error::Error;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use chrono::NaiveDate;
use copybook::FixedRecord;
use domain::messaging::{coacct01, codate01, copaua0c};
use domain::records::{CardXrefRecord, ErrorLogRecord};
use domain::{Clock, FixedClock};
use lapin::options::{ConfirmSelectOptions, QueueDeleteOptions};
use lapin::{BasicProperties, Channel, Connection};
use messaging::broker;
use messaging::workers::{Processed, Step, cdra, cdrd, cp00};
use persistence::{AccountRepository, CardXrefRepository, CsslErrorLogRepository, PgStore};
use runtime::config::Config;
use runtime::telemetry::{ALERT_TARGET, IdKey, ScreenedText};
use test_support::fixtures;
use test_support::rabbitmq::{self, RabbitMqGuard};
use test_support::trace_capture::{self, TraceCapture};
use tokio::time::{self, Instant};
use tracing::Dispatch;
use tracing::instrument::WithSubscriber as _;

/// The error of every helper and test: harness, broker, worker, store and timeout errors alike.
type BoxError = Box<dyn Error + Send + Sync>;

/// The result of every test.
type TestResult = Result<(), BoxError>;

/// Bound on every wait: a request being taken, a published message arriving, a broker answer.
const WAIT: Duration = Duration::from_secs(30);

/// Interval between two attempts to take the request just published.
const POLL: Duration = Duration::from_millis(25);

/// How long a reply queue that must stay empty is watched.
const QUIET: Duration = Duration::from_millis(500);

/// AMQP 0-9-1 `reply-success`, the code of the test's own connection close.
const REPLY_SUCCESS: u16 = 200;

/// The reason of every close the test makes.
const CLOSE_REASON: &str = "redaction test done";

/// An eight-digit numeric password the probes send: a requester-chosen value no fixture holds,
/// filling the 8-byte `SEC-USR-PWD PIC X(08)` (COACCT01-035, COPAUA0C-133).
const PROBE_PASSWORD: &str = "73914682";

/// A card number no fixture holds, the card of the CP00 request with no `card_xref` row (`A001`,
/// COPAUA0C-114).
const UNKNOWN_CARD: &str = "4000123412349990";

/// The `reply_to` of the CP00 request whose reply cannot be confirmed: no such queue exists, so
/// the mandatory publication comes back unroutable (`M004`, COPAUA0C-117).
const MISSING_REPLY_QUEUE: &str = "CARD.DEMO.REPLY.NO.SUCH.QUEUE";

/// Field 18 of the ordinary CP00 requests, which every reply echoes unchanged (COPAUA0C-062).
const ORDER_ID: &str = "ORDER-A";

/// Reply bytes 17-31 for [`ORDER_ID`]: the 15-byte `PA-RL-TRANSACTION-ID` as moved.
const ECHOED_ID: &[u8; 15] = b"ORDER-A        ";

/// Reply bytes 17-31 when field 18 holds eight or more consecutive non-space bytes
/// (COPAUA0C-062).
const WITHHELD_ID: &[u8; 15] = &[b'*'; 15];

/// Reply bytes 48-61 when the approved amount's edit holds eight or more consecutive digits
/// (COPAUA0C-064).
const WITHHELD_AMOUNT: &[u8; 14] = &[b'*'; 14];

/// The offsets of the six `,` the CP00 reply `STRING` writes, one after each of `CCPAURLY`'s six
/// fields, the last closing its [`copaua0c::REPLY_WRITTEN_LEN`] written bytes (COPAUA0C-060).
const REPLY_COMMAS: [usize; 6] = [16, 32, 39, 42, 47, copaua0c::REPLY_WRITTEN_LEN - 1];

/// CDRA reply bytes 37-47 of every not-found, bad-parameter and undecoded reply: the key
/// positions, masked (COACCT01-021, COACCT01-022, COACCT01-003).
const MASKED_KEY: &[u8; 11] = &[b'*'; 11];

// The fixture accounts whose cards the CP00 requests name; `cardxref.txt` holds one card per
// account, and every account decided under the one clock instant is distinct unless it is meant
// to meet `I004`.

/// Account 1 has 1,826.00 available (`acctdata.txt:1`), so it approves 100.00.
const APPROVAL_ACCOUNT: u64 = 1;
/// Account 2 has 4,550.00 available (`acctdata.txt:2`), so it declines 5,000.00 for
/// insufficient funds.
const DECLINE_ACCOUNT: u64 = 2;
/// Two approvals under one clock instant get one detail key, so the second takes `I004`
/// (COPAUA0C-099, COPAUA0C-120).
const DUPLICATE_KEY_ACCOUNT: u64 = 3;
/// The account of the request whose reply cannot be confirmed (`M004`).
const UNCONFIRMED_ACCOUNT: u64 = 4;
/// The account of the field-error request and of the unparseable request's card.
const FIELD_ERROR_ACCOUNT: u64 = 5;
/// The first of the five accounts, 6 to 10, of the field-18 probes, one each.
const FIRST_PROBE_ACCOUNT: u64 = 6;
/// The account of the approved request whose amount holds an SSN's nine digits.
const AMOUNT_PROBE_ACCOUNT: u64 = 11;

/// Field 1 of a CP00 request, `PA-RQ-AUTH-DATE X(06)` (COPAUA0C-002).
const AUTH_DATE: &str = "220618";
/// Field 2, `PA-RQ-AUTH-TIME X(06)`, which every reply echoes as its auth-ID code.
const AUTH_TIME: &str = "143012";
/// Fields 4 to 8 of a CP00 request.
const FIELDS_4_TO_8: [&str; 5] = ["0100", "0927", "0200", "POS", "000000"];
/// Fields 10 to 17 of a CP00 request.
const FIELDS_10_TO_17: [&str; 8] = [
    "5411",
    "840",
    "05",
    "MERCHANT0000001",
    "CORNER GROCERY",
    "SEATTLE",
    "WA",
    "98101",
];

/// What a protected value is, named in a failure in place of the value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Card,
    Ssn,
    Password,
}

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Card => "card number",
            Self::Ssn => "SSN",
            Self::Password => "password",
        }
    }
}

/// One protected value, with its kind and its index among the values of that kind.
struct Needle {
    kind: Kind,
    index: usize,
    bytes: Vec<u8>,
}

/// Every protected value the scans search for. Deliberately not `Debug`: nothing prints them.
struct Needles {
    entries: Vec<Needle>,
}

impl Needles {
    /// The fixture card numbers and [`UNKNOWN_CARD`], the fixture SSNs in both forms, and the
    /// fixture user passwords and [`PROBE_PASSWORD`].
    ///
    /// # Errors
    ///
    /// The harness error of a fixture that cannot be read, or an error when [`UNKNOWN_CARD`] is
    /// a fixture card number.
    fn fixture() -> Result<Self, BoxError> {
        let cards = fixtures::fixture_card_numbers()?;
        if cards.iter().any(|card| card == UNKNOWN_CARD) {
            return Err("the card number of the A001 request is a fixture card number".into());
        }
        let mut needles = Self {
            entries: Vec::new(),
        };
        for card in cards {
            needles.push(Kind::Card, card.into_bytes())?;
        }
        needles.push(Kind::Card, UNKNOWN_CARD.as_bytes().to_vec())?;
        for ssn in fixtures::fixture_ssn_forms()? {
            needles.push(Kind::Ssn, ssn.into_bytes())?;
        }
        for password in fixtures::fixture_user_passwords()? {
            needles.push(Kind::Password, password)?;
        }
        needles.push(Kind::Password, PROBE_PASSWORD.as_bytes().to_vec())?;
        Ok(needles)
    }

    /// Adds `bytes` as the next value of `kind`. An empty value would match nothing, so it is
    /// refused rather than skipped.
    fn push(&mut self, kind: Kind, bytes: Vec<u8>) -> Result<(), BoxError> {
        if bytes.is_empty() {
            return Err(format!("an empty {} needle", kind.name()).into());
        }
        let index = self
            .entries
            .iter()
            .filter(|needle| needle.kind == kind)
            .count();
        self.entries.push(Needle { kind, index, bytes });
        Ok(())
    }
}

/// Fails when `haystack` holds any of `needles`, naming `label` and each value found by its kind
/// and index only, so that the failure cannot itself print a protected value. Matching is byte
/// for byte and case-sensitive.
fn assert_no_sensitive(label: &str, haystack: &[u8], needles: &Needles) {
    let found: Vec<String> = needles
        .entries
        .iter()
        .filter(|needle| {
            haystack
                .windows(needle.bytes.len())
                .any(|window| window == needle.bytes.as_slice())
        })
        .map(|needle| format!("{} #{}", needle.kind.name(), needle.index))
        .collect();
    assert!(
        found.is_empty(),
        "{label} holds protected values: {}",
        found.join(", ")
    );
}

/// Why a captured trace line is not one JSON object with a member. It names the defect, never a
/// byte of the line.
type Malformed = &'static str;

/// Whether `bytes` holds `cvv` in any ASCII case, the test `api/tests/redaction.rs` applies to a
/// member name.
fn holds_cvv(bytes: &[u8]) -> bool {
    bytes
        .windows(3)
        .any(|window| window.eq_ignore_ascii_case(b"cvv"))
}

/// Decodes the escape whose backslash `chars` has just passed: `\"`, `\\`, `\/`, `\b`, `\f`,
/// `\n`, `\r`, `\t`, or `\u` and four hexadecimal digits. A `\u` value that is not a Unicode
/// scalar value, such as half of a surrogate pair, decodes to U+FFFD, which hides no ASCII
/// letter.
///
/// # Errors
///
/// An escape of any other form, or the line ending inside it.
fn json_escape(chars: &mut impl Iterator<Item = char>) -> Result<char, Malformed> {
    let decoded = match chars.next() {
        Some('"') => '"',
        Some('\\') => '\\',
        Some('/') => '/',
        Some('b') => '\u{8}',
        Some('f') => '\u{c}',
        Some('n') => '\n',
        Some('r') => '\r',
        Some('t') => '\t',
        Some('u') => {
            let mut code = 0_u32;
            for _ in 0..4 {
                let digit = chars
                    .next()
                    .and_then(|digit| digit.to_digit(16))
                    .ok_or("a \\u escape lacks four hexadecimal digits")?;
                code = (code << 4) | digit;
            }
            char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
        }
        Some(_) => return Err("a string holds an invalid escape"),
        None => return Err("a string is not terminated"),
    };
    Ok(decoded)
}

/// Reads the rest of a JSON string whose opening quote `chars` has just passed, through its
/// closing quote, with every escape decoded.
///
/// # Errors
///
/// The line ending inside the string, an invalid escape, or an unescaped control character.
fn json_string(chars: &mut impl Iterator<Item = char>) -> Result<String, Malformed> {
    let mut text = String::new();
    loop {
        match chars.next() {
            None => return Err("a string is not terminated"),
            Some('"') => return Ok(text),
            Some('\\') => text.push(json_escape(chars)?),
            Some(control) if control < ' ' => {
                return Err("a string holds an unescaped control character");
            }
            Some(character) => text.push(character),
        }
    }
}

/// The member names of `line`, one captured trace line, at every depth: each JSON string, its
/// escapes decoded, that JSON whitespace and then `:` follow. The JSON formatter writes every line
/// as one object and escapes the quotes inside a value, so no text inside a value is ever read as
/// a name. The lexer is total.
///
/// # Errors
///
/// What makes `line` other than one JSON object with a member: a first character other than `{`
/// or a last other than `}`, a malformed string, brackets that do not pair, text after the
/// object, or no member at all.
fn member_names(line: &str) -> Result<Vec<String>, Malformed> {
    if !(line.starts_with('{') && line.ends_with('}')) {
        return Err("it is not one JSON object");
    }
    let mut names = Vec::new();
    let mut closers = Vec::new();
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => {
                let text = json_string(&mut chars)?;
                let rest = chars.as_str().trim_start_matches([' ', '\t', '\n', '\r']);
                if rest.starts_with(':') {
                    names.push(text);
                }
            }
            '{' => closers.push('}'),
            '[' => closers.push(']'),
            '}' | ']' => {
                if closers.pop() != Some(character) {
                    return Err("its brackets do not pair");
                }
            }
            _ => {}
        }
        if closers.is_empty() && !chars.as_str().is_empty() {
            return Err("text follows its object");
        }
    }
    if !closers.is_empty() {
        return Err("its object is not closed");
    }
    if names.is_empty() {
        return Err("it has no member");
    }
    Ok(names)
}

/// The findings of the CVV member check over `lines`, the captured trace `label` names: one per
/// member, at any depth (the event's `fields`, its `span`, every entry of its `spans`), whose name
/// holds `cvv` in any ASCII case, and one per line [`member_names`] cannot read, since such a
/// line could hide one. A capture with no line is a finding too, so the check never passes over
/// nothing. Each finding names `label`, the line's index and the member name or the defect, never
/// a value.
fn cvv_member_findings(label: &str, lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return vec![format!("{label}: no trace line was captured")];
    }
    let mut findings = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        match member_names(line) {
            Ok(names) => findings.extend(
                names
                    .iter()
                    .filter(|name| holds_cvv(name.as_bytes()))
                    .map(|name| format!("{label} line {index}: member {name:?} names a CVV")),
            ),
            Err(defect) => findings.push(format!("{label} line {index}: {defect}")),
        }
    }
    findings
}

/// Fails when [`cvv_member_findings`] reports anything for `lines`, naming every finding.
fn assert_no_cvv_member(label: &str, lines: &[String]) {
    let findings = cvv_member_findings(label, lines);
    assert!(findings.is_empty(), "{}", findings.join("; "));
}

/// The finding of the CVV label check over `body`, the fixed-format output `label` names. A
/// published body or an encoded `cssl_error_log` record is fixed-width fields and literals, not
/// JSON, so a CVV could show only under a label naming it: `cvv` in any ASCII case. The finding
/// names `label`, never a byte.
fn cvv_label_finding(label: &str, body: &[u8]) -> Option<String> {
    holds_cvv(body).then(|| format!("{label} holds a CVV label"))
}

/// Fails when [`cvv_label_finding`] reports `body`.
fn assert_no_cvv_label(label: &str, body: &[u8]) {
    let finding = cvv_label_finding(label, body);
    assert!(finding.is_none(), "{}", finding.unwrap_or_default());
}

/// Fails unless `body`, a CDRD or CDRA output, fills the [`codate01::BUFFER_LEN`]-byte message
/// area and every byte from `written`, its audited written width, on is a space: the body carries
/// its audited layout and nothing after it, so no field outside that layout, a CVV among them,
/// rides in its padding. A failure names `label` and an offset, never a byte.
fn assert_fixed_shape(label: &str, body: &[u8], written: usize) {
    assert_eq!(
        body.len(),
        codate01::BUFFER_LEN,
        "{label}: bytes in the body"
    );
    let stray = body
        .iter()
        .enumerate()
        .skip(written)
        .find(|&(_, &byte)| byte != b' ')
        .map(|(offset, _)| offset);
    assert_eq!(
        stray, None,
        "{label}: the offset of a byte other than a space after the {written} written bytes"
    );
}

/// Fails unless `capture` recorded `count` settlement events, one per request the worker took,
/// so that a trace scan never passes over a capture the worker did not write to.
fn assert_settled(capture: &TraceCapture, label: &str, count: usize) {
    let settled = capture
        .lines()
        .iter()
        .filter(|line| line.contains(r#""message":"the request is settled""#))
        .count();
    assert_eq!(settled, count, "{label}: settlement events in the trace");
}

/// Request properties whose `message_id` and `correlation_id` are the requester's choice.
fn id_properties(message_id: &str, correlation_id: &str) -> BasicProperties {
    BasicProperties::default()
        .with_message_id(message_id.into())
        .with_correlation_id(correlation_id.into())
}

/// The digest that a worker opened with `config` shows for the request ID `id` in its span and
/// alert: the base64url HMAC-SHA256 under the worker's key of `config.amqp_url`.
fn keyed(config: &Config, id: &str) -> String {
    ScreenedText::keyed(id, &IdKey::new(&config.amqp_url)).to_string()
}

/// Fails unless `capture` shows each of `ids`, request IDs set to protected values, as its digest
/// keyed by `config.amqp_url` in a `message_id` or `correlation_id` field, so that the needle
/// scan of the trace runs over the keyed form, and unless the trace never holds that URL, the
/// key itself. A failure names `label` and the ID's position, never the ID or the URL.
fn assert_keyed(capture: &TraceCapture, config: &Config, label: &str, ids: &[&str]) {
    let text = capture.text();
    assert!(
        !text.contains(&config.amqp_url),
        "{label}: the trace holds the AMQP URL"
    );
    for (position, id) in ids.iter().enumerate() {
        let digest = keyed(config, id);
        let shown = ["message_id", "correlation_id"]
            .iter()
            .any(|field| text.contains(&format!(r#""{field}":"{digest}""#)));
        assert!(
            shown,
            "{label}: request ID {position} is not shown as its keyed digest"
        );
    }
}

/// Fixture values the requests themselves carry, to show that none comes back from the worker.
/// Deliberately not `Debug`: nothing prints them.
struct Probes {
    /// The first fixture card number.
    card: String,
    /// The first fixture SSN, nine digits.
    ssn: String,
    /// The same SSN as `NNN-NN-NNNN`.
    ssn_dashed: String,
    /// The first fixture SSN that does not start with zero, nine digits.
    unsigned_ssn: String,
    /// The first fixture user password, eight bytes.
    password: String,
}

impl Probes {
    /// The probe values, read from the fixtures as [`Needles::fixture`] reads them.
    ///
    /// # Errors
    ///
    /// The harness error of a fixture that cannot be read, or an error when a fixture lacks the
    /// value a probe needs.
    fn fixture() -> Result<Self, BoxError> {
        let card = fixtures::fixture_card_numbers()?
            .into_iter()
            .next()
            .ok_or("carddata.txt holds no card")?;
        let ssns = fixtures::fixture_ssns()?;
        let ssn = ssns.first().cloned().ok_or("custdata.txt holds no SSN")?;
        let unsigned_ssn = ssns
            .iter()
            .find(|ssn| !ssn.starts_with('0'))
            .cloned()
            .ok_or("every fixture SSN starts with zero")?;
        // The forms come plain then dashed, SSN by SSN, so the second is the first SSN dashed.
        let forms = fixtures::fixture_ssn_forms()?;
        if forms.first() != Some(&ssn) {
            return Err("the SSN forms do not start with the first SSN".into());
        }
        let ssn_dashed = forms.get(1).cloned().ok_or("no dashed SSN form")?;
        let password = fixtures::fixture_user_passwords()?
            .into_iter()
            .next()
            .ok_or("DUSRSECJ.jcl holds no user")?;
        Ok(Self {
            card,
            ssn,
            ssn_dashed,
            unsigned_ssn,
            password: String::from_utf8(password)?,
        })
    }
}

/// The test's own connection to the broker, with one channel in confirm mode: it publishes the
/// requests, declares the CP00 reply queues and takes every message a worker published.
struct Client {
    connection: Connection,
    channel: Channel,
}

impl Client {
    /// Connects as user `carddemo` and opens the confirm-mode channel.
    ///
    /// # Errors
    ///
    /// The harness or broker error of the connection or the channel, or a timeout after
    /// [`WAIT`].
    async fn open(broker: &RabbitMqGuard) -> Result<Self, BoxError> {
        let connection = broker.connect().await?;
        let channel = bounded(connection.create_channel()).await?;
        bounded(channel.confirm_select(ConfirmSelectOptions::default())).await?;
        Ok(Self {
            connection,
            channel,
        })
    }

    /// Publishes the request `body` to `queue` and returns once the broker has confirmed it, so
    /// the request is in its queue before a worker is asked to take it.
    ///
    /// # Errors
    ///
    /// The harness error of a failed, refused or unanswered publication.
    async fn request(
        &self,
        queue: &str,
        body: &[u8],
        properties: BasicProperties,
    ) -> Result<(), BoxError> {
        rabbitmq::publish(&self.channel, queue, body, properties).await?;
        Ok(())
    }

    /// Declares a reply queue for one CP00 request and returns its server-assigned name.
    ///
    /// # Errors
    ///
    /// The harness error of a failed or unanswered declaration.
    async fn reply_queue(&self) -> Result<String, BoxError> {
        Ok(rabbitmq::declare_reply_queue(&self.channel).await?)
    }

    /// Fails when a message arrives on `queue` within [`QUIET`].
    ///
    /// # Errors
    ///
    /// The harness error of a failed get, or an error naming `label` when a message arrived.
    async fn assert_empty(&self, queue: &str, label: &str) -> Result<(), BoxError> {
        match rabbitmq::get(&self.channel, queue, QUIET).await? {
            Some(_) => Err(format!("{label}: a reply arrived where none may").into()),
            None => Ok(()),
        }
    }

    /// Closes the connection, waiting at most [`WAIT`] for the broker.
    ///
    /// # Errors
    ///
    /// The broker error of the close, or a timeout.
    async fn close(self) -> Result<(), BoxError> {
        bounded(self.connection.close(REPLY_SUCCESS, CLOSE_REASON)).await
    }

    /// Deletes `queue`, so that a worker that checked it when it opened finds it gone.
    ///
    /// # Errors
    ///
    /// The broker error of the deletion, or a timeout after [`WAIT`].
    async fn delete_queue(&self, queue: &str) -> Result<(), BoxError> {
        bounded(
            self.channel
                .queue_delete(queue, QueueDeleteOptions::default()),
        )
        .await?;
        Ok(())
    }
}

/// Every body a worker published, with the queue it was taken from, in the order taken.
#[derive(Default)]
struct Published {
    bodies: Vec<(String, Vec<u8>)>,
}

impl Published {
    /// Takes exactly `count` messages from `queue`, each within [`WAIT`], checks that no further
    /// one is waiting, keeps them for [`Published::assert_redacted`] and returns their bodies in
    /// the order taken.
    ///
    /// # Errors
    ///
    /// The harness error of a failed get, or an error naming `queue` when fewer or more than
    /// `count` messages are there.
    async fn take(
        &mut self,
        client: &Client,
        queue: &str,
        count: usize,
    ) -> Result<Vec<Vec<u8>>, BoxError> {
        let mut taken = Vec::with_capacity(count);
        for position in 0..count {
            let message = rabbitmq::get(&client.channel, queue, WAIT)
                .await?
                .ok_or_else(|| format!("{queue}: message {position} of {count} did not arrive"))?;
            taken.push(message.delivery.data);
        }
        if rabbitmq::get(&client.channel, queue, Duration::ZERO)
            .await?
            .is_some()
        {
            return Err(format!("{queue}: more than {count} messages were published").into());
        }
        self.bodies
            .extend(taken.iter().map(|body| (queue.to_owned(), body.clone())));
        Ok(taken)
    }

    /// Fails when any body taken holds a protected value or a CVV label.
    fn assert_redacted(&self, needles: &Needles) {
        assert!(!self.bodies.is_empty(), "no published body was taken");
        for (position, (queue, body)) in self.bodies.iter().enumerate() {
            let label = format!("published body {position} from {queue}");
            assert_no_sensitive(&label, body, needles);
            assert_no_cvv_label(&label, body);
        }
    }
}

/// Runs `worker` under `dispatch`, the capture's dispatcher, until it takes a request, which
/// the caller has just published and the broker confirmed, and returns what it did with it.
///
/// # Errors
///
/// The worker's [`messaging::workers::StepError`], or a timeout when no request is taken within
/// [`WAIT`].
async fn settle<W: Step>(
    worker: &mut W,
    connection: &Connection,
    dispatch: &Dispatch,
) -> Result<Processed, BoxError> {
    let deadline = Instant::now() + WAIT;
    loop {
        // Boxed: a worker's step future exceeds the stack budget of `clippy::large_futures`.
        let step = Box::pin(
            worker
                .process_next(connection)
                .with_subscriber(dispatch.clone()),
        );
        // A request cut off at the deadline fails the test, whatever state it was left in.
        let processed = time::timeout_at(deadline, step).await??;
        if processed != Processed::Idle {
            return Ok(processed);
        }
        if Instant::now() >= deadline {
            return Err("the worker took no request within the wait".into());
        }
        time::sleep(POLL).await;
    }
}

/// The one instant every clock reading of a test returns, so that two CP00 decisions for one
/// account get one detail key (COPAUA0C-098, COPAUA0C-099).
///
/// # Errors
///
/// An error when the instant is not a valid date and time.
fn fixed_clock() -> Result<Arc<dyn Clock>, BoxError> {
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(19, 30, 5, 250))
        .ok_or("the fixed instant is not a valid date and time")?;
    Ok(Arc::new(FixedClock::new(at)))
}

/// Awaits `future` for at most [`WAIT`].
///
/// # Errors
///
/// The error `future` returns, or a timeout.
async fn bounded<T, E: Into<BoxError>>(
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, BoxError> {
    time::timeout(WAIT, future).await?.map_err(Into::into)
}

/// Runs `test` to completion on a current-thread runtime with the I/O and time drivers, the
/// runtime Tokio's test attribute builds, without the lint attributes, `expect` and unchecked
/// pinning that the attribute's expansion carries. A runtime that cannot be built fails the test.
/// The test future is boxed, so that it never sits on the stack.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(Box::pin(test))
}

/// Drives `first` and `second` concurrently on the calling task until both have succeeded, and
/// returns both values. Every poll polls each future that has not finished yet, and a finished
/// one is never polled again; both are boxed, so that they never move once polled. The first
/// error is returned at once, and the other future is dropped unfinished.
async fn try_join<T, U, E>(
    first: impl Future<Output = Result<T, E>>,
    second: impl Future<Output = Result<U, E>>,
) -> Result<(T, U), E> {
    let mut first = Box::pin(first);
    let mut second = Box::pin(second);
    let mut first_value = None;
    let mut second_value = None;
    std::future::poll_fn(|context| {
        if first_value.is_none() {
            match first.as_mut().poll(context) {
                Poll::Ready(Ok(value)) => first_value = Some(value),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {}
            }
        }
        if second_value.is_none() {
            match second.as_mut().poll(context) {
                Poll::Ready(Ok(value)) => second_value = Some(value),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {}
            }
        }
        match (first_value.take(), second_value.take()) {
            (Some(first_done), Some(second_done)) => Poll::Ready(Ok((first_done, second_done))),
            (first_left, second_left) => {
                first_value = first_left;
                second_value = second_left;
                Poll::Pending
            }
        }
    })
    .await
}

/// Opens the workers' AMQP session on `config.amqp_url` under `dispatch`, as `messaging::run`
/// connects.
///
/// # Errors
///
/// The [`messaging::broker::AmqpError`] of the connection, or a timeout after [`WAIT`].
async fn connect(config: &Config, dispatch: &Dispatch) -> Result<broker::Session, BoxError> {
    bounded(broker::connect(&config.amqp_url).with_subscriber(dispatch.clone())).await
}

/// Closes the workers' session while the workers still hold their channels, as `messaging::run`
/// does before it drops them, waiting at most [`WAIT`].
///
/// # Errors
///
/// A timeout.
async fn close(session: &broker::Session, dispatch: &Dispatch) -> Result<(), BoxError> {
    time::timeout(
        WAIT,
        session
            .close(CLOSE_REASON)
            .with_subscriber(dispatch.clone()),
    )
    .await?;
    Ok(())
}

/// The 16 bytes a redacted card number shows: twelve `*`, then the last four digits of `card`
/// (AAP 0.3.4.8).
fn redacted(card: &str) -> Vec<u8> {
    let mut form = vec![b'*'; 12];
    form.extend(card.bytes().skip(12));
    form
}

/// The card number of fixture account `acct_id`, from its `card_xref` row.
///
/// # Errors
///
/// An error when the account has no row or its card number is not UTF-8.
fn card_of(xrefs: &[CardXrefRecord], acct_id: u64) -> Result<String, BoxError> {
    let xref = xrefs
        .iter()
        .find(|xref| xref.acct_id.get() == acct_id)
        .ok_or_else(|| format!("fixture account {acct_id} has no card_xref row"))?;
    Ok(String::from_utf8(
        xref.card_num.expose().as_bytes().to_vec(),
    )?)
}

/// The CDRA requests whose reply masks the key positions (COACCT01-035):
///
/// - the ordinary not-found request, a key no fixture account holds, and the ordinary
///   bad-parameter request, a function other than `INQA`;
/// - the decoded probes, a key of `00` and an SSN's nine digits and a key of `000` and
///   [`PROBE_PASSWORD`], each with `INQA` (not found) and with another function (bad parameter);
/// - the malformed probes, non-digit keys and bodies shorter than 15 bytes, whose bytes 0-14
///   carry an SSN in either form, the fixture password or [`PROBE_PASSWORD`].
fn masked_cdra_requests(probes: &Probes) -> Vec<Vec<u8>> {
    let ssn_key = format!("00{}", probes.ssn);
    let password_key = format!("000{PROBE_PASSWORD}");
    [
        "INQA99999999999".to_owned(),
        "INQB00000000001".to_owned(),
        format!("INQA{ssn_key}"),
        format!("XXXX{ssn_key}"),
        format!("INQA{password_key}"),
        format!("XXXX{password_key}"),
        format!("INQA{}", probes.ssn_dashed),
        probes.ssn.clone(),
        format!("INQA{}000", probes.password),
        format!("INQA{}", probes.password),
        PROBE_PASSWORD.to_owned(),
    ]
    .into_iter()
    .map(String::into_bytes)
    .collect()
}

/// CDRD never reads its request (CODATE01-001), so a request whose body holds a fixture card
/// number, SSN and password is answered on `CARD.DEMO.REPLY.DATE`. With that queue deleted after
/// the worker opened, the next reply comes back unroutable and the `MQPUT ERR` body goes to
/// `CARD.DEMO.ERROR` (CODATE01-014). Both requests carry those values as their `message_id` and
/// `correlation_id` too, which the trace shows only as their keyed digests. Neither body nor the
/// trace holds a protected value.
// audit: CODATE01-009
#[test]
fn cdrd_outputs_and_traces_are_redacted() -> TestResult {
    block_on(async {
        let needles = Needles::fixture()?;
        let probes = Probes::fixture()?;
        let body = [
            probes.card.as_str(),
            probes.ssn_dashed.as_str(),
            probes.password.as_str(),
        ]
        .join(",")
        .into_bytes();
        // CDRD reads no store, so this test starts the broker alone.
        let broker = rabbitmq::start_rabbitmq().await?;
        let config = Config {
            amqp_url: broker.amqp_url().to_owned(),
            ..Config::default()
        };
        let client = Client::open(&broker).await?;
        let capture = trace_capture::capture();
        let dispatch = capture.dispatch();
        let session = connect(&config, &dispatch).await?;
        let connection = session.connection();
        let opening = cdrd::Worker::open(connection, &config, fixed_clock()?);
        let mut worker = bounded(opening.with_subscriber(dispatch.clone())).await?;
        let mut published = Published::default();
        let queue = config.queue_request_date.as_str();

        client
            .request(
                queue,
                &body,
                id_properties(&probes.card, &probes.ssn_dashed),
            )
            .await?;
        assert_eq!(
            settle(&mut worker, connection, &dispatch).await?,
            Processed::Replied
        );
        for reply in published.take(&client, codate01::REPLY_QUEUE, 1).await? {
            assert_fixed_shape("the CDRD reply", &reply, codate01::REPLY_TEXT_LEN);
        }

        client.delete_queue(codate01::REPLY_QUEUE).await?;
        client
            .request(queue, &body, id_properties(&probes.password, &probes.card))
            .await?;
        assert_eq!(
            settle(&mut worker, connection, &dispatch).await?,
            Processed::ErrorRouted
        );
        for error in published.take(&client, codate01::ERROR_QUEUE, 1).await? {
            assert_fixed_shape("the CDRD error body", &error, codate01::MQ_ERR_DISPLAY_LEN);
        }
        close(&session, &dispatch).await?;

        published.assert_redacted(&needles);
        assert_settled(&capture, "CDRD", 2);
        assert_keyed(
            &capture,
            &config,
            "CDRD",
            &[&probes.card, &probes.ssn_dashed, &probes.password],
        );
        assert_no_sensitive("the CDRD trace", capture.text().as_bytes(), &needles);
        assert_no_cvv_member("the CDRD trace", &capture.lines());
        client.close().await
    })
}

/// CDRA answers `INQA` for every fixture account, the masked requests of
/// [`masked_cdra_requests`], each reply holding eleven `*` at offsets 37-47, and a read through
/// a store whose pool is closed, which routes the error body to `CARD.DEMO.ERROR`
/// (COACCT01-028). No reply, error body or trace holds a protected value.
// audit: COACCT01-035
#[test]
fn cdra_outputs_and_traces_are_redacted() -> TestResult {
    block_on(async {
        let needles = Needles::fixture()?;
        let probes = Probes::fixture()?;
        let (broker, database) = try_join(
            rabbitmq::start_rabbitmq(),
            fixtures::start_loaded_postgres(),
        )
        .await?;
        let config = Config {
            amqp_url: broker.amqp_url().to_owned(),
            database_url: database.database_url().to_owned(),
            ..Config::default()
        };
        let store = PgStore::new(database.pool().clone());
        let accounts = AccountRepository::find_all(&mut store.clone()).await?;
        assert_eq!(accounts.len(), 50, "acctdata.txt holds 50 accounts");
        let mut requests: Vec<(Vec<u8>, bool)> = accounts
            .iter()
            .map(|account| {
                let id = account.id.get();
                (format!("INQA{id:011}").into_bytes(), false)
            })
            .collect();
        requests.extend(
            masked_cdra_requests(&probes)
                .into_iter()
                .map(|body| (body, true)),
        );
        // `cdra::Worker` reads only through a `PgStore`, so the failing store is one whose pool on
        // the same database is closed before the worker reads through it: the read fails.
        let closed = bounded(runtime::db::connect(&config)).await?;
        time::timeout(WAIT, closed.close()).await?;

        let client = Client::open(&broker).await?;
        let capture = trace_capture::capture();
        let dispatch = capture.dispatch();
        let session = connect(&config, &dispatch).await?;
        let connection = session.connection();
        let opening = cdra::Worker::open(connection, &config, store);
        let mut worker = bounded(opening.with_subscriber(dispatch.clone())).await?;
        let opening = cdra::Worker::open(connection, &config, PgStore::new(closed));
        let mut failing = bounded(opening.with_subscriber(dispatch.clone())).await?;
        let mut published = Published::default();
        let queue = config.queue_request_acct.as_str();

        for (body, _) in &requests {
            client
                .request(queue, body, BasicProperties::default())
                .await?;
            assert_eq!(
                settle(&mut worker, connection, &dispatch).await?,
                Processed::Replied
            );
        }
        let replies = published
            .take(&client, coacct01::REPLY_QUEUE, requests.len())
            .await?;
        for (position, ((_, masked), reply)) in requests.iter().zip(&replies).enumerate() {
            assert!(
                !masked || reply.get(37..48) == Some(MASKED_KEY.as_slice()),
                "CDRA reply {position}: offsets 37-47 are not eleven `*`"
            );
            // A masked reply is not-found text (48 bytes) or bad-parameter text (63 bytes), so
            // every byte from 63 on is padding either way.
            let written = if *masked {
                coacct01::BAD_PARAMETER_TEXT_LEN
            } else {
                coacct01::ACCT_RESPONSE_LEN
            };
            assert_fixed_shape(&format!("CDRA reply {position}"), reply, written);
        }

        client
            .request(queue, b"INQA00000000001", BasicProperties::default())
            .await?;
        assert_eq!(
            settle(&mut failing, connection, &dispatch).await?,
            Processed::ErrorRouted
        );
        for error in published.take(&client, coacct01::ERROR_QUEUE, 1).await? {
            assert_fixed_shape("the CDRA error body", &error, codate01::MQ_ERR_DISPLAY_LEN);
        }
        close(&session, &dispatch).await?;

        published.assert_redacted(&needles);
        assert_settled(&capture, "CDRA", requests.len() + 1);
        assert_no_sensitive("the CDRA trace", capture.text().as_bytes(), &needles);
        assert_no_cvv_member("the CDRA trace", &capture.lines());
        client.close().await
    })
}

/// What the screened fields of a CP00 reply hold (COPAUA0C-062, COPAUA0C-064).
#[derive(Clone, Copy)]
enum Screen {
    /// Field 18 [`ORDER_ID`] comes back unchanged.
    Echoed,
    /// Field 18 holds a protected value and comes back as 15 `*`.
    WithheldId,
    /// Field 18 [`ORDER_ID`] comes back unchanged, and the approved amount, which holds an SSN's
    /// nine digits, comes back as 14 `*`.
    WithheldAmount,
}

/// How the CP00 worker settles a request, and what reaches its `reply_to`.
#[derive(Clone, Copy)]
enum Outcome {
    /// One reply, confirmed, then the acknowledgement; its screened fields as [`Screen`] states.
    Replied(Screen),
    /// The reply is not confirmed: the `M004` row, then the dead-letter, with no reply
    /// (COPAUA0C-117).
    Unconfirmed,
    /// The card number is unparseable: the alert, then the dead-letter, with no reply
    /// (COPAUA0C-006).
    Unparseable,
}

impl Outcome {
    /// What [`Step::process_next`] returns for the request.
    const fn processed(self) -> Processed {
        match self {
            Self::Replied(_) => Processed::Replied,
            Self::Unconfirmed | Self::Unparseable => Processed::DeadLettered,
        }
    }
}

/// One CP00 request of the scenario. Deliberately not `Debug`: its body holds a card number.
struct Cp00Request {
    /// Names the request in a failure.
    label: &'static str,
    /// The card number the request names, whose redacted form the reply's card field holds.
    card: String,
    /// The comma-delimited body (`CCPAURQY`).
    body: Vec<u8>,
    /// How the worker settles it.
    outcome: Outcome,
    /// The `message_id` and `correlation_id` it carries, or `None` for neither.
    ids: Option<(String, String)>,
}

impl Cp00Request {
    /// The request for `card` with `amount` in field 9 and `transaction_id` in field 18, and no
    /// `message_id` or `correlation_id`.
    fn new(
        label: &'static str,
        card: String,
        amount: &str,
        transaction_id: &str,
        outcome: Outcome,
    ) -> Self {
        let body = cp00_body(&card, amount, transaction_id);
        Self {
            label,
            card,
            body,
            outcome,
            ids: None,
        }
    }

    /// This request carrying `message_id` and `correlation_id`.
    fn with_ids(mut self, message_id: &str, correlation_id: &str) -> Self {
        self.ids = Some((message_id.to_owned(), correlation_id.to_owned()));
        self
    }
}

/// The 18 comma-delimited fields of a CP00 request in wire order, with `card_field` in field 3,
/// `amount` in field 9 and `transaction_id` in field 18 (COPAUA0C-002).
fn cp00_body(card_field: &str, amount: &str, transaction_id: &str) -> Vec<u8> {
    let mut fields = vec![AUTH_DATE, AUTH_TIME, card_field];
    fields.extend(FIELDS_4_TO_8);
    fields.push(amount);
    fields.extend(FIELDS_10_TO_17);
    fields.push(transaction_id);
    fields.join(",").into_bytes()
}

/// The CP00 scenario of COPAUA0C-133, in processing order: the ordinary requests (an approval,
/// an insufficient-funds decline, the `A001`, `M004` and `I004` cases, a field error and an
/// unparseable card whose field 3 holds a fixture card number and one digit more), the five
/// field-18 probes and the approved request whose amount holds an SSN's nine digits. The
/// approval, the `M004` case and the unparseable card carry a fixture card number, a dashed
/// fixture SSN and the fixture password as their `message_id` and `correlation_id`.
///
/// # Errors
///
/// An error when a fixture account the scenario names has no `card_xref` row.
fn cp00_requests(xrefs: &[CardXrefRecord], probes: &Probes) -> Result<Vec<Cp00Request>, BoxError> {
    let echoed = Outcome::Replied(Screen::Echoed);
    let duplicate_key_card = card_of(xrefs, DUPLICATE_KEY_ACCOUNT)?;
    let mut requests = vec![
        Cp00Request::new(
            "approval",
            card_of(xrefs, APPROVAL_ACCOUNT)?,
            "100.00",
            ORDER_ID,
            echoed,
        )
        .with_ids(&probes.card, &probes.ssn_dashed),
        Cp00Request::new(
            "insufficient-funds decline",
            card_of(xrefs, DECLINE_ACCOUNT)?,
            "5000.00",
            ORDER_ID,
            echoed,
        ),
        Cp00Request::new(
            "card not in card_xref (A001)",
            UNKNOWN_CARD.to_owned(),
            "100.00",
            ORDER_ID,
            echoed,
        ),
        Cp00Request::new(
            "unconfirmed reply (M004)",
            card_of(xrefs, UNCONFIRMED_ACCOUNT)?,
            "1.00",
            ORDER_ID,
            Outcome::Unconfirmed,
        )
        .with_ids(&probes.ssn_dashed, &probes.password),
        Cp00Request::new(
            "first approval of the instant",
            duplicate_key_card.clone(),
            "1.00",
            ORDER_ID,
            echoed,
        ),
        Cp00Request::new(
            "second approval of the instant (I004)",
            duplicate_key_card,
            "1.00",
            ORDER_ID,
            echoed,
        ),
        Cp00Request::new(
            "field error",
            card_of(xrefs, FIELD_ERROR_ACCOUNT)?,
            "ABC",
            ORDER_ID,
            echoed,
        ),
        Cp00Request {
            label: "unparseable card number",
            card: probes.card.clone(),
            body: cp00_body(&format!("{}1", probes.card), "1.00", ORDER_ID),
            outcome: Outcome::Unparseable,
            ids: Some((probes.password.clone(), probes.card.clone())),
        },
    ];
    let identifiers = [
        ("field 18: an SSN", probes.ssn.clone()),
        ("field 18: a dashed SSN", probes.ssn_dashed.clone()),
        (
            "field 18: an SSN after two digits",
            format!("00{}", probes.ssn),
        ),
        ("field 18: the fixture password", probes.password.clone()),
        (
            "field 18: an eight-digit password",
            PROBE_PASSWORD.to_owned(),
        ),
    ];
    for ((label, identifier), account) in identifiers.into_iter().zip(FIRST_PROBE_ACCOUNT..) {
        requests.push(Cp00Request::new(
            label,
            card_of(xrefs, account)?,
            "1.00",
            &identifier,
            Outcome::Replied(Screen::WithheldId),
        ));
    }
    requests.push(Cp00Request::new(
        "approved amount: an SSN's nine digits",
        card_of(xrefs, AMOUNT_PROBE_ACCOUNT)?,
        &format!("-{}.00", probes.unsigned_ssn),
        ORDER_ID,
        Outcome::Replied(Screen::WithheldAmount),
    ));
    Ok(requests)
}

/// Checks the fields of one CP00 reply inside its 63 written bytes: the [`REPLY_COMMAS`], so that
/// those bytes are `CCPAURLY`'s six fields and no other, a CVV among them; the card field
/// (COPAUA0C-061), field 18 (COPAUA0C-062) and, for [`Screen::WithheldAmount`], the amount
/// (COPAUA0C-064). The body taken whole is HALTED(H6), so neither its length nor all of it is
/// compared (COPAUA0C-065).
fn assert_screened(request: &Cp00Request, reply: &[u8], screen: Screen) {
    let label = request.label;
    let misplaced: Vec<usize> = REPLY_COMMAS
        .into_iter()
        .filter(|&offset| reply.get(offset) != Some(&b','))
        .collect();
    assert!(
        misplaced.is_empty(),
        "{label}: no `,` at reply offsets {misplaced:?}"
    );
    assert!(
        reply.get(..16) == Some(redacted(&request.card).as_slice()),
        "{label}: bytes 0-15 are not twelve `*` and the last four digits of the request's card"
    );
    let identifier = match screen {
        Screen::WithheldId => WITHHELD_ID,
        Screen::Echoed | Screen::WithheldAmount => ECHOED_ID,
    };
    assert!(
        reply.get(17..32) == Some(identifier.as_slice()),
        "{label}: bytes 17-31 are not field 18 as the transaction-ID screen gives it"
    );
    if matches!(screen, Screen::WithheldAmount) {
        assert!(
            reply.get(48..62) == Some(WITHHELD_AMOUNT.as_slice()),
            "{label}: bytes 48-61 are not fourteen `*`"
        );
    }
}

/// Takes the reply to `request` from `reply_to` when one is due and checks its screened fields;
/// for the unparseable card, checks that none arrives. The caller calls it as soon as `request`
/// is settled: a CP00 reply expires five seconds after it is published (E16), so a reply taken
/// later could be gone however correct the worker.
///
/// # Errors
///
/// The error of [`Published::take`] or [`Client::assert_empty`].
async fn take_reply(
    client: &Client,
    published: &mut Published,
    request: &Cp00Request,
    reply_to: &str,
) -> Result<(), BoxError> {
    match request.outcome {
        Outcome::Replied(screen) => {
            for reply in published.take(client, reply_to, 1).await? {
                assert_screened(request, &reply, screen);
            }
        }
        Outcome::Unparseable => client.assert_empty(reply_to, request.label).await?,
        // No such queue exists to take from: the `M004` row stands for the reply.
        Outcome::Unconfirmed => {}
    }
    Ok(())
}

/// Checks the `cssl_error_log` rows of the scenario: `A001`, `M004` and `I004`, in that order
/// (COPAUA0C-114, COPAUA0C-117, COPAUA0C-120), the `A001` and `I004` event keys starting with the
/// redacted form of their request's card, and no row, encoded as its 122-byte record, holding a
/// protected value or a CVV label (COPAUA0C-110).
///
/// # Errors
///
/// The store error of the read, or a timeout after [`WAIT`].
async fn assert_log_rows(
    store: &mut PgStore,
    needles: &Needles,
    a001_card: &str,
    i004_card: &str,
) -> Result<(), BoxError> {
    let rows = bounded(CsslErrorLogRepository::find_all(store)).await?;
    let locations: Vec<&[u8; 4]> = rows
        .iter()
        .map(|(_, record)| record.location.as_bytes())
        .collect();
    assert_eq!(locations, [b"A001", b"M004", b"I004"], "cssl_error_log");
    for (id, record) in &rows {
        let encoded = record.encode();
        assert_eq!(
            encoded.len(),
            ErrorLogRecord::WIDTH,
            "cssl_error_log row {id}"
        );
        let label = format!("cssl_error_log row {id}");
        assert_no_sensitive(&label, &encoded, needles);
        assert_no_cvv_label(&label, &encoded);
    }
    for (location, card) in [(b"A001", a001_card), (b"I004", i004_card)] {
        let keyed = rows
            .iter()
            .filter(|(_, record)| record.location.as_bytes() == location)
            .all(|(_, record)| record.event_key.as_bytes().starts_with(&redacted(card)));
        assert!(
            keyed,
            "the event key of a {} row does not start with the redacted card",
            String::from_utf8_lossy(location)
        );
    }
    Ok(())
}

/// CP00 over the fixture-loaded database: every request of [`cp00_requests`] is published with a
/// reply queue of its own, or the missing queue of the `M004` case, and settled one at a time
/// under one clock instant. Each reply is taken as soon as its request is settled, before the
/// next request is published, because a CP00 reply expires five seconds after it is published
/// (E16). Every reply's card field is the redacted form of its request's card, `ORDER-A` comes
/// back unchanged, every probe comes back withheld, the `A001` and `I004` event keys hold the
/// redacted card, the unparseable card raises one alert and no reply, and no reply, log row or
/// trace holds a protected value.
// audit: COPAUA0C-006
// audit: COPAUA0C-061
// audit: COPAUA0C-110
// audit: COPAUA0C-114
// audit: COPAUA0C-133
#[test]
fn cp00_outputs_log_rows_and_traces_are_redacted() -> TestResult {
    block_on(async {
        let needles = Needles::fixture()?;
        let probes = Probes::fixture()?;
        let (broker, database) = try_join(
            rabbitmq::start_rabbitmq(),
            fixtures::start_loaded_postgres(),
        )
        .await?;
        let config = Config {
            amqp_url: broker.amqp_url().to_owned(),
            database_url: database.database_url().to_owned(),
            ..Config::default()
        };
        let mut store = PgStore::new(database.pool().clone());
        let xrefs = bounded(CardXrefRepository::find_all(&mut store)).await?;
        let requests = cp00_requests(&xrefs, &probes)?;
        let client = Client::open(&broker).await?;
        let mut reply_queues = Vec::with_capacity(requests.len());
        for request in &requests {
            reply_queues.push(match request.outcome {
                Outcome::Unconfirmed => MISSING_REPLY_QUEUE.to_owned(),
                Outcome::Replied(_) | Outcome::Unparseable => client.reply_queue().await?,
            });
        }

        let capture = trace_capture::capture();
        let dispatch = capture.dispatch();
        let session = connect(&config, &dispatch).await?;
        let connection = session.connection();
        let opening = cp00::Worker::open(connection, &config, store.clone(), fixed_clock()?);
        let mut worker = bounded(opening.with_subscriber(dispatch.clone())).await?;
        let mut published = Published::default();
        for (request, reply_to) in requests.iter().zip(&reply_queues) {
            let properties = match &request.ids {
                Some((message_id, correlation_id)) => id_properties(message_id, correlation_id),
                None => BasicProperties::default(),
            }
            .with_reply_to(reply_to.as_str().into());
            client
                .request(&config.queue_request_pauth, &request.body, properties)
                .await?;
            let processed = settle(&mut worker, connection, &dispatch).await?;
            assert_eq!(processed, request.outcome.processed(), "{}", request.label);
            // A CP00 reply expires five seconds after it is published (E16), so it is taken now,
            // before the next request is published, and never after later steps.
            take_reply(&client, &mut published, request, reply_to).await?;
        }
        close(&session, &dispatch).await?;

        published.assert_redacted(&needles);
        let i004_card = card_of(&xrefs, DUPLICATE_KEY_ACCOUNT)?;
        assert_log_rows(&mut store, &needles, UNKNOWN_CARD, &i004_card).await?;
        let alerts = capture.lines_with_target(ALERT_TARGET);
        assert!(
            alerts.len() == 1
                && alerts
                    .iter()
                    .all(|line| line.contains(copaua0c::ALERT_CARD_UNPARSEABLE)),
            "expected one {} alert, found {} alert lines",
            copaua0c::ALERT_CARD_UNPARSEABLE,
            alerts.len()
        );
        // The unparseable request's IDs are the fixture password and card number.
        let alert_ids = [
            format!(r#""message_id":"{}""#, keyed(&config, &probes.password)),
            format!(r#""correlation_id":"{}""#, keyed(&config, &probes.card)),
        ];
        let shows_ids = |line: &String| {
            alert_ids
                .iter()
                .all(|member| line.contains(member.as_str()))
        };
        assert!(
            alerts.iter().all(shows_ids),
            "the alert does not show the request IDs as their keyed digests"
        );
        assert_settled(&capture, "CP00", requests.len());
        assert_keyed(
            &capture,
            &config,
            "CP00",
            &[&probes.card, &probes.ssn_dashed, &probes.password],
        );
        assert_no_sensitive("the CP00 trace", capture.text().as_bytes(), &needles);
        assert_no_cvv_member("the CP00 trace", &capture.lines());
        client.close().await
    })
}

/// A synthetic CVV value, the CVV of no fixture card (`carddata.txt`), which no finding may print.
const SYNTHETIC_CVV: &str = "987";

/// A trace line in the JSON formatter's shape with no CVV member.
const CLEAN_LINE: &str = concat!(
    r#"{"timestamp":"2022-07-18T19:30:05.250Z","level":"INFO","#,
    r#""fields":{"message":"the request is settled","elapsed_ms":5,"redelivered":false,"#,
    r#""reply_to":null},"target":"carddemo::messaging","#,
    r#""span":{"queue":"CARD.DEMO.REQUEST.PAUTH","name":"message"},"#,
    r#""spans":[{"queue":"CARD.DEMO.REQUEST.PAUTH","name":"message"}]}"#,
);

/// What a case of [`cvv_checks_report_a_cvv_field_without_printing_its_value`] gives a check.
#[derive(Clone, Copy)]
enum Synthetic {
    /// Captured trace lines, for [`cvv_member_findings`].
    Lines(&'static [&'static str]),
    /// One fixed-format body, for [`cvv_label_finding`].
    Body(&'static [u8]),
}

/// The cases of [`cvv_checks_report_a_cvv_field_without_printing_its_value`]: a name, the input
/// and the number of findings the check must report. An input that gives a CVV member or label a
/// value gives it [`SYNTHETIC_CVV`].
const CVV_CASES: [(&str, Synthetic, usize); 24] = [
    (
        "a CVV member in the event fields",
        Synthetic::Lines(&[
            r#"{"level":"INFO","fields":{"message":"settled","cvv":"987"},"target":"carddemo"}"#,
        ]),
        1,
    ),
    (
        "a CVV member in the span",
        Synthetic::Lines(&[r#"{"fields":{"message":"new"},"span":{"card_cvv_cd":"987"}}"#]),
        1,
    ),
    (
        "a CVV member in an entry of spans",
        Synthetic::Lines(&[r#"{"fields":{"message":"m"},"spans":[{"name":"a"},{"cvvCode":987}]}"#]),
        1,
    ),
    (
        "a CVV member in upper case",
        Synthetic::Lines(&[r#"{"fields":{"CVV":"987"}}"#]),
        1,
    ),
    (
        "CVV members in mixed case, in the fields and the span",
        Synthetic::Lines(&[r#"{"fields":{"Card_Cvv":"987"},"span":{"cvV":"987"}}"#]),
        2,
    ),
    (
        "a CVV member whose name is escaped",
        Synthetic::Lines(&[r#"{"fields":{"\u0063v\u0056":"987"}}"#]),
        1,
    ),
    (
        "a CVV member followed by whitespace",
        Synthetic::Lines(&["{\"fields\":{\"cvv\" \t: \"987\"}}"]),
        1,
    ),
    (
        "a CVV member on the second line",
        Synthetic::Lines(&[CLEAN_LINE, r#"{"fields":{"cvv":"987"}}"#]),
        1,
    ),
    (
        "`cvv` only inside an escaped value",
        Synthetic::Lines(&[r#"{"fields":{"message":"a \"cvv\": 987 quoted"}}"#]),
        0,
    ),
    (
        "`cvv` only as values",
        Synthetic::Lines(&[r#"{"fields":{"message":"cvv","items":["CVV",":"]}}"#]),
        0,
    ),
    ("a clean line", Synthetic::Lines(&[CLEAN_LINE]), 0),
    (
        "an unterminated string",
        Synthetic::Lines(&[r#"{"fields":{"cvv":"987}}"#]),
        1,
    ),
    (
        "an invalid escape",
        Synthetic::Lines(&[r#"{"fields":{"message":"\x987"}}"#]),
        1,
    ),
    (
        "a short \\u escape",
        Synthetic::Lines(&[r#"{"fields":{"message":"\u98"}}"#]),
        1,
    ),
    (
        "an unescaped control character",
        Synthetic::Lines(&["{\"fields\":{\"message\":\"\u{1}987\"}}"]),
        1,
    ),
    (
        "an object that is not closed",
        Synthetic::Lines(&[r#"{"fields":{"message":"987"}"#]),
        1,
    ),
    (
        "brackets that do not pair",
        Synthetic::Lines(&[r#"{"spans":[{"cvv":"987"}}}"#]),
        1,
    ),
    (
        "text after the object",
        Synthetic::Lines(&[r#"{"fields":{"message":"m"}} {"cvv":"987"}"#]),
        1,
    ),
    (
        "a line that is not an object",
        Synthetic::Lines(&[r#"["cvv","987"]"#]),
        1,
    ),
    ("an object with no member", Synthetic::Lines(&["{}"]), 1),
    ("a capture with no line", Synthetic::Lines(&[]), 1),
    ("a body with a CVV label", Synthetic::Body(b"CVV : 987"), 1),
    ("a body with a mixed-case label", Synthetic::Body(b"Cvv"), 1),
    (
        "a clean body",
        Synthetic::Body(b"SYSTEM DATE : 07-18-2022SYSTEM TIME : 19:30:05"),
        0,
    ),
];

/// The CVV checks report a CVV member at any depth and in any ASCII case, every trace line they
/// cannot read, a capture with no line and a fixed-format body with a CVV label, and they pass a
/// clean line, a line whose only `cvv` is text inside a value and a clean body. No finding prints
/// [`SYNTHETIC_CVV`], and each names its case. The checks need no broker.
#[test]
fn cvv_checks_report_a_cvv_field_without_printing_its_value() {
    for (case, input, expected) in CVV_CASES {
        let findings = match input {
            Synthetic::Lines(lines) => {
                let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
                cvv_member_findings(case, &lines)
            }
            Synthetic::Body(body) => cvv_label_finding(case, body).into_iter().collect(),
        };
        assert_eq!(findings.len(), expected, "{case}: findings");
        assert!(
            findings
                .iter()
                .all(|finding| finding.starts_with(case) && !finding.contains(SYNTHETIC_CVV)),
            "{case}: a finding does not name its case, or prints the CVV value"
        );
    }
}
