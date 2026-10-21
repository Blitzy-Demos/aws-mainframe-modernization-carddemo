//! COPAUS0C, the pending-authorization summary: the rules of endpoint 21,
//! `GET /api/v1/accounts/{acct_id}/authorizations` (transaction `CPVS`, map `COPAU0A`;
//! AAP 0.3.4.1).
//!
//! This module ports the `server` and `both` rows of `modern/audit/online/COPAUS0C.md`: the
//! account edits (COPAUS0C-005, COPAUS0C-006), the account read (COPAUS0C-008), the
//! cross-reference, account and customer reads and their messages (COPAUS0C-013 to
//! COPAUS0C-019), the customer block (COPAUS0C-020 to COPAUS0C-025), the summary block
//! (COPAUS0C-027 to COPAUS0C-030), the list rows (COPAUS0C-031 to COPAUS0C-034), the next-page
//! flag (COPAUS0C-036), the detail read failures (COPAUS0C-037, COPAUS0C-038), paging forward and
//! backward from a key (COPAUS0C-041, COPAUS0C-047), the empty message of a successful read
//! (COPAUS0C-052), the absence of any card number (COPAUS0C-055) and the halted codes
//! (COPAUS0C-056).
//!
//! The `client` rows stay with the screen: the `S` transfer to `/authorizations/detail` and its
//! `Invalid selection. Valid value is S` message (COPAUS0C-009 to COPAUS0C-012), F3 and the other
//! keys (COPAUS0C-003, COPAUS0C-004), the paging state and its page number, and the top-of-page
//! and bottom-of-page messages, which the screen decides from that state and endpoint 21 never
//! produces (COPAUS0C-039, COPAUS0C-040, COPAUS0C-045, escalation candidate X-COPAUS0C-6). The
//! account status field is always blank and is not returned (COPAUS0C-026).
//!
//! # Flow
//!
//! `api::services::copaus0c` performs the reads; this module decides every outcome and does no
//! I/O:
//!
//! 1. [`edit_account`] edits the raw `acct_id` path segment.
//! 2. The cross-reference is read by that account and handed to [`xref_keys`], whose
//!    [`XrefKeys`] key the account and customer reads.
//! 3. The summary is read by [`summary_key`], and [`summary_block`] takes the account, customer
//!    and summary outcomes in the program's order and builds the [`SummaryBlock`].
//! 4. Only with [`SummaryState::Found`] are details read: [`PageRequest::new`] turns the query's
//!    `direction` and `page_key` into the request, [`PageRequest::start_key`] and
//!    [`PageRequest::fetch_limit`] parameterise the index-served forward page query, and [`page`]
//!    builds the [`AuthPage`]. Without a summary the list is [`AuthPage::without_details`]. A
//!    failed detail query is [`detail_failure`].
//!
//! The COBOL performs every read even after one fails and sends the map more than once; endpoint
//! 21 stops at the first failed read and returns its message alone (escalation candidate
//! X-COPAUS0C-1), so each step returns a [`AuthSummaryFailure`] that ends the request.
//!
//! # Halts
//!
//! - `HALTED(H4)`: the CICS `RESP` and `RESP2` values and the IMS status codes the COBOL strings
//!   into its failure messages have no `PostgreSQL` counterpart (COPAUS0C-056). The messages carry
//!   the fields' initialised values in their place, nine zeros for each `PIC 9(09)` code and two
//!   spaces for the `PIC X(02)` status, and [`AuthSummaryFailure::codes`] reports
//!   [`MessageCodes::HaltedH4`]. Those characters are interim handling and no parity result; the
//!   fixed text, the account or customer key, the status and the field stay assertable.
//! - Halt candidate X-COPAUS0C-H3: a date byte that is not a digit is moved into `PIC 9(02)` with
//!   a result the compile options decide; [`row_from`] shows the stored bytes, which is no parity
//!   result. Pages from the 21st on (COPAUS0C-050) are paged like any other.
//! - Halt candidate X-COPAUS0C-H4: an F8 without a last key, or after a summary miss, has no
//!   single COBOL result (COPAUS0C-043, COPAUS0C-044); the screen sends no such request, and a
//!   request without a summary reads no detail.
//!
//! # Redaction and totality
//!
//! No value this module returns holds a card number, full or redacted: the cross-reference's
//! card number goes only to the COBOL COMMAREA, and the detail's is never displayed
//! (COPAUS0C-055, E32). Every function accepts any input, text of any length included, and
//! returns a value or a typed failure; nothing indexes, slices, unwraps or panics (Total Parsers
//! Over Untrusted Input). Amounts are formatted only through `copybook::edited`, with no
//! arithmetic on money (R1, R18).

use core::fmt;
use std::iter;

use copybook::edited::EditedPicture;
use copybook::pic::{Count, PicS, PicU, PicX};

use crate::money::Money;
use crate::records::account::AccountRecord;
use crate::records::card_xref::CardXrefRecord;
use crate::records::customer::CustomerRecord;
use crate::records::pending_auth_detail::PendingAuthDetailRecord;
use crate::records::pending_auth_summary::PendingAuthSummaryRecord;

/// Rows of the list on `COPAU0A`, the page size endpoint 21 uses when the caller sends none
/// (Bounded Pagination; COPAUS0C-031). The API admits 1 to 100.
pub const DEFAULT_PAGE_SIZE: usize = 5;

/// `ERRMSG LENGTH=78`: the message row shows the first 78 characters of `WS-MESSAGE PIC X(80)`.
const MESSAGE_WIDTH: usize = 78;

/// `ACCTIDI PIC X(11)` and `CDEMO-ACCT-ID PIC 9(11)`: the account key's digits.
const ACCOUNT_DIGITS: usize = 11;

/// `WS-CARD-RID-CUST-ID-X PIC X(09)` and `CUSTIDO PIC X(9)`: the customer key's digits.
const CUSTOMER_DIGITS: usize = 9;

/// `CNAMEO PIC X(25)`.
const NAME_WIDTH: usize = 25;

/// `ADDR001O` and `ADDR002O PIC X(25)`.
const ADDRESS_WIDTH: usize = 25;

/// `PHONE1O PIC X(13)`.
const PHONE_WIDTH: usize = 13;

/// `CUST-ADDR-ZIP(1:5)`: the ZIP bytes the second address row shows.
const ZIP_PREFIX: usize = 5;

/// `APPRCNTO`, `DECLCNTO PIC X(3)` and `WS-DISPLAY-COUNT PIC 9(03)`.
const COUNT_WIDTH: usize = 3;

/// `CREDLIMO` and `CREDBALO PIC X(12)`, the width of `-zzzzzzz9.99`.
const AMT12_WIDTH: usize = 12;

/// `CASHLIMO` and `CASHBALO PIC X(9)`, the width of `-zzzz9.99`.
const AMT9_WIDTH: usize = 9;

/// `APPRAMTO` and `DECLAMTO PIC X(10)`: `-zzzz9.99` and one space.
const AMT10_WIDTH: usize = 10;

/// `TRNIDnnI PIC X(16)`, which receives `PA-TRANSACTION-ID PIC X(15)`.
const TRANSACTION_ID_WIDTH: usize = 16;

/// `PROCESS-ENTER-KEY`, an empty account field (COPAUS0C-005).
const PLEASE_ENTER_ACCT_ID: &str = "Please enter Acct Id...";

/// `PROCESS-ENTER-KEY`, an account field that is not 11 digits (COPAUS0C-006).
const ACCT_ID_NOT_NUMERIC: &str = "Acct Id must be Numeric ...";

/// The account key's label in the read messages (COPAUS0C-014 to COPAUS0C-017).
const ACCOUNT_LABEL: &str = "Account:";

/// The customer key's label in the read messages (COPAUS0C-018, COPAUS0C-019).
const CUSTOMER_LABEL: &str = "Customer:";

/// `GETCARDXREF-BYACCT`, `NOTFND` (COPAUS0C-014).
const XREF_NOT_FOUND: &str = " not found in XREF file. Resp:";

/// `GETCARDXREF-BYACCT`, any other response (COPAUS0C-015).
const XREF_ERROR: &str = " System error while reading XREF file. Resp:";

/// `GETACCTDATA-BYACCT`, `NOTFND` (COPAUS0C-016).
const ACCT_NOT_FOUND: &str = " not found in ACCT file. Resp:";

/// `GETACCTDATA-BYACCT`, any other response (COPAUS0C-017).
const ACCT_ERROR: &str = " System error while reading ACCT file. Resp:";

/// `GETCUSTDATA-BYCUST`, `NOTFND` (COPAUS0C-018).
const CUST_NOT_FOUND: &str = " not found in CUST file. Resp:";

/// `GETCUSTDATA-BYCUST`, any other response (COPAUS0C-019).
const CUST_ERROR: &str = " System error while reading CUST file. Resp:";

/// The text between the `RESP` and `RESP2` codes of every read message.
const REAS_LABEL: &str = " Reas:";

/// `GET-AUTH-SUMMARY`, an IMS status other than found or not found (COPAUS0C-030).
const SUMMARY_ERROR: &str = " System error while reading AUTH Summary: Code:";

/// `GET-AUTHORIZATIONS`, an IMS status other than found or end (COPAUS0C-037).
const DETAILS_ERROR: &str = " System error while reading AUTH Details: Code:";

/// `REPOSITION-AUTHORIZATIONS`, an IMS status other than found or end (COPAUS0C-038).
const REPOSITION_ERROR: &str = " System error while repos. AUTH Details: Code:";

/// `HALTED(H4)`, COPAUS0C-056: the `WS-RESP-CD-DIS` and `WS-REAS-CD-DIS PIC 9(09)` positions
/// hold CICS values with no counterpart. Interim handling, not a parity result: the numeric
/// field's initialised value, nine zeros.
const HALTED_RESP_CODE: &str = "000000000";

/// `HALTED(H4)`, COPAUS0C-056: the `IMS-RETURN-CODE PIC X(02)` positions hold an IMS status with
/// no counterpart. Interim handling, not a parity result: the alphanumeric field's initialised
/// value, two spaces.
const HALTED_IMS_STATUS: &str = "  ";

