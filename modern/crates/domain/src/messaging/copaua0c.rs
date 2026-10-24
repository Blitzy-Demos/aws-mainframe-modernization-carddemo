//! CP00, the card-authorization request COPAUA0C serves over MQ: the request parse, the
//! decision with the i10 credit-limit guard, the reply body, and the summary, detail and
//! error-log records the worker writes (`COPAUA0C.cbl:351-380,438-466,657-932,983-1011`; AAP
//! 0.3.4.2).
//!
//! The module is pure: it parses bytes, decides and builds records, and reads time only from the
//! [`Clock`] it is given. `messaging::workers::cp00` does everything else: the one `PostgreSQL`
//! transaction and the summary row lock taken by the worker (`lock_or_create`, COPAUA0C-025,
//! COPAUA0C-026), the account and customer reads, the confirmed publish, the acknowledgement,
//! the rejection that dead-letters a request, the alert event, and the separate transaction of
//! each `cssl_error_log` row (COPAUA0C-110).
//!
//! | Item | COBOL | Rules |
//! |---|---|---|
//! | [`parse_request`], [`ParseError`] | `2100-EXTRACT-REQUEST-MSG` | COPAUA0C-002 to COPAUA0C-017 |
//! | [`field_error_reply`] | `6000-MAKE-DECISION` for a field error (E31) | COPAUA0C-012 |
//! | [`store_failure_reply`] | none: the critical read rows end the task before its reply (`X-COPAUA0C-1`) | COPAUA0C-122 to COPAUA0C-126 |
//! | [`decide`], [`Decision`] | `5000-PROCESS-AUTH`, `6000-MAKE-DECISION`, the i10 guard (E30) | COPAUA0C-020 to COPAUA0C-024, COPAUA0C-040 to COPAUA0C-049 |
//! | [`reply_body`] | the `STRING` into `W02-PUT-BUFFER` | COPAUA0C-060 to COPAUA0C-065 |
//! | [`summary_template`], [`summary_update`] | `8400-UPDATE-SUMMARY` | COPAUA0C-025, COPAUA0C-090 to COPAUA0C-097 |
//! | [`detail_record`] | `8500-INSERT-AUTH` | COPAUA0C-098 to COPAUA0C-103 |
//! | [`LogSite`], [`error_log`] | `9500-LOG-ERROR` and its callers | COPAUA0C-110 to COPAUA0C-126 |
//!
//! # Order of use (E29)
//!
//! The worker keeps the COBOL's order, reply before writes (COPAUA0C-080, COPAUA0C-081):
//! [`parse_request`], then [`decide`] and [`reply_body`] before the publish; [`summary_update`]
//! and [`detail_record`] only after the request's acknowledgement, inside the transaction that
//! holds the summary row lock taken by the worker. A request whose card has no `card_xref` row,
//! a field error and a request whose store read failed write no authorization row (COPAUA0C-082,
//! COPAUA0C-103, COPAUA0C-122 to COPAUA0C-126).
//!
//! # One request is one task's first message
//!
//! Every function here builds its result from its arguments alone, so each request is processed
//! as the first message of a CICS task and two requests share nothing (E15, COPAUA0C-130). What
//! the COBOL carries from one message of a task to the next, the reply pointer and the decision
//! flags, is `HALTED(H1)` (COPAUA0C-131) and not modelled. The 501-message task limit has no
//! counterpart (E14).
//!
//! # Redaction
//!
//! The reply's card field and every card-bearing log key carry the redacted card number only;
//! the transaction-ID field and the approved amount pass the screens of COPAUA0C-062 and
//! COPAUA0C-064, so no requester-sent run of eight or more bytes that could hold a protected
//! value is echoed (Cardholder Data Redaction, E32, X-COPAUA0C-11, COPAUA0C-133). Errors name a
//! rule and a field, never a byte of the body.
//!
//! # Totality
//!
//! [`parse_request`] returns a request, a card-number error or a field error for every byte
//! sequence of any length and never panics (Total Parsers Over Untrusted Input, COPAUA0C-017).
//! It holds at most 18 field slices, whatever the body's length and comma count, so untrusted
//! input cannot grow its memory beyond the body itself. The module root denies `unwrap`,
//! `expect` and indexing here and in the unit tests.

use core::fmt;

use copybook::edited::EditedPicture;
use copybook::{Count, ErrorKind, PicS, PicU, PicX};

use crate::clock::{Clock, hhmmss, inverted_auth_keys, yymmdd};
use crate::money::{Decimal, Money, MoneyExpr};
use crate::records::error_log::card_event_key;
use crate::records::{
    AccountRecord, AuthReply, AuthRequestRecord, CardXrefRecord, ErrorLevel, ErrorLogRecord,
    ErrorSubsystem, PendingAuthDetailRecord, PendingAuthSummaryRecord,
};
use crate::sensitive::CardNumber;

/// `WS-PGM-AUTH PIC X(08) VALUE 'COPAUA0C'`, moved to `ERR-PROGRAM` (`COPAUA0C.cbl:33,997`;
/// COPAUA0C-110).
pub const PROGRAM: &str = "COPAUA0C";

/// `WS-CICS-TRANID PIC X(04) VALUE 'CP00'`, moved to `ERR-APPLICATION` (`COPAUA0C.cbl:34,996`;
/// COPAUA0C-110).
pub const TRANSACTION: &str = "CP00";

/// The `alert` field of the `ERROR` event, target `carddemo::alert`, that the worker emits for a
/// card-number error before it dead-letters the request (i11, E31; COPAUA0C-006).
pub const ALERT_CARD_UNPARSEABLE: &str = "CP00_CARD_UNPARSEABLE";

/// `W01-GET-BUFFER PIC X(500)`: a longer body is a field error (`COPAUA0C.cbl:103,398`;
/// COPAUA0C-010, X-COPAUA0C-5).
pub const GET_BUFFER_LEN: usize = 500;

/// The bytes the reply `STRING` writes: six fields and six commas (COPAUA0C-060). The COBOL sends
/// one byte more, which no source sets (COPAUA0C-065, `HALTED(H6)`).
pub const REPLY_WRITTEN_LEN: usize = 63;

/// The receivers of the `UNSTRING` (`COPAUA0C.cbl:356-373`; COPAUA0C-002).
const FIELD_COUNT: usize = 18;

/// The `UNSTRING` delimiter, and the separator the reply `STRING` writes after every field
/// (`COPAUA0C.cbl:355,722-727`; COPAUA0C-002, COPAUA0C-060).
const DELIMITER: u8 = b',';

/// The run the reply screens withhold at: eight consecutive bytes, the width of a password
/// filling `SEC-USR-PWD X(08)` (COPAUA0C-062, COPAUA0C-064, X-COPAUA0C-11). A shorter protected
/// value, such as a three-digit CVV or a password shorter than its field, passes unscreened.
const SCREEN_RUN: usize = 8;

/// The byte a screen writes over every position of a withheld field (COPAUA0C-062, COPAUA0C-064).
const WITHHELD: u8 = b'*';

/// `WS-APPROVED-AMT-DIS PIC -zzzzzzzzz9.99`, the amount the reply emits (`COPAUA0C.cbl:66,720`;
/// E03, ARITH-039).
const REPLY_AMOUNT: EditedPicture = EditedPicture::MinusZ10Dot99;

/// `WS-TRANSACTION-AMT-AN PIC X(13)`, the receiver of field 9 (`COPAUA0C.cbl:63`).
type AmountText = PicX<13>;

// The reply's field widths and its six commas add up to the 63 written bytes (COPAUA0C-060).
const _: () = assert!(
    PicX::<16>::WIDTH
        + PicX::<15>::WIDTH
        + PicX::<6>::WIDTH
        + PicX::<2>::WIDTH
        + PicX::<4>::WIDTH
        + REPLY_AMOUNT.width()
        + 6
        == REPLY_WRITTEN_LEN,
    "the CP00 reply writes 63 bytes"
);

const _: () = assert!(
    REPLY_AMOUNT.width() == 14,
    "WS-APPROVED-AMT-DIS is 14 bytes"
);

/// The 18 comma-delimited fields of a CP00 request, in wire order, each named by its `UNSTRING`
/// receiver (`COPAUA0C.cbl:356-373`; COPAUA0C-002, CCPAURQY-001 to CCPAURQY-018).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequestField {
    /// Field 1, `PA-RQ-AUTH-DATE X(06)`.
    AuthDate,
    /// Field 2, `PA-RQ-AUTH-TIME X(06)`.
    AuthTime,
    /// Field 3, `PA-RQ-CARD-NUM X(16)`.
    CardNum,
    /// Field 4, `PA-RQ-AUTH-TYPE X(04)`.
    AuthType,
    /// Field 5, `PA-RQ-CARD-EXPIRY-DATE X(04)`.
    CardExpiryDate,
    /// Field 6, `PA-RQ-MESSAGE-TYPE X(06)`.
    MessageType,
    /// Field 7, `PA-RQ-MESSAGE-SOURCE X(06)`.
    MessageSource,
    /// Field 8, `PA-RQ-PROCESSING-CODE 9(06)`.
    ProcessingCode,
    /// Field 9, `WS-TRANSACTION-AMT-AN X(13)`, the amount text.
    TransactionAmt,
    /// Field 10, `PA-RQ-MERCHANT-CATAGORY-CODE X(04)`.
    MerchantCatagoryCode,
    /// Field 11, `PA-RQ-ACQR-COUNTRY-CODE X(03)`.
    AcqrCountryCode,
    /// Field 12, `PA-RQ-POS-ENTRY-MODE 9(02)`.
    PosEntryMode,
    /// Field 13, `PA-RQ-MERCHANT-ID X(15)`.
    MerchantId,
    /// Field 14, `PA-RQ-MERCHANT-NAME X(22)`.
    MerchantName,
    /// Field 15, `PA-RQ-MERCHANT-CITY X(13)`.
    MerchantCity,
    /// Field 16, `PA-RQ-MERCHANT-STATE X(02)`.
    MerchantState,
    /// Field 17, `PA-RQ-MERCHANT-ZIP X(09)`.
    MerchantZip,
    /// Field 18, `PA-RQ-TRANSACTION-ID X(15)`.
    TransactionId,
}

/// The receivers in `UNSTRING` order: entry k − 1 receives field k.
const REQUEST_FIELDS: [RequestField; FIELD_COUNT] = [
    RequestField::AuthDate,
    RequestField::AuthTime,
    RequestField::CardNum,
    RequestField::AuthType,
    RequestField::CardExpiryDate,
    RequestField::MessageType,
    RequestField::MessageSource,
    RequestField::ProcessingCode,
    RequestField::TransactionAmt,
    RequestField::MerchantCatagoryCode,
    RequestField::AcqrCountryCode,
    RequestField::PosEntryMode,
    RequestField::MerchantId,
    RequestField::MerchantName,
    RequestField::MerchantCity,
    RequestField::MerchantState,
    RequestField::MerchantZip,
    RequestField::TransactionId,
];

impl RequestField {
    /// The COBOL name of the field's receiver, as errors report it (CCPAURQY-001 to
    /// CCPAURQY-018).
    #[must_use]
    pub const fn cobol_name(self) -> &'static str {
        match self {
            Self::AuthDate => "PA-RQ-AUTH-DATE",
            Self::AuthTime => "PA-RQ-AUTH-TIME",
            Self::CardNum => "PA-RQ-CARD-NUM",
            Self::AuthType => "PA-RQ-AUTH-TYPE",
            Self::CardExpiryDate => "PA-RQ-CARD-EXPIRY-DATE",
            Self::MessageType => "PA-RQ-MESSAGE-TYPE",
            Self::MessageSource => "PA-RQ-MESSAGE-SOURCE",
            Self::ProcessingCode => "PA-RQ-PROCESSING-CODE",
            Self::TransactionAmt => "WS-TRANSACTION-AMT-AN",
            Self::MerchantCatagoryCode => "PA-RQ-MERCHANT-CATAGORY-CODE",
            Self::AcqrCountryCode => "PA-RQ-ACQR-COUNTRY-CODE",
            Self::PosEntryMode => "PA-RQ-POS-ENTRY-MODE",
            Self::MerchantId => "PA-RQ-MERCHANT-ID",
            Self::MerchantName => "PA-RQ-MERCHANT-NAME",
            Self::MerchantCity => "PA-RQ-MERCHANT-CITY",
            Self::MerchantState => "PA-RQ-MERCHANT-STATE",
            Self::MerchantZip => "PA-RQ-MERCHANT-ZIP",
            Self::TransactionId => "PA-RQ-TRANSACTION-ID",
        }
    }
}

/// Why field 3 is not a card number (COPAUA0C-005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardNumberCause {
    /// The body has fewer than three fields, the empty body included.
    Absent,
    /// Field 3 is not exactly 16 bytes, each an ASCII digit `0`–`9`: empty, shorter, longer, or
    /// holding a space, sign, letter or any other byte.
    NotSixteenDigits,
}

/// A request whose card number cannot be read (COPAUA0C-005, COPAUA0C-006; i11, E31).
///
/// The worker sends no reply, because a reply would address a card number it may have misread.
/// It emits the [`ALERT_CARD_UNPARSEABLE`] event with the request's `message_id`,
/// `correlation_id` and [`body_len`](Self::body_len), and then rejects the request with
/// `requeue: false`, which dead-letters it to `CARD.DEMO.DEADLETTER.PAUTH`; no lookup, lock,
/// database write or log row follows. The error carries the body's length and the cause, never a
/// byte of the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CardNumberError {
    /// The length of the whole body in bytes.
    pub body_len: usize,
    /// Why field 3 is not a card number.
    pub cause: CardNumberCause,
}

impl fmt::Display for CardNumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.cause {
            CardNumberCause::Absent => "has no card-number field (field 3)",
            CardNumberCause::NotSixteenDigits => {
                "card number (field 3) is not exactly 16 ASCII digits"
            }
        };
        write!(f, "CP00 request {reason}; body of {} bytes", self.body_len)
    }
}

impl std::error::Error for CardNumberError {}

/// The malformed part of a request whose card number parses (E31). Each variant names the rule
/// that makes the COBOL result undefined; none carries a byte of the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldFault {
    /// The body is longer than [`GET_BUFFER_LEN`], which the COBOL get refuses (COPAUA0C-010,
    /// X-COPAUA0C-5).
    BodyTooLong {
        /// The length of the whole body in bytes.
        len: usize,
    },
    /// The body ends before this field, the first receiver the `UNSTRING` does not act upon, so
    /// the COBOL would keep stale or uninitialised storage there (COPAUA0C-007).
    Missing(RequestField),
    /// The field holds a byte outside printable ASCII, `0x20`–`0x7E`, whose COBOL value depends
    /// on character-set conversion no repository source fixes (COPAUA0C-011, X-COPAUA0C-9).
    NonPrintable(RequestField),
    /// A `PIC 9(n)` field holds a byte other than an ASCII digit (COPAUA0C-009).
    NonDigit(RequestField),
    /// The 13 bytes of `WS-TRANSACTION-AMT-AN` are not a valid `FUNCTION NUMVAL` argument
    /// (COPAUA0C-008).
    AmountNotNumeric,
}

