//! COTRN02C, transaction add: the rules of endpoint 13, `POST /api/v1/transactions`
//! (transaction `CT02`, map `COTRN2A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COTRN02C.md`: the key-field
//! edits and the cross-reference fill-in (COTRN02C-009 to COTRN02C-019), the data-field edits
//! (COTRN02C-020 to COTRN02C-039), the confirmation (COTRN02C-040 to COTRN02C-042), the record
//! the add inserts and the messages of its browse and write (COTRN02C-043 to COTRN02C-051), the
//! PF5 copy (COTRN02C-052 to COTRN02C-056) and what a 200 response carries (COTRN02C-057). The
//! `client` rows, F3, F4 and every other key among them (COTRN02C-004 to COTRN02C-006), send no
//! request and belong to the screen; an `aid` other than ENTER and PF5 never reaches this module,
//! because the request DTO answers it (COTRN02C-007).
//!
//! # Flow
//!
//! `api::services::cotrn02c` calls [`edit_keys`], reads the cross-reference the [`KeyRequest`]
//! names (by account through the `CXACAIX` path, or by card) and hands the result to
//! [`fill_key`]. On PF5 it reads the transaction with the highest ID and replaces the data fields
//! with [`copy_last`]. It then calls [`edit_data`] and [`confirm`]: a blank, `N` or `n`
//! confirmation answers with [`AddOutcome::prompt`]; `Y` or `y` takes the next ID
//! (`TransactionRepository::next_id`, highest plus one), inserts [`build_record`] and answers
//! with [`AddOutcome::added`]. Store results with an audited message become the [`AddFailure`]
//! constructors of this module. Nothing here reads or writes a store.
//!
//! Every send of the COBOL map ends the task with `EXEC CICS RETURN`, so the first message the
//! program sets is the response and nothing after it runs (COTRN02C-008): every edit returns
//! the first failure in the program's order, and the service writes nothing unless every edit
//! passed and the confirmation is `Y`.
//!
//! # Input
//!
//! The request fields enter as the raw text of their map fields ([`TranAddInput`]), the
//! raw-edit-input exception of AAP 0.3.4.1, so that the program's own edit messages stay
//! reproducible. Each is read as a `MOVE` into its `PIC X(n)` map field: its first `n`
//! characters, padded with spaces. A field is blank when it is all spaces or all low-values
//! (`\0`), the COBOL's `SPACES OR LOW-VALUES`.
//!
//! # Rules applied
//!
//! - Cardholder Data Redaction (`E32`): a card number lives only in [`CardNumber`], whose
//!   `Debug`, `Display` and `Serialize` print the redacted form, so the key the program fills in
//!   from the cross-reference leaves the API redacted (COTRN02C-012, COTRN02C-017). No message
//!   holds a request value.
//! - Typed Dates and Timestamps (`E33`): the two dates leave the edits as typed values, and the
//!   record stores them as layout-`S` timestamps, the date followed by 16 spaces (COTRN02C-046,
//!   PIC-020).
//! - Money: the amount is the exact decimal value of its edited text, stored into
//!   `S9(09)V99` by [`MoneyExpr::store`]; nothing here uses floating point (COTRN02C-047).
//!
//! # Halts
//!
//! - `HALTED(H5)`: which dates CSUTLDTC accepts is the verdict of `CEEDAYS`, outside the
//!   repository (COTRN02C-037, COTRN02C-038). [`edit_data`] passes the stand-in
//!   [`validate`], which is no parity result; [`edit_data_with`] takes the verdict as a
//!   parameter, so that a test substitutes it and proves the rows that hold for every verdict.
//! - Halt candidate X-COTRN02C-H2: the amount `-00000000.00` is a negative zero, whose COBOL
//!   value goes through floating point. The replacement stores 0.00 and re-edits it
//!   `+00000000.00`, which is no parity result (COTRN02C-036).
//! - Halt candidate X-ARITH-5: a card number above 2^53 converts through floating point in the
//!   COBOL. The replacement keys the read with the 16 digits as entered (COTRN02C-016).
//! - Halt candidate X-COTRN02C-H1: on an empty transaction table the browse has no single
//!   result. The replacement's choices are [`AddFailure::last_not_found`] for PF5 and ID 1 for
//!   an add (COTRN02C-044, COTRN02C-053), and the record's FILLER is then spaces.
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or an [`AddFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input). Digits are read with `char::to_digit` and checked arithmetic.

use core::fmt;

use chrono::{NaiveDate, NaiveTime};
use copybook::edited::EditedPicture;
use copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};

use super::csutldtc::{DateMask, LeResult, caller_accepts, validate};
use crate::money::{Decimal, Money, MoneyExpr};
use crate::records::card_xref::CardXrefRecord;
use crate::records::transaction::TransactionRecord;
use crate::sensitive::CardNumber;

/// Width of `ACTIDINI PIC X(11)` (COTRN02C-010).
const ACCT_WIDTH: usize = 11;

/// Width of `CARDNINI PIC X(16)` (COTRN02C-015).
const CARD_WIDTH: usize = 16;

/// Width of `TTYPCDI PIC X(2)` (COTRN02C-020, COTRN02C-031).
const TYPE_WIDTH: usize = 2;

/// Width of `TCATCDI PIC X(4)` (COTRN02C-021, COTRN02C-032).
const CAT_WIDTH: usize = 4;

/// Width of `TRNSRCI PIC X(10)` (COTRN02C-022).
const SOURCE_WIDTH: usize = 10;

/// Width of `TDESCI PIC X(60)` (COTRN02C-023).
const DESC_WIDTH: usize = 60;

/// Width of `TRNAMTI PIC X(12)` (COTRN02C-024, COTRN02C-033).
const AMOUNT_WIDTH: usize = 12;

/// Width of `TORIGDTI` and `TPROCDTI PIC X(10)` (COTRN02C-025, COTRN02C-026).
const DATE_WIDTH: usize = 10;

/// Width of `MIDI PIC X(9)` (COTRN02C-027, COTRN02C-039).
const MID_WIDTH: usize = 9;

/// Width of `MNAMEI PIC X(30)` (COTRN02C-028).
const NAME_WIDTH: usize = 30;

/// Width of `MCITYI PIC X(25)` (COTRN02C-029).
const CITY_WIDTH: usize = 25;

/// Width of `MZIPI PIC X(10)` (COTRN02C-030).
const ZIP_WIDTH: usize = 10;

/// Width of `CONFIRMI PIC X(1)` (COTRN02C-040).
const CONFIRM_WIDTH: usize = 1;

/// Bytes of a `PIC X(26)` timestamp, of which a copy keeps the first [`DATE_WIDTH`]
/// (COTRN02C-054).
const TIMESTAMP_WIDTH: usize = 26;

/// Scale of `WS-TRAN-AMT-N PIC S9(9)V99`.
const AMOUNT_SCALE: u32 = 2;

/// Key-field edits (COTRN02C-009, COTRN02C-010, COTRN02C-015).
const KEY_MISSING: &str = "Account or Card Number must be entered...";
const ACCT_NOT_NUMERIC: &str = "Account ID must be Numeric...";
const CARD_NOT_NUMERIC: &str = "Card Number must be Numeric...";

/// Cross-reference reads (COTRN02C-013, COTRN02C-014, COTRN02C-018, COTRN02C-019).
const ACCT_NOT_FOUND: &str = "Account ID NOT found...";
const ACCT_LOOKUP_FAILED: &str = "Unable to lookup Acct in XREF AIX file...";
const CARD_NOT_FOUND: &str = "Card Number NOT found...";
const CARD_LOOKUP_FAILED: &str = "Unable to lookup Card # in XREF file...";

/// Emptiness edits, in program order (COTRN02C-020 to COTRN02C-030).
const TYPE_EMPTY: &str = "Type CD can NOT be empty...";
const CAT_EMPTY: &str = "Category CD can NOT be empty...";
const SOURCE_EMPTY: &str = "Source can NOT be empty...";
const DESC_EMPTY: &str = "Description can NOT be empty...";
const AMOUNT_EMPTY: &str = "Amount can NOT be empty...";
const ORIG_EMPTY: &str = "Orig Date can NOT be empty...";
const PROC_EMPTY: &str = "Proc Date can NOT be empty...";
const MID_EMPTY: &str = "Merchant ID can NOT be empty...";
const NAME_EMPTY: &str = "Merchant Name can NOT be empty...";
const CITY_EMPTY: &str = "Merchant City can NOT be empty...";
const ZIP_EMPTY: &str = "Merchant Zip can NOT be empty...";

/// Numeric code edits (COTRN02C-031, COTRN02C-032).
const TYPE_NOT_NUMERIC: &str = "Type CD must be Numeric...";
const CAT_NOT_NUMERIC: &str = "Category CD must be Numeric...";

/// Amount shape edit (COTRN02C-033).
const AMOUNT_FORMAT: &str = "Amount should be in format -99999999.99";

/// Date shape edits (COTRN02C-034, COTRN02C-035).
const ORIG_FORMAT: &str = "Orig Date should be in format YYYY-MM-DD";
const PROC_FORMAT: &str = "Proc Date should be in format YYYY-MM-DD";

/// Date validity (COTRN02C-037, COTRN02C-038).
const ORIG_INVALID: &str = "Orig Date - Not a valid date...";
const PROC_INVALID: &str = "Proc Date - Not a valid date...";

/// Merchant ID edit, the last data edit (COTRN02C-039).
const MID_NOT_NUMERIC: &str = "Merchant ID must be Numeric...";

/// A free-text field holds a character no `PIC X(n)` field of the record can store: outside
/// printable ASCII (PIC-001, PIC-002). The terminal sent only the characters of its code page,
/// so no COBOL message covers the case; this is the request-validation text escalation
/// candidate X-COUSR01C-3 proposes for every online endpoint, and no parity is claimed for it.
const NOT_STORABLE: &str = "Only printable ASCII characters can be stored...";

/// The confirmation prompt (COTRN02C-041).
const CONFIRM_PROMPT: &str = "Confirm to add this transaction...";

/// A confirmation the program does not know (COTRN02C-042).
const CONFIRM_INVALID: &str = "Invalid value. Valid values are (Y/N)...";

/// The highest-ID browse (COTRN02C-044, COTRN02C-053).
const TRAN_NOT_FOUND: &str = "Transaction ID NOT found...";
const TRAN_LOOKUP_FAILED: &str = "Unable to lookup Transaction...";

/// The write (COTRN02C-049 to COTRN02C-051). The first literal ends in a space and the second
/// begins with one, so two spaces follow `successfully.`.
const ADDED_HEAD: &str = "Transaction added successfully. ";
const ADDED_ID_INTRO: &str = " Your Tran ID is ";
const ADDED_TAIL: &str = ".";
const DUPLICATE_ID: &str = "Tran ID already exist...";
const WRITE_FAILED: &str = "Unable to Add Transaction...";

/// The key a request carries, and the field the COBOL puts the cursor on: the response's
/// `field` (`MOVE -1 TO <field>L`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddField {
    /// `ACTIDIN`, the request's `acct_id`: the key edits, the account path and every browse or
    /// write failure (COTRN02C-009 to COTRN02C-014, COTRN02C-044, COTRN02C-050, COTRN02C-051),
    /// and the cursor after an add (COTRN02C-049).
    AcctId,
    /// `CARDNIN`, the request's `card_num`: the card path (COTRN02C-015 to COTRN02C-019).
    CardNum,
    /// `TTYPCD` (COTRN02C-020, COTRN02C-031).
    TypeCd,
    /// `TCATCD` (COTRN02C-021, COTRN02C-032).
    CatCd,
    /// `TRNSRC` (COTRN02C-022).
    Source,
    /// `TDESC` (COTRN02C-023).
    Description,
    /// `TRNAMT` (COTRN02C-024, COTRN02C-033).
    Amount,
    /// `TORIGDT` (COTRN02C-025, COTRN02C-034, COTRN02C-037).
    OrigDate,
    /// `TPROCDT` (COTRN02C-026, COTRN02C-035, COTRN02C-038).
    ProcDate,
    /// `MID` (COTRN02C-027, COTRN02C-039).
    MerchantId,
    /// `MNAME` (COTRN02C-028).
    MerchantName,
    /// `MCITY` (COTRN02C-029).
    MerchantCity,
    /// `MZIP` (COTRN02C-030).
    MerchantZip,
    /// `CONFIRM`: the prompt and an unknown value (COTRN02C-041, COTRN02C-042).
    Confirm,
}

/// The status class of an [`AddFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit of the request failed: 400.
    Input,
    /// The cross-reference holds no row for the key, or PF5 found no transaction: 404
    /// (COTRN02C-013, COTRN02C-018, COTRN02C-053).
    NotFound,
    /// A read or the write failed: 500 (COTRN02C-014, COTRN02C-019, COTRN02C-044, COTRN02C-050,
    /// COTRN02C-051).
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

/// A screen COTRN02C sends instead of adding: the message, the field the cursor goes to and the
/// status class (COTRN02C-008).
///
/// The message is the literal moved to `WS-MESSAGE PIC X(80)` without its trailing spaces, as
/// the response carries it. None holds a request value, so a failure can be logged or returned
/// as it stands (Cardholder Data Redaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AddFailure {
    message: &'static str,
    field: AddField,
    class: FailureClass,
}