/// The status class of an [`AuthSummaryFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The account field failed its edit: 400 (COPAUS0C-005, COPAUS0C-006).
    Input,
    /// A read found no record (`NOTFND`): 404 (COPAUS0C-014, COPAUS0C-016, COPAUS0C-018).
    NotFound,
    /// A read failed in the store: 500 (COPAUS0C-015, COPAUS0C-017, COPAUS0C-019, COPAUS0C-030,
    /// COPAUS0C-037, COPAUS0C-038).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400, 404 or 500.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Store => 500,
        }
    }
}

/// The request field a failure names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthSummaryField {
    /// The `acct_id` path segment, `ACCTID` on the map.
    AcctId,
}

/// Whether a failure message embeds code positions whose COBOL values are halted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageCodes {
    /// The whole message is fixed text and the account it names: every character is assertable.
    None,
    /// The message carries `RESP`/`RESP2` or IMS status positions, `HALTED(H4)`
    /// (COPAUS0C-056): only the text around them is assertable.
    HaltedH4,
}

/// A screen COPAUS0C sends instead of the summary: the message, the field it names, the status
/// class, and whether it embeds halted codes.
///
/// The message is the program's `STRING` result as the 78-character message row shows it, with
/// no padding. It holds no card number and no stored value but the account or customer key the
/// COBOL strings into it (COPAUS0C-055).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSummaryFailure {
    message: String,
    field: Option<AuthSummaryField>,
    class: FailureClass,
    codes: MessageCodes,
}

impl AuthSummaryFailure {
    /// A failed account edit: status class [`FailureClass::Input`] on the account field.
    fn input(message: &str) -> Self {
        Self {
            message: message.to_owned(),
            field: Some(AuthSummaryField::AcctId),
            class: FailureClass::Input,
            codes: MessageCodes::None,
        }
    }

    /// A failed file read: `label` and `key`, then `text`, the `RESP` code, `' Reas:'` and the
    /// `RESP2` code, `DELIMITED BY SIZE`, as far as the message row reaches.
    fn file_read(
        label: &str,
        key: &str,
        text: &str,
        class: FailureClass,
        field: Option<AuthSummaryField>,
    ) -> Self {
        Self {
            message: message_row(&[
                label,
                key,
                text,
                HALTED_RESP_CODE,
                REAS_LABEL,
                HALTED_RESP_CODE,
            ]),
            field,
            class,
            codes: MessageCodes::HaltedH4,
        }
    }

    /// A failed IMS call: `text`, then the IMS status.
    fn ims(text: &str) -> Self {
        Self {
            message: message_row(&[text, HALTED_IMS_STATUS]),
            field: None,
            class: FailureClass::Store,
            codes: MessageCodes::HaltedH4,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The field the message names, when the audit gives one.
    #[must_use]
    pub const fn field(&self) -> Option<AuthSummaryField> {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }

    /// Whether the message embeds halted code positions.
    #[must_use]
    pub const fn codes(&self) -> MessageCodes {
        self.codes
    }
}

impl fmt::Display for AuthSummaryFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthSummaryFailure {}

/// The first [`MESSAGE_WIDTH`] characters of `parts` strung together `DELIMITED BY SIZE`: the
/// `STRING` stops at the 80 bytes of `WS-MESSAGE`, and the map row shows 78 of them.
fn message_row(parts: &[&str]) -> String {
    parts
        .iter()
        .flat_map(|part| part.chars())
        .take(MESSAGE_WIDTH)
        .collect()
}

/// `PROCESS-ENTER-KEY`: the account field's edits (COPAUS0C-005, COPAUS0C-006, COPAUS0C-008).
///
/// `raw` is the `acct_id` path segment as received. It is blank when it is empty, all spaces or
/// all low-values (`ACCTIDI = SPACES OR LOW-VALUES`); a mix of the two is not blank. A value that
/// is not blank must be exactly 11 ASCII digits (`IS NUMERIC` over `PIC X(11)`): a shorter digit
/// string, a longer one, a space or any other character fails.
///
/// # Errors
///
/// A [`FailureClass::Input`] failure on [`AuthSummaryField::AcctId`]: `Please enter Acct Id...`
/// for a blank value (COPAUS0C-005), `Acct Id must be Numeric ...` for any other value that is not
/// 11 digits (COPAUS0C-006).
pub fn edit_account(raw: &str) -> Result<PicU<11>, AuthSummaryFailure> {
    if raw.chars().all(|c| c == ' ') || raw.chars().all(|c| c == '\0') {
        return Err(AuthSummaryFailure::input(PLEASE_ENTER_ACCT_ID));
    }
    let bytes = raw.as_bytes();
    if bytes.len() != ACCOUNT_DIGITS {
        return Err(AuthSummaryFailure::input(ACCT_ID_NOT_NUMERIC));
    }
    digits_value(bytes)
        .and_then(|value| PicU::<11>::new(value).ok())
        .ok_or_else(|| AuthSummaryFailure::input(ACCT_ID_NOT_NUMERIC))
}

/// The value of `bytes` read as unsigned decimal digits, or `None` when a byte is not an ASCII
/// digit or the value leaves `u64`.
fn digits_value(bytes: &[u8]) -> Option<u64> {
    bytes.iter().try_fold(0_u64, |value, &byte| {
        let digit = char::from(byte).to_digit(10)?;
        value.checked_mul(10)?.checked_add(u64::from(digit))
    })
}

/// `PA-AUTHORIZATION-KEY`, the sequence field `PAUT9CTS` of one pending-authorization detail:
/// the inverted date `PA-AUTH-DATE-9C PIC S9(05) COMP-3` and the inverted time
/// `PA-AUTH-TIME-9C PIC S9(09) COMP-3` (COPAUS0C-031, COPAUS0C-034).
///
/// It is the `{auth_key}` path segment of endpoints 22 and 23 and the `page_key` of endpoint 21,
/// and it orders like the twins: by date part, then time part. [`AuthKey::render`] writes it as
/// exactly 16 ASCII digits: a sign digit (`0` for zero or positive, `1` for negative) and the 5
/// digits of the date part's magnitude, then a sign digit and the 9 digits of the time part's
/// magnitude. COPAUA0C writes only non-negative keys, whose text is `0`, the 5 stored date
/// digits, `0` and the 9 stored time digits; a negative key, which the table's checks admit
/// (escalation candidate X-COPAUS0C-7), keeps a text of its own, so every stored row can be
/// paged to and selected. [`AuthKey::parse`] accepts exactly the texts `render` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthKey {
    date_9c: PicS<5>,
    time_9c: PicS<9>,
}

/// Characters of the date part of a rendered [`AuthKey`]: a sign digit and 5 digits.
const KEY_DATE_PART: usize = 6;

/// Characters of the time part of a rendered [`AuthKey`]: a sign digit and 9 digits.
const KEY_TIME_PART: usize = 10;

/// The text is not a rendered [`AuthKey`]. It carries none of the text it rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthKeyError;

impl fmt::Display for AuthKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "an authorization key is 16 digits: a sign digit 0 or 1 and 5 date digits, \
             then a sign digit 0 or 1 and 9 time digits, with no negative zero",
        )
    }
}

impl std::error::Error for AuthKeyError {}

impl AuthKey {
    /// Characters of [`AuthKey::render`]'s text.
    pub const RENDERED_WIDTH: usize = KEY_DATE_PART + KEY_TIME_PART;

    /// The key of these two parts.
    #[must_use]
    pub const fn new(date_9c: PicS<5>, time_9c: PicS<9>) -> Self {
        Self { date_9c, time_9c }
    }

    /// The key of a detail segment, `PA-AUTHORIZATION-KEY` as `POPULATE-AUTH-LIST` saves it for
    /// the row (COPAUS0C-034).
    #[must_use]
    pub const fn of(detail: &PendingAuthDetailRecord) -> Self {
        Self::new(detail.auth_date_9c, detail.auth_time_9c)
    }

    /// The lowest key any detail can hold, `−99999` and `−999999999`: the start of the first
    /// page (COPAUS0C-031).
    #[must_use]
    pub fn lowest() -> Self {
        Self::new(most_negative::<5>(), most_negative::<9>())
    }

    /// The inverted date part, `auth_date_9c`.
    #[must_use]
    pub const fn date_9c(self) -> PicS<5> {
        self.date_9c
    }

    /// The inverted time part, `auth_time_9c`.
    #[must_use]
    pub const fn time_9c(self) -> PicS<9> {
        self.time_9c
    }

    /// The key's 16-digit text (format in the type documentation).
    #[must_use]
    pub fn render(self) -> String {
        let date = self.date_9c.get();
        let time = self.time_9c.get();
        let date_sign = sign_digit(date);
        let date_digits = date.unsigned_abs();
        let time_sign = sign_digit(time);
        let time_digits = time.unsigned_abs();
        format!("{date_sign}{date_digits:05}{time_sign}{time_digits:09}")
    }

    /// The key a [`AuthKey::render`] text names; total over any text.
    ///
    /// # Errors
    ///
    /// [`AuthKeyError`] unless `text` is exactly 16 ASCII digits whose sign digits are `0` or `1`
    /// and whose negative parts are not zero.
    pub fn parse(text: &str) -> Result<Self, AuthKeyError> {
        let bytes =
            <&[u8; Self::RENDERED_WIDTH]>::try_from(text.as_bytes()).map_err(|_| AuthKeyError)?;
        let (date_part, time_part) = bytes
            .split_first_chunk::<KEY_DATE_PART>()
            .ok_or(AuthKeyError)?;
        let time_part = <&[u8; KEY_TIME_PART]>::try_from(time_part).map_err(|_| AuthKeyError)?;
        let [date_sign, date_digits @ ..] = date_part;
        let [time_sign, time_digits @ ..] = time_part;
        Ok(Self::new(
            signed_part(*date_sign, date_digits)?,
            signed_part(*time_sign, time_digits)?,
        ))
    }
}

/// `−MAX`, the most negative value of a `PicS<N>`.
fn most_negative<const N: u32>() -> PicS<N> {
    // `−MAX` is inside `PicS::<N>`'s range by its definition, so the default is never taken; it
    // only keeps this function free of a panic path.
    PicS::<N>::new(-PicS::<N>::MAX).unwrap_or_default()
}

/// The sign digit of a rendered key part.
const fn sign_digit(value: i64) -> char {
    if value < 0 { '1' } else { '0' }
}