impl fmt::Display for FieldFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLong { len } => write!(
                f,
                "body of {len} bytes exceeds the {GET_BUFFER_LEN}-byte W01-GET-BUFFER"
            ),
            Self::Missing(field) => write!(f, "body ends before {}", field.cobol_name()),
            Self::NonPrintable(field) => write!(
                f,
                "{} holds a byte outside printable ASCII",
                field.cobol_name()
            ),
            Self::NonDigit(field) => write!(
                f,
                "{} holds a byte other than an ASCII digit",
                field.cobol_name()
            ),
            Self::AmountNotNumeric => {
                f.write_str("WS-TRANSACTION-AMT-AN is not a valid NUMVAL argument")
            }
        }
    }
}

/// A request with a parseable card number and a malformed field (COPAUA0C-007 to COPAUA0C-011;
/// i11, E31).
///
/// The worker publishes [`field_error_reply`], a `05`/`9000` decline, with no lock and no write,
/// and acknowledges the request after the reply's confirmation (COPAUA0C-012, COPAUA0C-082). The
/// fields below fix every byte of that reply (X-COPAUA0C-6). `Debug` and `Display` show the
/// redacted card and the fault only, never the two echoed fields, which hold whatever the
/// requester sent.
#[derive(Clone, PartialEq, Eq)]
pub struct FieldError {
    /// The parsed card number of field 3 (COPAUA0C-005).
    pub card: CardNumber,
    /// The first malformed part found, in the order: body length, field count, then each field
    /// in wire order.
    pub fault: FieldFault,
    /// Field 18 moved into `PA-RQ-TRANSACTION-ID X(15)` when the body has 18 or more fields, 15
    /// spaces otherwise (COPAUA0C-012).
    pub transaction_id: PicX<15>,
    /// Field 2 moved into `PA-RQ-AUTH-TIME X(06)`, present whenever the card parses
    /// (COPAUA0C-012).
    pub auth_id_code: PicX<6>,
}

impl fmt::Debug for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldError")
            .field("card", &self.card)
            .field("fault", &self.fault)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CP00 request field error: {}", self.fault)
    }
}

impl std::error::Error for FieldError {}

/// Why a CP00 request body is not a request (COPAUA0C-017).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Field 3 is not a card number: no reply, alert, dead-letter (COPAUA0C-006).
    CardNumber(CardNumberError),
    /// The card number parses and another part is malformed: a `05`/`9000` decline with no
    /// write (COPAUA0C-012).
    Field(FieldError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CardNumber(error) => error.fmt(f),
            Self::Field(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CardNumber(error) => Some(error),
            Self::Field(error) => Some(error),
        }
    }
}

/// Parses a CP00 request body as `2100-EXTRACT-REQUEST-MSG` reads it: `UNSTRING … DELIMITED BY
/// ','` into the 18 receivers, then `COMPUTE PA-RQ-TRANSACTION-AMT = FUNCTION NUMVAL(…)`
/// (`COPAUA0C.cbl:354-379`; COPAUA0C-002 to COPAUA0C-017).
///
/// - The body is cut at every `,`; a final comma ends the last field and opens none, and fields
///   beyond the 18th are ignored and never held (COPAUA0C-002).
/// - Field 3 must be exactly 16 ASCII digits, and is checked before every other rule
///   (COPAUA0C-005, COPAUA0C-017).
/// - Alphanumeric fields are moved left-justified, space-padded and cut on the right; fields 8
///   and 12 keep their rightmost 6 and 2 digits, an empty one being zeros (COPAUA0C-003,
///   COPAUA0C-004).
/// - Field 9 is cut or padded to 13 bytes, parsed exactly as `NUMVAL` reads it and stored into
///   `PIC +9(10).99` by truncation (COPAUA0C-008, COPAUA0C-014).
///
/// The function is total: any byte sequence of any length gives exactly one outcome, and nothing
/// indexes, slices or unwraps (Total Parsers Over Untrusted Input). Its memory beside the body is
/// bounded: the split allocates nothing and holds at most 18 field slices, independent of the
/// body's length and comma count, so a body of any size reaches its card-number error or its
/// over-length field error (COPAUA0C-010) at that fixed cost.
///
/// # Errors
///
/// [`ParseError::CardNumber`] when field 3 is absent or not exactly 16 ASCII digits
/// (COPAUA0C-005); otherwise [`ParseError::Field`] for a body over [`GET_BUFFER_LEN`] bytes, fewer
/// than 18 fields, a byte outside printable ASCII in fields 1, 2 or 4 to 18, a non-digit in
/// field 8 or 12, or an amount that is not a valid `NUMVAL` argument (COPAUA0C-007 to
/// COPAUA0C-011).
pub fn parse_request(body: &[u8]) -> Result<AuthRequestRecord, ParseError> {
    let split = split_fields(body);
    let fields = split.present();
    let card = parse_card(fields, body.len()).map_err(ParseError::CardNumber)?;
    move_fields(card.clone(), body.len(), fields).map_err(|fault| {
        ParseError::Field(FieldError {
            card,
            fault,
            transaction_id: fields
                .get(FIELD_COUNT - 1)
                .map_or_else(PicX::spaces, |field| PicX::moved_from(field)),
            auth_id_code: PicX::moved_from(fields.get(1).copied().unwrap_or_default()),
        })
    })
}

/// The fields of a request body that reach the 18 `UNSTRING` receivers (COPAUA0C-002).
struct Fields<'a> {
    /// Field k + 1 at entry k; the entries from `count` on are empty and never read.
    received: [&'a [u8]; FIELD_COUNT],
    /// The number of fields present, n, capped at the 18 receivers.
    count: usize,
}

impl<'a> Fields<'a> {
    /// The fields present, in wire order: at most 18.
    fn present(&self) -> &[&'a [u8]] {
        // `count` never exceeds the 18 entries, so `get` always finds them; the empty default
        // only keeps this free of a panic path.
        self.received.get(..self.count).unwrap_or_default()
    }
}

/// The fields the `UNSTRING` delimits: the bytes between commas, the number present being the
/// commas plus one, less one when the body ends with a comma; fields after the 18th are ignored
/// (COPAUA0C-002).
///
/// The split reads at most 19 pieces and allocates nothing: it holds at most 18 field slices,
/// independent of the body's length and comma count, and reads the 19th piece only to learn
/// whether the body goes on past the 18th. A body of millions of commas therefore costs the same
/// fixed memory as an 18-field body before its over-length field error (COPAUA0C-010).
fn split_fields(body: &[u8]) -> Fields<'_> {
    let mut pieces = body.split(|&byte| byte == DELIMITER);
    let mut received: [&[u8]; FIELD_COUNT] = [&[]; FIELD_COUNT];
    let mut count = 0_usize;
    for (slot, piece) in received.iter_mut().zip(pieces.by_ref().take(FIELD_COUNT)) {
        *slot = piece;
        count += 1;
    }
    if pieces.next().is_none() && body.last() == Some(&DELIMITER) {
        // The transfer stops once every sending character is moved, so the empty piece after a
        // final comma, the last piece held, is no field. A piece past the 18th means 18 fields
        // or more whatever the last byte is, and the count stays at the 18 receivers.
        count = count.saturating_sub(1);
    }
    Fields { received, count }
}

/// Field 3 as a card number: exactly 16 ASCII digits (COPAUA0C-005).
fn parse_card(fields: &[&[u8]], body_len: usize) -> Result<CardNumber, CardNumberError> {
    let error = |cause| CardNumberError { body_len, cause };
    let field = fields
        .get(2)
        .ok_or_else(|| error(CardNumberCause::Absent))?;
    core::str::from_utf8(field)
        .ok()
        .and_then(|text| CardNumber::parse(text).ok())
        .ok_or_else(|| error(CardNumberCause::NotSixteenDigits))
}

/// Whether `byte` is printable ASCII, `0x20`–`0x7E` (COPAUA0C-011).
const fn is_printable(byte: u8) -> bool {
    matches!(byte, b' '..=b'~')
}

/// Moves the 18 fields into their receivers once the card number has parsed (COPAUA0C-003,
/// COPAUA0C-004, COPAUA0C-007 to COPAUA0C-011, COPAUA0C-013, COPAUA0C-014).
fn move_fields(
    card_num: CardNumber,
    body_len: usize,
    fields: &[&[u8]],
) -> Result<AuthRequestRecord, FieldFault> {
    if body_len > GET_BUFFER_LEN {
        return Err(FieldFault::BodyTooLong { len: body_len });
    }
    // Entry n of the receivers exists exactly when only n fields are present, n < 18.
    if let Some(&missing) = REQUEST_FIELDS.get(fields.len()) {
        return Err(FieldFault::Missing(missing));
    }
    for (&field, bytes) in REQUEST_FIELDS.iter().zip(fields) {
        if field != RequestField::CardNum && !bytes.iter().all(|&byte| is_printable(byte)) {
            return Err(FieldFault::NonPrintable(field));
        }
    }
    let received: [&[u8]; FIELD_COUNT] =
        core::array::from_fn(|k| fields.get(k).copied().unwrap_or_default());
    let [
        auth_date,
        auth_time,
        _,
        auth_type,
        card_expiry_date,
        message_type,
        message_source,
        processing_code,
        amount_text,
        merchant_catagory_code,
        acqr_country_code,
        pos_entry_mode,
        merchant_id,
        merchant_name,
        merchant_city,
        merchant_state,
        merchant_zip,
        transaction_id,
    ] = received;
    // Struct fields evaluate in the order written, so the numeric checks run in wire order.
    Ok(AuthRequestRecord {
        auth_date: PicX::moved_from(auth_date),
        auth_time: PicX::moved_from(auth_time),
        card_num,
        auth_type: PicX::moved_from(auth_type),
        card_expiry_date: PicX::moved_from(card_expiry_date),
        message_type: PicX::moved_from(message_type),
        message_source: PicX::moved_from(message_source),
        processing_code: numeric_field(processing_code, RequestField::ProcessingCode)?,
        transaction_amt: transaction_amount(amount_text)?,
        merchant_catagory_code: PicX::moved_from(merchant_catagory_code),
        acqr_country_code: PicX::moved_from(acqr_country_code),
        pos_entry_mode: numeric_field(pos_entry_mode, RequestField::PosEntryMode)?,
        merchant_id: PicX::moved_from(merchant_id),
        merchant_name: PicX::moved_from(merchant_name),
        merchant_city: PicX::moved_from(merchant_city),
        merchant_state: PicX::moved_from(merchant_state),
        merchant_zip: PicX::moved_from(merchant_zip),
        transaction_id: PicX::moved_from(transaction_id),
    })
}

/// Field 8 or 12 moved into its `PIC 9(N)` receiver as an unsigned integer: right-aligned,
/// zero-padded, the digits beyond `N` dropped from the left; an empty field is zeros
/// (COPAUA0C-004).
fn numeric_field<const N: u32>(bytes: &[u8], field: RequestField) -> Result<PicU<N>, FieldFault> {
    let modulus = PicU::<N>::MAX + 1;
    let mut value = 0_u64;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return Err(FieldFault::NonDigit(field));
        }
        // TRUNCATION: a numeric MOVE keeps the receiver's N low-order digits, so each step
        // keeps the value modulo 10^N; below 10^N before the step, it stays below 10^(N+1) ≤
        // 10^19 during it, inside `u64`.
        value = (value * 10 + u64::from(byte - b'0')) % modulus;
    }
    // The value is below 10^N by construction, so `new` accepts it; its error arm only keeps
    // this function free of a panic path.
    PicU::new(value).map_err(|_| FieldFault::NonDigit(field))
}

/// Field 9 into `WS-TRANSACTION-AMT-AN PIC X(13)`, then `COMPUTE PA-RQ-TRANSACTION-AMT =
/// FUNCTION NUMVAL(WS-TRANSACTION-AMT-AN)` into `PIC +9(10).99` (COPAUA0C-008, COPAUA0C-014 to
/// COPAUA0C-016).
fn transaction_amount(field: &[u8]) -> Result<Money<10>, FieldFault> {
    // TRUNCATION: the alphanumeric MOVE keeps the first 13 bytes of a longer field (X-ARITH-2),
    // so `+0000000123.45` is read as `+0000000123.4`.
    let text = AmountText::moved_from(field);
    let value = numval(text.as_bytes()).ok_or(FieldFault::AmountNotNumeric)?;
    // TRUNCATION: the COMPUTE has no ROUNDED and no ON SIZE ERROR, so the value is truncated
    // toward zero at 0.01 and its integer digits beyond ten are dropped (COPAUA0C-014). For text
    // with more than two decimal places the COBOL value is NUMVAL's floating-point
    // approximation, HALTED(X-ARITH-5) (COPAUA0C-015): this truncation is interim handling, not
    // a parity result. A zero written with a minus sign stores positive zero, HALTED(X-COPAUA0C-H3)
    // (COPAUA0C-016), interim as well.
    Ok(MoneyExpr::from_decimal(value).store::<10>())
}

/// The sign `NUMVAL` reads before or after the digits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sign {
    Plus,
    Minus,
}

/// `text` without its leading spaces.
fn skip_spaces(mut text: &[u8]) -> &[u8] {
    while let [b' ', rest @ ..] = text {
        text = rest;
    }
    text
}

/// The `+` or `-` that starts `text`, if any, and the bytes after it: the signs `NUMVAL` accepts
/// before the digits.
fn leading_sign(text: &[u8]) -> (Option<Sign>, &[u8]) {
    match text {
        [b'+', rest @ ..] => (Some(Sign::Plus), rest),
        [b'-', rest @ ..] => (Some(Sign::Minus), rest),
        _ => (None, text),
    }
}

/// The sign that starts `text` after the digits, if any, and the bytes after it: `+`, `-`, or
/// the negative `CR` and `DB`, upper case only.
fn trailing_sign(text: &[u8]) -> (Option<Sign>, &[u8]) {
    match text {
        [b'C', b'R', rest @ ..] | [b'D', b'B', rest @ ..] => (Some(Sign::Minus), rest),
        _ => leading_sign(text),
    }
}

/// The unsigned digit string `NUMVAL` reads: digits with at most one `.`, or a `.` followed by
/// digits, and at least one digit. Returns the digits as an integer mantissa, the count of digits
/// after the point, and the bytes after the string; `None` without a digit or on overflow.
fn digit_string(mut text: &[u8]) -> Option<(u64, u32, &[u8])> {
    let mut mantissa = 0_u64;
    let mut digits = 0_u32;
    let mut scale = 0_u32;
    let mut past_point = false;
    loop {
        match text {
            [digit @ b'0'..=b'9', rest @ ..] => {
                mantissa = mantissa
                    .checked_mul(10)?
                    .checked_add(u64::from(*digit - b'0'))?;
                digits = digits.saturating_add(1);
                if past_point {
                    scale = scale.saturating_add(1);
                }
                text = rest;
            }
            [b'.', rest @ ..] if !past_point => {
                past_point = true;
                text = rest;
            }
            _ => break,
        }
    }
    (digits > 0).then_some((mantissa, scale, text))
}

