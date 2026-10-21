//! COACTVWC, account view: the rules of endpoint 4, `GET /api/v1/accounts/{acct_id}/view`
//! (transaction `CAVW`, map `CACTVWA`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COACTVWC.md`: the account-number
//! edit (COACTVWC-029 to COACTVWC-032), the read chain and its message precedence
//! (COACTVWC-033 to COACTVWC-047), and the blocks and fields the view shows (COACTVWC-008 to
//! COACTVWC-026). The `client` rows, among them the first display (COACTVWC-001), the keys
//! (COACTVWC-003), F3 back to the menu (COACTVWC-004), the header (COACTVWC-006) and the constant
//! information row (COACTVWC-027), send no request and belong to the screen. No paragraph of the
//! program reads the signed-on user ID or type, so nothing here takes one and endpoint 4 has no
//! admin gate (COACTVWC-002); the program issues only `READ` commands, so no outcome writes
//! (COACTVWC-005).
//!
//! # Flow
//!
//! `api::services::coactvwc` hands the raw `acct_id` path text to [`edit_account`], which owns
//! the edit's messages (COACTVWC-029 to COACTVWC-032). When the edit passes, the service reads the
//! card cross-reference by account, then the account, then the customer the cross-reference
//! names, and passes the three outcomes to [`view`] as [`Lookup`] values. [`view`] decides the
//! response or the [`ViewFailure`] as `9000-READ-ACCT` and `1200-SETUP-SCREEN-VARS` do
//! (COACTVWC-033). This module does no I/O: no store, file, clock or environment.
//!
//! A failure is a [`ViewFailure`]: the message the program moves to `WS-RETURN-MSG PIC X(75)`,
//! without its trailing spaces, the request field it turns red, its status class, and the account
//! and customer blocks that the COBOL still displays beside some messages (COACTVWC-008,
//! escalation candidate X-COACTVWC-6).
//!
//! # Halts
//!
//! - HALTED(H4): the not-found and `File Error:` texts embed the CICS `RESP` and `RESP2` of the
//!   failed read (COACTVWC-036, COACTVWC-038, COACTVWC-041, COACTVWC-043, COACTVWC-045,
//!   COACTVWC-047). CICS defines those values and the store has none, so every code position
//!   holds the image of zero as interim handling, never a parity result. The fixed text, the
//!   keys, the status and the field around them are exact (COACTVWC-035, COACTVWC-037,
//!   COACTVWC-040, COACTVWC-042, COACTVWC-044, COACTVWC-046).
//! - Halt candidate X-COACTVWC-H1: when the account read finds nothing or fails and the customer
//!   read then finds the customer, the COBOL shows an account block moved from a record no read
//!   filled (COACTVWC-009). The block is present here as the program presents it, with the image
//!   of an initialised record as interim values, which are no parity result.
//! - Halt candidate X-COCRDSLC-H2: whether a re-entry edits its input at all depends on
//!   uninitialised COMMAREA storage (COACTVWC-048). The replacement has no COMMAREA and always
//!   edits the input; nothing here models that case.
//!
//! # Cardholder data
//!
//! The COBOL shows the full SSN as `NNN-NN-NNNN` (`COACTVWC.cbl:495-504`). Here it stays a
//! [`Ssn`], whose `Debug`, `Display` and `Serialize` produce only `***-**-` and the last four
//! digits (COACTVWC-017, E32), and no message interpolates it. The card number the
//! cross-reference read moves to the COMMAREA is in no value this module returns (COACTVWC-034).
//!
//! # Totality
//!
//! [`edit_account`] takes any text, empty, longer than the field or not ASCII, and returns a key
//! or a [`ViewFailure`]; [`view`] is total over every combination of lookups. Nothing indexes,
//! slices, unwraps or panics (Total Parsers Over Untrusted Input).

use core::fmt;

use chrono::NaiveDate;
use copybook::edited::EditedPicture;
use copybook::{PicU, PicX};

use crate::money::Money;
use crate::records::account::AccountRecord;
use crate::records::card_xref::CardXrefRecord;
use crate::records::customer::CustomerRecord;
use crate::sensitive::Ssn;

/// Width of `ACCTSIDI` and of `CC-ACCT-ID PIC X(11)`, into which the input is moved
/// (COACTVWC-029, COACTVW-013).
const ACCT_ID_WIDTH: usize = 11;

/// Width of `WS-RETURN-MSG PIC X(75)`, the field every message is built in (COACTVWC-035).
const RETURN_MSG_WIDTH: usize = 75;

/// `ACCTSIDI = '*'`: an asterisk, which the program itself shows after `'No input received'`,
/// padded with spaces to the field (COACTVWC-029).
const ASTERISK_ONLY: &[u8; ACCT_ID_WIDTH] = b"*          ";

/// `NO-SEARCH-CRITERIA-RECEIVED`, set by the cross-field edit for an account number not supplied;
/// it overwrites `WS-PROMPT-FOR-ACCT`, whose text is therefore never returned (COACTVWC-030,
/// X-COACTVWC-1).
const NO_INPUT_RECEIVED: &str = "No input received";

/// The literal `2210-EDIT-ACCOUNT` moves for a value that is not eleven digits or is all zeros,
/// two spaces after `must` included (`COACTVWC.cbl:671-673`; COACTVWC-031).
const ACCOUNT_FILTER_INVALID: &str = "Account Filter must  be a non-zero 11 digit number";

/// The pieces of the not-found texts of the account-keyed reads (COACTVWC-035, COACTVWC-040).
const ACCOUNT_LABEL: &[u8] = b"Account:";
const NOT_FOUND_IN: &[u8] = b" not found in";
const CROSS_REF_FILE_RESP: &[u8] = b" Cross ref file.  Resp:";
const ACCT_MASTER_FILE_RESP: &[u8] = b" Acct Master file.Resp:";
const REAS_LABEL: &[u8] = b" Reas:";

/// The pieces of the customer not-found text (COACTVWC-044).
const CUST_ID_LABEL: &[u8] = b"CustId:";
const NOT_FOUND: &[u8] = b" not found";
const IN_CUSTOMER_MASTER_RESP: &[u8] = b" in customer master.Resp: ";
const REAS_UPPER_LABEL: &[u8] = b" REAS:";

/// The `FILLER` literals of `WS-FILE-ERROR-MESSAGE`, which the `INITIALIZE` of
/// `COACTVWC.cbl:268` leaves in place (`COACTVWC.cbl:86-105`; COACTVWC-037).
const FILE_ERROR_HEAD: &[u8] = b"File Error: ";
const ON: &[u8] = b" on ";
const RETURNED_RESP: &[u8] = b" returned RESP ";
const RESP2_LABEL: &[u8] = b",RESP2 ";
const FILE_ERROR_FILLER: &[u8] = b"     ";

/// The operation every failed read names, moved to `ERROR-OPNAME PIC X(8)` (COACTVWC-037).
const READ_OPNAME: &[u8] = b"READ";

