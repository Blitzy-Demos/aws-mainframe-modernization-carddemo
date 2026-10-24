//! COPAUS2C, the fraud recorder: the store half of endpoint 23,
//! `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud` (CPVD F5; AAP 0.3.4.1).
//!
//! This module ports the rows of `modern/audit/online/COPAUS2C.md`, all `server`: the report
//! date (COPAUS2C-001), the `AUTH_TS` key rebuilt from the detail (COPAUS2C-002 to
//! COPAUS2C-005), the column moves (COPAUS2C-006), the insert and its `-803` fallback to
//! `FRAUD-UPDATE` (COPAUS2C-007, COPAUS2C-008), the status and message COPAUS2C hands back
//! (COPAUS2C-007, COPAUS2C-009 to COPAUS2C-011) and the redaction of the card number
//! (COPAUS2C-013). COPAUS2C has no screen and no transaction of its own: COPAUS1C links to it
//! from `MARK-AUTH-FRAUD`, so the key handling, the detail replace and the messages endpoint 23
//! answers with are `super::copaus1c`'s.
//!
//! # Flow
//!
//! `api::services::copaus1c` runs these rules inside endpoint 23's one `PostgreSQL`
//! transaction, the unit of work COPAUS2C's `EXEC CICS RETURN` leaves to COPAUS1C
//! (COPAUS2C-012):
//!
//! 1. [`with_report_date`] stamps the toggled detail with [`report_date`], the
//!    `MM/DD/YY` text COPAUS2C moves into the COMMAREA's `PA-FRAUD-RPT-DATE` on every path
//!    (COPAUS2C-001).
//! 2. [`fraud_row`] builds the 26-column `AUTHFRDS` row, an [`AuthFraudRow`], or fails with a
//!    [`FraudRowError`] when the rebuilt `AUTH_TS` text names no timestamp (COPAUS2C-005). That
//!    failure takes the insert-failure path: [`FraudOutcome::from`] gives
//!    [`FraudOutcome::InsertFailed`] before any statement runs.
//! 3. The service inserts the row, binding the clock's date as `FRAUD_RPT_DATE` (the Db2
//!    `CURRENT DATE`, COPAUS2C-007), and reports an [`InsertResult`]. Only
//!    [`InsertResult::Duplicate`], the unique violation of the `(card_num, auth_ts)` key
//!    (`SQLCODE -803`), asks for the update ([`InsertResult::needs_update`]): it sets
//!    `auth_fraud` and `fraud_rpt_date` of the row whose key is the new row's `card_num` and
//!    `auth_ts` (COPAUS2C-008), and reports an [`UpdateResult`].
//! 4. [`fraud_outcome`] turns the two results into the [`FraudOutcome`] COPAUS1C commits or rolls
//!    back on (COPAUS2C-007, COPAUS2C-009 to COPAUS2C-011).
//!
//! Every function is pure: it reads the record, the identifiers and the injected [`Clock`] it is
//! given, and does no I/O.
//!
//! # `AUTH_TS`
//!
//! COPAUS2C builds the text `YY-MM-DD HH.MI.SSmmm000` from `PA-AUTH-ORIG-DATE` and from
//! `999999999 − PA-AUTH-TIME-9C`, and Db2's `TIMESTAMP_FORMAT(.., 'YY-MM-DD HH24.MI.SSNNNNNN')`
//! converts it (COPAUS2C-002 to COPAUS2C-004). The replacement never stores that text: it builds
//! the typed [`NaiveDateTime`] the conversion yields (Typed Dates and Timestamps), the year
//! completed with the century of the [`Clock`]'s current year and the six fraction digits read
//! as microseconds, the documented Db2 rules the reviewer confirms for the deployed release
//! (X-COPAUS2C-2). The date is the request's and the time CP00's processing time, so two
//! authorizations of one card can share a key; that collision is preserved and takes the update
//! path (X-COPAUS2C-1).
//!
//! # Halts
//!
//! - `HALTED(H3)`: the `SQLCODE` and `SQLSTATE` values in the two error texts are Db2's, and
//!   `PostgreSQL` has no `SQLCODE` (X-COPAUS2C-H1). [`FraudOutcome::message`] keeps every fixed
//!   piece and fills each code position with spaces of the edited field's width, a stand-in that
//!   is no parity result; the tests report those positions `BLOCKED`.
//! - `HALTED(X-COPAUS2C-H2)`: bytes 49-50 of the update-failure text are storage COPAUS1C's task
//!   started with. The replacement leaves them as the spaces of the padded field, no parity
//!   result.
//! - `HALTED(X-COPAUS2C-H3)`: a failed `ASKTIME` or `FORMATTIME`. The [`Clock`] cannot fail, so no
//!   input of the replacement reaches the case.
//! - `HALTED(X-COPAUS2C-H4)`: an hour-24 time text and a day-00 date text, which Db2 accepts or
//!   may accept and which no [`NaiveDateTime`] holds. [`fraud_row`] refuses both with a typed
//!   error ([`FraudRowError::is_halted`]) and never moves them to another instant (R7); that
//!   failure is no parity result.
//! - Whether Db2 accepts a non-canonical date piece, such as one holding a blank, is X-COPAUS2C-H1
//!   too; the replacement refuses every piece that is not two ASCII digits.
//!
//! # Cardholder data
//!
//! The full card number reaches only [`AuthFraudRow::card_num`], a [`CardNumber`] whose `Debug`
//! is redacted and which the repository binds through [`CardNumber::expose`]. None of the texts
//! this module builds holds a card number or any database detail text (COPAUS2C-013, E32).
//!
//! # Totality
//!
//! Every function returns a value or a typed error for any record and any clock reading; nothing
//! indexes, slices, unwraps or panics, and integer conversions are checked (Total Parsers Over
//! Untrusted Input, Checked Integer Narrowing).

use core::fmt;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use copybook::{PicS, PicU, PicX};

use crate::clock::{Clock, mm_dd_yy_slash};
use crate::money::Money;
use crate::records::PendingAuthDetailRecord;
use crate::sensitive::CardNumber;

/// Width of `WS-FRD-ACT-MSG PIC X(50)`, the message COPAUS2C hands back (COPAUS2C-007).
pub const ACT_MSG_WIDTH: usize = 50;

/// The insert succeeded: `MOVE 'ADD SUCCESS' TO WS-FRD-ACT-MSG` (`COPAUS2C.cbl:201`;
/// COPAUS2C-007).
pub const ADD_SUCCESS: &str = "ADD SUCCESS";

/// The `-803` update succeeded: `MOVE 'UPDT SUCCESS' TO WS-FRD-ACT-MSG` (`COPAUS2C.cbl:232`;
/// COPAUS2C-009).
pub const UPDT_SUCCESS: &str = "UPDT SUCCESS";