impl AddFailure {
    /// A failed edit of the request: [`FailureClass::Input`].
    const fn input(message: &'static str, field: AddField) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Input,
        }
    }

    /// No row for the key: [`FailureClass::NotFound`].
    const fn not_found(message: &'static str, field: AddField) -> Self {
        Self {
            message,
            field,
            class: FailureClass::NotFound,
        }
    }

    /// A failed read or write: [`FailureClass::Store`].
    const fn store(message: &'static str, field: AddField) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Store,
        }
    }

    /// The highest-ID browse failed, on the add or on PF5: `Unable to lookup Transaction...` on
    /// `acct_id`, status 500 (COTRN02C-044, COTRN02C-053). The service answers a failed
    /// `TransactionRepository::next_id` or `find_last` with it.
    #[must_use]
    pub const fn transaction_lookup() -> Self {
        Self::store(TRAN_LOOKUP_FAILED, AddField::AcctId)
    }

    /// PF5 found no transaction to copy: `Transaction ID NOT found...` on `acct_id`, status 404
    /// (COTRN02C-053). On an empty table the COBOL outcome is halt candidate X-COTRN02C-H1; this
    /// is the replacement's choice of its `STARTBR` branch and no parity result.
    #[must_use]
    pub const fn last_not_found() -> Self {
        Self::not_found(TRAN_NOT_FOUND, AddField::AcctId)
    }

    /// The insert found a transaction with the new ID already there (`DUPREC`, a concurrent add
    /// or bill payment that took the same ID first): `Tran ID already exist...` on `acct_id`,
    /// status 500 (COTRN02C-050).
    #[must_use]
    pub const fn duplicate_id() -> Self {
        Self::store(DUPLICATE_ID, AddField::AcctId)
    }

    /// The insert failed for any other reason: `Unable to Add Transaction...` on `acct_id`,
    /// status 500 (COTRN02C-051).
    #[must_use]
    pub const fn write_failed() -> Self {
        Self::store(WRITE_FAILED, AddField::AcctId)
    }

    /// A confirmation other than `Y`, `y`, `N`, `n` and blank: `Invalid value. Valid values are
    /// (Y/N)...` on `confirm`, status 400 (COTRN02C-042), for [`Confirmation::Invalid`].
    #[must_use]
    pub const fn invalid_confirmation() -> Self {
        Self::input(CONFIRM_INVALID, AddField::Confirm)
    }

    /// The message, as the response carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the cursor goes to.
    #[must_use]
    pub const fn field(&self) -> AddField {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for AddFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for AddFailure {}

/// The two attention keys endpoint 13 serves (COTRN02C-007). Any other key the screen handles
/// itself or the request DTO refuses, so it never reaches this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aid {
    /// ENTER, `PROCESS-ENTER-KEY`: the key edits, the data edits on the request's fields, then
    /// the confirmation (COTRN02C-009 to COTRN02C-042).
    Enter,
    /// PF5, `COPY-LAST-TRAN-DATA`: the key edits, the copy of the highest-ID transaction's data
    /// fields ([`copy_last`]), then the ENTER processing on the copied fields (COTRN02C-052 to
    /// COTRN02C-056).
    Pf5,
}

/// The key the request names, which decides the cross-reference read the service performs
/// (COTRN02C-011, COTRN02C-016).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyRequest {
    /// The account path: read `card_xref` by `acct_id`, the `CXACAIX` path (COTRN02C-011).
    Account(PicU<ACCT_DIGITS>),
    /// The card path: read `card_xref` by `card_num` (COTRN02C-016).
    Card(CardNumber),
}

/// Digits of `WS-ACCT-ID-N PIC 9(11)` and `XREF-ACCT-ID PIC 9(11)` (CVACT03Y-003).
const ACCT_DIGITS: u32 = 11;

/// Digits of `TRAN-CAT-CD PIC 9(04)` (CVTRA05Y-003).
const CAT_DIGITS: u32 = 4;

/// Digits of `TRAN-MERCHANT-ID PIC 9(09)` (CVTRA05Y-007).
const MID_DIGITS: u32 = 9;

/// The outcome of the cross-reference read a [`KeyRequest`] asks for, as the service maps the
/// repository's answer (`READ-CXACAIX-FILE`, `READ-CCXREF-FILE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefLookup<'a> {
    /// `NORMAL`: exactly one row for the key (COTRN02C-012, COTRN02C-017).
    Found(&'a CardXrefRecord),
    /// `NOTFND`: no row for the key (COTRN02C-013, COTRN02C-018).
    NotFound,
    /// Any other condition: a database error, or two or more rows for an account, the `DUPKEY`
    /// the `NONUNIQUEKEY` path allows (COTRN02C-014, COTRN02C-019).
    Error,
}

/// The key once the cross-reference has filled it in: the account and the card of the
/// transaction (COTRN02C-012, COTRN02C-017).
///
/// `Debug` shows the card number redacted, and so does every serialisation of
/// [`FilledKey::card_num`] (`E32`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FilledKey {
    acct_id: PicU<ACCT_DIGITS>,
    card_num: CardNumber,
}

impl FilledKey {
    /// The account: the 11 digits entered on the account path, or `XREF-ACCT-ID` on the card
    /// path. The response carries it as 11 digits with leading zeros (COTRN02C-057). It is
    /// never stored: `CVTRA05Y` has no account field (COTRN02C-048).
    #[must_use]
    pub const fn acct_id(&self) -> PicU<ACCT_DIGITS> {
        self.acct_id
    }

    /// The card of the transaction: `XREF-CARD-NUM` on the account path, or the 16 digits entered
    /// on the card path. It is stored in full in `transactions.card_num` (COTRN02C-045) and
    /// leaves the API only redacted (COTRN02C-057, `E32`).
    #[must_use]
    pub const fn card_num(&self) -> &CardNumber {
        &self.card_num
    }
}

/// `VALIDATE-INPUT-KEY-FIELDS` before its read: the key the request names (COTRN02C-009 to
/// COTRN02C-011, COTRN02C-015, COTRN02C-016). `acct_id` and `card_num` are the raw request
/// fields; `None` is an absent field.
///
/// A non-blank account decides, whatever the card field holds; only a blank account lets the
/// card field decide. The account's 11 digits are its exact value. The card's 16 digits are
/// kept as entered: above 2^53 the COBOL `NUMVAL` result depends on the deployed floating-point
/// format (halt candidate X-ARITH-5), so that keying is no parity result.
///
/// # Errors
///
/// Status class [`FailureClass::Input`]:
///
/// - a non-blank account that is not 11 digits: `Account ID must be Numeric...` on `acct_id`
///   (COTRN02C-010);
/// - a blank account and a non-blank card that is not 16 digits: `Card Number must be
///   Numeric...` on `card_num` (COTRN02C-015);
/// - both blank: `Account or Card Number must be entered...` on `acct_id` (COTRN02C-009).
pub fn edit_keys(acct_id: Option<&str>, card_num: Option<&str>) -> Result<KeyRequest, AddFailure> {
    let account = field::<ACCT_WIDTH>(acct_id.unwrap_or(""));
    if !is_blank(&account) {
        return digits_value(&account)
            .and_then(|value| PicU::new(value).ok())
            .map(KeyRequest::Account)
            .ok_or(AddFailure::input(ACCT_NOT_NUMERIC, AddField::AcctId));
    }
    let card = field::<CARD_WIDTH>(card_num.unwrap_or(""));
    if !is_blank(&card) {
        return digit_bytes(&card)
            .map(|digits| KeyRequest::Card(CardNumber::new(PicX::from_array(digits))))
            .ok_or(AddFailure::input(CARD_NOT_NUMERIC, AddField::CardNum));
    }
    Err(AddFailure::input(KEY_MISSING, AddField::AcctId))
}

/// `READ-CXACAIX-FILE` or `READ-CCXREF-FILE` after the read: the filled-in key, or the read's
/// message (COTRN02C-012 to COTRN02C-014, COTRN02C-017 to COTRN02C-019).
///
/// On the account path the cross-reference supplies the card, and on the card path it supplies
/// the account. Neither the account file nor the card file is read: an account with no
/// cross-reference is not found, and a card with one is accepted whatever its status
/// (COTRN02C-011, COTRN02C-018).
///
/// # Errors
///
/// - the account path and [`XrefLookup::NotFound`]: `Account ID NOT found...` on `acct_id`,
///   status 404 (COTRN02C-013);
/// - the account path and [`XrefLookup::Error`]: `Unable to lookup Acct in XREF AIX file...` on
///   `acct_id`, status 500 (COTRN02C-014);
/// - the card path and [`XrefLookup::NotFound`]: `Card Number NOT found...` on `card_num`,
///   status 404 (COTRN02C-018);
/// - the card path and [`XrefLookup::Error`]: `Unable to lookup Card # in XREF file...` on
///   `card_num`, status 500 (COTRN02C-019).
pub fn fill_key(request: KeyRequest, lookup: XrefLookup<'_>) -> Result<FilledKey, AddFailure> {
    match (request, lookup) {
        (KeyRequest::Account(acct_id), XrefLookup::Found(xref)) => Ok(FilledKey {
            acct_id,
            card_num: xref.card_num.clone(),
        }),
        (KeyRequest::Account(_), XrefLookup::NotFound) => {
            Err(AddFailure::not_found(ACCT_NOT_FOUND, AddField::AcctId))
        }
        (KeyRequest::Account(_), XrefLookup::Error) => {
            Err(AddFailure::store(ACCT_LOOKUP_FAILED, AddField::AcctId))
        }
        (KeyRequest::Card(card_num), XrefLookup::Found(xref)) => Ok(FilledKey {
            acct_id: xref.acct_id,
            card_num,
        }),
        (KeyRequest::Card(_), XrefLookup::NotFound) => {
            Err(AddFailure::not_found(CARD_NOT_FOUND, AddField::CardNum))
        }
        (KeyRequest::Card(_), XrefLookup::Error) => {
            Err(AddFailure::store(CARD_LOOKUP_FAILED, AddField::CardNum))
        }
    }
}