/// `LIT-CARDXREFNAME-ACCT-PATH`, the path over the cross-reference's alternate index on account
/// ID (COACTVWC-037, COACTVWC-039).
const LIT_CARDXREFNAME_ACCT_PATH: &[u8] = b"CXACAIX ";

/// `LIT-ACCTFILENAME`, the account master (COACTVWC-042).
const LIT_ACCTFILENAME: &[u8] = b"ACCTDAT ";

/// `LIT-CUSTFILENAME`, the customer master (COACTVWC-046).
const LIT_CUSTFILENAME: &[u8] = b"CUSTDAT ";

// HALTED(H4): `ERROR-RESP` and `ERROR-RESP2 PIC X(10)` receive the CICS `RESP` and `RESP2` of
// the failed read, moved from `S9(09) COMP` as nine digits and a space. CICS defines the values
// and the store has no counterpart, so both hold this image of zero: interim handling, never a
// parity result (COACTVWC-036, COACTVWC-038, COACTVWC-041, COACTVWC-043, COACTVWC-045,
// COACTVWC-047).
/// The interim content of each code field of the read-failure texts (halt H4).
const HALTED_CODE_FIELD: &[u8] = b"000000000 ";

/// `PICOUT='+ZZZ,ZZZ,ZZZ.99'` of the five amount fields of the map (COACTVWC-011 to
/// COACTVWC-013; COACTVW-022, COACTVW-028, COACTVW-034, COACTVW-037, COACTVW-043).
const AMOUNT_PICTURE: EditedPicture = EditedPicture::PlusZCommas9Dot99;

/// The outcome of one read of the chain, as the service maps the store's answer.
///
/// A read the service did not issue, because the cross-reference read did not find exactly one
/// record, is never examined by [`view`] (COACTVWC-033): pass [`Lookup::NotFound`] for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup<T> {
    /// CICS `NORMAL`: the record was read.
    Found(T),
    /// CICS `NOTFND`: no record has the key (COACTVWC-035, COACTVWC-040, COACTVWC-044).
    NotFound,
    /// The `WHEN OTHER` branch: a store error, or for the cross-reference two or more rows with
    /// the account ID, which CICS answers with `DUPKEY` (COACTVWC-037, COACTVWC-039,
    /// COACTVWC-042, COACTVWC-046).
    Error,
}

/// The request field a failure turns red (COACTVWC-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewField {
    /// `ACCTSID`, the account number: after a failed edit and after a cross-reference or account
    /// read that did not succeed (COACTVWC-030, COACTVWC-031, COACTVWC-033).
    AcctId,
}

impl ViewField {
    /// The field's name in the request and in `ScreenError.field`: `acct_id`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AcctId => "acct_id",
        }
    }
}

/// The status class of a [`ViewFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The account-number edit failed: 400 (COACTVWC-030, COACTVWC-031).
    Input,
    /// A read found no record: 404 (COACTVWC-035, COACTVWC-040, COACTVWC-044).
    NotFound,
    /// A read failed other than not found: 500 (COACTVWC-037, COACTVWC-042, COACTVWC-046).
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

/// The account fields of the view, as `1200-SETUP-SCREEN-VARS` moves them from `ACCOUNT-RECORD`
/// into the map (COACTVWC-010 to COACTVWC-015).
///
/// Each amount is the 15-character text of the map's `+ZZZ,ZZZ,ZZZ.99` field, so no consumer
/// formats a number (AAP 0.3.4.1); a magnitude of 1,000,000,000.00 or more loses its tenth
/// integer digit there, as on the screen (X-COACTVWC-5). Dates are typed, `None` for a blank
/// field (Typed Dates and Timestamps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountBlock {
    /// `ACCT-ACTIVE-STATUS` → `ACSTTUSO PIC X(1)`, unchanged (COACTVWC-010, COACTVW-016).
    pub active_status: PicX<1>,
    /// `ACCT-CURR-BAL` → `ACURBALO PIC +ZZZ,ZZZ,ZZZ.99` (COACTVWC-011, COACTVW-034).
    pub current_balance: String,
    /// `ACCT-CREDIT-LIMIT` → `ACRDLIMO` (COACTVWC-012, COACTVW-022).
    pub credit_limit: String,
    /// `ACCT-CASH-CREDIT-LIMIT` → `ACSHLIMO` (COACTVWC-012, COACTVW-028).
    pub cash_credit_limit: String,
    /// `ACCT-CURR-CYC-CREDIT` → `ACRCYCRO` (COACTVWC-013, COACTVW-037).
    pub current_cycle_credit: String,
    /// `ACCT-CURR-CYC-DEBIT` → `ACRCYDBO` (COACTVWC-013, COACTVW-043).
    pub current_cycle_debit: String,
    /// `ACCT-OPEN-DATE` → `ADTOPENO PIC X(10)` (COACTVWC-014, COACTVW-019).
    pub open_date: Option<NaiveDate>,
    /// `ACCT-EXPIRAION-DATE` → `AEXPDTO PIC X(10)` (COACTVWC-014, COACTVW-025).
    pub expiration_date: Option<NaiveDate>,
    /// `ACCT-REISSUE-DATE` → `AREISDTO PIC X(10)` (COACTVWC-014, COACTVW-031).
    pub reissue_date: Option<NaiveDate>,
    /// `ACCT-GROUP-ID` → `AADDGRPO PIC X(10)`, unchanged; blank in every fixture record, while
    /// `ACCT-ADDR-ZIP`, which holds the group code there, is not shown (COACTVWC-015, E07).
    pub group_id: PicX<10>,
}

impl AccountBlock {
    /// The block `1200-SETUP-SCREEN-VARS` moves from an account the read found (COACTVWC-010 to
    /// COACTVWC-015).
    fn from_record(record: &AccountRecord) -> Self {
        Self {
            active_status: record.active_status.clone(),
            current_balance: edited_amount(record.curr_bal),
            credit_limit: edited_amount(record.credit_limit),
            cash_credit_limit: edited_amount(record.cash_credit_limit),
            current_cycle_credit: edited_amount(record.curr_cyc_credit),
            current_cycle_debit: edited_amount(record.curr_cyc_debit),
            open_date: record.open_date.value(),
            expiration_date: record.expiraion_date.value(),
            reissue_date: record.reissue_date.value(),
            group_id: record.group_id.clone(),
        }
    }

    /// The block shown when no read filled `ACCOUNT-RECORD` but the customer was found
    /// (COACTVWC-009).
    // HALTED(X-COACTVWC-H1): the COBOL moves these fields from working storage that no read of
    // the task filled, so their values have no single result. The image of an initialised
    // record (alphanumerics spaces, amounts zero, dates blank) is interim handling only; that
    // the block is present is the assertable part (COACTVWC-008).
    fn unfilled() -> Self {
        let zero = edited_amount(Money::<10>::ZERO);
        Self {
            active_status: PicX::spaces(),
            current_balance: zero.clone(),
            credit_limit: zero.clone(),
            cash_credit_limit: zero.clone(),
            current_cycle_credit: zero.clone(),
            current_cycle_debit: zero,
            open_date: None,
            expiration_date: None,
            reissue_date: None,
            group_id: PicX::spaces(),
        }
    }
}

