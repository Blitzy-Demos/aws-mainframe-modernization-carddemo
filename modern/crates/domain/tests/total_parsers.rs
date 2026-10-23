//! Total Parsers Over Untrusted Input, the `domain` half (AAP 0.3.3.5): every public `domain`
//! function that parses bytes or text from outside the process returns a value or a typed error
//! for any input of any length, and never panics. `copybook/tests/total_parsers.rs` covers the
//! PIC, zoned, packed and framing primitives these are built on, so none of those is fed here.
//!
//! The parsers fed:
//!
//! - `FixedRecord::decode` of the 18 byte layouts: the 17 of `domain::records` and
//!   `batch::cobtupdt::MaintenanceRecord`, the MNTTRDB2 input record.
//! - `Db2TransactionType::new` and `Db2TransactionCategory::new`, which take Db2 and screen text,
//!   and `ErrorLevel::from_pic` and `ErrorSubsystem::from_pic` over every byte.
//! - `messaging::coacct01::decode_request` (CDRA) and `read_failure`, which takes the configured
//!   queue name; `messaging::copaua0c::parse_request` (CP00), with `field_error_reply`,
//!   `reply_body` and `transaction_id_screen` over the echoed fields, and `SqlState::parse`.
//! - `batch::cobtupdt::{parse_input, decode_input}`; `batch::loads::{decode_fixture,
//!   check_key_sequence, usrsec_instream_lines, decode_usrsec}`, `decode_fixture` for the eight
//!   fixture layouts and `dailytran.txt`'s; `batch::cbpaup0c::PurgeParams::parse` (SYSIN);
//!   `batch::cbact04c::ParmDate::parse` (PARM).
//! - `online::copaus0c::{AuthKey::parse, edit_account}`, the `{auth_key}` and `{acct_id}` path
//!   segments.
//! - `sensitive`: `CardNumber::parse` and its `TryFrom<&str>`, `Ssn::parse` and its
//!   `TryFrom<&str>`, and `Password::from_input`, the password's text constructor (it has no
//!   `parse`). `Cvv` has no text or byte parser: its one constructor takes the decoded
//!   `PIC 9(03)` value, which it receives through `CardRecord::decode`.
//! - `password::verify`, which parses the `users.password_hash` PHC text read back from the
//!   database: the whole input as that text, with a synthetic 8-byte password.
//!
//! Text parsers receive `String::from_utf8_lossy` of the input and of its windows. When the bytes
//! are valid UTF-8 that text is exactly what `std::str::from_utf8` returns, so one call covers both
//! decodings.
//!
//! Inputs are generated, never read from a file: byte vectors of length 0 to 1,024, at least 1,024
//! cases per property, shapes biased toward each parser's valid input (record-width text, CP00 and
//! CDRA bodies, PHC-shaped text with bounded argon2 cost parameters, mutated decodable templates),
//! a deterministic sweep of every length with boundary fill bytes, and a sweep that rewrites each
//! numeric, date and timestamp field of each template with hostile values (negative zeros, bad
//! signs, impossible dates, the other timestamp layouts).
//! A panic anywhere fails the test. Beyond not panicking, each accepted input is checked against
//! its parser's documented contract (a record re-encodes to its bytes, a fixture to its lines), and
//! every error text is rendered and checked to hold no run of eight or more digits, so none echoes
//! a card number or SSN. Assertion messages name the parser and the input length, never input
//! bytes or decoded values (Cardholder Data Redaction); a failing property's report shows only the
//! generated input's length.

use std::any;
use std::fmt::{self, Display};
use std::iter;
use std::ops::Range;

use copybook::{FixedRecord, Framed, PicU, PicX};
use domain::batch::cbact04c::{
    DEFAULT_PARM_DATE, PARM_DATE_LEN, ParmDate, interest_transaction_id,
};
use domain::batch::cbpaup0c::{DEFAULT_SYSIN, PRM_INFO_LEN, PurgeParams, SYSIN_RECORD_LEN};
use domain::batch::cobtupdt::{
    INPUT_RECORD_LEN, Maintenance, MaintenanceRecord, decode_input, parse_input,
};
use domain::batch::loads::{
    LoadErrorKind, LoadJob, check_key_sequence, decode_fixture, decode_usrsec,
    usrsec_instream_lines,
};
use domain::messaging::coacct01::{self, CdraRequestError, REQUEST_LEN, decode_request};
use domain::messaging::copaua0c::{
    CardNumberCause, CardNumberError, FieldError, FieldFault, GET_BUFFER_LEN, ParseError,
    REPLY_WRITTEN_LEN, RequestField, SqlState, field_error_reply, parse_request, reply_body,
    transaction_id_screen,
};
use domain::online::copaus0c::{AuthKey, edit_account};
use domain::password::{self, PasswordError};
use domain::records::error_log::card_event_key;
use domain::records::{
    AccountRecord, AuthReply, AuthRequestRecord, CardRecord, CardXrefRecord, CustomerRecord,
    DailyTransactionRecord, Db2TransactionCategory, Db2TransactionType, DisclosureGroupRecord,
    ErrorLevel, ErrorLogRecord, ErrorSubsystem, PendingAuthDetailRecord, PendingAuthSummaryRecord,
    RejectRecord, TranCatBalanceRecord, TransactionCategoryRecord, TransactionRecord,
    TransactionTypeRecord, UserSecurityRecord,
};
use domain::sensitive::{CardNumber, Password, Ssn};
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::strategy::Union;

/// Longest generated input, in bytes.
const MAX_LEN: usize = 1_024;

/// Fill bytes of the deterministic sweep: low-value, blank, the highest digit, the positive and
/// negative zero overpunch, and high-value.
const FILLS: [u8; 6] = [0x00, b' ', b'9', b'{', b'}', 0xFF];

/// Bytes the record and message parsers give meaning to: digits, every overpunch sign, blank,
/// the CP00 delimiter, sign and point, CR and LF.
const RECORD_TEXT: &[u8] = b"0123456789{}ABCDEFGHIJKLMNOPQR ,-.\r\n";

/// [`RECORD_TEXT`] less the comma, CR and LF: bytes a CP00 field can hold and stay one printable
/// field.
const FIELD_TEXT: &[u8] = b"0123456789{}ABCDEFGHIJKLMNOPQR -.";

/// The byte width of each layout [`exercise_records`] decodes.
const LAYOUT_WIDTHS: [usize; 18] = [
    AccountRecord::WIDTH,
    CardRecord::WIDTH,
    CardXrefRecord::WIDTH,
    CustomerRecord::WIDTH,
    TransactionRecord::WIDTH,
    DailyTransactionRecord::WIDTH,
    RejectRecord::WIDTH,
    TranCatBalanceRecord::WIDTH,
    DisclosureGroupRecord::WIDTH,
    TransactionTypeRecord::WIDTH,
    TransactionCategoryRecord::WIDTH,
    UserSecurityRecord::WIDTH,
    PendingAuthSummaryRecord::WIDTH,
    PendingAuthDetailRecord::WIDTH,
    AuthRequestRecord::WIDTH,
    AuthReply::WIDTH,
    ErrorLogRecord::WIDTH,
    MaintenanceRecord::WIDTH,
];

/// `SEC-USR-PWD PIC X(08)`: the longest password text `Password::from_input` accepts.
const PASSWORD_LEN: usize = 8;

/// A synthetic 8-byte `SEC-USR-PWD` value that every stored text is verified against; no demo
/// or fixture password.
const VERIFY_PASSWORD: &[u8; PASSWORD_LEN] = b"TESTPW01";

/// `CUST-SSN PIC 9(09)`: the digits of an SSN.
const SSN_LEN: usize = 9;

/// Widths of the text parsers' valid input: a SQLSTATE (5), a password, an SSN, an account ID
/// (11), a card number and an authorization key (16).
const TEXT_WIDTHS: [usize; 5] = [5, PASSWORD_LEN, SSN_LEN, 11, AuthKey::RENDERED_WIDTH];

/// `TR_DESCRIPTION` and `TRC_CAT_DATA` are `VARCHAR(50)` (`TRNTYPE.ddl:3`, `TRNTYCAT.ddl:4`).
const VARCHAR_MAX: usize = 50;

/// `PA-RQ-CARD-NUM PIC X(16)` and `PicX<16>` card fields.
const CARD_LEN: usize = 16;

/// The twelve masked bytes that open a redacted card number.
const CARD_MASK: [u8; 12] = [b'*'; 12];

/// `WS-FUNC PIC X(04)`, the first bytes of a CDRA request.
const CDRA_FUNCTION_LEN: usize = 4;

