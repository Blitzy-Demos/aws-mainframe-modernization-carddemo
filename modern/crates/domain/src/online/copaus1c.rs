//! COPAUS1C, the authorization detail of transaction `CPVD` (map `COPAU1A`): the rules of
//! endpoint 22, `GET /api/v1/authorizations/{acct_id}/{auth_key}` (ENTER and F8), and of
//! endpoint 23, `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud` (F5; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COPAUS1C.md`: the request edit
//! (COPAUS1C-006), the read of the summary and the detail and its outcomes (COPAUS1C-008 to
//! COPAUS1C-011), the detail display (COPAUS1C-012 to COPAUS1C-023), F8 (COPAUS1C-024 to
//! COPAUS1C-027), F5 (COPAUS1C-028 to COPAUS1C-034), the message row (COPAUS1C-035) and the
//! halted IMS codes (COPAUS1C-038). The store half of F5, the `AUTHFRDS` row and the status
//! COPAUS2C hands back, is `super::copaus2c`'s.
//!
//! The `client` rows stay with the screen: the entries with and without a COMMAREA
//! (COPAUS1C-001, COPAUS1C-002), the keys the screen sends and the state it keeps
//! (COPAUS1C-003), F3 back to `/authorizations` (COPAUS1C-004), the invalid-key message
//! (COPAUS1C-005), the cursor and colours (COPAUS1C-036) and the header rows (COPAUS1C-037).
//!
//! # Flow
//!
//! `api::services::copaus1c` performs the reads and writes; this module decides every outcome
//! and does no I/O.
//!
//! - Every request: [`edit`] edits the two path segments into a [`DetailKey`]. The service
//!   reads the summary by the account and, when it is found, the detail by the key, and hands
//!   the outcome over as a [`DetailLookup`].
//! - ENTER: [`detail`] builds the [`AuthDetailView`].
//! - F8: [`DetailLookup::record`] takes the current detail; the service reads the first detail
//!   of the account above its key, a [`NextLookup`], and [`next`] builds the view.
//! - F5, inside one `PostgreSQL` transaction:
//!   1. [`DetailLookup::record`] takes the detail and [`toggle`] flips its fraud flag into a
//!      [`FraudToggle`].
//!   2. [`FraudToggle::with_report_date`] stamps it with `super::copaus2c::report_date`, the
//!      date COPAUS2C moves into the COMMAREA copy COPAUS1C writes back (COPAUS2C-001).
//!   3. [`fraud_customer`] takes the customer of the account's cross-reference
//!      (X-COPAUS1C-4); `super::copaus2c::fraud_row` builds the `AUTHFRDS` row from
//!      [`FraudToggle::updated`] and [`FraudToggle::action`], and the insert or its `-803`
//!      update ends in a `super::copaus2c::FraudOutcome`.
//!   4. Only when that outcome [`is_success`](super::copaus2c::FraudOutcome::is_success)
//!      does the service replace the detail row with [`FraudToggle::updated`], in the same
//!      transaction, and report a [`SegmentUpdate`].
//!   5. [`toggle_outcome`] gives the [`FraudToggleResult`]: [`FraudToggleResult::commits`] says
//!      whether the service commits (`TAKE-SYNCPOINT`) or rolls back (`ROLL-BACK`), and the
//!      result carries the response.
//!
//! The COBOL performs every read even after one fails or finds nothing, and goes on to flip,
//! link and replace on F5 or to read the next detail on F8; endpoints 22 and 23 stop at the
//! first failed or missing read, return that read's outcome alone and write nothing
//! (escalation candidate X-COPAUS1C-6).
//!
//! # Display values
//!
//! The view carries each map field as the string the program moves into it, padded with spaces
//! to the field's width, so the screen formats nothing (COPAUS1C-012 to COPAUS1C-023). Auth Date
//! and Auth Time are such strings and not typed values: `PA-AUTH-ORIG-DATE` and
//! `PA-AUTH-ORIG-TIME` are `PIC X(06)`, outside the `X(10)` dates and `X(26)` timestamps Typed
//! Dates and Timestamps covers, and the program copies their bytes with no calendar check, so a
//! stored `241399` shows `13/99/24`, which no date type holds (COPAUS1C-013, X-COPAUS1C-5).
//!
//! # Halts
//!
//! - `HALTED(H4)`: the 2-character IMS status the COBOL strings after each system-error text
//!   has no `PostgreSQL` counterpart (COPAUS1C-038). The texts carry two spaces, the
//!   alphanumeric field's initialised value, in its place, and [`AuthDetailFailure::codes`]
//!   reports [`MessageCodes::HaltedH4`]; those characters are no parity result, the fixed text
//!   and the status stay assertable.
//! - `HALTED(H3)`: the Db2 codes in the message COPAUS2C hands back (COPAUS2C-010,
//!   COPAUS2C-011), reported as [`MessageCodes::HaltedH3`] (COPAUS1C-031).
//! - `HALTED(X-COPAUS1C-H1)`, `HALTED(X-COPAUS1C-H2)`: the fields the COBOL shows after a missed
//!   read, F8 and F5 after a missed or failed current read, and F5's whole success branch
//!   (COPAUS1C-009, COPAUS1C-027, COPAUS1C-029, COPAUS1C-032). The replacement answers them as
//!   stated at each function, and none of those answers is a parity result.
//! - `HALTED(X-COPAUS1C-H3)`: the detail fields the terminal shows after F8 at the last
//!   authorization; the view keeps the current authorization, no parity result (COPAUS1C-025).
//!
//! # Cardholder data and totality
//!
//! The card number travels only as a [`CardNumber`], whose `Debug`, `Display` and `Serialize`
//! emit twelve `*` and its last four digits, and no message of this program holds one
//! (COPAUS1C-012, E32). Every function accepts any input, path text of any length included, and
//! returns a value or a typed failure; nothing indexes, slices, unwraps or panics, and amounts are
//! only edited through `copybook::edited`, with no arithmetic on money (Total Parsers Over
//! Untrusted Input, R1, R18).

use core::fmt;
use std::iter;

use copybook::edited::EditedPicture;
use copybook::pic::{PicU, PicX};

use super::copaus0c::{Approval, AuthKey, FailureClass, Lookup};
use super::copaus2c::{FraudAction, FraudOutcome, with_report_date};
use crate::records::pending_auth_detail::PendingAuthDetailRecord;
use crate::sensitive::CardNumber;

/// `WS-DECLINE-REASON-TABLE`, the ten `PIC X(20)` values redefined as `DECL-CODE PIC X(4)` and
/// `DECL-DESC PIC X(16)` (`COPAUS1C.cbl:56-73`; COPAUS1C-017): each code with its description as
/// written, the spellings kept and the padding to 16 bytes left out. The codes ascend, as
/// `ASCENDING KEY IS DECL-CODE` declares, so a binary search over them is `SEARCH ALL`.
pub const DECLINE_REASONS: [(&str, &str); 10] = [
    ("0000", "APPROVED"),
    ("3100", "INVALID CARD"),
    ("4100", "INSUFFICNT FUND"),
    ("4200", "CARD NOT ACTIVE"),
    ("4300", "ACCOUNT CLOSED"),
    ("4400", "EXCED DAILY LMT"),
    ("5100", "CARD FRAUD"),
    ("5200", "MERCHANT FRAUD"),
    ("5300", "LOST CARD"),
    ("9000", "UNKNOWN"),
];

/// `PROCESS-PF8-KEY`, no detail after the current one (`COPAUS1C.cbl:283`; COPAUS1C-025).
pub const ALREADY_AT_LAST: &str = "Already at the last Authorization...";

/// `UPDATE-AUTH-DETAILS`, the replace succeeded for a flag `F` (`COPAUS1C.cbl:537`;
/// COPAUS1C-032).
pub const AUTH_MARKED_FRAUD: &str = "AUTH MARKED FRAUD...";

/// `UPDATE-AUTH-DETAILS`, the replace succeeded for a flag `R` (`COPAUS1C.cbl:535`;
/// COPAUS1C-032).
pub const AUTH_FRAUD_REMOVED: &str = "AUTH FRAUD REMOVED...";