/// The value of `FUNCTION NUMVAL(text)` for a valid argument, computed exactly, or `None` when
/// the argument does not conform (COPAUA0C-008, COPAUA0C-014).
///
/// Format 1 is optional spaces, an optional `+` or `-`, optional spaces, the digit string and
/// optional spaces; format 2 is optional spaces, the digit string, optional spaces, one of `+`,
/// `-`, `CR` or `DB`, and optional spaces. A sign on both sides, a space inside the digit string,
/// a second point, an all-space argument and any other byte do not conform. The value is the
/// exact decimal of the text, which `NUMVAL`'s floating-point result rounds back to for at most
/// two decimal places (X-ARITH-1); no floating point is used (R1).
///
/// Outside the unit tests `text` is the 13 bytes of `WS-TRANSACTION-AMT-AN PIC X(13)`
/// (COPAUA0C-008), so the digit string holds at most 13 digits.
fn numval(text: &[u8]) -> Option<Decimal> {
    let (leading, rest) = leading_sign(skip_spaces(text));
    // Format 1 admits spaces between the sign and the digits; without a sign the leading spaces
    // are already skipped and this changes nothing.
    let (mantissa, scale, rest) = digit_string(skip_spaces(rest))?;
    let (trailing, rest) = trailing_sign(skip_spaces(rest));
    if !skip_spaces(rest).is_empty() {
        return None;
    }
    let negative = match (leading, trailing) {
        (Some(_), Some(_)) => return None,
        (Some(sign), None) | (None, Some(sign)) => sign == Sign::Minus,
        (None, None) => false,
    };
    // TRUNCATION: the checked u64 → i64 narrowing cannot lose a value here. Outside the unit
    // tests the only caller, `transaction_amount`, passes the 13 bytes of
    // `WS-TRANSACTION-AMT-AN PIC X(13)`, so the digit string holds at most 13 digits and the
    // mantissa is below 10^13, far inside `i64`. Its `None` arm stays the typed failure,
    // `FieldFault::AmountNotNumeric`, never a panic.
    let magnitude = i64::try_from(mantissa).ok()?;
    let signed = if negative { -magnitude } else { magnitude };
    Decimal::try_new(signed, scale).ok()
}

/// The reply to a field error: response `05`, reason `9000` (COPAUA0C's `WHEN OTHER` code,
/// `COPAUA0C.cbl:715-716`) and approved amount 0.00, with the card, transaction ID and auth-ID
/// code of the [`FieldError`] (COPAUA0C-012, X-COPAUA0C-6; E31).
///
/// The worker publishes it through [`reply_body`], which screens the transaction ID
/// (COPAUA0C-062), with no lock and no database write, and acknowledges the request after the
/// confirmation (COPAUA0C-082).
#[must_use]
pub fn field_error_reply(error: &FieldError) -> AuthReply {
    AuthReply {
        card_num: error.card.clone(),
        transaction_id: error.transaction_id.clone(),
        auth_id_code: error.auth_id_code.clone(),
        auth_resp_code: AuthStatus::Declined.response_code(),
        auth_resp_reason: ReasonCode::Other9000.pic(),
        approved_amt: Money::ZERO,
    }
}

/// The reply to a parsed `request` whose store read failed, the `C001`, `C002`, `C003`, `I001`
/// or `I002` case: response `05`, reason `9000` (COPAUA0C's `WHEN OTHER` code,
/// `COPAUA0C.cbl:715-716`) and approved amount 0.00, with the request's card, field 18 as the
/// transaction ID and field 2 as the auth-ID code, as [`decide`] maps them (COPAUA0C-122 to
/// COPAUA0C-126).
///
/// Interim handling, not a parity result: the COBOL's critical row ends the task before its
/// reply, so it sends none. This is the second outcome of the open escalation candidate
/// `X-COPAUA0C-1`, which keeps i11's one reply per removed request until the reviewer and the
/// user decide it. The worker publishes it through [`reply_body`], which screens the transaction
/// ID (COPAUA0C-062), after the critical row and the rollback, with no authorization write, and
/// acknowledges the request after the confirmation.
#[must_use]
pub fn store_failure_reply(request: &AuthRequestRecord) -> AuthReply {
    AuthReply {
        card_num: request.card_num.clone(),
        transaction_id: request.transaction_id.clone(),
        auth_id_code: request.auth_time.clone(),
        auth_resp_code: AuthStatus::Declined.response_code(),
        auth_resp_reason: ReasonCode::Other9000.pic(),
        approved_amt: Money::ZERO,
    }
}

/// Whether the customer read of `5300-READ-CUST-RECORD` found the cross-reference's customer
/// (COPAUA0C-023). Only the result matters: it feeds the decline reason (COPAUA0C-045).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CustomerLookup {
    /// `FOUND-CUST-IN-MSTR`.
    Found,
    /// `NFOUND-CUST-IN-MSTR`; the worker writes the `A003` row (COPAUA0C-116).
    NotFound,
}

/// The summary row `lock_or_create` locked for the account, and whether this request created it
/// (COPAUA0C-025).
///
/// Both variants carry the row as the worker's transaction sees it under its `FOR UPDATE` lock,
/// so the i10 guard reads the authoritative `approved_auth_amt`, which no other transaction can
/// change before this one commits or rolls back (COPAUA0C-049). `Existing` holds the row as last
/// committed. `Created` holds the zeroed [`summary_template`] row inserted by this request's
/// still-open transaction: it is provisional, made durable only by the commit that follows the
/// reply and the acknowledgement, and discarded by a rollback (E29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryState<'a> {
    /// The row existed before this request: COBOL `FOUND-PAUT-SMRY-SEG`.
    Existing(&'a PendingAuthSummaryRecord),
    /// The `INSERT … ON CONFLICT DO NOTHING` of this request created the row from
    /// [`summary_template`]: COBOL `NFOUND-PAUT-SMRY-SEG`.
    Created(&'a PendingAuthSummaryRecord),
}

impl<'a> SummaryState<'a> {
    /// The locked row, whichever path created it (COPAUA0C-025, COPAUA0C-049).
    #[must_use]
    pub const fn row(self) -> &'a PendingAuthSummaryRecord {
        match self {
            Self::Existing(row) | Self::Created(row) => row,
        }
    }
}

/// What the lookups of `5000-PROCESS-AUTH` found for a card found in the cross-reference
/// (COPAUA0C-020, COPAUA0C-022 to COPAUA0C-025).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoundContext<'a> {
    /// The `card_xref` row of the request's card (COPAUA0C-020).
    pub xref: &'a CardXrefRecord,
    /// The account of `xref.acct_id`, read in the worker's transaction after the summary lock;
    /// `None` is `NFOUND-ACCT-IN-MSTR`, after the `A002` row (COPAUA0C-022).
    pub account: Option<&'a AccountRecord>,
    /// The result of the customer read (COPAUA0C-023).
    pub customer: CustomerLookup,
    /// The locked summary row (COPAUA0C-025).
    pub summary: SummaryState<'a>,
}

/// The result of the cross-reference read of `5100-READ-XREF-RECORD` and what followed it
/// (COPAUA0C-020, COPAUA0C-021).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// `CARD-NFOUND-XREF`: no `card_xref` row; the worker writes the `A001` row and takes no
    /// lock (COPAUA0C-021).
    CardNotInXref,
    /// `CARD-FOUND-XREF`, with the account, customer and summary reads.
    CardFound(FoundContext<'a>),
}

/// `WS-AUTH-RESP-FLG`: the decision's outcome (COPAUA0C-044).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthStatus {
    /// `AUTH-RESP-APPROVED`, response code `00`.
    Approved,
    /// `AUTH-RESP-DECLINED`, response code `05`.
    Declined,
}

impl AuthStatus {
    /// `PA-RL-AUTH-RESP-CODE` for this outcome: [`AuthReply::APPROVED`] or
    /// [`AuthReply::DECLINED`] (`COPAUA0C.cbl:688,693`; COPAUA0C-044, COPAUA0C-063).
    #[must_use]
    pub const fn response_code(self) -> PicX<2> {
        match self {
            Self::Approved => PicX::from_array(*AuthReply::APPROVED),
            Self::Declined => PicX::from_array(*AuthReply::DECLINED),
        }
    }
}

/// `PA-RL-AUTH-RESP-REASON`, the reason codes a reply can carry (COPAUA0C-045, COPAUA0C-063).
///
/// `4200`, `4300`, `5100` and `5200` have no variant: their flags are set only by the profile
/// read, and `5600-READ-PROFILE-DATA` is a stub that does nothing, so no decision reaches them
/// (`COPAUA0C.cbl:647-651,707-714`; E13, COPAUA0C-024).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReasonCode {
    /// `0000`, every approval (`COPAUA0C.cbl:698`).
    Approved0000,
    /// `3100`: the card, the account or the customer was not found (`COPAUA0C.cbl:701-704`).
    NotFound3100,
    /// `4100`: `INSUFFICIENT-FUND`, and every decline of the i10 guard (`COPAUA0C.cbl:705-706`;
    /// COPAUA0C-046).
    InsufficientFunds4100,
    /// `9000`: `WHEN OTHER`, which a field error carries (`COPAUA0C.cbl:715-716`;
    /// COPAUA0C-012), and so does the interim reply to a failed store read,
    /// [`store_failure_reply`] (COPAUA0C-122 to COPAUA0C-126, `X-COPAUA0C-1`).
    Other9000,
}

impl ReasonCode {
    /// The four bytes `MOVE`d into `PA-RL-AUTH-RESP-REASON`.
    #[must_use]
    pub const fn code(self) -> &'static [u8; 4] {
        match self {
            Self::Approved0000 => b"0000",
            Self::NotFound3100 => b"3100",
            Self::InsufficientFunds4100 => b"4100",
            Self::Other9000 => b"9000",
        }
    }

    /// The code as the `PIC X(04)` field of the reply and the detail.
    #[must_use]
    pub const fn pic(self) -> PicX<4> {
        PicX::from_array(*self.code())
    }
}

/// What the i10 credit-limit guard did to a decision (E30; COPAUA0C-046 to COPAUA0C-048).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GuardOutcome {
    /// The guard did not run: the card has no cross-reference row, or COPAUA0C's own rule
    /// declined.
    NotApplied,
    /// COPAUA0C's rule approved, `acct_curr_bal + approved_auth_amt + amount ≤
    /// acct_credit_limit`, and `acct_curr_bal` plus the approved total as the summary update
    /// stores it into `S9(09)V99` is within `acct_credit_limit` too, so the approval stands
    /// (COPAUA0C-046, COPAUA0C-094, ARITH-043).
    Passed,
    /// COPAUA0C's rule approved but the approval would breach i10, by the exact sum or by the
    /// stored total, so the request is declined `05`/`4100` (COPAUA0C-046, COPAUA0C-047,
    /// COPAUA0C-094, ARITH-043).
    DeclinedByCreditLimit,
    /// COPAUA0C's rule approved on an existing summary while the account was not found, so the
    /// guard has no balance or limit to bound the approval by and the request is declined
    /// `05`/`3100` (COPAUA0C-048, X-COPAUA0C-3).
    DeclinedWithoutAccount,
}

/// One CP00 decision: the outcome, its reason, the guard's part and the reply fields
/// (`6000-MAKE-DECISION`, `COPAUA0C.cbl:657-718`; COPAUA0C-040 to COPAUA0C-048).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Approved or declined, after the guard.
    pub status: AuthStatus,
    /// The reason the reply carries.
    pub reason: ReasonCode,
    /// What the i10 guard did.
    pub guard: GuardOutcome,
    /// `PENDING-AUTH-RESPONSE` as the decision fills it (COPAUA0C-044, COPAUA0C-062,
    /// COPAUA0C-063); [`reply_body`] lays it out.
    pub reply: AuthReply,
    /// `WS-TRANSACTION-AMT`, the request amount the decision compared (COPAUA0C-014), which
    /// [`summary_update`] needs on the declined path (COPAUA0C-096).
    pub transaction_amt: Money<10>,
}

/// `WS-DECLINE-REASON-FLG` as COPAUA0C sets it on a task's first message: only
/// `INSUFFICIENT-FUND` is ever set (COPAUA0C-045).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclineFlag {
    Unset,
    InsufficientFund,
}

/// The outcome of COPAUA0C's own rule, before the guard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CobolOutcome {
    Approve,
    Decline(DeclineFlag),
}

/// An outcome and its reason.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Approved,
    Declined(ReasonCode),
}

/// Decides a request as `5000-PROCESS-AUTH` and `6000-MAKE-DECISION` do, with the i10
/// credit-limit guard (`COPAUA0C.cbl:438-466,657-718`; E30).
///
/// COPAUA0C's own rule, kept unchanged (COPAUA0C-041 to COPAUA0C-045, ARITH-034 to ARITH-037):
///
/// - With a summary that existed before the request, the available amount is
///   `(PA-CREDIT-LIMIT − PA-CREDIT-BALANCE)` stored into `WS-AVAILABLE-AMT S9(09)V99` (E12);
///   with a summary this request created and the account found, it is `(ACCT-CREDIT-LIMIT −
///   ACCT-CURR-BAL)` stored likewise; with neither, the request is declined.
/// - An amount strictly greater than the available amount is declined with `INSUFFICIENT-FUND`.
/// - A decline's reason is `3100` when the card, the account or the customer was not found,
///   else `4100` for insufficient funds, else `9000`. A customer not found never declines by
///   itself.
///
/// The guard runs only where COPAUA0C's rule approves a card found in the cross-reference
/// (COPAUA0C-046 to COPAUA0C-048, ARITH-045): the approval stands only when
/// `acct_curr_bal + approved_auth_amt + amount ≤ acct_credit_limit`, over the locked summary
/// row, and `acct_curr_bal` plus the approved total as [`summary_update`] stores it into
/// `S9(09)V99` is within `acct_credit_limit` too (COPAUA0C-094, ARITH-043); otherwise it is
/// declined `05`/`4100`. Without an account row it is declined `05`/`3100`. Every COBOL decline
/// is kept.
///
/// The reply takes the request's card, field 18 as the transaction ID and field 2 as the
/// auth-ID code; response `00` with the request amount when approved, `05` with 0.00 when
/// declined (COPAUA0C-044, COPAUA0C-062, COPAUA0C-063).
///
/// A card with no cross-reference row is declined `05`/`3100`. That is interim handling, not a
/// parity result: the COBOL then tests a summary flag no read set, `HALTED(X-HALTS-1)`
/// (COPAUA0C-040). Each request is decided as a task's first message; decisions after it are
/// `HALTED(H1)` (COPAUA0C-130, COPAUA0C-131).
#[must_use]
pub fn decide(request: &AuthRequestRecord, lookup: &Lookup<'_>) -> Decision {
    let amount = request.transaction_amt;
    let (verdict, guard) = match lookup {
        // HALTED(X-HALTS-1), COPAUA0C-040: interim `05`/`3100`, not a parity result.
        Lookup::CardNotInXref => (
            Verdict::Declined(ReasonCode::NotFound3100),
            GuardOutcome::NotApplied,
        ),
        Lookup::CardFound(context) => found_card_verdict(amount, context),
    };
    let (status, reason, approved_amt) = match verdict {
        Verdict::Approved => (AuthStatus::Approved, ReasonCode::Approved0000, amount),
        Verdict::Declined(reason) => (AuthStatus::Declined, reason, Money::ZERO),
    };
    Decision {
        status,
        reason,
        guard,
        reply: AuthReply {
            card_num: request.card_num.clone(),
            transaction_id: request.transaction_id.clone(),
            auth_id_code: request.auth_time.clone(),
            auth_resp_code: status.response_code(),
            auth_resp_reason: reason.pic(),
            approved_amt,
        },
        transaction_amt: amount,
    }
}