/// Fields of a CP00 request (`CCPAURQY`).
const CP00_FIELDS: usize = 18;

/// Bytes of the response code in a CP00 reply body, as `reply_body` lays it out.
const REPLY_RESPONSE_CODE: Range<usize> = 40..42;

/// Bytes of the reason code in a CP00 reply body, as `reply_body` lays it out.
const REPLY_REASON_CODE: Range<usize> = 43..47;

/// The shortest digit run an error text must not hold: no error carries input bytes, so a run
/// this long could only be an echoed card number, SSN or account key.
const DIGIT_RUN: usize = 8;

/// The DUSRSECJ statement that opens the in-stream `USRSEC` records (`DUSRSECJ.jcl:34`).
const SYSUT1_DD_STAR: &[u8] = b"//SYSUT1   DD *";

/// A valid `PIC X(10)` date and `PIC X(26)` timestamp, layout `I`, for template repair.
const SAMPLE_DATE: &[u8] = b"2022-07-18";

/// See [`SAMPLE_DATE`].
const SAMPLE_TIMESTAMP: &[u8] = b"2022-07-18 10:11:12.123456";

/// Bytes of a `PIC +9(n).99` edit besides its integer digits: the sign, the point and two
/// decimals (`PA-RQ-TRANSACTION-AMT`, `PA-RL-APPROVED-AMT`).
const EDITED_POINT_AND_SIGN: usize = 4;

/// The shared property configuration: 1,024 cases, the rule's minimum, and no failure
/// persistence, so a failing run writes no `proptest-regressions/` file into `tests/`.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1_024,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// A generated property input. Its bytes reach the record parsers as card number, SSN and
/// password fields, and every input but a fixture file reaches `password::verify` as stored-hash
/// text, so its `Debug`, which proptest prints as a failing property's minimal input, names only
/// its length (Cardholder Data Redaction).
#[derive(Clone)]
struct Generated(Vec<u8>);

impl fmt::Debug for Generated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<{} generated bytes>", self.0.len())
    }
}

/// `strategy`'s byte vectors as [`Generated`] inputs, shrunk through the bytes.
fn generated(strategy: impl Strategy<Value = Vec<u8>>) -> impl Strategy<Value = Generated> {
    strategy.prop_map(Generated)
}

/// `bytes`, then its first and its last `width` bytes when it is that long: the whole input
/// meets the length check, the windows carry its content past it.
fn windows(bytes: &[u8], width: usize) -> impl Iterator<Item = &[u8]> {
    let head = bytes.get(..width);
    let tail = bytes
        .len()
        .checked_sub(width)
        .and_then(|start| bytes.get(start..));
    iter::once(bytes).chain(head).chain(tail)
}

/// Whether `bytes` is exactly `width` ASCII digits.
fn is_digits(bytes: &[u8], width: usize) -> bool {
    bytes.len() == width && bytes.iter().all(u8::is_ascii_digit)
}

/// Whether `text` holds [`DIGIT_RUN`] or more consecutive ASCII digits.
fn has_digit_run(text: &str) -> bool {
    let mut run = 0_usize;
    for byte in text.bytes() {
        run = if byte.is_ascii_digit() {
            run.saturating_add(1)
        } else {
            0
        };
        if run >= DIGIT_RUN {
            return true;
        }
    }
    false
}

/// Renders `error` through `Display`, which must not panic, and checks that the text is not
/// empty and echoes no digit run of the input.
fn check_error(parser: &str, len: usize, error: &impl Display) {
    let text = error.to_string();
    assert!(
        !text.is_empty(),
        "{parser}: empty error text for a {len}-byte input"
    );
    assert!(
        !has_digit_run(&text),
        "{parser}: error text holds a run of {DIGIT_RUN} or more digits for a {len}-byte input"
    );
}

/// `R::decode` on `bytes`. An accepted slice must re-encode to exactly its own bytes and decode
/// back to the same record, the byte identity every layout promises; a rejected one renders its
/// error.
fn decode_record<R: FixedRecord + PartialEq>(bytes: &[u8]) -> Option<R> {
    let layout = any::type_name::<R>();
    let len = bytes.len();
    match R::decode(bytes) {
        Ok(record) => {
            let encoded = record.encode();
            assert!(
                encoded.len() == R::WIDTH,
                "{layout}: encode did not give {} bytes",
                R::WIDTH
            );
            assert!(
                encoded == bytes,
                "{layout}: an accepted {len}-byte record did not re-encode to its bytes"
            );
            assert!(
                R::decode(&encoded).is_ok_and(|again| again == record),
                "{layout}: a re-encoded {len}-byte record did not decode to the same record"
            );
            Some(record)
        }
        Err(error) => {
            check_error(layout, len, &error);
            None
        }
    }
}

/// `R::decode` on `bytes` and on its `R::WIDTH`-byte windows.
fn exercise_record<R: FixedRecord + PartialEq>(bytes: &[u8]) {
    for window in windows(bytes, R::WIDTH) {
        let _ = decode_record::<R>(window);
    }
}

/// `ErrorLogRecord::decode`, and the 88-level conditions of an accepted record.
fn exercise_error_log(bytes: &[u8]) {
    for window in windows(bytes, ErrorLogRecord::WIDTH) {
        if let Some(record) = decode_record::<ErrorLogRecord>(window) {
            let _ = record.level();
            let _ = record.subsystem();
        }
    }
}

/// Every byte layout's `decode` (Total Parsers Over Untrusted Input).
fn exercise_records(bytes: &[u8]) {
    exercise_record::<AccountRecord>(bytes);
    exercise_record::<CardRecord>(bytes);
    exercise_record::<CardXrefRecord>(bytes);
    exercise_record::<CustomerRecord>(bytes);
    exercise_record::<TransactionRecord>(bytes);
    exercise_record::<DailyTransactionRecord>(bytes);
    exercise_record::<RejectRecord>(bytes);
    exercise_record::<TranCatBalanceRecord>(bytes);
    exercise_record::<DisclosureGroupRecord>(bytes);
    exercise_record::<TransactionTypeRecord>(bytes);
    exercise_record::<TransactionCategoryRecord>(bytes);
    exercise_record::<UserSecurityRecord>(bytes);
    exercise_record::<PendingAuthSummaryRecord>(bytes);
    exercise_record::<PendingAuthDetailRecord>(bytes);
    exercise_record::<AuthRequestRecord>(bytes);
    exercise_record::<AuthReply>(bytes);
    exercise_error_log(bytes);
    exercise_record::<MaintenanceRecord>(bytes);
}

/// `decode_fixture::<R>` over the whole input. Accepted records must rebuild the input byte for
/// byte, terminators and short bodies included; with a `job`, `check_key_sequence` then runs
/// over them as the load would.
fn exercise_fixture<R: FixedRecord>(job: Option<LoadJob>, bytes: &[u8]) {
    let layout = any::type_name::<R>();
    let len = bytes.len();
    match decode_fixture::<R>(bytes) {
        Ok(records) => {
            let rebuilt: Vec<u8> = records.iter().flat_map(Framed::encode).collect();
            assert!(
                rebuilt == bytes,
                "decode_fixture::<{layout}>: accepted records did not rebuild a {len}-byte input"
            );
            if let Some(job) = job {
                if let Err(error) = check_key_sequence(job, &records) {
                    check_error("check_key_sequence", len, &error);
                }
            }
        }
        Err(error) => {
            assert!(
                matches!(error.kind(), LoadErrorKind::Record(_)) && error.line().is_some(),
                "decode_fixture::<{layout}>: a {len}-byte input failed other than on a line"
            );
            check_error("decode_fixture", len, &error);
        }
    }
}

/// The eight fixture loads and the `dailytran.txt` decode `posttran` shares with them.
fn exercise_fixture_loads(bytes: &[u8]) {
    exercise_fixture::<AccountRecord>(Some(LoadJob::Acctfile), bytes);
    exercise_fixture::<CardRecord>(Some(LoadJob::Cardfile), bytes);
    exercise_fixture::<CardXrefRecord>(Some(LoadJob::Xreffile), bytes);
    exercise_fixture::<CustomerRecord>(Some(LoadJob::Custfile), bytes);
    exercise_fixture::<DisclosureGroupRecord>(Some(LoadJob::Discgrp), bytes);
    exercise_fixture::<TranCatBalanceRecord>(Some(LoadJob::Tcatbalf), bytes);
    exercise_fixture::<TransactionCategoryRecord>(Some(LoadJob::Trancatg), bytes);
    exercise_fixture::<TransactionTypeRecord>(Some(LoadJob::Trantype), bytes);
    exercise_fixture::<DailyTransactionRecord>(None, bytes);
}

