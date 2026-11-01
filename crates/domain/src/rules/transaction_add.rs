//! `COTRN02C` (transaction CT02, map `COTRN02`): add a transaction to the live
//! transaction master, `POST /transactions`.
//!
//! The program runs as one `PROCESS-ENTER-KEY` [app/cbl/COTRN02C.cbl:L164-188]
//! that the replacement splits at the TRAN-ID allocation:
//!
//! 1. [`validate`]: `VALIDATE-INPUT-KEY-FIELDS` with its `CXACAIX` or `CCXREF`
//!    read, `VALIDATE-INPUT-DATA-FIELDS` with the `CSUTLDTC` date checks, and
//!    the confirmation (P-COTRN02C-004).
//! 2. The caller allocates the TRAN-ID: `STARTBR` at `HIGH-VALUES`, `READPREV`,
//!    `ENDBR` and `ADD 1` (P-COTRN02C-020) are the shared, serialized allocation
//!    of `crates/persistence/src/ids.rs`. [`allocated_tran_id`] turns its
//!    failures into the `STARTBR` and `READPREV` messages (V-COTRN02C-029 to
//!    V-COTRN02C-031); an add against an empty live master is not found (E-27).
//! 3. [`build`] fills `TRAN-RECORD` (P-COTRN02C-023); the caller writes it and
//!    [`write_result`] answers the write as `WRITE-TRANSACT-FILE` does
//!    (P-COTRN02C-024, P-COTRN02C-025).
//!
//! The caller runs all of it, and the reward accrual of the new transaction,
//! in one database transaction; this module never reads the clock, writes
//! nothing and computes no reward.
//!
//! # The first message ends the request
//!
//! `SEND-TRNADD-SCREEN` ends with `EXEC CICS RETURN`
//! [app/cbl/COTRN02C.cbl:L516-534] (P-COTRN02C-003), so the first failing check
//! ends the task: later checks and the confirmation never run. Every failure
//! sets `WS-ERR-FLG`, the `N` and blank confirmations included, so each one is
//! a [`DomainError`] on the BMS field the COBOL puts the cursor on
//! (`MOVE -1 TO <field>L`). There is no message-only outcome.
//!
//! # Receiving the map
//!
//! Each entered text is moved into its `COTRN2AI` field as `RECEIVE MAP` fills
//! it [app/cpy-bms/COTRN02.CPY]: left-justified and padded with spaces. A
//! text made only of NUL characters is `LOW-VALUES`, as for the other rules'
//! fields ([`Confirm::parse`]). The map fields hold no more characters than
//! their width, so the COBOL never receives a longer entry. For the fields
//! the COBOL tests by class or format (account ID, card number, type and
//! category codes, amount, dates, merchant ID) an entry with a non-space
//! character past the field width fails that test, because cutting it would
//! silently change the key, code, amount or date the entry names. The free
//! text fields (source, description, merchant name, city and zip) are cut at
//! their width, as the `MOVE` cuts them.
//!
//! # Halt classes
//!
//! A date whose `CEEDAYS` feedback no reviewed row pins (year `0000`, or a date
//! outside 1582-10-15 to 9999-12-31 that is not a calendar date: E-31, halt)
//! is answered with the unexpected-error text on its field, never with a
//! guessed accept or reject. The `FUNCTION NUMVAL` and `NUMVAL-C` calls see
//! only entries that passed their numeric or format edit, whose values are
//! exact (P-COTRN02C-027, P-COTRN02C-028), so no numeric class is unproven.
//!
//! # Screen mechanics
//!
//! PF3 and the invalid-key text (P-COTRN02C-002, P-COTRN02C-031, E-56), PF4
//! Clear (P-COTRN02C-035, P-COTRN02C-036) and the screen header
//! (P-COTRN02C-033) become SPA navigation and form controls. PF5 copies the
//! highest-keyed transaction of the whole file into the form
//! (P-COTRN02C-026): the SPA reads that record with
//! `GET /transactions?direction=backward&page_size=1`, and [`copied_fields`]
//! is the field mapping `COPY-LAST-TRAN-DATA` performs.

use super::Confirm;
use super::date_validation::{self, DatePicture, DateValidationError};
use crate::entities::{CardXref, NewTransaction, Transaction};
use crate::error::DomainError;
use crate::format::{PIC_TRAN_AMT_DISPLAY, edit_money};
use crate::messages::{
    ACCOUNT_ID_MUST_BE_NUMERIC, ACCOUNT_ID_NOT_FOUND, ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED,
    AMOUNT_CAN_NOT_BE_EMPTY, AMOUNT_SHOULD_BE_IN_FORMAT, CARD_NUMBER_MUST_BE_NUMERIC,
    CARD_NUMBER_NOT_FOUND, CATEGORY_CD_CAN_NOT_BE_EMPTY, CATEGORY_CD_MUST_BE_NUMERIC,
    CONFIRM_TO_ADD_THIS_TRANSACTION, DESCRIPTION_CAN_NOT_BE_EMPTY,
    INVALID_VALUE_VALID_VALUES_ARE_Y_N, MERCHANT_CITY_CAN_NOT_BE_EMPTY,
    MERCHANT_ID_CAN_NOT_BE_EMPTY, MERCHANT_ID_MUST_BE_NUMERIC, MERCHANT_NAME_CAN_NOT_BE_EMPTY,
    MERCHANT_ZIP_CAN_NOT_BE_EMPTY, ORIG_DATE_CAN_NOT_BE_EMPTY, ORIG_DATE_NOT_A_VALID_DATE,
    ORIG_DATE_SHOULD_BE_IN_FORMAT, PROC_DATE_CAN_NOT_BE_EMPTY, PROC_DATE_NOT_A_VALID_DATE,
    PROC_DATE_SHOULD_BE_IN_FORMAT, SOURCE_CAN_NOT_BE_EMPTY, TRAN_ID_ALREADY_EXIST,
    TRANSACTION_ID_NOT_FOUND, TYPE_CD_CAN_NOT_BE_EMPTY, TYPE_CD_MUST_BE_NUMERIC,
    UNABLE_TO_ADD_TRANSACTION, UNABLE_TO_LOOKUP_ACCT_IN_XREF_AIX_FILE,
    UNABLE_TO_LOOKUP_CARD_IN_XREF_FILE, UNABLE_TO_LOOKUP_TRANSACTION, UNEXPECTED_ERROR,
    transaction_added_message,
};
use crate::numeric::{
    AccountId, Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
};
use crate::ports::{PortError, PortResult, XrefReader};

/// The BMS field `ACTIDIN` (`ACTIDINI X(11)`): the account ID, and the cursor
/// field of the account, allocation and write messages (V-COTRN02C-001,
/// V-COTRN02C-003 to V-COTRN02C-005, V-COTRN02C-029 to V-COTRN02C-033).
pub const FIELD_ACTIDIN: &str = "ACTIDIN";

/// The BMS field `CARDNIN` (`CARDNINI X(16)`): the card number
/// (V-COTRN02C-002, V-COTRN02C-006, V-COTRN02C-007).
pub const FIELD_CARDNIN: &str = "CARDNIN";

/// The BMS field `TTYPCD` (`TTYPCDI X(2)`): the type code (V-COTRN02C-008,
/// V-COTRN02C-019).
pub const FIELD_TTYPCD: &str = "TTYPCD";

/// The BMS field `TCATCD` (`TCATCDI X(4)`): the category code
/// (V-COTRN02C-009, V-COTRN02C-020).
pub const FIELD_TCATCD: &str = "TCATCD";

/// The BMS field `TRNSRC` (`TRNSRCI X(10)`): the source (V-COTRN02C-010).
pub const FIELD_TRNSRC: &str = "TRNSRC";

/// The BMS field `TDESC` (`TDESCI X(60)`): the description (V-COTRN02C-011).
pub const FIELD_TDESC: &str = "TDESC";

/// The BMS field `TRNAMT` (`TRNAMTI X(12)`): the amount (V-COTRN02C-012,
/// V-COTRN02C-021).
pub const FIELD_TRNAMT: &str = "TRNAMT";

/// The BMS field `TORIGDT` (`TORIGDTI X(10)`): the origination date
/// (V-COTRN02C-013, V-COTRN02C-022, V-COTRN02C-024).
pub const FIELD_TORIGDT: &str = "TORIGDT";

/// The BMS field `TPROCDT` (`TPROCDTI X(10)`): the processing date
/// (V-COTRN02C-014, V-COTRN02C-023, V-COTRN02C-025).
pub const FIELD_TPROCDT: &str = "TPROCDT";

/// The BMS field `MID` (`MIDI X(9)`): the merchant ID (V-COTRN02C-015,
/// V-COTRN02C-026).
pub const FIELD_MID: &str = "MID";

/// The BMS field `MNAME` (`MNAMEI X(30)`): the merchant name (V-COTRN02C-016).
pub const FIELD_MNAME: &str = "MNAME";

/// The BMS field `MCITY` (`MCITYI X(25)`): the merchant city (V-COTRN02C-017).
pub const FIELD_MCITY: &str = "MCITY";

/// The BMS field `MZIP` (`MZIPI X(10)`): the merchant zip (V-COTRN02C-018).
pub const FIELD_MZIP: &str = "MZIP";

/// The BMS field `CONFIRM` (`CONFIRMI X(1)`): the confirmation
/// (V-COTRN02C-027, V-COTRN02C-028).
pub const FIELD_CONFIRM: &str = "CONFIRM";

/// `ACTIDINI PIC X(11)` [app/cpy-bms/COTRN02.CPY:L60].
const ACCOUNT_ID_LEN: usize = 11;