/// Bytes 1-24 of the insert-failure text, its leading space included (`COPAUS2C.cbl:211`;
/// COPAUS2C-010).
pub const SYSTEM_ERROR_PREFIX: &str = " SYSTEM ERROR DB2: CODE:";

/// Bytes 1-22 of the update-failure text, its leading space included (`COPAUS2C.cbl:239`;
/// COPAUS2C-011).
pub const UPDT_ERROR_PREFIX: &str = " UPDT ERROR DB2: CODE:";

/// The separator between the two codes of both failure texts: bytes 32-40 of the insert-failure
/// text and bytes 30-38 of the update-failure text (`COPAUS2C.cbl:212,240`; COPAUS2C-010,
/// COPAUS2C-011).
pub const STATE_SEPARATOR: &str = ", STATE: ";

/// `WS-SQLCODE PIC +9(06)`, sign and six digits, filled with spaces: the value is Db2's,
/// `HALTED(H3)` (COPAUS2C-010, COPAUS2C-011).
const SQLCODE_STAND_IN: [u8; 7] = [b' '; 7];

/// `WS-SQLSTATE PIC +9(09)`, sign and nine digits, filled with spaces: the value is Db2's,
/// `HALTED(H3)` (COPAUS2C-010, COPAUS2C-011).
const SQLSTATE_STAND_IN: [u8; 10] = [b' '; 10];

/// The modulus of a year's last two digits, and the century size `YY` completes
/// (COPAUS2C-004, X-COPAUS2C-2).
const CENTURY: i32 = 100;

/// `WS-AUTH-TIME-AN(1:2)`: the hour is the value's first two of nine digits (COPAUS2C-003).
const HOUR_DIVISOR: u32 = 10_000_000;

/// `WS-AUTH-TIME-AN(3:2)` and `(5:2)`: the minute and second divisors (COPAUS2C-003).
const MINUTE_DIVISOR: u32 = 100_000;
const SECOND_DIVISOR: u32 = 1_000;

/// Two decimal digits: the modulus that isolates one two-digit piece (COPAUS2C-003).
const TWO_DIGITS: u32 = 100;

/// `WS-AUTH-TIME-AN(7:3)`: the three millisecond digits (COPAUS2C-003).
const MILLI_DIGITS: u32 = 1_000;

/// Microseconds per millisecond: `SSNNNNNN` reads `mmm000` as six-digit microseconds
/// (COPAUS2C-004, X-COPAUS2C-2).
const MICROS_PER_MILLI: u32 = 1_000;

/// The highest month `TIMESTAMP_FORMAT` accepts (COPAUS2C-005).
const MAX_MONTH: u32 = 12;

/// The highest day `DD` accepts; day 00 is X-COPAUS2C-H4 (COPAUS2C-005).
const MAX_DAY: u32 = 31;

/// `HH24` accepts 00-24; 24 only with every later piece zero, and that text is X-COPAUS2C-H4
/// (COPAUS2C-005).
const HOUR_24: u32 = 24;

/// The highest minute and second (COPAUS2C-005).
const MAX_MINUTE_SECOND: u32 = 59;

/// The fraud action COPAUS1C sets in `WS-FRD-ACTION PIC X(01)` before the link, after flipping
/// the detail's flag (`COPAUS2C.cbl:80-82`; COPAUS2C-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FraudAction {
    /// `88 WS-REPORT-FRAUD VALUE 'F'`: the authorization is marked as fraud.
    Report,
    /// `88 WS-REMOVE-FRAUD VALUE 'R'`: the fraud mark is removed.
    Remove,
}

impl FraudAction {
    /// The one-byte flag of the action: `F` for [`FraudAction::Report`] and `R` for
    /// [`FraudAction::Remove`], the values of the detail's `PA-FRAUD-CONFIRMED` and
    /// `PA-FRAUD-REMOVED` 88-levels (CIPAUDTY-026). `MOVE WS-FRD-ACTION TO AUTH-FRAUD` stores it
    /// in the `AUTH_FRAUD` column, whatever `PA-AUTH-FRAUD` holds (COPAUS2C-006, COPAUS2C-008).
    #[must_use]
    pub const fn flag(self) -> PicX<1> {
        match self {
            Self::Report => PicX::from_array(*PendingAuthDetailRecord::FRAUD_CONFIRMED),
            Self::Remove => PicX::from_array(*PendingAuthDetailRecord::FRAUD_REMOVED),
        }
    }
}

/// `ASKTIME` then `FORMATTIME MMDDYY DATESEP`, with no separator value and so `/`, into
/// `WS-CUR-DATE PIC X(08)`: the current date as `MM/DD/YY` from the injected clock
/// (`COPAUS2C.cbl:91-100`; COPAUS2C-001). A failed CICS command is `HALTED(X-COPAUS2C-H3)`,
/// which the clock never reaches.
#[must_use]
pub fn report_date(clock: &dyn Clock) -> PicX<8> {
    PicX::moved_from(mm_dd_yy_slash(clock.now()).as_bytes())
}

/// `MOVE WS-CUR-DATE TO PA-FRAUD-RPT-DATE`: `detail` with its report date replaced by
/// `report_date` and every other byte unchanged (`COPAUS2C.cbl:101`; COPAUS2C-001). COPAUS2C
/// makes the move in the COMMAREA copy of the detail on every path, the SQL failure paths
/// included, and COPAUS1C replaces the segment with that copy only after a success
/// (COPAUS2C-012). The report date is no column of `AUTHFRDS`: [`fraud_row`] reads no part of it.
#[must_use]
pub fn with_report_date(
    detail: &PendingAuthDetailRecord,
    report_date: PicX<8>,
) -> PendingAuthDetailRecord {
    PendingAuthDetailRecord {
        fraud_rpt_date: report_date,
        ..detail.clone()
    }
}