/// One part of a rendered key: its sign digit and its magnitude's digits.
fn signed_part<const N: u32>(sign: u8, digits: &[u8]) -> Result<PicS<N>, AuthKeyError> {
    let magnitude = digits_value(digits).ok_or(AuthKeyError)?;
    // TRUNCATION: `digits_value` builds the magnitude with checked arithmetic, and
    // `AuthKey::parse` passes only the 5 date digits or the 9 time digits, so it is at most
    // 999_999_999 and always fits an `i64`; were it ever out of range, the conversion fails and
    // the key is refused with `AuthKeyError` rather than wrapped. `PicS::<N>::new` then bounds
    // the signed value to the part's picture, ±99_999 or ±999_999_999.
    let magnitude = i64::try_from(magnitude).map_err(|_| AuthKeyError)?;
    // A `1` sign over a zero magnitude is rejected, so every key has exactly one text.
    let value = match (sign, magnitude) {
        (b'0', _) => magnitude,
        (b'1', 1..) => -magnitude,
        _ => return Err(AuthKeyError),
    };
    PicS::<N>::new(value).map_err(|_| AuthKeyError)
}

/// The outcome of one read the service performs for the summary: a CICS `READ` or the DL/I `GU`
/// of the summary segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup<T> {
    /// The read returned its record (`DFHRESP(NORMAL)`; IMS status `'  '` or `'FW'`).
    Found(T),
    /// No record has the key (`DFHRESP(NOTFND)`; IMS status `'GE'`).
    NotFound,
    /// The read ended any other way: a store failure, or for the cross-reference more than one
    /// row for the account (CICS `DUPKEY` on the non-unique path, COPAUS0C-015).
    Failed,
}

impl<T> Lookup<T> {
    /// The record, or the failure the read's outcome gives.
    fn found_or(
        self,
        not_found: impl FnOnce() -> AuthSummaryFailure,
        failed: impl FnOnce() -> AuthSummaryFailure,
    ) -> Result<T, AuthSummaryFailure> {
        match self {
            Self::Found(record) => Ok(record),
            Self::NotFound => Err(not_found()),
            Self::Failed => Err(failed()),
        }
    }
}

/// The keys `GETCARDXREF-BYACCT` takes from the cross-reference row (COPAUS0C-013). The row's
/// card number is not among them (COPAUS0C-055).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XrefKeys {
    acct_id: PicU<11>,
    cust_id: PicU<9>,
}

impl XrefKeys {
    /// `XREF-ACCT-ID`, the key of the account read (`GETACCTDATA-BYACCT`).
    #[must_use]
    pub const fn acct_id(self) -> PicU<11> {
        self.acct_id
    }

    /// `XREF-CUST-ID`, the key of the customer read (`GETCUSTDATA-BYCUST`).
    #[must_use]
    pub const fn cust_id(self) -> PicU<9> {
        self.cust_id
    }
}

/// `GETCARDXREF-BYACCT`: the outcome of the cross-reference read by the edited account, through
/// the `acct_id` index of `card_xref` (COPAUS0C-013 to COPAUS0C-015).
///
/// # Errors
///
/// For [`Lookup::NotFound`], a [`FailureClass::NotFound`] failure on
/// [`AuthSummaryField::AcctId`]: `'Account:'`, the 11 account digits, `' not found in XREF file.
/// Resp:'`, the `RESP` code, `' Reas:'` and the `RESP2` code, 73 characters (COPAUS0C-014). For
/// [`Lookup::Failed`], a [`FailureClass::Store`] failure: `'Account:'`, the account, `' System
/// error while reading XREF file. Resp:'`, the `RESP` code and `' Reas:'`, the first 78
/// characters (COPAUS0C-015). The codes are `HALTED(H4)` (COPAUS0C-056).
pub fn xref_keys(
    acct_id: PicU<11>,
    xref: Lookup<&CardXrefRecord>,
) -> Result<XrefKeys, AuthSummaryFailure> {
    let account = account_digits(acct_id);
    let record = xref.found_or(
        || {
            AuthSummaryFailure::file_read(
                ACCOUNT_LABEL,
                &account,
                XREF_NOT_FOUND,
                FailureClass::NotFound,
                Some(AuthSummaryField::AcctId),
            )
        },
        || {
            AuthSummaryFailure::file_read(
                ACCOUNT_LABEL,
                &account,
                XREF_ERROR,
                FailureClass::Store,
                None,
            )
        },
    )?;
    Ok(XrefKeys {
        acct_id: record.acct_id,
        cust_id: record.cust_id,
    })
}

/// `GET-AUTH-SUMMARY`: the key of the summary read, `MOVE CDEMO-ACCT-ID TO PA-ACCT-ID`. The
/// edited account becomes the signed `PA-ACCT-ID PIC S9(11) COMP-3`, the `acct_id` of
/// `pending_auth_summaries` and `pending_auth_details` (COPAUS0C-027).
#[must_use]
pub fn summary_key(acct_id: PicU<11>) -> PicS<11> {
    PicS::<11>::from(acct_id)
}

/// Whether the account has a pending-authorization summary (COPAUS0C-028, COPAUS0C-029).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SummaryState {
    /// The summary segment was found: its counts and amounts are shown and the list is read.
    Found,
    /// No summary exists, a normal outcome: zero fields, an empty list and no message.
    NotFound,
}

/// The customer and summary block of `COPAU0A`, every value as the map field shows it: exactly
/// the field's width, built with the program's moves and pictures, so the screen formats
/// nothing (COPAUS0C-020 to COPAUS0C-025, COPAUS0C-027 to COPAUS0C-029).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SummaryBlock {
    /// Customer Id, `CUSTIDO PIC X(9)`: the 9 digits of `CUST-ID`, leading zeros kept
    /// (COPAUS0C-020).
    pub customer_id: String,
    /// Name, `CNAMEO PIC X(25)` (COPAUS0C-021).
    pub name: String,
    /// The first address row, `ADDR001O PIC X(25)` (COPAUS0C-022).
    pub address_line_1: String,
    /// The second address row, `ADDR002O PIC X(25)` (COPAUS0C-023).
    pub address_line_2: String,
    /// PH, `PHONE1O PIC X(13)`: the first 13 bytes of phone 1 (COPAUS0C-024).
    pub phone: String,
    /// Credit Lim, `CREDLIMO PIC X(12)`: `ACCT-CREDIT-LIMIT` edited by `-zzzzzzz9.99`
    /// (COPAUS0C-025).
    pub credit_limit: String,
    /// Cash Lim, `CASHLIMO PIC X(9)`: `ACCT-CASH-CREDIT-LIMIT` edited by `-zzzz9.99`
    /// (COPAUS0C-025).
    pub cash_limit: String,
    /// Whether the summary was found; the six fields below are its figures, or zeros without it.
    pub summary: SummaryState,
    /// Approval #, `APPRCNTO PIC X(3)` (COPAUS0C-028, COPAUS0C-029).
    pub approved_count: String,
    /// Decline #, `DECLCNTO PIC X(3)` (COPAUS0C-028, COPAUS0C-029).
    pub declined_count: String,
    /// Credit Bal, `CREDBALO PIC X(12)` (COPAUS0C-028, COPAUS0C-029).
    pub credit_balance: String,
    /// Cash Bal, `CASHBALO PIC X(9)` (COPAUS0C-028, COPAUS0C-029).
    pub cash_balance: String,
    /// Appr Amt, `APPRAMTO PIC X(10)` (COPAUS0C-028, COPAUS0C-029).
    pub approved_amount: String,
    /// Decl Amt, `DECLAMTO PIC X(10)` (COPAUS0C-028, COPAUS0C-029).
    pub declined_amount: String,
}

/// `GATHER-ACCOUNT-DETAILS`: the block from the account read by `XREF-ACCT-ID`, the customer read
/// by `XREF-CUST-ID` and the summary read by [`summary_key`], judged in the program's order
/// (COPAUS0C-016 to COPAUS0C-030).
///
/// With the account and the customer found, the block holds the customer's fields and the
/// account's limits; a found summary adds its counts and amounts ([`SummaryState::Found`]), and a
/// missing one, a normal outcome, leaves `000` counts and zero-filled amounts
/// ([`SummaryState::NotFound`]), status 200 and no message (COPAUS0C-029, COPAUS0C-052).
///
/// # Errors
///
/// The first failed read, in the order account, customer, summary (escalation candidate
/// X-COPAUS0C-1):
///
/// - account [`Lookup::NotFound`]: [`FailureClass::NotFound`], `'Account:'`, the 11 digits of
///   `XREF-ACCT-ID`, `' not found in ACCT file. Resp:'`, the `RESP` code, `' Reas:'` and the
///   `RESP2` code, 73 characters (COPAUS0C-016); [`Lookup::Failed`]: [`FailureClass::Store`],
///   the same with `' System error while reading ACCT file. Resp:'`, the first 78 characters
///   (COPAUS0C-017);
/// - customer [`Lookup::NotFound`]: [`FailureClass::NotFound`], `'Customer:'`, the 9 digits of
///   `XREF-CUST-ID` and `' not found in CUST file. Resp:'` with the codes, 72 characters
///   (COPAUS0C-018); [`Lookup::Failed`]: [`FailureClass::Store`], with `' System error while
///   reading CUST file. Resp:'`, the first 78 characters (COPAUS0C-019);
/// - summary [`Lookup::Failed`]: [`FailureClass::Store`], `' System error while reading AUTH
///   Summary: Code:'` and the IMS status, 49 characters (COPAUS0C-030).
///
/// Every code position is `HALTED(H4)` (COPAUS0C-056).
pub fn summary_block(
    keys: XrefKeys,
    account: Lookup<&AccountRecord>,
    customer: Lookup<&CustomerRecord>,
    summary: Lookup<&PendingAuthSummaryRecord>,
) -> Result<SummaryBlock, AuthSummaryFailure> {
    let account_key = account_digits(keys.acct_id);
    let account = account.found_or(
        || {
            AuthSummaryFailure::file_read(
                ACCOUNT_LABEL,
                &account_key,
                ACCT_NOT_FOUND,
                FailureClass::NotFound,
                None,
            )
        },
        || {
            AuthSummaryFailure::file_read(
                ACCOUNT_LABEL,
                &account_key,
                ACCT_ERROR,
                FailureClass::Store,
                None,
            )
        },
    )?;
    let customer_key = customer_digits(keys.cust_id);
    let customer = customer.found_or(
        || {
            AuthSummaryFailure::file_read(
                CUSTOMER_LABEL,
                &customer_key,
                CUST_NOT_FOUND,
                FailureClass::NotFound,
                None,
            )
        },
        || {
            AuthSummaryFailure::file_read(
                CUSTOMER_LABEL,
                &customer_key,
                CUST_ERROR,
                FailureClass::Store,
                None,
            )
        },
    )?;
    let figures = match summary {
        Lookup::Found(record) => SummaryFigures::found(record),
        Lookup::NotFound => SummaryFigures::not_found(),
        Lookup::Failed => return Err(AuthSummaryFailure::ims(SUMMARY_ERROR)),
    };
    Ok(SummaryBlock {
        customer_id: customer_digits(customer.id),
        name: customer_name(customer),
        address_line_1: address_line_1(customer),
        address_line_2: address_line_2(customer),
        phone: string_into(&[customer.phone_num_1.as_bytes()], PHONE_WIDTH),
        credit_limit: edited(EditedPicture::MinusZ8Dot99, account.credit_limit),
        cash_limit: edited(EditedPicture::MinusZ5Dot99, account.cash_credit_limit),
        summary: figures.state,
        approved_count: figures.approved_count,
        declined_count: figures.declined_count,
        credit_balance: figures.credit_balance,
        cash_balance: figures.cash_balance,
        approved_amount: figures.approved_amount,
        declined_amount: figures.declined_amount,
    })
}