/// `CARDNINI PIC X(16)` [app/cpy-bms/COTRN02.CPY:L66].
const CARD_NUMBER_LEN: usize = 16;

/// `TTYPCDI PIC X(2)` [app/cpy-bms/COTRN02.CPY:L72].
const TYPE_CD_LEN: usize = 2;

/// `TCATCDI PIC X(4)` [app/cpy-bms/COTRN02.CPY:L78].
const CAT_CD_LEN: usize = 4;

/// `TRNSRCI PIC X(10)` [app/cpy-bms/COTRN02.CPY:L84].
const SOURCE_LEN: usize = 10;

/// `TDESCI PIC X(60)` [app/cpy-bms/COTRN02.CPY:L90].
const DESCRIPTION_LEN: usize = 60;

/// `TRNAMTI PIC X(12)` [app/cpy-bms/COTRN02.CPY:L96].
const AMOUNT_LEN: usize = 12;

/// `TORIGDTI` and `TPROCDTI PIC X(10)` [app/cpy-bms/COTRN02.CPY:L102]
/// [app/cpy-bms/COTRN02.CPY:L108], passed whole to `CSUTLDTC-DATE PIC X(10)`.
const DATE_LEN: usize = 10;

/// `MIDI PIC X(9)` [app/cpy-bms/COTRN02.CPY:L114].
const MERCHANT_ID_LEN: usize = 9;

/// `MNAMEI PIC X(30)` [app/cpy-bms/COTRN02.CPY:L120].
const MERCHANT_NAME_LEN: usize = 30;

/// `MCITYI PIC X(25)` [app/cpy-bms/COTRN02.CPY:L126].
const MERCHANT_CITY_LEN: usize = 25;

/// `MZIPI PIC X(10)` [app/cpy-bms/COTRN02.CPY:L132].
const MERCHANT_ZIP_LEN: usize = 10;

/// The `(start:length)` reference modifications that must hold digits in a
/// `YYYY-MM-DD` date [app/cbl/COTRN02C.cbl:L354-358].
const DATE_DIGIT_PARTS: [(usize, usize); 3] = [(1, 4), (6, 2), (9, 2)];

/// The positions that must hold `-` in a `YYYY-MM-DD` date
/// [app/cbl/COTRN02C.cbl:L355] [app/cbl/COTRN02C.cbl:L357].
const DATE_DASH_POSITIONS: [usize; 2] = [5, 8];

/// The `COTRN2A` screen input of one ENTER, each field as entered.
///
/// `POST /transactions` carries one field per map input field. The account ID
/// is used when entered, otherwise the card number; the other is taken from
/// the cross-reference. Absent and empty texts are both blank.
#[derive(Clone, PartialEq, Eq)]
pub struct TransactionAddInput {
    /// `ACTIDIN`: the account ID, `None` when not sent.
    pub account_id: Option<String>,
    /// `CARDNIN`: the card number, `None` when not sent.
    pub card_number: Option<String>,
    /// `TTYPCD`: the transaction type code.
    pub type_cd: String,
    /// `TCATCD`: the transaction category code.
    pub cat_cd: String,
    /// `TRNSRC`: the source.
    pub source: String,
    /// `TDESC`: the description.
    pub description: String,
    /// `TRNAMT`: the amount as text in the form `-99999999.99`.
    pub amount: String,
    /// `TORIGDT`: the origination date as text in the form `YYYY-MM-DD`.
    pub orig_date: String,
    /// `TPROCDT`: the processing date as text in the form `YYYY-MM-DD`.
    pub proc_date: String,
    /// `MID`: the merchant ID.
    pub merchant_id: String,
    /// `MNAME`: the merchant name.
    pub merchant_name: String,
    /// `MCITY`: the merchant city.
    pub merchant_city: String,
    /// `MZIP`: the merchant zip.
    pub merchant_zip: String,
    /// `CONFIRM`: `Y` adds the transaction, `N` or blank asks for confirmation.
    pub confirm: Confirm,
}

/// The entered card number is shown as the `CARDNIN` field would hold it,
/// masked by [`CardNumber`]'s `Debug`, so diagnostics never carry a full card
/// number.
impl std::fmt::Debug for TransactionAddInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransactionAddInput")
            .field("account_id", &self.account_id)
            .field(
                "card_number",
                &self.card_number.as_deref().map(CardNumber::move_from),
            )
            .field("type_cd", &self.type_cd)
            .field("cat_cd", &self.cat_cd)
            .field("source", &self.source)
            .field("description", &self.description)
            .field("amount", &self.amount)
            .field("orig_date", &self.orig_date)
            .field("proc_date", &self.proc_date)
            .field("merchant_id", &self.merchant_id)
            .field("merchant_name", &self.merchant_name)
            .field("merchant_city", &self.merchant_city)
            .field("merchant_zip", &self.merchant_zip)
            .field("confirm", &self.confirm)
            .finish()
    }
}

/// A transaction whose key fields, data fields and confirmation passed every
/// `COTRN02C` edit: the `COTRN2AI` fields `ADD-TRANSACTION` moves into
/// `TRAN-RECORD`, with the cross-reference the key edit read.
///
/// Fields whose map width equals their record width hold their record type;
/// the others keep their map width, and [`build`] performs the `MOVE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTransaction {
    /// `CARD-XREF-RECORD` read through `CXACAIX` by account or `CCXREF` by card
    /// (P-COTRN02C-008, P-COTRN02C-009): its account is the one the reward
    /// accrual credits.
    pub xref: CardXref,
    /// `CARDNINI` after the key edit: the cross-reference's card on the account
    /// path, the entered card number on the card path (P-COTRN02C-005,
    /// P-COTRN02C-006).
    pub card_number: CardNumber,
    /// `TTYPCDI X(2)`, two digits (P-COTRN02C-011).
    pub type_cd: TranTypeCode,
    /// `TCATCDI X(4)` moved to `TRAN-CAT-CD 9(04)`, four digits
    /// (P-COTRN02C-011).
    pub cat_cd: TranCategoryCode,
    /// `TRNSRCI X(10)`.
    pub source: Alnum<SOURCE_LEN>,
    /// `TDESCI X(60)`.
    pub description: Alnum<DESCRIPTION_LEN>,
    /// `WS-TRAN-AMT-N S9(9)V99`, the exact `NUMVAL-C` of `TRNAMTI`
    /// (P-COTRN02C-014, P-COTRN02C-028).
    pub amount: Money<9>,
    /// `TORIGDTI X(10)`, a `YYYY-MM-DD` date `CSUTLDTC` accepts
    /// (P-COTRN02C-013, P-COTRN02C-015).
    pub orig_date: Alnum<DATE_LEN>,
    /// `TPROCDTI X(10)`, a `YYYY-MM-DD` date `CSUTLDTC` accepts
    /// (P-COTRN02C-013, P-COTRN02C-015).
    pub proc_date: Alnum<DATE_LEN>,
    /// `MIDI X(9)` moved to `TRAN-MERCHANT-ID 9(09)`, nine digits
    /// (P-COTRN02C-016).
    pub merchant_id: MerchantId,
    /// `MNAMEI X(30)`.
    pub merchant_name: Alnum<MERCHANT_NAME_LEN>,
    /// `MCITYI X(25)`.
    pub merchant_city: Alnum<MERCHANT_CITY_LEN>,
    /// `MZIPI X(10)`.
    pub merchant_zip: Alnum<MERCHANT_ZIP_LEN>,
}

impl ValidatedTransaction {
    /// `XREF-ACCT-ID`, the account the transaction belongs to: the entered
    /// account on the account path, the cross-reference's on the card path.
    #[must_use]
    pub const fn account_id(&self) -> AccountId {
        self.xref.xref_acct_id
    }
}

/// One `COTRN2AI` input field `PIC X(N)` as `RECEIVE MAP` fills it from the
/// entered text.
#[derive(Clone, Copy)]
struct Received<const N: usize> {
    /// The field: the text moved in, or `LOW-VALUES` for a text made only of
    /// NUL characters.
    field: Alnum<N>,
    /// Whether the text has a non-space character past byte `N`, which the
    /// move would drop.
    overlong: bool,
}

impl<const N: usize> Received<N> {
    /// The field `text` fills.
    fn from_text(text: &str) -> Self {
        let bytes = text.as_bytes();
        let low_values = !bytes.is_empty() && bytes.iter().all(|&byte| byte == 0);
        let field = if low_values {
            Alnum::from_array([0; N])
        } else {
            Alnum::move_from(text)
        };
        let overlong = bytes
            .get(N..)
            .is_some_and(|rest| rest.iter().any(|&byte| byte != b' '));
        Self { field, overlong }
    }

    /// `= SPACES OR LOW-VALUES`.
    fn is_blank(&self) -> bool {
        self.field.is_blank()
    }

    /// The field, when it holds the whole entry.
    fn whole(&self) -> Option<Alnum<N>> {
        (!self.overlong).then_some(self.field)
    }

    /// The field, when it holds the whole entry and passes `IS NUMERIC`.
    fn numeric(&self) -> Option<Alnum<N>> {
        self.whole().filter(Alnum::is_numeric)
    }
}