/// One `AUTHFRDS` row as COPAUS2C's insert writes it, table `auth_fraud` of migration 0004: the
/// 26 columns of `AUTHFRDS.ddl` but `FRAUD_RPT_DATE` (COPAUS2C-006, COPAUS2C-007).
///
/// `FRAUD_RPT_DATE` is Db2's `CURRENT DATE` in both statements, not a value COPAUS2C moves, so
/// the row does not carry it: the repository binds the clock's date beside the row on the insert
/// and on the update, and returns the stored date beside the row on a read (COPAUS2C-007,
/// COPAUS2C-008). `MERCHANT-NAME-LEN` takes `LENGTH OF PA-MERCHANT-NAME`, always 22, so the
/// `VARCHAR(22)` column holds all 22 bytes of [`AuthFraudRow::merchant_name`], trailing spaces
/// included.
///
/// The derived `Debug` shows the card number only in its redacted form (COPAUS2C-013) and every
/// other field as it is. The text the detail kept from the CP00 request, such as
/// `merchant_name`, `merchant_id`, `merchant_city`, `merchant_zip` and `transaction_id`, is
/// moved unchanged and may hold a protected value, such as a card number or an SSN, so a row
/// must not be formatted into a log line, a span or an error. Where such text reaches a
/// response, it passes through [`crate::sensitive::redact_free_text`], which masks its SSN- and
/// card-number-shaped numbers, as `api::dto::copaus0c` and `api::dto::copaus1c` do for
/// endpoints 21 to 23 (Cardholder Data Redaction, E32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFraudRow {
    /// `CARD_NUM CHAR(16)`, the first key part, from `PA-CARD-NUM`: the full number, kept in the
    /// database only and bound through [`CardNumber::expose`] (COPAUS2C-013).
    pub card_num: CardNumber,
    /// `AUTH_TS TIMESTAMP`, the second key part, rebuilt from the detail's original date and
    /// inverted time (COPAUS2C-002 to COPAUS2C-004).
    pub auth_ts: NaiveDateTime,
    /// `AUTH_TYPE CHAR(4)` from `PA-AUTH-TYPE`.
    pub auth_type: PicX<4>,
    /// `CARD_EXPIRY_DATE CHAR(4)` from `PA-CARD-EXPIRY-DATE`, text and not a typed date.
    pub card_expiry_date: PicX<4>,
    /// `MESSAGE_TYPE CHAR(6)` from `PA-MESSAGE-TYPE`.
    pub message_type: PicX<6>,
    /// `MESSAGE_SOURCE CHAR(6)` from `PA-MESSAGE-SOURCE`.
    pub message_source: PicX<6>,
    /// `AUTH_ID_CODE CHAR(6)` from `PA-AUTH-ID-CODE`.
    pub auth_id_code: PicX<6>,
    /// `AUTH_RESP_CODE CHAR(2)` from `PA-AUTH-RESP-CODE`.
    pub auth_resp_code: PicX<2>,
    /// `AUTH_RESP_REASON CHAR(4)` from `PA-AUTH-RESP-REASON`.
    pub auth_resp_reason: PicX<4>,
    /// `PROCESSING_CODE CHAR(6)` from `PA-PROCESSING-CODE PIC 9(06)`: its six digits, leading
    /// zeros included.
    pub processing_code: PicX<6>,
    /// `TRANSACTION_AMT DECIMAL(12,2)` from `PA-TRANSACTION-AMT`, the value unchanged.
    pub transaction_amt: Money<10>,
    /// `APPROVED_AMT DECIMAL(12,2)` from `PA-APPROVED-AMT`, the value unchanged.
    pub approved_amt: Money<10>,
    /// `MERCHANT_CATAGORY_CODE CHAR(4)` from `PA-MERCHANT-CATAGORY-CODE`, the COBOL spelling kept.
    pub merchant_catagory_code: PicX<4>,
    /// `ACQR_COUNTRY_CODE CHAR(3)` from `PA-ACQR-COUNTRY-CODE`.
    pub acqr_country_code: PicX<3>,
    /// `POS_ENTRY_MODE SMALLINT` from `PA-POS-ENTRY-MODE PIC 9(02)`: the value 0-99.
    pub pos_entry_mode: PicU<2>,
    /// `MERCHANT_ID CHAR(15)` from `PA-MERCHANT-ID`.
    pub merchant_id: PicX<15>,
    /// `MERCHANT_NAME VARCHAR(22)` from `PA-MERCHANT-NAME`: all 22 bytes.
    pub merchant_name: PicX<22>,
    /// `MERCHANT_CITY CHAR(13)` from `PA-MERCHANT-CITY`.
    pub merchant_city: PicX<13>,
    /// `MERCHANT_STATE CHAR(02)` from `PA-MERCHANT-STATE`.
    pub merchant_state: PicX<2>,
    /// `MERCHANT_ZIP CHAR(09)` from `PA-MERCHANT-ZIP`.
    pub merchant_zip: PicX<9>,
    /// `TRANSACTION_ID CHAR(15)` from `PA-TRANSACTION-ID`.
    pub transaction_id: PicX<15>,
    /// `MATCH_STATUS CHAR(1)` from `PA-MATCH-STATUS`.
    pub match_status: PicX<1>,
    /// `AUTH_FRAUD CHAR(1)` from `WS-FRD-ACTION` ([`FraudAction::flag`]), not from
    /// `PA-AUTH-FRAUD`; the only column besides `FRAUD_RPT_DATE` that the update sets
    /// (COPAUS2C-008).
    pub auth_fraud: PicX<1>,
    /// `ACCT_ID` from `WS-ACCT-ID PIC 9(11)`, the account of COPAUS1C's COMMAREA.
    pub acct_id: PicU<11>,
    /// `CUST_ID` from `WS-CUST-ID PIC 9(9)`, the customer of the account's cross-reference.
    pub cust_id: PicU<9>,
}

/// The rebuilt `AUTH_TS` text names no timestamp, so `TIMESTAMP_FORMAT` fails and the insert
/// writes nothing (COPAUS2C-005). Each variant takes the insert-failure path:
/// [`FraudOutcome::from`] gives [`FraudOutcome::InsertFailed`]. No variant carries any part of the
/// record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FraudRowError {
    /// A year, month or day piece of `PA-AUTH-ORIG-DATE` is not two ASCII digits. For a piece
    /// that holds a blank, whether Db2 accepts the text is X-COPAUS2C-H1, so that subset is no
    /// parity result.
    DateNotDigits,
    /// The month is outside 01-12, or the day is above 31 or past the last day of that month of
    /// that year.
    InvalidDate,
    /// The hour is above 24, the hour is 24 with a minute, second or millisecond that is not
    /// zero, or the minute or second is above 59. No key CP00 writes gives such a time: its hours
    /// are 00-23 (COPAUS2C-003).
    InvalidTime,
    /// The day piece is 00, every other piece valid: `HALTED(X-COPAUS2C-H4)`.
    DayZero,
    /// The time is 24:00:00.000, from an inverted key of 759999999, every other piece valid:
    /// Db2 accepts it and no `NaiveDateTime` holds it, `HALTED(X-COPAUS2C-H4)`.
    HourTwentyFour,
    /// The clock's century and `YY` give a year no `NaiveDate` holds, which no real clock
    /// reaches.
    YearOutOfRange,
}