/// `READ-AUTH-RECORD`, the summary `GU` failed: the text before the IMS status, its leading
/// space included (`COPAUS1C.cbl:456`; COPAUS1C-010).
pub const SUMMARY_READ_ERROR: &str = " System error while reading Auth Summary: Code:";

/// `READ-AUTH-RECORD`, the detail `GNP` failed (`COPAUS1C.cbl:482`; COPAUS1C-011).
pub const DETAIL_READ_ERROR: &str = " System error while reading Auth Details: Code:";

/// `READ-NEXT-AUTH-RECORD`, the unqualified `GNP` failed (`COPAUS1C.cbl:511`; COPAUS1C-026).
pub const NEXT_READ_ERROR: &str = " System error while reading next Auth: Code:";

/// `UPDATE-AUTH-DETAILS`, the replace failed: the literal with the two vertical bars it ends
/// with (`COPAUS1C.cbl:545`; COPAUS1C-033).
pub const FRAUD_TAGGING_ERROR: &str = " System error while FRAUD Tagging, ROLLBACK||";

/// `HALTED(H4)`, COPAUS1C-038: the `IMS-RETURN-CODE PIC X(02)` positions hold an IMS status with
/// no counterpart. Interim handling, not a parity result: the alphanumeric field's initialised
/// value, two spaces.
const HALTED_IMS_STATUS: &str = "  ";

/// `AUTHRSNO PIC X(20)` (COPAUS1C-017, COPAUS1C-018).
const REASON_WIDTH: usize = 20;

/// `AUTHRSNO(6:)`: the 15 description bytes the reason field holds after the code and `-`
/// (COPAUS1C-017).
const REASON_DESC_WIDTH: usize = 15;

/// The code `AT END` shows: `MOVE '9999' TO AUTHRSNO` (COPAUS1C-018).
const UNKNOWN_REASON_CODE: &[u8; 4] = b"9999";

/// The text `AT END` moves to `AUTHRSNO(6:)` (COPAUS1C-018).
const UNKNOWN_REASON_DESC: &str = "ERROR";

/// `AUTHDTO PIC X(10)` and `AUTHTMO PIC X(10)` (COPAUS1C-013, COPAUS1C-014).
const DATE_TIME_WIDTH: usize = 10;

/// `AUTHCDO PIC X(6)` (COPAUS1C-019).
const AUTH_CODE_WIDTH: usize = 6;

/// `POSEMDO PIC X(4)` (COPAUS1C-020).
const POS_ENTRY_MODE_WIDTH: usize = 4;

/// `AUTHSRCO PIC X(10)` (COPAUS1C-020).
const SOURCE_WIDTH: usize = 10;

/// `CRDEXPO PIC X(5)` (COPAUS1C-020).
const CARD_EXPIRY_WIDTH: usize = 5;

/// `AUTHTYPO PIC X(14)` (COPAUS1C-021).
const AUTH_TYPE_WIDTH: usize = 14;

/// `AUTHFRDO PIC X(10)` (COPAUS1C-022).
const FRAUD_STATUS_WIDTH: usize = 10;

/// `MERNAMEO PIC X(25)` and `MERCITYO PIC X(25)` (COPAUS1C-023).
const MERCHANT_TEXT_WIDTH: usize = 25;

/// `MERZIPO PIC X(10)` (COPAUS1C-023).
const MERCHANT_ZIP_WIDTH: usize = 10;

/// `AUTHRSNO`, `SEARCH ALL WS-DECLINE-REASON-TAB` over `PA-AUTH-RESP-REASON`: the Resp Reason
/// field, 20 characters (`COPAUS1C.cbl:319-328`; COPAUS1C-017, COPAUS1C-018).
///
/// For one of the ten codes of [`DECLINE_REASONS`], the stored reason, `-` at byte 5 and from
/// byte 6 the first 15 of the 16 description bytes, so `3100` gives `3100-INVALID CARD` padded
/// with spaces to 20; no description is longer than 15 characters. For any other four bytes,
/// spaces and lower-case letters included, `AT END` moves `9999` to the field, `-` to byte 5 and
/// `ERROR` from byte 6: `9999-ERROR` padded to 20, whatever reason is stored.
#[must_use]
pub fn reason_text(code: &PicX<4>) -> String {
    let code = code.as_bytes();
    let entry = DECLINE_REASONS
        .binary_search_by(|(decl_code, _)| decl_code.as_bytes().cmp(code.as_slice()))
        .ok()
        .and_then(|index| DECLINE_REASONS.get(index));
    let (shown_code, description) = match entry {
        Some((_, description)) => (code, *description),
        None => (UNKNOWN_REASON_CODE, UNKNOWN_REASON_DESC),
    };
    // `MOVE .. TO AUTHRSNO`, `MOVE '-' TO AUTHRSNO(5:1)`, `MOVE .. TO AUTHRSNO(6:)`: the code,
    // the dash, then the description cut or padded to the 15 bytes left.
    let reason: Vec<u8> = shown_code
        .iter()
        .copied()
        .chain(iter::once(b'-'))
        .chain(
            description
                .bytes()
                .chain(iter::repeat(b' '))
                .take(REASON_DESC_WIDTH),
        )
        .collect();
    padded(&reason, REASON_WIDTH)
}

/// `bytes` moved to an alphanumeric field of `width` characters: left-justified, padded with
/// spaces on the right and cut on the right when longer, one character per byte.
fn padded(bytes: &[u8], width: usize) -> String {
    bytes
        .iter()
        .map(|&byte| char::from(byte))
        .chain(iter::repeat(' '))
        .take(width)
        .collect()
}

/// `bytes`, one character each.
fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Whether a failure message embeds code positions whose COBOL values are halted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageCodes {
    /// The message is empty or fixed text: every character is assertable.
    None,
    /// The message is COPAUS2C's, whose `SQLCODE` and `SQLSTATE` positions are Db2's,
    /// `HALTED(H3)` (COPAUS1C-031, COPAUS2C-010, COPAUS2C-011).
    HaltedH3,
    /// The message ends with an IMS status position, `HALTED(H4)` (COPAUS1C-038).
    HaltedH4,
}

/// A response COPAUS1C sends without an authorization: the message, its status class and
/// whether it embeds halted codes (COPAUS1C-006, COPAUS1C-009 to COPAUS1C-011, COPAUS1C-026,
/// COPAUS1C-031, COPAUS1C-033).
///
/// The message is the program's text as the 78-character message row shows it, with no
/// padding, or empty where the COBOL sets none; it holds no card number and no stored value
/// (COPAUS1C-012). The response carries no detail field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthDetailFailure {
    message: String,
    class: FailureClass,
    codes: MessageCodes,
}

impl AuthDetailFailure {
    /// A failure with an empty message row.
    const fn blank(class: FailureClass) -> Self {
        Self {
            message: String::new(),
            class,
            codes: MessageCodes::None,
        }
    }

    /// A failed DL/I call: `text`, then the IMS status `DELIMITED BY SIZE`, status 500.
    fn ims(text: &str) -> Self {
        let mut message = String::with_capacity(text.len() + HALTED_IMS_STATUS.len());
        message.push_str(text);
        message.push_str(HALTED_IMS_STATUS);
        Self {
            message,
            class: FailureClass::Store,
            codes: MessageCodes::HaltedH4,
        }
    }