/// The eleven data fields of `COTRN2AI`, in screen order.
struct DataFields {
    /// `TTYPCDI`.
    type_cd: Received<TYPE_CD_LEN>,
    /// `TCATCDI`.
    cat_cd: Received<CAT_CD_LEN>,
    /// `TRNSRCI`.
    source: Received<SOURCE_LEN>,
    /// `TDESCI`.
    description: Received<DESCRIPTION_LEN>,
    /// `TRNAMTI`.
    amount: Received<AMOUNT_LEN>,
    /// `TORIGDTI`.
    orig_date: Received<DATE_LEN>,
    /// `TPROCDTI`.
    proc_date: Received<DATE_LEN>,
    /// `MIDI`.
    merchant_id: Received<MERCHANT_ID_LEN>,
    /// `MNAMEI`.
    merchant_name: Received<MERCHANT_NAME_LEN>,
    /// `MCITYI`.
    merchant_city: Received<MERCHANT_CITY_LEN>,
    /// `MZIPI`.
    merchant_zip: Received<MERCHANT_ZIP_LEN>,
}

/// The whole `COTRN2AI` input of one ENTER.
struct ReceivedMap {
    /// `ACTIDINI`.
    account_id: Received<ACCOUNT_ID_LEN>,
    /// `CARDNINI`.
    card_number: Received<CARD_NUMBER_LEN>,
    /// The data fields.
    data: DataFields,
    /// `CONFIRMI`.
    confirm: Confirm,
}

impl TransactionAddInput {
    /// COTRN02C RECEIVE-TRNADD-SCREEN — P-COTRN02C-032: the map fields the
    /// entered texts fill.
    fn receive(self) -> ReceivedMap {
        ReceivedMap {
            account_id: Received::from_text(self.account_id.as_deref().unwrap_or_default()),
            card_number: Received::from_text(self.card_number.as_deref().unwrap_or_default()),
            data: DataFields {
                type_cd: Received::from_text(&self.type_cd),
                cat_cd: Received::from_text(&self.cat_cd),
                source: Received::from_text(&self.source),
                description: Received::from_text(&self.description),
                amount: Received::from_text(&self.amount),
                orig_date: Received::from_text(&self.orig_date),
                proc_date: Received::from_text(&self.proc_date),
                merchant_id: Received::from_text(&self.merchant_id),
                merchant_name: Received::from_text(&self.merchant_name),
                merchant_city: Received::from_text(&self.merchant_city),
                merchant_zip: Received::from_text(&self.merchant_zip),
            },
            confirm: self.confirm,
        }
    }
}

/// COTRN02C PROCESS-ENTER-KEY — P-COTRN02C-003, P-COTRN02C-004,
/// P-COTRN02C-017 to P-COTRN02C-019; V-COTRN02C-001 to V-COTRN02C-028.
///
/// The edits of one ENTER [app/cbl/COTRN02C.cbl:L164-188], in the COBOL order:
/// the key fields and their cross-reference read, the data fields, then the
/// confirmation. Only `Y` or `y` lets the add go on; nothing is written here.
///
/// # Errors
///
/// The [`DomainError`] of the first check that fails, which ends the request:
///
/// 1. Key fields (`VALIDATE-INPUT-KEY-FIELDS`): account ID not numeric
///    (V-COTRN02C-001), card number not numeric (V-COTRN02C-002), neither
///    entered (V-COTRN02C-003); cross-reference not found or not readable
///    (V-COTRN02C-004 to V-COTRN02C-007).
/// 2. Data fields (`VALIDATE-INPUT-DATA-FIELDS`): the first blank field in
///    screen order (V-COTRN02C-008 to V-COTRN02C-018), type or category code
///    not numeric (V-COTRN02C-019, V-COTRN02C-020), amount not in the form
///    `-99999999.99` (V-COTRN02C-021), origination or processing date not in
///    the form `YYYY-MM-DD` (V-COTRN02C-022, V-COTRN02C-023), either date
///    rejected by `CSUTLDTC` (V-COTRN02C-024, V-COTRN02C-025) or of an
///    unproven `CEEDAYS` class (unexpected, E-31), merchant ID not numeric
///    (V-COTRN02C-026).
/// 3. Confirmation: `N` or blank (V-COTRN02C-027), any other value but `Y`
///    (V-COTRN02C-028).
pub async fn validate<S: XrefReader>(
    store: &mut S,
    input: TransactionAddInput,
) -> Result<ValidatedTransaction, DomainError> {
    let map = input.receive();
    let (xref, card_number) = validate_key_fields(store, &map.account_id, &map.card_number).await?;
    let validated = validate_data_fields(&map.data, xref, card_number)?;
    confirm_add(&map.confirm)?;
    Ok(validated)
}

/// COTRN02C VALIDATE-INPUT-KEY-FIELDS — P-COTRN02C-005 to P-COTRN02C-007,
/// P-COTRN02C-027; V-COTRN02C-001 to V-COTRN02C-003.
///
/// The `EVALUATE TRUE` of [app/cbl/COTRN02C.cbl:L193-230]: an entered account
/// ID is tested first, then an entered card number.
///
/// - Account ID entered: it must be 11 digits; `FUNCTION NUMVAL` of the digits
///   is the account key (exact, P-COTRN02C-027), `READ-CXACAIX-FILE` reads its
///   first cross-reference, and the card number is taken from it. An entered
///   card number is then ignored.
/// - Card number entered: it must be 16 digits, which are the key
///   `READ-CCXREF-FILE` reads by; the account is taken from the
///   cross-reference.
/// - Neither entered: rejected.
///
/// # Errors
///
/// Invalid on `ACTIDIN` or `CARDNIN` for a non-numeric key, empty on
/// `ACTIDIN` when neither is entered, and the read failures of
/// [`read_cxacaix_file`] and [`read_ccxref_file`].
async fn validate_key_fields<S: XrefReader>(
    store: &mut S,
    account_id: &Received<ACCOUNT_ID_LEN>,
    card_number: &Received<CARD_NUMBER_LEN>,
) -> Result<(CardXref, CardNumber), DomainError> {
    if !account_id.is_blank() {
        let key = account_id
            .numeric()
            .and_then(|field| field.as_text().parse::<AccountId>().ok())
            .ok_or_else(|| {
                DomainError::invalid(ACCOUNT_ID_MUST_BE_NUMERIC).with_field(FIELD_ACTIDIN)
            })?;
        let xref = read_cxacaix_file(store, key).await?;
        let card = xref.xref_card_num;
        return Ok((xref, card));
    }
    if !card_number.is_blank() {
        let card = card_number
            .numeric()
            .map(CardNumber::from_alnum)
            .ok_or_else(|| {
                DomainError::invalid(CARD_NUMBER_MUST_BE_NUMERIC).with_field(FIELD_CARDNIN)
            })?;
        let xref = read_ccxref_file(store, card).await?;
        return Ok((xref, card));
    }
    Err(DomainError::empty(ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED).with_field(FIELD_ACTIDIN))
}

/// COTRN02C READ-CXACAIX-FILE — P-COTRN02C-008; V-COTRN02C-004,
/// V-COTRN02C-005.
///
/// `READ DATASET('CXACAIX')` by `XREF-ACCT-ID` [app/cbl/COTRN02C.cbl:L576-604]:
/// the first cross-reference of the account. The `DISPLAY` of `RESP` and
/// `RESP2` on the `WHEN OTHER` branch is a console log the adapter keeps.
///
/// # Errors
///
/// Not found with `Account ID NOT found...` on `NOTFND`, unexpected with
/// `Unable to lookup Acct in XREF AIX file...` on any other response, both on
/// `ACTIDIN`.
async fn read_cxacaix_file<S: XrefReader>(
    store: &mut S,
    id: AccountId,
) -> Result<CardXref, DomainError> {
    store
        .read_xref_by_account(id)
        .await
        .map_err(|error| match error {
            PortError::NotFound => {
                DomainError::not_found(ACCOUNT_ID_NOT_FOUND).with_field(FIELD_ACTIDIN)
            }
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_LOOKUP_ACCT_IN_XREF_AIX_FILE)
                    .with_field(FIELD_ACTIDIN)
            }
        })
}