impl FraudRowError {
    /// Whether the case is `HALTED(X-COPAUS2C-H4)`: the typed error is the replacement's choice
    /// and no parity result, so the tests over it report `BLOCKED` (COPAUS2C-005).
    #[must_use]
    pub const fn is_halted(self) -> bool {
        matches!(self, Self::DayZero | Self::HourTwentyFour)
    }
}

impl fmt::Display for FraudRowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DateNotDigits => "authorization date piece is not two digits",
            Self::InvalidDate => "authorization date names no calendar date",
            Self::InvalidTime => "authorization time is out of range",
            Self::DayZero => "authorization date has day zero",
            Self::HourTwentyFour => "authorization time is hour twenty-four",
            Self::YearOutOfRange => "authorization year is out of range",
        })
    }
}

impl std::error::Error for FraudRowError {}

/// The `AUTHFRDS` row COPAUS2C inserts for `detail`, the toggled detail COPAUS1C passes, with
/// `action` as the fraud flag and `acct_id` and `cust_id` from COPAUS1C's COMMAREA
/// (`COPAUS2C.cbl:103-139`; COPAUS2C-002 to COPAUS2C-006).
///
/// Every column is a byte-for-byte move between equal widths except `processing_code` (the six
/// digits of `PA-PROCESSING-CODE`), `auth_fraud` (the action's flag, not `PA-AUTH-FRAUD`) and
/// `auth_ts`, the typed value of the `YY-MM-DD HH.MI.SSmmm000` text: year from the century of
/// `clock`'s current year and `YY` (X-COPAUS2C-2), month and day from `PA-AUTH-ORIG-DATE`,
/// hour, minute, second and millisecond from `999999999 − PA-AUTH-TIME-9C`, microseconds the
/// milliseconds times 1000 (COPAUS2C-004). The row's key repeats for two authorizations of one
/// card with the same request date and processing time of day, as in the COBOL (X-COPAUS2C-1).
///
/// # Errors
///
/// A [`FraudRowError`] when the text names no timestamp (COPAUS2C-005); the hour-24 and day-00
/// texts are among them as `HALTED(X-COPAUS2C-H4)` ([`FraudRowError::is_halted`]).
pub fn fraud_row(
    detail: &PendingAuthDetailRecord,
    action: FraudAction,
    acct_id: PicU<11>,
    cust_id: PicU<9>,
    clock: &dyn Clock,
) -> Result<AuthFraudRow, FraudRowError> {
    let auth_ts = auth_ts(detail, clock)?;
    Ok(AuthFraudRow {
        card_num: detail.card_num.clone(),
        auth_ts,
        auth_type: detail.auth_type.clone(),
        card_expiry_date: detail.card_expiry_date.clone(),
        message_type: detail.message_type.clone(),
        message_source: detail.message_source.clone(),
        auth_id_code: detail.auth_id_code.clone(),
        auth_resp_code: detail.auth_resp_code.clone(),
        auth_resp_reason: detail.auth_resp_reason.clone(),
        processing_code: processing_code_text(detail.processing_code),
        transaction_amt: detail.transaction_amt,
        approved_amt: detail.approved_amt,
        merchant_catagory_code: detail.merchant_catagory_code.clone(),
        acqr_country_code: detail.acqr_country_code.clone(),
        pos_entry_mode: detail.pos_entry_mode,
        merchant_id: detail.merchant_id.clone(),
        merchant_name: detail.merchant_name.clone(),
        merchant_city: detail.merchant_city.clone(),
        merchant_state: detail.merchant_state.clone(),
        merchant_zip: detail.merchant_zip.clone(),
        transaction_id: detail.transaction_id.clone(),
        match_status: detail.match_status.clone(),
        auth_fraud: action.flag(),
        acct_id,
        cust_id,
    })
}

/// `MOVE PA-PROCESSING-CODE TO PROCESSING-CODE`: the `9(06)` value as its six digits, leading
/// zeros included, in the `X(6)` host variable (COPAUS2C-006).
fn processing_code_text(code: PicU<6>) -> PicX<6> {
    let mut digits = Vec::with_capacity(PicX::<6>::WIDTH);
    code.encode_into(&mut digits);
    PicX::moved_from(&digits)
}

/// The value of one ASCII digit, or `None` for any other byte, a blank included.
fn digit(byte: u8) -> Option<u8> {
    byte.checked_sub(b'0').filter(|value| *value <= 9)
}

/// A two-digit piece, `None` unless both bytes are ASCII digits (COPAUS2C-002, COPAUS2C-005).
fn two_digits(high: u8, low: u8) -> Option<u8> {
    Some(digit(high)? * 10 + digit(low)?)
}

/// `WS-AUTH-TIME PIC 9(09) = 999999999 − PA-AUTH-TIME-9C` (`COPAUS2C.cbl:107`; COPAUS2C-003).
///
/// For a key CP00 wrote, `PicS::uncomplement` gives `HHMMSS × 1000 + milliseconds` back. A
/// negative key, which no CP00 key is, makes the difference `999999999 + |key|`, ten digits.
fn ws_auth_time(key: PicS<9>) -> u64 {
    key.uncomplement().unwrap_or_else(|_| {
        // TRUNCATION: COMPUTE stores into the unsigned nine-digit WS-AUTH-TIME with no ROUNDED
        // and no ON SIZE ERROR, so the high-order digit of 999999999 + |key| is dropped and the
        // low nine digits, |key| − 1, remain. `uncomplement` refuses only a negative key, whose
        // magnitude is at least 1, so the saturating subtraction is exact.
        key.get().unsigned_abs().saturating_sub(1)
    })
}

/// The typed `AUTH_TS` that `TIMESTAMP_FORMAT` yields for the text COPAUS2C builds from
/// `detail`, or the reason it yields none (COPAUS2C-002 to COPAUS2C-005).
fn auth_ts(
    detail: &PendingAuthDetailRecord,
    clock: &dyn Clock,
) -> Result<NaiveDateTime, FraudRowError> {
    // `PA-AUTH-ORIG-DATE(1:2)`, `(3:2)` and `(5:2)`: the tens and units of each piece.
    let [y1, y2, m1, m2, d1, d2] = *detail.orig_date.as_bytes();
    let yy = two_digits(y1, y2).ok_or(FraudRowError::DateNotDigits)?;
    let month = two_digits(m1, m2).ok_or(FraudRowError::DateNotDigits)?;
    let day = two_digits(d1, d2).ok_or(FraudRowError::DateNotDigits)?;
    let year = century_year(clock.now().year(), yy)?;
    let time = AuthTime::from_key(detail.auth_time_9c)?;
    timestamp(year, u32::from(month), u32::from(day), time)
}

