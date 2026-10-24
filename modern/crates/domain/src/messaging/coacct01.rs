//! CDRA, the account inquiry that COACCT01 serves over MQ (`COACCT01.cbl`; AAP 0.3.4.2): the
//! total request decoder, the branch of `4000-PROCESS-REQUEST-REPLY`, and the reply and error
//! bodies byte for byte as `modern/audit/messaging/COACCT01.md` states them.
//!
//! This module decodes, decides and formats; it reads no store and publishes nothing.
//! `messaging::workers::cdra` consumes the request, reads the account through `persistence`
//! (`AccountRepository::find_by_id`) when [`decide`] asks for it, publishes the body built here,
//! waits for the confirmation and acknowledges. Routing, properties and acknowledgement are the
//! worker's rows (COACCT01-023 to COACCT01-026, COACCT01-032 to COACCT01-034); this module
//! supplies the bytes they publish.
//!
//! | Item | COBOL | Rules |
//! |---|---|---|
//! | [`decode_request`], [`CdraRequest`], [`CdraRequestError`] | `REQUEST-MSG-COPY` | COACCT01-001, COACCT01-002 |
//! | [`undecoded_reply`] | none: interim replacement handling | COACCT01-003 |
//! | [`decide`], [`CdraAction`] | `IF WS-FUNC = 'INQA' AND WS-KEY > ZEROES` | COACCT01-004, COACCT01-005, COACCT01-022 |
//! | [`respond`], [`AccountLookup`], [`CdraResponse`] | `EVALUATE WS-RESP-CD` | COACCT01-006 to COACCT01-021, COACCT01-028 |
//! | [`account_response`] | `WS-ACCT-RESPONSE` | COACCT01-006 to COACCT01-020 |
//! | [`read_failure`] | `MQ-ERR-DISPLAY` after a failed `READ` | COACCT01-028 to COACCT01-031 |
//!
//! The request layout is `REQUEST-MSG-COPY`, not the README's message layouts (E02). The error
//! group is CODATE01's [`MqErrDisplay`], which COACCT01 declares identically
//! (`COACCT01.cbl:58-67`), and every body is padded through CODATE01's [`space_padded`] to
//! [`BUFFER_LEN`]. The paths the audit does not carry over (`CICS RETRIEVE`, `MQOPEN`, `MQGET`
//! and `MQCLOSE` errors with their artefacts, E17) have no code here. Every body is built from
//! its own request and its own read, so nothing carries from one message to the next
//! (COACCT01-027, as E14).
//!
//! # Redaction
//!
//! The not-found and bad-parameter replies carry eleven `*` where the COBOL echoes `WS-KEY`, and
//! a body that does not decode is answered with `*` in every key and function position: a
//! requester's digits can hold an SSN or a numeric password, which Cardholder Data Redaction
//! keeps out of every published body (COACCT01-021, COACCT01-022, COACCT01-035; refinement
//! override X-COACCT01-9, proposed to fold into E32). Those masked positions are replacement behaviour,
//! never a parity result. [`CdraRequest`], [`CdraAction`], [`CdraResponse`] and
//! [`CdraRequestError`] render no key digit and no body byte in `Debug` or `Display`.
//!
//! # Halts
//!
//! The error body's condition and reason codes are halt H4 (COACCT01-030, COACCT01-031). A body
//! shorter than [`REQUEST_LEN`] and a key that is not eleven ASCII digits have no single COBOL
//! result (halt candidates X-COACCT01-H2 and X-COACCT01-H1, COACCT01-003): they are typed
//! decoder errors, and their interim reply is no parity result.

use std::fmt;

use copybook::{PicDate, PicU, PicX};

use super::codate01::{BUFFER_LEN, MqErrDisplay, space_padded};
use crate::money::Money;
use crate::records::AccountRecord;

/// The reply queue, the literal of `MOVE 'CARD.DEMO.REPLY.ACCT' TO REPLY-QUEUE-NAME`
/// (`COACCT01.cbl:198`; COACCT01-023). The worker publishes every [`CdraResponse::Reply`] and
/// every [`CdraAction::Reply`] to the default exchange with this routing key, never to the
/// request's `reply_to`; the README's queue names are not used (E01).
pub const REPLY_QUEUE: &str = "CARD.DEMO.REPLY.ACCT";

/// The error queue, the literal of `MOVE 'CARD.DEMO.ERROR' TO ERROR-QUEUE-NAME`
/// (`COACCT01.cbl:294`; COACCT01-028, COACCT01-032). The worker publishes every
/// [`CdraResponse::Error`] here.
pub const ERROR_QUEUE: &str = "CARD.DEMO.ERROR";

/// Request bytes the program reads: `WS-FUNC PIC X(04)` and `WS-KEY PIC 9(11)` of
/// `REQUEST-MSG-COPY`; `WS-FILLER PIC X(985)` and every byte after offset 14 are never read
/// (`COACCT01.cbl:109-112`; COACCT01-001).
pub const REQUEST_LEN: usize = 15;

/// Bytes in `WS-ACCT-RESPONSE`, the found reply ahead of its padding: eleven labels and eleven
/// account fields (`COACCT01.cbl:130-169`; COACCT01-020).
pub const ACCT_RESPONSE_LEN: usize = 252;

/// Bytes of not-found reply text ahead of the padding: `INVALID REQUEST PARAMETERS `,
/// `ACCT ID : ` and the eleven key positions, 27 + 10 + 11 (COACCT01-021).
pub const NOT_FOUND_TEXT_LEN: usize = 48;

/// Bytes of bad-parameter reply text ahead of the padding: the not-found text, `FUNCTION : ` and
/// the four function positions, 48 + 11 + 4 (COACCT01-022).
pub const BAD_PARAMETER_TEXT_LEN: usize = 63;

/// `WS-FUNC PIC X(04)`, bytes 0-3 of the request (`COACCT01.cbl:110`).
type Function = PicX<4>;

/// `WS-KEY PIC 9(11)`, bytes 4-14 of the request (`COACCT01.cbl:111`), and equally
/// `WS-CARD-RID-ACCT-ID PIC 9(11)` and `ACCT-ID PIC 9(11)`, which it is compared with.
type Key = PicU<11>;

/// Bytes of `WS-FUNC`.
const FUNCTION_LEN: usize = Function::WIDTH;

/// Bytes of `WS-KEY`, eleven digits.
const KEY_LEN: usize = 11;