/// The customer fields of the view, as `1200-SETUP-SCREEN-VARS` moves them from
/// `CUSTOMER-RECORD` into the map (COACTVWC-016 to COACTVWC-026).
///
/// An alphanumeric `MOVE` into a narrower map field keeps its leftmost characters: the ZIP code
/// keeps five of ten and each phone number 13 of 15 (X-COACTVWC-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerBlock {
    /// `CUST-ID` → `ACSTNUMO PIC X(9)`: nine digits with leading zeros, the cross-reference's
    /// customer ID that keyed the read (COACTVWC-016, COACTVW-047).
    pub customer_id: PicU<9>,
    /// `CUST-SSN` → `ACSTSSNO PIC X(12)`, held as [`Ssn`] so that only `***-**-` and the last
    /// four digits ever leave (COACTVWC-017, COACTVW-050, E32).
    pub ssn: Ssn,
    /// `CUST-FICO-CREDIT-SCORE` → `ACSTFCOO PIC X(3)`: three digits with leading zeros
    /// (COACTVWC-018, COACTVW-056).
    pub fico_score: PicU<3>,
    /// `CUST-DOB-YYYY-MM-DD` → `ACSTDOBO PIC X(10)`, typed, `None` for a blank field
    /// (COACTVWC-019, COACTVW-053).
    pub date_of_birth: Option<NaiveDate>,
    /// `CUST-FIRST-NAME` → `ACSFNAMO PIC X(25)` (COACTVWC-020, COACTVW-061).
    pub first_name: PicX<25>,
    /// `CUST-MIDDLE-NAME` → `ACSMNAMO PIC X(25)` (COACTVWC-020, COACTVW-063).
    pub middle_name: PicX<25>,
    /// `CUST-LAST-NAME` → `ACSLNAMO PIC X(25)` (COACTVWC-020, COACTVW-065).
    pub last_name: PicX<25>,
    /// `CUST-ADDR-LINE-1` → `ACSADL1O PIC X(50)` (COACTVWC-021, COACTVW-067).
    pub address_line_1: PicX<50>,
    /// `CUST-ADDR-LINE-2` → `ACSADL2O PIC X(50)` (COACTVWC-021, COACTVW-072).
    pub address_line_2: PicX<50>,
    /// `CUST-ADDR-LINE-3` → `ACSCITYO PIC X(50)`, the field the map labels `City`
    /// (COACTVWC-021, COACTVW-078).
    pub city: PicX<50>,
    /// `CUST-ADDR-STATE-CD` → `ACSSTTEO PIC X(2)` (COACTVWC-022, COACTVW-070).
    pub state: PicX<2>,
    /// `CUST-ADDR-ZIP PIC X(10)` → `ACSZIPCO PIC X(5)`: its first five characters
    /// (COACTVWC-023, COACTVW-075).
    pub zip: PicX<5>,
    /// `CUST-ADDR-COUNTRY-CD` → `ACSCTRYO PIC X(3)` (COACTVWC-022, COACTVW-081).
    pub country: PicX<3>,
    /// `CUST-PHONE-NUM-1 PIC X(15)` → `ACSPHN1O PIC X(13)`: its first 13 characters
    /// (COACTVWC-024, COACTVW-084).
    pub phone_1: PicX<13>,
    /// `CUST-PHONE-NUM-2 PIC X(15)` → `ACSPHN2O PIC X(13)`: its first 13 characters
    /// (COACTVWC-024, COACTVW-089).
    pub phone_2: PicX<13>,
    /// `CUST-GOVT-ISSUED-ID` → `ACSGOVTO PIC X(20)` (COACTVWC-025, COACTVW-086).
    pub government_id: PicX<20>,
    /// `CUST-EFT-ACCOUNT-ID` → `ACSEFTCO PIC X(10)` (COACTVWC-025, COACTVW-091).
    pub eft_account_id: PicX<10>,
    /// `CUST-PRI-CARD-HOLDER-IND` → `ACSPFLGO PIC X(1)` (COACTVWC-026, COACTVW-094).
    pub primary_card_holder: PicX<1>,
}

impl CustomerBlock {
    /// The block `1200-SETUP-SCREEN-VARS` moves from a customer the read found (COACTVWC-016 to
    /// COACTVWC-026).
    fn from_record(record: &CustomerRecord) -> Self {
        Self {
            customer_id: record.id,
            ssn: record.ssn,
            fico_score: record.fico_credit_score,
            date_of_birth: record.dob_yyyy_mm_dd.value(),
            first_name: record.first_name.clone(),
            middle_name: record.middle_name.clone(),
            last_name: record.last_name.clone(),
            address_line_1: record.addr_line_1.clone(),
            address_line_2: record.addr_line_2.clone(),
            city: record.addr_line_3.clone(),
            state: record.addr_state_cd.clone(),
            zip: PicX::moved_from(record.addr_zip.as_bytes()),
            country: record.addr_country_cd.clone(),
            phone_1: PicX::moved_from(record.phone_num_1.as_bytes()),
            phone_2: PicX::moved_from(record.phone_num_2.as_bytes()),
            government_id: record.govt_issued_id.clone(),
            eft_account_id: record.eft_account_id.clone(),
            primary_card_holder: record.pri_card_holder_ind.clone(),
        }
    }
}

/// A successful view: all three reads found their record, so the screen shows both blocks and
/// no message (COACTVWC-008).
///
/// No card number is part of it (COACTVWC-034), and the SSN is a [`Ssn`] (COACTVWC-017).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountView {
    /// The eleven-digit key every read used, leading zeros included (COACTVWC-032).
    pub acct_id: PicU<11>,
    /// The account fields (COACTVWC-010 to COACTVWC-015).
    pub account: AccountBlock,
    /// The customer fields (COACTVWC-016 to COACTVWC-026).
    pub customer: CustomerBlock,
}

/// A screen COACTVWC sends with a message instead of a full view.
///
/// The message is `WS-RETURN-MSG PIC X(75)` without its trailing spaces, as the response
/// carries it; it never holds the SSN or a card number. Beside some messages the COBOL still
/// shows a block: the account block after the customer read failed, and both blocks after the
/// account read failed and the customer read found the customer (COACTVWC-008,
/// X-COACTVWC-6). [`ViewFailure::account`] and [`ViewFailure::customer`] carry them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFailure {
    message: String,
    field: Option<ViewField>,
    class: FailureClass,
    // Boxed so that every `Result` carrying the failure stays small.
    account: Option<Box<AccountBlock>>,
    customer: Option<Box<CustomerBlock>>,
}