/// The eleven data fields of `COTRN2A` and the confirmation, each the raw text of its map field
/// (`TranAddRequest.fields` and `.confirm` of endpoint 13; COTRN02C-060). An absent field is
/// the empty text.
///
/// The two dates stay raw text because COTRN02C's date edits own their messages (AAP 0.3.4.1);
/// [`edit_data`] turns them into typed values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranAddInput<'a> {
    /// `TTYPCD`, `PIC X(2)`.
    pub type_cd: &'a str,
    /// `TCATCD`, `PIC X(4)`.
    pub cat_cd: &'a str,
    /// `TRNSRC`, `PIC X(10)`.
    pub source: &'a str,
    /// `TDESC`, `PIC X(60)`.
    pub description: &'a str,
    /// `TRNAMT`, `PIC X(12)`.
    pub amount: &'a str,
    /// `TORIGDT`, `PIC X(10)`.
    pub orig_date: &'a str,
    /// `TPROCDT`, `PIC X(10)`.
    pub proc_date: &'a str,
    /// `MID`, `PIC X(9)`.
    pub merchant_id: &'a str,
    /// `MNAME`, `PIC X(30)`.
    pub merchant_name: &'a str,
    /// `MCITY`, `PIC X(25)`.
    pub merchant_city: &'a str,
    /// `MZIP`, `PIC X(10)`.
    pub merchant_zip: &'a str,
    /// `CONFIRM`, `PIC X(1)`, read by [`confirm`].
    pub confirm: &'a str,
}

/// The data fields as the map holds them once received: each `MOVE`d into its `PIC X(n)` field.
struct ScreenFields {
    type_cd: [char; TYPE_WIDTH],
    cat_cd: [char; CAT_WIDTH],
    source: [char; SOURCE_WIDTH],
    description: [char; DESC_WIDTH],
    amount: [char; AMOUNT_WIDTH],
    orig_date: [char; DATE_WIDTH],
    proc_date: [char; DATE_WIDTH],
    merchant_id: [char; MID_WIDTH],
    merchant_name: [char; NAME_WIDTH],
    merchant_city: [char; CITY_WIDTH],
    merchant_zip: [char; ZIP_WIDTH],
}

impl ScreenFields {
    /// `RECEIVE MAP`: every input fitted to its map field.
    fn receive(input: &TranAddInput<'_>) -> Self {
        Self {
            type_cd: field(input.type_cd),
            cat_cd: field(input.cat_cd),
            source: field(input.source),
            description: field(input.description),
            amount: field(input.amount),
            orig_date: field(input.orig_date),
            proc_date: field(input.proc_date),
            merchant_id: field(input.merchant_id),
            merchant_name: field(input.merchant_name),
            merchant_city: field(input.merchant_city),
            merchant_zip: field(input.merchant_zip),
        }
    }
}

/// The data fields once every edit of `VALIDATE-INPUT-DATA-FIELDS` has passed: what the 200
/// prompt shows (COTRN02C-057) and what [`build_record`] moves into the record (COTRN02C-045).
///
/// Only [`edit_data`] and [`edit_data_with`] build one. Every text field holds printable ASCII
/// as its map field holds it, trailing spaces included; the codes are digits, the amount is its
/// stored value with the re-edited text, and the two dates are typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTran {
    type_cd: PicX<TYPE_WIDTH>,
    cat_cd: PicX<CAT_WIDTH>,
    cat_value: PicU<CAT_DIGITS>,
    source: PicX<SOURCE_WIDTH>,
    description: PicX<DESC_WIDTH>,
    amount: Money<9>,
    amount_text: String,
    orig_ts: CobolTimestamp,
    proc_ts: CobolTimestamp,
    merchant_id: PicX<MID_WIDTH>,
    merchant_id_value: PicU<MID_DIGITS>,
    merchant_name: PicX<NAME_WIDTH>,
    merchant_city: PicX<CITY_WIDTH>,
    merchant_zip: PicX<ZIP_WIDTH>,
}

impl ValidatedTran {
    /// `TTYPCD`, two digits (COTRN02C-031).
    #[must_use]
    pub const fn type_cd(&self) -> &PicX<TYPE_WIDTH> {
        &self.type_cd
    }

    /// `TCATCD`, four digits as entered or copied (COTRN02C-032).
    #[must_use]
    pub const fn cat_cd(&self) -> &PicX<CAT_WIDTH> {
        &self.cat_cd
    }

    /// `TRNSRC` (COTRN02C-022).
    #[must_use]
    pub const fn source(&self) -> &PicX<SOURCE_WIDTH> {
        &self.source
    }

    /// `TDESC` (COTRN02C-023).
    #[must_use]
    pub const fn description(&self) -> &PicX<DESC_WIDTH> {
        &self.description
    }

    /// The amount `TRAN-AMT` receives: the exact value of the edited text (COTRN02C-047).
    #[must_use]
    pub const fn amount(&self) -> Money<9> {
        self.amount
    }

    /// `TRNAMT` as the screen shows it after the edit, through `WS-TRAN-AMT-E PIC
    /// +99999999.99`: a sign, eight digits, `.` and two digits (COTRN02C-036).
    #[must_use]
    pub fn amount_text(&self) -> &str {
        &self.amount_text
    }

    /// `TORIGDT`, typed (COTRN02C-037, COTRN02C-057).
    #[must_use]
    pub fn orig_date(&self) -> NaiveDate {
        self.orig_ts.value().date()
    }

    /// `TPROCDT`, typed (COTRN02C-038, COTRN02C-057).
    #[must_use]
    pub fn proc_date(&self) -> NaiveDate {
        self.proc_ts.value().date()
    }

    /// `MID`, nine digits (COTRN02C-039).
    #[must_use]
    pub const fn merchant_id(&self) -> &PicX<MID_WIDTH> {
        &self.merchant_id
    }

    /// `MNAME` (COTRN02C-028).
    #[must_use]
    pub const fn merchant_name(&self) -> &PicX<NAME_WIDTH> {
        &self.merchant_name
    }

    /// `MCITY` (COTRN02C-029).
    #[must_use]
    pub const fn merchant_city(&self) -> &PicX<CITY_WIDTH> {
        &self.merchant_city
    }

    /// `MZIP` (COTRN02C-030).
    #[must_use]
    pub const fn merchant_zip(&self) -> &PicX<ZIP_WIDTH> {
        &self.merchant_zip
    }
}

/// `VALIDATE-INPUT-DATA-FIELDS` with CSUTLDTC's stand-in verdict on the two dates
/// (COTRN02C-020 to COTRN02C-039).
///
/// `HALTED(H5)`: [`validate`] decides which dates pass, and no test may present that as the
/// COBOL's (COTRN02C-037, COTRN02C-038). Everything else is [`edit_data_with`]'s.
///
/// # Errors
///
/// The first edit that fails, as [`edit_data_with`] lists them.
pub fn edit_data(input: &TranAddInput<'_>) -> Result<ValidatedTran, AddFailure> {
    edit_data_with(input, validate)
}

/// [`edit_data`] with the date-validity verdict supplied by `verdict`, which receives the
/// 10-character date and the picture `YYYY-MM-DD` and returns CSUTLDTC's result
/// (CSUTLDTC-028, CSUTLDTC-029).
///
/// The edits run in program order, and the first that fails is the outcome (COTRN02C-008):
/// the eleven emptiness tests (COTRN02C-020 to COTRN02C-030), the type and category digits
/// (COTRN02C-031, COTRN02C-032), the amount's shape (COTRN02C-033), the two dates' shapes
/// (COTRN02C-034, COTRN02C-035), the amount's re-edit, which never fails (COTRN02C-036), the
/// original date's verdict and then the processing date's (COTRN02C-037, COTRN02C-038), and the
/// merchant ID's digits (COTRN02C-039). A date is accepted when [`caller_accepts`] its result.
/// Nothing else is checked: no code is looked up, and the source, description, merchant name,
/// city and ZIP need only be non-blank (COTRN02C-030, COTRN02C-031, COTRN02C-032).
///
/// # Errors
///
/// Status class [`FailureClass::Input`], on the field the edit names:
///
/// - a blank field: `Type CD can NOT be empty...` and its ten siblings (COTRN02C-020 to
///   COTRN02C-030);
/// - `Type CD must be Numeric...`, `Category CD must be Numeric...` (COTRN02C-031,
///   COTRN02C-032);
/// - `Amount should be in format -99999999.99` (COTRN02C-033);
/// - `Orig Date should be in format YYYY-MM-DD`, `Proc Date should be in format YYYY-MM-DD`
///   (COTRN02C-034, COTRN02C-035);
/// - `Orig Date - Not a valid date...`, `Proc Date - Not a valid date...` (COTRN02C-037,
///   COTRN02C-038);
/// - `Merchant ID must be Numeric...` (COTRN02C-039);
/// - after every COBOL edit has passed, a source, description, merchant name, city or ZIP that
///   holds a character outside printable ASCII, which no record field can store (PIC-002):
///   `Only printable ASCII characters can be stored...`, the replacement's request-validation
///   text and no parity result (X-COUSR01C-3).
pub fn edit_data_with(
    input: &TranAddInput<'_>,
    verdict: impl Fn(&str, DateMask) -> LeResult,
) -> Result<ValidatedTran, AddFailure> {
    let fields = ScreenFields::receive(input);
    check_not_empty(&fields)?;
    let (type_cd, cat_cd, cat_value) = edit_codes(&fields)?;
    check_shapes(&fields)?;
    let (amount, amount_text) = reedit_amount(&fields.amount)?;
    let orig_ts = accepted_date(
        &fields.orig_date,
        ORIG_INVALID,
        AddField::OrigDate,
        &verdict,
    )?;
    let proc_ts = accepted_date(
        &fields.proc_date,
        PROC_INVALID,
        AddField::ProcDate,
        &verdict,
    )?;
    let (merchant_id, merchant_id_value) = numeric_field::<MID_WIDTH, MID_DIGITS>(
        &fields.merchant_id,
        AddField::MerchantId,
        MID_NOT_NUMERIC,
    )?;
    Ok(ValidatedTran {
        type_cd,
        cat_cd,
        cat_value,
        source: storable(&fields.source, AddField::Source)?,
        description: storable(&fields.description, AddField::Description)?,
        amount,
        amount_text,
        orig_ts,
        proc_ts,
        merchant_id,
        merchant_id_value,
        merchant_name: storable(&fields.merchant_name, AddField::MerchantName)?,
        merchant_city: storable(&fields.merchant_city, AddField::MerchantCity)?,
        merchant_zip: storable(&fields.merchant_zip, AddField::MerchantZip)?,
    })
}