/// Digits after the implied point of every amount picture (`V99`).
const AMOUNT_SCALE: u32 = 2;

/// The function value that reads an account (`COACCT01.cbl:393`; COACCT01-004).
const INQUIRY: Function = PicX::from_array(*b"INQA");

/// The first `STRING` operand of the not-found and bad-parameter replies, one trailing space
/// (`COACCT01.cbl:429,449`).
const INVALID_PARAMETERS: &[u8; 27] = b"INVALID REQUEST PARAMETERS ";

/// The second `STRING` operand of both replies (`COACCT01.cbl:430,450`).
const ACCT_ID_LABEL: &[u8; 10] = b"ACCT ID : ";

/// The label the bad-parameter reply strings straight after the key, with no separator
/// (`COACCT01.cbl:451`).
const FUNCTION_LABEL: &[u8; 11] = b"FUNCTION : ";

/// The key positions of every not-found and bad-parameter reply (X-COACCT01-9).
const MASKED_KEY: [u8; KEY_LEN] = [b'*'; KEY_LEN];

/// The function positions of the reply to a body that does not decode (COACCT01-003).
const MASKED_FUNCTION: [u8; FUNCTION_LEN] = [b'*'; FUNCTION_LEN];

/// The 28-byte literal the read failure moves into `MQ-APPL-RETURN-MESSAGE PIC X(25)`
/// (`COACCT01.cbl:442-443`; E18).
const READ_FAILED: &[u8; 28] = b"ERROR WHILE READING ACCTFILE";

// The `WS-ACCT-RESPONSE` labels in declaration order, each a `PIC X(n)` whose `VALUE` is its
// literal (`COACCT01.cbl:132-168`; COACCT01-006 to COACCT01-016).
const ACCOUNT_ID_LABEL: &[u8; 13] = b"ACCOUNT ID : ";
const STATUS_LABEL: &[u8; 17] = b"ACCOUNT STATUS : ";
const BALANCE_LABEL: &[u8; 10] = b"BALANCE : ";
const CREDIT_LIMIT_LABEL: &[u8; 15] = b"CREDIT LIMIT : ";
const CASH_LIMIT_LABEL: &[u8; 13] = b"CASH LIMIT : ";
const OPEN_DATE_LABEL: &[u8; 12] = b"OPEN DATE : ";
const EXPIRY_DATE_LABEL: &[u8; 12] = b"EXPR DATE : ";
const REISSUE_DATE_LABEL: &[u8; 12] = b"REIS DATE : ";
const CYCLE_CREDIT_LABEL: &[u8; 13] = b"CREDIT BAL : ";
const CYCLE_DEBIT_LABEL: &[u8; 12] = b"DEBIT BAL : ";
const GROUP_ID_LABEL: &[u8; 11] = b"GROUP ID : ";

const _: () = assert!(
    FUNCTION_LEN + KEY_LEN == REQUEST_LEN && Key::MAX == 99_999_999_999,
    "the request is WS-FUNC PIC X(04) and WS-KEY PIC 9(11)"
);

// Each `PIC S9(10)V99` DISPLAY amount is 12 bytes: ten integer and two decimal digits, the last
// carrying the sign overpunch (PIC-005, PIC-006).
const _: () = assert!(
    ACCOUNT_ID_LABEL.len()
        + KEY_LEN
        + STATUS_LABEL.len()
        + PicX::<1>::WIDTH
        + BALANCE_LABEL.len()
        + 12
        + CREDIT_LIMIT_LABEL.len()
        + 12
        + CASH_LIMIT_LABEL.len()
        + 12
        + OPEN_DATE_LABEL.len()
        + PicDate::WIDTH
        + EXPIRY_DATE_LABEL.len()
        + PicDate::WIDTH
        + REISSUE_DATE_LABEL.len()
        + PicDate::WIDTH
        + CYCLE_CREDIT_LABEL.len()
        + 12
        + CYCLE_DEBIT_LABEL.len()
        + 12
        + GROUP_ID_LABEL.len()
        + PicX::<10>::WIDTH
        == ACCT_RESPONSE_LEN,
    "the WS-ACCT-RESPONSE labels and fields sum to 252 bytes"
);

const _: () = assert!(
    INVALID_PARAMETERS.len() + ACCT_ID_LABEL.len() + KEY_LEN == NOT_FOUND_TEXT_LEN
        && NOT_FOUND_TEXT_LEN + FUNCTION_LABEL.len() + FUNCTION_LEN == BAD_PARAMETER_TEXT_LEN,
    "the not-found text is 48 bytes and the bad-parameter text 63"
);

const _: () = assert!(
    ACCT_RESPONSE_LEN <= BUFFER_LEN && BAD_PARAMETER_TEXT_LEN <= BUFFER_LEN,
    "every reply text fits the 1,000-byte buffer"
);

/// A decoded CDRA request: the two fields of `REQUEST-MSG-COPY` the program reads
/// (`COACCT01.cbl:109-112`; COACCT01-001, COACCT01-002).
///
/// The function is bytes 0-3 exactly as received, never validated, trimmed or case-folded. The
/// key is bytes 4-14, eleven ASCII digits, as a `PIC 9(11)` value with its leading zeros (PIC-003).
/// `Debug` shows the function and hides the key's digits, which a requester can fill with an
/// SSN or a numeric password (COACCT01-035).
#[derive(Clone, PartialEq, Eq)]
pub struct CdraRequest {
    function: Function,
    key: Key,
}

impl CdraRequest {
    /// `WS-FUNC`, bytes 0-3 as received (COACCT01-002).
    #[must_use]
    pub const fn function(&self) -> &PicX<4> {
        &self.function
    }

    /// `WS-KEY`, bytes 4-14 as an eleven-digit value (COACCT01-002).
    #[must_use]
    pub const fn key(&self) -> PicU<11> {
        self.key
    }
}

impl fmt::Debug for CdraRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CdraRequest")
            .field("function", &self.function)
            .field("key", &format_args!("<{KEY_LEN} digits>"))
            .finish()
    }
}