/// `usrsec_instream_lines` and `decode_usrsec` over one deck. Both report a missing statement or
/// delimiter alike; an accepted deck gives one record per in-stream line, each re-encoding to
/// that line, terminator included.
fn exercise_usrsec(jcl: &[u8]) {
    let len = jcl.len();
    match (usrsec_instream_lines(jcl), decode_usrsec(jcl)) {
        (Ok(lines), Ok(records)) => {
            assert!(
                records.len() == lines.len(),
                "decode_usrsec: record count differs from the in-stream lines of a {len}-byte deck"
            );
            for (framed, line) in records.iter().zip(&lines) {
                let original = [line.body, line.framing.terminator.as_bytes()].concat();
                assert!(
                    framed.encode() == original,
                    "decode_usrsec: a record did not re-encode to its line in a {len}-byte deck"
                );
            }
        }
        (Ok(_), Err(error)) => {
            assert!(
                matches!(
                    error.kind(),
                    LoadErrorKind::Record(_) | LoadErrorKind::KeySequence { .. }
                ),
                "decode_usrsec: a {len}-byte deck with in-stream lines failed on its structure"
            );
            check_error("decode_usrsec", len, &error);
        }
        (Err(lines_error), Err(error)) => {
            assert!(
                lines_error == error,
                "decode_usrsec: a {len}-byte deck's structure error differs from its line split's"
            );
            check_error("usrsec_instream_lines", len, &lines_error);
        }
        (Err(_), Ok(_)) => {
            panic!("decode_usrsec: accepted a {len}-byte deck whose in-stream lines are refused")
        }
    }
}

/// `bytes` as the in-stream data of a DUSRSECJ deck, statement and delimiter around it.
fn usrsec_deck(instream: &[u8]) -> Vec<u8> {
    [SYSUT1_DD_STAR, b"\n", instream, b"\n/*\n"].concat()
}

/// The DUSRSECJ parsers over the input as a whole deck and as the in-stream data of one.
fn exercise_usrsec_decks(bytes: &[u8]) {
    exercise_usrsec(bytes);
    exercise_usrsec(&usrsec_deck(bytes));
}

/// `coacct01::decode_request` (CDRA). Only bytes 0-14 decide the outcome: a body shorter than 15
/// bytes is a short-body error, a non-digit in bytes 4-14 names its offset, and an accepted
/// body yields its first four bytes and its eleven key digits unchanged.
// audit: COACCT01-002
fn exercise_cdra_request(body: &[u8]) {
    let len = body.len();
    match decode_request(body) {
        Ok(request) => {
            assert!(
                Some(request.function().as_bytes().as_slice()) == body.get(..CDRA_FUNCTION_LEN),
                "decode_request: the function is not bytes 0-3 of a {len}-byte body"
            );
            let key = format!("{:011}", request.key().get());
            assert!(
                Some(key.as_bytes()) == body.get(CDRA_FUNCTION_LEN..REQUEST_LEN),
                "decode_request: the key is not bytes 4-14 of a {len}-byte body"
            );
        }
        Err(error) => {
            let consistent = match error {
                CdraRequestError::ShortBody { len: reported } => {
                    reported == len && len < REQUEST_LEN
                }
                CdraRequestError::NonDigitKey { offset } => {
                    len >= REQUEST_LEN
                        && (CDRA_FUNCTION_LEN..REQUEST_LEN).contains(&offset)
                        && body.get(offset).is_some_and(|byte| !byte.is_ascii_digit())
                }
            };
            assert!(
                consistent,
                "decode_request: the error does not describe a {len}-byte body"
            );
            check_error("decode_request", len, &error);
        }
    }
}

/// Whether field 3 of a CP00 body, cut at every comma, is exactly 16 ASCII digits: the card rule
/// that decides between a card-number error and every other outcome.
fn card_field_parses(body: &[u8]) -> bool {
    body.split(|&byte| byte == b',')
        .nth(2)
        .is_some_and(|field| is_digits(field, CARD_LEN))
}

/// The decline a field error is answered with: 63 bytes opening with the redacted card, response
/// `05` and reason `9000`.
fn check_field_error_reply(error: &FieldError, len: usize) {
    let reply = reply_body(&field_error_reply(error));
    assert!(
        reply.len() == REPLY_WRITTEN_LEN,
        "reply_body: a field-error reply is not {REPLY_WRITTEN_LEN} bytes for a {len}-byte body"
    );
    assert!(
        reply.get(..CARD_LEN) == Some(error.card.redacted_bytes().as_slice())
            && reply.starts_with(&CARD_MASK),
        "reply_body: a field-error reply does not open with the redacted card for a {len}-byte body"
    );
    assert!(
        reply.get(REPLY_RESPONSE_CODE) == Some(AuthReply::DECLINED.as_slice())
            && reply.get(REPLY_REASON_CODE) == Some(b"9000".as_slice()),
        "reply_body: a field-error reply is not the 05/9000 decline for a {len}-byte body"
    );
    check_error("FieldError", len, error);
}

/// `copaua0c::parse_request` (CP00): exactly one outcome per body. The card check runs first, so
/// a card-number error is returned exactly when field 3 is not 16 digits and carries the body's
/// length; a field error's reply is the redacted `05`/`9000` decline; a request fits the 500-byte
/// buffer and encodes to its storage layout.
// audit: COPAUA0C-005
// audit: COPAUA0C-017
fn exercise_cp00_request(body: &[u8]) {
    let len = body.len();
    let card_parses = card_field_parses(body);
    let outcome = parse_request(body);
    match &outcome {
        Ok(request) => {
            assert!(
                card_parses && len <= GET_BUFFER_LEN,
                "parse_request: accepted a {len}-byte body that breaks the card or length rule"
            );
            assert!(
                request.encode().len() == AuthRequestRecord::WIDTH,
                "parse_request: an accepted {len}-byte body did not encode to its layout"
            );
        }
        Err(ParseError::CardNumber(error)) => {
            assert!(
                !card_parses && error.body_len == len,
                "parse_request: a card-number error does not describe a {len}-byte body"
            );
            check_error("CardNumberError", len, error);
        }
        Err(ParseError::Field(error)) => {
            assert!(
                card_parses,
                "parse_request: a field error for a {len}-byte body whose card does not parse"
            );
            assert!(
                len <= GET_BUFFER_LEN || error.fault == FieldFault::BodyTooLong { len },
                "parse_request: a {len}-byte body over the buffer is not reported as too long"
            );
            check_field_error_reply(error, len);
        }
    }
    if let Err(error) = &outcome {
        check_error("parse_request", len, error);
    }
    let id = PicX::<15>::moved_from(body);
    let screened = transaction_id_screen(&id);
    assert!(
        &screened == id.as_bytes() || screened == [b'*'; 15],
        "transaction_id_screen: neither copied nor withheld a {len}-byte field"
    );
}

/// `cobtupdt::parse_input` on the input and its 53-byte windows, and `decode_input` over the
/// whole file; every accepted record is planned as the job would plan it.
fn exercise_cobtupdt_input(bytes: &[u8]) {
    let len = bytes.len();
    let mut maintenance = Maintenance::new();
    for window in windows(bytes, INPUT_RECORD_LEN) {
        match parse_input(window) {
            Ok(record) => {
                assert!(
                    record.raw().as_slice() == window,
                    "parse_input: an accepted record is not its {len}-byte body"
                );
                let _ = maintenance.plan(&record);
            }
            Err(error) => {
                assert!(
                    error.line.is_none(),
                    "parse_input: a line number for a body checked alone"
                );
                check_error("parse_input", len, &error);
            }
        }
    }
    match decode_input(bytes) {
        Ok(records) => {
            for record in &records {
                let _ = maintenance.plan(record);
            }
        }
        Err(error) => {
            assert!(
                error.line.is_some(),
                "decode_input: no line number for a {len}-byte file"
            );
            check_error("decode_input", len, &error);
        }
    }
}

/// `PurgeParams::parse` (SYSIN): an accepted value holds at most 80 bytes, and `PRM-INFO` is its
/// first 17 bytes padded with spaces.
fn exercise_purge_params(sysin: &[u8]) {
    let len = sysin.len();
    match PurgeParams::parse(sysin) {
        Ok(params) => {
            let padded = sysin
                .iter()
                .copied()
                .chain(iter::repeat(b' '))
                .take(PRM_INFO_LEN);
            assert!(
                len <= SYSIN_RECORD_LEN && params.raw().iter().copied().eq(padded),
                "PurgeParams::parse: PRM-INFO is not the padded start of a {len}-byte value"
            );
        }
        Err(error) => check_error("PurgeParams::parse", len, &error),
    }
}