impl ViewFailure {
    /// A failure with no block.
    fn new(message: String, class: FailureClass, field: Option<ViewField>) -> Self {
        Self {
            message,
            field,
            class,
            account: None,
            customer: None,
        }
    }

    /// A failed account-number edit: status class [`FailureClass::Input`], field `acct_id`
    /// (COACTVWC-030, COACTVWC-031).
    fn input(message: &str) -> Self {
        Self::new(
            message.to_owned(),
            FailureClass::Input,
            Some(ViewField::AcctId),
        )
    }

    /// This failure with the account block shown beside its message.
    fn with_account(mut self, block: AccountBlock) -> Self {
        self.account = Some(Box::new(block));
        self
    }

    /// This failure with the customer block shown beside its message.
    fn with_customer(mut self, block: CustomerBlock) -> Self {
        self.customer = Some(Box::new(block));
        self
    }

    /// The message, as the response carries it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The field the program turns red, if any: `acct_id` after a failed edit or a
    /// cross-reference or account read that did not succeed, none when only the customer read
    /// did not (COACTVWC-028, COACTVWC-033).
    #[must_use]
    pub const fn field(&self) -> Option<ViewField> {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }

    /// The account block the screen shows beside the message, if any (COACTVWC-008). After an
    /// account read that did not succeed its values are the interim ones of halt candidate
    /// X-COACTVWC-H1 (COACTVWC-009): only its presence is a parity result.
    #[must_use]
    pub fn account(&self) -> Option<&AccountBlock> {
        self.account.as_deref()
    }

    /// The customer block the screen shows beside the message, if any (COACTVWC-008).
    #[must_use]
    pub fn customer(&self) -> Option<&CustomerBlock> {
        self.customer.as_deref()
    }
}

impl fmt::Display for ViewFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ViewFailure {}

/// `2200-EDIT-MAP-INPUTS` and `2210-EDIT-ACCOUNT` over the raw text of the account-number field:
/// the key of every read when the edit passes (COACTVWC-029 to COACTVWC-032).
///
/// `raw` is moved into `CC-ACCT-ID PIC X(11)`, padded with spaces on the right. The field is not
/// supplied when it is eleven spaces, an asterisk followed by ten spaces or eleven low-values
/// (`\0`); otherwise it must be eleven ASCII digits, not all zeros, and the digits, leading zeros
/// included, are the key. Any other value, a shorter one, one with a leading space, a sign, a
/// point or a letter, fails the numeric edit.
///
/// # Errors
///
/// A [`ViewFailure`] of class [`FailureClass::Input`] with field [`ViewField::AcctId`] and no
/// block: `No input received` for a value not supplied (COACTVWC-030), or
/// `Account Filter must  be a non-zero 11 digit number` for any other value that is not a
/// non-zero eleven-digit number (COACTVWC-031).
pub fn edit_account(raw: &str) -> Result<PicU<11>, ViewFailure> {
    let bytes = raw.as_bytes();
    // A text longer than the 11-byte field cannot be what the terminal sent, and the COBOL MOVE
    // would drop its tail; it is refused by the numeric edit rather than cut, so a request never
    // reads an account other than the one typed.
    if bytes.len() > ACCT_ID_WIDTH {
        return Err(ViewFailure::input(ACCOUNT_FILTER_INVALID));
    }
    let field = PicX::<ACCT_ID_WIDTH>::moved_from(bytes);
    if is_not_supplied(field.as_bytes()) {
        return Err(ViewFailure::input(NO_INPUT_RECEIVED));
    }
    match PicU::<11>::decode(field.as_bytes()) {
        Ok(key) if key.get() != 0 => Ok(key),
        Ok(_) | Err(_) => Err(ViewFailure::input(ACCOUNT_FILTER_INVALID)),
    }
}

/// Whether the account-number field counts as not supplied: `ACCTSIDI = '*' OR SPACES` sets
/// `CC-ACCT-ID` to low-values, and `2210-EDIT-ACCOUNT` tests `LOW-VALUES OR SPACES`
/// (COACTVWC-029, COACTVWC-030).
fn is_not_supplied(field: &[u8; ACCT_ID_WIDTH]) -> bool {
    field == ASTERISK_ONLY
        || field.iter().all(|&byte| byte == b' ')
        || field.iter().all(|&byte| byte == 0)
}

/// `9000-READ-ACCT` and `1200-SETUP-SCREEN-VARS`: the view of account `acct_id` from the
/// outcomes of its three reads, in program order (COACTVWC-008, COACTVWC-009, COACTVWC-033 to
/// COACTVWC-047).
///
/// `acct_id` is the key [`edit_account`] returned; `xref` is the cross-reference read by that
/// account, `account` the account read by the same key, and `customer` the customer read by the
/// cross-reference's customer ID. Unless `xref` is [`Lookup::Found`] the program reads nothing
/// else, so `account` and `customer` are then not examined. Otherwise the customer read follows
/// whatever the account read returned, because the exit that would skip it tests a message no
/// statement sets (X-COACTVWC-2). The first message set stands, except that a customer read
/// failing other than not found replaces any earlier one.
///
/// # Errors
///
/// A [`ViewFailure`] unless all three reads found their record:
///
/// - cross-reference not found: 404 and field `acct_id` (COACTVWC-035, COACTVWC-036);
///   failed, two or more rows included: 500, the `CXACAIX` file error and field `acct_id`
///   (COACTVWC-037 to COACTVWC-039); no block either way;
/// - account not found: 404 (COACTVWC-040, COACTVWC-041); failed: 500, the `ACCTDAT` file error
///   (COACTVWC-042, COACTVWC-043); field `acct_id`, and both blocks when the customer is found
///   (COACTVWC-009);
/// - account found and customer not found: 404 with the customer message (COACTVWC-044,
///   COACTVWC-045), no field and the account block;
/// - customer read failed: 500, the `CUSTDAT` file error, whatever came before (COACTVWC-046,
///   COACTVWC-047); field `acct_id` only when the account read did not succeed, and the account
///   block only when it did.
pub fn view(
    acct_id: PicU<11>,
    xref: Lookup<&CardXrefRecord>,
    account: Lookup<&AccountRecord>,
    customer: Lookup<&CustomerRecord>,
) -> Result<AccountView, ViewFailure> {
    let failed_key = Some(ViewField::AcctId);
    // 9200-GETCARDXREF-BYACCT: only exactly one record lets the chain go on (COACTVWC-034).
    let xref = match xref {
        Lookup::Found(record) => record,
        Lookup::NotFound => {
            let message = account_keyed_not_found(acct_id, CROSS_REF_FILE_RESP);
            return Err(ViewFailure::new(
                message,
                FailureClass::NotFound,
                failed_key,
            ));
        }
        Lookup::Error => {
            let message = file_error(LIT_CARDXREFNAME_ACCT_PATH);
            return Err(ViewFailure::new(message, FailureClass::Store, failed_key));
        }
    };
    // 9300-GETACCTDATA-BYACCT: a failure sets the message, which the customer read keeps unless
    // that read fails other than not found.
    let account = match account {
        Lookup::Found(record) => Ok(record),
        Lookup::NotFound => Err(ViewFailure::new(
            account_keyed_not_found(acct_id, ACCT_MASTER_FILE_RESP),
            FailureClass::NotFound,
            failed_key,
        )),
        Lookup::Error => Err(ViewFailure::new(
            file_error(LIT_ACCTFILENAME),
            FailureClass::Store,
            failed_key,
        )),
    };
    // 9400-GETCUSTDATA-BYCUST, then the blocks of 1200-SETUP-SCREEN-VARS: the account block
    // when either master read found its record, the customer block when the customer read did.
    match (account, customer) {
        (Ok(account), Lookup::Found(customer)) => Ok(AccountView {
            acct_id,
            account: AccountBlock::from_record(account),
            customer: CustomerBlock::from_record(customer),
        }),
        (Ok(account), Lookup::NotFound) => Err(ViewFailure::new(
            customer_not_found(xref.cust_id),
            FailureClass::NotFound,
            None,
        )
        .with_account(AccountBlock::from_record(account))),
        (Ok(account), Lookup::Error) => {
            Err(
                ViewFailure::new(file_error(LIT_CUSTFILENAME), FailureClass::Store, None)
                    .with_account(AccountBlock::from_record(account)),
            )
        }
        (Err(failure), Lookup::Found(customer)) => Err(failure
            .with_account(AccountBlock::unfilled())
            .with_customer(CustomerBlock::from_record(customer))),
        // The account message stands: the customer message is set only while none is.
        (Err(failure), Lookup::NotFound) => Err(failure),
        (Err(_), Lookup::Error) => Err(ViewFailure::new(
            file_error(LIT_CUSTFILENAME),
            FailureClass::Store,
            failed_key,
        )),
    }
}