/// Why a CDRA body does not decode (COACCT01-002). Neither error is a reply: the worker answers
/// both with [`undecoded_reply`] (COACCT01-003).
///
/// Each variant carries lengths and offsets only, never a body byte, so neither `Debug` nor
/// `Display` can echo requester text (COACCT01-035).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CdraRequestError {
    /// The body is shorter than [`REQUEST_LEN`]; this check comes first. The COBOL takes the
    /// missing bytes from the unassigned residue of `MQ-BUFFER PIC X(1000)`, which has no
    /// `VALUE` clause, so they have no single value.
    // HALTED(X-COACCT01-H2): no parity result exists for a body shorter than 15 bytes.
    ShortBody {
        /// The body's length in bytes, below [`REQUEST_LEN`].
        len: usize,
    },
    /// A byte of `WS-KEY` is not an ASCII digit: a space, a sign overpunch or any other byte.
    /// `WS-KEY > ZEROES` over invalid numeric DISPLAY data has no single COBOL result.
    // HALTED(X-COACCT01-H1): no parity result exists for a key that is not eleven digits.
    NonDigitKey {
        /// Body offset of the first such byte, from 4 to 14.
        offset: usize,
    },
}

impl fmt::Display for CdraRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShortBody { len } => write!(
                f,
                "CDRA request body is {len} bytes, shorter than the {REQUEST_LEN} bytes of \
                 WS-FUNC and WS-KEY"
            ),
            Self::NonDigitKey { offset } => write!(
                f,
                "CDRA request byte at offset {offset} is not an ASCII digit of WS-KEY PIC 9(11)"
            ),
        }
    }
}

impl std::error::Error for CdraRequestError {}

/// Decodes a CDRA request body of any length (COACCT01-001, COACCT01-002).
///
/// Only bytes 0-14 are read: bytes 0-3 are `WS-FUNC`, kept as received, and bytes 4-14 are
/// `WS-KEY`. Every later byte, `WS-FILLER` and anything beyond the 1,000-byte buffer included,
/// is ignored, so no byte after offset 14 affects any outcome. The decoder is total: for every
/// input it returns a request or a typed error and never panics (Total Parsers Over Untrusted
/// Input).
///
/// # Errors
///
/// - [`CdraRequestError::ShortBody`] when `body` is shorter than [`REQUEST_LEN`], checked first;
/// - [`CdraRequestError::NonDigitKey`] when a byte of offsets 4-14 is not an ASCII digit.
///
/// Neither input is normalised or defaulted (R7); the worker answers both with
/// [`undecoded_reply`] (COACCT01-003).
pub fn decode_request(body: &[u8]) -> Result<CdraRequest, CdraRequestError> {
    let short = CdraRequestError::ShortBody { len: body.len() };
    let (function, rest) = body.split_first_chunk::<FUNCTION_LEN>().ok_or(short)?;
    let (key, _filler) = rest.split_first_chunk::<KEY_LEN>().ok_or(short)?;
    Ok(CdraRequest {
        function: PicX::from_array(*function),
        key: decode_key(key)?,
    })
}

/// `WS-KEY` from its eleven bytes, digits only (PIC-004).
fn decode_key(bytes: &[u8; KEY_LEN]) -> Result<Key, CdraRequestError> {
    // With exactly eleven bytes, a non-digit byte is the only input `PicU::decode` rejects, so
    // the error names the first byte that is not an ASCII digit.
    Key::decode(bytes).map_err(|_| CdraRequestError::NonDigitKey {
        offset: bytes
            .iter()
            .position(|byte| !byte.is_ascii_digit())
            .map_or(FUNCTION_LEN, |at| FUNCTION_LEN.saturating_add(at)),
    })
}

/// The reply to a body that [`decode_request`] rejects (COACCT01-003): the bad-parameter layout
/// of COACCT01-022 with `*` in all eleven key and all four function positions, then spaces to
/// [`BUFFER_LEN`].
///
/// | Offset | Length | Bytes |
/// |---|---|---|
/// | 0 | 27 | `INVALID REQUEST PARAMETERS ` |
/// | 27 | 10 | `ACCT ID : ` |
/// | 37 | 11 | eleven `*` |
/// | 48 | 11 | `FUNCTION : ` |
/// | 59 | 4 | four `*` |
/// | 63 | 937 | spaces |
///
/// No request byte is echoed: the bytes of a body that does not decode are arbitrary requester
/// text that can carry a password or an SSN (Cardholder Data Redaction, X-COACCT01-9). The
/// worker publishes it like any reply, to [`REPLY_QUEUE`] with the request's IDs.
// HALTED(X-COACCT01-H1): interim handling of a non-digit key, never a parity result.
// HALTED(X-COACCT01-H2): interim handling of a body shorter than 15 bytes, never a parity result.
#[must_use]
pub fn undecoded_reply() -> [u8; BUFFER_LEN] {
    bad_parameter_reply(MASKED_FUNCTION)
}

/// What `4000-PROCESS-REQUEST-REPLY` does with a decoded request (`COACCT01.cbl:393-457`).
///
/// `Debug` hides the key's digits and shows a reply by its length only (COACCT01-035).
#[derive(Clone, PartialEq, Eq)]
pub enum CdraAction {
    /// Read the account whose `ACCT-ID` equals the key, exactly one equal-key read: `MOVE WS-KEY
    /// TO WS-CARD-RID-ACCT-ID`, between identical `PIC 9(11)` fields, then `EXEC CICS READ
    /// DATASET('ACCTDAT ')` (COACCT01-004). The worker calls
    /// `AccountRepository::find_by_id` with this key and passes the outcome to [`respond`] as an
    /// [`AccountLookup`].
    ReadAccount(PicU<11>),
    /// Reply without a read: the bad-parameter body of the `ELSE` branch, [`BUFFER_LEN`] bytes
    /// for [`REPLY_QUEUE`] (COACCT01-005, COACCT01-022). Boxed, so the action stays small.
    Reply(Box<[u8; BUFFER_LEN]>),
}

impl fmt::Debug for CdraAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadAccount(_) => write!(f, "ReadAccount(<{KEY_LEN} digits>)"),
            Self::Reply(body) => write!(f, "Reply(<{} bytes>)", body.len()),
        }
    }
}