/// The six summary fields of the block and the state they come from.
struct SummaryFigures {
    state: SummaryState,
    approved_count: String,
    declined_count: String,
    credit_balance: String,
    cash_balance: String,
    approved_amount: String,
    declined_amount: String,
}

impl SummaryFigures {
    /// A found summary (COPAUS0C-028): the counts through `WS-DISPLAY-COUNT PIC 9(03)`, Credit Bal
    /// through `-zzzzzzz9.99`, Cash Bal, Appr Amt and Decl Amt through `-zzzz9.99`, the last two
    /// followed by one space in their 10-character fields.
    fn found(record: &PendingAuthSummaryRecord) -> Self {
        let five_digit_amount = |value: Money<9>| {
            let text = edited(EditedPicture::MinusZ5Dot99, value);
            string_into(&[text.as_bytes()], AMT10_WIDTH)
        };
        Self {
            state: SummaryState::Found,
            approved_count: display_count(record.approved_auth_cnt),
            declined_count: display_count(record.declined_auth_cnt),
            credit_balance: edited(EditedPicture::MinusZ8Dot99, record.credit_balance),
            cash_balance: edited(EditedPicture::MinusZ5Dot99, record.cash_balance),
            approved_amount: five_digit_amount(record.approved_auth_amt),
            declined_amount: five_digit_amount(record.declined_auth_amt),
        }
    }

    /// No summary (COPAUS0C-029): `MOVE ZERO` fills each alphanumeric field with `0` characters,
    /// which is not an edited zero such as `0.00`.
    fn not_found() -> Self {
        let zeros = |width: usize| "0".repeat(width);
        Self {
            state: SummaryState::NotFound,
            approved_count: zeros(COUNT_WIDTH),
            declined_count: zeros(COUNT_WIDTH),
            credit_balance: zeros(AMT12_WIDTH),
            cash_balance: zeros(AMT9_WIDTH),
            approved_amount: zeros(AMT10_WIDTH),
            declined_amount: zeros(AMT10_WIDTH),
        }
    }
}

/// An account key as the messages string it: its 11 digits, leading zeros kept.
fn account_digits(acct_id: PicU<11>) -> String {
    let value = acct_id.get();
    format!("{value:0ACCOUNT_DIGITS$}")
}

/// A customer key as the block and the messages show it: its 9 digits, leading zeros kept.
fn customer_digits(cust_id: PicU<9>) -> String {
    let value = cust_id.get();
    format!("{value:0CUSTOMER_DIGITS$}")
}

/// The Name row (COPAUS0C-021): `CUST-FIRST-NAME DELIMITED BY SPACES`, `' '`,
/// `CUST-MIDDLE-NAME(1:1)`, `' '`, `CUST-LAST-NAME DELIMITED BY SPACES`, into `CNAMEO`.
fn customer_name(customer: &CustomerRecord) -> String {
    let middle = customer.middle_name.as_bytes();
    let middle_initial = middle.get(..1).unwrap_or_default();
    string_into(
        &[
            up_to_space(customer.first_name.as_bytes()),
            b" ",
            middle_initial,
            b" ",
            up_to_space(customer.last_name.as_bytes()),
        ],
        NAME_WIDTH,
    )
}

/// The first address row (COPAUS0C-022): `CUST-ADDR-LINE-1 DELIMITED BY '  '`, `','`,
/// `CUST-ADDR-LINE-2 DELIMITED BY '  '`, into `ADDR001O`.
fn address_line_1(customer: &CustomerRecord) -> String {
    string_into(
        &[
            up_to_two_spaces(customer.addr_line_1.as_bytes()),
            b",",
            up_to_two_spaces(customer.addr_line_2.as_bytes()),
        ],
        ADDRESS_WIDTH,
    )
}

/// The second address row (COPAUS0C-023): `CUST-ADDR-LINE-3 DELIMITED BY '  '`, `','`, the
/// state code as stored, `','` and `CUST-ADDR-ZIP(1:5)`, into `ADDR002O`.
fn address_line_2(customer: &CustomerRecord) -> String {
    let zip = customer.addr_zip.as_bytes();
    string_into(
        &[
            up_to_two_spaces(customer.addr_line_3.as_bytes()),
            b",",
            customer.addr_state_cd.as_bytes(),
            b",",
            zip.get(..ZIP_PREFIX).unwrap_or(zip),
        ],
        ADDRESS_WIDTH,
    )
}

/// `field DELIMITED BY SPACES`: the bytes before its first space, all of it when it has none.
fn up_to_space(field: &[u8]) -> &[u8] {
    field
        .iter()
        .position(|&byte| byte == b' ')
        .and_then(|end| field.get(..end))
        .unwrap_or(field)
}

/// `field DELIMITED BY '  '`: the bytes before its first two consecutive spaces, all of it when
/// it has none.
fn up_to_two_spaces(field: &[u8]) -> &[u8] {
    field
        .windows(2)
        .position(|pair| pair == b"  ")
        .and_then(|end| field.get(..end))
        .unwrap_or(field)
}

/// COBOL `STRING parts DELIMITED BY SIZE INTO` an alphanumeric field of `width` characters that
/// holds spaces: the bytes in order, cut at `width`, the rest of the field blank. Each byte is
/// one character.
fn string_into(parts: &[&[u8]], width: usize) -> String {
    let mut field: String = parts
        .iter()
        .flat_map(|part| part.iter())
        .take(width)
        .map(|&byte| char::from(byte))
        .collect();
    let filled = field.chars().count();
    field.extend(iter::repeat_n(' ', width.saturating_sub(filled)));
    field
}

/// `value` moved to the numeric-edited `picture`.
fn edited<const N: u32>(picture: EditedPicture, value: Money<N>) -> String {
    picture.format(value.as_decimal())
}

/// A `PIC S9(4) COMP` count moved to `WS-DISPLAY-COUNT PIC 9(03)` (COPAUS0C-028): its sign
/// dropped and its digits beyond the third dropped from the left, so 7 is `007`, 1234 is `234`
/// and −5 is `005`.
fn display_count(count: Count) -> String {
    // TRUNCATION: the COBOL MOVE into the unsigned 3-digit receiver drops the sign and every
    // digit above the hundreds (escalation candidate X-COPAUS0C-4); the remainder is below 1000.
    let value = count.get().unsigned_abs() % 1000;
    format!("{value:0COUNT_WIDTH$}")
}

/// `PAPRVnnI`: `A` when `PA-AUTH-RESP-CODE` is `00`, `D` for any other value (COPAUS0C-033).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Approval {
    /// `A`: the response code is `00`.
    Approved,
    /// `D`: any other response code.
    Declined,
}

impl Approval {
    /// The one-character field value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "A",
            Self::Declined => "D",
        }
    }
}

/// One row of the list, as `POPULATE-AUTH-LIST` fills it (COPAUS0C-032 to COPAUS0C-034), and the
/// key the screen keeps for the row. It holds no card number (COPAUS0C-055).
///
/// The date and time are display strings, not typed values: `PA-AUTH-ORIG-DATE` and
/// `PA-AUTH-ORIG-TIME` are `PIC X(06)`, outside the `X(10)` dates and `X(26)` timestamps that
/// Typed Dates and Timestamps covers, and the program copies their bytes with no calendar check,
/// so `241399` shows `13/99/24`, which no date type holds (COPAUS0C-034).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthRow {
    /// The row's `PA-AUTHORIZATION-KEY`, `CDEMO-CPVS-AUTH-KEYS(n)` (COPAUS0C-034).
    pub key: AuthKey,
    /// Transaction ID, `TRNIDnnI PIC X(16)`: the 15 stored bytes and one space (COPAUS0C-032).
    pub transaction_id: String,
    /// Date, `PDATEnnI PIC X(8)`: `MM/DD/YY` from the stored `YYMMDD` (COPAUS0C-034).
    pub date: String,
    /// Time, `PTIMEnnI PIC X(8)`: `HH:MM:SS` from the stored `HHMMSS` (COPAUS0C-034).
    pub time: String,
    /// Type, `PTYPEnnI PIC X(4)`: the 4 stored bytes (COPAUS0C-032).
    pub auth_type: String,
    /// A/D, `PAPRVnnI PIC X(1)` (COPAUS0C-033).
    pub approval: Approval,
    /// STS, `PSTATnnI PIC X(1)`: the stored match status byte (COPAUS0C-032).
    pub match_status: String,
    /// Amount, `PAMTnnnI PIC X(12)`: `PA-APPROVED-AMT` edited by `-zzzzzzz9.99` (COPAUS0C-033).
    pub amount: String,
}