/// An amount as the `MOVE` into the map's `+ZZZ,ZZZ,ZZZ.99` field edits it (COACTVWC-011 to
/// COACTVWC-013).
fn edited_amount(amount: Money<10>) -> String {
    AMOUNT_PICTURE.format(amount.as_decimal())
}

/// `pieces` strung into `WS-RETURN-MSG PIC X(75)`, which holds spaces before, and the field
/// without its trailing spaces. `STRING` with no `ON OVERFLOW` stops at the end of the field and
/// a `MOVE` of a longer group keeps its leftmost bytes, so both cut on the right at 75 bytes.
fn return_msg(pieces: &[&[u8]]) -> String {
    PicX::<RETURN_MSG_WIDTH>::moved_from(&pieces.concat()).to_text()
}

/// The digits of a `PIC 9(n)` key, leading zeros included, as the `MOVE` to its `X(n)`
/// redefinition shows them.
fn key_digits<const N: u32>(key: PicU<N>) -> Vec<u8> {
    let mut digits = Vec::new();
    key.encode_into(&mut digits);
    digits
}

/// The not-found text of a read keyed by account: `'Account:'`, the eleven-digit key,
/// `' not found in'`, the file's label, the `RESP` field, `' Reas:'` and the first four
/// characters of the `RESP2` field, cut at 75 bytes (COACTVWC-035, COACTVWC-040).
fn account_keyed_not_found(acct_id: PicU<11>, file_label: &[u8]) -> String {
    return_msg(&[
        ACCOUNT_LABEL,
        &key_digits(acct_id),
        NOT_FOUND_IN,
        file_label,
        HALTED_CODE_FIELD,
        REAS_LABEL,
        HALTED_CODE_FIELD,
    ])
}

/// The customer not-found text: `'CustId:'`, the nine-digit customer ID, `' not found'`,
/// `' in customer master.Resp: '`, the `RESP` field, `' REAS:'` and the first seven characters
/// of the `RESP2` field, cut at 75 bytes (COACTVWC-044).
fn customer_not_found(cust_id: PicU<9>) -> String {
    return_msg(&[
        CUST_ID_LABEL,
        &key_digits(cust_id),
        NOT_FOUND,
        IN_CUSTOMER_MASTER_RESP,
        HALTED_CODE_FIELD,
        REAS_UPPER_LABEL,
        HALTED_CODE_FIELD,
    ])
}