/// `ParmDate::parse` (PARM): an accepted value's first 10 bytes are `PARM-DATE`, and every
/// SYSTRAN `TRAN-ID` built from it starts with them.
fn exercise_parm_date(parm: &[u8]) {
    let len = parm.len();
    match ParmDate::parse(parm) {
        Ok(date) => {
            let head = parm.get(..PARM_DATE_LEN);
            let id = interest_transaction_id(&date, PicU::<6>::ZERO);
            assert!(
                Some(date.as_bytes().as_slice()) == head
                    && id.as_bytes().get(..PARM_DATE_LEN) == head,
                "ParmDate::parse: PARM-DATE is not the first 10 bytes of a {len}-byte value"
            );
        }
        Err(error) => check_error("ParmDate::parse", len, &error),
    }
}

/// `AuthKey::parse` and `edit_account`, the path segments of endpoints 21 to 23: an accepted key
/// renders back to its text, an accepted account is its 11 digits.
fn exercise_path_segments(text: &str) {
    let len = text.len();
    match AuthKey::parse(text) {
        Ok(key) => assert!(
            key.render() == text,
            "AuthKey::parse: an accepted {len}-byte key does not render back to its text"
        ),
        Err(error) => check_error("AuthKey::parse", len, &error),
    }
    match edit_account(text) {
        Ok(acct_id) => assert!(
            format!("{:011}", acct_id.get()) == text,
            "edit_account: an accepted {len}-byte account is not its 11 digits"
        ),
        Err(error) => check_error("edit_account", len, &error),
    }
}

/// The `sensitive` text parsers. Each accepts exactly the text its rule names, its `TryFrom`
/// agrees with its `parse`, and an accepted value formats only in its redacted form.
fn exercise_sensitive_text(text: &str) {
    let len = text.len();
    let card = CardNumber::parse(text);
    assert!(
        card.is_ok() == is_digits(text.as_bytes(), CARD_LEN) && CardNumber::try_from(text) == card,
        "CardNumber::parse: wrong outcome for {len}-byte text"
    );
    match &card {
        Ok(card) => assert!(
            card.to_string() == card.redacted() && format!("{card:?}") == card.redacted(),
            "CardNumber: a parsed card formats other than redacted"
        ),
        Err(error) => check_error("CardNumber::parse", len, error),
    }
    let ssn = Ssn::parse(text);
    assert!(
        ssn.is_ok() == is_digits(text.as_bytes(), SSN_LEN) && Ssn::try_from(text) == ssn,
        "Ssn::parse: wrong outcome for {len}-byte text"
    );
    match &ssn {
        Ok(ssn) => assert!(
            ssn.to_string() == ssn.redacted() && ssn.redacted().starts_with("***-**-"),
            "Ssn: a parsed SSN formats other than redacted"
        ),
        Err(error) => check_error("Ssn::parse", len, error),
    }
    match Password::from_input(text) {
        Ok(password) => assert!(
            len <= PASSWORD_LEN && password.to_string() == "*".repeat(PASSWORD_LEN),
            "Password::from_input: accepted {len}-byte text or formats it unredacted"
        ),
        Err(error) => {
            assert!(
                len > PASSWORD_LEN,
                "Password::from_input: refused {len}-byte text"
            );
            check_error("Password::from_input", len, &error);
        }
    }
}

/// The remaining text parsers: the path segments, the `sensitive` values, a `PostgreSQL`
/// SQLSTATE for CP00's log row, and the configured CDRA request queue name of `read_failure`.
fn exercise_text(text: &str) {
    exercise_path_segments(text);
    exercise_sensitive_text(text);
    if let Some(state) = SqlState::parse(text) {
        assert!(
            state.as_bytes().as_slice() == text.as_bytes(),
            "SqlState::parse: an accepted state is not its text"
        );
    }
    let _ = coacct01::read_failure(text).error_body();
}

/// [`exercise_text`] over the lossy text of the input and of its windows at every text width.
fn exercise_text_parsers(bytes: &[u8]) {
    exercise_text(&String::from_utf8_lossy(bytes));
    for width in TEXT_WIDTHS {
        for window in windows(bytes, width).skip(1) {
            exercise_text(&String::from_utf8_lossy(window));
        }
    }
}

/// `password::verify` of [`VERIFY_PASSWORD`] against the lossy text of the input, as the
/// `users.password_hash` PHC string read back from the database. The PHC parser refuses empty
/// text and text with anything before its first `$`, so text not opening with `$` is a
/// malformed hash; an answer needs the algorithm, salt and hash output fields, so at least three
/// `$`; an error is one of the two stored-hash failures, never a hashing failure.
fn exercise_password_verify(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let len = text.len();
    let phc_shaped = text.starts_with('$');
    match password::verify(VERIFY_PASSWORD, &text) {
        Ok(_) => assert!(
            phc_shaped && text.matches('$').count() >= 3,
            "password::verify: answered for {len}-byte text that is not a complete PHC string"
        ),
        Err(error) => {
            assert!(
                phc_shaped || error == PasswordError::MalformedHash,
                "password::verify: {len}-byte text not opening with `$` is not a malformed hash"
            );
            assert!(
                error != PasswordError::Hash,
                "password::verify: a hashing failure for {len}-byte text"
            );
            check_error("password::verify", len, &error);
        }
    }
}

/// `Db2TransactionType::new` and `Db2TransactionCategory::new`: text up to `VARCHAR(50)` is
/// stored unchanged, longer text is a typed error.
fn exercise_db2_rows(bytes: &[u8]) {
    for window in windows(bytes, VARCHAR_MAX).chain(windows(bytes, VARCHAR_MAX + 1)) {
        let text = String::from_utf8_lossy(window).into_owned();
        let len = text.len();
        let fits = len <= VARCHAR_MAX;
        let code = PicX::<2>::moved_from(window);
        match Db2TransactionType::new(code.clone(), text.clone()) {
            Ok(row) => assert!(
                fits && row.tr_description() == text && row.tr_type() == &code,
                "Db2TransactionType::new: a {len}-byte description not stored unchanged"
            ),
            Err(error) => {
                assert!(!fits, "Db2TransactionType::new: refused {len}-byte text");
                check_error("Db2TransactionType::new", len, &error);
            }
        }
        let category = PicX::<4>::moved_from(window);
        match Db2TransactionCategory::new(code, category, text.clone()) {
            Ok(row) => assert!(
                fits && row.cat_data() == text,
                "Db2TransactionCategory::new: a {len}-byte description not stored unchanged"
            ),
            Err(error) => {
                assert!(
                    !fits,
                    "Db2TransactionCategory::new: refused {len}-byte text"
                );
                check_error("Db2TransactionCategory::new", len, &error);
            }
        }
    }
}

/// A card field holding any 16 bytes, as a decoded record holds it: its redacted bytes, text and
/// `CSSL` event key show twelve `*` and the field's last four bytes only.
fn exercise_card_field(bytes: &[u8]) {
    for window in windows(bytes, CARD_LEN) {
        let raw = PicX::<CARD_LEN>::moved_from(window);
        let last4 = raw.as_bytes().get(CARD_MASK.len()..);
        let card = CardNumber::new(raw.clone());
        let redacted = card.redacted_bytes();
        let key = card_event_key(&card);
        assert!(
            redacted.starts_with(&CARD_MASK)
                && redacted.get(CARD_MASK.len()..) == last4
                && card.last4().as_slice() == last4.unwrap_or_default()
                && key.as_bytes().starts_with(&redacted)
                && card.redacted().chars().count() == CARD_LEN,
            "CardNumber: a {}-byte field is not redacted to its last four bytes",
            window.len()
        );
    }
}

/// Every parser of this file over `bytes`.
fn exercise_all(bytes: &[u8]) {
    exercise_records(bytes);
    exercise_fixture_loads(bytes);
    exercise_usrsec_decks(bytes);
    exercise_cdra_request(bytes);
    exercise_cp00_request(bytes);
    exercise_cobtupdt_input(bytes);
    exercise_purge_params(bytes);
    exercise_parm_date(bytes);
    exercise_text_parsers(bytes);
    exercise_password_verify(bytes);
    exercise_db2_rows(bytes);
    exercise_card_field(bytes);
}

