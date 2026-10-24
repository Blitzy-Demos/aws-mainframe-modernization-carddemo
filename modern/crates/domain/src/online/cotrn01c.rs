//! COTRN01C, the transaction view: the rules of endpoint 12,
//! `GET /api/v1/transactions/{tran_id}` (transaction `CT01`, map `COTRN1A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COTRN01C.md` and the server half
//! of its one `both` row: the blank Tran ID (COTRN01C-009), the key as entered (COTRN01C-010),
//! the read and its outcomes (COTRN01C-011 to COTRN01C-013), and the values a successful read
//! shows (COTRN01C-015 to COTRN01C-028). The `client` rows send no request and belong to the
//! screen: the sign-on redirect, the first entry from main-menu option 07 or from an `S`
//! selection on COTRN00, F3 back, F4 clear, F5 to the list, the invalid-key message, the origin
//! kept across passes, the blank detail fields after a failed fetch, the header rows and the
//! message row (COTRN01C-001 to COTRN01C-008, COTRN01C-014, COTRN01C-029, COTRN01C-030).
//!
//! # Flow
//!
//! `api::services::cotrn01c` calls [`edit_tran_id`] on the raw path parameter, reads the
//! `transactions` row whose primary key is the [`PicX`] it returns, states what the read found
//! as a [`TranLookup`] and passes that to [`view`]. The answer is a [`TranView`] or a
//! [`TranViewFailure`], which names the literal the program moves to `WS-MESSAGE`, the request
//! field the cursor goes to and the status class; the service decides nothing else.
//!
//! COTRN01C reads `TRANSACT` with `UPDATE` but never rewrites, deletes or unlocks the record, so
//! the lock lasts only until its task ends (X-COTRN01C-2). The replacement takes no lock and
//! writes nothing, whatever the outcome (COTRN01C-011): this module does no I/O, and the service
//! issues one keyed read. COTRN01C checks no user type, so a `U` and an `A` token get the same
//! view (COTRN01C-015).
//!
//! # Values
//!
//! A [`TranView`] holds one value per detail field of the map: the bytes the screen shows, so
//! the screen formats nothing but the two timestamps.
//!
//! - Text fields keep their `PIC X(n)` bytes, trailing spaces included, cut on the right to the
//!   map width where the map field is narrower: the description to 60 bytes, the merchant name
//!   to 30 and the merchant city to 25 (COTRN01C-021, COTRN01C-026, COTRN01C-027).
//! - The category code and the merchant ID are their digits with leading zeros, as the move of a
//!   `PIC 9(n)` field into an alphanumeric field keeps them (COTRN01C-019, COTRN01C-025).
//! - The amount is the 12-character string of `WS-TRAN-AMT PIC +99999999.99`, whose eight
//!   integer positions drop the ninth digit of `TRAN-AMT PIC S9(09)V99` (COTRN01C-022,
//!   X-COTRN01C-1).
//! - The card number is a [`CardNumber`], whose `Debug`, `Display` and `Serialize` emit only
//!   twelve `*` and its last four characters, for every user type (COTRN01C-017, E32, Cardholder
//!   Data Redaction). No message of this module carries any request or record value.
//! - The two timestamps are typed values, `None` when the stored field is blank; their layout
//!   code is a storage detail and is not returned. The screen shows the date part, the first ten
//!   bytes the COBOL moves (COTRN01C-023, COTRN01C-024, E33, Typed Dates and Timestamps).
//!
//! # Totality
//!
//! [`edit_tran_id`] takes any text, empty, longer than the field or not ASCII, and returns a key
//! or a [`TranViewFailure`]; nothing here indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input).

use core::fmt;

use chrono::NaiveDateTime;
use copybook::PicTimestamp;
use copybook::PicU;
use copybook::PicX;
use copybook::edited::EditedPicture;

use crate::records::transaction::TransactionRecord;
use crate::sensitive::CardNumber;

/// Width of the Tran ID input `TRNIDINI PIC X(16)` and of the key `TRAN-ID PIC X(16)`
/// (COTRN01C-009, COTRN01C-010).
const TRAN_ID_WIDTH: usize = 16;