    /// COPAUS2C reported the failure `outcome`: `MOVE WS-FRD-ACT-MSG TO WS-MESSAGE`, all 50
    /// bytes of the field, status 500 (COPAUS1C-031). Only [`toggle_outcome`] calls it, and only
    /// for a failed outcome, whose code positions are Db2's, `HALTED(H3)`.
    fn fraud(outcome: FraudOutcome) -> Self {
        Self {
            message: text(outcome.message().as_bytes()),
            class: FailureClass::Store,
            codes: MessageCodes::HaltedH3,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The status class: 400, 404 or 500.
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

impl fmt::Display for AuthDetailFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthDetailFailure {}

/// The authorization a request names: the account, `CDEMO-ACCT-ID PIC 9(11)`, and the detail's
/// `PA-AUTHORIZATION-KEY`, `CDEMO-CPVD-PAU-SELECTED PIC X(08)` (COPAUS1C-006, COPAUS1C-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DetailKey {
    acct_id: PicU<11>,
    auth_key: AuthKey,
}

impl DetailKey {
    /// The account. The authorization tables key it as `PicS::<11>::from(acct_id)`
    /// (COPAUS1C-008).
    #[must_use]
    pub const fn acct_id(self) -> PicU<11> {
        self.acct_id
    }

    /// The detail's key, `PAUT9CTS`.
    #[must_use]
    pub const fn auth_key(self) -> AuthKey {
        self.auth_key
    }
}

/// `PROCESS-ENTER-KEY`'s test before any read, `CDEMO-ACCT-ID IS NUMERIC AND
/// CDEMO-CPVD-PAU-SELECTED NOT = SPACES AND LOW-VALUES` (`COPAUS1C.cbl:208-216`; COPAUS1C-006,
/// COPAUS1C-009).
///
/// `acct_id` is the path segment as received and must be exactly 11 ASCII digits, the numeric
/// test over `PIC 9(11)`. `auth_key` is blank when it is empty, all spaces or all low-values; any
/// other text must be an [`AuthKey`] rendering. Endpoint 23 applies the same test before it reads
/// or writes, although `MARK-AUTH-FRAUD` tests nothing: the COBOL reaches F5 only with the state
/// COPAUS0C set, which passes it (X-COPAUS1C-6).
///
/// # Errors
///
/// - [`FailureClass::Input`] with an empty message, as the COBOL sets the error flag and no
///   message, when the account is not 11 digits or the key is blank (COPAUS1C-006).
/// - [`FailureClass::NotFound`] with an empty message when the key is not blank but names no
///   key: no detail has it, so the read ends `GE` (COPAUS1C-009).
pub fn edit(acct_id: &str, auth_key: &str) -> Result<DetailKey, AuthDetailFailure> {
    let account = PicU::<11>::decode(acct_id.as_bytes());
    let key_is_blank = auth_key.chars().all(|c| c == ' ') || auth_key.chars().all(|c| c == '\0');
    let Ok(acct_id) = account else {
        return Err(AuthDetailFailure::blank(FailureClass::Input));
    };
    if key_is_blank {
        return Err(AuthDetailFailure::blank(FailureClass::Input));
    }
    let auth_key =
        AuthKey::parse(auth_key).map_err(|_| AuthDetailFailure::blank(FailureClass::NotFound))?;
    Ok(DetailKey { acct_id, auth_key })
}

/// The outcome of `READ-AUTH-RECORD`: the `GU` of the summary by `ACCNTID`, then the `GNP` of
/// the detail under it by `PAUT9CTS` (`COPAUS1C.cbl:431-491`; COPAUS1C-008 to COPAUS1C-011).
///
/// The service reads the `pending_auth_summaries` row of the account and, when it is found, the
/// `pending_auth_details` row of the account and key. The COBOL issues the detail call even after
/// a failed summary call, and its text then replaces the summary's (X-COPAUS1C-6); the
/// replacement stops at the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailLookup<'a> {
    /// Both calls ended `'  '` or `'FW'`: the detail (COPAUS1C-008).
    Found(&'a PendingAuthDetailRecord),
    /// The summary or the detail is not found, status `'GE'` or `'GB'` (COPAUS1C-009).
    NotFound,
    /// The summary read failed in the store, any other status (COPAUS1C-010).
    SummaryError,
    /// The summary is found and the detail read failed in the store (COPAUS1C-011).
    DetailError,
}

impl<'a> DetailLookup<'a> {
    /// The detail the read found, the current authorization of F8 and F5.
    ///
    /// # Errors
    ///
    /// - [`DetailLookup::NotFound`]: [`FailureClass::NotFound`], message empty, as the COBOL
    ///   sets only the end flag (COPAUS1C-009). The fields the COBOL then shows from its unwritten
    ///   I/O area are `HALTED(X-COPAUS1C-H1)`; the replacement shows none.
    /// - [`DetailLookup::SummaryError`]: [`FailureClass::Store`], [`SUMMARY_READ_ERROR`] and the
    ///   IMS status, 49 characters (COPAUS1C-010).
    /// - [`DetailLookup::DetailError`]: [`FailureClass::Store`], [`DETAIL_READ_ERROR`] and the
    ///   IMS status, 49 characters (COPAUS1C-011).
    ///
    /// The status positions are `HALTED(H4)` (COPAUS1C-038). On F8 and F5 the COBOL goes on after
    /// these outcomes, `HALTED(X-COPAUS1C-H1)` and `HALTED(X-COPAUS1C-H2)` (COPAUS1C-027,
    /// COPAUS1C-029); the replacement returns the failure and writes nothing, no parity result.
    pub fn record(self) -> Result<&'a PendingAuthDetailRecord, AuthDetailFailure> {
        match self {
            Self::Found(record) => Ok(record),
            Self::NotFound => Err(AuthDetailFailure::blank(FailureClass::NotFound)),
            Self::SummaryError => Err(AuthDetailFailure::ims(SUMMARY_READ_ERROR)),
            Self::DetailError => Err(AuthDetailFailure::ims(DETAIL_READ_ERROR)),
        }
    }
}

/// The outcome of `READ-NEXT-AUTH-RECORD`, the unqualified `GNP` after the current detail
/// (`COPAUS1C.cbl:493-518`; COPAUS1C-024 to COPAUS1C-026). The replacement reads the
/// `pending_auth_details` row of the same account with the smallest key above the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextLookup<'a> {
    /// The next detail segment under the same summary (COPAUS1C-024).
    Found(&'a PendingAuthDetailRecord),
    /// No detail follows, status `'GE'` or `'GB'` (COPAUS1C-025).
    End,
    /// The read failed in the store, any other status (COPAUS1C-026).
    Error,
}

/// One authorization as `POPULATE-AUTH-DETAILS` fills the map (`COPAUS1C.cbl:291-358`;
/// COPAUS1C-012 to COPAUS1C-023), the key the screen keeps for it (COPAUS1C-024, COPAUS1C-034)
/// and the message row (COPAUS1C-035).
///
/// Every text field is the string the program moves into its map field, padded with spaces to
/// the field's width (module documentation, Display values). `PA-MESSAGE-TYPE`,
/// `PA-AUTH-ID-CODE`, `PA-TRANSACTION-AMT`, `PA-ACQR-COUNTRY-CODE` and the `FILLER` are read but
/// never shown, so the view does not carry them (COPAUS1C-023). The derived `Debug` shows the card
/// number only redacted; no other field holds a protected value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthDetailView {
    /// The account, `CDEMO-ACCT-ID`, which the screen keeps unchanged (COPAUS1C-003).
    pub acct_id: PicU<11>,
    /// The detail's `PA-AUTHORIZATION-KEY`, the next `CDEMO-CPVD-PAU-SELECTED` (COPAUS1C-024,
    /// COPAUS1C-034).
    pub key: AuthKey,
    /// Card #, `CARDNUMO PIC X(16)`: shown redacted, twelve `*` and the last four digits
    /// (COPAUS1C-012, E32).
    pub card_num: CardNumber,
    /// Auth Date, `AUTHDTO PIC X(10)`: `MM/DD/YY` from the stored `YYMMDD`, then two spaces
    /// (COPAUS1C-013).
    pub auth_date: String,
    /// Auth Time, `AUTHTMO PIC X(10)`: `HH:MM:SS` from the stored `HHMMSS`, then two spaces
    /// (COPAUS1C-014).
    pub auth_time: String,
    /// Auth Resp, `AUTHRSPO PIC X(1)`: `A` for response code `00`, `D` otherwise
    /// (COPAUS1C-016).
    pub response: Approval,
    /// Resp Reason, `AUTHRSNO PIC X(20)`: [`reason_text`] (COPAUS1C-017, COPAUS1C-018).
    pub reason: String,
    /// Auth Code, `AUTHCDO PIC X(6)`: the processing code's 6 digits (COPAUS1C-019,
    /// X-COPAUS1C-2).
    pub auth_code: String,
    /// Amount, `AUTHAMTO PIC X(12)`: `PA-APPROVED-AMT` edited by `WS-AUTH-AMT PIC -zzzzzzz9.99`
    /// (COPAUS1C-015).
    pub amount: String,
    /// POS Entry Mode, `POSEMDO PIC X(4)`: the 2 digits, then 2 spaces (COPAUS1C-020).
    pub pos_entry_mode: String,
    /// Source, `AUTHSRCO PIC X(10)`: `PA-MESSAGE-SOURCE`, then 4 spaces (COPAUS1C-020).
    pub source: String,
    /// MCC Code, `MCCCDO PIC X(4)`: `PA-MERCHANT-CATAGORY-CODE` (COPAUS1C-020).
    pub merchant_category_code: String,
    /// Card Exp. Date, `CRDEXPO PIC X(5)`: bytes 1-2, `/`, bytes 3-4 of `PA-CARD-EXPIRY-DATE`
    /// (COPAUS1C-020).
    pub card_expiry: String,
    /// Auth Type, `AUTHTYPO PIC X(14)`: `PA-AUTH-TYPE`, then 10 spaces (COPAUS1C-021).
    pub auth_type: String,
    /// Tran Id, `TRNIDO PIC X(15)`: `PA-TRANSACTION-ID` (COPAUS1C-021).
    pub transaction_id: String,
    /// Match Status, `AUTHMTCO PIC X(1)`: the stored byte (COPAUS1C-021).
    pub match_status: String,
    /// Fraud Status, `AUTHFRDO PIC X(10)`: the flag, `-` and the report date for a flag `F` or
    /// `R`, `-` and 9 spaces otherwise (COPAUS1C-022).
    pub fraud_status: String,
    /// Name, `MERNAMEO PIC X(25)`: `PA-MERCHANT-NAME`, then 3 spaces (COPAUS1C-023).
    pub merchant_name: String,
    /// Merchant ID, `MERIDO PIC X(15)`: `PA-MERCHANT-ID` (COPAUS1C-023).
    pub merchant_id: String,
    /// City, `MERCITYO PIC X(25)`: `PA-MERCHANT-CITY`, then 12 spaces (COPAUS1C-023).
    pub merchant_city: String,
    /// State, `MERSTO PIC X(2)`: `PA-MERCHANT-STATE` (COPAUS1C-023).
    pub merchant_state: String,
    /// Zip, `MERZIPO PIC X(10)`: `PA-MERCHANT-ZIP`, then 1 space (COPAUS1C-023).
    pub merchant_zip: String,
    /// The message row, `ERRMSGO PIC X(78)`: none after ENTER, first entry or a found F8
    /// (COPAUS1C-035), [`ALREADY_AT_LAST`] after F8 at the last authorization (COPAUS1C-025),
    /// and [`AUTH_MARKED_FRAUD`] or [`AUTH_FRAUD_REMOVED`] after a committed F5 (COPAUS1C-032).
    pub message: Option<&'static str>,
}