/// Replacement values for a field `width` bytes wide that its decoder rejects, in the order
/// [`template`] tries them: a date or timestamp of that width, a zero amount with its positive
/// overpunch, a zero `PIC +9(n).99` edit, zero digits, spaces, a packed zero and a binary zero.
fn repairs(width: usize) -> Vec<Vec<u8>> {
    let Some(lead) = width.checked_sub(1) else {
        return Vec::new();
    };
    let mut values: Vec<Vec<u8>> = [SAMPLE_DATE, SAMPLE_TIMESTAMP]
        .into_iter()
        .filter(|sample| sample.len() == width)
        .map(<[u8]>::to_vec)
        .collect();
    values.push(iter::repeat_n(b'0', lead).chain(iter::once(b'{')).collect());
    if let Some(integer_digits) = width.checked_sub(EDITED_POINT_AND_SIGN) {
        let digits = iter::repeat_n(b'0', integer_digits);
        values.push(iter::once(b'+').chain(digits).chain(*b".00").collect());
    }
    values.push(vec![b'0'; width]);
    values.push(vec![b' '; width]);
    values.push(iter::repeat_n(0x00, lead).chain(iter::once(0x0C)).collect());
    values.push(vec![0x00; width]);
    values
}

/// Dates and timestamps the field decoders must reject or keep in their own layout: impossible
/// or partly blank dates, the Db2 `D` and screen `S` timestamp layouts, and impossible times.
const HOSTILE_TEXT: [&[u8]; 12] = [
    b"2022-02-30",
    b"2022-13-01",
    b"2022-07-  ",
    b"0000-00-00",
    b"2022/07/18",
    b"9999-12-31",
    b"2022-07-18-10.11.12.123456",
    b"2022-07-18                ",
    b"2022-07-18 24:00:00.000000",
    b"2022-02-30 10:11:12.123456",
    b"2022-07-18 10:11:12.12345 ",
    b"          10:11:12.123456",
];

/// Values a field `width` bytes wide is rewritten with by the hostile-value sweep: zoned negative
/// zero, the extreme overpunch digits, packed negative zero, an `F` and an invalid sign, the
/// extreme and negative-zero `PIC +9(n).99` edits, high-values, and [`HOSTILE_TEXT`] of that
/// width.
fn hostile_values(width: usize) -> Vec<Vec<u8>> {
    let Some(lead) = width.checked_sub(1) else {
        return Vec::new();
    };
    let ending = |fill: u8, last: u8| -> Vec<u8> {
        iter::repeat_n(fill, lead).chain(iter::once(last)).collect()
    };
    let mut values = vec![
        ending(b'0', b'}'),
        ending(b'9', b'I'),
        ending(b'9', b'R'),
        ending(0x00, 0x0D),
        ending(0x99, 0x9D),
        ending(0x00, 0x0F),
        ending(0x00, 0x0A),
        vec![0xFF; width],
    ];
    if let Some(integer_digits) = width.checked_sub(EDITED_POINT_AND_SIGN) {
        for (sign, digit, decimals) in [
            (b'-', b'0', b".00"),
            (b'+', b'9', b".99"),
            (b'-', b'9', b",99"),
        ] {
            let digits = iter::repeat_n(digit, integer_digits);
            values.push(iter::once(sign).chain(digits).chain(*decimals).collect());
        }
    }
    values.extend(
        HOSTILE_TEXT
            .into_iter()
            .filter(|text| text.len() == width)
            .map(<[u8]>::to_vec),
    );
    values
}

/// A record of one layout that its decode accepts, built by [`template`].
struct Template {
    /// The layout's type name.
    layout: &'static str,
    /// The record bytes.
    bytes: Vec<u8>,
    /// Whether the repair reached a record the decode accepts.
    decodes: bool,
    /// `(offset, width)` of every field the repair rewrote: the numeric, date and timestamp
    /// fields, which spaces do not satisfy.
    fields: Vec<(usize, usize)>,
}

/// A record of `R::WIDTH` bytes that `R::decode` accepts.
///
/// It starts from spaces and repairs the first rejected field, the one the decode error names by
/// offset and width, with the first of its [`repairs`] that moves the error past it, until the
/// record decodes. The templates seed the mutation property, whose inputs then reach every field
/// decoder of every layout rather than stopping at a record's first field, and the hostile-value
/// sweep, which rewrites each repaired field.
fn template<R: FixedRecord>() -> Template {
    let mut bytes = vec![b' '; R::WIDTH];
    let mut fields = Vec::new();
    for _ in 0..R::WIDTH {
        let Err(error) = R::decode(&bytes) else {
            break;
        };
        let end = error.offset.saturating_add(error.width);
        fields.push((error.offset, error.width));
        let mut moved_on = false;
        for value in repairs(error.width) {
            if let Some(field) = bytes.get_mut(error.offset..end) {
                field.copy_from_slice(&value);
            }
            match R::decode(&bytes) {
                Err(next) if next.offset <= error.offset => {}
                _ => {
                    moved_on = true;
                    break;
                }
            }
        }
        if !moved_on {
            break;
        }
    }
    Template {
        layout: any::type_name::<R>(),
        decodes: R::decode(&bytes).is_ok(),
        bytes,
        fields,
    }
}

/// The template of every layout [`exercise_records`] decodes.
fn record_templates() -> Vec<Template> {
    vec![
        template::<AccountRecord>(),
        template::<CardRecord>(),
        template::<CardXrefRecord>(),
        template::<CustomerRecord>(),
        template::<TransactionRecord>(),
        template::<DailyTransactionRecord>(),
        template::<RejectRecord>(),
        template::<TranCatBalanceRecord>(),
        template::<DisclosureGroupRecord>(),
        template::<TransactionTypeRecord>(),
        template::<TransactionCategoryRecord>(),
        template::<UserSecurityRecord>(),
        template::<PendingAuthSummaryRecord>(),
        template::<PendingAuthDetailRecord>(),
        template::<AuthRequestRecord>(),
        template::<AuthReply>(),
        template::<ErrorLogRecord>(),
        template::<MaintenanceRecord>(),
    ]
}

/// The bytes of every record template.
fn record_template_bytes() -> Vec<Vec<u8>> {
    record_templates()
        .into_iter()
        .map(|template| template.bytes)
        .collect()
}

/// A synthetic card number, built at run time so that no 16-digit literal sits in the source.
fn synthetic_card() -> Vec<u8> {
    vec![b'7'; CARD_LEN]
}

/// The fields of a CP00 body every rule accepts, with `card` as field 3: 18 printable fields,
/// fields 8 and 12 digits, field 9 a valid `NUMVAL` amount.
fn cp00_fields(card: &[u8]) -> Vec<Vec<u8>> {
    let fields: [&[u8]; CP00_FIELDS] = [
        b"260101",
        b"120000",
        card,
        b"0100",
        b"1225",
        b"MSGTYP",
        b"SOURCE",
        b"000000",
        b"100.00",
        b"5411",
        b"USA",
        b"01",
        b"MERCHANT0000001",
        b"TEST MERCHANT",
        b"TEST CITY",
        b"NY",
        b"100010000",
        b"TXN000000000001",
    ];
    fields.into_iter().map(<[u8]>::to_vec).collect()
}

/// [`cp00_fields`] joined by commas: the body with 17 commas.
fn cp00_body_with(card: &[u8]) -> Vec<u8> {
    cp00_fields(card).join(&b',')
}

/// A synthetic 57-byte DUSRSECJ in-stream line for user `id`: names, an 8-byte password of `Z`s
/// and type `U`, the shape of `DUSRSECJ.jcl:35-44` with none of its values.
fn usrsec_line(id: &[u8]) -> Vec<u8> {
    let user = PicX::<8>::moved_from(id);
    let first_name = PicX::<20>::moved_from(b"TEST");
    let last_name = PicX::<20>::moved_from(b"USER");
    let parts: [&[u8]; 5] = [
        user.as_bytes(),
        first_name.as_bytes(),
        last_name.as_bytes(),
        &[b'Z'; 8],
        b"U",
    ];
    parts.concat()
}

/// A MNTTRDB2 input record: action `A`, type code `01` and a 50-byte description.
fn mnttrdb2_line() -> Vec<u8> {
    let description = PicX::<50>::moved_from(b"TEST TYPE");
    [b"A01".as_slice(), description.as_bytes()].concat()
}