/// `POPULATE-AUTH-LIST`: the row one detail segment fills (COPAUS0C-032 to COPAUS0C-034).
///
/// The amount is the approved amount, not the transaction amount, in 12 characters with 8
/// integer digits, so the two high-order digits of the `S9(10)V99` value are dropped
/// (X-COPAUS0C-4). The date takes the stored bytes 3-4, 5-6 and 1-2 and the time bytes 1-2, 3-4
/// and 5-6, as stored; a date byte that is not a digit is shown as stored, which is no parity
/// result (halt candidate X-COPAUS0C-H3).
#[must_use]
pub fn row_from(detail: &PendingAuthDetailRecord) -> AuthRow {
    let [year_1, year_2, month_1, month_2, day_1, day_2] = *detail.orig_date.as_bytes();
    let [hour_1, hour_2, minute_1, minute_2, second_1, second_2] = *detail.orig_time.as_bytes();
    let approval = if detail.is_approved() {
        Approval::Approved
    } else {
        Approval::Declined
    };
    AuthRow {
        key: AuthKey::of(detail),
        transaction_id: string_into(&[detail.transaction_id.as_bytes()], TRANSACTION_ID_WIDTH),
        date: text(&[month_1, month_2, b'/', day_1, day_2, b'/', year_1, year_2]),
        time: text(&[
            hour_1, hour_2, b':', minute_1, minute_2, b':', second_1, second_2,
        ]),
        auth_type: field_text(&detail.auth_type),
        approval,
        match_status: field_text(&detail.match_status),
        amount: edited(EditedPicture::MinusZ8Dot99, detail.approved_amt),
    }
}

/// `bytes`, one character each.
fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// An alphanumeric field's bytes, one character each, padding kept.
fn field_text<const N: usize>(field: &PicX<N>) -> String {
    text(field.as_bytes())
}

/// The paging key the caller sends with a `page_key`: F8 pages forward, F7 backward
/// (COPAUS0C-041, COPAUS0C-047).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// F8: the rows after the key.
    Forward,
    /// F7: the rows from the key.
    Backward,
}

/// The page endpoint 21 reads, `PROCESS-PAGE-FORWARD` after the paragraph that positions it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageRequest {
    /// ENTER and first entry: the account's details from the first (COPAUS0C-008,
    /// COPAUS0C-031).
    First,
    /// F8 with the displayed page's last key: `REPOSITION-AUTHORIZATIONS` at the key, then the
    /// details after it (COPAUS0C-041).
    After(AuthKey),
    /// F7 with the first key saved for the previous page: `REPOSITION-AUTHORIZATIONS` at the
    /// key, which is the page's first row (COPAUS0C-047).
    From(AuthKey),
}

impl PageRequest {
    /// The request a query names. Without a `page_key` it is [`PageRequest::First`], whatever
    /// the direction; with one it is [`PageRequest::From`] for [`Direction::Backward`] and
    /// [`PageRequest::After`] otherwise, an absent direction paging forward.
    #[must_use]
    pub const fn new(direction: Option<Direction>, page_key: Option<AuthKey>) -> Self {
        match (direction, page_key) {
            (_, None) => Self::First,
            (Some(Direction::Backward), Some(key)) => Self::From(key),
            (Some(Direction::Forward) | None, Some(key)) => Self::After(key),
        }
    }

    /// The inclusive start key of the forward page query, `(auth_date_9c, auth_time_9c) >=`
    /// this key in key order: the lowest key for the first page, the request's key otherwise.
    #[must_use]
    pub fn start_key(self) -> AuthKey {
        match self {
            Self::First => AuthKey::lowest(),
            Self::After(key) | Self::From(key) => key,
        }
    }

    /// The `LIMIT` of the forward page query for `page_size` rows (Bounded Pagination).
    ///
    /// [`PageRequest::First`] and [`PageRequest::From`] read the page and one lookahead row for
    /// the next-page flag: `page_size + 1`. [`PageRequest::After`] reads one row more, the row at
    /// its key: the query is inclusive and the repositioning call must find that row before any
    /// row after it is shown (COPAUS0C-041), so the anchor, the page and the lookahead are
    /// `page_size + 2`.
    #[must_use]
    pub const fn fetch_limit(self, page_size: usize) -> usize {
        match self {
            Self::First | Self::From(_) => page_size.saturating_add(1),
            Self::After(_) => page_size.saturating_add(2),
        }
    }
}

/// `CDEMO-CPVS-NEXT-PAGE-FLG` as endpoint 21 reports it (COPAUS0C-036).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NextPage {
    /// The page is full and another detail follows it: `NEXT-PAGE-YES`.
    Yes,
    /// The page is full and no detail follows it: `NEXT-PAGE-NO`.
    No,
    /// The read stopped before the page was full, or read nothing: the flag is not set, and the
    /// screen keeps the value an earlier page gave it (escalation candidate X-COPAUS0C-3).
    Unreported,
}

/// The list of one response: its rows in key order and the next-page flag (COPAUS0C-031 to
/// COPAUS0C-036).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthPage {
    /// Up to `page_size` rows.
    pub rows: Vec<AuthRow>,
    /// The next-page flag.
    pub next_page: NextPage,
}

impl AuthPage {
    /// The list of a request that reads no detail, or whose repositioning call finds no detail
    /// at its key: no rows and no flag (COPAUS0C-029, COPAUS0C-041, COPAUS0C-047).
    #[must_use]
    pub const fn without_details() -> Self {
        Self {
            rows: Vec::new(),
            next_page: NextPage::Unreported,
        }
    }

    /// The first row's key, which the screen saves as the page's first key (COPAUS0C-039).
    #[must_use]
    pub fn first_key(&self) -> Option<AuthKey> {
        self.rows.first().map(|row| row.key)
    }

    /// The last row's key, `CDEMO-CPVS-PAUKEY-LAST`: none when the page shows no row
    /// (COPAUS0C-039).
    #[must_use]
    pub fn last_key(&self) -> Option<AuthKey> {
        self.rows.last().map(|row| row.key)
    }
}

/// `PROCESS-PAGE-FORWARD`: the page of `request` from `details`, the rows the forward page query
/// returned for [`PageRequest::start_key`] and [`PageRequest::fetch_limit`], in ascending key
/// order (COPAUS0C-031, COPAUS0C-036, COPAUS0C-041, COPAUS0C-047).
///
/// - [`PageRequest::First`]: the first `page_size` details.
/// - [`PageRequest::After`]: when the first detail has the request's key, the `page_size`
///   details after it; otherwise no detail has the key and the list is empty.
/// - [`PageRequest::From`]: when the first detail has the request's key, `page_size` details
///   from it; otherwise the list is empty.
///
/// A page that fills all `page_size` rows reads one more detail and reports [`NextPage::Yes`]
/// when it exists and [`NextPage::No`] when it does not; any other page reports
/// [`NextPage::Unreported`].
#[must_use]
pub fn page(
    request: PageRequest,
    details: &[PendingAuthDetailRecord],
    page_size: usize,
) -> AuthPage {
    let from_position = match request {
        PageRequest::First => details,
        PageRequest::From(key) => match details.first() {
            Some(first) if AuthKey::of(first) == key => details,
            _ => return AuthPage::without_details(),
        },
        PageRequest::After(key) => match details.split_first() {
            Some((first, rest)) if AuthKey::of(first) == key => rest,
            _ => return AuthPage::without_details(),
        },
    };
    let rows: Vec<AuthRow> = from_position.iter().take(page_size).map(row_from).collect();
    let next_page = if rows.len() < page_size {
        NextPage::Unreported
    } else if from_position.len() > page_size {
        NextPage::Yes
    } else {
        NextPage::No
    };
    AuthPage { rows, next_page }
}