/// The branch of `IF WS-FUNC = 'INQA' AND WS-KEY > ZEROES` (`COACCT01.cbl:393`).
///
/// - The function equal to `INQA` byte for byte (upper case, exactly four bytes, no padding or
///   case folding) and a key above zero: [`CdraAction::ReadAccount`] with the key
///   (COACCT01-004).
/// - Any other function (`inqa`, `INQ ` and the README's `ACCT` included), a key of eleven
///   zeros, or both: [`CdraAction::Reply`] with the bad-parameter body and no read
///   (COACCT01-005, COACCT01-022).
///
/// The bad-parameter body, `STRING 'INVALID REQUEST PARAMETERS ' 'ACCT ID : ' WS-KEY
/// 'FUNCTION : ' WS-FUNC` into a cleared `REPLY-MESSAGE` (`COACCT01.cbl:391,448-456`):
///
/// | Offset | Length | Bytes |
/// |---|---|---|
/// | 0 | 27 | `INVALID REQUEST PARAMETERS ` |
/// | 27 | 10 | `ACCT ID : ` |
/// | 37 | 11 | eleven `*`, whatever key the request sent |
/// | 48 | 11 | `FUNCTION : `, with no separator after the key |
/// | 59 | 4 | the four function bytes as received |
/// | 63 | 937 | spaces |
///
/// The COBOL echoes `WS-KEY` at offsets 37-47; Cardholder Data Redaction replaces it with `*`
/// (X-COACCT01-9), so only offsets 0-36 and 48-999 are parity. Four function bytes hold no
/// card number, SSN or eight-byte password, so they are the only request bytes a reply carries
/// (COACCT01-035).
#[must_use]
pub fn decide(request: &CdraRequest) -> CdraAction {
    if request.function == INQUIRY && request.key.get() > 0 {
        CdraAction::ReadAccount(request.key)
    } else {
        CdraAction::Reply(Box::new(bad_parameter_reply(*request.function.as_bytes())))
    }
}

/// The outcome of the account read of [`CdraAction::ReadAccount`]: `EVALUATE WS-RESP-CD`
/// (`COACCT01.cbl:406-447`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountLookup<'a> {
    /// `DFHRESP(NORMAL)`: `find_by_id` returned the row (COACCT01-006).
    Found(&'a AccountRecord),
    /// `DFHRESP(NOTFND)`: `find_by_id` returned no row (COACCT01-021).
    NotFound,
    /// `WHEN OTHER`: `find_by_id` failed, whatever the store error (COACCT01-028).
    ReadFailed,
}

/// A body ready to publish after the account read: `Reply` to [`REPLY_QUEUE`], `Error` to
/// [`ERROR_QUEUE`], each with the request's `message_id` and `correlation_id` copied unchanged,
/// `delivery_mode` 2 and no `expiration`, all of which the worker sets (COACCT01-024,
/// COACCT01-025, COACCT01-032).
///
/// `Debug` shows a body by its length only, so the found reply's account fields never reach a
/// log through it.
#[derive(Clone, PartialEq, Eq)]
pub enum CdraResponse {
    /// A reply of [`BUFFER_LEN`] bytes (COACCT01-020, COACCT01-021).
    Reply([u8; BUFFER_LEN]),
    /// An error message of [`BUFFER_LEN`] bytes; no reply is sent (COACCT01-028, COACCT01-029).
    Error([u8; BUFFER_LEN]),
}

impl fmt::Debug for CdraResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, body) = match self {
            Self::Reply(body) => ("Reply", body),
            Self::Error(body) => ("Error", body),
        };
        write!(f, "{name}(<{} bytes>)", body.len())
    }
}

/// The body `4000-PROCESS-REQUEST-REPLY` publishes after the account read
/// (`COACCT01.cbl:406-447`):
///
/// - [`AccountLookup::Found`]: [`CdraResponse::Reply`] with the [`account_response`] bytes
///   followed by 748 spaces, `MOVE WS-ACCT-RESPONSE TO REPLY-MESSAGE` into the cleared 1,000-byte
///   area (COACCT01-006 to COACCT01-020).
/// - [`AccountLookup::NotFound`]: [`CdraResponse::Reply`] with `INVALID REQUEST PARAMETERS `
///   (offset 0, 27 bytes), `ACCT ID : ` (27, 10) and eleven `*` (37, 11), then 952 spaces. The
///   COBOL strings `WS-KEY` into offsets 37-47; Cardholder Data Redaction masks them
///   (COACCT01-021, X-COACCT01-9), so the reply depends on nothing in the request.
/// - [`AccountLookup::ReadFailed`]: [`CdraResponse::Error`] with the [`read_failure`] error body
///   for `request_queue`, the configured request queue being consumed. The program then
///   terminates without a reply (`PERFORM 9000-ERROR`, `PERFORM 8000-TERMINATION`); the worker
///   publishes this body to [`ERROR_QUEUE`] and acknowledges the request after its confirmation
///   (COACCT01-028, COACCT01-029).
#[must_use]
pub fn respond(lookup: AccountLookup<'_>, request_queue: &str) -> CdraResponse {
    match lookup {
        AccountLookup::Found(account) => {
            CdraResponse::Reply(space_padded(&account_response(account)))
        }
        AccountLookup::NotFound => CdraResponse::Reply(space_padded(&not_found_text())),
        AccountLookup::ReadFailed => CdraResponse::Error(read_failure(request_queue).error_body()),
    }
}