/// A valid input of every message, deck, option and path-segment parser.
fn message_templates() -> Vec<Vec<u8>> {
    let instream = [usrsec_line(b"TESTUSR1"), usrsec_line(b"TESTUSR2")].join(&b'\n');
    vec![
        cp00_body_with(&synthetic_card()),
        b"INQA00000000001".to_vec(),
        usrsec_deck(&instream),
        mnttrdb2_line(),
        DEFAULT_SYSIN.as_bytes().to_vec(),
        DEFAULT_PARM_DATE.as_bytes().to_vec(),
        AuthKey::lowest().render().into_bytes(),
    ]
}

/// Every template, records and messages.
fn templates() -> Vec<Vec<u8>> {
    let mut all = record_template_bytes();
    all.extend(message_templates());
    all
}

/// `template` with each `(position, byte)` written over it.
fn mutated(template: &[u8], mutations: &[(Index, u8)]) -> Vec<u8> {
    let mut bytes = template.to_vec();
    let len = bytes.len();
    if len > 0 {
        for (position, byte) in mutations {
            if let Some(slot) = bytes.get_mut(position.index(len)) {
                *slot = *byte;
            }
        }
    }
    bytes
}

/// Leading parts of a generated CP00 amount: none, blank, either sign, or a blank then a sign.
const AMOUNT_LEADS: [&[u8]; 5] = [b"", b" ", b"+", b"-", b" -"];

/// Trailing parts of a generated CP00 amount: none, blank, either sign, `CR` or `DB`.
const AMOUNT_TRAILS: [&[u8]; 6] = [b"", b" ", b"-", b"+", b"CR", b"DB"];

/// Line terminators of a generated fixture file: LF, CRLF, and none on the last line.
const TERMINATORS: [&[u8]; 3] = [b"\n", b"\r\n", b""];

/// A byte written over a template: any byte, or one the parsers give meaning to.
fn mutation_byte() -> impl Strategy<Value = u8> {
    Union::new([
        any::<u8>().boxed(),
        prop::sample::select(RECORD_TEXT).boxed(),
    ])
}

/// One of [`templates`] with one to eight [`mutation_byte`]s written over it.
fn mutated_template() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::sample::select(templates()),
        prop::collection::vec((any::<Index>(), mutation_byte()), 1..=8),
    )
        .prop_map(|(template, mutations)| mutated(&template, &mutations))
}

/// Text of [`RECORD_TEXT`] bytes whose length is within two bytes of a layout's width.
fn record_shaped() -> impl Strategy<Value = Vec<u8>> {
    prop::sample::select(LAYOUT_WIDTHS.to_vec()).prop_flat_map(|width| {
        prop::collection::vec(
            prop::sample::select(RECORD_TEXT),
            width.saturating_sub(2)..=width.saturating_add(2),
        )
    })
}

/// A CP00 field: mostly printable text, sometimes CR, LF, commas or any byte.
fn cp00_text_field() -> impl Strategy<Value = Vec<u8>> {
    Union::new_weighted(vec![
        (
            8,
            prop::collection::vec(prop::sample::select(FIELD_TEXT), 0..=24).boxed(),
        ),
        (
            1,
            prop::collection::vec(prop::sample::select(RECORD_TEXT), 0..=24).boxed(),
        ),
        (1, prop::collection::vec(any::<u8>(), 0..=4).boxed()),
    ])
}

/// A CP00 `PIC 9(n)` field, 8 or 12: mostly digits.
fn cp00_digit_field() -> impl Strategy<Value = Vec<u8>> {
    Union::new_weighted(vec![
        (3, prop::collection::vec(b'0'..=b'9', 0..=8).boxed()),
        (1, cp00_text_field().boxed()),
    ])
}

/// CP00 field 3: mostly 16 digits, so the parse passes the card check.
fn cp00_card_field() -> impl Strategy<Value = Vec<u8>> {
    Union::new_weighted(vec![
        (3, prop::collection::vec(b'0'..=b'9', CARD_LEN).boxed()),
        (1, cp00_text_field().boxed()),
    ])
}

/// CP00 field 9: mostly `NUMVAL`-shaped text, valid or not (two signs, a bare point).
fn cp00_amount_field() -> impl Strategy<Value = Vec<u8>> {
    let numeric = (
        prop::sample::select(AMOUNT_LEADS.to_vec()),
        prop::collection::vec(b'0'..=b'9', 0..=11),
        prop::option::of(prop::collection::vec(b'0'..=b'9', 0..=3)),
        prop::sample::select(AMOUNT_TRAILS.to_vec()),
    )
        .prop_map(|(lead, integer, fraction, trail)| {
            let mut amount = lead.to_vec();
            amount.extend(integer);
            if let Some(fraction) = fraction {
                amount.push(b'.');
                amount.extend(fraction);
            }
            amount.extend_from_slice(trail);
            amount
        });
    Union::new_weighted(vec![(3, numeric.boxed()), (1, cp00_text_field().boxed())])
}

/// A CP00 body of 18 fields in wire order, sometimes cut to fewer, followed by up to two extra
/// fields and sometimes a final comma.
fn cp00_body() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec(cp00_text_field(), 2),
        cp00_card_field(),
        prop::collection::vec(cp00_text_field(), 4),
        cp00_digit_field(),
        cp00_amount_field(),
        prop::collection::vec(cp00_text_field(), 2),
        cp00_digit_field(),
        prop::collection::vec(cp00_text_field(), 6),
        prop::collection::vec(cp00_text_field(), 0..=2),
        Union::new_weighted(vec![
            (4, Just(usize::MAX).boxed()),
            (1, (0..=CP00_FIELDS).boxed()),
        ]),
        any::<bool>(),
    )
        .prop_map(
            |(
                head,
                card,
                middle,
                processing,
                amount,
                codes,
                entry_mode,
                merchant,
                extra,
                keep,
                comma,
            )| {
                let mut fields: Vec<Vec<u8>> = head
                    .into_iter()
                    .chain([card])
                    .chain(middle)
                    .chain([processing, amount])
                    .chain(codes)
                    .chain([entry_mode])
                    .chain(merchant)
                    .chain(extra)
                    .collect();
                fields.truncate(keep);
                let mut body = fields.join(&b',');
                if comma {
                    body.push(b',');
                }
                body
            },
        )
}

/// A CDRA body: a function (often `INQA`), a key of mostly digits, of any length up to 11, and
/// any trailing bytes.
fn cdra_body() -> impl Strategy<Value = Vec<u8>> {
    (
        Union::new([
            Just(b"INQA".to_vec()).boxed(),
            prop::collection::vec(prop::sample::select(FIELD_TEXT), 0..=CDRA_FUNCTION_LEN).boxed(),
        ]),
        prop::collection::vec(
            Union::new_weighted(vec![
                (9, (b'0'..=b'9').boxed()),
                (1, mutation_byte().boxed()),
            ]),
            0..=11,
        ),
        prop::collection::vec(any::<u8>(), 0..=24),
    )
        .prop_map(|(function, key, rest)| [function, key, rest].concat())
}

/// Algorithm fields of a generated PHC string: argon2's three and one argon2 does not run.
const PHC_ALGORITHMS: [&[u8]; 4] = [b"argon2id", b"argon2i", b"argon2d", b"md5"];

/// Version fields of a generated PHC string: argon2's two and one it does not support.
const PHC_VERSIONS: [&[u8]; 3] = [b"v=19", b"v=16", b"v=99"];

/// The B64 alphabet of PHC salts and hash outputs. It holds no `=`, so the parser never reads a
/// salt or output field as parameters.
const PHC_B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Bytes of a free-form PHC field: the PHC separators, letters and a byte that is not UTF-8, but
/// no digit, so no free-form field can set an argon2 cost.
const PHC_FREE: &[u8] = b"=,.-+/abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\xFF";

/// A free-form PHC field of up to `max` [`PHC_FREE`] bytes.
fn phc_free(max: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop::sample::select(PHC_FREE), 0..=max)
}

/// A field of up to `max` [`PHC_B64`] characters, half the time a whole number of four-character
/// groups, which always decodes, so salts and outputs reach argon2 as well as the parser's length
/// and encoding errors.
fn phc_b64(max: usize) -> impl Strategy<Value = Vec<u8>> {
    let groups = (0..=max / 4).prop_map(|count| count.saturating_mul(4));
    Union::new([groups.boxed(), (0..=max).boxed()])
        .prop_flat_map(|len| prop::collection::vec(prop::sample::select(PHC_B64), len))
}

/// A PHC version field: one of [`PHC_VERSIONS`], or free-form text with or without the `v=`
/// that marks a version.
fn phc_version() -> impl Strategy<Value = Vec<u8>> {
    Union::new_weighted(vec![
        (
            3,
            prop::sample::select(PHC_VERSIONS.to_vec())
                .prop_map(<[u8]>::to_vec)
                .boxed(),
        ),
        (1, phc_free(12).boxed()),
        (
            1,
            phc_free(12)
                .prop_map(|value| [b"v=".as_slice(), &value].concat())
                .boxed(),
        ),
    ])
}