/// The first `EVALUATE TRUE` of the data edits: the first blank field, in map order
/// (COTRN02C-020 to COTRN02C-030).
fn check_not_empty(fields: &ScreenFields) -> Result<(), AddFailure> {
    let tests: [(&[char], &'static str, AddField); 11] = [
        (fields.type_cd.as_slice(), TYPE_EMPTY, AddField::TypeCd),
        (fields.cat_cd.as_slice(), CAT_EMPTY, AddField::CatCd),
        (fields.source.as_slice(), SOURCE_EMPTY, AddField::Source),
        (
            fields.description.as_slice(),
            DESC_EMPTY,
            AddField::Description,
        ),
        (fields.amount.as_slice(), AMOUNT_EMPTY, AddField::Amount),
        (fields.orig_date.as_slice(), ORIG_EMPTY, AddField::OrigDate),
        (fields.proc_date.as_slice(), PROC_EMPTY, AddField::ProcDate),
        (
            fields.merchant_id.as_slice(),
            MID_EMPTY,
            AddField::MerchantId,
        ),
        (
            fields.merchant_name.as_slice(),
            NAME_EMPTY,
            AddField::MerchantName,
        ),
        (
            fields.merchant_city.as_slice(),
            CITY_EMPTY,
            AddField::MerchantCity,
        ),
        (
            fields.merchant_zip.as_slice(),
            ZIP_EMPTY,
            AddField::MerchantZip,
        ),
    ];
    match tests.into_iter().find(|(chars, _, _)| is_blank(chars)) {
        Some((_, message, field)) => Err(AddFailure::input(message, field)),
        None => Ok(()),
    }
}

/// The second `EVALUATE TRUE`: the type code is two digits, then the category code four
/// (COTRN02C-031, COTRN02C-032). Returns the two map fields and the category's value, which
/// `TRAN-CAT-CD PIC 9(04)` receives (`0005` is 5).
fn edit_codes(
    fields: &ScreenFields,
) -> Result<(PicX<TYPE_WIDTH>, PicX<CAT_WIDTH>, PicU<CAT_DIGITS>), AddFailure> {
    let type_cd = digit_bytes(&fields.type_cd)
        .map(PicX::from_array)
        .ok_or(AddFailure::input(TYPE_NOT_NUMERIC, AddField::TypeCd))?;
    let (cat_cd, cat_value) =
        numeric_field::<CAT_WIDTH, CAT_DIGITS>(&fields.cat_cd, AddField::CatCd, CAT_NOT_NUMERIC)?;
    Ok((type_cd, cat_cd, cat_value))
}

/// The third, fourth and fifth `EVALUATE TRUE`: the amount's shape, then the original date's,
/// then the processing date's (COTRN02C-033 to COTRN02C-035).
fn check_shapes(fields: &ScreenFields) -> Result<(), AddFailure> {
    if !fits_shape(&fields.amount, &AMOUNT_SHAPE) {
        return Err(AddFailure::input(AMOUNT_FORMAT, AddField::Amount));
    }
    if !fits_shape(&fields.orig_date, &DATE_SHAPE) {
        return Err(AddFailure::input(ORIG_FORMAT, AddField::OrigDate));
    }
    if !fits_shape(&fields.proc_date, &DATE_SHAPE) {
        return Err(AddFailure::input(PROC_FORMAT, AddField::ProcDate));
    }
    Ok(())
}

/// `COMPUTE WS-TRAN-AMT-N = FUNCTION NUMVAL-C(TRNAMTI)`, then `MOVE WS-TRAN-AMT-N TO
/// WS-TRAN-AMT-E` and back into `TRNAMTI` (COTRN02C-036, COTRN02C-047): the stored amount and
/// the text the screen then shows.
///
/// The shape edit has admitted eight integer digits against the receiver's nine, so the store
/// keeps every digit. A negative zero, `-00000000.00`, becomes 0.00, edited `+00000000.00`; the
/// COBOL value is halt candidate X-COTRN02C-H2 and this is no parity result.
fn reedit_amount(amount: &[char; AMOUNT_WIDTH]) -> Result<(Money<9>, String), AddFailure> {
    // The amount has passed COTRN02C-033, so `numval_c` always yields a value; the failure
    // keeps this function total without a panic path.
    let value = numval_c(amount).ok_or(AddFailure::input(AMOUNT_FORMAT, AddField::Amount))?;
    let stored = MoneyExpr::from_decimal(value).store::<9>();
    Ok((
        stored,
        EditedPicture::Plus9x8Dot99.format(stored.as_decimal()),
    ))
}

/// `FUNCTION NUMVAL-C` of an amount that has the shape of COTRN02C-033: a sign, eight digits,
/// `.` and two digits. The value is the ten digits read as hundredths, negative when the sign
/// is `-`, computed exactly with checked integer arithmetic. `None` for any other text, which
/// the shape edit has already refused.
fn numval_c(amount: &[char; AMOUNT_WIDTH]) -> Option<Decimal> {
    if !fits_shape(amount, &AMOUNT_SHAPE) {
        return None;
    }
    let hundredths = amount
        .iter()
        .filter(|c| c.is_ascii_digit())
        .try_fold(0_i64, |value, c| {
            let digit = c.to_digit(10)?;
            value.checked_mul(10)?.checked_add(i64::from(digit))
        })?;
    let signed = if amount.first() == Some(&'-') {
        hundredths.checked_neg()?
    } else {
        hundredths
    };
    Some(Decimal::new(signed, AMOUNT_SCALE))
}

/// CSUTLDTC's check of one date and COTRN02C's test of its result (COTRN02C-037, COTRN02C-038,
/// CSUTLDTC-028, CSUTLDTC-029): the date as the layout-`S` timestamp the record stores, or the
/// date's message on `field`.
fn accepted_date(
    date: &[char; DATE_WIDTH],
    message: &'static str,
    field: AddField,
    verdict: &impl Fn(&str, DateMask) -> LeResult,
) -> Result<CobolTimestamp, AddFailure> {
    let text: String = date.iter().collect();
    // HALTED(H5): which dates CEEDAYS accepts lives outside the repository; the verdict decides
    // and is no parity result (COTRN02C-037, COTRN02C-038).
    let result = verdict(&text, DateMask::YyyyMmDdDash);
    if !caller_accepts(&result) {
        return Err(AddFailure::input(message, field));
    }
    // Only a substituted message-2513 verdict accepts text that spells no calendar date. The
    // COBOL would store that text, but a typed timestamp cannot hold it (Typed Dates and
    // Timestamps), so it is refused with the date's own message: the replacement's choice for
    // halt candidate X-CSUTLDTC-H2, no parity result. A four-digit year always fits the
    // timestamp, so its construction never fails for a calendar date.
    result
        .date()
        .and_then(|day| CobolTimestamp::new(day.and_time(NaiveTime::MIN), TsForm::ScreenDate).ok())
        .ok_or(AddFailure::input(message, field))
}

/// A map field of `N` characters that must be `N` digits, and the value of `PIC 9(D)` it moves
/// into; `message` on `field` otherwise (COTRN02C-032, COTRN02C-039).
fn numeric_field<const N: usize, const D: u32>(
    chars: &[char; N],
    field: AddField,
    message: &'static str,
) -> Result<(PicX<N>, PicU<D>), AddFailure> {
    let failure = AddFailure::input(message, field);
    let digits = digit_bytes(chars).ok_or(failure)?;
    let value = digits_value(chars)
        .and_then(|value| PicU::new(value).ok())
        .ok_or(failure)?;
    Ok((PicX::from_array(digits), value))
}

/// A free-text map field as its record field stores it: printable ASCII only (PIC-001,
/// PIC-002), refused on `field` otherwise (X-COUSR01C-3).
fn storable<const N: usize>(chars: &[char; N], field: AddField) -> Result<PicX<N>, AddFailure> {
    printable_bytes(chars)
        .map(PicX::from_array)
        .ok_or(AddFailure::input(NOT_STORABLE, field))
}

/// One position of a fixed-shape map field.
#[derive(Clone, Copy)]
enum Position {
    /// `0` to `9`, the COBOL `NUMERIC` class of one character.
    Digit,
    /// `-` or `+`.
    Sign,
    /// Exactly this character.
    Exactly(char),
}

impl Position {
    /// Whether `c` may stand at this position.
    fn admits(self, c: char) -> bool {
        match self {
            Self::Digit => c.is_ascii_digit(),
            Self::Sign => c == '-' || c == '+',
            Self::Exactly(expected) => c == expected,
        }
    }
}

/// `TRNAMTI`: position 1 `-` or `+`, positions 2-9 digits, position 10 `.`, positions 11-12
/// digits (COTRN02C-033).
const AMOUNT_SHAPE: [Position; AMOUNT_WIDTH] = [
    Position::Sign,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Exactly('.'),
    Position::Digit,
    Position::Digit,
];

/// `TORIGDTI` and `TPROCDTI`: positions 1-4 digits, 5 `-`, 6-7 digits, 8 `-`, 9-10 digits
/// (COTRN02C-034, COTRN02C-035).
const DATE_SHAPE: [Position; DATE_WIDTH] = [
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Digit,
    Position::Exactly('-'),
    Position::Digit,
    Position::Digit,
    Position::Exactly('-'),
    Position::Digit,
    Position::Digit,
];

/// Whether every character of `chars` is admitted by the position of `shape` it stands at; the
/// COBOL tests each position, so the order of the tests does not change the outcome.
fn fits_shape(chars: &[char], shape: &[Position]) -> bool {
    chars.len() == shape.len()
        && chars
            .iter()
            .zip(shape)
            .all(|(&c, position)| position.admits(c))
}

/// A COBOL `MOVE` of `text` into a `PIC X(N)` field: its first `N` characters, padded on the
/// right with spaces.
fn field<const N: usize>(text: &str) -> [char; N] {
    let mut chars = [' '; N];
    // TRUNCATION: characters beyond `N` are dropped, the COBOL MOVE rule for a shorter
    // alphanumeric receiver; the request DTO already refuses a value longer than its map field.
    for (slot, c) in chars.iter_mut().zip(text.chars()) {
        *slot = c;
    }
    chars
}

/// `= SPACES OR LOW-VALUES`: every character a space, or every one `\0`.
fn is_blank(chars: &[char]) -> bool {
    chars.iter().all(|&c| c == ' ') || chars.iter().all(|&c| c == '\0')
}

/// The field's bytes when every character is printable ASCII (`0x20`-`0x7E`).
fn printable_bytes<const N: usize>(chars: &[char; N]) -> Option<[u8; N]> {
    let mut bytes = [b' '; N];
    for (slot, &c) in bytes.iter_mut().zip(chars) {
        *slot = u8::try_from(c)
            .ok()
            .filter(|byte| (b' '..=b'~').contains(byte))?;
    }
    Some(bytes)
}

/// The field's bytes when it passes the COBOL `NUMERIC` class test: every character a digit.
fn digit_bytes<const N: usize>(chars: &[char; N]) -> Option<[u8; N]> {
    printable_bytes(chars).filter(|bytes| bytes.iter().all(u8::is_ascii_digit))
}

/// The value of a field of digits, computed with checked arithmetic; `None` when a character
/// is not a digit or the value exceeds `u64`.
fn digits_value(chars: &[char]) -> Option<u64> {
    chars.iter().try_fold(0_u64, |value, c| {
        let digit = c.to_digit(10)?;
        value.checked_mul(10)?.checked_add(u64::from(digit))
    })
}

/// What `PROCESS-ENTER-KEY`'s `EVALUATE CONFIRMI` decides once every edit has passed
/// (COTRN02C-040 to COTRN02C-042).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confirmation {
    /// `Y` or `y`: add the transaction (COTRN02C-040, COTRN02C-043 to COTRN02C-051).
    Yes,
    /// `N`, `n`, a space or a low-value: 200 with `Confirm to add this transaction...` and
    /// nothing written; `N` neither clears nor cancels ([`AddOutcome::prompt`], COTRN02C-041).
    NotConfirmed,
    /// Any other character: [`AddFailure::invalid_confirmation`] (COTRN02C-042).
    Invalid,
}