/// `WS-ACCT-RESPONSE` filled from `account` by the moves of `COACCT01.cbl:408-425`, each label
/// followed by its field (`COACCT01.cbl:130-169`):
///
/// | Offset | Length | Bytes | Rule |
/// |---|---|---|---|
/// | 0 | 13 | `ACCOUNT ID : ` | COACCT01-006 |
/// | 13 | 11 | `ACCT-ID`, eleven digits with leading zeros | COACCT01-006 |
/// | 24 | 17 | `ACCOUNT STATUS : ` | COACCT01-007 |
/// | 41 | 1 | `ACCT-ACTIVE-STATUS` as stored | COACCT01-007 |
/// | 42 | 10 | `BALANCE : ` | COACCT01-008 |
/// | 52 | 12 | `ACCT-CURR-BAL` | COACCT01-008 |
/// | 64 | 15 | `CREDIT LIMIT : ` | COACCT01-009 |
/// | 79 | 12 | `ACCT-CREDIT-LIMIT` | COACCT01-009 |
/// | 91 | 13 | `CASH LIMIT : ` | COACCT01-010 |
/// | 104 | 12 | `ACCT-CASH-CREDIT-LIMIT` | COACCT01-010 |
/// | 116 | 12 | `OPEN DATE : ` | COACCT01-011 |
/// | 128 | 10 | `ACCT-OPEN-DATE` | COACCT01-011 |
/// | 138 | 12 | `EXPR DATE : ` | COACCT01-012 |
/// | 150 | 10 | `ACCT-EXPIRAION-DATE` | COACCT01-012 |
/// | 160 | 12 | `REIS DATE : ` | COACCT01-013 |
/// | 172 | 10 | `ACCT-REISSUE-DATE` | COACCT01-013 |
/// | 182 | 13 | `CREDIT BAL : ` | COACCT01-014 |
/// | 195 | 12 | `ACCT-CURR-CYC-CREDIT` | COACCT01-014 |
/// | 207 | 12 | `DEBIT BAL : ` | COACCT01-015 |
/// | 219 | 12 | `ACCT-CURR-CYC-DEBIT` | COACCT01-015 |
/// | 231 | 11 | `GROUP ID : ` | COACCT01-016 |
/// | 242 | 10 | `ACCT-GROUP-ID` as stored, padding kept | COACCT01-016 |
///
/// Each amount is `PIC S9(10)V99` DISPLAY: twelve digits with no point and no sign byte, the
/// last carrying a trailing overpunch, `{` or `A`–`I` for zero and positive values and `}` or
/// `J`–`R` for negative ones. Source and receiver share the picture, so nothing is truncated or
/// rounded, and a zero always renders with `{` (COACCT01-017). Each date is the typed
/// [`PicDate`] rendered as `YYYY-MM-DD`, or ten spaces when it is blank; only this byte
/// rendering is text (COACCT01-018). `ACCT-ADDR-ZIP` and `FILLER` are never moved into the
/// response, so no byte comes from either (COACCT01-019). The account record holds no card
/// number, SSN, CVV or password (COACCT01-035).
#[must_use]
pub fn account_response(account: &AccountRecord) -> [u8; ACCT_RESPONSE_LEN] {
    let mut text = Vec::with_capacity(ACCT_RESPONSE_LEN);
    text.extend_from_slice(ACCOUNT_ID_LABEL);
    account.id.encode_into(&mut text);
    text.extend_from_slice(STATUS_LABEL);
    account.active_status.encode_into(&mut text);
    push_amount(&mut text, BALANCE_LABEL, account.curr_bal);
    push_amount(&mut text, CREDIT_LIMIT_LABEL, account.credit_limit);
    push_amount(&mut text, CASH_LIMIT_LABEL, account.cash_credit_limit);
    push_date(&mut text, OPEN_DATE_LABEL, account.open_date);
    push_date(&mut text, EXPIRY_DATE_LABEL, account.expiraion_date);
    push_date(&mut text, REISSUE_DATE_LABEL, account.reissue_date);
    push_amount(&mut text, CYCLE_CREDIT_LABEL, account.curr_cyc_credit);
    push_amount(&mut text, CYCLE_DEBIT_LABEL, account.curr_cyc_debit);
    text.extend_from_slice(GROUP_ID_LABEL);
    account.group_id.encode_into(&mut text);
    // The labels and field widths sum to the response's length (checked at compile time above),
    // so this group MOVE copies every byte and neither pads nor cuts.
    *PicX::<ACCT_RESPONSE_LEN>::moved_from(&text).as_bytes()
}

/// The `MQ-ERR-DISPLAY` of a failed account read, `WHEN OTHER` of `EVALUATE WS-RESP-CD`
/// (`COACCT01.cbl:437-445`; COACCT01-028, COACCT01-029): [`MqErrDisplay::initialized`], then
///
/// - `MQ-APPL-RETURN-MESSAGE` ← `ERROR WHILE READING ACCTFILE`, a 28-byte literal moved into
///   `PIC X(25)`, so `ERROR WHILE READING ACCTF` with no padding (E18);
/// - `MQ-APPL-QUEUE-NAME` ← `INPUT-QUEUE-NAME`, the configured request queue, `request_queue`,
///   space-padded to 48. The worker refuses a configured name longer than 48 bytes or holding a
///   byte outside printable ASCII at start (X-COACCT01-8), so the name is never cut in service.
///
/// `MQ-ERROR-PARA` stays spaces. The condition and reason codes receive `WS-RESP-CD` and
/// `WS-REAS-CD` in the COBOL, CICS `RESP` and `RESP2` values that `PostgreSQL` has no
/// counterpart for, so they keep the initialised zeros and are no parity result (COACCT01-030,
/// COACCT01-031). [`MqErrDisplay::error_body`] gives the 1,000 bytes [`respond`] returns.
// HALTED(H4): the condition and reason code fields carry no parity value.
#[must_use]
pub fn read_failure(request_queue: &str) -> MqErrDisplay {
    MqErrDisplay {
        // TRUNCATION: the COBOL alphanumeric MOVE of the 28-byte literal into `PIC X(25)` drops
        // its last three bytes, `ILE`; reproduced as audited (E18).
        return_message: PicX::moved_from(READ_FAILED),
        // TRUNCATION: a name beyond `PIC X(48)` would lose its tail as the MOVE drops it; the
        // worker's start-up check keeps every configured name within 48 bytes (X-COACCT01-8).
        queue_name: PicX::moved_from(request_queue.as_bytes()),
        ..MqErrDisplay::initialized()
    }
}

/// `INVALID REQUEST PARAMETERS `, `ACCT ID : ` and the masked key: the 48 bytes of the not-found
/// reply and the head of every bad-parameter reply (COACCT01-021, COACCT01-022).
fn not_found_text() -> Vec<u8> {
    let mut text = Vec::with_capacity(BAD_PARAMETER_TEXT_LEN);
    text.extend_from_slice(INVALID_PARAMETERS);
    text.extend_from_slice(ACCT_ID_LABEL);
    text.extend_from_slice(&MASKED_KEY);
    text
}

/// The bad-parameter reply with `function` in its four function positions, padded to
/// [`BUFFER_LEN`] (COACCT01-003, COACCT01-022).
fn bad_parameter_reply(function: [u8; FUNCTION_LEN]) -> [u8; BUFFER_LEN] {
    let mut text = not_found_text();
    text.extend_from_slice(FUNCTION_LABEL);
    text.extend_from_slice(&function);
    space_padded(&text)
}

/// Appends `label` and then `amount` as a signed DISPLAY `PIC S9(N)V99` field, `N + 2` bytes with
/// a trailing overpunch; the picture's width is the amount's type (COACCT01-017).
fn push_amount<const N: u32>(text: &mut Vec<u8>, label: &[u8], amount: Money<N>) {
    text.extend_from_slice(label);
    copybook::zoned::encode_signed(amount.as_decimal(), N, AMOUNT_SCALE, text);
}