/// A PHC parameter field: `m`, `t` and `p` of at most 64 KiB, 3 passes and 3 lanes, on both
/// sides of argon2's minimums, or free-form text running past the parser's 127-byte limit.
fn phc_params() -> impl Strategy<Value = Vec<u8>> {
    Union::new_weighted(vec![
        (
            4,
            (0_u32..=64, 0_u32..=3, 0_u32..=3)
                .prop_map(|(m, t, p)| format!("m={m},t={t},p={p}").into_bytes())
                .boxed(),
        ),
        (1, phc_free(130).boxed()),
    ])
}

/// Text shaped like the PHC string `users.password_hash` stores: `$` and an algorithm, then an
/// optional version, parameter field, salt with an optional hash output after it, and trailing
/// field, each valid or not, so the text reaches the `password::verify` mismatch answer
/// `Ok(false)` and both stored-hash errors, `MalformedHash` and `Verify`. `Ok(true)` needs a
/// hash of [`VERIFY_PASSWORD`] and is proved by `src/password.rs`'s own tests.
///
/// Cost bound: `verify` runs argon2 with the cost the text sets, so only the bounded `m`, `t`
/// and `p` of [`phc_params`] may set one. Free-form fields hold no digit, the B64 salt and output
/// hold no `=` and so are never read as parameters, and the selected algorithm and version sit
/// where the parser reads only an algorithm and a version: no text sets more than 64 KiB, 3
/// passes and 3 lanes. A text that sets no cost runs argon2's defaults (19 MiB, 2 passes), the
/// cost of every stored hash, so the parameter field is left out one time in forty. A text is at
/// most 374 bytes, within [`MAX_LEN`].
fn phc_text() -> impl Strategy<Value = Vec<u8>> {
    (
        Union::new_weighted(vec![
            (
                4,
                prop::sample::select(PHC_ALGORITHMS.to_vec())
                    .prop_map(<[u8]>::to_vec)
                    .boxed(),
            ),
            (1, phc_free(40).boxed()),
        ]),
        Union::new_weighted(vec![
            (4, phc_version().prop_map(Some).boxed()),
            (1, Just(None).boxed()),
        ]),
        Union::new_weighted(vec![
            (39, phc_params().prop_map(Some).boxed()),
            (1, Just(None).boxed()),
        ]),
        Union::new_weighted(vec![
            (
                4,
                (phc_b64(70), phc_b64(90))
                    .prop_map(|(salt, hash)| vec![salt, hash])
                    .boxed(),
            ),
            (1, phc_b64(70).prop_map(|salt| vec![salt]).boxed()),
            (1, Just(Vec::new()).boxed()),
        ]),
        Union::new_weighted(vec![
            (3, Just(None).boxed()),
            (1, phc_free(24).prop_map(Some).boxed()),
        ]),
    )
        .prop_map(|(algorithm, version, params, salt_and_hash, tail)| {
            let fields = iter::once(algorithm)
                .chain(version)
                .chain(params)
                .chain(salt_and_hash)
                .chain(tail);
            let mut text = Vec::new();
            for field in fields {
                text.push(b'$');
                text.extend(field);
            }
            text
        })
}

/// A fixture file of one to four lines from one record template, each mutated in up to two
/// bytes, a quarter of them cut short as the 36-byte `cardxref.txt` bodies are, each ended by
/// any terminator.
fn fixture_file() -> impl Strategy<Value = Vec<u8>> {
    let line = (
        prop::collection::vec((any::<Index>(), mutation_byte()), 0..=2),
        Union::new_weighted(vec![
            (3, Just(None).boxed()),
            (1, any::<Index>().prop_map(Some).boxed()),
        ]),
        prop::sample::select(TERMINATORS.to_vec()),
    );
    (
        prop::sample::select(record_template_bytes()),
        prop::collection::vec(line, 1..=4),
    )
        .prop_map(|(template, lines)| {
            let mut file = Vec::new();
            for (mutations, cut, terminator) in lines {
                let mut body = mutated(&template, &mutations);
                if let Some(cut) = cut {
                    body.truncate(cut.index(body.len().saturating_add(1)));
                }
                file.extend(body);
                file.extend_from_slice(terminator);
            }
            file
        })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn no_parser_panics(bytes in generated(prop::collection::vec(any::<u8>(), 0..=MAX_LEN))) {
        exercise_all(&bytes.0);
    }

    #[test]
    fn record_shaped_text_reaches_every_parser(bytes in generated(record_shaped())) {
        exercise_all(&bytes.0);
    }

    #[test]
    fn cp00_bodies_reach_every_outcome(body in generated(cp00_body())) {
        exercise_all(&body.0);
    }

    #[test]
    fn cdra_bodies_reach_every_outcome(body in generated(cdra_body())) {
        exercise_all(&body.0);
    }

    #[test]
    fn phc_shaped_text_reaches_verify_mismatch_and_errors(text in generated(phc_text())) {
        prop_assert!(
            text.0.len() <= MAX_LEN,
            "phc_text: a {}-byte text is over the input bound",
            text.0.len()
        );
        exercise_all(&text.0);
    }

    #[test]
    fn mutated_templates_reach_every_field_decoder(bytes in generated(mutated_template())) {
        exercise_all(&bytes.0);
    }

    #[test]
    fn fixture_files_reach_every_load(file in generated(fixture_file())) {
        exercise_fixture_loads(&file.0);
        exercise_cobtupdt_input(&file.0);
        exercise_usrsec_decks(&file.0);
    }
}

#[test]
fn uniform_fills_at_every_length() {
    for fill in FILLS {
        for len in 0..=MAX_LEN {
            exercise_all(&vec![fill; len]);
        }
    }
}

/// Each fill at exactly each layout's width, where a decoder first reads field content, alone,
/// as a fixture line under every terminator, and as the in-stream card of a deck.
#[test]
fn uniform_fills_at_every_layout_width() {
    for fill in FILLS {
        for width in LAYOUT_WIDTHS {
            let body = vec![fill; width];
            exercise_all(&body);
            for terminator in TERMINATORS {
                let line = [body.as_slice(), terminator].concat();
                exercise_fixture_loads(&line);
                exercise_cobtupdt_input(&line);
            }
            exercise_usrsec(&usrsec_deck(&body));
        }
    }
}

/// The mutation seeds decode, so mutated records reach every field of every layout; every prefix
/// of each, and two copies as a fixture file, reach the short-body and key-sequence paths.
#[test]
fn every_layout_has_a_decodable_template() {
    for template in record_templates() {
        let Template {
            layout,
            bytes,
            decodes,
            ..
        } = template;
        assert!(decodes, "{layout}: the repaired template does not decode");
        for end in 0..=bytes.len() {
            if let Some(prefix) = bytes.get(..end) {
                exercise_all(prefix);
            }
        }
        let twice = [bytes.as_slice(), b"\r\n", &bytes, b"\n"].concat();
        exercise_all(&twice);
    }
}

/// Each repaired field of each template rewritten with every [`hostile_values`] entry of its
/// width: valid other layouts and extreme values that must round-trip, and the typed-error
/// classes (negative zero, bad sign, impossible date) that random bytes almost never form.
#[test]
fn hostile_values_in_every_template_field() {
    for template in record_templates() {
        for &(offset, width) in &template.fields {
            for value in hostile_values(width) {
                let mut bytes = template.bytes.clone();
                if let Some(field) = bytes.get_mut(offset..offset.saturating_add(width)) {
                    field.copy_from_slice(&value);
                }
                exercise_records(&bytes);
                exercise_fixture_loads(&bytes);
            }
        }
    }
}

/// The message, deck, option and path-segment seeds are accepted by their parsers.
#[test]
fn message_templates_are_accepted() {
    let templates = message_templates();
    let [cp00, cdra, deck, mnttrdb2, sysin, parm, key] = templates.as_slice() else {
        panic!("message_templates: expected seven seeds");
    };
    assert!(parse_request(cp00).is_ok(), "CP00 seed refused");
    assert!(decode_request(cdra).is_ok(), "CDRA seed refused");
    assert!(
        decode_usrsec(deck).is_ok_and(|records| records.len() == 2),
        "DUSRSECJ seed refused"
    );
    assert!(parse_input(mnttrdb2).is_ok(), "MNTTRDB2 seed refused");
    assert!(PurgeParams::parse(sysin).is_ok(), "SYSIN seed refused");
    assert!(ParmDate::parse(parm).is_ok(), "PARM seed refused");
    assert!(
        AuthKey::parse(&String::from_utf8_lossy(key)).is_ok(),
        "authorization key seed refused"
    );
    for seed in &templates {
        exercise_all(seed);
    }
}