/// The failure of a forward page query that ends in a store error, named by the first DL/I call
/// the query stands for (COPAUS0C-037, COPAUS0C-038).
///
/// For [`PageRequest::First`] that is `GET-AUTHORIZATIONS`: `' System error while reading AUTH
/// Details: Code:'` and the IMS status, 49 characters. For [`PageRequest::After`] and
/// [`PageRequest::From`] it is `REPOSITION-AUTHORIZATIONS`: `' System error while repos. AUTH
/// Details: Code:'` and the status, 48 characters. Status class [`FailureClass::Store`]; the
/// status is `HALTED(H4)` (COPAUS0C-056). The rows the COBOL read before the failure are not
/// returned (X-COPAUS0C-1).
#[must_use]
pub fn detail_failure(request: PageRequest) -> AuthSummaryFailure {
    match request {
        PageRequest::First => AuthSummaryFailure::ims(DETAILS_ERROR),
        PageRequest::After(_) | PageRequest::From(_) => AuthSummaryFailure::ims(REPOSITION_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use copybook::pic::{Count, PicDate, PicS, PicU, PicX};

    use super::{
        AuthKey, AuthKeyError, AuthPage, AuthSummaryFailure, AuthSummaryField, DEFAULT_PAGE_SIZE,
        Direction, FailureClass, Lookup, MessageCodes, NextPage, PageRequest, SummaryState,
        XrefKeys, detail_failure, edit_account, page, row_from, summary_block, summary_key,
        xref_keys,
    };
    use crate::money::{Decimal, Money};
    use crate::records::account::AccountRecord;
    use crate::records::card_xref::CardXrefRecord;
    use crate::records::customer::CustomerRecord;
    use crate::records::pending_auth_detail::PendingAuthDetailRecord;
    use crate::records::pending_auth_summary::PendingAuthSummaryRecord;
    use crate::sensitive::{CardNumber, Ssn};

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field, HTTP status and halted codes.
    type Shown = (String, Option<AuthSummaryField>, u16, MessageCodes);

    /// An invented card number; no fixture value is quoted.
    const CARD: &[u8; 16] = b"4000000000009876";

    fn shown(failure: &AuthSummaryFailure) -> Shown {
        (
            failure.message().to_owned(),
            failure.field(),
            failure.class().http_status(),
            failure.codes(),
        )
    }

    fn pic<const N: usize>(text: &str) -> Result<PicX<N>, String> {
        PicX::<N>::from_text(text).map_err(|error| format!("{text:?}: {error:?}"))
    }

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, String> {
        Money::<N>::new(Decimal::new(cents, 2)).map_err(|error| error.to_string())
    }

    fn acct(value: u64) -> Result<PicU<11>, String> {
        PicU::<11>::new(value).map_err(|error| format!("{error:?}"))
    }

    fn cust(value: u64) -> Result<PicU<9>, String> {
        PicU::<9>::new(value).map_err(|error| format!("{error:?}"))
    }

    fn key(date: i64, time: i64) -> Result<AuthKey, String> {
        let date = PicS::<5>::new(date).map_err(|error| format!("{error:?}"))?;
        let time = PicS::<9>::new(time).map_err(|error| format!("{error:?}"))?;
        Ok(AuthKey::new(date, time))
    }

    fn xref(acct_id: u64, cust_id: u64) -> Result<CardXrefRecord, String> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(*CARD)),
            cust_id: cust(cust_id)?,
            acct_id: acct(acct_id)?,
            filler: PicX::spaces(),
        })
    }

    fn keys(acct_id: u64, cust_id: u64) -> Result<XrefKeys, String> {
        let record = xref(acct_id, cust_id)?;
        xref_keys(acct(acct_id)?, Lookup::Found(&record)).map_err(|failure| failure.to_string())
    }

    fn account(credit_limit: i64, cash_limit: i64) -> Result<AccountRecord, String> {
        Ok(AccountRecord {
            id: acct(50)?,
            active_status: pic("Y")?,
            curr_bal: Money::ZERO,
            credit_limit: money(credit_limit)?,
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

    /// Name and address lines of a customer, as stored.
    struct Person<'a> {
        first: &'a str,
        middle: &'a str,
        last: &'a str,
        lines: [&'a str; 3],
        state: &'a str,
        zip: &'a str,
        phone: &'a str,
    }

    const ANN: Person<'static> = Person {
        first: "ANN MARIE",
        middle: "LOU",
        last: "DOE",
        lines: ["12 MAIN STREET  APT 4", "BLDG 7", "SPRINGFIELD"],
        state: "IL",
        zip: "62701-1234",
        phone: "(217)555-0100XY",
    };

    fn customer(person: &Person<'_>) -> Result<CustomerRecord, String> {
        let [line_1, line_2, line_3] = person.lines;
        Ok(CustomerRecord {
            id: cust(50)?,
            first_name: pic(person.first)?,
            middle_name: pic(person.middle)?,
            last_name: pic(person.last)?,
            addr_line_1: pic(line_1)?,
            addr_line_2: pic(line_2)?,
            addr_line_3: pic(line_3)?,
            addr_state_cd: pic(person.state)?,
            addr_country_cd: pic("USA")?,
            addr_zip: pic(person.zip)?,
            phone_num_1: pic(person.phone)?,
            phone_num_2: PicX::spaces(),
            ssn: Ssn::new(PicU::<9>::new(123_456_789).map_err(|error| format!("{error:?}"))?),
            govt_issued_id: PicX::spaces(),
            dob_yyyy_mm_dd: PicDate::BLANK,
            eft_account_id: PicX::spaces(),
            pri_card_holder_ind: pic("Y")?,
            fico_credit_score: PicU::<3>::new(700).map_err(|error| format!("{error:?}"))?,
            filler: PicX::spaces(),
        })
    }

    /// The NAV seed's shape: one approval of 25.00, limits and balances zero.
    fn nav_summary() -> Result<PendingAuthSummaryRecord, String> {
        let mut summary = PendingAuthSummaryRecord::initialized(summary_key(acct(50)?), cust(50)?);
        summary.approved_auth_cnt = Count::new(1);
        summary.approved_auth_amt = money(2_500)?;
        Ok(summary)
    }

    /// A detail with these key parts, stored date and time, response code and approved amount.
    fn detail(
        auth_key: AuthKey,
        orig: (&str, &str),
        resp: &str,
        approved_cents: i64,
    ) -> Result<PendingAuthDetailRecord, String> {
        let (orig_date, orig_time) = orig;
        Ok(PendingAuthDetailRecord {
            auth_date_9c: auth_key.date_9c(),
            auth_time_9c: auth_key.time_9c(),
            orig_date: pic(orig_date)?,
            orig_time: pic(orig_time)?,
            card_num: CardNumber::new(PicX::from_array(*CARD)),
            auth_type: pic("SALE")?,
            card_expiry_date: pic("0327")?,
            message_type: pic("0100")?,
            message_source: pic("POS")?,
            auth_id_code: pic("000001")?,
            auth_resp_code: pic(resp)?,
            auth_resp_reason: pic("0000")?,
            processing_code: PicU::ZERO,
            transaction_amt: money(2_500)?,
            approved_amt: money(approved_cents)?,
            merchant_catagory_code: pic("5411")?,
            acqr_country_code: pic("840")?,
            pos_entry_mode: PicU::ZERO,
            merchant_id: pic("MERCHANT0000001")?,
            merchant_name: pic("TEST MERCHANT")?,
            merchant_city: pic("SEATTLE")?,
            merchant_state: pic("WA")?,
            merchant_zip: pic("98101")?,
            transaction_id: pic("TRN000000000001")?,
            match_status: pic("P")?,
            auth_fraud: PicX::spaces(),
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// `count` approved details with ascending keys from (77800, 896999999).
    fn details(count: i64) -> Result<Vec<PendingAuthDetailRecord>, String> {
        (0..count)
            .map(|offset| {
                detail(
                    key(77_800, 896_999_999 + offset)?,
                    ("220718", "103000"),
                    "00",
                    2_500,
                )
            })
            .collect()
    }

    // audit: COPAUS0C-005
    #[test]
    fn a_blank_account_asks_for_one() {
        let blank = (
            "Please enter Acct Id...".to_owned(),
            Some(AuthSummaryField::AcctId),
            400,
            MessageCodes::None,
        );
        for raw in [
            "",
            " ",
            "           ",
            "                    ",
            "\0",
            "\0\0\0\0\0\0\0\0\0\0\0",
        ] {
            let shown_failure = edit_account(raw).map_err(|failure| shown(&failure));
            assert_eq!(shown_failure, Err(blank.clone()), "{raw:?}");
        }
    }

    // audit: COPAUS0C-006
    #[test]
    fn an_account_that_is_not_eleven_digits_is_not_numeric() {
        let not_numeric = (
            "Acct Id must be Numeric ...".to_owned(),
            Some(AuthSummaryField::AcctId),
            400,
            MessageCodes::None,
        );
        for raw in [
            "50",
            "0000000005",
            "000000000500",
            "0000000005O",
            " 0000000050",
            "00000 00050",
            "0000000050 ",
            "-0000000050",
            "+0000000050",
            "\0 ",
            " \0",
            "\u{0660}\u{0661}\u{0662}\u{0663}\u{0664}\u{0665}",
            "0000000005\u{00e9}",
        ] {
            let shown_failure = edit_account(raw).map_err(|failure| shown(&failure));
            assert_eq!(shown_failure, Err(not_numeric.clone()), "{raw:?}");
        }
    }

    // audit: COPAUS0C-008
    #[test]
    fn eleven_digits_are_the_account() -> TestResult {
        assert_eq!(edit_account("00000000050"), Ok(acct(50)?));
        assert_eq!(edit_account("00000000000"), Ok(acct(0)?));
        assert_eq!(edit_account("99999999999"), Ok(acct(99_999_999_999)?));
        Ok(())
    }

    // audit: COPAUS0C-013
    #[test]
    fn the_cross_reference_keys_the_account_and_customer_reads() -> TestResult {
        let record = xref(50, 49)?;
        let keys = xref_keys(acct(50)?, Lookup::Found(&record)).map_err(|f| f.to_string())?;
        assert_eq!(keys.acct_id(), acct(50)?);
        assert_eq!(keys.cust_id(), cust(49)?);
        Ok(())
    }

    // audit: COPAUS0C-014
    // audit: COPAUS0C-015
    // audit: COPAUS0C-056
    #[test]
    fn a_failed_cross_reference_read_ends_the_request() -> TestResult {
        let not_found = xref_keys(acct(50)?, Lookup::NotFound).map_err(|failure| shown(&failure));
        let expected = "Account:00000000050 not found in XREF file. Resp:000000000 Reas:000000000";
        assert_eq!(
            not_found,
            Err((
                expected.to_owned(),
                Some(AuthSummaryField::AcctId),
                404,
                MessageCodes::HaltedH4
            ))
        );
        assert_eq!(expected.chars().count(), 73);
        let failed = xref_keys(acct(50)?, Lookup::Failed).map_err(|failure| shown(&failure));
        let fixed = "Account:00000000050 System error while reading XREF file. Resp:";
        let Err((message, field, status, codes)) = failed else {
            return Err("a failed read gives a failure".to_owned());
        };
        assert_eq!((field, status, codes), (None, 500, MessageCodes::HaltedH4));
        assert_eq!(message.chars().count(), 78);
        assert!(message.starts_with(fixed), "{message}");
        assert!(message.ends_with(" Reas:"), "{message}");
        Ok(())
    }

    // audit: COPAUS0C-016
    // audit: COPAUS0C-017
    // audit: COPAUS0C-018
    // audit: COPAUS0C-019
    #[test]
    fn the_first_failed_master_read_ends_the_request() -> TestResult {
        let keys = keys(50, 49)?;
        let account = account(0, 0)?;
        let customer = customer(&ANN)?;
        let summary = nav_summary()?;
        let found_summary = Lookup::Found(&summary);
        let block = |a, c, s| summary_block(keys, a, c, s).map_err(|failure| shown(&failure));

        let Err(account_missing) = block(Lookup::NotFound, Lookup::NotFound, Lookup::Failed) else {
            return Err("a missing account fails".to_owned());
        };
        let expected = "Account:00000000050 not found in ACCT file. Resp:000000000 Reas:000000000";
        assert_eq!(
            account_missing,
            (expected.to_owned(), None, 404, MessageCodes::HaltedH4)
        );
        assert_eq!(expected.chars().count(), 73);

        let Err(account_failed) = block(Lookup::Failed, Lookup::Found(&customer), found_summary)
        else {
            return Err("a failed account read fails".to_owned());
        };
        assert_eq!(account_failed.0.chars().count(), 78);
        assert!(account_failed.0.starts_with(
            "Account:00000000050 System error while reading ACCT file. Resp:000000000 Reas:"
        ));
        assert_eq!(account_failed.2, 500);

        let Err(customer_missing) = block(Lookup::Found(&account), Lookup::NotFound, found_summary)
        else {
            return Err("a missing customer fails".to_owned());
        };
        let expected = "Customer:000000049 not found in CUST file. Resp:000000000 Reas:000000000";
        assert_eq!(
            customer_missing,
            (expected.to_owned(), None, 404, MessageCodes::HaltedH4)
        );
        assert_eq!(expected.chars().count(), 72);

        let Err(customer_failed) = block(Lookup::Found(&account), Lookup::Failed, found_summary)
        else {
            return Err("a failed customer read fails".to_owned());
        };
        let fixed = "Customer:000000049 System error while reading CUST file. Resp:000000000 Reas:";
        assert_eq!(customer_failed.0.chars().count(), 78);
        assert!(
            customer_failed.0.starts_with(fixed),
            "{}",
            customer_failed.0
        );
        assert_eq!(
            (customer_failed.2, customer_failed.3),
            (500, MessageCodes::HaltedH4)
        );
        Ok(())
    }

    // audit: COPAUS0C-030
    #[test]
    fn a_failed_summary_read_ends_the_request() -> TestResult {
        let account = account(0, 0)?;
        let customer = customer(&ANN)?;
        let failed = summary_block(
            keys(50, 50)?,
            Lookup::Found(&account),
            Lookup::Found(&customer),
            Lookup::Failed,
        )
        .map_err(|failure| shown(&failure));
        let Err((message, field, status, codes)) = failed else {
            return Err("a failed summary read fails".to_owned());
        };
        assert!(message.starts_with(" System error while reading AUTH Summary: Code:"));
        assert_eq!(message.chars().count(), 49);
        assert_eq!((field, status, codes), (None, 500, MessageCodes::HaltedH4));
        Ok(())
    }

    // audit: COPAUS0C-020
    // audit: COPAUS0C-021
    // audit: COPAUS0C-022
    // audit: COPAUS0C-023
    // audit: COPAUS0C-024
    // audit: COPAUS0C-025
    // audit: COPAUS0C-028
    // audit: COPAUS0C-052
    #[test]
    fn the_block_shows_the_customer_limits_and_summary() -> TestResult {
        let account = account(123_456, 1_234_567_890)?;
        let customer = customer(&ANN)?;
        let mut summary = nav_summary()?;
        summary.declined_auth_cnt = Count::new(1234);
        summary.credit_balance = money(-525)?;
        summary.cash_balance = money(0)?;
        summary.declined_auth_amt = money(12_345_678)?;
        let block = summary_block(
            keys(50, 50)?,
            Lookup::Found(&account),
            Lookup::Found(&customer),
            Lookup::Found(&summary),
        )
        .map_err(|failure| failure.to_string())?;
        assert_eq!(block.customer_id, "000000050");
        assert_eq!(block.name, "ANN L DOE                ");
        assert_eq!(block.address_line_1, "12 MAIN STREET,BLDG 7    ");
        assert_eq!(block.address_line_2, "SPRINGFIELD,IL,62701     ");
        assert_eq!(block.phone, "(217)555-0100");
        assert_eq!(block.credit_limit, "     1234.56");
        assert_eq!(block.cash_limit, " 45678.90");
        assert_eq!(block.summary, SummaryState::Found);
        assert_eq!(block.approved_count, "001");
        assert_eq!(block.declined_count, "234");
        assert_eq!(block.credit_balance, "-       5.25");
        assert_eq!(block.cash_balance, "     0.00");
        assert_eq!(block.approved_amount, "    25.00 ");
        assert_eq!(block.declined_amount, " 23456.78 ");
        Ok(())
    }

    // audit: COPAUS0C-021
    #[test]
    fn the_name_keeps_first_words_and_the_middle_initial() -> TestResult {
        let cases = [
            ("ANN", "", "DOE", "ANN   DOE"),
            (" ANN", "LOU", "DOE", " L DOE"),
            ("ANN", "LOU", "VAN DER BERG", "ANN L VAN"),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXY",
                "Z",
                "DOE",
                "ABCDEFGHIJKLMNOPQRSTUVWXY",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTU",
                "M",
                "LAST",
                "ABCDEFGHIJKLMNOPQRSTU M L",
            ),
        ];
        let account = account(0, 0)?;
        for (first, middle, last, name) in cases {
            let person = Person {
                first,
                middle,
                last,
                ..ANN
            };
            let customer = customer(&person)?;
            let block = summary_block(
                keys(50, 50)?,
                Lookup::Found(&account),
                Lookup::Found(&customer),
                Lookup::NotFound,
            )
            .map_err(|failure| failure.to_string())?;
            assert_eq!(
                block.name,
                format!("{name:<25}"),
                "{first:?} {middle:?} {last:?}"
            );
        }
        Ok(())
    }

    // audit: COPAUS0C-022
    // audit: COPAUS0C-023
    // audit: COPAUS0C-024
    #[test]
    fn the_address_rows_stop_at_two_spaces() -> TestResult {
        let long = "A-VERY-LONG-ADDRESS-LINE-WITHOUT-DOUBLE-SPACES.123";
        let cases = [
            (["1 ELM", "", "TOWN"], ("1 ELM,", "TOWN,IL,62701")),
            (
                ["  1 ELM", "SUITE 2  X", "  TOWN"],
                (",SUITE 2", ",IL,62701"),
            ),
            ([long, "B", long], (long, long)),
            (
                ["PO BOX 7", "FLOOR 3", "NEW YORK CITY  NY"],
                ("PO BOX 7,FLOOR 3", "NEW YORK CITY,IL,62701"),
            ),
        ];
        let account = account(0, 0)?;
        for (lines, (row_1, row_2)) in cases {
            let person = Person { lines, ..ANN };
            let customer = customer(&person)?;
            let block = summary_block(
                keys(50, 50)?,
                Lookup::Found(&account),
                Lookup::Found(&customer),
                Lookup::NotFound,
            )
            .map_err(|failure| failure.to_string())?;
            let fit = |text: &str| format!("{:<25}", text.chars().take(25).collect::<String>());
            assert_eq!(block.address_line_1, fit(row_1), "{lines:?}");
            assert_eq!(block.address_line_2, fit(row_2), "{lines:?}");
            assert_eq!(block.phone.chars().count(), 13);
        }
        Ok(())
    }

    // audit: COPAUS0C-025
    // audit: COPAUS0C-028
    #[test]
    fn amounts_and_counts_use_the_display_pictures() -> TestResult {
        let cases = [
            (0, "        0.00", "     0.00"),
            (-525, "-       5.25", "-    5.25"),
            (123_456, "     1234.56", "  1234.56"),
            (12_345_678, "   123456.78", " 23456.78"),
        ];
        let customer = customer(&ANN)?;
        for (cents, credit, cash) in cases {
            let account = account(cents, cents)?;
            let block = summary_block(
                keys(50, 50)?,
                Lookup::Found(&account),
                Lookup::Found(&customer),
                Lookup::NotFound,
            )
            .map_err(|failure| failure.to_string())?;
            assert_eq!(block.credit_limit, credit, "{cents}");
            assert_eq!(block.cash_limit, cash, "{cents}");
        }
        for (count, shown) in [
            (7, "007"),
            (1234, "234"),
            (-5, "005"),
            (0, "000"),
            (i16::MIN, "768"),
        ] {
            let mut summary = nav_summary()?;
            summary.approved_auth_cnt = Count::new(count);
            let account = account(0, 0)?;
            let block = summary_block(
                keys(50, 50)?,
                Lookup::Found(&account),
                Lookup::Found(&customer),
                Lookup::Found(&summary),
            )
            .map_err(|failure| failure.to_string())?;
            assert_eq!(block.approved_count, shown, "{count}");
        }
        Ok(())
    }

    // audit: COPAUS0C-029
    #[test]
    fn without_a_summary_the_figures_are_zero_filled() -> TestResult {
        let account = account(250_000, 50_000)?;
        let customer = customer(&ANN)?;
        let block = summary_block(
            keys(50, 50)?,
            Lookup::Found(&account),
            Lookup::Found(&customer),
            Lookup::NotFound,
        )
        .map_err(|failure| failure.to_string())?;
        assert_eq!(block.summary, SummaryState::NotFound);
        assert_eq!(block.credit_limit, "     2500.00");
        assert_eq!(block.cash_limit, "   500.00");
        assert_eq!(block.approved_count, "000");
        assert_eq!(block.declined_count, "000");
        assert_eq!(block.credit_balance, "000000000000");
        assert_eq!(block.cash_balance, "000000000");
        assert_eq!(block.approved_amount, "0000000000");
        assert_eq!(block.declined_amount, "0000000000");
        assert_eq!(AuthPage::without_details().rows, Vec::new());
        assert_eq!(AuthPage::without_details().next_page, NextPage::Unreported);
        Ok(())
    }

    // audit: COPAUS0C-027
    #[test]
    fn the_summary_is_keyed_by_the_signed_account() -> TestResult {
        assert_eq!(summary_key(acct(50)?).get(), 50);
        assert_eq!(summary_key(acct(99_999_999_999)?).get(), 99_999_999_999);
        Ok(())
    }

    // audit: COPAUS0C-032
    // audit: COPAUS0C-033
    // audit: COPAUS0C-034
    #[test]
    fn a_row_shows_the_detail_as_stored() -> TestResult {
        let approved = detail(key(77_800, 896_999_999)?, ("240131", "103045"), "00", 2_500)?;
        let row = row_from(&approved);
        assert_eq!(row.key, key(77_800, 896_999_999)?);
        assert_eq!(row.transaction_id, "TRN000000000001 ");
        assert_eq!(row.date, "01/31/24");
        assert_eq!(row.time, "10:30:45");
        assert_eq!(row.auth_type, "SALE");
        assert_eq!(row.approval.as_str(), "A");
        assert_eq!(row.match_status, "P");
        assert_eq!(row.amount, "       25.00");

        let mut declined = detail(key(77_800, 1)?, ("241399", "2 3 4 "), "05", 0)?;
        declined.match_status = pic("D")?;
        let row = row_from(&declined);
        assert_eq!(row.date, "13/99/24");
        assert_eq!(row.time, "2 :3 :4 ");
        assert_eq!(row.approval.as_str(), "D");
        assert_eq!(row.match_status, "D");
        assert_eq!(row.amount, "        0.00");

        let large = detail(key(1, 1)?, ("220718", "103000"), "00", 123_456_789_012)?;
        assert_eq!(row_from(&large).amount, " 34567890.12");
        let negative = detail(key(1, 2)?, ("220718", "103000"), "00", -100)?;
        assert_eq!(row_from(&negative).amount, "-       1.00");
        Ok(())
    }

    // audit: COPAUS0C-055
    #[test]
    fn no_row_or_block_holds_a_card_number() -> TestResult {
        let record = xref(50, 50)?;
        let keys = xref_keys(acct(50)?, Lookup::Found(&record)).map_err(|f| f.to_string())?;
        let account = account(0, 0)?;
        let customer = customer(&ANN)?;
        let summary = nav_summary()?;
        let block = summary_block(
            keys,
            Lookup::Found(&account),
            Lookup::Found(&customer),
            Lookup::Found(&summary),
        )
        .map_err(|failure| failure.to_string())?;
        let list = page(PageRequest::First, &details(6)?, DEFAULT_PAGE_SIZE);
        let card = String::from_utf8_lossy(CARD).into_owned();
        let last4 = card.chars().skip(12).collect::<String>();
        for shown_text in [
            format!("{keys:?}"),
            format!("{block:?}"),
            format!("{list:?}"),
        ] {
            assert!(!shown_text.contains(&card), "a full card number is shown");
            assert!(
                !shown_text.contains(&last4),
                "a redacted card number is shown"
            );
        }
        Ok(())
    }

    // audit: COPAUS0C-034
    #[test]
    fn a_key_renders_as_sixteen_digits_and_parses_back() -> TestResult {
        let cases = [
            (77_800, 896_999_999, "0778000896999999"),
            (0, 0, "0000000000000000"),
            (99_999, 999_999_999, "0999990999999999"),
            (-99_999, -999_999_999, "1999991999999999"),
            (-1, 5, "1000010000000005"),
            (7, -1, "0000071000000001"),
        ];
        for (date, time, text) in cases {
            let auth_key = key(date, time)?;
            assert_eq!(auth_key.render(), text);
            assert_eq!(auth_key.render().len(), AuthKey::RENDERED_WIDTH);
            assert_eq!(AuthKey::parse(text), Ok(auth_key));
            assert_eq!(auth_key.date_9c().get(), date);
            assert_eq!(auth_key.time_9c().get(), time);
        }
        assert_eq!(AuthKey::lowest(), key(-99_999, -999_999_999)?);
        assert!(key(77_800, 1)? < key(77_800, 2)? && key(77_800, 999)? < key(77_801, 0)?);
        Ok(())
    }

    #[test]
    fn malformed_key_text_is_rejected() {
        for text in [
            "",
            "077800089699999",
            "07780008969999990",
            "2778000896999999",
            "0778002896999999",
            "1000000000000001",
            "0000001000000000",
            "077800089699999X",
            "+778000896999999",
            " 778000896999999",
            "07780008969999\u{e9}",
            "\u{0660}778000896999999",
        ] {
            assert_eq!(AuthKey::parse(text), Err(AuthKeyError), "{text:?}");
        }
        assert!(!AuthKeyError.to_string().is_empty());
    }

    // audit: COPAUS0C-031
    // audit: COPAUS0C-036
    #[test]
    fn the_first_page_reports_the_flag_only_when_full() -> TestResult {
        let request = PageRequest::new(Some(Direction::Forward), None);
        assert_eq!(request, PageRequest::First);
        assert_eq!(request.start_key(), AuthKey::lowest());
        assert_eq!(request.fetch_limit(DEFAULT_PAGE_SIZE), 6);
        let cases = [
            (0, 0, NextPage::Unreported),
            (3, 3, NextPage::Unreported),
            (5, 5, NextPage::No),
            (6, 5, NextPage::Yes),
        ];
        for (available, shown, flag) in cases {
            let rows = details(available)?;
            let list = page(request, &rows, DEFAULT_PAGE_SIZE);
            assert_eq!(list.rows.len(), shown, "{available}");
            assert_eq!(list.next_page, flag, "{available}");
            let expected: Vec<AuthKey> = rows.iter().take(shown).map(AuthKey::of).collect();
            let keys: Vec<AuthKey> = list.rows.iter().map(|row| row.key).collect();
            assert_eq!(keys, expected);
            assert_eq!(list.first_key(), expected.first().copied());
            assert_eq!(list.last_key(), expected.last().copied());
        }
        let rows = details(3)?;
        assert_eq!(page(request, &rows, 2).next_page, NextPage::Yes);
        assert_eq!(page(request, &rows, 3).next_page, NextPage::No);
        assert_eq!(page(request, &rows, 0).next_page, NextPage::Yes);
        assert_eq!(page(request, &[], 0).next_page, NextPage::No);
        Ok(())
    }

    // audit: COPAUS0C-041
    #[test]
    fn forward_paging_shows_the_rows_after_the_key() -> TestResult {
        let rows = details(8)?;
        let anchor = AuthKey::of(rows.first().ok_or("no detail")?);
        let request = PageRequest::new(Some(Direction::Forward), Some(anchor));
        assert_eq!(request, PageRequest::After(anchor));
        assert_eq!(PageRequest::new(None, Some(anchor)), request);
        assert_eq!(request.start_key(), anchor);
        assert_eq!(request.fetch_limit(DEFAULT_PAGE_SIZE), 7);
        let list = page(request, &rows, DEFAULT_PAGE_SIZE);
        let keys: Vec<AuthKey> = list.rows.iter().map(|row| row.key).collect();
        let expected: Vec<AuthKey> = rows.iter().skip(1).take(5).map(AuthKey::of).collect();
        assert_eq!(keys, expected);
        assert_eq!(list.next_page, NextPage::Yes);

        let tail = rows.get(3..).ok_or("too few details")?;
        let tail_anchor = AuthKey::of(tail.first().ok_or("no detail")?);
        let list = page(PageRequest::After(tail_anchor), tail, DEFAULT_PAGE_SIZE);
        assert_eq!(list.rows.len(), 4);
        assert_eq!(list.next_page, NextPage::Unreported);

        let missing = key(1, 1)?;
        let list = page(PageRequest::After(missing), &rows, DEFAULT_PAGE_SIZE);
        assert_eq!(list, AuthPage::without_details());
        assert_eq!(list.last_key(), None);
        let list = page(PageRequest::After(anchor), &[], DEFAULT_PAGE_SIZE);
        assert_eq!(list, AuthPage::without_details());
        Ok(())
    }

    // audit: COPAUS0C-047
    #[test]
    fn backward_paging_shows_the_rows_from_the_key() -> TestResult {
        let rows = details(6)?;
        let anchor = AuthKey::of(rows.first().ok_or("no detail")?);
        let request = PageRequest::new(Some(Direction::Backward), Some(anchor));
        assert_eq!(request, PageRequest::From(anchor));
        assert_eq!(request.start_key(), anchor);
        assert_eq!(request.fetch_limit(DEFAULT_PAGE_SIZE), 6);
        let list = page(request, &rows, DEFAULT_PAGE_SIZE);
        let keys: Vec<AuthKey> = list.rows.iter().map(|row| row.key).collect();
        let expected: Vec<AuthKey> = rows.iter().take(5).map(AuthKey::of).collect();
        assert_eq!(keys, expected);
        assert_eq!(list.first_key(), Some(anchor));
        assert_eq!(list.next_page, NextPage::Yes);

        let list = page(PageRequest::From(key(1, 1)?), &rows, DEFAULT_PAGE_SIZE);
        assert_eq!(list, AuthPage::without_details());
        let list = page(PageRequest::From(anchor), &[], DEFAULT_PAGE_SIZE);
        assert_eq!(list, AuthPage::without_details());
        assert_eq!(
            PageRequest::After(anchor).fetch_limit(usize::MAX),
            usize::MAX
        );
        Ok(())
    }

    // audit: COPAUS0C-037
    // audit: COPAUS0C-038
    #[test]
    fn a_failed_detail_query_names_its_first_call() -> TestResult {
        let anchor = key(77_800, 896_999_999)?;
        let first = shown(&detail_failure(PageRequest::First));
        assert!(
            first
                .0
                .starts_with(" System error while reading AUTH Details: Code:")
        );
        assert_eq!(first.0.chars().count(), 49);
        assert_eq!(
            (first.1, first.2, first.3),
            (None, 500, MessageCodes::HaltedH4)
        );
        for request in [PageRequest::After(anchor), PageRequest::From(anchor)] {
            let repositioned = shown(&detail_failure(request));
            assert!(
                repositioned
                    .0
                    .starts_with(" System error while repos. AUTH Details: Code:")
            );
            assert_eq!(repositioned.0.chars().count(), 48);
            assert_eq!(repositioned.2, 500);
        }
        assert_eq!(FailureClass::Store.http_status(), 500);
        let failure = detail_failure(PageRequest::First);
        assert_eq!(failure.to_string(), failure.message());
        Ok(())
    }

    // audit: COPAUS0C-008
    // audit: COPAUS0C-028
    // audit: COPAUS0C-031
    #[test]
    fn the_nav_seed_shape_shows_one_approved_authorization() -> TestResult {
        let acct_id = edit_account("00000000050").map_err(|failure| failure.to_string())?;
        let record = xref(50, 50)?;
        let keys = xref_keys(acct_id, Lookup::Found(&record)).map_err(|f| f.to_string())?;
        let account = account(0, 0)?;
        let customer = customer(&ANN)?;
        let summary = nav_summary()?;
        let block = summary_block(
            keys,
            Lookup::Found(&account),
            Lookup::Found(&customer),
            Lookup::Found(&summary),
        )
        .map_err(|failure| failure.to_string())?;
        assert_eq!(block.summary, SummaryState::Found);
        assert_eq!(
            (block.approved_count.as_str(), block.declined_count.as_str()),
            ("001", "000")
        );
        assert_eq!(block.approved_amount, "    25.00 ");
        assert_eq!(block.declined_amount, "     0.00 ");
        let rows = details(1)?;
        let list = page(PageRequest::First, &rows, DEFAULT_PAGE_SIZE);
        assert_eq!(list.next_page, NextPage::Unreported);
        let row = list.rows.first().ok_or("no row")?;
        assert_eq!(row.key.render(), "0778000896999999");
        assert_eq!(
            (row.date.as_str(), row.time.as_str()),
            ("07/18/22", "10:30:00")
        );
        assert_eq!(row.approval.as_str(), "A");
        assert_eq!(row.amount, "       25.00");
        Ok(())
    }
}