impl AuthDetailView {
    /// `POPULATE-AUTH-DETAILS` for `detail` of account `acct_id`, with `message` in the message
    /// row (COPAUS1C-012 to COPAUS1C-023, COPAUS1C-035).
    ///
    /// The date takes the stored bytes 3-4, 5-6 and 1-2 and the time bytes 1-2, 3-4 and 5-6, as
    /// stored and with no calendar check; a date byte that is not a digit is moved into
    /// `PIC 9(02)` with a result the compile options decide, the case halt candidate
    /// X-COPAUS0C-H3 names, and is shown as stored, no parity result (COPAUS1C-013).
    fn of(
        acct_id: PicU<11>,
        detail: &PendingAuthDetailRecord,
        message: Option<&'static str>,
    ) -> Self {
        let [year_1, year_2, month_1, month_2, day_1, day_2] = *detail.orig_date.as_bytes();
        let [hour_1, hour_2, minute_1, minute_2, second_1, second_2] = *detail.orig_time.as_bytes();
        let [expiry_1, expiry_2, expiry_3, expiry_4] = *detail.card_expiry_date.as_bytes();
        let response = if detail.is_approved() {
            Approval::Approved
        } else {
            Approval::Declined
        };
        Self {
            acct_id,
            key: AuthKey::of(detail),
            card_num: detail.card_num.clone(),
            auth_date: padded(
                &[month_1, month_2, b'/', day_1, day_2, b'/', year_1, year_2],
                DATE_TIME_WIDTH,
            ),
            auth_time: padded(
                &[
                    hour_1, hour_2, b':', minute_1, minute_2, b':', second_1, second_2,
                ],
                DATE_TIME_WIDTH,
            ),
            response,
            reason: reason_text(&detail.auth_resp_reason),
            auth_code: digits(detail.processing_code, AUTH_CODE_WIDTH),
            amount: EditedPicture::MinusZ8Dot99.format(detail.approved_amt.as_decimal()),
            pos_entry_mode: digits(detail.pos_entry_mode, POS_ENTRY_MODE_WIDTH),
            source: field(&detail.message_source, SOURCE_WIDTH),
            merchant_category_code: text(detail.merchant_catagory_code.as_bytes()),
            card_expiry: padded(
                &[expiry_1, expiry_2, b'/', expiry_3, expiry_4],
                CARD_EXPIRY_WIDTH,
            ),
            auth_type: field(&detail.auth_type, AUTH_TYPE_WIDTH),
            transaction_id: text(detail.transaction_id.as_bytes()),
            match_status: text(detail.match_status.as_bytes()),
            fraud_status: fraud_status(detail),
            merchant_name: field(&detail.merchant_name, MERCHANT_TEXT_WIDTH),
            merchant_id: text(detail.merchant_id.as_bytes()),
            merchant_city: field(&detail.merchant_city, MERCHANT_TEXT_WIDTH),
            merchant_state: text(detail.merchant_state.as_bytes()),
            merchant_zip: field(&detail.merchant_zip, MERCHANT_ZIP_WIDTH),
            message,
        }
    }
}

/// An alphanumeric field moved to a map field of `width` characters.
fn field<const N: usize>(value: &PicX<N>, width: usize) -> String {
    padded(value.as_bytes(), width)
}

/// A `PIC 9(n)` value moved to an alphanumeric map field of `width` characters: its `n` digits,
/// leading zeros kept, then spaces (COPAUS1C-019, COPAUS1C-020).
fn digits<const N: u32>(value: PicU<N>, width: usize) -> String {
    let mut bytes = Vec::with_capacity(width);
    value.encode_into(&mut bytes);
    padded(&bytes, width)
}

/// `AUTHFRDO`: for a flag `F` (`PA-FRAUD-CONFIRMED`) or `R` (`PA-FRAUD-REMOVED`), the flag at
/// byte 1, `-` at byte 2 and the 8 bytes of `PA-FRAUD-RPT-DATE` from byte 3; for any other flag,
/// `MOVE '-' TO AUTHFRDO`, a dash and 9 spaces (`COPAUS1C.cbl:344-350`; COPAUS1C-022).
fn fraud_status(detail: &PendingAuthDetailRecord) -> String {
    let flag = detail.auth_fraud.as_bytes();
    if flag == PendingAuthDetailRecord::FRAUD_CONFIRMED
        || flag == PendingAuthDetailRecord::FRAUD_REMOVED
    {
        let status: Vec<u8> = flag
            .iter()
            .copied()
            .chain(iter::once(b'-'))
            .chain(detail.fraud_rpt_date.as_bytes().iter().copied())
            .collect();
        padded(&status, FRAUD_STATUS_WIDTH)
    } else {
        padded(b"-", FRAUD_STATUS_WIDTH)
    }
}