/// COPAUA0C's rule, then the guard, for a card found in the cross-reference.
fn found_card_verdict(amount: Money<10>, context: &FoundContext<'_>) -> (Verdict, GuardOutcome) {
    match cobol_outcome(amount, context) {
        CobolOutcome::Decline(flag) => (
            Verdict::Declined(decline_reason(lookup_failed(context), flag)),
            GuardOutcome::NotApplied,
        ),
        CobolOutcome::Approve => credit_limit_guard(amount, context),
    }
}

/// The available-amount test of `6000-MAKE-DECISION` (COPAUA0C-041 to COPAUA0C-043, ARITH-034
/// to ARITH-037).
fn cobol_outcome(amount: Money<10>, context: &FoundContext<'_>) -> CobolOutcome {
    // TRUNCATION: `WS-AVAILABLE-AMT PIC S9(09)V99 COMP-3` is narrower than the limits and
    // balances it is computed from, so the COMPUTE drops their high-order digit (E12).
    let available = match context.summary {
        SummaryState::Existing(row) => Some((row.credit_limit - row.credit_balance).store::<9>()),
        SummaryState::Created(_) => context
            .account
            .map(|account| (account.credit_limit - account.curr_bal).store::<9>()),
    };
    match available {
        Some(available) if amount > available => {
            CobolOutcome::Decline(DeclineFlag::InsufficientFund)
        }
        Some(_) => CobolOutcome::Approve,
        None => CobolOutcome::Decline(DeclineFlag::Unset),
    }
}

/// Whether `NFOUND-ACCT-IN-MSTR` or `NFOUND-CUST-IN-MSTR` holds for a found card
/// (COPAUA0C-045).
fn lookup_failed(context: &FoundContext<'_>) -> bool {
    context.account.is_none() || context.customer == CustomerLookup::NotFound
}

/// The `EVALUATE TRUE` of `COPAUA0C.cbl:700-717`, first match wins (COPAUA0C-045): a lookup that
/// failed gives `3100`, then `INSUFFICIENT-FUND` `4100`, otherwise `9000`. On a task's first
/// message every decline of a found card either fails a lookup or sets `INSUFFICIENT-FUND`, so
/// COPAUA0C's own rule never reaches `9000`.
fn decline_reason(not_found: bool, flag: DeclineFlag) -> ReasonCode {
    if not_found {
        ReasonCode::NotFound3100
    } else if flag == DeclineFlag::InsufficientFund {
        ReasonCode::InsufficientFunds4100
    } else {
        ReasonCode::Other9000
    }
}

/// The i10 guard over an approval of COPAUA0C's rule (E30; COPAUA0C-046 to COPAUA0C-048,
/// ARITH-045).
///
/// The approval stands only when the exact `acct_curr_bal + approved_auth_amt + amount` is
/// within `acct_credit_limit` and, that holding, `acct_curr_bal` plus the approved total as
/// [`summary_update`] will store it into `S9(09)V99` is within it too (COPAUA0C-094,
/// ARITH-043). i10 binds the stored total, and a sum below −999999999.99 loses its high-order
/// digits in that store, which raises the stored total above the exact sum. Otherwise the
/// request is declined `05`/`4100`.
fn credit_limit_guard(amount: Money<10>, context: &FoundContext<'_>) -> (Verdict, GuardOutcome) {
    let Some(account) = context.account else {
        // An approval on an existing summary with no account row: the guard has nothing to
        // bound it by (COPAUA0C-048, X-COPAUA0C-3).
        return (
            Verdict::Declined(ReasonCode::NotFound3100),
            GuardOutcome::DeclinedWithoutAccount,
        );
    };
    let pending = context.summary.row().approved_auth_amt;
    // Three operands below 10^10 in magnitude sum below 10^12, so storing the exact sum into 12
    // integer digits drops nothing at scale 2: the comparison is the exact one ARITH-045 states.
    let committed = (account.curr_bal + pending + amount).store::<12>();
    if committed <= account.credit_limit {
        // The balance below 10^10 and the stored total below 10^9 in magnitude sum below 10^12,
        // so this store drops nothing either: it is the committed state i10 binds.
        let stored = (account.curr_bal + approved_total(pending, amount)).store::<12>();
        if stored <= account.credit_limit {
            return (Verdict::Approved, GuardOutcome::Passed);
        }
    }
    // Every guard decline carries `4100`, a customer not found included (COPAUA0C-047,
    // X-COPAUA0C-4).
    (
        Verdict::Declined(ReasonCode::InsufficientFunds4100),
        GuardOutcome::DeclinedByCreditLimit,
    )
}

/// `PA-APPROVED-AUTH-AMT` after `ADD WS-APPROVED-AMT TO PA-APPROVED-AUTH-AMT` adds `approved`
/// (`COPAUA0C.cbl:815`; COPAUA0C-094, ARITH-043): the total [`summary_update`] writes and the
/// i10 guard bounds (COPAUA0C-046), computed in one place so the two cannot differ.
fn approved_total(approved_auth_amt: Money<9>, approved: Money<10>) -> Money<9> {
    // TRUNCATION: the ADD stores into `PA-APPROVED-AUTH-AMT PIC S9(09)V99 COMP-3`, dropping the
    // integer digits of the sum above the ninth and keeping its sign (ARITH-043).
    (approved_auth_amt + approved).store::<9>()
}

/// The 15 bytes of `PA-RL-TRANSACTION-ID` the reply carries: field 18 unchanged, unless it
/// holds eight or more consecutive bytes none of which is a space, which are all withheld as
/// `*` (COPAUA0C-062, X-COPAUA0C-11).
///
/// The screen keeps a requester-sent SSN, dashed or undashed, an 8-byte password and any
/// card-number fragment of eight or more digits out of the reply; a blank ID and an ID whose
/// space-separated parts are each at most seven bytes pass unchanged. Total, and decided from
/// the 15 bytes alone.
#[must_use]
pub fn transaction_id_screen(transaction_id: &PicX<15>) -> [u8; 15] {
    let bytes = *transaction_id.as_bytes();
    if has_run(&bytes, |byte| byte != b' ') {
        [WITHHELD; 15]
    } else {
        bytes
    }
}

/// The 14 bytes of the reply's amount: `WS-APPROVED-AMT-DIS PIC -zzzzzzzzz9.99` edited from the
/// approved amount, unless the edit holds eight or more consecutive digits, an absolute value
/// of 10,000,000.00 or more, which are all withheld as `*` (COPAUA0C-064, ARITH-039,
/// X-COPAUA0C-11).
///
/// 0.00 gives ten spaces then `0.00`; −5.25 gives `-`, nine spaces, then `5.25`; 9,999,999.99
/// passes; 10,000,000.00 is withheld.
#[must_use]
pub fn amount_screen(approved_amt: Money<10>) -> [u8; 14] {
    let edit = REPLY_AMOUNT.format(approved_amt.as_decimal());
    // The editor returns exactly 14 characters; the MOVE keeps that width whatever it returns.
    let bytes = *PicX::<14>::moved_from(edit.as_bytes()).as_bytes();
    if has_run(&bytes, |byte| byte.is_ascii_digit()) {
        [WITHHELD; 14]
    } else {
        bytes
    }
}

/// Whether `bytes` holds [`SCREEN_RUN`] or more consecutive bytes that satisfy `member`.
fn has_run(bytes: &[u8], member: impl Fn(u8) -> bool) -> bool {
    let mut run = 0_usize;
    for &byte in bytes {
        run = if member(byte) {
            run.saturating_add(1)
        } else {
            0
        };
        if run >= SCREEN_RUN {
            return true;
        }
    }
    false
}

/// The reply body: the 63 bytes the `STRING … DELIMITED BY SIZE INTO W02-PUT-BUFFER` writes,
/// each field at its full width followed by `,` (`COPAUA0C.cbl:720-731`; COPAUA0C-060 to
/// COPAUA0C-064, E03):
///
/// | Offset | Length | Bytes | Rule |
/// |---|---|---|---|
/// | 0 | 16 | the card number, redacted: twelve `*` and its last four digits | COPAUA0C-061 |
/// | 17 | 15 | the transaction ID through [`transaction_id_screen`] | COPAUA0C-062 |
/// | 33 | 6 | the auth-ID code, field 2, unscreened | COPAUA0C-062 |
/// | 40 | 2 | the response code, `00` or `05` | COPAUA0C-063 |
/// | 43 | 4 | the reason code | COPAUA0C-063 |
/// | 48 | 14 | the approved amount through [`amount_screen`] | COPAUA0C-064 |
///
/// with `,` at offsets 16, 32, 39, 42, 47 and 62. Field order is `CCPAURLY`'s and the bytes are
/// the emitted `STRING`'s, `-zzzzzzzzz9.99` rather than `+9(10).99` (E03). The COBOL copies the
/// full card number; the redaction is Cardholder Data Redaction (E32).
///
/// The worker publishes these bytes to the request's `reply_to` with `mandatory: true` and the
/// properties of COPAUA0C-066 and COPAUA0C-067: the request's `correlation_id`, a newly
/// generated `message_id`, `delivery_mode` 2 (persistent, overriding `MQPER-NOT-PERSISTENT`) and
/// `expiration` `"5000"` (E16).
#[must_use]
pub fn reply_body(reply: &AuthReply) -> Vec<u8> {
    let card = reply.card_num.redacted_bytes();
    let transaction_id = transaction_id_screen(&reply.transaction_id);
    let amount = amount_screen(reply.approved_amt);
    let fields: [&[u8]; 6] = [
        &card,
        &transaction_id,
        reply.auth_id_code.as_bytes(),
        reply.auth_resp_code.as_bytes(),
        reply.auth_resp_reason.as_bytes(),
        &amount,
    ];
    let mut body = Vec::with_capacity(REPLY_WRITTEN_LEN);
    for field in fields {
        body.extend_from_slice(field);
        body.push(DELIMITER);
    }
    // HALTED(H6), COPAUA0C-065: the COBOL sends 64 bytes, the pointer's end value, and the 64th
    // comes from `W02-PUT-BUFFER`, which has no VALUE; only the 63 written bytes are returned.
    // HALTED(H1), COPAUA0C-131: a later reply of the same task would carry the earlier replies'
    // bytes first; each body here is a task's first.
    body
}

/// The summary `lock_or_create` inserts for an account with none: `INITIALIZE
/// PENDING-AUTH-SUMMARY REPLACING NUMERIC DATA BY ZERO`, then the cross-reference's account and
/// customer IDs (`COPAUA0C.cbl:801-806`; COPAUA0C-025, COPAUA0C-092, ARITH-041).
///
/// Every limit, balance and amount is 0.00 and both counts are 0. The account key is the
/// unsigned `XREF-ACCT-ID` as the signed `PA-ACCT-ID`, through the exact conversion
/// (COPAUA0C-027). The alphanumeric fields are spaces, interim handling for
/// `HALTED(X-ARITH-8)` (COPAUA0C-093).
#[must_use]
pub fn summary_template(xref: &CardXrefRecord) -> PendingAuthSummaryRecord {
    PendingAuthSummaryRecord::initialized(PicS::from(xref.acct_id), xref.cust_id)
}

/// A summary count that `ADD 1` would take beyond its four digits, where the COBOL result
/// depends on the `TRUNC` compiler option and has no expected value (ARITH-042, X-ARITH-6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SummaryError {
    /// `PA-APPROVED-AUTH-CNT` is already 9999 (COPAUA0C-094).
    ApprovedCountAtLimit,
    /// `PA-DECLINED-AUTH-CNT` is already 9999 (COPAUA0C-095).
    DeclinedCountAtLimit,
}

impl fmt::Display for SummaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApprovedCountAtLimit => "PA-APPROVED-AUTH-CNT is at 9999 and cannot be added to",
            Self::DeclinedCountAtLimit => "PA-DECLINED-AUTH-CNT is at 9999 and cannot be added to",
        })
    }
}

impl std::error::Error for SummaryError {}

/// The summary row `8400-UPDATE-SUMMARY` writes for a decided request of a found card, built from
/// the row the worker locked (`COPAUA0C.cbl:798-834`; COPAUA0C-090 to COPAUA0C-097).
///
/// - A row this request created gets the cross-reference's account and customer IDs over the
///   zeroed template (COPAUA0C-092).
/// - With the account found, `credit_limit` and `cash_limit` are the account's limits stored into
///   `S9(09)V99`, losing a high-order digit (COPAUA0C-090, ARITH-040).
/// - An approval adds 1 to `approved_auth_cnt` and the approved amount to `approved_auth_amt` and
///   `credit_balance`, each stored into `S9(09)V99`, and sets `cash_balance` to 0.00
///   (COPAUA0C-094, ARITH-042, ARITH-043). The stored `approved_auth_amt` is the total the i10
///   guard bounded before approving (COPAUA0C-046). `credit_balance` is never reduced here or by
///   the purge (E30).
/// - A decline adds 1 to `declined_auth_cnt` (COPAUA0C-095).
///
/// Call it only after the request's acknowledgement (E29, COPAUA0C-081); the worker writes the
/// result with one `UPDATE` of the locked row (COPAUA0C-097).
///
/// # Errors
///
/// [`SummaryError`] when the count to be added to is already 9999, which has no expected next
/// value (X-ARITH-6); the worker writes no summary it cannot build.
pub fn summary_update(
    summary: SummaryState<'_>,
    xref: &CardXrefRecord,
    account: Option<&AccountRecord>,
    decision: &Decision,
) -> Result<PendingAuthSummaryRecord, SummaryError> {
    let mut row = summary.row().clone();
    if let SummaryState::Created(_) = summary {
        row.acct_id = PicS::from(xref.acct_id);
        row.cust_id = xref.cust_id;
    }
    if let Some(account) = account {
        // TRUNCATION: `MOVE ACCT-CREDIT-LIMIT TO PA-CREDIT-LIMIT` and the cash limit move into a
        // `S9(09)V99` receiver from a `S9(10)V99` sender, dropping the high-order digit.
        row.credit_limit = MoneyExpr::from(account.credit_limit).store::<9>();
        row.cash_limit = MoneyExpr::from(account.cash_credit_limit).store::<9>();
    }
    // HALTED(X-COPAUA0C-H2), COPAUA0C-091: with no account row the COBOL moves whatever
    // ACCOUNT-RECORD held; the locked row's limits are kept as interim handling, not a parity
    // result.
    match decision.status {
        AuthStatus::Approved => {
            let approved = decision.reply.approved_amt;
            row.approved_auth_cnt =
                add_one(row.approved_auth_cnt, SummaryError::ApprovedCountAtLimit)?;
            row.approved_auth_amt = approved_total(row.approved_auth_amt, approved);
            // TRUNCATION: the ADD stores into `S9(09)V99`, dropping a high-order digit
            // (ARITH-043).
            row.credit_balance = (row.credit_balance + approved).store::<9>();
            row.cash_balance = Money::ZERO;
        }
        AuthStatus::Declined => {
            row.declined_auth_cnt =
                add_one(row.declined_auth_cnt, SummaryError::DeclinedCountAtLimit)?;
            // HALTED(H2), COPAUA0C-096: the COBOL adds PA-TRANSACTION-AMT before 8500-INSERT-AUTH
            // first assigns it; this request's amount, the value line 885 later moves there, is
            // interim handling, not a parity result.
            row.declined_auth_amt = (row.declined_auth_amt + decision.transaction_amt).store::<9>();
        }
    }
    Ok(row)
}