/// Appends `label` and then `date` as its ten `PIC X(10)` bytes (COACCT01-018).
fn push_date(text: &mut Vec<u8>, label: &[u8], date: PicDate) {
    text.extend_from_slice(label);
    date.encode_into(text);
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{PicDate, PicU, PicX};

    use super::{
        ACCT_RESPONSE_LEN, AccountLookup, BAD_PARAMETER_TEXT_LEN, BUFFER_LEN, CdraAction,
        CdraRequestError, CdraResponse, ERROR_QUEUE, NOT_FOUND_TEXT_LEN, REPLY_QUEUE, REQUEST_LEN,
        account_response, decide, decode_request, read_failure, respond, undecoded_reply,
    };
    use crate::messaging::codate01::MQ_ERR_DISPLAY_LEN;
    use crate::money::{Decimal, Money};
    use crate::records::AccountRecord;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The configured request queue the error body names, the `.env.example` default.
    const REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

    /// The `len` bytes of `bytes` from `start`.
    fn segment(bytes: &[u8], start: usize, len: usize) -> Result<&[u8], &'static str> {
        start
            .checked_add(len)
            .and_then(|end| bytes.get(start..end))
            .ok_or("segment out of range")
    }

    fn all_spaces(bytes: &[u8]) -> bool {
        bytes.iter().all(|&byte| byte == b' ')
    }

    /// `text` followed by spaces to `width` bytes.
    fn padded(text: &[u8], width: usize) -> Vec<u8> {
        let mut field = text.to_vec();
        field.resize(width, b' ');
        field
    }

    /// Whether `bytes` has the layout `pattern`, where `9` stands for any ASCII digit and every
    /// other byte for itself.
    fn has_format(bytes: &[u8], pattern: &[u8]) -> bool {
        bytes.len() == pattern.len()
            && bytes.iter().zip(pattern).all(|(&byte, &shape)| {
                if shape == b'9' {
                    byte.is_ascii_digit()
                } else {
                    byte == shape
                }
            })
    }

    /// A request body: `function`, `key`, then `filler` bytes of `WS-FILLER`.
    fn body(function: [u8; 4], key: &[u8; 11], filler: &[u8]) -> Vec<u8> {
        let mut bytes = function.to_vec();
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(filler);
        bytes
    }

    /// `cents` hundredths as a `PIC S9(10)V99` amount.
    fn money(cents: i64) -> Result<Money<10>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    /// The calendar date `year`-`month`-`day` as a `PIC X(10)` date.
    fn date(year: i32, month: u32, day: u32) -> Result<PicDate, Box<dyn StdError>> {
        let value = NaiveDate::from_ymd_opt(year, month, day).ok_or("not a calendar date")?;
        Ok(PicDate::new(Some(value))?)
    }

    /// A synthetic account, built here rather than quoted from a fixture, with a negative
    /// balance, a blank reissue date and a non-blank `ACCT-ADDR-ZIP` and `FILLER`, which the
    /// reply must not carry.
    fn account() -> Result<AccountRecord, Box<dyn StdError>> {
        Ok(AccountRecord {
            id: PicU::new(42_000_000_017)?,
            active_status: PicX::from_text("Y")?,
            curr_bal: money(-123_456)?,
            credit_limit: money(2_020_000)?,
            cash_credit_limit: money(1_020_001)?,
            open_date: date(2014, 11, 20)?,
            expiraion_date: date(2025, 5, 20)?,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: money(-9)?,
            addr_zip: PicX::from_text("ZIPZIPZIP1")?,
            group_id: PicX::from_text("GRP01")?,
            filler: PicX::from_array([b'#'; 178]),
        })
    }

    /// The 1,000-byte body of a reply; an error body fails the test.
    fn reply_of(response: &CdraResponse) -> Result<[u8; BUFFER_LEN], &'static str> {
        match response {
            CdraResponse::Reply(body) => Ok(*body),
            CdraResponse::Error(_) => Err("expected a reply, got an error body"),
        }
    }

    // audit: COACCT01-001
    // audit: COACCT01-023
    // audit: COACCT01-032
    #[test]
    fn queues_and_lengths_are_the_cobol_values() {
        assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.ACCT");
        assert_eq!(ERROR_QUEUE, "CARD.DEMO.ERROR");
        assert_eq!(REQUEST_LEN, 15);
        assert_eq!(ACCT_RESPONSE_LEN, 252);
        assert_eq!(NOT_FOUND_TEXT_LEN, 48);
        assert_eq!(BAD_PARAMETER_TEXT_LEN, 63);
    }

    // audit: COACCT01-001
    // audit: COACCT01-002
    #[test]
    fn decodes_the_function_and_key_and_ignores_the_filler() -> TestResult {
        let exact = decode_request(&body(*b"INQA", b"00000000001", b""))?;
        assert_eq!(exact.function().as_bytes(), b"INQA");
        assert_eq!(exact.key(), PicU::new(1)?);
        let mut full = body(*b"INQA", b"00000000001", &[b'Z'; 985]);
        assert_eq!(decode_request(&full)?, exact);
        // Filler bytes of any value, and bytes beyond 1,000, change nothing.
        full.extend_from_slice(&[0xFF; 200]);
        assert_eq!(decode_request(&full)?, exact);
        let raw = decode_request(&body([0x00, 0xFF, b'q', b' '], b"99999999999", b""))?;
        assert_eq!(raw.function().as_bytes(), &[0x00, 0xFF, b'q', b' ']);
        assert_eq!(raw.key().get(), 99_999_999_999);
        Ok(())
    }

    // audit: COACCT01-002
    #[test]
    fn short_bodies_are_typed_errors_checked_first() {
        for len in [0, 1, 4, 14] {
            let short = vec![b'x'; len];
            assert_eq!(
                decode_request(&short),
                Err(CdraRequestError::ShortBody { len })
            );
        }
        assert_eq!(
            decode_request(b"INQA0000000001"),
            Err(CdraRequestError::ShortBody { len: 14 })
        );
    }

    // audit: COACCT01-002
    #[test]
    fn a_key_byte_that_is_not_a_digit_is_a_typed_error() {
        let cases: [(&[u8; 11], usize); 4] = [
            (b"     000001", 4),
            (b"00000 00001", 9),
            (b"0000000000{", 14),
            (b"0000000000\xC1", 14),
        ];
        for (key, offset) in cases {
            assert_eq!(
                decode_request(&body(*b"INQA", key, b"")),
                Err(CdraRequestError::NonDigitKey { offset })
            );
        }
    }

    // audit: COACCT01-004
    #[test]
    fn inqa_with_a_key_above_zero_reads_that_account() -> TestResult {
        let request = decode_request(&body(*b"INQA", b"00000000001", b""))?;
        assert_eq!(decide(&request), CdraAction::ReadAccount(PicU::new(1)?));
        let widest = decode_request(&body(*b"INQA", b"99999999999", b""))?;
        assert_eq!(
            decide(&widest),
            CdraAction::ReadAccount(PicU::new(99_999_999_999)?)
        );
        Ok(())
    }

    /// The bad-parameter reply that `decide` gives for `function` and `key`.
    fn bad_parameter(function: [u8; 4], key: &[u8; 11]) -> Result<Vec<u8>, Box<dyn StdError>> {
        match decide(&decode_request(&body(function, key, b""))?) {
            CdraAction::Reply(reply) => Ok(reply.to_vec()),
            CdraAction::ReadAccount(_) => Err("expected the bad-parameter reply".into()),
        }
    }

    // audit: COACCT01-005
    // audit: COACCT01-022
    #[test]
    fn other_functions_and_a_zero_key_get_the_bad_parameter_reply() -> TestResult {
        for (function, key) in [
            (b"INQA", b"00000000000"),
            (b"INQB", b"00000000042"),
            (b"inqa", b"00000000042"),
            (b"ACCT", b"00000000000"),
        ] {
            let reply = bad_parameter(*function, key)?;
            assert_eq!(reply.len(), BUFFER_LEN);
            assert_eq!(
                segment(&reply, 0, 27)?,
                b"INVALID REQUEST PARAMETERS ".as_slice()
            );
            assert_eq!(segment(&reply, 27, 10)?, b"ACCT ID : ".as_slice());
            assert_eq!(segment(&reply, 37, 11)?, [b'*'; 11].as_slice());
            assert_eq!(segment(&reply, 48, 11)?, b"FUNCTION : ".as_slice());
            assert_eq!(segment(&reply, 59, 4)?, function.as_slice());
            assert!(all_spaces(segment(&reply, BAD_PARAMETER_TEXT_LEN, 937)?));
        }
        Ok(())
    }

    // audit: COACCT01-022
    // audit: COACCT01-035
    #[test]
    fn the_bad_parameter_reply_never_echoes_the_key() -> TestResult {
        // `NOPE` and `00` followed by nine digits: an SSN-shaped run inside a decodable key.
        let reply = bad_parameter(*b"NOPE", b"00123456789")?;
        assert!(!reply.windows(9).any(|run| run == b"123456789"));
        assert_eq!(segment(&reply, 37, 11)?, [b'*'; 11].as_slice());
        Ok(())
    }

    // audit: COACCT01-003
    // HALTED(X-COACCT01-H1), HALTED(X-COACCT01-H2): checks the interim body only, never parity.
    #[test]
    fn an_undecoded_body_gets_the_fully_masked_reply() -> TestResult {
        let reply = undecoded_reply();
        assert_eq!(reply.len(), BUFFER_LEN);
        assert_eq!(
            segment(&reply, 0, 37)?,
            b"INVALID REQUEST PARAMETERS ACCT ID : ".as_slice()
        );
        assert_eq!(segment(&reply, 37, 11)?, [b'*'; 11].as_slice());
        assert_eq!(segment(&reply, 48, 11)?, b"FUNCTION : ".as_slice());
        assert_eq!(segment(&reply, 59, 4)?, [b'*'; 4].as_slice());
        assert!(all_spaces(segment(&reply, BAD_PARAMETER_TEXT_LEN, 937)?));
        Ok(())
    }

    // audit: COACCT01-006
    // audit: COACCT01-007
    // audit: COACCT01-008
    // audit: COACCT01-009
    // audit: COACCT01-010
    // audit: COACCT01-011
    // audit: COACCT01-012
    // audit: COACCT01-013
    // audit: COACCT01-014
    // audit: COACCT01-015
    // audit: COACCT01-016
    #[test]
    fn the_found_reply_lays_out_labels_and_fields() -> TestResult {
        let account = account()?;
        let reply = reply_of(&respond(AccountLookup::Found(&account), REQUEST_QUEUE))?;
        let expected: [(usize, &[u8]); 22] = [
            (0, b"ACCOUNT ID : "),
            (13, b"42000000017"),
            (24, b"ACCOUNT STATUS : "),
            (41, b"Y"),
            (42, b"BALANCE : "),
            (52, b"00000012345O"),
            (64, b"CREDIT LIMIT : "),
            (79, b"00000202000{"),
            (91, b"CASH LIMIT : "),
            (104, b"00000102000A"),
            (116, b"OPEN DATE : "),
            (128, b"2014-11-20"),
            (138, b"EXPR DATE : "),
            (150, b"2025-05-20"),
            (160, b"REIS DATE : "),
            (172, b"          "),
            (182, b"CREDIT BAL : "),
            (195, b"00000000000{"),
            (207, b"DEBIT BAL : "),
            (219, b"00000000000R"),
            (231, b"GROUP ID : "),
            (242, b"GRP01     "),
        ];
        for (offset, bytes) in expected {
            assert_eq!(
                segment(&reply, offset, bytes.len())?,
                bytes,
                "offset {offset}"
            );
        }
        Ok(())
    }

    // audit: COACCT01-008
    // audit: COACCT01-017
    #[test]
    fn amounts_are_zoned_with_a_trailing_overpunch() -> TestResult {
        let mut account = account()?;
        for (cents, field) in [
            (-123_456, b"00000012345O"),
            (-100, b"00000000010}"),
            (194_000, b"00000019400{"),
            (999_999_999_999, b"99999999999I"),
            (-999_999_999_999, b"99999999999R"),
            (0, b"00000000000{"),
        ] {
            account.curr_bal = money(cents)?;
            let response = account_response(&account);
            assert_eq!(segment(&response, 52, 12)?, field.as_slice());
        }
        Ok(())
    }

    // audit: COACCT01-018
    #[test]
    fn dates_render_as_yyyy_mm_dd_or_ten_spaces() -> TestResult {
        let mut account = account()?;
        account.open_date = PicDate::BLANK;
        account.expiraion_date = date(1999, 1, 2)?;
        account.reissue_date = date(2030, 12, 31)?;
        let response = account_response(&account);
        assert!(all_spaces(segment(&response, 128, 10)?));
        assert_eq!(segment(&response, 150, 10)?, b"1999-01-02".as_slice());
        assert_eq!(segment(&response, 172, 10)?, b"2030-12-31".as_slice());
        Ok(())
    }

    // audit: COACCT01-019
    // audit: COACCT01-020
    #[test]
    fn the_found_reply_is_252_bytes_then_spaces_without_zip_or_filler() -> TestResult {
        let account = account()?;
        let response = account_response(&account);
        assert_eq!(response.len(), ACCT_RESPONSE_LEN);
        let reply = reply_of(&respond(AccountLookup::Found(&account), REQUEST_QUEUE))?;
        assert_eq!(reply.len(), BUFFER_LEN);
        assert_eq!(segment(&reply, 0, ACCT_RESPONSE_LEN)?, response.as_slice());
        assert!(all_spaces(segment(&reply, ACCT_RESPONSE_LEN, 748)?));
        assert!(!reply.windows(9).any(|run| run == b"ZIPZIPZIP"));
        assert!(!reply.contains(&b'#'));
        Ok(())
    }

    // audit: COACCT01-021
    #[test]
    fn the_not_found_reply_is_48_bytes_of_text_then_spaces() -> TestResult {
        let reply = reply_of(&respond(AccountLookup::NotFound, REQUEST_QUEUE))?;
        assert_eq!(reply.len(), BUFFER_LEN);
        assert_eq!(
            segment(&reply, 0, 37)?,
            b"INVALID REQUEST PARAMETERS ACCT ID : ".as_slice()
        );
        assert_eq!(segment(&reply, 37, 11)?, [b'*'; 11].as_slice());
        assert!(all_spaces(segment(&reply, NOT_FOUND_TEXT_LEN, 952)?));
        Ok(())
    }

    // audit: COACCT01-028
    // audit: COACCT01-029
    #[test]
    fn a_failed_read_is_an_error_body_with_the_truncated_text() -> TestResult {
        let CdraResponse::Error(error) = respond(AccountLookup::ReadFailed, REQUEST_QUEUE) else {
            return Err("expected an error body".into());
        };
        assert_eq!(error.len(), BUFFER_LEN);
        assert!(all_spaces(segment(&error, 0, 27)?));
        assert_eq!(
            segment(&error, 27, 25)?,
            b"ERROR WHILE READING ACCTF".as_slice()
        );
        assert!(all_spaces(segment(&error, 52, 2)?));
        assert!(all_spaces(segment(&error, 56, 2)?));
        assert!(all_spaces(segment(&error, 63, 2)?));
        assert_eq!(
            segment(&error, 65, 48)?,
            padded(REQUEST_QUEUE.as_bytes(), 48).as_slice()
        );
        assert!(all_spaces(segment(
            &error,
            MQ_ERR_DISPLAY_LEN,
            BUFFER_LEN - MQ_ERR_DISPLAY_LEN
        )?));
        assert_eq!(error, read_failure(REQUEST_QUEUE).error_body());
        Ok(())
    }

    // HALTED(H4): the code fields are checked by format only, never as a parity value, so this
    // test carries no audit tag.
    #[test]
    fn the_read_failure_code_fields_are_digits() -> TestResult {
        let error = read_failure(REQUEST_QUEUE).encode();
        assert!(has_format(segment(&error, 54, 2)?, b"99"));
        assert!(has_format(segment(&error, 58, 5)?, b"99999"));
        assert!(read_failure(REQUEST_QUEUE).error_para.is_spaces());
        Ok(())
    }

    // A name longer than 48 bytes never reaches service (X-COACCT01-8); this checks the MOVE
    // rule the field would apply, not a parity value.
    #[test]
    fn a_queue_name_beyond_48_bytes_keeps_its_first_48() -> TestResult {
        let long_name = "Q".repeat(60);
        let error = read_failure(&long_name).encode();
        assert_eq!(segment(&error, 65, 48)?, [b'Q'; 48].as_slice());
        assert_eq!(error.len(), MQ_ERR_DISPLAY_LEN);
        Ok(())
    }

    // audit: COACCT01-027
    #[test]
    fn each_body_depends_only_on_its_own_request_and_read() -> TestResult {
        let account = account()?;
        let found = respond(AccountLookup::Found(&account), REQUEST_QUEUE);
        let not_found = respond(AccountLookup::NotFound, REQUEST_QUEUE);
        assert_ne!(found, not_found);
        let first = decide(&decode_request(&body(*b"INQB", b"00000000007", b""))?);
        let read = decide(&decode_request(&body(*b"INQA", b"00000000007", b""))?);
        assert_eq!(read, CdraAction::ReadAccount(PicU::new(7)?));
        assert_eq!(
            decide(&decode_request(&body(*b"INQB", b"00000000007", b""))?),
            first
        );
        assert_eq!(respond(AccountLookup::NotFound, REQUEST_QUEUE), not_found);
        Ok(())
    }

    // audit: COACCT01-035
    #[test]
    fn debug_and_display_show_no_key_digit_or_body_byte() -> TestResult {
        let request = decode_request(&body(*b"INQA", b"00123456789", b""))?;
        let action = decide(&request);
        let account = account()?;
        let rendered = [
            format!("{request:?}"),
            format!("{action:?}"),
            format!(
                "{:?}",
                respond(AccountLookup::Found(&account), REQUEST_QUEUE)
            ),
            format!("{:?}", respond(AccountLookup::ReadFailed, REQUEST_QUEUE)),
            format!(
                "{:?}",
                decide(&decode_request(&body(*b"NOPE", b"00123456789", b""))?)
            ),
        ];
        for text in &rendered {
            assert!(!text.contains("123456789"), "{text}");
            assert!(!text.contains("00000012345O"), "{text}");
        }
        assert!(rendered.iter().any(|text| text.contains("INQA")));
        for error in [
            CdraRequestError::ShortBody { len: 9 },
            CdraRequestError::NonDigitKey { offset: 7 },
        ] {
            let shown = format!("{error} {error:?}");
            assert!(shown.contains("CDRA request"));
            assert!(!shown.contains("123456789"));
        }
        let rejected = decode_request(b"INQA12345678 ")
            .err()
            .ok_or("expected an error")?;
        assert_eq!(rejected, CdraRequestError::ShortBody { len: 13 });
        Ok(())
    }
}