/// ENTER, first entry or another key: `PROCESS-ENTER-KEY` then `POPULATE-AUTH-DETAILS` for the
/// authorization `lookup` holds (`COPAUS1C.cbl:208-228,291-358`; COPAUS1C-008, COPAUS1C-012 to
/// COPAUS1C-023, COPAUS1C-035).
///
/// A found detail gives a 200 with no message, its key and its display fields.
///
/// # Errors
///
/// The failure of [`DetailLookup::record`]: 404 with an empty message (COPAUS1C-009), or 500
/// with the summary or detail read text (COPAUS1C-010, COPAUS1C-011, `HALTED(H4)` status
/// positions).
pub fn detail(
    acct_id: PicU<11>,
    lookup: DetailLookup<'_>,
) -> Result<AuthDetailView, AuthDetailFailure> {
    lookup
        .record()
        .map(|record| AuthDetailView::of(acct_id, record, None))
}

/// F8, `PROCESS-PF8-KEY` after the current detail `current` was found: the authorization
/// `following` holds (`COPAUS1C.cbl:268-289,493-518`; COPAUS1C-024 to COPAUS1C-026).
///
/// - [`NextLookup::Found`]: the next authorization, 200 with no message and its key, which
///   becomes the screen's key (COPAUS1C-024).
/// - [`NextLookup::End`]: 200 with [`ALREADY_AT_LAST`] and the current authorization, its key
///   unchanged (COPAUS1C-025). The COBOL sends the map without `ERASE` and moves nothing to the
///   detail fields, so what the terminal shows in them is `HALTED(X-COPAUS1C-H3)`; the view's
///   fields are the current authorization's, no parity result.
///
/// The current detail comes from [`DetailLookup::record`], whose failures end F8 before the next
/// read: the COBOL's next read after a missed or failed current read is
/// `HALTED(X-COPAUS1C-H1)` and `HALTED(X-COPAUS1C-H2)` (COPAUS1C-027), and the replacement stops
/// at the first failed or missing read (X-COPAUS1C-6), no parity result.
///
/// # Errors
///
/// [`NextLookup::Error`]: [`FailureClass::Store`] with [`NEXT_READ_ERROR`] and the IMS status,
/// 46 characters, the status `HALTED(H4)` (COPAUS1C-026, COPAUS1C-038); the screen keeps its key.
pub fn next(
    acct_id: PicU<11>,
    current: &PendingAuthDetailRecord,
    following: NextLookup<'_>,
) -> Result<AuthDetailView, AuthDetailFailure> {
    match following {
        NextLookup::Found(record) => Ok(AuthDetailView::of(acct_id, record, None)),
        NextLookup::End => Ok(AuthDetailView::of(acct_id, current, Some(ALREADY_AT_LAST))),
        NextLookup::Error => Err(AuthDetailFailure::ims(NEXT_READ_ERROR)),
    }
}

/// F5's flip of the fraud flag: the action COPAUS1C sets in `WS-FRD-ACTION` and the detail with
/// the flipped flag, the `WS-FRAUD-AUTH-RECORD` it passes to COPAUS2C and later writes back
/// (`COPAUS1C.cbl:236-244`; COPAUS1C-028, COPAUS1C-030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FraudToggle {
    /// [`FraudAction::Remove`] for a stored flag `F`, [`FraudAction::Report`] otherwise.
    pub action: FraudAction,
    /// The detail as read, `PA-AUTH-FRAUD` set to the action's flag and every other byte
    /// unchanged; after [`FraudToggle::with_report_date`], also the new `PA-FRAUD-RPT-DATE`.
    pub updated: PendingAuthDetailRecord,
}

impl FraudToggle {
    /// The toggle with COPAUS2C's `MOVE WS-CUR-DATE TO PA-FRAUD-RPT-DATE` applied to the
    /// detail, through `super::copaus2c::with_report_date` (COPAUS2C-001). The record the
    /// replace writes, and the view a committed F5 shows, carry that date (COPAUS1C-032).
    #[must_use]
    pub fn with_report_date(self, report_date: PicX<8>) -> Self {
        Self {
            updated: with_report_date(&self.updated, report_date),
            action: self.action,
        }
    }

    /// `IF PA-FRAUD-REMOVED`: the message of a committed replace, chosen by the flag the record
    /// holds, which is the action's (COPAUS1C-032).
    const fn committed_message(&self) -> &'static str {
        match self.action {
            FraudAction::Remove => AUTH_FRAUD_REMOVED,
            FraudAction::Report => AUTH_MARKED_FRAUD,
        }
    }
}

/// `MARK-AUTH-FRAUD`: the flag of the detail read in this request decides, not what the screen
/// shows (`COPAUS1C.cbl:236-242`; COPAUS1C-028).
///
/// `PA-FRAUD-CONFIRMED` (`F`) becomes `PA-FRAUD-REMOVED` (`R`) with [`FraudAction::Remove`];
/// any other value, `R` and the space CP00 writes included, becomes `F` with
/// [`FraudAction::Report`]. F5 therefore alternates report and remove, and a removed
/// authorization is reported again. The request carries no flag.
#[must_use]
pub fn toggle(detail: &PendingAuthDetailRecord) -> FraudToggle {
    let action = if detail.is_fraud_confirmed() {
        FraudAction::Remove
    } else {
        FraudAction::Report
    };
    FraudToggle {
        action,
        updated: PendingAuthDetailRecord {
            auth_fraud: action.flag(),
            ..detail.clone()
        },
    }
}

/// The customer COPAUS2C receives in `WS-FRD-CUST-ID`: `CDEMO-CUST-ID`, which COPAUS0C left in
/// the COMMAREA from the account's cross-reference (COPAUS1C-030). Endpoint 23 carries no
/// customer, so the service reads the `card_xref` row of the account, the read COPAUS0C-013
/// makes (X-COPAUS1C-4).
///
/// # Errors
///
/// None of these is a parity result. [`Lookup::NotFound`], an account COPAUS0C cannot list
/// (COPAUS0C-014): [`FailureClass::NotFound`] with an empty message, as COPAUS1C-009
/// (X-COPAUS1C-4). [`Lookup::Failed`]: [`FailureClass::Store`] with an empty message, the
/// replacement's answer to a read the COBOL does not make. Either way nothing is written.
pub fn fraud_customer(lookup: Lookup<PicU<9>>) -> Result<PicU<9>, AuthDetailFailure> {
    match lookup {
        Lookup::Found(cust_id) => Ok(cust_id),
        Lookup::NotFound => Err(AuthDetailFailure::blank(FailureClass::NotFound)),
        Lookup::Failed => Err(AuthDetailFailure::blank(FailureClass::Store)),
    }
}

/// How the replace of the detail row ended: `EXEC DLI REPL SEGMENT (PAUTDTL1)`, the update of the
/// `pending_auth_details` row by its primary key in the replacement (`COPAUS1C.cbl:525-530`;
/// COPAUS1C-032, COPAUS1C-033).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentUpdate {
    /// The row was replaced, status `'  '` or `'FW'` (COPAUS1C-032).
    Replaced,
    /// Any other result: no row matched, or a store failure (COPAUS1C-033).
    Failed,
}

/// What endpoint 23 does with its transaction and answers, decided by [`toggle_outcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FraudToggleResult {
    /// `TAKE-SYNCPOINT`: the `AUTHFRDS` write and the replace commit together, and the response is
    /// a 200 with the message and the detail as replaced, its key unchanged (COPAUS1C-032,
    /// COPAUS1C-034). `HALTED(X-COPAUS1C-H2)`: the COBOL replace follows a non-hold read, so no
    /// parity value is asserted for this branch.
    Committed(Box<AuthDetailView>),
    /// `ROLL-BACK`: neither write remains, and the response is the failure, a 500 with no
    /// detail (COPAUS1C-031, COPAUS1C-033).
    RolledBack(AuthDetailFailure),
}

impl FraudToggleResult {
    /// Whether the service commits the transaction; otherwise it rolls it back.
    #[must_use]
    pub const fn commits(&self) -> bool {
        matches!(self, Self::Committed(_))
    }

    /// The message row the response carries.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Committed(view) => view.message.unwrap_or_default(),
            Self::RolledBack(failure) => failure.message(),
        }
    }

    /// The response: the view of a committed toggle, or the failure of a rolled-back one.
    ///
    /// # Errors
    ///
    /// The [`AuthDetailFailure`] of [`FraudToggleResult::RolledBack`].
    pub fn into_result(self) -> Result<AuthDetailView, AuthDetailFailure> {
        match self {
            Self::Committed(view) => Ok(*view),
            Self::RolledBack(failure) => Err(failure),
        }
    }
}