/// `body`'s parse outcome is a card-number error with `cause`.
fn is_card_error(body: &[u8], cause: CardNumberCause) -> bool {
    matches!(
        parse_request(body),
        Err(ParseError::CardNumber(CardNumberError { cause: found, body_len }))
            if found == cause && body_len == body.len()
    )
}

/// `body`'s parse outcome is a field error with `fault`.
fn is_field_error(body: &[u8], fault: &FieldFault) -> bool {
    matches!(parse_request(body), Err(ParseError::Field(error)) if error.fault == *fault)
}

/// CP00 bodies at the field-count, card and amount boundaries: one outcome each, the card check
/// first.
// audit: COPAUA0C-005
// audit: COPAUA0C-017
#[test]
fn cp00_edge_bodies() {
    let card = synthetic_card();
    let fields = cp00_fields(&card);
    let seventeen_commas = fields.join(&b',');
    let with = |index: usize, value: &[u8]| {
        let mut changed = fields.clone();
        if let Some(field) = changed.get_mut(index) {
            *field = value.to_vec();
        }
        changed.join(&b',')
    };
    let sixteen_commas = fields
        .get(..CP00_FIELDS - 1)
        .unwrap_or_default()
        .join(&b',');
    let eighteen_commas = [seventeen_commas.as_slice(), b","].concat();
    let nineteen_commas = [seventeen_commas.as_slice(), b",EXTRA,"].concat();
    let trailing_bytes = [seventeen_commas.as_slice(), b",TRAILING BYTES"].concat();

    assert!(is_card_error(b"", CardNumberCause::Absent), "empty body");
    assert!(
        is_card_error(b"260101", CardNumberCause::Absent),
        "0 commas"
    );
    assert!(
        is_card_error(b"260101,120000", CardNumberCause::Absent),
        "1 comma"
    );
    assert!(
        parse_request(&seventeen_commas).is_ok(),
        "17 commas, 18 fields"
    );
    assert!(
        parse_request(&eighteen_commas).is_ok(),
        "18 commas, the last ending field 18"
    );
    assert!(
        parse_request(&nineteen_commas).is_ok(),
        "19 commas, a 19th field ignored"
    );
    assert!(
        parse_request(&trailing_bytes).is_ok(),
        "bytes after the last comma ignored"
    );
    assert!(
        is_field_error(
            &sixteen_commas,
            &FieldFault::Missing(RequestField::TransactionId)
        ),
        "16 commas, field 18 missing"
    );
    assert!(
        is_field_error(&with(8, b""), &FieldFault::AmountNotNumeric),
        "a 16-digit card with an empty amount"
    );
    assert!(
        is_card_error(&with(2, b"ABC"), CardNumberCause::NotSixteenDigits),
        "card field ABC, the i11 dead-letter class"
    );
    assert!(
        is_card_error(b"260101,120000,ABC", CardNumberCause::NotSixteenDigits),
        "card field ABC with fields missing: the card check runs first"
    );
    let card_17 = [card.as_slice(), b"7"].concat();
    let card_15 = card.get(1..).unwrap_or_default();
    for wrong in [card_17.as_slice(), card_15, b"", b"77777777 7777777"] {
        assert!(
            is_card_error(&with(2, wrong), CardNumberCause::NotSixteenDigits),
            "a {}-byte card field that is not 16 digits",
            wrong.len()
        );
    }
    let long = [seventeen_commas.as_slice(), b",", &[b'X'; GET_BUFFER_LEN]].concat();
    assert!(
        is_field_error(&long, &FieldFault::BodyTooLong { len: long.len() }),
        "a body over the 500-byte buffer"
    );
    for body in [seventeen_commas, sixteen_commas, nineteen_commas, long] {
        exercise_all(&body);
    }
}

/// CDRA bodies at the length and key-digit boundaries.
// audit: COACCT01-002
#[test]
fn cdra_edge_bodies() {
    let short = |len| Err(CdraRequestError::ShortBody { len });
    let non_digit = |offset| Err(CdraRequestError::NonDigitKey { offset });
    assert!(decode_request(b"") == short(0), "empty body");
    assert!(decode_request(&[b'X'; 14]) == short(14), "14 bytes");
    assert!(
        decode_request(&[b'X'; REQUEST_LEN]) == non_digit(CDRA_FUNCTION_LEN),
        "exactly 15 bytes of non-digits"
    );
    assert!(
        decode_request(b"INQA0000000000X") == non_digit(14),
        "a non-digit in the last key byte"
    );
    assert!(
        decode_request(b"INQA0000000000 ") == non_digit(14),
        "a space in the last key byte"
    );
    let accepted = decode_request(b"inqa00000000001\xFF\x00 any trailing bytes");
    assert!(
        accepted.is_ok_and(
            |request| request.function().as_bytes() == b"inqa" && request.key().get() == 1
        ),
        "a body whose bytes 4-14 are digits, with arbitrary bytes after them"
    );
}

/// DUSRSECJ decks without the statement, without the delimiter, with the delimiter before the
/// statement, with a card over 80 bytes and with a duplicate user: each a typed error naming its
/// line, never a panic or a truncated card.
#[test]
fn usrsec_deck_edges() {
    let first = usrsec_line(b"TESTUSR1");
    let second = usrsec_line(b"TESTUSR2");
    let kind_and_line = |jcl: &[u8]| {
        let error = decode_usrsec(jcl).err();
        let lines_error = usrsec_instream_lines(jcl).err();
        assert!(
            error.is_some() && error == lines_error,
            "a {}-byte deck with a structure error",
            jcl.len()
        );
        error.map(|error| (error.kind(), error.line()))
    };

    let no_statement = [first.as_slice(), b"\n/*\n"].concat();
    assert!(
        kind_and_line(&no_statement) == Some((LoadErrorKind::InStreamStartMissing, None)),
        "no //SYSUT1 statement"
    );
    let no_delimiter = [SYSUT1_DD_STAR, b"\n", &first, b"\n"].concat();
    assert!(
        kind_and_line(&no_delimiter) == Some((LoadErrorKind::InStreamEndMissing, Some(1))),
        "a //SYSUT1 statement and no /*"
    );
    let delimiter_first = [b"/*\n", SYSUT1_DD_STAR, b"\n", &first, b"\n"].concat();
    assert!(
        kind_and_line(&delimiter_first) == Some((LoadErrorKind::InStreamEndMissing, Some(2))),
        "the /* line before the //SYSUT1 statement"
    );

    let long_card = usrsec_deck(&[b'A'; UserSecurityRecord::WIDTH + 1]);
    assert!(
        usrsec_instream_lines(&long_card).is_ok_and(|lines| lines.len() == 1)
            && decode_usrsec(&long_card).is_err_and(|error| {
                matches!(error.kind(), LoadErrorKind::Record(_)) && error.line() == Some(2)
            }),
        "an 81-byte in-stream card"
    );
    let duplicate = usrsec_deck(&[first.as_slice(), b"\n", &first].concat());
    assert!(
        decode_usrsec(&duplicate).is_err_and(|error| {
            error.kind() == LoadErrorKind::KeySequence { previous_line: 2 }
                && error.line() == Some(3)
        }),
        "a duplicate user ID"
    );
    let deck = usrsec_deck(&[first.as_slice(), b"\r\n", &second].concat());
    assert!(
        decode_usrsec(&deck).is_ok_and(|records| records.len() == 2),
        "two ascending users, the first ended by CRLF"
    );
    for jcl in [
        no_statement,
        no_delimiter,
        delimiter_first,
        long_card,
        duplicate,
        deck,
    ] {
        exercise_all(&jcl);
    }
}

/// `ErrorLevel::from_pic` and `ErrorSubsystem::from_pic` name a condition only for its own code
/// byte, for every byte value.
#[test]
fn error_log_conditions_cover_every_byte() {
    for byte in 0..=u8::MAX {
        let field = PicX::from_array([byte]);
        assert!(
            ErrorLevel::from_pic(&field).is_none_or(|level| level.code() == byte)
                && ErrorSubsystem::from_pic(&field)
                    .is_none_or(|subsystem| subsystem.code() == byte),
            "ERR-LEVEL or ERR-SUBSYSTEM names a condition for another byte"
        );
    }
}