/// `YY` completed with the first two digits of the current year: the current year's century
/// plus `yy` (COPAUS2C-004, X-COPAUS2C-2).
fn century_year(current_year: i32, yy: u8) -> Result<i32, FraudRowError> {
    current_year
        .div_euclid(CENTURY)
        .checked_mul(CENTURY)
        .and_then(|century| century.checked_add(i32::from(yy)))
        .filter(|year| NaiveDate::from_ymd_opt(*year, 1, 1).is_some())
        .ok_or(FraudRowError::YearOutOfRange)
}

/// The time pieces COPAUS2C takes from the nine digits of `WS-AUTH-TIME-AN`: digits 1-2, 3-4,
/// 5-6 and 7-9 (`COPAUS2C.cbl:108-111`; COPAUS2C-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuthTime {
    hour: u32,
    minute: u32,
    second: u32,
    milli: u32,
}

impl AuthTime {
    /// The pieces of `999999999 − key`; each is 00-99, the milliseconds 000-999.
    fn from_key(key: PicS<9>) -> Result<Self, FraudRowError> {
        // TRUNCATION: `ws_auth_time` is at most 999999999, below `u32::MAX`, so the conversion is
        // exact; the error arm is unreachable and keeps the conversion checked.
        let digits = u32::try_from(ws_auth_time(key)).map_err(|_| FraudRowError::InvalidTime)?;
        Ok(Self {
            hour: digits / HOUR_DIVISOR,
            minute: digits / MINUTE_DIVISOR % TWO_DIGITS,
            second: digits / SECOND_DIVISOR % TWO_DIGITS,
            milli: digits % MILLI_DIGITS,
        })
    }

    /// Whether a later piece than the hour is not zero.
    const fn has_later_piece(self) -> bool {
        self.minute != 0 || self.second != 0 || self.milli != 0
    }

    /// Whether `HH24.MI.SS` refuses the pieces outright: an hour above 24, hour 24 with a later
    /// piece that is not zero, or a minute or second above 59 (COPAUS2C-005).
    const fn is_out_of_range(self) -> bool {
        self.hour > HOUR_24
            || (self.hour == HOUR_24 && self.has_later_piece())
            || self.minute > MAX_MINUTE_SECOND
            || self.second > MAX_MINUTE_SECOND
    }
}

/// The timestamp of the pieces, the definite refusals of COPAUS2C-005 checked before the two
/// boundary texts of X-COPAUS2C-H4, so that a text is reported halted only when nothing else
/// refuses it.
fn timestamp(
    year: i32,
    month: u32,
    day: u32,
    time: AuthTime,
) -> Result<NaiveDateTime, FraudRowError> {
    let date_out_of_range = !(1..=MAX_MONTH).contains(&month)
        || day > MAX_DAY
        || (day != 0 && NaiveDate::from_ymd_opt(year, month, day).is_none());
    if date_out_of_range {
        return Err(FraudRowError::InvalidDate);
    }
    if time.is_out_of_range() {
        return Err(FraudRowError::InvalidTime);
    }
    if day == 0 {
        return Err(FraudRowError::DayZero);
    }
    if time.hour == HOUR_24 {
        return Err(FraudRowError::HourTwentyFour);
    }
    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or(FraudRowError::InvalidDate)?;
    let micro = time
        .milli
        .checked_mul(MICROS_PER_MILLI)
        .ok_or(FraudRowError::InvalidTime)?;
    let clock_time = NaiveTime::from_hms_micro_opt(time.hour, time.minute, time.second, micro)
        .ok_or(FraudRowError::InvalidTime)?;
    Ok(NaiveDateTime::new(date, clock_time))
}

/// How the insert of the [`AuthFraudRow`] ended, as the service reports it (COPAUS2C-007,
/// COPAUS2C-008, COPAUS2C-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsertResult {
    /// `SQLCODE` 0: the row was written (COPAUS2C-007).
    Inserted,
    /// `SQLCODE -803`, SQLSTATE `23505` in the replacement: a row with the same `card_num` and
    /// `auth_ts` exists, so `FRAUD-UPDATE` runs (COPAUS2C-008).
    Duplicate,
    /// Any other result, positive warnings included, and a [`FraudRowError`] that stopped the
    /// insert before it ran (COPAUS2C-005, COPAUS2C-010).
    Failed,
}

impl InsertResult {
    /// Whether COPAUS2C performs `FRAUD-UPDATE`: only after [`InsertResult::Duplicate`]
    /// (`COPAUS2C.cbl:203-204`; COPAUS2C-008). The service runs the update, inside the same
    /// transaction once the failed insert's savepoint is rolled back, exactly when this holds,
    /// and passes its [`UpdateResult`] to [`fraud_outcome`].
    #[must_use]
    pub const fn needs_update(self) -> bool {
        matches!(self, Self::Duplicate)
    }
}

/// How `FRAUD-UPDATE` ended: `UPDATE .. SET AUTH_FRAUD, FRAUD_RPT_DATE WHERE CARD_NUM AND
/// AUTH_TS` (`COPAUS2C.cbl:221-243`; COPAUS2C-008, COPAUS2C-009, COPAUS2C-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateResult {
    /// `SQLCODE` 0: the row's flag and report date were set (COPAUS2C-009).
    Updated,
    /// Any other `SQLCODE`, `+100` (no row matched) included (COPAUS2C-011).
    Failed,
}

/// What COPAUS2C hands back to COPAUS1C in `WS-FRD-UPDATE-STATUS` and `WS-FRD-ACT-MSG`
/// (`COPAUS2C.cbl:83-86,199-243`; COPAUS2C-007, COPAUS2C-009 to COPAUS2C-012).
///
/// COPAUS1C goes on to the detail replace after a success and answers with its own message; after
/// a failure it rolls back, so neither write remains, and endpoint 23 returns 500 with
/// [`FraudOutcome::message`] (COPAUS2C-010, COPAUS2C-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FraudOutcome {
    /// The insert succeeded: status `S`, `'ADD SUCCESS'` (COPAUS2C-007).
    Added,
    /// The `-803` update succeeded: status `S`, `'UPDT SUCCESS'` (COPAUS2C-009).
    Updated,
    /// The insert failed with a code other than `-803`, or the row could not be built: status
    /// `F`, `' SYSTEM ERROR DB2: CODE:'` .. (COPAUS2C-005, COPAUS2C-010).
    InsertFailed,
    /// The update failed: status `F`, `' UPDT ERROR DB2: CODE:'` .. (COPAUS2C-011).
    UpdateFailed,
}