/// The byte COBOL `SPACES` fills a field with.
const SPACE: u8 = b' ';

/// The byte COBOL `LOW-VALUES` fills a field with.
const LOW_VALUE: u8 = 0x00;

/// ENTER with a blank Tran ID (COTRN01C-009).
const TRAN_ID_EMPTY: &str = "Tran ID can NOT be empty...";

/// The read answered `DFHRESP(NOTFND)` (COTRN01C-012).
const TRAN_ID_NOT_FOUND: &str = "Transaction ID NOT found...";

/// The read failed for any other reason (COTRN01C-013).
const LOOKUP_FAILED: &str = "Unable to lookup Transaction...";

/// The request field a failure names: where the COBOL puts the cursor (`MOVE -1 TO TRNIDINL`).
/// Every failure of endpoint 12 names the Tran ID (COTRN01C-009, COTRN01C-012, COTRN01C-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranViewField {
    /// `TRNIDIN`, the Tran ID input, which endpoint 12 carries as its `tran_id` path parameter.
    TranId,
}

impl TranViewField {
    /// The field's name in the request, `tran_id`: the `field` of the endpoint's `ScreenError`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TranId => "tran_id",
        }
    }
}

/// The status class of a [`TranViewFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The Tran ID is blank: 400 (COTRN01C-009).
    Input,
    /// No transaction has the key: 404 (COTRN01C-012).
    NotFound,
    /// The read failed: 500 (COTRN01C-013).
    Lookup,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`], 404 for
    /// [`FailureClass::NotFound`] and 500 for [`FailureClass::Lookup`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Lookup => 500,
        }
    }
}

/// A screen COTRN01C sends instead of the transaction: its message names no value, and the
/// response carries no transaction field (COTRN01C-009, COTRN01C-012, COTRN01C-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranViewFailure {
    /// ENTER with a Tran ID that is all spaces or all low-values: no read (COTRN01C-009).
    Blank,
    /// No transaction has the key (COTRN01C-012), which includes a text no 16-byte key can equal
    /// (COTRN01C-010).
    NotFound,
    /// The read failed for another reason; in the replacement, a database error (COTRN01C-013).
    LookupFailed,
}

impl TranViewFailure {
    /// The literal moved to `WS-MESSAGE PIC X(80)`, without padding, as the response carries it;
    /// each fits the 78-byte message row (COTRN01C-030).
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Blank => TRAN_ID_EMPTY,
            Self::NotFound => TRAN_ID_NOT_FOUND,
            Self::LookupFailed => LOOKUP_FAILED,
        }
    }

    /// The field the cursor goes to: the Tran ID for every failure.
    #[must_use]
    pub const fn field(self) -> TranViewField {
        match self {
            Self::Blank | Self::NotFound | Self::LookupFailed => TranViewField::TranId,
        }
    }

    /// The status class.
    #[must_use]
    pub const fn class(self) -> FailureClass {
        match self {
            Self::Blank => FailureClass::Input,
            Self::NotFound => FailureClass::NotFound,
            Self::LookupFailed => FailureClass::Lookup,
        }
    }
}

impl fmt::Display for TranViewFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TranViewFailure {}

/// The record key the raw Tran ID names, as `PROCESS-ENTER-KEY` moves `TRNIDINI` to `TRAN-ID`
/// (COTRN01C-009, COTRN01C-010).
///
/// The text is the 16-byte input field right-padded with spaces. A field that is all spaces or
/// all low-values is blank; any other becomes the key exactly as entered, with no trimming of
/// leading spaces, no case change, no digit check and no zero-filling, so an ID entered without
/// its leading zeros is a different key and is not found.
///
/// The map field holds 16 bytes of terminal text, so the program never sees a longer text or a
/// byte outside printable ASCII. The request DTO of endpoint 12 refuses a text longer than 16
/// characters before this edit runs (COTRN01-013); a text within 16 characters can still hold a
/// byte outside `0x20` to `0x7E`, and multi-byte characters can make it longer than 16 bytes.
/// Any such text, or a longer one, that reaches the edit names no stored key: it yields the
/// not-found outcome, never a truncated or cleaned key (R7).
///
/// # Errors
///
/// [`TranViewFailure::Blank`] when the text, right-padded with spaces to 16 bytes, is all spaces
/// or all low-values: the empty text, any text of at most 16 spaces, or exactly sixteen `\0`
/// bytes. [`TranViewFailure::NotFound`] when the text is longer than 16 bytes or holds a byte
/// outside `0x20` to `0x7E`, low-values mixed with other bytes included.
pub fn edit_tran_id(raw: &str) -> Result<PicX<TRAN_ID_WIDTH>, TranViewFailure> {
    let bytes = raw.as_bytes();
    if bytes.len() <= TRAN_ID_WIDTH && is_blank_field(bytes) {
        return Err(TranViewFailure::Blank);
    }
    // A text longer than the field or holding a byte no stored key holds cannot equal any
    // `TRAN-ID`, so it is not found without a read rather than truncated or cleaned (R7).
    PicX::from_text(raw).map_err(|_| TranViewFailure::NotFound)
}

/// Whether `bytes`, at most 16 of them, right-padded with spaces to 16 bytes, are all spaces or
/// all low-values: the `SPACES OR LOW-VALUES` test of `PROCESS-ENTER-KEY` (COTRN01C-009).
/// Padding adds spaces, so the field is all low-values only when all 16 bytes are `\0`.
fn is_blank_field(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == SPACE)
        || (bytes.len() == TRAN_ID_WIDTH && bytes.iter().all(|&byte| byte == LOW_VALUE))
}

/// What the read of the `transactions` row by the key of [`edit_tran_id`] found: the response
/// of `READ-TRANSACT-FILE`'s `EXEC CICS READ` (COTRN01C-011 to COTRN01C-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranLookup<'a> {
    /// `DFHRESP(NORMAL)`: the row with the key (COTRN01C-015).
    Found(&'a TransactionRecord),
    /// `DFHRESP(NOTFND)`: no row has the key (COTRN01C-012).
    NotFound,
    /// Any other response; in the replacement, a database error on the read (COTRN01C-013).
    Error,
}

/// The transaction as the map shows it after a successful read: one value per detail field,
/// moved from `TRAN-RECORD` by `PROCESS-ENTER-KEY` (COTRN01C-015 to COTRN01C-028).
///
/// A view carries no message, because every pass clears `WS-MESSAGE` and a successful read moves
/// none (COTRN01C-015), and it does not depend on the user type. The record's trailing
/// `FILLER PIC X(20)` is not part of it (COTRN01C-028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranView {
    /// `TRNID`, from `TRAN-ID PIC X(16)`: the record's key as stored (COTRN01C-016).
    pub tran_id: PicX<16>,
    /// `CARDNUM`, from `TRAN-CARD-NUM PIC X(16)`: shown and serialised only in its redacted form,
    /// for every user type (COTRN01C-017, E32).
    pub card_num: CardNumber,
    /// `TTYPCD`, from `TRAN-TYPE-CD PIC X(02)`: the 2 bytes as stored (COTRN01C-018).
    pub type_cd: PicX<2>,
    /// `TCATCD PIC X(4)`, from `TRAN-CAT-CD PIC 9(04)`: the four digits with their leading zeros,
    /// category 5 as `0005` (COTRN01C-019).
    pub cat_cd: PicX<4>,
    /// `TRNSRC`, from `TRAN-SOURCE PIC X(10)`: the 10 bytes as stored (COTRN01C-020).
    pub source: PicX<10>,
    /// `TDESC PIC X(60)`, from `TRAN-DESC PIC X(100)`: its first 60 bytes (COTRN01C-021).
    pub desc: PicX<60>,
    /// `TRNAMT PIC X(12)`, from `TRAN-AMT PIC S9(09)V99` through `WS-TRAN-AMT PIC +99999999.99`:
    /// `+00000050.47`, `-00000012.00`, and `+23456789.01` for 123456789.01 (COTRN01C-022,
    /// X-COTRN01C-1).
    pub amount: String,
    /// `TORIGDT`, from `TRAN-ORIG-TS PIC X(26)`: the typed timestamp in any stored layout, `None`
    /// when the field is blank (COTRN01C-023, E33).
    pub orig_ts: Option<NaiveDateTime>,
    /// `TPROCDT`, from `TRAN-PROC-TS PIC X(26)`: as [`TranView::orig_ts`]; a transaction posted
    /// by `posttran` shows its posting instant (COTRN01C-024, E33).
    pub proc_ts: Option<NaiveDateTime>,
    /// `MID PIC X(9)`, from `TRAN-MERCHANT-ID PIC 9(09)`: the nine digits with their leading
    /// zeros (COTRN01C-025).
    pub merchant_id: PicX<9>,
    /// `MNAME PIC X(30)`, from `TRAN-MERCHANT-NAME PIC X(50)`: its first 30 bytes
    /// (COTRN01C-026).
    pub merchant_name: PicX<30>,
    /// `MCITY PIC X(25)`, from `TRAN-MERCHANT-CITY PIC X(50)`: its first 25 bytes
    /// (COTRN01C-027).
    pub merchant_city: PicX<25>,
    /// `MZIP`, from `TRAN-MERCHANT-ZIP PIC X(10)`: the 10 bytes as stored (COTRN01C-028).
    pub merchant_zip: PicX<10>,
}

impl TranView {
    /// The moves of `PROCESS-ENTER-KEY` after a successful read, field by field in the order of
    /// `COTRN01C.cbl:177-190` (COTRN01C-016 to COTRN01C-028).
    fn of(record: &TransactionRecord) -> Self {
        Self {
            tran_id: record.id.clone(),
            card_num: record.card_num.clone(),
            type_cd: record.type_cd.clone(),
            cat_cd: digits_of(record.cat_cd),
            source: record.source.clone(),
            // The alphanumeric MOVE into a shorter map field drops the bytes beyond its width.
            desc: PicX::moved_from(record.desc.as_bytes()),
            // `MOVE TRAN-AMT TO WS-TRAN-AMT`: the editor truncates and drops the high-order digit
            // the picture has no position for, as the COBOL move does (X-COTRN01C-1).
            amount: EditedPicture::Plus9x8Dot99.format(record.amt.as_decimal()),
            orig_ts: typed(record.orig_ts),
            proc_ts: typed(record.proc_ts),
            merchant_id: digits_of(record.merchant_id),
            merchant_name: PicX::moved_from(record.merchant_name.as_bytes()),
            merchant_city: PicX::moved_from(record.merchant_city.as_bytes()),
            merchant_zip: record.merchant_zip.clone(),
        }
    }
}

/// The view of the transaction `lookup` found, or the screen the program sends instead
/// (COTRN01C-011 to COTRN01C-028).
///
/// A found record yields the values of [`TranView`], identical for every user type and with no
/// message (COTRN01C-015 to COTRN01C-028). Nothing is written in any case (COTRN01C-011).
///
/// # Errors
///
/// [`TranViewFailure::NotFound`] for [`TranLookup::NotFound`] (404, COTRN01C-012) and
/// [`TranViewFailure::LookupFailed`] for [`TranLookup::Error`] (500, COTRN01C-013).
pub fn view(lookup: TranLookup<'_>) -> Result<TranView, TranViewFailure> {
    match lookup {
        TranLookup::Found(record) => Ok(TranView::of(record)),
        TranLookup::NotFound => Err(TranViewFailure::NotFound),
        TranLookup::Error => Err(TranViewFailure::LookupFailed),
    }
}

/// A `PIC 9(n)` value moved to an alphanumeric field of the same width: its digits, leading
/// zeros included (COTRN01C-019, COTRN01C-025).
fn digits_of<const DIGITS: u32, const WIDTH: usize>(value: PicU<DIGITS>) -> PicX<WIDTH> {
    let mut digits = Vec::with_capacity(WIDTH);
    value.encode_into(&mut digits);
    PicX::moved_from(&digits)
}

/// A stored timestamp as the typed value the response carries, `None` for a blank field; the
/// layout code stays behind (COTRN01C-023, COTRN01C-024).
fn typed(field: PicTimestamp) -> Option<NaiveDateTime> {
    field.value().map(|stamp| stamp.value())
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};

    use super::{FailureClass, TranLookup, TranView, TranViewFailure, TranViewField};
    use super::{edit_tran_id, view};
    use crate::money::{Decimal, Money};
    use crate::records::transaction::TransactionRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field and HTTP status.
    type Shown = (&'static str, &'static str, u16);

    // The audited literals, written out apart from the module's own constants.
    const EMPTY: &str = "Tran ID can NOT be empty...";
    const NOT_FOUND: &str = "Transaction ID NOT found...";
    const LOOKUP: &str = "Unable to lookup Transaction...";

    /// `TRAN-CARD-NUM` of the test record and its redacted form (PIC-024).
    const CARD: &str = "4859452612877065";
    const CARD_REDACTED: &str = "************7065";

    /// The key of the test record, `dailytran.txt` line 1.
    const TRAN_ID: &str = "0000000000683580";

    fn shown(failure: TranViewFailure) -> Shown {
        (
            failure.message(),
            failure.field().name(),
            failure.class().http_status(),
        )
    }

    fn pic<const N: usize>(text: &str) -> Result<PicX<N>, String> {
        PicX::from_text(text).map_err(|error| format!("{text:?}: {error:?}"))
    }

    fn money(units: i64, scale: u32) -> Result<Money<9>, String> {
        Money::new(Decimal::new(units, scale)).map_err(|error| format!("{units}: {error:?}"))
    }

    /// The instant `date` at `time` and `micros` microseconds.
    fn at(
        (year, month, day): (i32, u32, u32),
        (hour, minute, second): (u32, u32, u32),
        micros: u32,
    ) -> Result<NaiveDateTime, String> {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_micro_opt(hour, minute, second, micros))
            .ok_or_else(|| format!("no instant {year}-{month}-{day} {hour}:{minute}:{second}"))
    }

    /// A stored timestamp holding `value` in layout `form`.
    fn stamp(value: NaiveDateTime, form: TsForm) -> Result<PicTimestamp, String> {
        CobolTimestamp::new(value, form)
            .map(|stamp| PicTimestamp::new(Some(stamp)))
            .map_err(|error| format!("{value} {form:?}: {error:?}"))
    }

    /// `dailytran.txt` line 1 as a `transactions` row, with a description, merchant name and
    /// city filled past the map widths so that the cut is visible, and a `posttran` processing
    /// timestamp in layout `D`.
    fn record() -> Result<TransactionRecord, String> {
        let desc = format!("{:<60}{}", "Purchase at Abshire-Lowe", "D".repeat(40));
        let name = format!("{:<30}{}", "Abshire-Lowe", "N".repeat(20));
        let city = format!("{:<25}{}", "North Enoshaven", "C".repeat(25));
        Ok(TransactionRecord {
            id: pic(TRAN_ID)?,
            type_cd: pic("01")?,
            cat_cd: PicU::new(1).map_err(|error| format!("{error:?}"))?,
            source: pic("POS TERM")?,
            desc: pic(&desc)?,
            amt: money(5047, 2)?,
            merchant_id: PicU::new(800_000_000).map_err(|error| format!("{error:?}"))?,
            merchant_name: pic(&name)?,
            merchant_city: pic(&city)?,
            merchant_zip: pic("72112")?,
            card_num: CardNumber::parse(CARD).map_err(|error| error.to_string())?,
            orig_ts: stamp(at((2022, 6, 10), (19, 27, 53), 0)?, TsForm::Iso)?,
            proc_ts: stamp(at((2022, 7, 18), (10, 15, 42), 370_000)?, TsForm::Db2)?,
            filler: pic(&"F".repeat(20))?,
        })
    }

    fn view_of(record: &TransactionRecord) -> Result<TranView, String> {
        view(TranLookup::Found(record)).map_err(|failure| failure.to_string())
    }

    // audit: COTRN01C-009
    #[test]
    fn a_blank_tran_id_is_rejected_before_any_read() {
        let sixteen_nuls = "\0".repeat(16);
        let blanks = ["", " ", "                ", sixteen_nuls.as_str()];
        for raw in blanks {
            assert_eq!(edit_tran_id(raw), Err(TranViewFailure::Blank), "{raw:?}");
        }
        assert_eq!(shown(TranViewFailure::Blank), (EMPTY, "tran_id", 400));
        assert_eq!(TranViewFailure::Blank.to_string(), EMPTY);
        assert_eq!(TranViewFailure::Blank.class(), FailureClass::Input);
    }

    // audit: COTRN01C-010
    #[test]
    fn a_tran_id_becomes_the_key_exactly_as_entered_padded_to_sixteen() -> TestResult {
        let cases: [(&str, &[u8; 16]); 4] = [
            (TRAN_ID, b"0000000000683580"),
            // No zero-filling: a short ID is a different, space-padded key.
            ("683580", b"683580          "),
            // No trimming of leading spaces and no case change.
            (" abc", b" abc            "),
            // Spaces inside or after the ID are kept as entered.
            ("6835 80 ", b"6835 80         "),
        ];
        for (raw, key) in cases {
            assert_eq!(
                edit_tran_id(raw)
                    .map_err(|failure| failure.to_string())?
                    .as_bytes(),
                key,
                "{raw:?}"
            );
        }
        Ok(())
    }

    // audit: COTRN01C-010
    // audit: COTRN01C-012
    #[test]
    fn a_text_no_sixteen_byte_key_can_equal_is_not_found_without_a_read() {
        let four_nuls = "\0".repeat(4);
        let mixed = format!("{}{}", "\0".repeat(8), " ".repeat(8));
        let seventeen_spaces = " ".repeat(17);
        let unreadable = [
            "00000000006835801",
            seventeen_spaces.as_str(),
            // Low-values padded with spaces are neither all spaces nor all low-values.
            four_nuls.as_str(),
            mixed.as_str(),
            "\t683580",
            "68358é",
        ];
        for raw in unreadable {
            assert_eq!(edit_tran_id(raw), Err(TranViewFailure::NotFound), "{raw:?}");
        }
    }

    // audit: COTRN01C-012
    // audit: COTRN01C-013
    #[test]
    fn a_missing_or_failed_read_carries_only_its_message() {
        assert_eq!(view(TranLookup::NotFound), Err(TranViewFailure::NotFound));
        assert_eq!(view(TranLookup::Error), Err(TranViewFailure::LookupFailed));
        assert_eq!(
            shown(TranViewFailure::NotFound),
            (NOT_FOUND, "tran_id", 404)
        );
        assert_eq!(
            shown(TranViewFailure::LookupFailed),
            (LOOKUP, "tran_id", 500)
        );
        assert_eq!(TranViewFailure::NotFound.field(), TranViewField::TranId);
        assert_eq!(TranViewFailure::LookupFailed.to_string(), LOOKUP);
    }

    // audit: COTRN01C-015
    // audit: COTRN01C-016
    // audit: COTRN01C-018
    // audit: COTRN01C-019
    // audit: COTRN01C-020
    // audit: COTRN01C-021
    // audit: COTRN01C-025
    // audit: COTRN01C-026
    // audit: COTRN01C-027
    // audit: COTRN01C-028
    #[test]
    fn a_found_record_shows_one_value_per_map_field() -> TestResult {
        let shown = view_of(&record()?)?;
        assert_eq!(shown.tran_id.as_bytes(), TRAN_ID.as_bytes());
        assert_eq!(shown.type_cd.as_bytes(), b"01");
        assert_eq!(shown.cat_cd.as_bytes(), b"0001");
        assert_eq!(shown.source.as_bytes(), b"POS TERM  ");
        let desc = format!("{:<60}", "Purchase at Abshire-Lowe");
        assert_eq!(shown.desc.as_bytes(), desc.as_bytes());
        assert_eq!(shown.amount, "+00000050.47");
        assert_eq!(shown.merchant_id.as_bytes(), b"800000000");
        let name = format!("{:<30}", "Abshire-Lowe");
        assert_eq!(shown.merchant_name.as_bytes(), name.as_bytes());
        let city = format!("{:<25}", "North Enoshaven");
        assert_eq!(shown.merchant_city.as_bytes(), city.as_bytes());
        assert_eq!(shown.merchant_zip.as_bytes(), b"72112     ");
        Ok(())
    }

    // audit: COTRN01C-019
    // audit: COTRN01C-025
    #[test]
    fn codes_and_ids_keep_their_leading_zeros() -> TestResult {
        let mut stored = record()?;
        stored.cat_cd = PicU::new(5).map_err(|error| format!("{error:?}"))?;
        stored.merchant_id = PicU::new(42).map_err(|error| format!("{error:?}"))?;
        let shown = view_of(&stored)?;
        assert_eq!(shown.cat_cd.as_bytes(), b"0005");
        assert_eq!(shown.merchant_id.as_bytes(), b"000000042");
        Ok(())
    }

    // audit: COTRN01C-022
    #[test]
    fn the_amount_is_edited_by_ws_tran_amt_dropping_the_ninth_digit() -> TestResult {
        let cases: [(i64, &str); 6] = [
            (5047, "+00000050.47"),
            (0, "+00000000.00"),
            (-1200, "-00000012.00"),
            (12_345_678_901, "+23456789.01"),
            (9_999_999_999, "+99999999.99"),
            (-99_999_999_999, "-99999999.99"),
        ];
        for (cents, edited) in cases {
            let mut stored = record()?;
            stored.amt = money(cents, 2)?;
            let shown = view_of(&stored)?;
            assert_eq!(shown.amount, edited, "{cents}");
            assert_eq!(shown.amount.len(), 12);
        }
        Ok(())
    }

    // audit: COTRN01C-023
    // audit: COTRN01C-024
    #[test]
    fn timestamps_are_typed_in_every_layout_and_none_when_blank() -> TestResult {
        let iso = at((2022, 6, 10), (19, 27, 53), 0)?;
        let db2 = at((2022, 7, 18), (10, 15, 42), 370_000)?;
        let screen = at((2022, 7, 18), (0, 0, 0), 0)?;
        let cases = [
            (stamp(iso, TsForm::Iso)?, Some(iso)),
            (stamp(db2, TsForm::Db2)?, Some(db2)),
            (stamp(screen, TsForm::ScreenDate)?, Some(screen)),
            (PicTimestamp::BLANK, None),
        ];
        for (field, expected) in cases {
            let mut stored = record()?;
            stored.orig_ts = field;
            stored.proc_ts = field;
            let shown = view_of(&stored)?;
            assert_eq!(shown.orig_ts, expected);
            assert_eq!(shown.proc_ts, expected);
        }
        let shown = view_of(&record()?)?;
        assert_eq!(shown.orig_ts, Some(iso));
        assert_eq!(shown.proc_ts, Some(db2));
        Ok(())
    }

    // audit: COTRN01C-017
    #[test]
    fn the_card_number_leaves_only_in_its_redacted_form() -> TestResult {
        let shown = view_of(&record()?)?;
        assert_eq!(shown.card_num.redacted(), CARD_REDACTED);
        assert_eq!(shown.card_num.to_string(), CARD_REDACTED);
        let debug = format!("{shown:?}");
        assert!(!debug.contains(CARD), "{debug}");
        assert!(debug.contains(CARD_REDACTED), "{debug}");
        for failure in [
            TranViewFailure::Blank,
            TranViewFailure::NotFound,
            TranViewFailure::LookupFailed,
        ] {
            assert!(!failure.message().contains(CARD));
        }
        Ok(())
    }
}