/// The end of F5 for `toggle` of account `acct_id`, from COPAUS2C's outcome `fraud` and, when
/// the service ran it, the replace's `segment_update` (`COPAUS1C.cbl:253-265,520-552,557-569`;
/// COPAUS1C-031 to COPAUS1C-034).
///
/// The service runs the replace, with [`FraudToggle::updated`], exactly when
/// `fraud.is_success()`, and passes `None` otherwise.
///
/// - `fraud` failed (`WS-FRD-UPDT-FAILED`, `super::copaus2c::FraudOutcome::InsertFailed` or
///   `UpdateFailed`): roll back, and 500 with COPAUS2C's 50-byte message, whatever
///   `segment_update` says (COPAUS1C-031). Its codes are `HALTED(H3)`, and bytes 49-50 of the
///   update-failure text `HALTED(X-COPAUS2C-H2)`. The COBOL then shows the detail with the
///   flipped flag from storage; the error response carries no detail, so the screen keeps the
///   fields it showed (X-COPAUS1C-3).
/// - `fraud` succeeded and the replace [`SegmentUpdate::Replaced`]: commit, and 200 with
///   [`AUTH_MARKED_FRAUD`] for a report or [`AUTH_FRAUD_REMOVED`] for a removal and the detail
///   as replaced (COPAUS1C-032, COPAUS1C-034), `HALTED(X-COPAUS1C-H2)`.
/// - `fraud` succeeded and the replace [`SegmentUpdate::Failed`], or was not run (`None`, so no
///   success is claimed): roll back, and 500 with [`FRAUD_TAGGING_ERROR`] and the IMS status,
///   47 characters, the status `HALTED(H4)` (COPAUS1C-033, COPAUS1C-038).
#[must_use]
pub fn toggle_outcome(
    acct_id: PicU<11>,
    toggle: &FraudToggle,
    fraud: FraudOutcome,
    segment_update: Option<SegmentUpdate>,
) -> FraudToggleResult {
    if !fraud.is_success() {
        return FraudToggleResult::RolledBack(AuthDetailFailure::fraud(fraud));
    }
    match segment_update {
        Some(SegmentUpdate::Replaced) => FraudToggleResult::Committed(Box::new(
            AuthDetailView::of(acct_id, &toggle.updated, Some(toggle.committed_message())),
        )),
        Some(SegmentUpdate::Failed) | None => {
            FraudToggleResult::RolledBack(AuthDetailFailure::ims(FRAUD_TAGGING_ERROR))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::pic::{PicS, PicU, PicX};

    use super::{
        ALREADY_AT_LAST, AUTH_FRAUD_REMOVED, AUTH_MARKED_FRAUD, AuthDetailFailure, AuthDetailView,
        DECLINE_REASONS, DETAIL_READ_ERROR, DetailLookup, FRAUD_TAGGING_ERROR, FraudToggleResult,
        MessageCodes, NEXT_READ_ERROR, NextLookup, SUMMARY_READ_ERROR, SegmentUpdate, detail, edit,
        fraud_customer, next, reason_text, toggle, toggle_outcome,
    };
    use crate::money::{Decimal, Money};
    use crate::online::copaus0c::{Approval, AuthKey, FailureClass, Lookup};
    use crate::online::copaus2c::{
        FraudAction, FraudOutcome, SYSTEM_ERROR_PREFIX, UPDT_ERROR_PREFIX,
    };
    use crate::records::pending_auth_detail::PendingAuthDetailRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A card number outside the fixture corpus, its first ten digits and its redacted form.
    const CARD: &str = "1234567890123456";
    const CARD_HEAD: &str = "1234567890";
    const CARD_REDACTED: &str = "************3456";

    /// The inverted keys CP00 stores for 2024-01-31 (`24031`) at 10:30:45.123.
    const DATE_9C: i64 = 75_968;
    const TIME_9C: i64 = 896_954_876;

    /// The account every test reads.
    fn acct() -> Result<PicU<11>, Box<dyn StdError>> {
        Ok(PicU::new(50)?)
    }

    /// An approved detail as CP00 writes it, its fraud flag blank.
    fn record() -> Result<PendingAuthDetailRecord, Box<dyn StdError>> {
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::new(DATE_9C)?,
            auth_time_9c: PicS::new(TIME_9C)?,
            orig_date: PicX::from_text("240131")?,
            orig_time: PicX::from_text("103045")?,
            card_num: CardNumber::parse(CARD)?,
            auth_type: PicX::from_text("SALE")?,
            card_expiry_date: PicX::from_text("0927")?,
            message_type: PicX::from_text("AUTH")?,
            message_source: PicX::from_text("POS")?,
            auth_id_code: PicX::from_text("103045")?,
            auth_resp_code: PicX::from_text("00")?,
            auth_resp_reason: PicX::from_text("0000")?,
            processing_code: PicU::new(1_234)?,
            transaction_amt: Money::new(Decimal::new(99_999, 2))?,
            approved_amt: Money::new(Decimal::new(123_456, 2))?,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_text("MERCHANT0000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
            match_status: PicX::from_text("P")?,
            auth_fraud: PicX::spaces(),
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// The view ENTER shows for `detail`.
    fn shown(stored: &PendingAuthDetailRecord) -> Result<AuthDetailView, Box<dyn StdError>> {
        Ok(detail(acct()?, DetailLookup::Found(stored))?)
    }

    /// Whether `failure` has `class`, codes `codes`, starts with `prefix` and holds `len`
    /// characters.
    fn is_failure(
        failure: &AuthDetailFailure,
        class: FailureClass,
        codes: MessageCodes,
        prefix: &str,
        len: usize,
    ) -> bool {
        failure.class() == class
            && failure.codes() == codes
            && failure.message().starts_with(prefix)
            && failure.message().chars().count() == len
    }

    // audit: COPAUS1C-017
    #[test]
    fn the_decline_table_ascends_for_search_all() {
        assert!(
            DECLINE_REASONS
                .windows(2)
                .all(|pair| matches!(pair, [(low, _), (high, _)] if low < high))
        );
        assert!(
            DECLINE_REASONS
                .iter()
                .all(|(code, description)| code.len() == 4 && description.len() <= 15)
        );
    }

    // audit: COPAUS1C-017
    #[test]
    fn a_table_code_shows_the_code_a_dash_and_its_description() -> TestResult {
        for (code, expected) in [
            ("0000", "0000-APPROVED"),
            ("3100", "3100-INVALID CARD"),
            ("4100", "4100-INSUFFICNT FUND"),
            ("4200", "4200-CARD NOT ACTIVE"),
            ("4300", "4300-ACCOUNT CLOSED"),
            ("4400", "4400-EXCED DAILY LMT"),
            ("5100", "5100-CARD FRAUD"),
            ("5200", "5200-MERCHANT FRAUD"),
            ("5300", "5300-LOST CARD"),
            ("9000", "9000-UNKNOWN"),
        ] {
            assert_eq!(
                reason_text(&PicX::from_text(code)?),
                format!("{expected:<20}")
            );
        }
        Ok(())
    }

    // audit: COPAUS1C-018
    #[test]
    fn any_other_reason_shows_9999_error() -> TestResult {
        for code in ["    ", "1234", "0001", "9999", "530", "a000", "310 "] {
            assert_eq!(
                reason_text(&PicX::from_text(code)?),
                "9999-ERROR          ",
                "{code:?}"
            );
        }
        Ok(())
    }

    // audit: COPAUS1C-006
    #[test]
    fn an_account_that_is_not_11_digits_or_a_blank_key_is_a_blank_400() -> TestResult {
        let key = AuthKey::new(PicS::new(DATE_9C)?, PicS::new(TIME_9C)?).render();
        let cases = [
            ("", key.as_str()),
            ("0000000005", key.as_str()),
            ("000000000500", key.as_str()),
            ("0000000005A", key.as_str()),
            (" 0000000050", key.as_str()),
            ("           ", key.as_str()),
            ("+0000000050", key.as_str()),
            ("00000000050", ""),
            ("00000000050", "        "),
            ("00000000050", "\0\0\0\0\0\0\0\0"),
            ("bad", ""),
        ];
        for (acct_id, auth_key) in cases {
            let failure = edit(acct_id, auth_key).err().ok_or("edit accepted")?;
            assert!(is_failure(
                &failure,
                FailureClass::Input,
                MessageCodes::None,
                "",
                0
            ));
            assert_eq!(failure.class().http_status(), 400);
            assert_eq!(failure.to_string(), "");
        }
        Ok(())
    }

    // audit: COPAUS1C-008, COPAUS1C-009
    #[test]
    fn a_valid_request_names_its_account_and_key_and_an_unknown_key_is_a_blank_404() -> TestResult {
        let key = AuthKey::new(PicS::new(DATE_9C)?, PicS::new(TIME_9C)?);
        let edited = edit("00000000050", &key.render())?;
        assert_eq!(edited.acct_id(), acct()?);
        assert_eq!(edited.auth_key(), key);
        for auth_key in ["ABC", "123", "2075968089695487", "1000000000000000", " \0"] {
            let failure = edit("00000000050", auth_key).err().ok_or("edit accepted")?;
            assert!(is_failure(
                &failure,
                FailureClass::NotFound,
                MessageCodes::None,
                "",
                0
            ));
            assert_eq!(failure.class().http_status(), 404);
        }
        Ok(())
    }

    // audit: COPAUS1C-008, COPAUS1C-012, COPAUS1C-013, COPAUS1C-014, COPAUS1C-015
    // audit: COPAUS1C-016, COPAUS1C-017, COPAUS1C-019, COPAUS1C-020, COPAUS1C-021
    // audit: COPAUS1C-022, COPAUS1C-023, COPAUS1C-035
    #[test]
    fn a_found_detail_shows_every_map_field_as_the_program_moves_it() -> TestResult {
        let detail = record()?;
        let view = shown(&detail)?;
        assert_eq!(view.acct_id, acct()?);
        assert_eq!(view.key, AuthKey::of(&detail));
        assert_eq!(view.card_num.to_string(), CARD_REDACTED);
        assert_eq!(view.auth_date, "01/31/24  ");
        assert_eq!(view.auth_time, "10:30:45  ");
        assert_eq!(view.response, Approval::Approved);
        assert_eq!(view.response.as_str(), "A");
        assert_eq!(view.reason, "0000-APPROVED       ");
        assert_eq!(view.auth_code, "001234");
        assert_eq!(view.amount, "     1234.56");
        assert_eq!(view.pos_entry_mode, "05  ");
        assert_eq!(view.source, "POS       ");
        assert_eq!(view.merchant_category_code, "5411");
        assert_eq!(view.card_expiry, "09/27");
        assert_eq!(view.auth_type, "SALE          ");
        assert_eq!(view.transaction_id, "TXN000000000001");
        assert_eq!(view.match_status, "P");
        assert_eq!(view.fraud_status, "-         ");
        assert_eq!(view.merchant_name, format!("{:<25}", "CORNER GROCERY"));
        assert_eq!(view.merchant_id, "MERCHANT0000001");
        assert_eq!(view.merchant_city, format!("{:<25}", "SEATTLE"));
        assert_eq!(view.merchant_state, "WA");
        assert_eq!(view.merchant_zip, "98101     ");
        assert_eq!(view.message, None);
        for (value, width) in [
            (&view.auth_date, 10),
            (&view.auth_time, 10),
            (&view.reason, 20),
            (&view.auth_code, 6),
            (&view.amount, 12),
            (&view.pos_entry_mode, 4),
            (&view.source, 10),
            (&view.merchant_category_code, 4),
            (&view.card_expiry, 5),
            (&view.auth_type, 14),
            (&view.transaction_id, 15),
            (&view.match_status, 1),
            (&view.fraud_status, 10),
            (&view.merchant_name, 25),
            (&view.merchant_id, 15),
            (&view.merchant_city, 25),
            (&view.merchant_state, 2),
            (&view.merchant_zip, 10),
        ] {
            assert_eq!(value.chars().count(), width, "{value:?}");
        }
        Ok(())
    }

    // audit: COPAUS1C-013, COPAUS1C-014
    #[test]
    fn the_date_and_time_are_copied_with_no_calendar_check() -> TestResult {
        let detail = PendingAuthDetailRecord {
            orig_date: PicX::from_text("241399")?,
            orig_time: PicX::from_text("256199")?,
            ..record()?
        };
        let view = shown(&detail)?;
        assert_eq!(view.auth_date, "13/99/24  ");
        assert_eq!(view.auth_time, "25:61:99  ");
        Ok(())
    }

    // audit: COPAUS1C-015
    #[test]
    fn the_amount_is_the_approved_amount_edited_by_minus_z8_dot_99() -> TestResult {
        for (scaled, expected) in [
            (123_456, "     1234.56"),
            (0, "        0.00"),
            (-525, "-       5.25"),
            (123_456_789_012, " 34567890.12"),
        ] {
            let detail = PendingAuthDetailRecord {
                approved_amt: Money::new(Decimal::new(scaled, 2))?,
                ..record()?
            };
            assert_eq!(shown(&detail)?.amount, expected);
        }
        Ok(())
    }

    // audit: COPAUS1C-016
    #[test]
    fn auth_resp_is_a_for_00_and_d_otherwise() -> TestResult {
        for (code, expected) in [("00", "A"), ("05", "D"), ("  ", "D"), ("0 ", "D")] {
            let detail = PendingAuthDetailRecord {
                auth_resp_code: PicX::from_text(code)?,
                ..record()?
            };
            assert_eq!(shown(&detail)?.response.as_str(), expected);
        }
        Ok(())
    }

    // audit: COPAUS1C-022
    #[test]
    fn fraud_status_shows_the_flag_and_date_only_for_f_and_r() -> TestResult {
        for (flag, expected) in [
            ("F", "F-06/01/24"),
            ("R", "R-06/01/24"),
            (" ", "-         "),
            ("X", "-         "),
        ] {
            let detail = PendingAuthDetailRecord {
                auth_fraud: PicX::from_text(flag)?,
                fraud_rpt_date: PicX::from_text("06/01/24")?,
                ..record()?
            };
            assert_eq!(shown(&detail)?.fraud_status, expected);
        }
        Ok(())
    }

    // audit: COPAUS1C-009, COPAUS1C-010, COPAUS1C-011, COPAUS1C-038
    #[test]
    fn a_missing_or_failed_read_returns_its_outcome_alone() -> TestResult {
        let acct_id = acct()?;
        let missing = detail(acct_id, DetailLookup::NotFound)
            .err()
            .ok_or("not found accepted")?;
        assert!(is_failure(
            &missing,
            FailureClass::NotFound,
            MessageCodes::None,
            "",
            0
        ));
        // The two status positions after each text are HALTED(H4): BLOCKED, only the fixed text
        // and the length are asserted.
        for (lookup, prefix) in [
            (DetailLookup::SummaryError, SUMMARY_READ_ERROR),
            (DetailLookup::DetailError, DETAIL_READ_ERROR),
        ] {
            let failure = detail(acct_id, lookup).err().ok_or("failure accepted")?;
            assert!(is_failure(
                &failure,
                FailureClass::Store,
                MessageCodes::HaltedH4,
                prefix,
                49
            ));
            assert_eq!(failure.class().http_status(), 500);
        }
        let current = record()?;
        let found = DetailLookup::Found(&current).record()?;
        assert_eq!(found, &current);
        Ok(())
    }

    // audit: COPAUS1C-024, COPAUS1C-035
    #[test]
    fn f8_shows_the_next_authorization_and_its_key() -> TestResult {
        let current = record()?;
        let following = PendingAuthDetailRecord {
            auth_time_9c: PicS::new(TIME_9C + 1)?,
            auth_resp_code: PicX::from_text("05")?,
            auth_resp_reason: PicX::from_text("4100")?,
            approved_amt: Money::new(Decimal::ZERO)?,
            ..record()?
        };
        let view = next(acct()?, &current, NextLookup::Found(&following))?;
        assert_eq!(view.key, AuthKey::of(&following));
        assert_ne!(view.key, AuthKey::of(&current));
        assert_eq!(view.response, Approval::Declined);
        assert_eq!(view.reason, "4100-INSUFFICNT FUND");
        assert_eq!(view.amount, "        0.00");
        assert_eq!(view.message, None);
        Ok(())
    }

    // audit: COPAUS1C-025
    #[test]
    fn f8_at_the_last_authorization_keeps_the_current_one_with_the_message() -> TestResult {
        let current = record()?;
        let view = next(acct()?, &current, NextLookup::End)?;
        assert_eq!(view.message, Some(ALREADY_AT_LAST));
        assert_eq!(ALREADY_AT_LAST.len(), 36);
        assert_eq!(view.key, AuthKey::of(&current));
        // The fields beside the message are HALTED(X-COPAUS1C-H3): the current authorization's,
        // the replacement's choice, not asserted as parity.
        Ok(())
    }

    // audit: COPAUS1C-026, COPAUS1C-038
    #[test]
    fn a_failed_next_read_is_a_500_with_its_text() -> TestResult {
        let current = record()?;
        let failure = next(acct()?, &current, NextLookup::Error)
            .err()
            .ok_or("failure accepted")?;
        assert!(is_failure(
            &failure,
            FailureClass::Store,
            MessageCodes::HaltedH4,
            NEXT_READ_ERROR,
            46
        ));
        Ok(())
    }

    // audit: COPAUS1C-028
    #[test]
    fn f5_flips_the_stored_flag_and_alternates_report_and_remove() -> TestResult {
        for (stored, action, flipped) in [
            ("F", FraudAction::Remove, "R"),
            ("R", FraudAction::Report, "F"),
            (" ", FraudAction::Report, "F"),
            ("X", FraudAction::Report, "F"),
        ] {
            let detail = PendingAuthDetailRecord {
                auth_fraud: PicX::from_text(stored)?,
                ..record()?
            };
            let flip = toggle(&detail);
            assert_eq!(flip.action, action);
            assert_eq!(flip.updated.auth_fraud, PicX::from_text(flipped)?);
            let restored = PendingAuthDetailRecord {
                auth_fraud: detail.auth_fraud.clone(),
                ..flip.updated
            };
            assert_eq!(restored, detail);
        }
        Ok(())
    }

    // audit: COPAUS1C-030
    #[test]
    fn the_fraud_customer_is_the_cross_reference_customer() -> TestResult {
        let cust_id = PicU::new(50)?;
        assert_eq!(fraud_customer(Lookup::Found(cust_id))?, cust_id);
        Ok(())
    }

    /// X-COPAUS1C-4: the replacement's answers to a cross-reference the read cannot give, no
    /// parity result.
    #[test]
    fn a_missing_or_failed_cross_reference_writes_nothing() -> TestResult {
        let missing = fraud_customer(Lookup::NotFound)
            .err()
            .ok_or("missing accepted")?;
        assert!(is_failure(
            &missing,
            FailureClass::NotFound,
            MessageCodes::None,
            "",
            0
        ));
        let failed = fraud_customer(Lookup::Failed)
            .err()
            .ok_or("failure accepted")?;
        assert!(is_failure(
            &failed,
            FailureClass::Store,
            MessageCodes::None,
            "",
            0
        ));
        Ok(())
    }

    // audit: COPAUS1C-031
    #[test]
    fn a_copaus2c_failure_rolls_back_with_its_message() -> TestResult {
        let flip = toggle(&record()?).with_report_date(PicX::from_text("06/01/24")?);
        for (fraud, prefix) in [
            (FraudOutcome::InsertFailed, SYSTEM_ERROR_PREFIX),
            (FraudOutcome::UpdateFailed, UPDT_ERROR_PREFIX),
        ] {
            for segment_update in [None, Some(SegmentUpdate::Replaced)] {
                let result = toggle_outcome(acct()?, &flip, fraud, segment_update);
                assert!(!result.commits());
                assert!(result.message().starts_with(prefix));
                // The code positions are HALTED(H3) and bytes 49-50 of the update-failure text
                // HALTED(X-COPAUS2C-H2): BLOCKED, only the fixed prefix and the length asserted.
                let failure = result.into_result().err().ok_or("rollback committed")?;
                assert!(is_failure(
                    &failure,
                    FailureClass::Store,
                    MessageCodes::HaltedH3,
                    prefix,
                    50
                ));
            }
        }
        Ok(())
    }

    // audit: COPAUS1C-033, COPAUS1C-038
    #[test]
    fn a_failed_replace_rolls_back_with_the_tagging_error() -> TestResult {
        let flip = toggle(&record()?).with_report_date(PicX::from_text("06/01/24")?);
        assert_eq!(FRAUD_TAGGING_ERROR.len(), 45);
        for fraud in [FraudOutcome::Added, FraudOutcome::Updated] {
            for segment_update in [Some(SegmentUpdate::Failed), None] {
                let result = toggle_outcome(acct()?, &flip, fraud, segment_update);
                assert!(!result.commits());
                assert!(result.message().starts_with(FRAUD_TAGGING_ERROR));
                let failure = result.into_result().err().ok_or("rollback committed")?;
                assert!(is_failure(
                    &failure,
                    FailureClass::Store,
                    MessageCodes::HaltedH4,
                    FRAUD_TAGGING_ERROR,
                    47
                ));
            }
        }
        Ok(())
    }

    /// HALTED(X-COPAUS1C-H2), COPAUS1C-032: the success branch is the replacement's intended
    /// behaviour, asserted as its documented choice and reported `BLOCKED`, no parity result.
    #[test]
    fn a_committed_toggle_shows_the_replaced_detail_with_its_message() -> TestResult {
        let report_date = PicX::from_text("06/01/24")?;
        for (stored, message, status) in [
            (" ", AUTH_MARKED_FRAUD, "F-06/01/24"),
            ("F", AUTH_FRAUD_REMOVED, "R-06/01/24"),
        ] {
            let detail = PendingAuthDetailRecord {
                auth_fraud: PicX::from_text(stored)?,
                ..record()?
            };
            let flip = toggle(&detail).with_report_date(report_date.clone());
            assert_eq!(flip.updated.fraud_rpt_date, report_date);
            let result = toggle_outcome(
                acct()?,
                &flip,
                FraudOutcome::Added,
                Some(SegmentUpdate::Replaced),
            );
            assert!(result.commits());
            assert_eq!(result.message(), message);
            let view = result.into_result()?;
            assert_eq!(view.message, Some(message));
            assert_eq!(view.fraud_status, status);
            assert_eq!(view.key, AuthKey::of(&detail));
        }
        assert_eq!(AUTH_MARKED_FRAUD.len(), 20);
        assert_eq!(AUTH_FRAUD_REMOVED.len(), 21);
        Ok(())
    }

    // audit: COPAUS1C-012
    #[test]
    fn no_output_holds_the_full_card_number() -> TestResult {
        let detail = record()?;
        let view = shown(&detail)?;
        let flip = toggle(&detail);
        let committed = toggle_outcome(
            acct()?,
            &flip,
            FraudOutcome::Updated,
            Some(SegmentUpdate::Replaced),
        );
        let shown_card = [
            format!("{view:?}"),
            format!("{flip:?}"),
            format!("{committed:?}"),
            view.card_num.to_string(),
        ];
        assert!(shown_card.iter().all(|text| text.contains(CARD_REDACTED)));
        let rolled_back = toggle_outcome(acct()?, &flip, FraudOutcome::InsertFailed, None);
        let failed_read = next(acct()?, &detail, NextLookup::Error)
            .err()
            .ok_or("failure accepted")?;
        let without_card = [
            format!("{rolled_back:?}"),
            rolled_back.message().to_owned(),
            format!("{failed_read:?}"),
            committed.message().to_owned(),
        ];
        assert!(
            shown_card
                .iter()
                .chain(&without_card)
                .all(|text| !text.contains(CARD_HEAD))
        );
        assert!(matches!(rolled_back, FraudToggleResult::RolledBack(_)));
        Ok(())
    }
}