/// `ADD 1 TO` a summary count, or `limit` when the count has no room (ARITH-042, X-ARITH-6).
fn add_one(count: Count, limit: SummaryError) -> Result<Count, SummaryError> {
    count.add_one().map_err(|_: ErrorKind| limit)
}

/// The detail `8500-INSERT-AUTH` inserts for a decided request of a found card
/// (`COPAUA0C.cbl:854-919`; COPAUA0C-098 to COPAUA0C-103).
///
/// - The key comes from one [`Clock`] reading: `99999 − YYDDD` and `999999999 − HHMMSSmmm`, the
///   time in milliseconds, so two requests decided under one instant get the same key and the
///   second insert takes the `I004` path (COPAUA0C-098, COPAUA0C-099).
/// - The request's fields are copied as moved, the card number in full, which is storage and not
///   egress (COPAUA0C-100).
/// - The auth-ID code, response code, reason and approved amount are the reply's;
///   `match_status` is `P` when approved and `D` when declined; `auth_fraud` is a space and
///   `fraud_rpt_date` eight spaces (COPAUA0C-101).
///
/// The account key, the parent `ACCNTID`, is no field of the segment: the worker inserts the
/// detail under the cross-reference's account (COPAUA0C-027, COPAUA0C-101). Call it only after
/// the acknowledgement (E29).
#[must_use]
pub fn detail_record(
    request: &AuthRequestRecord,
    decision: &Decision,
    clock: &dyn Clock,
) -> PendingAuthDetailRecord {
    let (auth_date_9c, auth_time_9c) = inverted_auth_keys(clock.now());
    let match_status = match decision.status {
        AuthStatus::Approved => PendingAuthDetailRecord::MATCH_PENDING,
        AuthStatus::Declined => PendingAuthDetailRecord::MATCH_AUTH_DECLINED,
    };
    let reply = &decision.reply;
    PendingAuthDetailRecord {
        auth_date_9c,
        auth_time_9c,
        orig_date: request.auth_date.clone(),
        orig_time: request.auth_time.clone(),
        card_num: request.card_num.clone(),
        auth_type: request.auth_type.clone(),
        card_expiry_date: request.card_expiry_date.clone(),
        message_type: request.message_type.clone(),
        message_source: request.message_source.clone(),
        auth_id_code: reply.auth_id_code.clone(),
        auth_resp_code: reply.auth_resp_code.clone(),
        auth_resp_reason: reply.auth_resp_reason.clone(),
        processing_code: request.processing_code,
        transaction_amt: request.transaction_amt,
        approved_amt: reply.approved_amt,
        merchant_catagory_code: request.merchant_catagory_code.clone(),
        acqr_country_code: request.acqr_country_code.clone(),
        pos_entry_mode: request.pos_entry_mode,
        merchant_id: request.merchant_id.clone(),
        merchant_name: request.merchant_name.clone(),
        merchant_city: request.merchant_city.clone(),
        merchant_state: request.merchant_state.clone(),
        merchant_zip: request.merchant_zip.clone(),
        transaction_id: request.transaction_id.clone(),
        match_status: PicX::from_array(*match_status),
        auth_fraud: PicX::spaces(),
        fraud_rpt_date: PicX::spaces(),
        // HALTED(H2), COPAUA0C-102: FILLER is never assigned and the segment has no VALUE;
        // spaces are interim handling, not a parity result.
        filler: PicX::spaces(),
    }
}

/// A `PostgreSQL` SQLSTATE, five ASCII digits or upper-case letters, the interim content of
/// `ERR-CODE-1` for a failed database step (COPAUA0C-112, `HALTED(H4)`).
///
/// Five such characters hold no card number, SSN or password, so a log row's code field cannot
/// carry key data (Cardholder Data Redaction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SqlState([u8; 5]);

impl SqlState {
    /// The SQLSTATE `text` names, or `None` when it is not five ASCII digits or upper-case
    /// letters.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = <[u8; 5]>::try_from(text.as_bytes()).ok()?;
        bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
            .then_some(Self(bytes))
    }

    /// The five characters.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 5] {
        &self.0
    }
}

/// A `PERFORM 9500-LOG-ERROR` call site the replacement reaches, with the data its event key and
/// codes come from (COPAUA0C-110 to COPAUA0C-126).
///
/// Each variant names its `ERR-LOCATION`. A card number reaches the event key only through its
/// redacted form, and an account or customer ID only as its digits, so no site can put a full
/// card number into a log row (E32). `sqlstate` is the failed statement's SQLSTATE where the
/// worker has one. `M001`, `M003` and `M005` have no counterpart operation and no variant (not
/// carried over).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSite<'a> {
    /// `A001` `CARD NOT FOUND IN XREF`, a warning: the request's card has no `card_xref` row
    /// (COPAUA0C-114).
    A001 {
        /// The request's card number.
        card: &'a CardNumber,
    },
    /// `A002` `ACCT NOT FOUND IN XREF`, a warning: the cross-reference's account has no row
    /// (COPAUA0C-115).
    A002 {
        /// `XREF-ACCT-ID`.
        acct_id: PicU<11>,
    },
    /// `A003` `CUST NOT FOUND IN XREF`, a warning: the cross-reference's customer has no row
    /// (COPAUA0C-116).
    A003 {
        /// `XREF-CUST-ID`.
        cust_id: PicU<9>,
    },
    /// `C001` `FAILED TO READ XREF FILE`: the `card_xref` read failed (COPAUA0C-122).
    C001 {
        /// The request's card number.
        card: &'a CardNumber,
        /// The failed read's SQLSTATE.
        sqlstate: Option<SqlState>,
    },
    /// `C002` `FAILED TO READ ACCT FILE`: the `accounts` read failed (COPAUA0C-123).
    C002 {
        /// `XREF-ACCT-ID`.
        acct_id: PicU<11>,
        /// The failed read's SQLSTATE.
        sqlstate: Option<SqlState>,
    },
    /// `C003` `FAILED TO READ CUST FILE`: the `customers` read failed (COPAUA0C-124).
    C003 {
        /// `XREF-CUST-ID`.
        cust_id: PicU<9>,
        /// The failed read's SQLSTATE.
        sqlstate: Option<SqlState>,
    },
    /// `I001` `IMS SCHD FAILED`: the nearest case is the worker failing to open its
    /// authorization transaction (COPAUA0C-126).
    I001,
    /// `I002` `IMS GET SUMMARY FAILED`: `lock_or_create` failed (COPAUA0C-125).
    I002 {
        /// The request's card number.
        card: &'a CardNumber,
        /// The failed statement's SQLSTATE.
        sqlstate: Option<SqlState>,
    },
    /// `M004` `FAILED TO PUT ON REPLY MQ`: the reply's publication was not confirmed
    /// (COPAUA0C-117, COPAUA0C-118).
    M004 {
        /// The request's card number.
        card: &'a CardNumber,
    },
    /// `I003` `IMS UPDATE SUMRY FAILED`: the summary update failed (COPAUA0C-119).
    I003 {
        /// The request's card number.
        card: &'a CardNumber,
        /// The failed update's SQLSTATE.
        sqlstate: Option<SqlState>,
    },
    /// `I004` `IMS INSERT DETL FAILED`: the detail insert failed, for example on a duplicate key
    /// (COPAUA0C-099, COPAUA0C-120).
    I004 {
        /// The request's card number, which line 879 moved into `PA-CARD-NUM`.
        card: &'a CardNumber,
        /// The failed insert's SQLSTATE.
        sqlstate: Option<SqlState>,
    },
}

impl LogSite<'_> {
    /// `ERR-LOCATION` (CCPAUERY-005).
    #[must_use]
    pub const fn location(self) -> &'static [u8; 4] {
        match self {
            Self::A001 { .. } => b"A001",
            Self::A002 { .. } => b"A002",
            Self::A003 { .. } => b"A003",
            Self::C001 { .. } => b"C001",
            Self::C002 { .. } => b"C002",
            Self::C003 { .. } => b"C003",
            Self::I001 => b"I001",
            Self::I002 { .. } => b"I002",
            Self::M004 { .. } => b"M004",
            Self::I003 { .. } => b"I003",
            Self::I004 { .. } => b"I004",
        }
    }

    /// `ERR-LEVEL`: `ERR-WARNING` for the not-found sites, `ERR-CRITICAL` for the failures
    /// (CCPAUERY-006).
    #[must_use]
    pub const fn level(self) -> ErrorLevel {
        match self {
            Self::A001 { .. } | Self::A002 { .. } | Self::A003 { .. } => ErrorLevel::Warning,
            Self::C001 { .. }
            | Self::C002 { .. }
            | Self::C003 { .. }
            | Self::I001
            | Self::I002 { .. }
            | Self::M004 { .. }
            | Self::I003 { .. }
            | Self::I004 { .. } => ErrorLevel::Critical,
        }
    }

    /// `ERR-SUBSYSTEM`: `ERR-APP`, `ERR-CICS`, `ERR-IMS` or `ERR-MQ` (CCPAUERY-007).
    #[must_use]
    pub const fn subsystem(self) -> ErrorSubsystem {
        match self {
            Self::A001 { .. } | Self::A002 { .. } | Self::A003 { .. } => ErrorSubsystem::App,
            Self::C001 { .. } | Self::C002 { .. } | Self::C003 { .. } => ErrorSubsystem::Cics,
            Self::I001 | Self::I002 { .. } | Self::I003 { .. } | Self::I004 { .. } => {
                ErrorSubsystem::Ims
            }
            Self::M004 { .. } => ErrorSubsystem::Mq,
        }
    }

    /// The `ERR-MESSAGE` literal, before its padding to 50 bytes (CCPAUERY-010).
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::A001 { .. } => "CARD NOT FOUND IN XREF",
            Self::A002 { .. } => "ACCT NOT FOUND IN XREF",
            Self::A003 { .. } => "CUST NOT FOUND IN XREF",
            Self::C001 { .. } => "FAILED TO READ XREF FILE",
            Self::C002 { .. } => "FAILED TO READ ACCT FILE",
            Self::C003 { .. } => "FAILED TO READ CUST FILE",
            Self::I001 => "IMS SCHD FAILED",
            Self::I002 { .. } => "IMS GET SUMMARY FAILED",
            Self::M004 { .. } => "FAILED TO PUT ON REPLY MQ",
            Self::I003 { .. } => "IMS UPDATE SUMRY FAILED",
            Self::I004 { .. } => "IMS INSERT DETL FAILED",
        }
    }

    /// Whether the site is `ERR-CRITICAL`, after which `9500-LOG-ERROR` performs
    /// `9990-END-ROUTINE` and the COBOL task ends with a normal `EXEC CICS RETURN`, committing
    /// its unit of work (`COPAUA0C.cbl:1008-1022`; COPAUA0C-110). The worker has no task boundary
    /// and takes the next request; a warning lets processing continue (COPAUA0C-114).
    #[must_use]
    pub const fn ends_task(self) -> bool {
        matches!(self.level(), ErrorLevel::Critical)
    }

    /// `ERR-EVENT-KEY`, 20 bytes (CCPAUERY-011).
    fn event_key(self) -> PicX<20> {
        match self {
            // The redacted card number, then four spaces (E32): COPAUA0C-114, COPAUA0C-120 and
            // COPAUA0C-122 for `A001`, `I004` and `C001`. For `I002`, `M004` and `I003` the COBOL
            // takes the key from a `PA-CARD-NUM` not yet assigned for this request (E34, pending
            // its fold into H2; COPAUA0C-113), so there the request's card is interim handling,
            // not a parity result.
            Self::A001 { card }
            | Self::C001 { card, .. }
            | Self::I004 { card, .. }
            | Self::I002 { card, .. }
            | Self::M004 { card }
            | Self::I003 { card, .. } => card_event_key(card),
            // COPAUA0C-115, COPAUA0C-123: the account ID's 11 digits, then nine spaces.
            Self::A002 { acct_id } | Self::C002 { acct_id, .. } => digits_key(acct_id),
            // COPAUA0C-116, COPAUA0C-124: the customer ID's nine digits, then 11 spaces.
            Self::A003 { cust_id } | Self::C003 { cust_id, .. } => digits_key(cust_id),
            // HALTED(X-COPAUA0C-H1), COPAUA0C-111: `I001` never moves the key; interim spaces.
            Self::I001 => PicX::spaces(),
        }
    }

    /// `ERR-CODE-1`, 9 bytes (CCPAUERY-008).
    ///
    /// HALTED(H4), COPAUA0C-112: the COBOL holds the CICS `RESP`, the MQ completion code or the
    /// DL/I status here, which have no counterpart. The interim content is the failed statement's
    /// SQLSTATE, left-justified, for `C001` to `C003` and `I002` to `I004`, and spaces otherwise;
    /// `A001` to `A003` never move the field, HALTED(X-COPAUA0C-H1) (COPAUA0C-111).
    fn code_1(self) -> PicX<9> {
        match self {
            Self::C001 { sqlstate, .. }
            | Self::C002 { sqlstate, .. }
            | Self::C003 { sqlstate, .. }
            | Self::I002 { sqlstate, .. }
            | Self::I003 { sqlstate, .. }
            | Self::I004 { sqlstate, .. } => {
                sqlstate.map_or_else(PicX::spaces, |state| PicX::moved_from(state.as_bytes()))
            }
            Self::A001 { .. }
            | Self::A002 { .. }
            | Self::A003 { .. }
            | Self::I001
            | Self::M004 { .. } => PicX::spaces(),
        }
    }
}

/// An account or customer ID as `ERR-EVENT-KEY` receives it: its digits, leading zeros kept,
/// then spaces (COPAUA0C-115, COPAUA0C-116).
fn digits_key<const N: u32>(id: PicU<N>) -> PicX<20> {
    let mut digits = Vec::new();
    id.encode_into(&mut digits);
    PicX::moved_from(&digits)
}