/// The first 75 of the 80 bytes of `WS-FILE-ERROR-MESSAGE` after a failed read of `file`:
/// `'File Error: '`, `READ` in `ERROR-OPNAME PIC X(8)`, `' on '`, the file in
/// `ERROR-FILE PIC X(9)`, `' returned RESP '`, the `RESP` field, `',RESP2 '` and the `RESP2`
/// field; the trailing `FILLER` falls outside the message (COACTVWC-037, COACTVWC-042,
/// COACTVWC-046).
fn file_error(file: &[u8]) -> String {
    return_msg(&[
        FILE_ERROR_HEAD,
        PicX::<8>::moved_from(READ_OPNAME).as_bytes(),
        ON,
        PicX::<9>::moved_from(file).as_bytes(),
        RETURNED_RESP,
        HALTED_CODE_FIELD,
        RESP2_LABEL,
        HALTED_CODE_FIELD,
        FILE_ERROR_FILLER,
    ])
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use copybook::framing::decode_all;
    use copybook::{PicDate, PicU, PicX};

    use super::{
        AccountBlock, AccountView, CustomerBlock, FailureClass, Lookup, ViewFailure, ViewField,
        edit_account, view,
    };
    use crate::money::{Decimal, Money};
    use crate::records::account::AccountRecord;
    use crate::records::card_xref::CardXrefRecord;
    use crate::records::customer::CustomerRecord;
    use crate::sensitive::{CardNumber, Ssn};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The three master files, read in place (Local-Only Runtime).
    const ACCTDATA: &[u8] = include_bytes!("../../../../../app/data/ASCII/acctdata.txt");
    const CUSTDATA: &[u8] = include_bytes!("../../../../../app/data/ASCII/custdata.txt");
    const CARDXREF: &[u8] = include_bytes!("../../../../../app/data/ASCII/cardxref.txt");

    /// The literal of COACTVWC-031, with its two spaces after `must`.
    const FILTER_MESSAGE: &str = "Account Filter must  be a non-zero 11 digit number";

    /// The synthetic account key of the hand-built records.
    const ACCT: u64 = 1_234_567_890;

    /// The synthetic customer key and SSN of the hand-built customer; neither is a fixture value.
    const CUST: u64 = 1_234;
    const SSN: u64 = 987_654_321;

    /// A synthetic card number, never a fixture one.
    const CARD: [u8; 16] = *b"4000000000001234";

    fn money(cents: i64) -> Result<Money<10>, Box<dyn std::error::Error>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, Box<dyn std::error::Error>> {
        Ok(NaiveDate::from_ymd_opt(year, month, day).ok_or("not a calendar date")?)
    }

    fn text<const N: usize>(value: &str) -> Result<PicX<N>, Box<dyn std::error::Error>> {
        Ok(PicX::from_text(value)?)
    }

    fn digits<const N: u32>(key: PicU<N>) -> String {
        let mut out = Vec::new();
        key.encode_into(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    /// An account whose amounts are the worked examples of COACTVWC-011 and whose reissue date
    /// is blank.
    fn account() -> Result<AccountRecord, Box<dyn std::error::Error>> {
        Ok(AccountRecord {
            id: PicU::new(ACCT)?,
            active_status: text("Y")?,
            curr_bal: money(123_456)?,
            credit_limit: money(-5_000)?,
            cash_credit_limit: money(0)?,
            open_date: PicDate::new(Some(date(2014, 11, 20)?))?,
            expiraion_date: PicDate::new(Some(date(2025, 5, 20)?))?,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: money(123_456_789_012)?,
            curr_cyc_debit: money(19_400)?,
            addr_zip: text("A000000000")?,
            group_id: text("GRP0000001")?,
            filler: PicX::spaces(),
        })
    }

    /// A customer whose ZIP code and phone numbers fill their fields, so that the narrower map
    /// fields drop characters (COACTVWC-023, COACTVWC-024).
    fn customer() -> Result<CustomerRecord, Box<dyn std::error::Error>> {
        Ok(CustomerRecord {
            id: PicU::new(CUST)?,
            first_name: text("Ana")?,
            middle_name: text("Maria")?,
            last_name: text("Lopez")?,
            addr_line_1: text("1 Main Street")?,
            addr_line_2: text("Apt 2")?,
            addr_line_3: text("Springfield")?,
            addr_state_cd: text("NY")?,
            addr_country_cd: text("USA")?,
            addr_zip: text("12345-6789")?,
            phone_num_1: text("(212)555-010099")?,
            phone_num_2: text("(212)555-020088")?,
            ssn: Ssn::new(PicU::new(SSN)?),
            govt_issued_id: text("GOVT-ID-0001")?,
            dob_yyyy_mm_dd: PicDate::new(Some(date(1980, 1, 31)?))?,
            eft_account_id: text("0000000001")?,
            pri_card_holder_ind: text("Y")?,
            fico_credit_score: PicU::new(75)?,
            filler: PicX::spaces(),
        })
    }

    fn xref() -> Result<CardXrefRecord, Box<dyn std::error::Error>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(CUST)?,
            acct_id: PicU::new(ACCT)?,
            filler: PicX::spaces(),
        })
    }

    fn key() -> Result<PicU<11>, Box<dyn std::error::Error>> {
        Ok(PicU::new(ACCT)?)
    }

    /// The failure `outcome` holds, or an error naming the case.
    fn failure(outcome: Result<AccountView, ViewFailure>) -> Result<ViewFailure, &'static str> {
        match outcome {
            Ok(_) => Err("expected a failure, got a view"),
            Err(failure) => Ok(failure),
        }
    }

    /// Whether `rendered` shows the synthetic SSN or card number in any form.
    fn shows_protected(rendered: &str) -> bool {
        ["987654321", "987-65-4321", "4000000000001234"]
            .iter()
            .any(|value| rendered.contains(value))
    }

    // audit: COACTVWC-029
    // audit: COACTVWC-030
    #[test]
    fn values_not_supplied_get_no_input_received() {
        let not_supplied = [
            "",
            " ",
            "           ",
            "*",
            "*          ",
            "\0\0\0\0\0\0\0\0\0\0\0",
        ];
        for raw in not_supplied {
            let outcome = edit_account(raw);
            let Err(failure) = outcome else {
                panic!("a value not supplied passed the edit");
            };
            assert_eq!(failure.message(), "No input received");
            assert_eq!(failure.field(), Some(ViewField::AcctId));
            assert_eq!(failure.class(), FailureClass::Input);
            assert_eq!(failure.class().http_status(), 400);
            assert!(failure.account().is_none() && failure.customer().is_none());
        }
    }

    // audit: COACTVWC-029
    // audit: COACTVWC-031
    #[test]
    fn values_that_are_not_a_non_zero_eleven_digit_number_fail_the_numeric_edit() {
        let rejected = [
            "1",
            "1234567890",
            "0000000000a",
            "00000000000",
            " 0000000001",
            "0000000001 ",
            "+0000000001",
            "0000000001-",
            "00000000.01",
            "*0000000001",
            "**",
            "\0",
            "\0\0\0\0\0\0\0\0\0\0 ",
            "0000000000\u{ff11}",
            "000000000001",
            "00000000001          ",
        ];
        for raw in rejected {
            let outcome = edit_account(raw);
            let Err(failure) = outcome else {
                panic!("a value that is not eleven digits passed the edit");
            };
            assert_eq!(failure.message(), FILTER_MESSAGE);
            assert!(failure.message().contains("must  be"));
            assert_eq!(failure.field(), Some(ViewField::AcctId));
            assert_eq!(failure.class().http_status(), 400);
            assert!(failure.account().is_none() && failure.customer().is_none());
        }
    }

    // audit: COACTVWC-032
    #[test]
    fn eleven_digits_not_all_zero_are_the_key() -> TestResult {
        for (raw, value) in [
            ("00000000001", 1),
            ("01234567890", ACCT),
            ("99999999999", 99_999_999_999),
        ] {
            let key = edit_account(raw)?;
            assert_eq!(key.get(), value);
            assert_eq!(digits(key), raw);
        }
        Ok(())
    }

    // audit: COACTVWC-008
    // audit: COACTVWC-010
    // audit: COACTVWC-011
    // audit: COACTVWC-012
    // audit: COACTVWC-013
    // audit: COACTVWC-014
    // audit: COACTVWC-015
    // audit: COACTVWC-033
    // audit: COACTVWC-034
    #[test]
    fn three_found_reads_give_the_account_block() -> TestResult {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        let shown = view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )?;
        assert_eq!(shown.acct_id, key()?);
        let block = &shown.account;
        assert_eq!(block.active_status.as_bytes(), b"Y");
        assert_eq!(block.current_balance, "+      1,234.56");
        assert_eq!(block.credit_limit, "-         50.00");
        assert_eq!(block.cash_credit_limit, "+           .00");
        // The tenth integer digit has no position in the picture (X-COACTVWC-5).
        assert_eq!(block.current_cycle_credit, "+234,567,890.12");
        assert_eq!(block.current_cycle_debit, "+        194.00");
        assert_eq!(block.open_date, Some(date(2014, 11, 20)?));
        assert_eq!(block.expiration_date, Some(date(2025, 5, 20)?));
        assert_eq!(block.reissue_date, None);
        assert_eq!(block.group_id.as_bytes(), b"GRP0000001");
        // No card number is part of the view, in full or redacted (COACTVWC-034).
        let rendered = format!("{shown:?}");
        assert!(!shows_protected(&rendered), "COACTVWC-034");
        assert!(!rendered.contains("************"), "COACTVWC-034");
        Ok(())
    }

    // audit: COACTVWC-016
    // audit: COACTVWC-017
    // audit: COACTVWC-018
    // audit: COACTVWC-019
    // audit: COACTVWC-020
    // audit: COACTVWC-021
    // audit: COACTVWC-022
    // audit: COACTVWC-023
    // audit: COACTVWC-024
    // audit: COACTVWC-025
    // audit: COACTVWC-026
    #[test]
    fn three_found_reads_give_the_customer_block() -> TestResult {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        let shown = view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )?;
        let block = &shown.customer;
        assert_eq!(digits(block.customer_id), "000001234");
        assert_eq!(block.ssn.to_string(), "***-**-4321");
        assert!(!shows_protected(&format!("{shown:?}")), "COACTVWC-017");
        assert_eq!(digits(block.fico_score), "075");
        assert_eq!(block.date_of_birth, Some(date(1980, 1, 31)?));
        assert_eq!(block.first_name, customer.first_name);
        assert_eq!(block.middle_name, customer.middle_name);
        assert_eq!(block.last_name, customer.last_name);
        assert_eq!(block.address_line_1, customer.addr_line_1);
        assert_eq!(block.address_line_2, customer.addr_line_2);
        assert_eq!(block.city.to_text(), "Springfield");
        assert_eq!(block.state.as_bytes(), b"NY");
        assert_eq!(block.country.as_bytes(), b"USA");
        assert_eq!(block.zip.as_bytes(), b"12345");
        assert_eq!(block.phone_1.as_bytes(), b"(212)555-0100");
        assert_eq!(block.phone_2.as_bytes(), b"(212)555-0200");
        assert_eq!(block.government_id, customer.govt_issued_id);
        assert_eq!(block.eft_account_id.as_bytes(), b"0000000001");
        assert_eq!(block.primary_card_holder.as_bytes(), b"Y");
        Ok(())
    }

    // audit: COACTVWC-033
    // audit: COACTVWC-035
    #[test]
    fn a_cross_reference_not_found_stops_the_chain_with_its_message() -> TestResult {
        let (account, customer) = (account()?, customer()?);
        // The account and customer outcomes are never examined once the cross-reference fails.
        let failure = failure(view(
            key()?,
            Lookup::NotFound,
            Lookup::Found(&account),
            Lookup::Found(&customer),
        ))?;
        // HALTED(H4): positions 56-65 and 72-75 hold the CICS codes (COACTVWC-036); they are
        // BLOCKED and not asserted, only the text around them.
        let message = failure.message();
        assert_eq!(
            message.get(..55),
            Some("Account:01234567890 not found in Cross ref file.  Resp:")
        );
        assert_eq!(message.get(65..71), Some(" Reas:"));
        assert_eq!(failure.class(), FailureClass::NotFound);
        assert_eq!(failure.class().http_status(), 404);
        assert_eq!(failure.field(), Some(ViewField::AcctId));
        assert!(failure.account().is_none() && failure.customer().is_none());
        Ok(())
    }

    // audit: COACTVWC-037
    // audit: COACTVWC-039
    #[test]
    fn a_failed_cross_reference_read_gives_the_cxacaix_file_error() -> TestResult {
        let failure = failure(view(
            key()?,
            Lookup::Error,
            Lookup::NotFound,
            Lookup::NotFound,
        ))?;
        // HALTED(H4): positions 49-58 and 66-75 hold the CICS codes (COACTVWC-038); BLOCKED.
        let message = failure.message();
        assert_eq!(
            message.get(..48),
            Some("File Error: READ     on CXACAIX   returned RESP ")
        );
        assert_eq!(message.get(58..65), Some(",RESP2 "));
        assert_eq!(failure.class(), FailureClass::Store);
        assert_eq!(failure.class().http_status(), 500);
        assert_eq!(failure.field(), Some(ViewField::AcctId));
        assert!(failure.account().is_none() && failure.customer().is_none());
        Ok(())
    }

    // audit: COACTVWC-008
    // audit: COACTVWC-009
    // audit: COACTVWC-033
    // audit: COACTVWC-040
    // audit: COACTVWC-046
    #[test]
    fn an_account_not_found_keeps_its_message_unless_the_customer_read_fails() -> TestResult {
        let (customer, xref) = (customer()?, xref()?);
        let head = Some("Account:01234567890 not found in Acct Master file.Resp:");
        // HALTED(H4): the codes at positions 56-65 and 72-75 (COACTVWC-041) are not asserted.
        let found = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::NotFound,
            Lookup::Found(&customer),
        ))?;
        assert_eq!(found.message().get(..55), head);
        assert_eq!(found.message().get(65..71), Some(" Reas:"));
        assert_eq!(found.class().http_status(), 404);
        assert_eq!(found.field(), Some(ViewField::AcctId));
        // HALTED(X-COACTVWC-H1): the account block is present; its values are not asserted.
        assert!(found.account().is_some());
        assert_eq!(
            found.customer(),
            Some(&CustomerBlock::from_record(&customer))
        );

        let neither = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::NotFound,
            Lookup::NotFound,
        ))?;
        assert_eq!(neither.message().get(..55), head);
        assert_eq!(neither.class(), FailureClass::NotFound);
        assert_eq!(neither.field(), Some(ViewField::AcctId));
        assert!(neither.account().is_none() && neither.customer().is_none());

        let replaced = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::NotFound,
            Lookup::Error,
        ))?;
        assert_eq!(
            replaced.message().get(..48),
            Some("File Error: READ     on CUSTDAT   returned RESP ")
        );
        assert_eq!(replaced.class().http_status(), 500);
        assert_eq!(replaced.field(), Some(ViewField::AcctId));
        assert!(replaced.account().is_none() && replaced.customer().is_none());
        Ok(())
    }

    // audit: COACTVWC-008
    // audit: COACTVWC-009
    // audit: COACTVWC-042
    // audit: COACTVWC-046
    #[test]
    fn a_failed_account_read_gives_the_acctdat_file_error() -> TestResult {
        let (customer, xref) = (customer()?, xref()?);
        let head = Some("File Error: READ     on ACCTDAT   returned RESP ");
        // HALTED(H4): the codes at positions 49-58 and 66-75 (COACTVWC-043) are not asserted.
        let found = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Error,
            Lookup::Found(&customer),
        ))?;
        assert_eq!(found.message().get(..48), head);
        assert_eq!(found.message().get(58..65), Some(",RESP2 "));
        assert_eq!(found.class(), FailureClass::Store);
        assert_eq!(found.field(), Some(ViewField::AcctId));
        // HALTED(X-COACTVWC-H1): only the presence of the account block is asserted.
        assert!(found.account().is_some());
        assert_eq!(
            found.customer(),
            Some(&CustomerBlock::from_record(&customer))
        );

        let neither = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Error,
            Lookup::NotFound,
        ))?;
        assert_eq!(neither.message().get(..48), head);
        assert_eq!(neither.class().http_status(), 500);
        assert!(neither.account().is_none() && neither.customer().is_none());

        let replaced = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Error,
            Lookup::Error,
        ))?;
        assert_eq!(
            replaced.message().get(..48),
            Some("File Error: READ     on CUSTDAT   returned RESP ")
        );
        assert_eq!(replaced.field(), Some(ViewField::AcctId));
        Ok(())
    }

    // audit: COACTVWC-008
    // audit: COACTVWC-044
    #[test]
    fn a_customer_not_found_after_the_account_shows_the_account_block() -> TestResult {
        let (account, xref) = (account()?, xref()?);
        let failure = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::NotFound,
        ))?;
        // HALTED(H4): the codes at positions 53-62 and 69-75 (COACTVWC-045) are not asserted.
        let message = failure.message();
        assert_eq!(
            message.get(..52),
            Some("CustId:000001234 not found in customer master.Resp: ")
        );
        assert_eq!(message.get(62..68), Some(" REAS:"));
        assert_eq!(failure.class(), FailureClass::NotFound);
        assert_eq!(failure.class().http_status(), 404);
        assert_eq!(failure.field(), None);
        assert_eq!(
            failure.account(),
            Some(&AccountBlock::from_record(&account))
        );
        assert!(failure.customer().is_none());
        Ok(())
    }

    // audit: COACTVWC-008
    // audit: COACTVWC-046
    #[test]
    fn a_failed_customer_read_after_the_account_has_no_field() -> TestResult {
        let (account, xref) = (account()?, xref()?);
        let failure = failure(view(
            key()?,
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Error,
        ))?;
        // HALTED(H4): the codes at positions 49-58 and 66-75 (COACTVWC-047) are not asserted.
        assert_eq!(
            failure.message().get(..48),
            Some("File Error: READ     on CUSTDAT   returned RESP ")
        );
        assert_eq!(failure.message().get(58..65), Some(",RESP2 "));
        assert_eq!(failure.class(), FailureClass::Store);
        assert_eq!(failure.field(), None);
        assert_eq!(
            failure.account(),
            Some(&AccountBlock::from_record(&account))
        );
        assert!(failure.customer().is_none());
        Ok(())
    }

    #[test]
    fn classes_map_to_their_statuses_and_failures_display_their_message() -> TestResult {
        assert_eq!(FailureClass::Input.http_status(), 400);
        assert_eq!(FailureClass::NotFound.http_status(), 404);
        assert_eq!(FailureClass::Store.http_status(), 500);
        assert_eq!(ViewField::AcctId.name(), "acct_id");
        let failure = failure(view(
            key()?,
            Lookup::NotFound,
            Lookup::NotFound,
            Lookup::NotFound,
        ))?;
        assert_eq!(failure.to_string(), failure.message());
        let as_error: &dyn std::error::Error = &failure;
        assert!(as_error.source().is_none());
        Ok(())
    }

    // audit: COACTVWC-017
    // audit: COACTVWC-034
    #[test]
    fn no_outcome_renders_the_ssn_or_the_card_number() -> TestResult {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        let links = [Lookup::Found(()), Lookup::NotFound, Lookup::Error];
        for xref_link in links {
            for account_link in links {
                for customer_link in links {
                    let outcome = view(
                        key()?,
                        pick(xref_link, &xref),
                        pick(account_link, &account),
                        pick(customer_link, &customer),
                    );
                    let rendered = match &outcome {
                        Ok(shown) => format!("{shown:?}"),
                        Err(failure) => format!("{failure:?} {failure}"),
                    };
                    assert!(!shows_protected(&rendered), "COACTVWC-017");
                }
            }
        }
        Ok(())
    }

    /// `link` with `record` in place of its unit payload.
    fn pick<T>(link: Lookup<()>, record: &T) -> Lookup<&T> {
        match link {
            Lookup::Found(()) => Lookup::Found(record),
            Lookup::NotFound => Lookup::NotFound,
            Lookup::Error => Lookup::Error,
        }
    }

    // audit: COACTVWC-014
    // audit: COACTVWC-017
    // audit: COACTVWC-019
    // audit: COACTVWC-023
    // audit: COACTVWC-024
    #[test]
    fn every_fixture_cross_reference_views_its_account_and_customer() -> TestResult {
        let accounts = decode_all::<AccountRecord>(ACCTDATA)?;
        let customers = decode_all::<CustomerRecord>(CUSTDATA)?;
        let xrefs = decode_all::<CardXrefRecord>(CARDXREF)?;
        assert_eq!(xrefs.len(), 50);
        let mut cut_zips = 0_usize;
        for framed in &xrefs {
            let xref = &framed.record;
            let account = accounts
                .iter()
                .map(|framed| &framed.record)
                .find(|account| account.id == xref.acct_id)
                .ok_or("cross-reference without its account")?;
            let customer = customers
                .iter()
                .map(|framed| &framed.record)
                .find(|customer| customer.id == xref.cust_id)
                .ok_or("cross-reference without its customer")?;
            let key = edit_account(&digits(xref.acct_id))?;
            let shown = view(
                key,
                Lookup::Found(xref),
                Lookup::Found(account),
                Lookup::Found(customer),
            )?;
            let amounts = [
                &shown.account.current_balance,
                &shown.account.credit_limit,
                &shown.account.cash_credit_limit,
                &shown.account.current_cycle_credit,
                &shown.account.current_cycle_debit,
            ];
            assert!(amounts.iter().all(|amount| amount.len() == 15));
            assert!(shown.account.open_date.is_some());
            assert!(shown.account.expiration_date.is_some());
            assert!(shown.account.reissue_date.is_some());
            assert!(shown.customer.date_of_birth.is_some());
            let mut redacted = String::from("***-**-");
            redacted.push_str(&String::from_utf8_lossy(&customer.ssn.last4()));
            assert!(shown.customer.ssn.to_string() == redacted, "COACTVWC-017");
            let rendered = format!("{shown:?}");
            assert!(
                !rendered.contains(&digits(*customer.ssn.expose())),
                "COACTVWC-017"
            );
            assert!(!rendered.contains(&customer.ssn.expose().get().to_string()));
            let zip = customer.addr_zip.as_bytes();
            assert_eq!(shown.customer.zip, PicX::<5>::moved_from(zip));
            if customer.addr_zip.trim_end().len() > 5 {
                cut_zips = cut_zips.saturating_add(1);
            }
            let phone = customer.phone_num_1.as_bytes();
            assert_eq!(shown.customer.phone_1, PicX::<13>::moved_from(phone));
        }
        // COACTVWC-023: 30 of the 50 ZIP codes are longer than the five characters shown.
        assert_eq!(cut_zips, 30);
        Ok(())
    }
}