/// The `EVALUATE CONFIRMI OF COTRN2AI` of `PROCESS-ENTER-KEY` (COTRN02C-040 to COTRN02C-042).
/// `raw` is the raw text of `CONFIRM`, read as its one-character map field, so an empty value
/// is blank.
///
/// The service calls it only after [`edit_data`] has passed: the confirmation is tested after
/// every edit, so a `Y` with a failing field gets that field's message (COTRN02C-040).
#[must_use]
pub fn confirm(raw: &str) -> Confirmation {
    let [answer] = field::<CONFIRM_WIDTH>(raw);
    match answer {
        'Y' | 'y' => Confirmation::Yes,
        'N' | 'n' | ' ' | '\0' => Confirmation::NotConfirmed,
        _ => Confirmation::Invalid,
    }
}

/// The 200 prompt's content: what the COBOL screen shows once every edit has passed and the
/// confirmation is blank, `N` or `n` (COTRN02C-041, COTRN02C-056, COTRN02C-057).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    key: FilledKey,
    fields: ValidatedTran,
    copied: Option<CopiedFields>,
}

impl ConfirmPrompt {
    /// The account and the card, entered or filled in; the card leaves redacted (`E32`).
    #[must_use]
    pub const fn key(&self) -> &FilledKey {
        &self.key
    }

    /// The data fields as the screen shows them: the amount re-edited and the dates typed.
    #[must_use]
    pub const fn fields(&self) -> &ValidatedTran {
        &self.fields
    }

    /// On PF5, the fields copied from the highest-ID transaction (COTRN02C-054, COTRN02C-056).
    #[must_use]
    pub const fn copied(&self) -> Option<&CopiedFields> {
        self.copied.as_ref()
    }
}

/// A 200 response of endpoint 13 (COTRN02C-057).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddOutcome {
    /// The confirmation prompt: nothing was written (COTRN02C-041, COTRN02C-056).
    Prompt(Box<ConfirmPrompt>),
    /// The transaction with this ID was inserted; every field of the screen is empty
    /// (COTRN02C-049).
    Added(PicX<16>),
}

impl AddOutcome {
    /// The prompt for a request whose edits passed with a blank, `N` or `n` confirmation;
    /// `copied` holds the PF5 copy (COTRN02C-041, COTRN02C-056).
    #[must_use]
    pub fn prompt(key: FilledKey, fields: ValidatedTran, copied: Option<CopiedFields>) -> Self {
        Self::Prompt(Box::new(ConfirmPrompt {
            key,
            fields,
            copied,
        }))
    }

    /// The answer to a committed insert of the transaction `new_id` (COTRN02C-049).
    #[must_use]
    pub const fn added(new_id: PicX<16>) -> Self {
        Self::Added(new_id)
    }

    /// The message: `Confirm to add this transaction...` for the prompt, [`success_message`]
    /// after the add (COTRN02C-041, COTRN02C-049).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Prompt(_) => CONFIRM_PROMPT.to_owned(),
            Self::Added(new_id) => success_message(new_id),
        }
    }

    /// The field the cursor goes to: the confirmation for the prompt, the account after the add,
    /// which `INITIALIZE-ALL-FIELDS` empties (COTRN02C-041, COTRN02C-049, COTRN02C-060).
    #[must_use]
    pub const fn focus(&self) -> AddField {
        match self {
            Self::Prompt(_) => AddField::Confirm,
            Self::Added(_) => AddField::AcctId,
        }
    }
}

/// `ADD-TRANSACTION` after its browse: the record `WRITE-TRANSACT-FILE` inserts (COTRN02C-045
/// to COTRN02C-048).
///
/// `new_id` is `TransactionRepository::next_id`, the highest ID plus one, 16 digits
/// (COTRN02C-043). `filler` is the FILLER of the transaction that browse read, because
/// `INITIALIZE TRAN-RECORD` skips FILLER and `READPREV` filled the record (COTRN02C-045,
/// PIC-012); on an empty table the COBOL bytes are halt candidate X-COTRN02C-H1 and the service
/// passes spaces, no parity result.
///
/// Each field is moved as `ADD-TRANSACTION` moves it: the type's two digits, the category's
/// value, the source, the description padded to 100, the stored amount, the merchant ID's value,
/// the name and the city padded to 50, the ZIP, the key's full card number, and each date as a
/// layout-`S` timestamp, the date at midnight, which encodes as the 10 characters followed by
/// 16 spaces (COTRN02C-046, PIC-020, `E33`). No clock value is written, and no account field
/// exists to receive the account (COTRN02C-048).
#[must_use]
pub fn build_record(
    new_id: PicX<16>,
    key: &FilledKey,
    fields: &ValidatedTran,
    filler: PicX<20>,
) -> TransactionRecord {
    TransactionRecord {
        id: new_id,
        type_cd: fields.type_cd.clone(),
        cat_cd: fields.cat_value,
        source: fields.source.clone(),
        desc: PicX::moved_from(fields.description.as_bytes()),
        amt: fields.amount,
        merchant_id: fields.merchant_id_value,
        merchant_name: PicX::moved_from(fields.merchant_name.as_bytes()),
        merchant_city: PicX::moved_from(fields.merchant_city.as_bytes()),
        merchant_zip: fields.merchant_zip.clone(),
        card_num: key.card_num.clone(),
        orig_ts: PicTimestamp::new(Some(fields.orig_ts)),
        proc_ts: PicTimestamp::new(Some(fields.proc_ts)),
        filler,
    }
}

/// The success message of `WRITE-TRANSACT-FILE` (COTRN02C-049): `Transaction added
/// successfully. `, ` Your Tran ID is `, the ID up to its first space (`DELIMITED BY SPACE`) and
/// `.`. A 16-digit ID gives 66 characters, with two spaces after `successfully.`.
#[must_use]
pub fn success_message(new_id: &PicX<16>) -> String {
    let id: String = new_id
        .as_bytes()
        .iter()
        .take_while(|&&byte| byte != b' ')
        .copied()
        .map(char::from)
        .collect();
    format!("{ADDED_HEAD}{ADDED_ID_INTRO}{id}{ADDED_TAIL}")
}

/// The data fields PF5 copies from the highest-ID transaction, as the screen receives them
/// (COTRN02C-054, COTRN02C-055).
///
/// Only [`copy_last`] builds one. The text of each field is what the COBOL `MOVE` puts in its
/// map field, trailing spaces included; [`CopiedFields::as_input`] feeds that text to the ENTER
/// processing that follows (COTRN02C-056). The two dates are also given typed, `None` for a
/// blank timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedFields {
    type_cd: String,
    cat_cd: String,
    source: String,
    description: String,
    amount: String,
    orig_date: String,
    proc_date: String,
    orig_date_value: Option<NaiveDate>,
    proc_date_value: Option<NaiveDate>,
    merchant_id: String,
    merchant_name: String,
    merchant_city: String,
    merchant_zip: String,
}

impl CopiedFields {
    /// The copied fields with the request's confirmation, as the input of the ENTER processing
    /// `COPY-LAST-TRAN-DATA` performs at once (COTRN02C-056).
    #[must_use]
    pub fn as_input<'a>(&'a self, confirm: &'a str) -> TranAddInput<'a> {
        TranAddInput {
            type_cd: &self.type_cd,
            cat_cd: &self.cat_cd,
            source: &self.source,
            description: &self.description,
            amount: &self.amount,
            orig_date: &self.orig_date,
            proc_date: &self.proc_date,
            merchant_id: &self.merchant_id,
            merchant_name: &self.merchant_name,
            merchant_city: &self.merchant_city,
            merchant_zip: &self.merchant_zip,
            confirm,
        }
    }

    /// `TTYPCD` from `TRAN-TYPE-CD`.
    #[must_use]
    pub fn type_cd(&self) -> &str {
        &self.type_cd
    }

    /// `TCATCD` from `TRAN-CAT-CD`, four digits.
    #[must_use]
    pub fn cat_cd(&self) -> &str {
        &self.cat_cd
    }

    /// `TRNSRC` from `TRAN-SOURCE`.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// `TDESC` from the first 60 characters of `TRAN-DESC`.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// `TRNAMT`: `TRAN-AMT` through `WS-TRAN-AMT-E PIC +99999999.99`, which drops a ninth
    /// integer digit (COTRN02C-055, X-COTRN02C-4).
    #[must_use]
    pub fn amount(&self) -> &str {
        &self.amount
    }

    /// `TORIGDT` as text: the first 10 characters of `TRAN-ORIG-TS`, `YYYY-MM-DD` for every
    /// layout and 10 spaces for a blank timestamp.
    #[must_use]
    pub fn orig_date_text(&self) -> &str {
        &self.orig_date
    }

    /// `TPROCDT` as text: the first 10 characters of `TRAN-PROC-TS`.
    #[must_use]
    pub fn proc_date_text(&self) -> &str {
        &self.proc_date
    }

    /// `TORIGDT`, typed: the date of `TRAN-ORIG-TS`, `None` when it is blank.
    #[must_use]
    pub const fn orig_date(&self) -> Option<NaiveDate> {
        self.orig_date_value
    }

    /// `TPROCDT`, typed: the date of `TRAN-PROC-TS`, `None` when it is blank.
    #[must_use]
    pub const fn proc_date(&self) -> Option<NaiveDate> {
        self.proc_date_value
    }

    /// `MID` from `TRAN-MERCHANT-ID`, nine digits.
    #[must_use]
    pub fn merchant_id(&self) -> &str {
        &self.merchant_id
    }

    /// `MNAME` from the first 30 characters of `TRAN-MERCHANT-NAME`.
    #[must_use]
    pub fn merchant_name(&self) -> &str {
        &self.merchant_name
    }

    /// `MCITY` from the first 25 characters of `TRAN-MERCHANT-CITY`.
    #[must_use]
    pub fn merchant_city(&self) -> &str {
        &self.merchant_city
    }

    /// `MZIP` from `TRAN-MERCHANT-ZIP`.
    #[must_use]
    pub fn merchant_zip(&self) -> &str {
        &self.merchant_zip
    }
}

/// `COPY-LAST-TRAN-DATA` after its browse: the data fields replaced from `last`, the transaction
/// with the highest ID in the whole file, whatever its card (COTRN02C-053 to COTRN02C-055,
/// X-COTRN02C-3).
///
/// Each `MOVE` into a shorter map field keeps the first characters: `TDESC` 60 of 100, `MNAME`
/// 30 and `MCITY` 25 of 50. The category and merchant ID become their digits with leading zeros,
/// the amount the 12-character `+99999999.99` edit of `TRAN-AMT`, and each date the first 10
/// characters of its timestamp's text. The copied record's ID and card number are not copied:
/// the key stays the one the key edits resolved (COTRN02C-052, COTRN02C-054).
#[must_use]
pub fn copy_last(last: &TransactionRecord) -> CopiedFields {
    CopiedFields {
        type_cd: prefix_text(last.type_cd.as_bytes(), TYPE_WIDTH),
        cat_cd: digits_text(last.cat_cd),
        source: prefix_text(last.source.as_bytes(), SOURCE_WIDTH),
        description: prefix_text(last.desc.as_bytes(), DESC_WIDTH),
        amount: EditedPicture::Plus9x8Dot99.format(last.amt.as_decimal()),
        orig_date: date_text(&last.orig_ts),
        proc_date: date_text(&last.proc_ts),
        orig_date_value: last.orig_ts.value().map(|stamp| stamp.value().date()),
        proc_date_value: last.proc_ts.value().map(|stamp| stamp.value().date()),
        merchant_id: digits_text(last.merchant_id),
        merchant_name: prefix_text(last.merchant_name.as_bytes(), NAME_WIDTH),
        merchant_city: prefix_text(last.merchant_city.as_bytes(), CITY_WIDTH),
        merchant_zip: prefix_text(last.merchant_zip.as_bytes(), ZIP_WIDTH),
    }
}