/// COTRN02C READ-CCXREF-FILE — P-COTRN02C-009; V-COTRN02C-006,
/// V-COTRN02C-007.
///
/// `READ DATASET('CCXREF')` by `XREF-CARD-NUM` [app/cbl/COTRN02C.cbl:L609-637].
///
/// # Errors
///
/// Not found with `Card Number NOT found...` on `NOTFND`, unexpected with
/// `Unable to lookup Card # in XREF file...` on any other response, both on
/// `CARDNIN`.
async fn read_ccxref_file<S: XrefReader>(
    store: &mut S,
    card: CardNumber,
) -> Result<CardXref, DomainError> {
    store
        .read_xref_by_card(card)
        .await
        .map_err(|error| match error {
            PortError::NotFound => {
                DomainError::not_found(CARD_NUMBER_NOT_FOUND).with_field(FIELD_CARDNIN)
            }
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_LOOKUP_CARD_IN_XREF_FILE)
                    .with_field(FIELD_CARDNIN)
            }
        })
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-010 to P-COTRN02C-016;
/// V-COTRN02C-008 to V-COTRN02C-026.
///
/// The data-field edits of [app/cbl/COTRN02C.cbl:L235-437], one function per
/// `EVALUATE` or `IF` block, in the COBOL order: the blank checks, the code
/// class tests, the amount format, the two date formats, the `NUMVAL-C` of the
/// amount, the two `CSUTLDTC` calls and the merchant-ID class test. The
/// `IF ERR-FLG-ON MOVE SPACES …` at the start never acts, because every earlier
/// failure has already ended the task.
///
/// # Errors
///
/// The first failing edit, as [`validate`] lists them.
fn validate_data_fields(
    fields: &DataFields,
    xref: CardXref,
    card_number: CardNumber,
) -> Result<ValidatedTransaction, DomainError> {
    check_not_empty(fields)?;
    let (type_cd, cat_cd) = edit_codes(fields)?;
    let amount = edit_amount(&fields.amount)?;
    let orig_date = edit_date_format(
        &fields.orig_date,
        ORIG_DATE_SHOULD_BE_IN_FORMAT,
        FIELD_TORIGDT,
    )?;
    let proc_date = edit_date_format(
        &fields.proc_date,
        PROC_DATE_SHOULD_BE_IN_FORMAT,
        FIELD_TPROCDT,
    )?;
    check_date(&orig_date, ORIG_DATE_NOT_A_VALID_DATE, FIELD_TORIGDT)?;
    check_date(&proc_date, PROC_DATE_NOT_A_VALID_DATE, FIELD_TPROCDT)?;
    let merchant_id = edit_merchant_id(&fields.merchant_id)?;
    Ok(ValidatedTransaction {
        xref,
        card_number,
        type_cd,
        cat_cd,
        source: fields.source.field,
        description: fields.description.field,
        amount,
        orig_date,
        proc_date,
        merchant_id,
        merchant_name: fields.merchant_name.field,
        merchant_city: fields.merchant_city.field,
        merchant_zip: fields.merchant_zip.field,
    })
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-010; V-COTRN02C-008 to
/// V-COTRN02C-018.
///
/// The first `EVALUATE TRUE` [app/cbl/COTRN02C.cbl:L251-320]: each field
/// `= SPACES OR LOW-VALUES` in screen order, and the first blank one wins.
///
/// # Errors
///
/// Empty with `<field> can NOT be empty...` on that field.
fn check_not_empty(fields: &DataFields) -> Result<(), DomainError> {
    let checks: [(bool, &'static str, &'static str); 11] = [
        (
            fields.type_cd.is_blank(),
            TYPE_CD_CAN_NOT_BE_EMPTY,
            FIELD_TTYPCD,
        ),
        (
            fields.cat_cd.is_blank(),
            CATEGORY_CD_CAN_NOT_BE_EMPTY,
            FIELD_TCATCD,
        ),
        (
            fields.source.is_blank(),
            SOURCE_CAN_NOT_BE_EMPTY,
            FIELD_TRNSRC,
        ),
        (
            fields.description.is_blank(),
            DESCRIPTION_CAN_NOT_BE_EMPTY,
            FIELD_TDESC,
        ),
        (
            fields.amount.is_blank(),
            AMOUNT_CAN_NOT_BE_EMPTY,
            FIELD_TRNAMT,
        ),
        (
            fields.orig_date.is_blank(),
            ORIG_DATE_CAN_NOT_BE_EMPTY,
            FIELD_TORIGDT,
        ),
        (
            fields.proc_date.is_blank(),
            PROC_DATE_CAN_NOT_BE_EMPTY,
            FIELD_TPROCDT,
        ),
        (
            fields.merchant_id.is_blank(),
            MERCHANT_ID_CAN_NOT_BE_EMPTY,
            FIELD_MID,
        ),
        (
            fields.merchant_name.is_blank(),
            MERCHANT_NAME_CAN_NOT_BE_EMPTY,
            FIELD_MNAME,
        ),
        (
            fields.merchant_city.is_blank(),
            MERCHANT_CITY_CAN_NOT_BE_EMPTY,
            FIELD_MCITY,
        ),
        (
            fields.merchant_zip.is_blank(),
            MERCHANT_ZIP_CAN_NOT_BE_EMPTY,
            FIELD_MZIP,
        ),
    ];
    match checks.iter().find(|(blank, _, _)| *blank) {
        Some(&(_, message, field)) => Err(DomainError::empty(message).with_field(field)),
        None => Ok(()),
    }
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-011; V-COTRN02C-019,
/// V-COTRN02C-020.
///
/// The second `EVALUATE TRUE` [app/cbl/COTRN02C.cbl:L322-337]: `TTYPCDI` and
/// then `TCATCDI` must pass `IS NUMERIC`. The two digits are the type code
/// `MOVE TTYPCDI TO TRAN-TYPE-CD` copies, the four digits the value
/// `MOVE TCATCDI TO TRAN-CAT-CD` stores.
///
/// # Errors
///
/// Invalid with `Type CD must be Numeric...` on `TTYPCD`, else with
/// `Category CD must be Numeric...` on `TCATCD`.
fn edit_codes(fields: &DataFields) -> Result<(TranTypeCode, TranCategoryCode), DomainError> {
    let type_cd = fields
        .type_cd
        .numeric()
        .map(TranTypeCode::from_alnum)
        .ok_or_else(|| DomainError::invalid(TYPE_CD_MUST_BE_NUMERIC).with_field(FIELD_TTYPCD))?;
    let cat_cd = fields
        .cat_cd
        .numeric()
        .and_then(|field| field.as_text().parse::<TranCategoryCode>().ok())
        .ok_or_else(|| {
            DomainError::invalid(CATEGORY_CD_MUST_BE_NUMERIC).with_field(FIELD_TCATCD)
        })?;
    Ok((type_cd, cat_cd))
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-012, P-COTRN02C-014,
/// P-COTRN02C-028; V-COTRN02C-021.
///
/// The third `EVALUATE TRUE` [app/cbl/COTRN02C.cbl:L339-351]: byte 1 `-` or
/// `+`, bytes 2-9 digits, byte 10 `.` and bytes 11-12 digits. An amount in
/// that form is a valid `NUMVAL-C` argument whose value is exact
/// [app/cbl/COTRN02C.cbl:L383-386]: its sign, eight integer digits and two
/// decimals, at most 99999999.99, which `WS-TRAN-AMT-N S9(9)V99` holds whole.
/// Re-editing it into `TRNAMTI` through `+99999999.99` and reading it again in
/// `ADD-TRANSACTION` gives the same value; `-00000000.00` is zero.
///
/// # Errors
///
/// Invalid with `Amount should be in format -99999999.99` on `TRNAMT`.
fn edit_amount(amount: &Received<AMOUNT_LEN>) -> Result<Money<9>, DomainError> {
    amount_value(amount)
        .ok_or_else(|| DomainError::invalid(AMOUNT_SHOULD_BE_IN_FORMAT).with_field(FIELD_TRNAMT))
}

/// The `NUMVAL-C` value of an amount in the form `±99999999.99`, by the
/// reference modifications `TRNAMTI(1:1)`, `(2:8)`, `(10:1)` and `(11:2)`;
/// `None` for an amount in any other form.
fn amount_value(amount: &Received<AMOUNT_LEN>) -> Option<Money<9>> {
    let field = amount.whole()?;
    let negative = match field.ref_mod(1, 1)? {
        b"-" => true,
        b"+" => false,
        _ => return None,
    };
    if field.ref_mod(10, 1)? != b".".as_slice() {
        return None;
    }
    let units = digits_value(field.ref_mod(2, 8)?)?;
    let hundredths = digits_value(field.ref_mod(11, 2)?)?;
    let cents = units * 100 + hundredths;
    Money::from_cents(if negative { -cents } else { cents }).ok()
}

/// The value of `part` when every byte is a digit, `None` otherwise. Callers
/// pass at most eight digits, far inside the `i64` range.
fn digits_value(part: &[u8]) -> Option<i64> {
    part.iter().try_fold(0_i64, |total, &byte| {
        let digit = char::from(byte).to_digit(10)?;
        Some(total * 10 + i64::from(digit))
    })
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-013; V-COTRN02C-022,
/// V-COTRN02C-023.
///
/// The fourth and fifth `EVALUATE TRUE` [app/cbl/COTRN02C.cbl:L353-381], one
/// per date: `(1:4)`, `(6:2)` and `(9:2)` digits, `(5:1)` and `(8:1)` a dash.
/// Only positions are tested; the calendar check is [`check_date`].
///
/// # Errors
///
/// Invalid with `message` on `field`: `Orig Date should be in format
/// YYYY-MM-DD` on `TORIGDT` or `Proc Date should be in format YYYY-MM-DD` on
/// `TPROCDT`.
fn edit_date_format(
    date: &Received<DATE_LEN>,
    message: &'static str,
    field: &'static str,
) -> Result<Alnum<DATE_LEN>, DomainError> {
    date.whole()
        .filter(is_dashed_date)
        .ok_or_else(|| DomainError::invalid(message).with_field(field))
}

/// Whether `date` holds digits and dashes in the `YYYY-MM-DD` positions.
fn is_dashed_date(date: &Alnum<DATE_LEN>) -> bool {
    let digits = DATE_DIGIT_PARTS.iter().all(|&(start, len)| {
        date.ref_mod(start, len)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    });
    let dashes = DATE_DASH_POSITIONS
        .iter()
        .all(|&position| date.ref_mod(position, 1) == Some(b"-".as_slice()));
    digits && dashes
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-015; V-COTRN02C-024,
/// V-COTRN02C-025.
///
/// `CALL 'CSUTLDTC'` with the date and `WS-DATE-FORMAT` `YYYY-MM-DD`
/// [app/cbl/COTRN02C.cbl:L389-427]: the date passes when the severity is
/// `0000` or the message number `2513`, so a calendar date outside the range
/// `CEEDAYS` supports is accepted.
///
/// # Errors
///
/// - Invalid with `message` on `field` for any other feedback:
///   `Orig Date - Not a valid date...` on `TORIGDT` or
///   `Proc Date - Not a valid date...` on `TPROCDT`.
/// - Unexpected with the unexpected-error text on `field` for a date whose
///   feedback no reviewed row pins (year `0000`, or out of range and not a
///   calendar date: E-31, halt), which is never guessed.
fn check_date(
    date: &Alnum<DATE_LEN>,
    message: &'static str,
    field: &'static str,
) -> Result<(), DomainError> {
    match date_validation::validate(date.as_bytes(), DatePicture::YyyyMmDdDashed) {
        Ok(result) if result.accepts_with_2513() => Ok(()),
        Ok(_) => Err(DomainError::invalid(message).with_field(field)),
        Err(DateValidationError::Unproven { .. }) => {
            Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(field))
        }
    }
}

/// COTRN02C VALIDATE-INPUT-DATA-FIELDS — P-COTRN02C-016; V-COTRN02C-026.
///
/// `IF MIDI OF COTRN2AI IS NOT NUMERIC` [app/cbl/COTRN02C.cbl:L430-436]: the
/// nine digits are the value `MOVE MIDI TO TRAN-MERCHANT-ID` stores.
///
/// # Errors
///
/// Invalid with `Merchant ID must be Numeric...` on `MID`.
fn edit_merchant_id(merchant_id: &Received<MERCHANT_ID_LEN>) -> Result<MerchantId, DomainError> {
    merchant_id
        .numeric()
        .and_then(|field| field.as_text().parse::<MerchantId>().ok())
        .ok_or_else(|| DomainError::invalid(MERCHANT_ID_MUST_BE_NUMERIC).with_field(FIELD_MID))
}

/// COTRN02C PROCESS-ENTER-KEY — P-COTRN02C-017 to P-COTRN02C-019;
/// V-COTRN02C-027, V-COTRN02C-028.
///
/// `EVALUATE CONFIRMI` [app/cbl/COTRN02C.cbl:L169-188], reached only when
/// every field edit passed: `Y` or `y` performs `ADD-TRANSACTION`.
///
/// # Errors
///
/// Invalid on `CONFIRM`: `Confirm to add this transaction...` for `N`, `n`,
/// `SPACES` or `LOW-VALUES`, which set `WS-ERR-FLG` (400); `Invalid value.
/// Valid values are (Y/N)...` for any other value.
fn confirm_add(confirm: &Confirm) -> Result<(), DomainError> {
    match confirm {
        Confirm::Yes => Ok(()),
        Confirm::No | Confirm::Blank => {
            Err(DomainError::invalid(CONFIRM_TO_ADD_THIS_TRANSACTION).with_field(FIELD_CONFIRM))
        }
        Confirm::Other(_) => {
            Err(DomainError::invalid(INVALID_VALUE_VALID_VALUES_ARE_Y_N).with_field(FIELD_CONFIRM))
        }
    }
}

/// COTRN02C ADD-TRANSACTION, STARTBR-TRANSACT-FILE, READPREV-TRANSACT-FILE —
/// P-COTRN02C-020 to P-COTRN02C-022, P-COTRN02C-029; V-COTRN02C-029 to
/// V-COTRN02C-031.
///
/// The outcome of the TRAN-ID allocation [app/cbl/COTRN02C.cbl:L442-449]
/// [app/cbl/COTRN02C.cbl:L642-697], which `crates/persistence/src/ids.rs`
/// performs as one serialized highest-key-plus-one read. The allocation
/// answers [`PortError::NotFound`] when the live master is empty, the
/// `STARTBR` `NOTFND` at `HIGH-VALUES` after which the COBOL adds nothing
/// (E-27), and any other [`PortError`] when the browse fails.
///
/// # Errors
///
/// Not found with `Transaction ID NOT found...` for an empty live master;
/// unexpected with `Unable to lookup Transaction...` for any other failure,
/// the text both the `STARTBR` and the `READPREV` failures show. Both are on
/// `ACTIDIN`.
pub fn allocated_tran_id(result: PortResult<TranId>) -> Result<TranId, DomainError> {
    result.map_err(|error| match error {
        PortError::NotFound => {
            DomainError::not_found(TRANSACTION_ID_NOT_FOUND).with_field(FIELD_ACTIDIN)
        }
        PortError::Duplicate
        | PortError::Restricted
        | PortError::Contention
        | PortError::Unavailable(_) => {
            DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION).with_field(FIELD_ACTIDIN)
        }
    })
}

/// COTRN02C ADD-TRANSACTION — P-COTRN02C-023, P-COTRN02C-030.
///
/// `TRAN-RECORD` as [app/cbl/COTRN02C.cbl:L450-465] fills it: `INITIALIZE`,
/// then the allocated `tran_id` and every field moved from the map, so
/// nothing `INITIALIZE` set survives. The text fields move into their wider
/// record fields padded with spaces: `TDESCI X(60)` into `TRAN-DESC X(100)`,
/// `MNAMEI X(30)` and `MCITYI X(25)` into `X(50)`, and each date `X(10)` into
/// its timestamp `X(26)`, which then holds the entered date followed by 16
/// spaces. No clock is read. The card number is `CARDNINI` after the key edit.
#[must_use]
pub fn build(validated: &ValidatedTransaction, tran_id: TranId) -> NewTransaction {
    NewTransaction {
        tran_id,
        tran_type_cd: validated.type_cd,
        tran_cat_cd: validated.cat_cd,
        tran_source: Alnum::move_from_bytes(validated.source.as_bytes()),
        tran_desc: Alnum::move_from_bytes(validated.description.as_bytes()),
        tran_amt: validated.amount,
        tran_merchant_id: validated.merchant_id,
        tran_merchant_name: Alnum::move_from_bytes(validated.merchant_name.as_bytes()),
        tran_merchant_city: Alnum::move_from_bytes(validated.merchant_city.as_bytes()),
        tran_merchant_zip: Alnum::move_from_bytes(validated.merchant_zip.as_bytes()),
        tran_card_num: validated.card_number,
        tran_orig_ts: Alnum::move_from_bytes(validated.orig_date.as_bytes()),
        tran_proc_ts: Alnum::move_from_bytes(validated.proc_date.as_bytes()),
    }
}

/// COTRN02C WRITE-TRANSACT-FILE — P-COTRN02C-024, P-COTRN02C-025;
/// V-COTRN02C-032, V-COTRN02C-033.
///
/// The answer to the `WRITE DATASET('TRANSACT')` of the record [`build`] made
/// with `tran_id` [app/cbl/COTRN02C.cbl:L711-749]. `NORMAL` clears the screen
/// and shows `Transaction added successfully.  Your Tran ID is <TRAN-ID>.`,
/// the text this returns for the 201 response.
///
/// # Errors
///
/// Duplicate with `Tran ID already exist...` for `DUPKEY` or `DUPREC`;
/// unexpected with `Unable to Add Transaction...` for any other response. Both
/// are on `ACTIDIN`.
pub fn write_result<T>(result: PortResult<T>, tran_id: &TranId) -> Result<String, DomainError> {
    result
        .map(|_written| transaction_added_message(&tran_id.as_alnum().as_text()))
        .map_err(|error| match error {
            PortError::Duplicate => {
                DomainError::duplicate(TRAN_ID_ALREADY_EXIST).with_field(FIELD_ACTIDIN)
            }
            PortError::NotFound
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_ADD_TRANSACTION).with_field(FIELD_ACTIDIN)
            }
        })
}

/// COTRN02C COPY-LAST-TRAN-DATA — P-COTRN02C-026.
///
/// The map fields PF5 fills from `last`, the highest-keyed transaction of the
/// whole live master [app/cbl/COTRN02C.cbl:L480-493], each moved into its map
/// field: the amount through `WS-TRAN-AMT-E PIC +99999999.99`, which keeps
/// the sign and the low-order eight integer digits; the numeric codes as their
/// zero-padded digits; every text cut or padded with spaces to its map width,
/// so a timestamp becomes its first ten characters, the date.
///
/// The key fields are left blank and the confirmation [`Confirm::Blank`]: PF5
/// keeps what the user entered in them, so the caller sets them, and the
/// result then goes through [`validate`] as `PROCESS-ENTER-KEY` follows the
/// copy. Each text is the full map field, trailing spaces included, so the
/// fields [`validate`] receives hold exactly the bytes the COBOL moved.
#[must_use]
pub fn copied_fields(last: &Transaction) -> TransactionAddInput {
    TransactionAddInput {
        account_id: None,
        card_number: None,
        type_cd: map_text::<TYPE_CD_LEN>(last.tran_type_cd.as_bytes()),
        cat_cd: last.tran_cat_cd.to_string(),
        source: map_text::<SOURCE_LEN>(last.tran_source.as_bytes()),
        description: map_text::<DESCRIPTION_LEN>(last.tran_desc.as_bytes()),
        amount: edit_money(&PIC_TRAN_AMT_DISPLAY, last.tran_amt),
        orig_date: map_text::<DATE_LEN>(last.tran_orig_ts.as_bytes()),
        proc_date: map_text::<DATE_LEN>(last.tran_proc_ts.as_bytes()),
        merchant_id: last.tran_merchant_id.to_string(),
        merchant_name: map_text::<MERCHANT_NAME_LEN>(last.tran_merchant_name.as_bytes()),
        merchant_city: map_text::<MERCHANT_CITY_LEN>(last.tran_merchant_city.as_bytes()),
        merchant_zip: map_text::<MERCHANT_ZIP_LEN>(last.tran_merchant_zip.as_bytes()),
        confirm: Confirm::Blank,
    }
}

/// The text of the `PIC X(N)` map field a record field's `bytes` are moved
/// into.
fn map_text<const N: usize>(bytes: &[u8]) -> String {
    Alnum::<N>::move_from_bytes(bytes).as_text().into_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        FIELD_ACTIDIN, FIELD_CARDNIN, FIELD_CONFIRM, FIELD_MCITY, FIELD_MID, FIELD_MNAME,
        FIELD_MZIP, FIELD_TCATCD, FIELD_TDESC, FIELD_TORIGDT, FIELD_TPROCDT, FIELD_TRNAMT,
        FIELD_TRNSRC, FIELD_TTYPCD, TransactionAddInput, ValidatedTransaction, allocated_tran_id,
        build, copied_fields, validate, write_result,
    };
    use crate::entities::{CardXref, NewTransaction, Transaction};
    use crate::error::DomainError;
    use crate::messages::{
        ACCOUNT_ID_MUST_BE_NUMERIC, ACCOUNT_ID_NOT_FOUND, ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED,
        AMOUNT_CAN_NOT_BE_EMPTY, AMOUNT_SHOULD_BE_IN_FORMAT, CARD_NUMBER_MUST_BE_NUMERIC,
        CARD_NUMBER_NOT_FOUND, CATEGORY_CD_CAN_NOT_BE_EMPTY, CATEGORY_CD_MUST_BE_NUMERIC,
        CONFIRM_TO_ADD_THIS_TRANSACTION, DESCRIPTION_CAN_NOT_BE_EMPTY,
        INVALID_VALUE_VALID_VALUES_ARE_Y_N, MERCHANT_CITY_CAN_NOT_BE_EMPTY,
        MERCHANT_ID_CAN_NOT_BE_EMPTY, MERCHANT_ID_MUST_BE_NUMERIC, MERCHANT_NAME_CAN_NOT_BE_EMPTY,
        MERCHANT_ZIP_CAN_NOT_BE_EMPTY, ORIG_DATE_CAN_NOT_BE_EMPTY, ORIG_DATE_NOT_A_VALID_DATE,
        ORIG_DATE_SHOULD_BE_IN_FORMAT, PROC_DATE_CAN_NOT_BE_EMPTY, PROC_DATE_NOT_A_VALID_DATE,
        PROC_DATE_SHOULD_BE_IN_FORMAT, SOURCE_CAN_NOT_BE_EMPTY, TRAN_ID_ALREADY_EXIST,
        TRANSACTION_ADDED_SUCCESSFULLY, TRANSACTION_ID_NOT_FOUND, TYPE_CD_CAN_NOT_BE_EMPTY,
        TYPE_CD_MUST_BE_NUMERIC, UNABLE_TO_ADD_TRANSACTION, UNABLE_TO_LOOKUP_ACCT_IN_XREF_AIX_FILE,
        UNABLE_TO_LOOKUP_CARD_IN_XREF_FILE, UNABLE_TO_LOOKUP_TRANSACTION, UNEXPECTED_ERROR,
        YOUR_TRAN_ID_IS, transaction_added_message,
    };
    use crate::numeric::{
        AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money, TranCategoryCode, TranId,
        TranTypeCode, TransactionRowId,
    };
    use crate::ports::PortError;
    use crate::rules::Confirm;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The card of account 50.
    const CARD_A: &str = "0500024453765740";
    /// The card of account 1.
    const CARD_B: &str = "0683586198171516";
    /// The TRAN-ID the tests allocate.
    const NEXT_TRAN_ID: &str = "0000000000000301";

    /// The blank-field messages and fields of V-COTRN02C-008 to
    /// V-COTRN02C-018, in screen order.
    const EMPTY_MESSAGES: [(&str, &str); 11] = [
        (TYPE_CD_CAN_NOT_BE_EMPTY, FIELD_TTYPCD),
        (CATEGORY_CD_CAN_NOT_BE_EMPTY, FIELD_TCATCD),
        (SOURCE_CAN_NOT_BE_EMPTY, FIELD_TRNSRC),
        (DESCRIPTION_CAN_NOT_BE_EMPTY, FIELD_TDESC),
        (AMOUNT_CAN_NOT_BE_EMPTY, FIELD_TRNAMT),
        (ORIG_DATE_CAN_NOT_BE_EMPTY, FIELD_TORIGDT),
        (PROC_DATE_CAN_NOT_BE_EMPTY, FIELD_TPROCDT),
        (MERCHANT_ID_CAN_NOT_BE_EMPTY, FIELD_MID),
        (MERCHANT_NAME_CAN_NOT_BE_EMPTY, FIELD_MNAME),
        (MERCHANT_CITY_CAN_NOT_BE_EMPTY, FIELD_MCITY),
        (MERCHANT_ZIP_CAN_NOT_BE_EMPTY, FIELD_MZIP),
    ];

    fn xref(card: &str, customer: u32, account: u64) -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(card),
            xref_cust_id: CustomerId::new(customer).expect("a 9-digit customer"),
            xref_acct_id: AccountId::new(account).expect("an 11-digit account"),
        }
    }

    fn store() -> FakeStore {
        FakeStore::default()
            .with_xref(xref(CARD_A, 50, 50))
            .with_xref(xref(CARD_B, 1, 1))
    }

    /// An ENTER every edit accepts: account 50, type 01, category 0001.
    fn valid_input() -> TransactionAddInput {
        TransactionAddInput {
            account_id: Some("00000000050".to_owned()),
            card_number: None,
            type_cd: "01".to_owned(),
            cat_cd: "0001".to_owned(),
            source: "POS TERM".to_owned(),
            description: "Purchase at Acme".to_owned(),
            amount: "+00000010.00".to_owned(),
            orig_date: "2022-07-01".to_owned(),
            proc_date: "2022-07-02".to_owned(),
            merchant_id: "800000001".to_owned(),
            merchant_name: "Acme Store".to_owned(),
            merchant_city: "Seattle".to_owned(),
            merchant_zip: "98101".to_owned(),
            confirm: Confirm::Yes,
        }
    }

    /// The valid input with one data field, by screen position, set to `text`.
    fn with_data_field(
        mut input: TransactionAddInput,
        index: usize,
        text: &str,
    ) -> TransactionAddInput {
        let field = match index {
            0 => &mut input.type_cd,
            1 => &mut input.cat_cd,
            2 => &mut input.source,
            3 => &mut input.description,
            4 => &mut input.amount,
            5 => &mut input.orig_date,
            6 => &mut input.proc_date,
            7 => &mut input.merchant_id,
            8 => &mut input.merchant_name,
            9 => &mut input.merchant_city,
            10 => &mut input.merchant_zip,
            _ => panic!("COTRN2A has 11 data fields, not {index}"),
        };
        text.clone_into(field);
        input
    }

    fn run(
        store: &mut FakeStore,
        input: TransactionAddInput,
    ) -> Result<ValidatedTransaction, DomainError> {
        block_on(validate(store, input))
    }

    fn accepted(input: TransactionAddInput) -> ValidatedTransaction {
        run(&mut store(), input).expect("every edit passes")
    }

    fn rejection(input: TransactionAddInput) -> DomainError {
        run(&mut store(), input).expect_err("an edit fails")
    }

    fn invalid(message: &str, field: &'static str) -> DomainError {
        DomainError::invalid(message).with_field(field)
    }

    fn tran_id() -> TranId {
        TranId::move_from(NEXT_TRAN_ID)
    }

    // P-COTRN02C-005, P-COTRN02C-008: the account wins over an entered card.
    #[test]
    fn account_path_reads_cxacaix_and_takes_the_card_from_the_cross_reference() {
        let mut store = store();
        let input = TransactionAddInput {
            account_id: Some("00000000050   ".to_owned()),
            card_number: Some(CARD_B.to_owned()),
            ..valid_input()
        };
        let validated = run(&mut store, input).expect("a valid add");
        assert_eq!(validated.xref, xref(CARD_A, 50, 50));
        assert_eq!(validated.card_number, CardNumber::move_from(CARD_A));
        assert_eq!(
            validated.account_id(),
            AccountId::new(50).expect("an account")
        );
        assert_eq!(store.calls, ["read_xref_by_account"]);
    }

    // P-COTRN02C-006, P-COTRN02C-009: a blank or LOW-VALUES account uses the card.
    #[test]
    fn card_path_reads_ccxref_and_takes_the_account_from_the_cross_reference() {
        for account_id in [None, Some(""), Some("           "), Some("\0\0")] {
            let mut store = store();
            let input = TransactionAddInput {
                account_id: account_id.map(str::to_owned),
                card_number: Some(CARD_B.to_owned()),
                ..valid_input()
            };
            let validated = run(&mut store, input).expect("a valid add");
            assert_eq!(validated.xref, xref(CARD_B, 1, 1), "{account_id:?}");
            assert_eq!(validated.card_number, CardNumber::move_from(CARD_B));
            assert_eq!(store.calls, ["read_xref_by_card"]);
        }
    }

    // V-COTRN02C-001 to V-COTRN02C-004, V-COTRN02C-006: key edits run before
    // any data edit, so the blank type code never shows.
    #[test]
    fn key_field_edits_reject_before_the_data_fields() {
        let not_found = |message, field| DomainError::not_found(message).with_field(field);
        let cases = [
            (
                Some("0000000005A"),
                Some(CARD_A),
                invalid(ACCOUNT_ID_MUST_BE_NUMERIC, FIELD_ACTIDIN),
            ),
            (
                Some("50"),
                None,
                invalid(ACCOUNT_ID_MUST_BE_NUMERIC, FIELD_ACTIDIN),
            ),
            (
                Some("000000000500"),
                None,
                invalid(ACCOUNT_ID_MUST_BE_NUMERIC, FIELD_ACTIDIN),
            ),
            (
                None,
                Some("050002445376574X"),
                invalid(CARD_NUMBER_MUST_BE_NUMERIC, FIELD_CARDNIN),
            ),
            (
                Some(" "),
                Some("0500024453765"),
                invalid(CARD_NUMBER_MUST_BE_NUMERIC, FIELD_CARDNIN),
            ),
            (
                None,
                None,
                DomainError::empty(ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED)
                    .with_field(FIELD_ACTIDIN),
            ),
            (
                Some("\0\0\0"),
                Some("   "),
                DomainError::empty(ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED)
                    .with_field(FIELD_ACTIDIN),
            ),
            (
                Some("99999999999"),
                None,
                not_found(ACCOUNT_ID_NOT_FOUND, FIELD_ACTIDIN),
            ),
            (
                None,
                Some("9999999999999999"),
                not_found(CARD_NUMBER_NOT_FOUND, FIELD_CARDNIN),
            ),
        ];
        for (account_id, card_number, expected) in cases {
            let input = TransactionAddInput {
                account_id: account_id.map(str::to_owned),
                card_number: card_number.map(str::to_owned),
                type_cd: String::new(),
                ..valid_input()
            };
            assert_eq!(rejection(input), expected, "{account_id:?} {card_number:?}");
        }
    }

    // V-COTRN02C-005, V-COTRN02C-007.
    #[test]
    fn cross_reference_read_failures_name_their_file() {
        let card_input = TransactionAddInput {
            account_id: None,
            card_number: Some(CARD_A.to_owned()),
            ..valid_input()
        };
        let cases = [
            (
                "read_xref_by_account",
                valid_input(),
                DomainError::unexpected(UNABLE_TO_LOOKUP_ACCT_IN_XREF_AIX_FILE)
                    .with_field(FIELD_ACTIDIN),
            ),
            (
                "read_xref_by_card",
                card_input,
                DomainError::unexpected(UNABLE_TO_LOOKUP_CARD_IN_XREF_FILE)
                    .with_field(FIELD_CARDNIN),
            ),
        ];
        for (method, input, expected) in cases {
            for error in [
                PortError::Unavailable("connection reset".to_owned()),
                PortError::Contention,
            ] {
                let mut store = store();
                store.fail_next(method, error);
                assert_eq!(
                    run(&mut store, input.clone()),
                    Err(expected.clone()),
                    "{method}"
                );
            }
        }
    }

    // V-COTRN02C-008 to V-COTRN02C-018: one EVALUATE, the first blank field
    // in screen order wins; LOW-VALUES are blank too.
    #[test]
    fn the_first_blank_data_field_in_screen_order_wins() {
        for (index, &(message, field)) in EMPTY_MESSAGES.iter().enumerate() {
            let blank_from_here = (index..EMPTY_MESSAGES.len())
                .fold(valid_input(), |input, later| {
                    with_data_field(input, later, "")
                });
            let expected = DomainError::empty(message).with_field(field);
            assert_eq!(rejection(blank_from_here), expected, "field {index}");
            let low_values = with_data_field(valid_input(), index, "\0");
            assert_eq!(rejection(low_values), expected, "field {index} LOW-VALUES");
        }
    }

    // P-COTRN02C-010, P-COTRN02C-011: the blank checks run before the class
    // and format edits.
    #[test]
    fn blank_checks_run_before_the_format_edits() {
        let input = TransactionAddInput {
            type_cd: "0A".to_owned(),
            amount: "10.00".to_owned(),
            merchant_zip: "   ".to_owned(),
            ..valid_input()
        };
        assert_eq!(
            rejection(input),
            DomainError::empty(MERCHANT_ZIP_CAN_NOT_BE_EMPTY).with_field(FIELD_MZIP)
        );
    }

    // V-COTRN02C-019, V-COTRN02C-020: type before category, both before the
    // amount format.
    #[test]
    fn type_and_category_codes_must_be_numeric() {
        let type_error = invalid(TYPE_CD_MUST_BE_NUMERIC, FIELD_TTYPCD);
        let category_error = invalid(CATEGORY_CD_MUST_BE_NUMERIC, FIELD_TCATCD);
        let cases = [
            ("0A", "0001", &type_error),
            ("1", "0001", &type_error),
            ("011", "0001", &type_error),
            ("0A", "00A1", &type_error),
            ("01", "00A1", &category_error),
            ("01", "001", &category_error),
        ];
        for (type_cd, cat_cd, expected) in cases {
            let input = TransactionAddInput {
                type_cd: type_cd.to_owned(),
                cat_cd: cat_cd.to_owned(),
                amount: "10.00".to_owned(),
                ..valid_input()
            };
            assert_eq!(&rejection(input), expected, "{type_cd:?} {cat_cd:?}");
        }
    }

    // P-COTRN02C-014, P-COTRN02C-028: the exact NUMVAL-C of a format-valid
    // amount.
    #[test]
    fn a_format_valid_amount_is_its_exact_value() {
        let cases = [
            ("+00000010.00", 1_000),
            ("-00000001.50", -150),
            ("-00000000.00", 0),
            ("+99999999.99", 9_999_999_999),
            ("+00000010.00   ", 1_000),
        ];
        for (amount, cents) in cases {
            let validated = accepted(TransactionAddInput {
                amount: amount.to_owned(),
                ..valid_input()
            });
            let expected = Money::<9>::from_cents(cents).expect("a 9-digit amount");
            assert_eq!(validated.amount, expected, "{amount:?}");
            assert!(!validated.amount.is_negative() || cents < 0, "{amount:?}");
        }
    }

    // V-COTRN02C-021, before the date formats.
    #[test]
    fn an_amount_not_in_the_form_sign_8_digits_point_2_digits_is_rejected() {
        for amount in [
            "10.00",
            "+00000012.5X",
            "*00000010.00",
            "+0000001000",
            "+0000010.00",
            "+00000010.001",
            "+0000 010.00",
            "+00000010,00",
        ] {
            let input = TransactionAddInput {
                amount: amount.to_owned(),
                orig_date: "2022/07/01".to_owned(),
                ..valid_input()
            };
            assert_eq!(
                rejection(input),
                invalid(AMOUNT_SHOULD_BE_IN_FORMAT, FIELD_TRNAMT),
                "{amount:?}"
            );
        }
    }

    // V-COTRN02C-022, V-COTRN02C-023: both formats are edited before either
    // date goes to CSUTLDTC.
    #[test]
    fn dates_must_be_in_the_form_yyyy_mm_dd() {
        let orig_error = invalid(ORIG_DATE_SHOULD_BE_IN_FORMAT, FIELD_TORIGDT);
        let proc_error = invalid(PROC_DATE_SHOULD_BE_IN_FORMAT, FIELD_TPROCDT);
        let cases = [
            ("2022/07/01", "2022-07-02", &orig_error),
            ("2022-7-01", "2022-07-02", &orig_error),
            ("2022-07-011", "2022-07-02", &orig_error),
            ("2022/07/01", "20220701", &orig_error),
            ("2022-07-01", "20220701", &proc_error),
            ("2022-02-30", "2022.07.02", &proc_error),
        ];
        for (orig_date, proc_date, expected) in cases {
            let input = TransactionAddInput {
                orig_date: orig_date.to_owned(),
                proc_date: proc_date.to_owned(),
                ..valid_input()
            };
            assert_eq!(&rejection(input), expected, "{orig_date:?} {proc_date:?}");
        }
    }

    // V-COTRN02C-024, V-COTRN02C-025, P-COTRN02C-015, E-31: CSUTLDTC accepts
    // severity 0000 and message 2513; unproven classes are never guessed.
    #[test]
    fn dates_are_checked_by_csutldtc_with_the_2513_exception() {
        let orig_error = invalid(ORIG_DATE_NOT_A_VALID_DATE, FIELD_TORIGDT);
        let proc_error = invalid(PROC_DATE_NOT_A_VALID_DATE, FIELD_TPROCDT);
        let unproven_orig = DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_TORIGDT);
        let unproven_proc = DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_TPROCDT);
        let cases = [
            ("2022-02-30", "2022-07-02", Some(&orig_error)),
            ("2022-00-10", "2022-07-02", Some(&orig_error)),
            ("2022-07-01", "2022-13-01", Some(&proc_error)),
            ("2022-02-30", "2022-13-01", Some(&orig_error)),
            ("0000-01-01", "2022-07-02", Some(&unproven_orig)),
            ("2022-07-01", "1500-02-30", Some(&unproven_proc)),
            ("1500-01-01", "2022-07-02", None),
            ("2022-07-01", "1500-01-01", None),
            ("2024-02-29", "9999-12-31", None),
        ];
        for (orig_date, proc_date, expected) in cases {
            let input = TransactionAddInput {
                orig_date: orig_date.to_owned(),
                proc_date: proc_date.to_owned(),
                merchant_id: "12345678A".to_owned(),
                ..valid_input()
            };
            let expected = expected
                .cloned()
                .unwrap_or_else(|| invalid(MERCHANT_ID_MUST_BE_NUMERIC, FIELD_MID));
            assert_eq!(rejection(input), expected, "{orig_date:?} {proc_date:?}");
        }
    }

    // V-COTRN02C-026, before the confirmation.
    #[test]
    fn the_merchant_id_must_be_numeric() {
        for merchant_id in ["12345678A", "1234", "1234567890", "-12345678"] {
            let input = TransactionAddInput {
                merchant_id: merchant_id.to_owned(),
                confirm: Confirm::Other("X".to_owned()),
                ..valid_input()
            };
            assert_eq!(
                rejection(input),
                invalid(MERCHANT_ID_MUST_BE_NUMERIC, FIELD_MID),
                "{merchant_id:?}"
            );
        }
    }

    // P-COTRN02C-017 to P-COTRN02C-019; V-COTRN02C-027, V-COTRN02C-028.
    #[test]
    fn only_y_confirms_the_add() {
        let ask = invalid(CONFIRM_TO_ADD_THIS_TRANSACTION, FIELD_CONFIRM);
        let other = invalid(INVALID_VALUE_VALID_VALUES_ARE_Y_N, FIELD_CONFIRM);
        let cases = [
            (Some("Y"), None),
            (Some("y"), None),
            (Some("N"), Some(&ask)),
            (Some("n"), Some(&ask)),
            (Some(""), Some(&ask)),
            (Some(" "), Some(&ask)),
            (Some("\0"), Some(&ask)),
            (None, Some(&ask)),
            (Some("X"), Some(&other)),
            (Some("YY"), Some(&other)),
        ];
        for (confirm, expected) in cases {
            let input = TransactionAddInput {
                confirm: Confirm::parse(confirm),
                ..valid_input()
            };
            assert_eq!(
                run(&mut store(), input).err().as_ref(),
                expected,
                "{confirm:?}"
            );
        }
    }

    // P-COTRN02C-023, P-COTRN02C-030: every field moved, the dates padded to
    // the 26-byte timestamps, no clock read.
    #[test]
    fn build_moves_every_map_field_into_the_record() {
        let validated = accepted(valid_input());
        let record = build(&validated, tran_id());
        let expected = NewTransaction {
            tran_id: tran_id(),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: "0001".parse::<TranCategoryCode>().expect("a category"),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Acme"),
            tran_amt: Money::from_cents(1_000).expect("an amount"),
            tran_merchant_id: "800000001".parse::<MerchantId>().expect("a merchant"),
            tran_merchant_name: Alnum::move_from("Acme Store"),
            tran_merchant_city: Alnum::move_from("Seattle"),
            tran_merchant_zip: Alnum::move_from("98101"),
            tran_card_num: CardNumber::move_from(CARD_A),
            tran_orig_ts: Alnum::move_from("2022-07-01"),
            tran_proc_ts: Alnum::move_from("2022-07-02"),
        };
        assert_eq!(record, expected);
        for timestamp in [record.tran_orig_ts, record.tran_proc_ts] {
            let (date, padding) = timestamp.as_bytes().split_at(10);
            let digits_and_dashes = date.iter().enumerate().all(|(index, byte)| {
                if index == 4 || index == 7 {
                    *byte == b'-'
                } else {
                    byte.is_ascii_digit()
                }
            });
            assert!(digits_and_dashes, "{timestamp:?}");
            assert_eq!(padding, [b' '; 16], "{timestamp:?}");
        }
    }

    // The free text fields are cut at their map width, then padded to their
    // record width.
    #[test]
    fn free_text_is_cut_at_its_map_field_width() {
        let validated = accepted(TransactionAddInput {
            description: "D".repeat(70),
            merchant_name: "N".repeat(35),
            merchant_city: "C".repeat(30),
            ..valid_input()
        });
        let record = build(&validated, tran_id());
        assert_eq!(record.tran_desc, Alnum::move_from(&"D".repeat(60)));
        assert_eq!(record.tran_merchant_name, Alnum::move_from(&"N".repeat(30)));
        assert_eq!(record.tran_merchant_city, Alnum::move_from(&"C".repeat(25)));
    }

    // P-COTRN02C-024, P-COTRN02C-025; V-COTRN02C-032, V-COTRN02C-033.
    #[test]
    fn write_result_answers_the_write_as_write_transact_file() {
        let message = write_result(Ok(TransactionRowId::from(7)), &tran_id()).expect("a write");
        assert_eq!(message, transaction_added_message(NEXT_TRAN_ID));
        assert_eq!(
            message,
            format!("{TRANSACTION_ADDED_SUCCESSFULLY}{YOUR_TRAN_ID_IS}{NEXT_TRAN_ID}.")
        );
        let duplicate = DomainError::duplicate(TRAN_ID_ALREADY_EXIST).with_field(FIELD_ACTIDIN);
        let failed = DomainError::unexpected(UNABLE_TO_ADD_TRANSACTION).with_field(FIELD_ACTIDIN);
        let cases = [
            (PortError::Duplicate, &duplicate),
            (PortError::NotFound, &failed),
            (PortError::Restricted, &failed),
            (PortError::Contention, &failed),
            (PortError::Unavailable("disk full".to_owned()), &failed),
        ];
        for (error, expected) in cases {
            let answer = write_result::<TransactionRowId>(Err(error.clone()), &tran_id());
            assert_eq!(answer.as_ref().err(), Some(expected), "{error:?}");
        }
    }

    // P-COTRN02C-020 to P-COTRN02C-022; V-COTRN02C-029 to V-COTRN02C-031, E-27.
    #[test]
    fn allocation_failures_show_the_browse_messages() {
        assert_eq!(allocated_tran_id(Ok(tran_id())), Ok(tran_id()));
        let empty = DomainError::not_found(TRANSACTION_ID_NOT_FOUND).with_field(FIELD_ACTIDIN);
        let failed =
            DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION).with_field(FIELD_ACTIDIN);
        let cases = [
            (PortError::NotFound, &empty),
            (PortError::Unavailable("timeout".to_owned()), &failed),
            (PortError::Contention, &failed),
            (PortError::Duplicate, &failed),
            (PortError::Restricted, &failed),
        ];
        for (error, expected) in cases {
            let answer = allocated_tran_id(Err(error.clone()));
            assert_eq!(answer.as_ref().err(), Some(expected), "{error:?}");
        }
    }

    /// The highest-keyed transaction PF5 copies, with amount `cents`.
    fn last_transaction(cents: i64) -> Transaction {
        Transaction::from_new(
            TransactionRowId::from(9),
            NewTransaction {
                tran_id: TranId::move_from("0000000000000300"),
                tran_type_cd: TranTypeCode::move_from("03"),
                tran_cat_cd: "0005".parse::<TranCategoryCode>().expect("a category"),
                tran_source: Alnum::move_from("OPERATOR"),
                tran_desc: Alnum::move_from(&"D".repeat(100)),
                tran_amt: Money::from_cents(cents).expect("a 9-digit amount"),
                tran_merchant_id: "000000042".parse::<MerchantId>().expect("a merchant"),
                tran_merchant_name: Alnum::move_from(&"N".repeat(50)),
                tran_merchant_city: Alnum::move_from("Springfield"),
                tran_merchant_zip: Alnum::move_from("62701"),
                tran_card_num: CardNumber::move_from(CARD_A),
                tran_orig_ts: Alnum::move_from("2022-06-01 12:00:00.000000"),
                tran_proc_ts: Alnum::move_from("2022-06-02 08:30:00.000000"),
            },
        )
    }

    // P-COTRN02C-026: every field moved into its map field, the amount
    // through +99999999.99, which drops a ninth integer digit.
    #[test]
    fn copied_fields_fill_the_map_from_the_last_transaction() {
        let copied = copied_fields(&last_transaction(-150));
        let expected = TransactionAddInput {
            account_id: None,
            card_number: None,
            type_cd: "03".to_owned(),
            cat_cd: "0005".to_owned(),
            source: "OPERATOR  ".to_owned(),
            description: "D".repeat(60),
            amount: "-00000001.50".to_owned(),
            orig_date: "2022-06-01".to_owned(),
            proc_date: "2022-06-02".to_owned(),
            merchant_id: "000000042".to_owned(),
            merchant_name: "N".repeat(30),
            merchant_city: format!("Springfield{}", " ".repeat(14)),
            merchant_zip: "62701     ".to_owned(),
            confirm: Confirm::Blank,
        };
        assert_eq!(copied, expected);
        for (cents, amount) in [
            (1_000, "+00000010.00"),
            (0, "+00000000.00"),
            (12_345_678_900, "+23456789.00"),
        ] {
            assert_eq!(copied_fields(&last_transaction(cents)).amount, amount);
        }
    }

    // P-COTRN02C-026: PROCESS-ENTER-KEY follows the copy with the key fields
    // and confirmation the user entered.
    #[test]
    fn a_copied_transaction_passes_enter_and_builds_its_record() {
        let last = last_transaction(12_345_678_900);
        let input = TransactionAddInput {
            account_id: Some("00000000001".to_owned()),
            confirm: Confirm::Yes,
            ..copied_fields(&last)
        };
        let record = build(&accepted(input), tran_id());
        assert_eq!(record.tran_card_num, CardNumber::move_from(CARD_B));
        assert_eq!(
            record.tran_amt,
            Money::<9>::from_cents(2_345_678_900).expect("an amount")
        );
        assert_eq!(record.tran_desc, Alnum::move_from(&"D".repeat(60)));
        assert_eq!(record.tran_merchant_name, Alnum::move_from(&"N".repeat(30)));
        assert_eq!(record.tran_orig_ts, Alnum::move_from("2022-06-01"));
        assert_eq!(record.tran_proc_ts, Alnum::move_from("2022-06-02"));
        let unchanged = NewTransaction {
            tran_id: last.tran_id,
            tran_amt: last.tran_amt,
            tran_desc: last.tran_desc,
            tran_merchant_name: last.tran_merchant_name,
            tran_card_num: last.tran_card_num,
            tran_orig_ts: last.tran_orig_ts,
            tran_proc_ts: last.tran_proc_ts,
            ..record
        };
        assert_eq!(unchanged, last.to_new());
    }

    // Diagnostics never carry a full card number.
    #[test]
    fn debug_masks_the_entered_card_number() {
        let input = TransactionAddInput {
            card_number: Some(CARD_A.to_owned()),
            ..valid_input()
        };
        let debug = format!("{input:?}");
        assert!(!debug.contains(CARD_A), "{debug}");
        assert!(debug.contains("************5740"), "{debug}");
    }
}