/// The `cssl_error_log` row of one `9500-LOG-ERROR` call (`COPAUA0C.cbl:983-1006`;
/// COPAUA0C-110 to COPAUA0C-126).
///
/// - `date` and `time` are the `YYMMDD` and 24-hour `HHMMSS` texts of one [`Clock`] reading.
/// - `application` is `CP00` and four spaces, `program` `COPAUA0C`.
/// - Location, level, subsystem, the message padded to 50 bytes and the event key padded to 20
///   come from the site; a card number appears only as `************NNNN` (E32).
/// - `code_1` is interim handling for `HALTED(H4)` (COPAUA0C-112); `code_2` is spaces, the
///   field the COBOL leaves unassigned at `A001` to `A003` and `I001` to `I004`
///   (`HALTED(X-COPAUA0C-H1)`, COPAUA0C-111) and fills with MQ or CICS codes elsewhere
///   (`HALTED(H4)`).
///
/// The worker writes the row in a transaction of its own, committed at once, whether or not the
/// authorization transaction commits (AAP 0.5.2). A site whose [`LogSite::ends_task`] holds ended
/// the COBOL task; the worker takes the next request.
#[must_use]
pub fn error_log(site: LogSite<'_>, clock: &dyn Clock) -> ErrorLogRecord {
    let at = clock.now();
    ErrorLogRecord {
        date: PicX::moved_from(yymmdd(at).as_bytes()),
        time: PicX::moved_from(hhmmss(at).as_bytes()),
        application: PicX::moved_from(TRANSACTION.as_bytes()),
        program: PicX::moved_from(PROGRAM.as_bytes()),
        location: PicX::from_array(*site.location()),
        level: site.level().pic(),
        subsystem: site.subsystem().pic(),
        code_1: site.code_1(),
        // HALTED(H4) and HALTED(X-COPAUA0C-H1), COPAUA0C-111: interim spaces.
        code_2: PicX::spaces(),
        message: PicX::moved_from(site.message().as_bytes()),
        event_key: site.event_key(),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{Count, FixedRecord, PicDate, PicU, PicX};

    use super::{
        ALERT_CARD_UNPARSEABLE, AuthStatus, CardNumberCause, CardNumberError, CustomerLookup,
        DeclineFlag, FieldFault, FoundContext, GET_BUFFER_LEN, GuardOutcome, LogSite, Lookup,
        PROGRAM, ParseError, REPLY_WRITTEN_LEN, REQUEST_FIELDS, ReasonCode, RequestField, SqlState,
        SummaryError, SummaryState, TRANSACTION, amount_screen, decide, decline_reason,
        detail_record, error_log, field_error_reply, numval, parse_request, reply_body,
        store_failure_reply, summary_template, summary_update, transaction_id_screen,
    };
    use crate::clock::FixedClock;
    use crate::money::{Decimal, Money, MoneyError};
    use crate::records::{
        AccountRecord, AuthRequestRecord, CardXrefRecord, ErrorLevel, ErrorSubsystem,
        PendingAuthDetailRecord, PendingAuthSummaryRecord,
    };
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A synthetic card number, in no fixture.
    const CARD: &str = "4000123412345740";

    /// The redacted form of [`CARD`].
    const CARD_REDACTED: &[u8; 16] = b"************5740";

    /// A well-formed request, one entry per field in wire order.
    const FIELDS: [&str; 18] = [
        "220618",
        "143012",
        CARD,
        "0100",
        "0927",
        "0200",
        "POS",
        "000000",
        "100.00",
        "5411",
        "840",
        "05",
        "MERCHANT0000001",
        "CORNER GROCERY",
        "SEATTLE",
        "WA",
        "98101",
        "ORDER-A",
    ];

    /// `fields` joined by commas.
    fn body_of(fields: &[&str]) -> Vec<u8> {
        fields.join(",").into_bytes()
    }

    /// [`FIELDS`] with field `position` (1-based) replaced by `value`.
    fn body_with(position: usize, value: &str) -> Vec<u8> {
        let fields: Vec<&str> = FIELDS
            .iter()
            .enumerate()
            .map(|(k, &field)| if k + 1 == position { value } else { field })
            .collect();
        body_of(&fields)
    }

    /// `cents` hundredths as `Money<N>`.
    fn money<const N: u32>(cents: i64) -> Result<Money<N>, MoneyError> {
        Money::new(Decimal::new(cents, 2))
    }

    /// The fault of a field error, or the parse outcome that is not one.
    fn fault_of(body: &[u8]) -> Result<FieldFault, Box<dyn StdError>> {
        match parse_request(body) {
            Err(ParseError::Field(error)) => Ok(error.fault),
            Err(ParseError::CardNumber(_)) => Err("card-number error".into()),
            Ok(_) => Err("parsed".into()),
        }
    }

    /// The cause of a card-number error.
    fn card_cause(body: &[u8]) -> Result<CardNumberCause, Box<dyn StdError>> {
        match parse_request(body) {
            Err(ParseError::CardNumber(error)) => {
                assert_eq!(error.body_len, body.len());
                Ok(error.cause)
            }
            _ => Err("not a card-number error".into()),
        }
    }

    /// Whether `needle` occurs in `haystack`.
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    // audit: COPAUA0C-002
    // audit: COPAUA0C-003
    // audit: COPAUA0C-013
    #[test]
    fn a_well_formed_body_parses_into_the_request_storage() -> TestResult {
        let request = parse_request(&body_of(&FIELDS))?;
        assert_eq!(request.auth_date.as_bytes(), b"220618");
        assert_eq!(request.auth_time.as_bytes(), b"143012");
        assert_eq!(request.card_num, CardNumber::parse(CARD)?);
        assert_eq!(request.auth_type.as_bytes(), b"0100");
        assert_eq!(request.card_expiry_date.as_bytes(), b"0927");
        assert_eq!(request.message_type.as_bytes(), b"0200  ");
        assert_eq!(request.message_source.as_bytes(), b"POS   ");
        assert_eq!(request.processing_code.get(), 0);
        assert_eq!(request.transaction_amt, money::<10>(10_000)?);
        assert_eq!(request.merchant_catagory_code.as_bytes(), b"5411");
        assert_eq!(request.acqr_country_code.as_bytes(), b"840");
        assert_eq!(request.pos_entry_mode.get(), 5);
        assert_eq!(request.merchant_id.as_bytes(), b"MERCHANT0000001");
        assert_eq!(request.merchant_name.as_bytes(), b"CORNER GROCERY        ");
        assert_eq!(request.merchant_city.as_bytes(), b"SEATTLE      ");
        assert_eq!(request.merchant_state.as_bytes(), b"WA");
        assert_eq!(request.merchant_zip.as_bytes(), b"98101    ");
        assert_eq!(request.transaction_id.as_bytes(), b"ORDER-A        ");
        Ok(())
    }

    // audit: COPAUA0C-003
    #[test]
    fn alphanumeric_fields_move_left_justified_padded_and_cut() -> TestResult {
        let long_name = parse_request(&body_with(14, "ABCDEFGHIJKLMNOPQRSTUVWXY"))?;
        assert_eq!(
            long_name.merchant_name.as_bytes(),
            b"ABCDEFGHIJKLMNOPQRSTUV"
        );
        let empty_date = parse_request(&body_with(1, ""))?;
        assert_eq!(empty_date.auth_date.as_bytes(), b"      ");
        let leading_space = parse_request(&body_with(16, " w"))?;
        assert_eq!(leading_space.merchant_state.as_bytes(), b" w");
        Ok(())
    }

    // audit: COPAUA0C-004
    #[test]
    fn numeric_fields_keep_their_rightmost_digits() -> TestResult {
        let cases: [(usize, &str, u64); 6] = [
            (8, "7", 7),
            (8, "1234567", 234_567),
            (8, "", 0),
            (12, "123", 23),
            (12, "7", 7),
            (12, "", 0),
        ];
        for (position, text, expected) in cases {
            let request = parse_request(&body_with(position, text))?;
            let value = if position == 8 {
                request.processing_code.get()
            } else {
                request.pos_entry_mode.get()
            };
            assert_eq!(value, expected, "field {position}");
        }
        Ok(())
    }

    // audit: COPAUA0C-002
    // audit: COPAUA0C-007
    #[test]
    fn a_final_comma_opens_no_field_and_extra_fields_are_ignored() -> TestResult {
        let expected = parse_request(&body_of(&FIELDS))?;
        let mut trailing = body_of(&FIELDS);
        trailing.push(b',');
        assert_eq!(parse_request(&trailing)?, expected);
        let mut extra = body_of(&FIELDS);
        extra.extend_from_slice(b",EXTRA,\t,MORE");
        assert_eq!(parse_request(&extra)?, expected);
        let (_, seventeen) = FIELDS.split_last().ok_or("no fields")?;
        let mut short = body_of(seventeen);
        short.push(b',');
        assert_eq!(
            fault_of(&short)?,
            FieldFault::Missing(RequestField::TransactionId)
        );
        assert_eq!(
            fault_of(&body_of(&[
                FIELDS.first().copied().unwrap_or_default(),
                "1",
                CARD
            ]))?,
            FieldFault::Missing(RequestField::AuthType)
        );
        Ok(())
    }

    // audit: COPAUA0C-005
    // audit: COPAUA0C-006
    // audit: COPAUA0C-017
    #[test]
    fn field_three_must_be_exactly_sixteen_ascii_digits() -> TestResult {
        assert_eq!(ALERT_CARD_UNPARSEABLE, "CP00_CARD_UNPARSEABLE");
        for body in [
            b"".as_slice(),
            b"220618",
            b"220618,143012",
            b"220618,143012,",
        ] {
            assert_eq!(card_cause(body)?, CardNumberCause::Absent);
        }
        let malformed: [&[u8]; 6] = [
            b"ABC",
            b"400012341234574",
            b"40001234123457401",
            b"4000 12341234574",
            b"",
            b"40001234123457\xC34",
        ];
        for card in malformed {
            let mut body = b"220618,143012,".to_vec();
            body.extend_from_slice(card);
            body.extend_from_slice(b",0100");
            assert_eq!(card_cause(&body)?, CardNumberCause::NotSixteenDigits);
        }
        // The card check runs before every field check (COPAUA0C-017).
        let both = body_of(&["220618", "143012", "ABC", "0100", "NOT-AN-AMOUNT"]);
        assert_eq!(card_cause(&both)?, CardNumberCause::NotSixteenDigits);
        // A well-formed number is a valid card whether or not a cross-reference row exists.
        assert!(parse_request(&body_with(3, "9999999999999999")).is_ok());
        Ok(())
    }

    // audit: COPAUA0C-007
    // audit: COPAUA0C-009
    // audit: COPAUA0C-010
    // audit: COPAUA0C-011
    #[test]
    fn malformed_fields_with_a_parseable_card_are_field_errors() -> TestResult {
        assert_eq!(
            fault_of(&body_of(&["220618", "143012", CARD]))?,
            FieldFault::Missing(RequestField::AuthType)
        );
        assert_eq!(
            fault_of(&body_with(8, "12 4"))?,
            FieldFault::NonDigit(RequestField::ProcessingCode)
        );
        assert_eq!(
            fault_of(&body_with(12, "+5"))?,
            FieldFault::NonDigit(RequestField::PosEntryMode)
        );
        assert_eq!(
            fault_of(&body_with(14, "CORNER\tGROCERY"))?,
            FieldFault::NonPrintable(RequestField::MerchantName)
        );
        assert_eq!(
            fault_of(&body_with(1, "22061\u{80}"))?,
            FieldFault::NonPrintable(RequestField::AuthDate)
        );
        let long_name = "N".repeat(GET_BUFFER_LEN);
        let long = body_with(14, &long_name);
        assert_eq!(
            fault_of(&long)?,
            FieldFault::BodyTooLong { len: long.len() }
        );
        // A body of exactly 500 bytes is read whole and parses.
        let fitting_name = "N".repeat(GET_BUFFER_LEN - body_with(14, "").len());
        let fitting = body_with(14, &fitting_name);
        assert_eq!(fitting.len(), GET_BUFFER_LEN);
        assert!(parse_request(&fitting).is_ok());
        Ok(())
    }

    // audit: COPAUA0C-008
    // audit: COPAUA0C-014
    #[test]
    fn numval_reads_both_argument_formats_exactly() {
        let valid: [(&[u8], i64); 12] = [
            (b"100", 10_000),
            (b"  100", 10_000),
            (b"+100.50", 10_050),
            (b"- 5", -500),
            (b"5.25-", -525),
            (b"5 CR", -500),
            (b"5DB", -500),
            (b".5", 50),
            (b"5.", 500),
            (b"  7  ", 700),
            (b"+ .25", 25),
            (b"12 +", 1_200),
        ];
        for (text, cents) in valid {
            assert_eq!(numval(text), Some(Decimal::new(cents, 2)));
        }
        let invalid: [&[u8]; 14] = [
            b"", b"     ", b"1 000", b"1.2.3", b"+5-", b"-", b"+.", b".", b"5cr", b"5db", b"5X",
            b"--5", b"CR5", b"5 CR X",
        ];
        for text in invalid {
            assert_eq!(numval(text), None);
        }
    }

    // audit: COPAUA0C-008
    // audit: COPAUA0C-014
    #[test]
    fn the_amount_is_cut_to_thirteen_bytes_and_stored_by_truncation() -> TestResult {
        let cases: [(&str, i64); 4] = [
            ("12345678901.5", 234_567_890_150),
            ("+0000000123.45", 12_340),
            ("  100", 10_000),
            ("5.25-", -525),
        ];
        for (text, cents) in cases {
            let request = parse_request(&body_with(9, text))?;
            assert_eq!(request.transaction_amt, money::<10>(cents)?);
        }
        for text in ["ABC", "", "1 000", "+5-"] {
            assert_eq!(fault_of(&body_with(9, text))?, FieldFault::AmountNotNumeric);
        }
        Ok(())
    }

    // HALTED(X-ARITH-5), COPAUA0C-015, and HALTED(X-COPAUA0C-H3), COPAUA0C-016: interim handling
    // runs here and no value is asserted as parity.
    #[test]
    fn halted_amount_forms_parse_without_failing() {
        for text in ["1.005", ".125", "-0", "0CR", ".00DB"] {
            assert!(parse_request(&body_with(9, text)).is_ok());
        }
    }

    // audit: COPAUA0C-012
    #[test]
    fn a_field_error_reply_declines_with_reason_9000() -> TestResult {
        let Err(ParseError::Field(error)) = parse_request(&body_with(9, "ABC")) else {
            return Err("not a field error".into());
        };
        let reply = field_error_reply(&error);
        assert_eq!(reply.card_num, CardNumber::parse(CARD)?);
        assert_eq!(reply.auth_resp_code.as_bytes(), b"05");
        assert_eq!(reply.auth_resp_reason.as_bytes(), b"9000");
        assert!(reply.approved_amt.is_zero());
        assert_eq!(reply.auth_id_code.as_bytes(), b"143012");
        assert_eq!(reply.transaction_id.as_bytes(), b"ORDER-A        ");
        let Err(ParseError::Field(short)) = parse_request(&body_of(&["220618", "143012", CARD]))
        else {
            return Err("not a field error".into());
        };
        let reply = field_error_reply(&short);
        assert!(reply.transaction_id.is_spaces());
        assert_eq!(
            reply_body(&reply),
            b"************5740,               ,143012,05,9000,          0.00,"
        );
        Ok(())
    }

    // The builder serves the five critical read rows alike.
    // audit: COPAUA0C-122
    // audit: COPAUA0C-123
    // audit: COPAUA0C-124
    // audit: COPAUA0C-125
    // audit: COPAUA0C-126
    #[test]
    fn a_store_failure_reply_declines_with_reason_9000() -> TestResult {
        let request = request(10_000)?;
        let reply = store_failure_reply(&request);
        assert_eq!(reply.card_num, CardNumber::parse(CARD)?);
        assert_eq!(reply.auth_resp_code.as_bytes(), b"05");
        assert_eq!(reply.auth_resp_reason.as_bytes(), b"9000");
        assert!(reply.approved_amt.is_zero());
        assert_eq!(reply.auth_id_code.as_bytes(), b"143012");
        assert_eq!(reply.transaction_id.as_bytes(), b"ORDER-A        ");
        let body = reply_body(&reply);
        assert_eq!(body.len(), REPLY_WRITTEN_LEN);
        assert_eq!(
            body,
            b"************5740,ORDER-A        ,143012,05,9000,          0.00,"
        );
        assert!(!contains(&body, CARD.as_bytes()), "COPAUA0C-061");
        // The transaction-ID screen applies as in every reply (COPAUA0C-062).
        let numeric_id = AuthRequestRecord {
            transaction_id: PicX::from_array(*b"123456789012345"),
            ..request
        };
        assert_eq!(
            reply_body(&store_failure_reply(&numeric_id)),
            b"************5740,***************,143012,05,9000,          0.00,"
        );
        Ok(())
    }

    #[test]
    fn errors_name_rules_and_fields_never_body_bytes() -> TestResult {
        let card_error = CardNumberError {
            body_len: 3,
            cause: CardNumberCause::NotSixteenDigits,
        };
        assert!(card_error.to_string().contains("16 ASCII digits"));
        let absent = ParseError::CardNumber(CardNumberError {
            body_len: 0,
            cause: CardNumberCause::Absent,
        });
        assert!(absent.to_string().contains("field 3"));
        assert!(std::error::Error::source(&absent).is_some());
        assert!(
            SummaryError::ApprovedCountAtLimit
                .to_string()
                .contains("PA-APPROVED-AUTH-CNT")
        );
        let Err(error) = parse_request(&body_with(8, "X")) else {
            return Err("not an error".into());
        };
        for rendered in [format!("{error}"), format!("{error:?}")] {
            assert!(!rendered.contains(CARD), "COPAUA0C-133");
            assert!(!rendered.contains("ORDER-A"), "COPAUA0C-133");
        }
        assert!(error.to_string().contains("PA-RQ-PROCESSING-CODE"));
        assert!(std::error::Error::source(&error).is_some());
        for fault in [
            FieldFault::BodyTooLong { len: 501 },
            FieldFault::Missing(RequestField::AuthType),
            FieldFault::NonPrintable(RequestField::AuthDate),
            FieldFault::AmountNotNumeric,
        ] {
            assert!(!fault.to_string().is_empty());
        }
        for field in REQUEST_FIELDS {
            let name = field.cobol_name();
            assert!(name.starts_with("PA-RQ-") || name == "WS-TRANSACTION-AMT-AN");
        }
        Ok(())
    }

    /// A cross-reference row for [`CARD`]: customer 50, account 50.
    fn xref() -> Result<CardXrefRecord, Box<dyn StdError>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::parse(CARD)?,
            cust_id: PicU::new(50)?,
            acct_id: PicU::new(50)?,
            filler: PicX::spaces(),
        })
    }

    /// Account 50 with `balance`, `limit` and a cash limit, in hundredths.
    fn account(
        balance: i64,
        limit: i64,
        cash_limit: i64,
    ) -> Result<AccountRecord, Box<dyn StdError>> {
        Ok(AccountRecord {
            id: PicU::new(50)?,
            active_status: PicX::from_array(*b"Y"),
            curr_bal: money(balance)?,
            credit_limit: money(limit)?,
            cash_credit_limit: money(cash_limit)?,
            open_date: PicDate::BLANK,
            expiraion_date: PicDate::BLANK,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: Money::ZERO,
            addr_zip: PicX::spaces(),
            group_id: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// A summary of account 50 with a credit limit, credit balance and approved amount, in
    /// hundredths.
    fn summary(
        limit: i64,
        balance: i64,
        approved: i64,
    ) -> Result<PendingAuthSummaryRecord, Box<dyn StdError>> {
        Ok(PendingAuthSummaryRecord {
            credit_limit: money(limit)?,
            credit_balance: money(balance)?,
            approved_auth_amt: money(approved)?,
            ..summary_template(&xref()?)
        })
    }

    /// The well-formed request with an amount of `cents` hundredths.
    fn request(cents: i64) -> Result<AuthRequestRecord, Box<dyn StdError>> {
        Ok(AuthRequestRecord {
            transaction_amt: money(cents)?,
            ..parse_request(&body_of(&FIELDS))?
        })
    }

    /// The decision for `cents` with the given lookups.
    fn decision_for(
        cents: i64,
        account: Option<&AccountRecord>,
        customer: CustomerLookup,
        summary: SummaryState<'_>,
    ) -> Result<(AuthStatus, ReasonCode, GuardOutcome), Box<dyn StdError>> {
        let xref = xref()?;
        let lookup = Lookup::CardFound(FoundContext {
            xref: &xref,
            account,
            customer,
            summary,
        });
        let decision = decide(&request(cents)?, &lookup);
        Ok((decision.status, decision.reason, decision.guard))
    }

    // HALTED(X-HALTS-1), COPAUA0C-040: the interim `05`/`3100` runs here and is not asserted as
    // parity.
    #[test]
    fn a_card_without_a_cross_reference_row_is_declined_in_the_interim() -> TestResult {
        let decision = decide(&request(10_000)?, &Lookup::CardNotInXref);
        assert_eq!(decision.guard, GuardOutcome::NotApplied);
        assert!(decision.reply.approved_amt.is_zero());
        Ok(())
    }

    // audit: COPAUA0C-041
    // audit: COPAUA0C-044
    // audit: COPAUA0C-045
    #[test]
    fn the_summary_path_declines_only_strictly_above_the_available_amount() -> TestResult {
        let room = account(0, 1_000_000, 0)?;
        let row = summary(500_000, 120_000, 0)?;
        let found = CustomerLookup::Found;
        assert_eq!(
            decision_for(380_000, Some(&room), found, SummaryState::Existing(&row))?,
            (
                AuthStatus::Approved,
                ReasonCode::Approved0000,
                GuardOutcome::Passed
            )
        );
        assert_eq!(
            decision_for(380_001, Some(&room), found, SummaryState::Existing(&row))?,
            (
                AuthStatus::Declined,
                ReasonCode::InsufficientFunds4100,
                GuardOutcome::NotApplied
            )
        );
        // Insufficient funds with the customer not found carries the not-found reason.
        let missing = CustomerLookup::NotFound;
        assert_eq!(
            decision_for(380_001, Some(&room), missing, SummaryState::Existing(&row))?,
            (
                AuthStatus::Declined,
                ReasonCode::NotFound3100,
                GuardOutcome::NotApplied
            )
        );
        Ok(())
    }

    // audit: COPAUA0C-041
    #[test]
    fn the_summary_available_amount_drops_its_high_order_digit() -> TestResult {
        // 999999999.99 − (−0.01) = 1000000000.00, stored into S9(09)V99 as 0.00 (ARITH-034).
        let room = account(0, 1_000_000, 0)?;
        let row = summary(99_999_999_999, -1, 0)?;
        assert_eq!(
            decision_for(
                1,
                Some(&room),
                CustomerLookup::Found,
                SummaryState::Existing(&row)
            )?
            .1,
            ReasonCode::InsufficientFunds4100
        );
        Ok(())
    }

    // audit: COPAUA0C-042
    // audit: COPAUA0C-043
    // audit: COPAUA0C-044
    #[test]
    fn a_new_summary_decides_on_the_account_balance() -> TestResult {
        let funded = account(84_300, 500_000, 0)?;
        let row = summary_template(&xref()?);
        let found = CustomerLookup::Found;
        // 5000.00 − 843.00 = 4157.00; the guard's sum equals the limit, which approves.
        assert_eq!(
            decision_for(415_700, Some(&funded), found, SummaryState::Created(&row))?,
            (
                AuthStatus::Approved,
                ReasonCode::Approved0000,
                GuardOutcome::Passed
            )
        );
        assert_eq!(
            decision_for(415_701, Some(&funded), found, SummaryState::Created(&row))?.1,
            ReasonCode::InsufficientFunds4100
        );
        assert_eq!(
            decision_for(100, None, found, SummaryState::Created(&row))?,
            (
                AuthStatus::Declined,
                ReasonCode::NotFound3100,
                GuardOutcome::NotApplied
            )
        );
        Ok(())
    }

    // audit: COPAUA0C-046
    // audit: COPAUA0C-047
    // audit: COPAUA0C-048
    #[test]
    fn the_guard_declines_approvals_that_would_breach_the_limit() -> TestResult {
        // ARITH-045: balance 0.00, limit 2450.00, amount 100.00.
        let tight = account(0, 245_000, 0)?;
        let found = CustomerLookup::Found;
        let full = summary(99_999_999, 0, 240_000)?;
        assert_eq!(
            decision_for(10_000, Some(&tight), found, SummaryState::Existing(&full))?,
            (
                AuthStatus::Declined,
                ReasonCode::InsufficientFunds4100,
                GuardOutcome::DeclinedByCreditLimit
            )
        );
        let room = summary(99_999_999, 0, 230_000)?;
        assert_eq!(
            decision_for(10_000, Some(&tight), found, SummaryState::Existing(&room))?.2,
            GuardOutcome::Passed
        );
        let missing = CustomerLookup::NotFound;
        assert_eq!(
            decision_for(10_000, Some(&tight), missing, SummaryState::Existing(&full))?.1,
            ReasonCode::InsufficientFunds4100
        );
        assert_eq!(
            decision_for(10_000, None, found, SummaryState::Existing(&room))?,
            (
                AuthStatus::Declined,
                ReasonCode::NotFound3100,
                GuardOutcome::DeclinedWithoutAccount
            )
        );
        Ok(())
    }

    // audit: COPAUA0C-046
    // audit: COPAUA0C-049
    #[test]
    fn approvals_on_one_summary_stop_at_the_credit_limit() -> TestResult {
        // L − B = 2450.00 and A = 100.00, so exactly 24 approvals.
        let xref = xref()?;
        let funded = account(100_000, 345_000, 0)?;
        let mut row = summary_template(&xref);
        let mut created = true;
        let mut approvals = 0_u32;
        for _ in 0..30 {
            let state = if created {
                SummaryState::Created(&row)
            } else {
                SummaryState::Existing(&row)
            };
            let lookup = Lookup::CardFound(FoundContext {
                xref: &xref,
                account: Some(&funded),
                customer: CustomerLookup::Found,
                summary: state,
            });
            let decision = decide(&request(10_000)?, &lookup);
            if decision.status == AuthStatus::Approved {
                approvals += 1;
            }
            row = summary_update(state, &xref, Some(&funded), &decision)?;
            created = false;
        }
        assert_eq!(approvals, 24);
        assert_eq!(row.approved_auth_amt, money::<9>(240_000)?);
        assert_eq!(row.approved_auth_cnt.get(), 24);
        assert_eq!(row.declined_auth_cnt.get(), 6);
        Ok(())
    }

    // audit: COPAUA0C-045
    #[test]
    fn the_decline_reason_takes_the_first_condition_that_holds() {
        assert_eq!(
            decline_reason(true, DeclineFlag::InsufficientFund),
            ReasonCode::NotFound3100
        );
        assert_eq!(
            decline_reason(false, DeclineFlag::InsufficientFund),
            ReasonCode::InsufficientFunds4100
        );
        assert_eq!(
            decline_reason(false, DeclineFlag::Unset),
            ReasonCode::Other9000
        );
    }

    // audit: COPAUA0C-060
    // audit: COPAUA0C-061
    // audit: COPAUA0C-063
    // audit: COPAUA0C-064
    #[test]
    fn the_reply_writes_sixty_three_bytes_with_a_redacted_card() -> TestResult {
        let funded = account(0, 500_000, 0)?;
        let row = summary_template(&xref()?);
        let xref = xref()?;
        let lookup = Lookup::CardFound(FoundContext {
            xref: &xref,
            account: Some(&funded),
            customer: CustomerLookup::Found,
            summary: SummaryState::Created(&row),
        });
        let approved = reply_body(&decide(&request(10_000)?, &lookup).reply);
        assert_eq!(approved.len(), REPLY_WRITTEN_LEN);
        assert_eq!(
            approved,
            b"************5740,ORDER-A        ,143012,00,0000,        100.00,"
        );
        assert!(approved.starts_with(CARD_REDACTED));
        assert!(!contains(&approved, CARD.as_bytes()), "COPAUA0C-061");
        let declined = reply_body(&decide(&request(600_000)?, &lookup).reply);
        assert_eq!(
            declined,
            b"************5740,ORDER-A        ,143012,05,4100,          0.00,"
        );
        // A 15-digit transaction ID is withheld in the reply (COPAUA0C-062).
        let numeric_id = AuthRequestRecord {
            transaction_id: PicX::from_array(*b"123456789012345"),
            ..request(10_000)?
        };
        assert_eq!(
            reply_body(&decide(&numeric_id, &lookup).reply),
            b"************5740,***************,143012,00,0000,        100.00,"
        );
        Ok(())
    }

    // audit: COPAUA0C-062
    #[test]
    fn the_transaction_id_screen_withholds_runs_of_eight_non_spaces() {
        let passing: [&[u8; 15]; 4] = [
            b"ORDER-A        ",
            b"               ",
            b"1234567 1234567",
            b"A B C D E F G H",
        ];
        for id in passing {
            assert_eq!(&transaction_id_screen(&PicX::from_array(*id)), id);
        }
        let withheld: [&[u8; 15]; 4] = [
            b"123-45-6789    ",
            b"00123456789    ",
            b"123456789012345",
            b"PASSWORD       ",
        ];
        for id in withheld {
            assert_eq!(transaction_id_screen(&PicX::from_array(*id)), [b'*'; 15]);
        }
        // A run of exactly eight at each of its eight possible start positions.
        for start in 0..8 {
            let id = PicX::<15>::moved_from(format!("{}ABCDEFGH", " ".repeat(start)).as_bytes());
            assert_eq!(transaction_id_screen(&id), [b'*'; 15], "start {start}");
        }
    }

    // audit: COPAUA0C-064
    #[test]
    fn the_amount_screen_withholds_eight_consecutive_digits() -> TestResult {
        let cases: [(i64, &[u8; 14]); 5] = [
            (0, b"          0.00"),
            (10_000, b"        100.00"),
            (-525, b"-         5.25"),
            (999_999_999, b"    9999999.99"),
            (-999_999_999, b"-   9999999.99"),
        ];
        for (cents, edit) in cases {
            assert_eq!(&amount_screen(money(cents)?), edit);
        }
        for cents in [1_000_000_000, -1_000_000_000, 123_456_789_012] {
            assert_eq!(amount_screen(money(cents)?), [b'*'; 14]);
        }
        Ok(())
    }

    // audit: COPAUA0C-090
    // audit: COPAUA0C-092
    // audit: COPAUA0C-094
    #[test]
    fn an_approval_updates_the_counts_amounts_and_limits() -> TestResult {
        let xref = xref()?;
        let funded = account(0, 500_000, 150_000)?;
        let row = summary_template(&xref);
        assert_eq!(row.acct_id.get(), 50);
        assert_eq!(row.cust_id.get(), 50);
        let state = SummaryState::Created(&row);
        let lookup = Lookup::CardFound(FoundContext {
            xref: &xref,
            account: Some(&funded),
            customer: CustomerLookup::Found,
            summary: state,
        });
        let decision = decide(&request(10_000)?, &lookup);
        let updated = summary_update(state, &xref, Some(&funded), &decision)?;
        assert_eq!(updated.acct_id.get(), 50);
        assert_eq!(updated.cust_id.get(), 50);
        assert_eq!(updated.credit_limit, money::<9>(500_000)?);
        assert_eq!(updated.cash_limit, money::<9>(150_000)?);
        assert_eq!(updated.approved_auth_cnt.get(), 1);
        assert_eq!(updated.approved_auth_amt, money::<9>(10_000)?);
        assert_eq!(updated.credit_balance, money::<9>(10_000)?);
        assert!(updated.cash_balance.is_zero());
        assert_eq!(updated.declined_auth_cnt.get(), 0);
        // ARITH-040 and ARITH-043: the S9(09)V99 receivers drop the high-order digit.
        let wide = account(0, 123_456_789_012, 0)?;
        let near = summary(0, 99_999_995_000, 99_999_995_000)?;
        let updated = summary_update(SummaryState::Existing(&near), &xref, Some(&wide), &decision)?;
        assert_eq!(updated.credit_limit, money::<9>(23_456_789_012)?);
        assert_eq!(updated.approved_auth_amt, money::<9>(5_000)?);
        assert_eq!(updated.credit_balance, money::<9>(5_000)?);
        Ok(())
    }

    // audit: COPAUA0C-095
    #[test]
    fn a_decline_counts_and_leaves_the_approved_fields() -> TestResult {
        let xref = xref()?;
        let row = summary(500_000, 120_000, 120_000)?;
        let state = SummaryState::Existing(&row);
        let lookup = Lookup::CardFound(FoundContext {
            xref: &xref,
            account: None,
            customer: CustomerLookup::Found,
            summary: state,
        });
        let decision = decide(&request(900_000)?, &lookup);
        assert_eq!(decision.status, AuthStatus::Declined);
        // With no account row the limits are HALTED(X-COPAUA0C-H2) and the declined amount
        // HALTED(H2): neither is asserted.
        let updated = summary_update(state, &xref, None, &decision)?;
        assert_eq!(updated.declined_auth_cnt.get(), 1);
        assert_eq!(updated.approved_auth_cnt.get(), 0);
        assert_eq!(updated.approved_auth_amt, row.approved_auth_amt);
        assert_eq!(updated.credit_balance, row.credit_balance);
        assert_eq!(updated.cash_balance, row.cash_balance);
        Ok(())
    }

    #[test]
    fn a_count_at_its_four_digit_limit_is_an_error() -> TestResult {
        // ARITH-042: adding 1 to 9999 has no expected value (X-ARITH-6).
        let xref = xref()?;
        let row = PendingAuthSummaryRecord {
            approved_auth_cnt: Count::new(Count::MAX),
            declined_auth_cnt: Count::new(Count::MAX),
            ..summary(500_000, 0, 0)?
        };
        let state = SummaryState::Existing(&row);
        let funded = account(0, 500_000, 0)?;
        let lookup = Lookup::CardFound(FoundContext {
            xref: &xref,
            account: Some(&funded),
            customer: CustomerLookup::Found,
            summary: state,
        });
        let approved = decide(&request(100)?, &lookup);
        assert_eq!(
            summary_update(state, &xref, Some(&funded), &approved),
            Err(SummaryError::ApprovedCountAtLimit)
        );
        let declined = decide(&request(900_000)?, &lookup);
        let error = summary_update(state, &xref, Some(&funded), &declined);
        assert_eq!(error, Err(SummaryError::DeclinedCountAtLimit));
        assert!(error.is_err_and(|error| error.to_string().contains("9999")));
        Ok(())
    }

    /// Day 32 of 2025 at 13:45:07.123, the worked instant of COPAUA0C-098.
    fn instant() -> Result<NaiveDateTime, Box<dyn StdError>> {
        Ok(NaiveDate::from_ymd_opt(2025, 2, 1)
            .ok_or("date")?
            .and_hms_milli_opt(13, 45, 7, 123)
            .ok_or("time")?)
    }

    // audit: COPAUA0C-098
    // audit: COPAUA0C-100
    // audit: COPAUA0C-101
    #[test]
    fn the_detail_takes_inverted_keys_and_the_request_and_reply_fields() -> TestResult {
        let clock = FixedClock::new(instant()?);
        let request = request(10_000)?;
        let decision = decide(&request, &Lookup::CardNotInXref);
        let declined = detail_record(&request, &decision, &clock);
        assert_eq!(declined.auth_date_9c.get(), 74_967);
        assert_eq!(declined.auth_time_9c.get(), 865_492_876);
        assert_eq!(
            declined.match_status.as_bytes(),
            PendingAuthDetailRecord::MATCH_AUTH_DECLINED
        );
        let xref = xref()?;
        let funded = account(0, 500_000, 0)?;
        let row = summary_template(&xref);
        let lookup = Lookup::CardFound(FoundContext {
            xref: &xref,
            account: Some(&funded),
            customer: CustomerLookup::Found,
            summary: SummaryState::Created(&row),
        });
        let decision = decide(&request, &lookup);
        let detail = detail_record(&request, &decision, &clock);
        assert_eq!(
            detail.match_status.as_bytes(),
            PendingAuthDetailRecord::MATCH_PENDING
        );
        assert_eq!(detail.orig_date, request.auth_date);
        assert_eq!(detail.orig_time, request.auth_time);
        assert_eq!(detail.card_num, request.card_num);
        assert_eq!(detail.auth_type, request.auth_type);
        assert_eq!(detail.card_expiry_date, request.card_expiry_date);
        assert_eq!(detail.message_type, request.message_type);
        assert_eq!(detail.message_source, request.message_source);
        assert_eq!(detail.processing_code, request.processing_code);
        assert_eq!(detail.transaction_amt, request.transaction_amt);
        assert_eq!(
            detail.merchant_catagory_code,
            request.merchant_catagory_code
        );
        assert_eq!(detail.acqr_country_code, request.acqr_country_code);
        assert_eq!(detail.pos_entry_mode, request.pos_entry_mode);
        assert_eq!(detail.merchant_id, request.merchant_id);
        assert_eq!(detail.merchant_name, request.merchant_name);
        assert_eq!(detail.merchant_city, request.merchant_city);
        assert_eq!(detail.merchant_state, request.merchant_state);
        assert_eq!(detail.merchant_zip, request.merchant_zip);
        assert_eq!(detail.transaction_id, request.transaction_id);
        assert_eq!(detail.auth_id_code.as_bytes(), b"143012");
        assert_eq!(detail.auth_resp_code.as_bytes(), b"00");
        assert_eq!(detail.auth_resp_reason.as_bytes(), b"0000");
        assert_eq!(detail.approved_amt, request.transaction_amt);
        assert_eq!(detail.auth_fraud.as_bytes(), b" ");
        assert_eq!(detail.fraud_rpt_date.as_bytes(), b"        ");
        // The same instant gives the same key, the duplicate of COPAUA0C-099.
        let again = detail_record(&request, &decision, &clock);
        assert_eq!(
            (again.auth_date_9c, again.auth_time_9c),
            (detail.auth_date_9c, detail.auth_time_9c)
        );
        Ok(())
    }

    /// A log row's location, level byte, subsystem byte and unpadded message.
    type RowFields = ([u8; 4], u8, u8, String);

    /// The location, level, subsystem and message of `site`'s row, the fields every log row
    /// fixes.
    fn row_fields(site: LogSite<'_>) -> Result<RowFields, Box<dyn StdError>> {
        let row = error_log(site, &FixedClock::new(instant()?));
        assert_eq!(row.date.as_bytes(), b"250201");
        assert_eq!(row.time.as_bytes(), b"134507");
        assert_eq!(row.application.as_bytes(), b"CP00    ");
        assert_eq!(row.program.as_bytes(), b"COPAUA0C");
        assert_eq!(row.application.to_text(), TRANSACTION);
        assert_eq!(row.program.to_text(), PROGRAM);
        assert_eq!(row.encode().len(), 122);
        assert!(!contains(&row.encode(), CARD.as_bytes()), "COPAUA0C-133");
        let [level] = *row.level.as_bytes();
        let [subsystem] = *row.subsystem.as_bytes();
        Ok((
            *row.location.as_bytes(),
            level,
            subsystem,
            row.message.to_text(),
        ))
    }

    // audit: COPAUA0C-110
    // audit: COPAUA0C-114
    // audit: COPAUA0C-115
    // audit: COPAUA0C-116
    // audit: COPAUA0C-120
    #[test]
    fn warning_rows_and_the_detail_failure_carry_their_audited_keys() -> TestResult {
        let card = CardNumber::parse(CARD)?;
        let a001 = LogSite::A001 { card: &card };
        assert_eq!(
            row_fields(a001)?,
            (*b"A001", b'W', b'A', "CARD NOT FOUND IN XREF".to_owned())
        );
        let clock = FixedClock::new(instant()?);
        assert_eq!(
            error_log(a001, &clock).event_key.as_bytes(),
            b"************5740    "
        );
        let a002 = LogSite::A002 {
            acct_id: PicU::new(50)?,
        };
        assert_eq!(
            row_fields(a002)?,
            (*b"A002", b'W', b'A', "ACCT NOT FOUND IN XREF".to_owned())
        );
        assert_eq!(
            error_log(a002, &clock).event_key.as_bytes(),
            b"00000000050         "
        );
        let a003 = LogSite::A003 {
            cust_id: PicU::new(50)?,
        };
        assert_eq!(
            row_fields(a003)?,
            (*b"A003", b'W', b'A', "CUST NOT FOUND IN XREF".to_owned())
        );
        assert_eq!(
            error_log(a003, &clock).event_key.as_bytes(),
            b"000000050           "
        );
        let i004 = LogSite::I004 {
            card: &card,
            sqlstate: SqlState::parse("23505"),
        };
        assert_eq!(
            row_fields(i004)?,
            (*b"I004", b'C', b'I', "IMS INSERT DETL FAILED".to_owned())
        );
        let row = error_log(i004, &clock);
        assert_eq!(row.event_key.as_bytes(), b"************5740    ");
        assert_eq!(row.message.as_bytes().len(), 50);
        assert_eq!(row.level(), Some(ErrorLevel::Critical));
        assert_eq!(row.subsystem(), Some(ErrorSubsystem::Ims));
        Ok(())
    }

    // audit: COPAUA0C-117
    // audit: COPAUA0C-119
    // audit: COPAUA0C-122
    // audit: COPAUA0C-123
    // audit: COPAUA0C-124
    // audit: COPAUA0C-125
    // audit: COPAUA0C-126
    #[test]
    fn critical_rows_carry_their_location_level_subsystem_and_message() -> TestResult {
        let card = CardNumber::parse(CARD)?;
        let acct_id = PicU::new(50)?;
        let cust_id = PicU::new(50)?;
        let sqlstate = SqlState::parse("08006");
        let cases: [(LogSite<'_>, &[u8; 4], u8, &str); 7] = [
            (
                LogSite::M004 { card: &card },
                b"M004",
                b'M',
                "FAILED TO PUT ON REPLY MQ",
            ),
            (
                LogSite::I003 {
                    card: &card,
                    sqlstate,
                },
                b"I003",
                b'I',
                "IMS UPDATE SUMRY FAILED",
            ),
            (
                LogSite::C001 {
                    card: &card,
                    sqlstate,
                },
                b"C001",
                b'C',
                "FAILED TO READ XREF FILE",
            ),
            (
                LogSite::C002 { acct_id, sqlstate },
                b"C002",
                b'C',
                "FAILED TO READ ACCT FILE",
            ),
            (
                LogSite::C003 { cust_id, sqlstate },
                b"C003",
                b'C',
                "FAILED TO READ CUST FILE",
            ),
            (
                LogSite::I002 {
                    card: &card,
                    sqlstate,
                },
                b"I002",
                b'I',
                "IMS GET SUMMARY FAILED",
            ),
            (LogSite::I001, b"I001", b'I', "IMS SCHD FAILED"),
        ];
        for (site, location, subsystem, message) in cases {
            assert!(site.ends_task());
            assert_eq!(
                row_fields(site)?,
                (*location, b'C', subsystem, message.to_owned())
            );
        }
        for site in [
            LogSite::A001 { card: &card },
            LogSite::A002 { acct_id },
            LogSite::A003 { cust_id },
        ] {
            assert!(!site.ends_task());
        }
        Ok(())
    }

    // HALTED(H4), COPAUA0C-112, and HALTED(X-COPAUA0C-H1), COPAUA0C-111: the interim code fields
    // run here and are not asserted as parity.
    #[test]
    fn code_fields_hold_only_a_sqlstate_or_spaces() -> TestResult {
        assert!(SqlState::parse("23505").is_some());
        for text in ["2350", "235055", "ab123", "4000123412345740"] {
            assert_eq!(SqlState::parse(text), None);
        }
        let card = CardNumber::parse(CARD)?;
        let clock = FixedClock::new(instant()?);
        let row = error_log(
            LogSite::I004 {
                card: &card,
                sqlstate: SqlState::parse("23505"),
            },
            &clock,
        );
        assert!(
            row.code_1
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b' ')
        );
        let row = error_log(LogSite::I001, &clock);
        assert!(row.code_1.is_spaces() && row.code_2.is_spaces());
        let row = error_log(
            LogSite::C002 {
                acct_id: PicU::new(50)?,
                sqlstate: None,
            },
            &clock,
        );
        assert!(row.code_1.is_spaces());
        Ok(())
    }

    // audit: COPAUA0C-133
    #[test]
    fn no_debug_form_shows_the_full_card_number() -> TestResult {
        let card = CardNumber::parse(CARD)?;
        let request = request(10_000)?;
        let decision = decide(&request, &Lookup::CardNotInXref);
        let rendered = [
            format!("{:?}", LogSite::M004 { card: &card }),
            format!("{decision:?}"),
            format!("{:?}", parse_request(&body_with(9, "X"))),
            format!("{:?}", parse_request(&body_with(3, "4000123412345X40"))),
        ];
        for text in rendered {
            assert!(!text.contains(CARD), "COPAUA0C-133");
        }
        Ok(())
    }
}