/// The first `width` bytes of a record field as text, one character per byte, as an
/// alphanumeric `MOVE` into a map field of that width keeps them.
fn prefix_text(bytes: &[u8], width: usize) -> String {
    bytes.iter().take(width).copied().map(char::from).collect()
}

/// A `PIC 9(D)` value moved into an alphanumeric field: its `D` digits with leading zeros.
fn digits_text<const D: u32>(value: PicU<D>) -> String {
    let mut digits = Vec::new();
    value.encode_into(&mut digits);
    digits.into_iter().map(char::from).collect()
}

/// A `PIC X(26)` timestamp moved into a `PIC X(10)` date field: the first 10 characters of its
/// text, the date `YYYY-MM-DD` in every layout and 10 spaces for a blank timestamp.
fn date_text(stamp: &PicTimestamp) -> String {
    let mut text = Vec::with_capacity(TIMESTAMP_WIDTH);
    stamp.encode_into(&mut text);
    prefix_text(&text, DATE_WIDTH)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{CobolTimestamp, FixedRecord, PicTimestamp, PicU, PicX, TsForm};

    use super::super::csutldtc::{DateMask, LeFeedback, LeResult};
    use super::{
        AddFailure, AddField, AddOutcome, Aid, Confirmation, FailureClass, FilledKey, KeyRequest,
        TranAddInput, ValidatedTran, XrefLookup, build_record, confirm, copy_last, edit_data,
        edit_data_with, edit_keys, fill_key, numval_c, success_message,
    };
    use crate::money::{Decimal, Money};
    use crate::records::card_xref::CardXrefRecord;
    use crate::records::transaction::TransactionRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field and HTTP status.
    type Shown = (&'static str, AddField, u16);

    // The audited literals, written out apart from the module's own constants.
    const KEY_MISSING: &str = "Account or Card Number must be entered...";
    const ACCT_NOT_NUMERIC: &str = "Account ID must be Numeric...";
    const CARD_NOT_NUMERIC: &str = "Card Number must be Numeric...";
    const AMOUNT_FORMAT: &str = "Amount should be in format -99999999.99";
    const ORIG_FORMAT: &str = "Orig Date should be in format YYYY-MM-DD";
    const PROC_FORMAT: &str = "Proc Date should be in format YYYY-MM-DD";
    const ORIG_INVALID: &str = "Orig Date - Not a valid date...";
    const PROC_INVALID: &str = "Proc Date - Not a valid date...";
    const MID_NOT_NUMERIC: &str = "Merchant ID must be Numeric...";
    const NOT_STORABLE: &str = "Only printable ASCII characters can be stored...";

    /// The eleven data fields in map order with their emptiness messages (COTRN02C-020 to
    /// COTRN02C-030).
    const EMPTY_MESSAGES: [(AddField, &str); 11] = [
        (AddField::TypeCd, "Type CD can NOT be empty..."),
        (AddField::CatCd, "Category CD can NOT be empty..."),
        (AddField::Source, "Source can NOT be empty..."),
        (AddField::Description, "Description can NOT be empty..."),
        (AddField::Amount, "Amount can NOT be empty..."),
        (AddField::OrigDate, "Orig Date can NOT be empty..."),
        (AddField::ProcDate, "Proc Date can NOT be empty..."),
        (AddField::MerchantId, "Merchant ID can NOT be empty..."),
        (AddField::MerchantName, "Merchant Name can NOT be empty..."),
        (AddField::MerchantCity, "Merchant City can NOT be empty..."),
        (AddField::MerchantZip, "Merchant Zip can NOT be empty..."),
    ];

    /// A synthetic card number, in no fixture.
    const CARD: &str = "1234567890123456";

    /// Eleven data fields that pass every edit, and a blank confirmation.
    fn valid() -> TranAddInput<'static> {
        TranAddInput {
            type_cd: "01",
            cat_cd: "0005",
            source: "POS TERM",
            description: "Grocery purchase",
            amount: "+00000012.34",
            orig_date: "2022-07-18",
            proc_date: "2022-07-19",
            merchant_id: "800000001",
            merchant_name: "Corner Store",
            merchant_city: "Springfield",
            merchant_zip: "12345",
            confirm: "",
        }
    }

    /// Replaces the input field `field` names; the key fields are not part of the input, so
    /// they set the confirmation, which no data edit reads.
    fn set<'a>(input: &mut TranAddInput<'a>, field: AddField, value: &'a str) {
        let slot = match field {
            AddField::TypeCd => &mut input.type_cd,
            AddField::CatCd => &mut input.cat_cd,
            AddField::Source => &mut input.source,
            AddField::Description => &mut input.description,
            AddField::Amount => &mut input.amount,
            AddField::OrigDate => &mut input.orig_date,
            AddField::ProcDate => &mut input.proc_date,
            AddField::MerchantId => &mut input.merchant_id,
            AddField::MerchantName => &mut input.merchant_name,
            AddField::MerchantCity => &mut input.merchant_city,
            AddField::MerchantZip => &mut input.merchant_zip,
            AddField::AcctId | AddField::CardNum | AddField::Confirm => &mut input.confirm,
        };
        *slot = value;
    }

    /// [`valid`] with one data field replaced.
    fn with(field: AddField, value: &str) -> TranAddInput<'_> {
        let mut input = valid();
        set(&mut input, field, value);
        input
    }

    /// A verdict that accepts every date, substituted for the H5 call.
    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    /// A verdict that rejects every date with a non-2513 message, substituted for the H5 call.
    fn reject_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::BadDateValue)
    }

    /// A verdict that answers every date with message 2513, substituted for the H5 call.
    fn unsupported_range(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::UnsupportedRange)
    }

    fn shown(failure: &AddFailure) -> Shown {
        (
            failure.message(),
            failure.field(),
            failure.class().http_status(),
        )
    }

    fn failed<T>(result: &Result<T, AddFailure>) -> Option<Shown> {
        result.as_ref().err().map(shown)
    }

    fn passed(input: &TranAddInput<'_>) -> Result<ValidatedTran, String> {
        edit_data_with(input, accept_all).map_err(|failure| failure.to_string())
    }

    fn pic<const N: usize>(text: &str) -> Result<PicX<N>, String> {
        PicX::from_text(text).map_err(|kind| format!("{kind:?}"))
    }

    fn number<const N: u32>(value: u64) -> Result<PicU<N>, String> {
        PicU::new(value).map_err(|kind| format!("{kind:?}"))
    }

    fn day(year: i32, month: u32, date: u32) -> Result<NaiveDate, String> {
        NaiveDate::from_ymd_opt(year, month, date).ok_or_else(|| format!("{year}-{month}-{date}"))
    }

    fn stamp(at: NaiveDateTime, form: TsForm) -> Result<PicTimestamp, String> {
        CobolTimestamp::new(at, form)
            .map(|value| PicTimestamp::new(Some(value)))
            .map_err(|kind| format!("{kind:?}"))
    }

    fn card() -> Result<CardNumber, String> {
        CardNumber::parse(CARD).map_err(|error| error.to_string())
    }

    fn xref(acct_id: u64) -> Result<CardXrefRecord, String> {
        Ok(CardXrefRecord {
            card_num: card()?,
            cust_id: number(7)?,
            acct_id: number(acct_id)?,
            filler: PicX::spaces(),
        })
    }

    fn filled_key() -> Result<FilledKey, String> {
        let record = xref(50)?;
        fill_key(KeyRequest::Account(number(50)?), XrefLookup::Found(&record))
            .map_err(|failure| failure.to_string())
    }

    /// The highest-ID transaction a PF5 copies: a nine-digit negative amount, an `I`-layout
    /// original timestamp and a blank processing timestamp.
    fn last_transaction() -> Result<TransactionRecord, String> {
        let at = day(2022, 6, 10)?
            .and_hms_opt(19, 27, 53)
            .ok_or("no such time")?;
        Ok(TransactionRecord {
            id: pic("0000000000000100")?,
            type_cd: pic("03")?,
            cat_cd: number(2)?,
            source: pic("OPERATOR")?,
            desc: pic(&"D".repeat(100))?,
            amt: Money::new(Decimal::new(-12_345_678_912, 2)).map_err(|e| e.to_string())?,
            merchant_id: number(42)?,
            merchant_name: pic(&format!("{}{}", "N".repeat(30), "x".repeat(20)))?,
            merchant_city: pic(&format!("{}{}", "C".repeat(25), "y".repeat(25)))?,
            merchant_zip: pic("98765")?,
            card_num: CardNumber::parse("6543210987654321").map_err(|e| e.to_string())?,
            orig_ts: stamp(at, TsForm::Iso)?,
            proc_ts: PicTimestamp::BLANK,
            filler: pic("FILLER-OF-LAST-READ!")?,
        })
    }

    #[test]
    fn statuses_messages_and_attention_keys() {
        assert_eq!(FailureClass::Input.http_status(), 400);
        assert_eq!(FailureClass::NotFound.http_status(), 404);
        assert_eq!(FailureClass::Store.http_status(), 500);
        let failure = AddFailure::duplicate_id();
        assert_eq!(failure.to_string(), failure.message());
        let as_error: &dyn std::error::Error = &failure;
        assert_eq!(as_error.to_string(), "Tran ID already exist...");
        assert_ne!(Aid::Enter, Aid::Pf5);
    }

    // audit: COTRN02C-009, COTRN02C-010, COTRN02C-015
    #[test]
    fn key_edits_reject_in_program_order() {
        let eleven_nuls = "\0".repeat(11);
        let cases: [(Option<&str>, Option<&str>, Shown); 11] = [
            (None, None, (KEY_MISSING, AddField::AcctId, 400)),
            (Some(""), Some("   "), (KEY_MISSING, AddField::AcctId, 400)),
            (
                Some(&eleven_nuls),
                None,
                (KEY_MISSING, AddField::AcctId, 400),
            ),
            (Some("123"), None, (ACCT_NOT_NUMERIC, AddField::AcctId, 400)),
            (
                Some(" 0000000005"),
                None,
                (ACCT_NOT_NUMERIC, AddField::AcctId, 400),
            ),
            (
                Some("-0000000005"),
                None,
                (ACCT_NOT_NUMERIC, AddField::AcctId, 400),
            ),
            (
                Some("0000000005X"),
                Some(CARD),
                (ACCT_NOT_NUMERIC, AddField::AcctId, 400),
            ),
            (Some("\0"), None, (ACCT_NOT_NUMERIC, AddField::AcctId, 400)),
            (
                None,
                Some("123456789012345"),
                (CARD_NOT_NUMERIC, AddField::CardNum, 400),
            ),
            (
                Some(" "),
                Some("************3456"),
                (CARD_NOT_NUMERIC, AddField::CardNum, 400),
            ),
            (
                None,
                Some("1234567890１23456"),
                (CARD_NOT_NUMERIC, AddField::CardNum, 400),
            ),
        ];
        for (acct, card_num, expected) in cases {
            assert_eq!(
                failed(&edit_keys(acct, card_num)),
                Some(expected),
                "{acct:?}"
            );
        }
    }

    // audit: COTRN02C-011, COTRN02C-016
    #[test]
    fn a_numeric_account_decides_before_the_card() -> TestResult {
        let account = KeyRequest::Account(number(50)?);
        for card_num in [None, Some("ABC"), Some(CARD), Some("************3456")] {
            assert_eq!(
                edit_keys(Some("00000000050"), card_num),
                Ok(account.clone())
            );
        }
        // Characters beyond the 11 of the map field are dropped by the MOVE.
        assert_eq!(edit_keys(Some("00000000050999"), None), Ok(account));
        // HALTED(X-ARITH-5) above 2^53: the 16 digits key the read as entered.
        assert_eq!(edit_keys(None, Some(CARD)), Ok(KeyRequest::Card(card()?)));
        assert_eq!(
            edit_keys(Some("           "), Some(CARD)),
            Ok(KeyRequest::Card(card()?))
        );
        Ok(())
    }

    // audit: COTRN02C-012, COTRN02C-013, COTRN02C-014, COTRN02C-017, COTRN02C-018, COTRN02C-019
    #[test]
    fn the_cross_reference_fills_the_other_half_of_the_key() -> TestResult {
        let record = xref(77)?;
        let by_account = fill_key(KeyRequest::Account(number(50)?), XrefLookup::Found(&record))
            .map_err(|failure| failure.to_string())?;
        assert_eq!(by_account.acct_id(), number(50)?);
        assert_eq!(by_account.card_num(), &card()?);
        assert_eq!(format!("{by_account:?}").matches(CARD).count(), 0);
        let by_card = fill_key(KeyRequest::Card(card()?), XrefLookup::Found(&record))
            .map_err(|failure| failure.to_string())?;
        assert_eq!(by_card.acct_id(), number(77)?);
        assert_eq!(by_card.card_num().redacted(), "************3456");
        let account = || -> Result<KeyRequest, String> { Ok(KeyRequest::Account(number(50)?)) };
        let cases: [(KeyRequest, XrefLookup<'_>, Shown); 4] = [
            (
                account()?,
                XrefLookup::NotFound,
                ("Account ID NOT found...", AddField::AcctId, 404),
            ),
            (
                account()?,
                XrefLookup::Error,
                (
                    "Unable to lookup Acct in XREF AIX file...",
                    AddField::AcctId,
                    500,
                ),
            ),
            (
                KeyRequest::Card(card()?),
                XrefLookup::NotFound,
                ("Card Number NOT found...", AddField::CardNum, 404),
            ),
            (
                KeyRequest::Card(card()?),
                XrefLookup::Error,
                (
                    "Unable to lookup Card # in XREF file...",
                    AddField::CardNum,
                    500,
                ),
            ),
        ];
        for (request, lookup, expected) in cases {
            assert_eq!(failed(&fill_key(request, lookup)), Some(expected));
        }
        Ok(())
    }

    // audit: COTRN02C-020, COTRN02C-021, COTRN02C-022, COTRN02C-023, COTRN02C-024, COTRN02C-025
    // audit: COTRN02C-026, COTRN02C-027, COTRN02C-028, COTRN02C-029, COTRN02C-030
    #[test]
    fn the_first_blank_data_field_in_map_order_is_reported() {
        for (position, (field, message)) in EMPTY_MESSAGES.iter().enumerate() {
            // Every field from this one on is blank, so the message shows it is tested first.
            let mut input = valid();
            for (later, _) in EMPTY_MESSAGES.iter().skip(position) {
                set(&mut input, *later, "");
            }
            assert_eq!(
                failed(&edit_data_with(&input, accept_all)),
                Some((*message, *field, 400))
            );
            // A field of low-values is blank too; a single space is padded to blank.
            let low_values = "\0".repeat(60);
            for blank in [low_values.as_str(), " "] {
                assert_eq!(
                    failed(&edit_data_with(&with(*field, blank), accept_all)),
                    Some((*message, *field, 400))
                );
            }
        }
    }

    // audit: COTRN02C-031, COTRN02C-032
    #[test]
    fn the_codes_must_be_all_digits_type_first() {
        let cases: [(AddField, &str, Shown); 6] = [
            (
                AddField::TypeCd,
                "1",
                ("Type CD must be Numeric...", AddField::TypeCd, 400),
            ),
            (
                AddField::TypeCd,
                "A1",
                ("Type CD must be Numeric...", AddField::TypeCd, 400),
            ),
            (
                AddField::TypeCd,
                "+1",
                ("Type CD must be Numeric...", AddField::TypeCd, 400),
            ),
            (
                AddField::CatCd,
                "5",
                ("Category CD must be Numeric...", AddField::CatCd, 400),
            ),
            (
                AddField::CatCd,
                " 005",
                ("Category CD must be Numeric...", AddField::CatCd, 400),
            ),
            (
                AddField::CatCd,
                "00.5",
                ("Category CD must be Numeric...", AddField::CatCd, 400),
            ),
        ];
        for (field, value, expected) in cases {
            assert_eq!(
                failed(&edit_data_with(&with(field, value), accept_all)),
                Some(expected),
                "{value:?}"
            );
        }
        let both = TranAddInput {
            type_cd: "X",
            cat_cd: "Y",
            ..valid()
        };
        assert_eq!(
            failed(&edit_data_with(&both, accept_all)),
            Some(("Type CD must be Numeric...", AddField::TypeCd, 400))
        );
    }

    // audit: COTRN02C-033
    #[test]
    fn the_amount_must_be_a_sign_eight_digits_a_point_and_two_digits() -> TestResult {
        let refused = [
            "12.34",
            "+0000001234",
            "*00000012.34",
            "+0000001A.34",
            "+00000012,34",
            "+00000012.3",
            "+00000012.3X",
            " +0000012.34",
        ];
        for amount in refused {
            assert_eq!(
                failed(&edit_data_with(&with(AddField::Amount, amount), accept_all)),
                Some((AMOUNT_FORMAT, AddField::Amount, 400)),
                "{amount:?}"
            );
        }
        // The amount's shape is tested before either date's.
        let both = TranAddInput {
            amount: "12.34",
            orig_date: "18/07/2022",
            ..valid()
        };
        assert_eq!(
            failed(&edit_data_with(&both, accept_all)),
            Some((AMOUNT_FORMAT, AddField::Amount, 400))
        );
        // The last is cut to its 12-character map field by the MOVE.
        for accepted in [
            "+00000012.34",
            "-00000012.34",
            "+99999999.99",
            "-00000000.01",
            "+00000012.345",
        ] {
            passed(&with(AddField::Amount, accepted))?;
        }
        Ok(())
    }

    // audit: COTRN02C-034, COTRN02C-035
    #[test]
    fn each_date_must_have_the_shape_yyyy_mm_dd() -> TestResult {
        let refused = [
            "2022/07/18",
            "22-07-18",
            "2022-7-18",
            "2022-07-1",
            "YYYY-MM-DD",
        ];
        for date in refused {
            assert_eq!(
                failed(&edit_data_with(&with(AddField::OrigDate, date), accept_all)),
                Some((ORIG_FORMAT, AddField::OrigDate, 400)),
                "{date:?}"
            );
            assert_eq!(
                failed(&edit_data_with(&with(AddField::ProcDate, date), accept_all)),
                Some((PROC_FORMAT, AddField::ProcDate, 400)),
                "{date:?}"
            );
        }
        // Characters beyond the 10 of the map field are dropped by the MOVE.
        passed(&with(AddField::OrigDate, "2022-07-18X"))?;
        // The shape alone is checked here: a month 13 passes it, and the verdict decides it.
        assert_eq!(
            failed(&edit_data_with(
                &with(AddField::OrigDate, "2022-13-45"),
                reject_all
            )),
            Some((ORIG_INVALID, AddField::OrigDate, 400))
        );
        Ok(())
    }

    // audit: COTRN02C-036, COTRN02C-047
    #[test]
    fn the_amount_is_stored_exactly_and_re_edited() -> TestResult {
        let cases = [
            ("+00000012.34", 1_234, "+00000012.34"),
            ("-99999999.99", -9_999_999_999, "-99999999.99"),
            ("+00000000.00", 0, "+00000000.00"),
            ("-00000000.01", -1, "-00000000.01"),
            // HALTED(X-COTRN02C-H2): a negative zero is stored as 0.00 and re-edited with `+`;
            // BLOCKED, this is the replacement's choice and no parity value.
            ("-00000000.00", 0, "+00000000.00"),
        ];
        for (text, hundredths, edited) in cases {
            let validated = passed(&with(AddField::Amount, text))?;
            let expected =
                Money::<9>::new(Decimal::new(hundredths, 2)).map_err(|e| e.to_string())?;
            assert_eq!(validated.amount(), expected, "{text}");
            assert_eq!(validated.amount_text(), edited, "{text}");
        }
        let mut shaped = ['0'; 12];
        for (slot, c) in shaped.iter_mut().zip("+00001234.56".chars()) {
            *slot = c;
        }
        assert_eq!(numval_c(&shaped), Some(Decimal::new(123_456, 2)));
        let unshaped = ['x'; 12];
        assert_eq!(numval_c(&unshaped), None);
        Ok(())
    }

    // audit: COTRN02C-037, COTRN02C-038
    #[test]
    fn the_verdict_decides_each_date_original_first() -> TestResult {
        assert_eq!(
            failed(&edit_data_with(&valid(), reject_all)),
            Some((ORIG_INVALID, AddField::OrigDate, 400))
        );
        let only_proc = |text: &str, mask: DateMask| {
            if text == "2022-07-19" {
                reject_all(text, mask)
            } else {
                accept_all(text, mask)
            }
        };
        assert_eq!(
            failed(&edit_data_with(&valid(), only_proc)),
            Some((PROC_INVALID, AddField::ProcDate, 400))
        );
        // Message 2513 accepts despite its severity (CSUTLDTC-029).
        let accepted = edit_data_with(&valid(), unsupported_range).map_err(|f| f.to_string())?;
        assert_eq!(accepted.orig_date(), day(2022, 7, 18)?);
        assert_eq!(accepted.proc_date(), day(2022, 7, 19)?);
        // A 2513 for text that spells no calendar date cannot be stored typed: refused with the
        // date's message (X-CSUTLDTC-H2, the replacement's choice).
        let impossible = with(AddField::ProcDate, "2022-02-30");
        assert_eq!(
            failed(&edit_data_with(&impossible, unsupported_range)),
            Some((PROC_INVALID, AddField::ProcDate, 400))
        );
        Ok(())
    }

    // audit: COTRN02C-037, COTRN02C-038
    #[test]
    fn blocked_h5_the_stand_in_verdict_is_a_calendar_check() -> TestResult {
        // BLOCKED (HALTED(H5)): `validate` stands in for CEEDAYS; these values are the stand-in's
        // own rule and no parity result.
        assert_eq!(
            failed(&edit_data(&with(AddField::OrigDate, "2022-02-30"))),
            Some((ORIG_INVALID, AddField::OrigDate, 400))
        );
        assert_eq!(
            failed(&edit_data(&with(AddField::ProcDate, "2022-13-01"))),
            Some((PROC_INVALID, AddField::ProcDate, 400))
        );
        let accepted = edit_data(&valid()).map_err(|failure| failure.to_string())?;
        assert_eq!(accepted.orig_date(), day(2022, 7, 18)?);
        Ok(())
    }

    // audit: COTRN02C-039
    #[test]
    fn the_merchant_id_is_tested_last() {
        for merchant_id in ["ABC", "12345678", "-12345678", "1234 5678"] {
            assert_eq!(
                failed(&edit_data_with(
                    &with(AddField::MerchantId, merchant_id),
                    accept_all
                )),
                Some((MID_NOT_NUMERIC, AddField::MerchantId, 400)),
                "{merchant_id:?}"
            );
            // A rejected date comes first, because its call precedes the merchant edit.
            assert_eq!(
                failed(&edit_data_with(
                    &with(AddField::MerchantId, merchant_id),
                    reject_all
                )),
                Some((ORIG_INVALID, AddField::OrigDate, 400))
            );
        }
    }

    #[test]
    fn free_text_outside_printable_ascii_is_refused_after_every_cobol_edit() {
        // The replacement's request-validation rule (X-COUSR01C-3, PIC-002), no parity result.
        let cases = [
            (AddField::Source, "POS\tTERM"),
            (AddField::Description, "Café"),
            (AddField::MerchantName, "Store\u{7f}"),
            (AddField::MerchantCity, "City\0\0"),
            (AddField::MerchantZip, "１２３４５"),
        ];
        for (field, value) in cases {
            assert_eq!(
                failed(&edit_data_with(&with(field, value), accept_all)),
                Some((NOT_STORABLE, field, 400)),
                "{value:?}"
            );
        }
        // Every COBOL edit keeps its place: the merchant ID's message wins.
        let both = TranAddInput {
            description: "Café",
            merchant_id: "ABC",
            ..valid()
        };
        assert_eq!(
            failed(&edit_data_with(&both, accept_all)),
            Some((MID_NOT_NUMERIC, AddField::MerchantId, 400))
        );
    }

    // audit: COTRN02C-040, COTRN02C-041, COTRN02C-042
    #[test]
    fn the_confirmation_decides_after_the_edits() {
        let cases = [
            ("Y", Confirmation::Yes),
            ("y", Confirmation::Yes),
            ("Yes", Confirmation::Yes),
            ("N", Confirmation::NotConfirmed),
            ("n", Confirmation::NotConfirmed),
            (" ", Confirmation::NotConfirmed),
            ("", Confirmation::NotConfirmed),
            ("\0", Confirmation::NotConfirmed),
            ("X", Confirmation::Invalid),
            ("1", Confirmation::Invalid),
            ("é", Confirmation::Invalid),
        ];
        for (raw, expected) in cases {
            assert_eq!(confirm(raw), expected, "{raw:?}");
        }
        assert_eq!(
            shown(&AddFailure::invalid_confirmation()),
            (
                "Invalid value. Valid values are (Y/N)...",
                AddField::Confirm,
                400
            )
        );
    }

    // audit: COTRN02C-044, COTRN02C-050, COTRN02C-051, COTRN02C-053
    #[test]
    fn browse_and_write_failures_carry_their_literals() {
        let cases = [
            (
                AddFailure::transaction_lookup(),
                ("Unable to lookup Transaction...", AddField::AcctId, 500),
            ),
            (
                AddFailure::last_not_found(),
                ("Transaction ID NOT found...", AddField::AcctId, 404),
            ),
            (
                AddFailure::duplicate_id(),
                ("Tran ID already exist...", AddField::AcctId, 500),
            ),
            (
                AddFailure::write_failed(),
                ("Unable to Add Transaction...", AddField::AcctId, 500),
            ),
        ];
        for (failure, expected) in cases {
            assert_eq!(shown(&failure), expected);
        }
    }

    // audit: COTRN02C-043, COTRN02C-045, COTRN02C-046, COTRN02C-047, COTRN02C-048
    #[test]
    fn the_record_holds_every_field_as_add_transaction_moves_it() -> TestResult {
        let key = filled_key()?;
        let fields = passed(&TranAddInput {
            amount: "-00001234.50",
            ..valid()
        })?;
        let filler = pic::<20>("FILLER-OF-LAST-READ!")?;
        let record = build_record(pic("0000000000000101")?, &key, &fields, filler.clone());
        assert_eq!(record.id, pic::<16>("0000000000000101")?);
        assert_eq!(record.type_cd, pic::<2>("01")?);
        assert_eq!(record.cat_cd, number::<4>(5)?);
        assert_eq!(record.source, pic::<10>("POS TERM")?);
        assert_eq!(record.desc, pic::<100>("Grocery purchase")?);
        assert_eq!(
            record.amt,
            Money::<9>::new(Decimal::new(-123_450, 2)).map_err(|e| e.to_string())?
        );
        assert_eq!(record.merchant_id, number::<9>(800_000_001)?);
        assert_eq!(record.merchant_name, pic::<50>("Corner Store")?);
        assert_eq!(record.merchant_city, pic::<50>("Springfield")?);
        assert_eq!(record.merchant_zip, pic::<10>("12345")?);
        assert_eq!(record.card_num, card()?);
        assert_eq!(record.filler, filler);
        let midnight = day(2022, 7, 18)?.and_hms_opt(0, 0, 0).ok_or("midnight")?;
        assert_eq!(record.orig_ts, stamp(midnight, TsForm::ScreenDate)?);
        let bytes = record.encode();
        let orig = bytes.get(278..304).ok_or("TRAN-ORIG-TS")?;
        let proc = bytes.get(304..330).ok_or("TRAN-PROC-TS")?;
        assert!(orig == b"2022-07-18                ", "orig_ts layout S");
        assert!(proc == b"2022-07-19                ", "proc_ts layout S");
        assert_eq!(TransactionRecord::decode(&bytes), Ok(record));
        Ok(())
    }

    // audit: COTRN02C-049, COTRN02C-041, COTRN02C-057
    #[test]
    fn the_outcomes_carry_the_prompt_or_the_success_message() -> TestResult {
        let message = success_message(&pic("0000000000000101")?);
        assert_eq!(
            message,
            "Transaction added successfully.  Your Tran ID is 0000000000000101."
        );
        assert_eq!(message.chars().count(), 66);
        // `TRAN-ID DELIMITED BY SPACE` stops at the first space.
        assert_eq!(
            success_message(&pic("12345 7890123456")?),
            "Transaction added successfully.  Your Tran ID is 12345."
        );
        let added = AddOutcome::added(pic("0000000000000101")?);
        assert_eq!(added.message(), message);
        assert_eq!(added.focus(), AddField::AcctId);
        let key = filled_key()?;
        let fields = passed(&valid())?;
        let prompt = AddOutcome::prompt(key.clone(), fields.clone(), None);
        assert_eq!(prompt.message(), "Confirm to add this transaction...");
        assert_eq!(prompt.focus(), AddField::Confirm);
        let AddOutcome::Prompt(content) = prompt else {
            return Err("a prompt".to_owned());
        };
        assert_eq!(content.key(), &key);
        assert_eq!(content.fields(), &fields);
        assert_eq!(content.copied(), None);
        assert_eq!(content.fields().type_cd(), &pic::<2>("01")?);
        assert_eq!(content.fields().cat_cd(), &pic::<4>("0005")?);
        assert_eq!(content.fields().source(), &pic::<10>("POS TERM")?);
        assert_eq!(
            content.fields().description(),
            &pic::<60>("Grocery purchase")?
        );
        assert_eq!(content.fields().merchant_id(), &pic::<9>("800000001")?);
        assert_eq!(
            content.fields().merchant_name(),
            &pic::<30>("Corner Store")?
        );
        assert_eq!(content.fields().merchant_city(), &pic::<25>("Springfield")?);
        assert_eq!(content.fields().merchant_zip(), &pic::<10>("12345")?);
        Ok(())
    }

    // audit: COTRN02C-053, COTRN02C-054, COTRN02C-055
    #[test]
    fn pf5_copies_the_highest_id_transaction_as_the_map_receives_it() -> TestResult {
        let copied = copy_last(&last_transaction()?);
        assert_eq!(copied.type_cd(), "03");
        assert_eq!(copied.cat_cd(), "0002");
        assert_eq!(copied.source(), "OPERATOR  ");
        assert_eq!(copied.description(), "D".repeat(60));
        // The ninth integer digit is dropped by `+99999999.99` (X-COTRN02C-4).
        assert_eq!(copied.amount(), "-23456789.12");
        assert_eq!(copied.orig_date_text(), "2022-06-10");
        assert_eq!(copied.orig_date(), Some(day(2022, 6, 10)?));
        assert_eq!(copied.proc_date_text(), "          ");
        assert_eq!(copied.proc_date(), None);
        assert_eq!(copied.merchant_id(), "000000042");
        assert_eq!(copied.merchant_name(), "N".repeat(30));
        assert_eq!(copied.merchant_city(), "C".repeat(25));
        assert_eq!(copied.merchant_zip(), "98765     ");
        Ok(())
    }

    // audit: COTRN02C-056, COTRN02C-026, COTRN02C-055
    #[test]
    fn pf5_runs_the_enter_processing_on_the_copied_fields() -> TestResult {
        let mut last = last_transaction()?;
        // A blank copied timestamp fails the emptiness edit.
        let blank_proc = copy_last(&last);
        assert_eq!(
            failed(&edit_data_with(&blank_proc.as_input("Y"), accept_all)),
            Some(("Proc Date can NOT be empty...", AddField::ProcDate, 400))
        );
        let at = day(2022, 7, 18)?
            .and_hms_micro_opt(10, 15, 42, 370_000)
            .ok_or("no such time")?;
        last.proc_ts = stamp(at, TsForm::Db2)?;
        let copied = copy_last(&last);
        assert_eq!(copied.proc_date_text(), "2022-07-18");
        let input = copied.as_input("y");
        assert_eq!(confirm(input.confirm), Confirmation::Yes);
        let fields = edit_data_with(&input, accept_all).map_err(|f| f.to_string())?;
        assert_eq!(fields.amount_text(), "-23456789.12");
        assert_eq!(
            fields.amount(),
            Money::<9>::new(Decimal::new(-2_345_678_912, 2)).map_err(|e| e.to_string())?
        );
        assert_eq!(fields.proc_date(), day(2022, 7, 18)?);
        let record = build_record(
            pic("0000000000000101")?,
            &filled_key()?,
            &fields,
            last.filler.clone(),
        );
        // The copy's text fields reach the record as the map fields held them.
        assert_eq!(record.desc, pic::<100>(&"D".repeat(60))?);
        assert_eq!(record.merchant_name, pic::<50>(&"N".repeat(30))?);
        assert_eq!(
            record.proc_ts.value().map(|ts| ts.form()),
            Some(TsForm::ScreenDate)
        );
        let prompt = AddOutcome::prompt(filled_key()?, fields, Some(copied.clone()));
        let AddOutcome::Prompt(content) = prompt else {
            return Err("a prompt".to_owned());
        };
        assert_eq!(content.copied(), Some(&copied));
        Ok(())
    }

    #[test]
    fn every_edit_is_total_over_awkward_text() {
        let long = "9".repeat(4_096);
        let samples = [
            "",
            "\0",
            "é",
            "１２３",
            "\u{10FFFF}",
            "-",
            "+.",
            long.as_str(),
        ];
        for sample in samples {
            let _ = edit_keys(Some(sample), Some(sample));
            let _ = edit_keys(None, Some(sample));
            let _ = confirm(sample);
            let input = TranAddInput {
                type_cd: sample,
                cat_cd: sample,
                source: sample,
                description: sample,
                amount: sample,
                orig_date: sample,
                proc_date: sample,
                merchant_id: sample,
                merchant_name: sample,
                merchant_city: sample,
                merchant_zip: sample,
                confirm: sample,
            };
            let _ = edit_data(&input);
            for field in EMPTY_MESSAGES.map(|(field, _)| field) {
                let _ = edit_data(&with(field, sample));
            }
        }
        // A long all-digit key is cut to its map field and read as the 11 or 16 digits.
        assert!(edit_keys(Some(&long), None).is_ok());
        assert!(edit_keys(None, Some(&long)).is_ok());
    }
}