impl FraudOutcome {
    /// `WS-FRD-UPDT-SUCCESS`, status `S`: [`FraudOutcome::Added`] or [`FraudOutcome::Updated`].
    /// Otherwise `WS-FRD-UPDT-FAILED`, status `F` (COPAUS2C-007, COPAUS2C-009 to COPAUS2C-011).
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Added | Self::Updated)
    }

    /// `WS-FRD-ACT-MSG PIC X(50)`, space-padded (COPAUS2C-007, COPAUS2C-009 to COPAUS2C-011).
    ///
    /// The insert-failure text fills all 50 bytes: [`SYSTEM_ERROR_PREFIX`] (24), `WS-SQLCODE`
    /// (7), [`STATE_SEPARATOR`] (9) and `WS-SQLSTATE` (10). The update-failure text fills 48:
    /// [`UPDT_ERROR_PREFIX`] (22) and the same three parts. Each code position holds spaces of its
    /// width, `HALTED(H3)`, because the values are Db2's; the replacement never puts database
    /// detail text there (COPAUS2C-013). Bytes 49-50 of the update-failure text are spaces,
    /// `HALTED(X-COPAUS2C-H2)`.
    #[must_use]
    pub fn message(self) -> PicX<ACT_MSG_WIDTH> {
        match self {
            Self::Added => PicX::moved_from(ADD_SUCCESS.as_bytes()),
            Self::Updated => PicX::moved_from(UPDT_SUCCESS.as_bytes()),
            Self::InsertFailed => db2_error_text(SYSTEM_ERROR_PREFIX),
            Self::UpdateFailed => db2_error_text(UPDT_ERROR_PREFIX),
        }
    }
}

/// A row that cannot be built never reaches the insert, and takes its failure path with the
/// same fixed pieces and status (COPAUS2C-005, COPAUS2C-010).
impl From<FraudRowError> for FraudOutcome {
    fn from(_: FraudRowError) -> Self {
        Self::InsertFailed
    }
}

/// `STRING prefix WS-SQLCODE ', STATE: ' WS-SQLSTATE DELIMITED BY SIZE INTO WS-FRD-ACT-MSG`,
/// with the two codes `HALTED(H3)` (`COPAUS2C.cbl:211-213,239-241`; COPAUS2C-010,
/// COPAUS2C-011).
fn db2_error_text(prefix: &str) -> PicX<ACT_MSG_WIDTH> {
    let mut text = Vec::with_capacity(ACT_MSG_WIDTH);
    text.extend_from_slice(prefix.as_bytes());
    text.extend_from_slice(&SQLCODE_STAND_IN);
    text.extend_from_slice(STATE_SEPARATOR.as_bytes());
    text.extend_from_slice(&SQLSTATE_STAND_IN);
    PicX::moved_from(&text)
}

/// The outcome COPAUS2C reports for the insert's result and, after a duplicate, the update's
/// (`COPAUS2C.cbl:199-216,230-243`; COPAUS2C-007 to COPAUS2C-011).
///
/// - [`InsertResult::Inserted`] gives [`FraudOutcome::Added`]; no update runs, so `update` is
///   not read.
/// - [`InsertResult::Duplicate`] gives [`FraudOutcome::Updated`] for
///   `Some(UpdateResult::Updated)` and [`FraudOutcome::UpdateFailed`] otherwise. `None` there
///   means the update the `-803` path requires was not run, so no success is claimed and the
///   caller rolls back.
/// - [`InsertResult::Failed`] gives [`FraudOutcome::InsertFailed`]; no update runs.
#[must_use]
pub const fn fraud_outcome(insert: InsertResult, update: Option<UpdateResult>) -> FraudOutcome {
    match (insert, update) {
        (InsertResult::Inserted, _) => FraudOutcome::Added,
        (InsertResult::Duplicate, Some(UpdateResult::Updated)) => FraudOutcome::Updated,
        (InsertResult::Duplicate, Some(UpdateResult::Failed) | None) => FraudOutcome::UpdateFailed,
        (InsertResult::Failed, _) => FraudOutcome::InsertFailed,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{PicS, PicU, PicX};

    use super::{
        ACT_MSG_WIDTH, AuthFraudRow, FraudAction, FraudOutcome, FraudRowError, InsertResult,
        STATE_SEPARATOR, SYSTEM_ERROR_PREFIX, UPDT_ERROR_PREFIX, UpdateResult, fraud_outcome,
        fraud_row, report_date, with_report_date,
    };
    use crate::clock::FixedClock;
    use crate::money::{Decimal, Money};
    use crate::records::PendingAuthDetailRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A card number outside the fixture corpus, and its last four digits.
    const CARD: &str = "1234567890123456";
    const CARD_LAST4: &str = "3456";

    /// The inverted key CP00 stores for 10:30:45.123: `999999999 − 103045123`.
    const KEY_10_30_45_123: i64 = 896_954_876;

    /// A local date and time, failing the test when it names none.
    fn at(
        (year, month, day): (i32, u32, u32),
        (hour, minute, second, milli): (u32, u32, u32, u32),
    ) -> Result<NaiveDateTime, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_milli_opt(hour, minute, second, milli))
            .ok_or_else(|| "no such date and time".into())
    }

    /// A clock reading in June 2024.
    fn clock_2024() -> Result<FixedClock, Box<dyn StdError>> {
        Ok(FixedClock::new(at((2024, 6, 1), (9, 5, 1, 0))?))
    }

    /// A toggled detail with original date `orig_date` and inverted time key `auth_time_9c`, its
    /// other fields as CP00 writes them, the fraud flag already flipped to `F`.
    fn detail(
        orig_date: [u8; 6],
        auth_time_9c: i64,
    ) -> Result<PendingAuthDetailRecord, Box<dyn StdError>> {
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::new(77_984)?,
            auth_time_9c: PicS::new(auth_time_9c)?,
            orig_date: PicX::from_array(orig_date),
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
            transaction_amt: Money::new(Decimal::new(12_345, 2))?,
            approved_amt: Money::new(Decimal::new(-500, 2))?,
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
            auth_fraud: PicX::from_text("F")?,
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// The row for `orig_date` and `auth_time_9c` under `clock`, reported as `Report`.
    fn row_for(
        orig_date: [u8; 6],
        auth_time_9c: i64,
        clock: &FixedClock,
    ) -> Result<Result<AuthFraudRow, FraudRowError>, Box<dyn StdError>> {
        Ok(fraud_row(
            &detail(orig_date, auth_time_9c)?,
            FraudAction::Report,
            PicU::new(50)?,
            PicU::new(50)?,
            clock,
        ))
    }

    // audit: COPAUS2C-001
    #[test]
    fn report_date_is_the_clock_date_as_mm_dd_yy() -> TestResult {
        let clock = FixedClock::new(at((2024, 3, 7), (23, 59, 59, 999))?);
        assert_eq!(report_date(&clock).as_bytes(), b"03/07/24");
        let clock = FixedClock::new(at((1999, 12, 31), (0, 0, 0, 0))?);
        assert_eq!(report_date(&clock).as_bytes(), b"12/31/99");
        Ok(())
    }

    // audit: COPAUS2C-001
    #[test]
    fn the_report_date_replaces_only_pa_fraud_rpt_date() -> TestResult {
        let original = detail(*b"220115", KEY_10_30_45_123)?;
        let stamped = with_report_date(&original, report_date(&clock_2024()?));
        assert_eq!(stamped.fraud_rpt_date.as_bytes(), b"06/01/24");
        let restored = with_report_date(&stamped, PicX::spaces());
        assert_eq!(restored, original);
        Ok(())
    }

    // audit: COPAUS2C-006, COPAUS2C-008
    #[test]
    fn each_action_has_its_88_level_flag_and_sets_auth_fraud() -> TestResult {
        let toggled = detail(*b"220115", KEY_10_30_45_123)?;
        let clock = clock_2024()?;
        for (action, flag) in [
            (
                FraudAction::Report,
                PendingAuthDetailRecord::FRAUD_CONFIRMED,
            ),
            (FraudAction::Remove, PendingAuthDetailRecord::FRAUD_REMOVED),
        ] {
            assert_eq!(action.flag().as_bytes(), flag);
            let row = fraud_row(&toggled, action, PicU::new(50)?, PicU::new(50)?, &clock)?;
            // `AUTH-FRAUD` comes from `WS-FRD-ACTION`, not from the detail's own `F`.
            assert_eq!(row.auth_fraud.as_bytes(), flag);
        }
        Ok(())
    }

    // audit: COPAUS2C-002, COPAUS2C-003, COPAUS2C-004
    #[test]
    fn auth_ts_joins_the_request_date_and_the_uninverted_time() -> TestResult {
        let clock = clock_2024()?;
        for (orig_date, key, expected) in [
            (
                b"220115",
                KEY_10_30_45_123,
                at((2022, 1, 15), (10, 30, 45, 123))?,
            ),
            (
                b"240229",
                764_040_000,
                at((2024, 2, 29), (23, 59, 59, 999))?,
            ),
            (b"991231", 999_999_999, at((2099, 12, 31), (0, 0, 0, 0))?),
            (b"000101", 999_999_998, at((2000, 1, 1), (0, 0, 0, 1))?),
        ] {
            let row = row_for(*orig_date, key, &clock)??;
            assert_eq!(row.auth_ts, expected);
        }
        Ok(())
    }

    // audit: COPAUS2C-004
    #[test]
    fn yy_takes_the_century_of_the_clock_year() -> TestResult {
        for (clock_at, expected_year) in [
            (at((1999, 12, 31), (23, 59, 59, 0))?, 1922),
            (at((2000, 1, 1), (0, 0, 0, 0))?, 2022),
            (at((2199, 6, 1), (12, 0, 0, 0))?, 2122),
        ] {
            let row = row_for(*b"220115", KEY_10_30_45_123, &FixedClock::new(clock_at))??;
            assert_eq!(row.auth_ts, at((expected_year, 1, 15), (10, 30, 45, 123))?);
        }
        // The last and first centuries `chrono` holds, completed past their ends.
        for (extreme, orig_date) in [
            (NaiveDateTime::MAX, b"990115"),
            (NaiveDateTime::MIN, b"000115"),
        ] {
            let result = row_for(*orig_date, KEY_10_30_45_123, &FixedClock::new(extreme))?;
            assert_eq!(result, Err(FraudRowError::YearOutOfRange));
        }
        Ok(())
    }

    // audit: COPAUS2C-003
    #[test]
    fn a_negative_key_keeps_the_low_nine_digits_of_the_difference() -> TestResult {
        let clock = clock_2024()?;
        for (key, expected_time) in [(-5, (0, 0, 0, 4)), (-1, (0, 0, 0, 0))] {
            let row = row_for(*b"220115", key, &clock)??;
            assert_eq!(row.auth_ts, at((2022, 1, 15), expected_time)?);
        }
        let result = row_for(*b"220115", -999_999_999, &clock)?;
        assert_eq!(result, Err(FraudRowError::InvalidTime));
        Ok(())
    }

    // audit: COPAUS2C-005
    #[test]
    fn a_text_that_names_no_timestamp_is_a_typed_error() -> TestResult {
        let clock = clock_2024()?;
        for (orig_date, key, expected) in [
            (b"2A0115", KEY_10_30_45_123, FraudRowError::DateNotDigits),
            (b"22011/", KEY_10_30_45_123, FraudRowError::DateNotDigits),
            (b"221315", KEY_10_30_45_123, FraudRowError::InvalidDate),
            (b"220015", KEY_10_30_45_123, FraudRowError::InvalidDate),
            (b"220132", KEY_10_30_45_123, FraudRowError::InvalidDate),
            (b"220431", KEY_10_30_45_123, FraudRowError::InvalidDate),
            (b"220230", KEY_10_30_45_123, FraudRowError::InvalidDate),
            (b"230229", KEY_10_30_45_123, FraudRowError::InvalidDate),
            (b"221300", KEY_10_30_45_123, FraudRowError::InvalidDate),
            // 25.00.00.000, 24.00.00.001, minute 60, second 60, and the 99 hours of key 0.
            (b"220115", 749_999_999, FraudRowError::InvalidTime),
            (b"220115", 759_999_998, FraudRowError::InvalidTime),
            (b"220115", 893_999_999, FraudRowError::InvalidTime),
            (b"220115", 896_939_999, FraudRowError::InvalidTime),
            (b"220115", 0, FraudRowError::InvalidTime),
            (b"220100", 749_999_999, FraudRowError::InvalidTime),
        ] {
            let result = row_for(*orig_date, key, &clock)?;
            assert_eq!(result, Err(expected));
            assert!(!expected.is_halted());
            assert_eq!(FraudOutcome::from(expected), FraudOutcome::InsertFailed);
        }
        Ok(())
    }

    // audit: COPAUS2C-005
    // BLOCKED (X-COPAUS2C-H4): Db2's result for the hour-24 and day-00 texts is not fixed, so
    // this asserts only the replacement's typed refusal, no parity value.
    #[test]
    fn hour_24_and_day_00_are_refused_as_halted_cases() -> TestResult {
        let clock = clock_2024()?;
        for (orig_date, key, expected) in [
            (b"220100", KEY_10_30_45_123, FraudRowError::DayZero),
            (b"220100", 759_999_999, FraudRowError::DayZero),
            (b"220115", 759_999_999, FraudRowError::HourTwentyFour),
        ] {
            let result = row_for(*orig_date, key, &clock)?;
            assert_eq!(result, Err(expected));
            assert!(expected.is_halted());
        }
        Ok(())
    }

    // audit: COPAUS2C-006, COPAUS2C-007
    #[test]
    fn every_other_column_is_moved_from_the_detail_and_the_commarea() -> TestResult {
        let toggled = detail(*b"220115", KEY_10_30_45_123)?;
        let row = fraud_row(
            &toggled,
            FraudAction::Report,
            PicU::new(12_345_678_901)?,
            PicU::new(987_654_321)?,
            &clock_2024()?,
        )?;
        assert_eq!(row.card_num, toggled.card_num);
        assert_eq!(row.auth_type, toggled.auth_type);
        assert_eq!(row.card_expiry_date, toggled.card_expiry_date);
        assert_eq!(row.message_type, toggled.message_type);
        assert_eq!(row.message_source, toggled.message_source);
        assert_eq!(row.auth_id_code, toggled.auth_id_code);
        assert_eq!(row.auth_resp_code, toggled.auth_resp_code);
        assert_eq!(row.auth_resp_reason, toggled.auth_resp_reason);
        assert_eq!(row.processing_code.as_bytes(), b"001234");
        assert_eq!(row.transaction_amt, toggled.transaction_amt);
        assert_eq!(row.approved_amt, toggled.approved_amt);
        assert_eq!(row.merchant_catagory_code, toggled.merchant_catagory_code);
        assert_eq!(row.acqr_country_code, toggled.acqr_country_code);
        assert_eq!(row.pos_entry_mode, toggled.pos_entry_mode);
        assert_eq!(row.merchant_id, toggled.merchant_id);
        assert_eq!(
            row.merchant_name.as_bytes(),
            b"CORNER GROCERY        ",
            "all 22 bytes, trailing spaces included"
        );
        assert_eq!(row.merchant_city, toggled.merchant_city);
        assert_eq!(row.merchant_state, toggled.merchant_state);
        assert_eq!(row.merchant_zip, toggled.merchant_zip);
        assert_eq!(row.transaction_id, toggled.transaction_id);
        assert_eq!(row.match_status, toggled.match_status);
        assert_eq!(row.acct_id.get(), 12_345_678_901);
        assert_eq!(row.cust_id.get(), 987_654_321);
        Ok(())
    }

    // audit: COPAUS2C-007, COPAUS2C-008, COPAUS2C-009, COPAUS2C-010, COPAUS2C-011
    #[test]
    fn the_insert_and_update_results_give_the_outcome() {
        for (insert, update, expected) in [
            (InsertResult::Inserted, None, FraudOutcome::Added),
            (
                InsertResult::Inserted,
                Some(UpdateResult::Failed),
                FraudOutcome::Added,
            ),
            (
                InsertResult::Duplicate,
                Some(UpdateResult::Updated),
                FraudOutcome::Updated,
            ),
            (
                InsertResult::Duplicate,
                Some(UpdateResult::Failed),
                FraudOutcome::UpdateFailed,
            ),
            (InsertResult::Duplicate, None, FraudOutcome::UpdateFailed),
            (InsertResult::Failed, None, FraudOutcome::InsertFailed),
            (
                InsertResult::Failed,
                Some(UpdateResult::Updated),
                FraudOutcome::InsertFailed,
            ),
        ] {
            assert_eq!(fraud_outcome(insert, update), expected);
            assert_eq!(insert.needs_update(), insert == InsertResult::Duplicate);
        }
        for (outcome, success) in [
            (FraudOutcome::Added, true),
            (FraudOutcome::Updated, true),
            (FraudOutcome::InsertFailed, false),
            (FraudOutcome::UpdateFailed, false),
        ] {
            assert_eq!(outcome.is_success(), success);
        }
    }

    // audit: COPAUS2C-007, COPAUS2C-009
    #[test]
    fn the_success_messages_are_space_padded_to_50() {
        for (outcome, text) in [
            (FraudOutcome::Added, "ADD SUCCESS"),
            (FraudOutcome::Updated, "UPDT SUCCESS"),
        ] {
            let message = outcome.message();
            assert_eq!(message.as_bytes().len(), ACT_MSG_WIDTH);
            assert_eq!(message.to_text(), text);
        }
    }

    // audit: COPAUS2C-010, COPAUS2C-011
    // BLOCKED (H3): the SQLCODE and SQLSTATE values at bytes 25-31 and 41-50 of the insert text
    // and 23-29 and 39-48 of the update text are Db2's; BLOCKED (X-COPAUS2C-H2): bytes 49-50 of
    // the update text. Only the fixed pieces and their positions are asserted.
    #[test]
    fn the_failure_messages_keep_their_fixed_pieces_in_place() {
        for (outcome, prefix, separator_at) in [
            (FraudOutcome::InsertFailed, SYSTEM_ERROR_PREFIX, 31),
            (FraudOutcome::UpdateFailed, UPDT_ERROR_PREFIX, 29),
        ] {
            let message = outcome.message();
            let bytes = message.as_bytes();
            assert_eq!(bytes.get(..prefix.len()), Some(prefix.as_bytes()));
            assert_eq!(
                bytes.get(separator_at..separator_at + STATE_SEPARATOR.len()),
                Some(STATE_SEPARATOR.as_bytes())
            );
        }
        assert_eq!(SYSTEM_ERROR_PREFIX.len(), 24);
        assert_eq!(UPDT_ERROR_PREFIX.len(), 22);
        assert_eq!(STATE_SEPARATOR.len(), 9);
    }

    // audit: COPAUS2C-013
    #[test]
    fn no_text_or_debug_output_discloses_the_card_number() -> TestResult {
        let row = row_for(*b"220115", KEY_10_30_45_123, &clock_2024()?)??;
        let shown = format!("{row:?}");
        assert!(!shown.contains(CARD));
        assert!(shown.contains("************3456"));
        for outcome in [
            FraudOutcome::Added,
            FraudOutcome::Updated,
            FraudOutcome::InsertFailed,
            FraudOutcome::UpdateFailed,
        ] {
            let text = outcome.message().to_text();
            assert!(!text.contains(CARD_LAST4));
            // No code position carries database detail text: only fixed pieces and spaces.
            assert!(
                text.bytes()
                    .filter(u8::is_ascii_digit)
                    .all(|digit| digit == b'2'),
                "only the 2 of DB2 is a digit"
            );
        }
        for error in [
            FraudRowError::DateNotDigits,
            FraudRowError::InvalidDate,
            FraudRowError::InvalidTime,
            FraudRowError::DayZero,
            FraudRowError::HourTwentyFour,
            FraudRowError::YearOutOfRange,
        ] {
            assert!(!error.to_string().bytes().any(|byte| byte.is_ascii_digit()));
        }
        Ok(())
    }
}
