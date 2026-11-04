//! The wire types of the transaction resources: the CT00 list
//! `GET /transactions` (`COTRN00C`, map `COTRN00`), the CT01 view
//! `GET /transactions/{id}` (`COTRN01C`, map `COTRN01`) and the CT02 add
//! `POST /transactions` (`COTRN02C`, map `COTRN02`).
//!
//! # List
//!
//! The page parameters are the shared [`PageQuery`](crate::pagination::PageQuery):
//! its `start_key` is the `TRNIDIN` start key (`TRNIDINI X(16)`
//! [app/cpy-bms/COTRN00.CPY:L66]), whose numeric edit and message
//! (`Tran ID must be Numeric ...`, V-COTRN00C-004) belong to
//! `transaction_list::edit_start_key`. [`TransactionListQuery`] adds only the
//! optional account filter, extracted beside the page query and never
//! flattened into it. `COTRN00C` has no such filter: it browses every
//! transaction from a start key [app/cbl/COTRN00C.cbl:L206-360]. The filter
//! is the REST resource's addition and parity is measured without it (E-17).
//! A filter that is not 11 digits has no COBOL message (V-COTRN00C-001,
//! `no-message (E-49)`): 400 `INVALID` with an empty message and no field.
//!
//! # View
//!
//! The Tran ID is the path segment, as entered, and `transaction_view` edits
//! it (V-COTRN01C-001 to V-COTRN01C-003), so the view has no request type.
//! The validation audit records no over-length row for `TRNIDIN`: the rule
//! moves the key into `TRAN-ID X(16)` as the COBOL `MOVE` does.
//!
//! # Add
//!
//! [`TransactionAddRequest`] carries the 14 input fields of map `COTRN2A`
//! [app/cpy-bms/COTRN02.CPY:L60-138] as entered. [`TransactionAddRequest::check`]
//! applies the BMS field lengths and keeps the free-text fields ASCII and
//! storable; every other edit, with its audited message, is
//! `transaction_add::validate`'s (V-COTRN02C-001 to V-COTRN02C-028).
//!
//! The 3270 field cannot hold more characters than its width, so an
//! over-length entry has no COBOL path, and the validation audit records no
//! over-length row for `COTRN02C`: the length check answers 400 `INVALID` with
//! no message and names each over-length BMS field, the no-message answer of a
//! REST input without a COBOL counterpart (E-49).
//!
//! The source, the description and the merchant name, city and zip (`TRNSRC`,
//! `TDESC`, `MNAME`, `MCITY`, `MZIP`) have no class edit: `ADD-TRANSACTION`
//! moves them byte for byte into their `X(n)` record fields
//! [app/cbl/COTRN02C.cbl:L450-465], through `Alnum::move_from`, which would
//! store a non-ASCII character as its two to four UTF-8 bytes, each read back
//! as one Latin-1 character, and cut it at the field width. A 3270 map field
//! holds one byte per character, so such an entry has no COBOL counterpart
//! either, and the check answers it as it answers an over-length one: 400
//! `INVALID`, no message, its BMS field named (E-49). The same five fields
//! also refuse NUL characters beside other text, through
//! [`storable_text`](crate::dto::storable_text): the rule reads an entry of
//! NUL characters only as `LOW-VALUES` and answers it `can NOT be empty`
//! (V-COTRN02C-010, V-COTRN02C-011, V-COTRN02C-016 to V-COTRN02C-018), so
//! such an entry passes; any other entry the rule stores as written, and
//! `PostgreSQL` text cannot hold NUL (E-07). A 3270 field never sends NUL
//! characters mixed with other text, so that entry has no COBOL counterpart
//! and gets the same no-message answer (E-49). No other member reaches the
//! record unless its class or format edit passes it, and only ASCII digits,
//! signs, points and dashes pass those edits (V-COTRN02C-001,
//! V-COTRN02C-002, V-COTRN02C-019 to V-COTRN02C-023, V-COTRN02C-026); the
//! confirmation is never stored, and any value but `Y`, `y`, `N`, `n` or blank
//! is V-COTRN02C-028. The rule therefore answers a non-ASCII or NUL entry
//! there with its audited message, in its audited order.
//!
//! Every `COTRN02C` outcome other than the add sets `WS-ERR-FLG`, the `N` and
//! blank confirmations included (V-COTRN02C-027, V-COTRN02C-028), so the
//! program has no message-only path and [`TransactionAddResponse`] always
//! carries the added transaction.
//!
//! # Records
//!
//! [`TransactionResponse`] is the `CVTRA05Y` record [app/cpy/CVTRA05Y.cpy:L4-17]:
//! the list rows (P-COTRN00C-008), the view (P-COTRN01C-005) and the added
//! transaction. The amount is the exact scale-2 value; its screen edit
//! `+99999999.99` is presentation, made where it is shown.

use std::fmt;

use carddemo_domain::numeric::{AccountId, CardNumber};
use carddemo_domain::rules::transaction_add::TransactionAddInput;
use carddemo_domain::rules::transaction_list::TransactionListInput;
use carddemo_domain::rules::transaction_view::Transaction;
use carddemo_domain::rules::{Confirm, Direction, ErrorKind};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::dto::{BmsField, id_string, money, reject_invalid, text};
use crate::error::ApiError;

/// The message of an answer the validation audit records as
/// `no-message (E-49)`: the account filter (V-COTRN00C-001) and the
/// over-length, non-ASCII and mixed-NUL `COTRN02C` entries, none of which a
/// 3270 screen can send.
const NO_MESSAGE: &str = "";

/// The BMS field of a `COTRN00C` message (`MOVE -1 TO TRNIDINL`,
/// V-COTRN00C-002 to V-COTRN00C-013): the only value `fields[].field` takes
/// for `GET /transactions`.
///
/// The account filter's rejection names no field (V-COTRN00C-001, BMS field
/// `—`), so this enum has no variant for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TransactionListField {
    /// `TRNIDIN` (`TRNIDINI X(16)` [app/cpy-bms/COTRN00.CPY:L66]): the start
    /// key.
    #[serde(rename = "TRNIDIN")]
    TrnIdIn,
}

impl TransactionListField {
    /// The BMS field name, as `fields[].field` carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrnIdIn => "TRNIDIN",
        }
    }
}

impl BmsField for TransactionListField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The single field ranks first.
    fn rank(self) -> u16 {
        match self {
            Self::TrnIdIn => 1,
        }
    }
}

/// The BMS field of a `COTRN01C` message (`MOVE -1 TO TRNIDINL`,
/// V-COTRN01C-001 to V-COTRN01C-003): the only value `fields[].field` takes
/// for `GET /transactions/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TransactionViewField {
    /// `TRNIDIN` (`TRNIDINI X(16)` [app/cpy-bms/COTRN01.CPY:L60]): the Tran
    /// ID, the path segment.
    #[serde(rename = "TRNIDIN")]
    TrnIdIn,
}

impl TransactionViewField {
    /// The BMS field name, as `fields[].field` carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrnIdIn => "TRNIDIN",
        }
    }
}

impl BmsField for TransactionViewField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The single field ranks first (V-COTRN01C-001, Order 1).
    fn rank(self) -> u16 {
        match self {
            Self::TrnIdIn => 1,
        }
    }
}

/// The BMS fields a `COTRN02C` message or a failed length check of
/// `POST /transactions` names: the distinct `BMS field` values of the
/// V-COTRN02C rows, in screen order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TransactionAddField {
    /// `ACTIDIN` (`ACTIDINI X(11)`): the account ID.
    #[serde(rename = "ACTIDIN")]
    ActIdIn,
    /// `CARDNIN` (`CARDNINI X(16)`): the card number.
    #[serde(rename = "CARDNIN")]
    CardNIn,
    /// `TTYPCD` (`TTYPCDI X(2)`): the transaction type code.
    #[serde(rename = "TTYPCD")]
    TTypCd,
    /// `TCATCD` (`TCATCDI X(4)`): the transaction category code.
    #[serde(rename = "TCATCD")]
    TCatCd,
    /// `TRNSRC` (`TRNSRCI X(10)`): the source.
    #[serde(rename = "TRNSRC")]
    TrnSrc,
    /// `TDESC` (`TDESCI X(60)`): the description.
    #[serde(rename = "TDESC")]
    TDesc,
    /// `TRNAMT` (`TRNAMTI X(12)`): the amount.
    #[serde(rename = "TRNAMT")]
    TrnAmt,
    /// `TORIGDT` (`TORIGDTI X(10)`): the origination date.
    #[serde(rename = "TORIGDT")]
    TOrigDt,
    /// `TPROCDT` (`TPROCDTI X(10)`): the processing date.
    #[serde(rename = "TPROCDT")]
    TProcDt,
    /// `MID` (`MIDI X(9)`): the merchant ID.
    #[serde(rename = "MID")]
    Mid,
    /// `MNAME` (`MNAMEI X(30)`): the merchant name.
    #[serde(rename = "MNAME")]
    MName,
    /// `MCITY` (`MCITYI X(25)`): the merchant city.
    #[serde(rename = "MCITY")]
    MCity,
    /// `MZIP` (`MZIPI X(10)`): the merchant zip.
    #[serde(rename = "MZIP")]
    MZip,
    /// `CONFIRM` (`CONFIRMI X(1)`): the confirmation.
    #[serde(rename = "CONFIRM")]
    Confirm,
}

impl TransactionAddField {
    /// The BMS field name, as `fields[].field` carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActIdIn => "ACTIDIN",
            Self::CardNIn => "CARDNIN",
            Self::TTypCd => "TTYPCD",
            Self::TCatCd => "TCATCD",
            Self::TrnSrc => "TRNSRC",
            Self::TDesc => "TDESC",
            Self::TrnAmt => "TRNAMT",
            Self::TOrigDt => "TORIGDT",
            Self::TProcDt => "TPROCDT",
            Self::Mid => "MID",
            Self::MName => "MNAME",
            Self::MCity => "MCITY",
            Self::MZip => "MZIP",
            Self::Confirm => "CONFIRM",
        }
    }

    /// The field of the `POST /transactions` body member named `name`, the
    /// JSON name `validator` reports a failed length check under.
    fn of_member(name: &str) -> Option<Self> {
        match name {
            "account_id" => Some(Self::ActIdIn),
            "card_number" => Some(Self::CardNIn),
            "tran_type_cd" => Some(Self::TTypCd),
            "tran_cat_cd" => Some(Self::TCatCd),
            "tran_source" => Some(Self::TrnSrc),
            "tran_desc" => Some(Self::TDesc),
            "tran_amt" => Some(Self::TrnAmt),
            "tran_orig_date" => Some(Self::TOrigDt),
            "tran_proc_date" => Some(Self::TProcDt),
            "tran_merchant_id" => Some(Self::Mid),
            "tran_merchant_name" => Some(Self::MName),
            "tran_merchant_city" => Some(Self::MCity),
            "tran_merchant_zip" => Some(Self::MZip),
            "confirm" => Some(Self::Confirm),
            _ => None,
        }
    }
}

impl BmsField for TransactionAddField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The field's position in the `(Order, ID)` order that the validation
    /// audit's conventions give the V-COTRN02C rows, taken at the field's
    /// first row. Most data fields share Order 3 (the blank-field `EVALUATE`),
    /// where the lower ID, the screen position, comes first, so the ranks
    /// follow the screen.
    fn rank(self) -> u16 {
        match self {
            // Order 1: V-COTRN02C-001, V-COTRN02C-002.
            Self::ActIdIn => 1,
            Self::CardNIn => 2,
            // Order 3: V-COTRN02C-008 to V-COTRN02C-018.
            Self::TTypCd => 3,
            Self::TCatCd => 4,
            Self::TrnSrc => 5,
            Self::TDesc => 6,
            Self::TrnAmt => 7,
            Self::TOrigDt => 8,
            Self::TProcDt => 9,
            Self::Mid => 10,
            Self::MName => 11,
            Self::MCity => 12,
            Self::MZip => 13,
            // Order 11: V-COTRN02C-027.
            Self::Confirm => 14,
        }
    }
}

/// The answer to an account filter that is not 11 digits (V-COTRN00C-001,
/// E-49): 400 `INVALID`, no message and no field.
///
/// The row records the path as `no-message`, so this is the `no_message`
/// answer and not a deserialization `rejection` carrying a text of its own.
fn filter_rejected() -> ApiError {
    ApiError::no_message(ErrorKind::Invalid, NO_MESSAGE, Vec::new())
}

/// Whether `text` stands for no entry: empty, or `SPACES` or `LOW-VALUES`
/// only, as the start key beside it is read (`transaction_list::edit_start_key`).
fn is_blank(text: &str) -> bool {
    text.bytes().all(|byte| byte == b' ') || text.bytes().all(|byte| byte == 0)
}

/// The query parameter `GET /transactions` adds to the shared page query: the
/// optional account filter (E-17).
///
/// `COTRN00C` browses every transaction from a start key and has no account
/// filter [app/cbl/COTRN00C.cbl:L206-360]. The filter is the REST resource's
/// addition, used by the dashboard's latest rows; parity is measured on the
/// unfiltered browse. The start key and the paging are
/// [`PageQuery`](crate::pagination::PageQuery)'s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct TransactionListQuery {
    /// Only the transactions whose card the cross-reference maps to this
    /// account: 11 digits. Blank or absent lists every transaction.
    #[serde(default)]
    #[param(max_length = 11)]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
}

impl TransactionListQuery {
    /// The length check of the filter: at most the 11 digits of an account
    /// number.
    ///
    /// # Errors
    ///
    /// A longer `account_id` is answered as every malformed filter is
    /// (V-COTRN00C-001, `no-message (E-49)`): 400 `INVALID` with an empty
    /// message and no field, because the audit names no BMS field for it.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate().map_err(|_| filter_rejected())
    }

    /// The `transaction_list` input of the request: the resolved page
    /// parameters of [`PageQuery::resolve`](crate::pagination::PageQuery::resolve)
    /// and the typed account filter.
    ///
    /// An absent filter, or one of at most 11 characters that is empty or
    /// holds only spaces or only NUL characters, lists every transaction. The
    /// start key is passed on as entered, for the rule's own edit.
    ///
    /// # Errors
    ///
    /// A filter longer than 11 characters, blank ones included, fails
    /// [`check`](Self::check) before the blank test; any other non-blank
    /// filter that is not exactly 11 ASCII digits fails the digit test
    /// (V-COTRN00C-001, `no-message (E-49)`). Both answer 400 `INVALID` with an
    /// empty message and no field.
    pub fn into_input(
        self,
        start_key: Option<String>,
        direction: Direction,
        page_size: u16,
    ) -> Result<TransactionListInput, ApiError> {
        self.check()?;
        let account_id = match self.account_id.as_deref() {
            Some(entered) if !is_blank(entered) => Some(account_filter(entered)?),
            Some(_) | None => None,
        };
        Ok(TransactionListInput {
            start_key,
            direction,
            page_size,
            account_id,
        })
    }
}

/// The account of a non-blank filter: exactly the `9(11)` digits of
/// `ACCT-ID`, leading zeros included, as every account key is shown.
///
/// The width is tested before `AccountId::from_str`, which also takes fewer
/// digits, because V-COTRN00C-001 rejects every filter that is not 11 digits.
fn account_filter(entered: &str) -> Result<AccountId, ApiError> {
    let eleven_digits =
        entered.len() == AccountId::WIDTH && entered.bytes().all(|byte| byte.is_ascii_digit());
    if !eleven_digits {
        return Err(filter_rejected());
    }
    entered.parse().map_err(|_| filter_rejected())
}

/// One transaction master record, `CVTRA05Y TRAN-RECORD`
/// [app/cpy/CVTRA05Y.cpy:L4-17], as the list, the view and the add return it.
///
/// The fields are the 13 copybook fields, `FILLER` aside, under their
/// lower-snake-case copybook names. The row identity `transaction_id` that
/// the reward accruals reference is internal (E-09) and is not shown: the
/// COBOL key is `tran_id`.
///
/// It also deserializes, so a client decodes the bodies it receives into it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TransactionResponse {
    /// `TRAN-ID X(16)` (P-CVTRA05Y-001): the 16-digit key.
    pub tran_id: String,
    /// `TRAN-TYPE-CD X(02)` (P-CVTRA05Y-002).
    pub tran_type_cd: String,
    /// `TRAN-CAT-CD 9(04)` (P-CVTRA05Y-003): four digits, zero-padded.
    pub tran_cat_cd: String,
    /// `TRAN-SOURCE X(10)` (P-CVTRA05Y-004).
    pub tran_source: String,
    /// `TRAN-DESC X(100)` (P-CVTRA05Y-005).
    pub tran_desc: String,
    /// `TRAN-AMT S9(09)V99` (P-CVTRA05Y-006): a decimal string with two
    /// places, such as `-12.34`.
    #[serde(with = "rust_decimal::serde::str")]
    pub tran_amt: Decimal,
    /// `TRAN-MERCHANT-ID 9(09)` (P-CVTRA05Y-007): nine digits, zero-padded.
    pub tran_merchant_id: String,
    /// `TRAN-MERCHANT-NAME X(50)` (P-CVTRA05Y-008).
    pub tran_merchant_name: String,
    /// `TRAN-MERCHANT-CITY X(50)` (P-CVTRA05Y-009).
    pub tran_merchant_city: String,
    /// `TRAN-MERCHANT-ZIP X(10)` (P-CVTRA05Y-010).
    pub tran_merchant_zip: String,
    /// `TRAN-CARD-NUM X(16)` (P-CVTRA05Y-011).
    pub tran_card_num: String,
    /// `TRAN-ORIG-TS X(26)` (P-CVTRA05Y-012): the timestamp text as stored,
    /// without trailing spaces.
    pub tran_orig_ts: String,
    /// `TRAN-PROC-TS X(26)` (P-CVTRA05Y-013): the timestamp text as stored,
    /// without trailing spaces.
    pub tran_proc_ts: String,
}

impl From<&Transaction> for TransactionResponse {
    fn from(transaction: &Transaction) -> Self {
        Self {
            tran_id: id_string(&transaction.tran_id),
            tran_type_cd: id_string(&transaction.tran_type_cd),
            tran_cat_cd: id_string(&transaction.tran_cat_cd),
            tran_source: text(&transaction.tran_source),
            tran_desc: text(&transaction.tran_desc),
            tran_amt: money(transaction.tran_amt),
            tran_merchant_id: id_string(&transaction.tran_merchant_id),
            tran_merchant_name: text(&transaction.tran_merchant_name),
            tran_merchant_city: text(&transaction.tran_merchant_city),
            tran_merchant_zip: text(&transaction.tran_merchant_zip),
            tran_card_num: id_string(&transaction.tran_card_num),
            tran_orig_ts: text(&transaction.tran_orig_ts),
            tran_proc_ts: text(&transaction.tran_proc_ts),
        }
    }
}

/// The card number is shown masked, as `CardNumber`'s `Debug` shows it, so
/// diagnostics never carry a full card number.
impl fmt::Debug for TransactionResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransactionResponse")
            .field("tran_id", &self.tran_id)
            .field("tran_type_cd", &self.tran_type_cd)
            .field("tran_cat_cd", &self.tran_cat_cd)
            .field("tran_source", &self.tran_source)
            .field("tran_desc", &self.tran_desc)
            .field("tran_amt", &self.tran_amt)
            .field("tran_merchant_id", &self.tran_merchant_id)
            .field("tran_merchant_name", &self.tran_merchant_name)
            .field("tran_merchant_city", &self.tran_merchant_city)
            .field("tran_merchant_zip", &self.tran_merchant_zip)
            .field("tran_card_num", &CardNumber::move_from(&self.tran_card_num))
            .field("tran_orig_ts", &self.tran_orig_ts)
            .field("tran_proc_ts", &self.tran_proc_ts)
            .finish()
    }
}

/// The body of `POST /transactions`: the input fields of map `COTRN2A`
/// [app/cpy-bms/COTRN02.CPY:L60-138], each as entered.
///
/// A missing, `null` or empty member is the blank field the COBOL answers
/// with its `can NOT be empty` message (V-COTRN02C-003, V-COTRN02C-008 to
/// V-COTRN02C-018). The account ID is used when entered, otherwise the card
/// number. Every member is limited to its map field's width, and the source,
/// description and merchant name, city and zip to ASCII characters, with NUL
/// characters only as the whole entry; the formats are the rule's edits, so
/// that a malformed entry gets its audited message. The published `pattern`
/// of a code, amount, date or merchant ID is the form its edit accepts
/// (V-COTRN02C-019 to V-COTRN02C-023, V-COTRN02C-026); the dates' is their
/// shape only, the calendar check being `CSUTLDTC`'s (V-COTRN02C-024,
/// V-COTRN02C-025).
///
/// It also serializes, so a client builds the bodies it sends from it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct TransactionAddRequest {
    /// `ACTIDIN` (`ACTIDINI X(11)`): the account ID, 11 digits.
    #[serde(default)]
    #[schema(max_length = 11)]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
    /// `CARDNIN` (`CARDNINI X(16)`): the card number, 16 digits; read only
    /// when no account ID is entered.
    #[serde(default)]
    #[schema(max_length = 16)]
    #[validate(length(max = 16))]
    pub card_number: Option<String>,
    /// `TTYPCD` (`TTYPCDI X(2)`): the transaction type code, 2 digits.
    #[serde(default)]
    // V-COTRN02C-019: `TTYPCDI IS NUMERIC`.
    #[schema(max_length = 2, pattern = "^[0-9]{2}$")]
    #[validate(length(max = 2))]
    pub tran_type_cd: Option<String>,
    /// `TCATCD` (`TCATCDI X(4)`): the transaction category code, 4 digits.
    #[serde(default)]
    // V-COTRN02C-020: `TCATCDI IS NUMERIC`.
    #[schema(max_length = 4, pattern = "^[0-9]{4}$")]
    #[validate(length(max = 4))]
    pub tran_cat_cd: Option<String>,
    /// `TRNSRC` (`TRNSRCI X(10)`): the source, in ASCII without NUL
    /// characters, or NUL characters only, the `LOW-VALUES` answered as empty.
    #[serde(default)]
    #[schema(max_length = 10, pattern = r"^(?:\x00+|[\x01-\x7F]*)$")]
    #[validate(length(max = 10), custom(function = "crate::dto::storable_text"))]
    pub tran_source: Option<String>,
    /// `TDESC` (`TDESCI X(60)`): the description, in ASCII without NUL
    /// characters, or NUL characters only, the `LOW-VALUES` answered as empty.
    #[serde(default)]
    #[schema(max_length = 60, pattern = r"^(?:\x00+|[\x01-\x7F]*)$")]
    #[validate(length(max = 60), custom(function = "crate::dto::storable_text"))]
    pub tran_desc: Option<String>,
    /// `TRNAMT` (`TRNAMTI X(12)`): the amount as text in the form
    /// `-99999999.99`, a sign, eight digits, a point and two digits, such as
    /// `-00000012.34`.
    #[serde(default)]
    // V-COTRN02C-021: `(1:1)` sign, `(2:8)` digits, `(10:1)` point, `(11:2)`
    // digits.
    #[schema(max_length = 12, pattern = r"^[+-][0-9]{8}\.[0-9]{2}$")]
    #[validate(length(max = 12))]
    pub tran_amt: Option<String>,
    /// `TORIGDT` (`TORIGDTI X(10)`): the origination date, `YYYY-MM-DD`.
    #[serde(default)]
    // V-COTRN02C-022: digits and dashes in the `YYYY-MM-DD` positions only.
    #[schema(max_length = 10, pattern = "^[0-9]{4}-[0-9]{2}-[0-9]{2}$")]
    #[validate(length(max = 10))]
    pub tran_orig_date: Option<String>,
    /// `TPROCDT` (`TPROCDTI X(10)`): the processing date, `YYYY-MM-DD`.
    #[serde(default)]
    // V-COTRN02C-023: digits and dashes in the `YYYY-MM-DD` positions only.
    #[schema(max_length = 10, pattern = "^[0-9]{4}-[0-9]{2}-[0-9]{2}$")]
    #[validate(length(max = 10))]
    pub tran_proc_date: Option<String>,
    /// `MID` (`MIDI X(9)`): the merchant ID, 9 digits.
    #[serde(default)]
    // V-COTRN02C-026: `MIDI IS NUMERIC`.
    #[schema(max_length = 9, pattern = "^[0-9]{9}$")]
    #[validate(length(max = 9))]
    pub tran_merchant_id: Option<String>,
    /// `MNAME` (`MNAMEI X(30)`): the merchant name, in ASCII without NUL
    /// characters, or NUL characters only, the `LOW-VALUES` answered as empty.
    #[serde(default)]
    #[schema(max_length = 30, pattern = r"^(?:\x00+|[\x01-\x7F]*)$")]
    #[validate(length(max = 30), custom(function = "crate::dto::storable_text"))]
    pub tran_merchant_name: Option<String>,
    /// `MCITY` (`MCITYI X(25)`): the merchant city, in ASCII without NUL
    /// characters, or NUL characters only, the `LOW-VALUES` answered as empty.
    #[serde(default)]
    #[schema(max_length = 25, pattern = r"^(?:\x00+|[\x01-\x7F]*)$")]
    #[validate(length(max = 25), custom(function = "crate::dto::storable_text"))]
    pub tran_merchant_city: Option<String>,
    /// `MZIP` (`MZIPI X(10)`): the merchant zip, in ASCII without NUL
    /// characters, or NUL characters only, the `LOW-VALUES` answered as empty.
    #[serde(default)]
    #[schema(max_length = 10, pattern = r"^(?:\x00+|[\x01-\x7F]*)$")]
    #[validate(length(max = 10), custom(function = "crate::dto::storable_text"))]
    pub tran_merchant_zip: Option<String>,
    /// `CONFIRM` (`CONFIRMI X(1)`): `Y` adds the transaction; `N` or blank asks
    /// for confirmation (V-COTRN02C-027) and any other value is rejected
    /// (V-COTRN02C-028), both without a write.
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1))]
    pub confirm: Option<String>,
}

impl TransactionAddRequest {
    /// The checks of the entry as a 3270 map could hold it: every member
    /// within its map field's width, and the five free-text members the rule
    /// moves byte for byte into `X(n)` fields (`TRNSRC`, `TDESC`, `MNAME`,
    /// `MCITY`, `MZIP`) ASCII and storable, through
    /// [`storable_text`](crate::dto::storable_text): NUL characters only as
    /// the whole entry.
    ///
    /// Blank members, `SPACES` or `LOW-VALUES` of any length within the width
    /// included, pass: the rule answers them with their `can NOT be empty`
    /// messages (V-COTRN02C-008 to V-COTRN02C-018).
    ///
    /// # Errors
    ///
    /// 400 `INVALID` with an empty message, the audit having no over-length,
    /// non-ASCII or mixed-NUL row for `COTRN02C` (E-49), and one
    /// [`TransactionAddField`] entry per member that is longer than its width
    /// or, among the five, holds a non-ASCII character or a NUL character
    /// beside other text, in screen order.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, TransactionAddField::of_member, NO_MESSAGE))
    }

    /// The `transaction_add` input of the request: every member as entered,
    /// absent members blank, and the confirmation classified by
    /// [`Confirm::parse`].
    #[must_use]
    pub fn into_input(self) -> TransactionAddInput {
        TransactionAddInput {
            account_id: self.account_id,
            card_number: self.card_number,
            type_cd: self.tran_type_cd.unwrap_or_default(),
            cat_cd: self.tran_cat_cd.unwrap_or_default(),
            source: self.tran_source.unwrap_or_default(),
            description: self.tran_desc.unwrap_or_default(),
            amount: self.tran_amt.unwrap_or_default(),
            orig_date: self.tran_orig_date.unwrap_or_default(),
            proc_date: self.tran_proc_date.unwrap_or_default(),
            merchant_id: self.tran_merchant_id.unwrap_or_default(),
            merchant_name: self.tran_merchant_name.unwrap_or_default(),
            merchant_city: self.tran_merchant_city.unwrap_or_default(),
            merchant_zip: self.tran_merchant_zip.unwrap_or_default(),
            confirm: Confirm::parse(self.confirm.as_deref()),
        }
    }
}

/// The entered card number is shown masked, as `CardNumber`'s `Debug` shows
/// it, so diagnostics never carry a full card number.
impl fmt::Debug for TransactionAddRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransactionAddRequest")
            .field("account_id", &self.account_id)
            .field(
                "card_number",
                &self.card_number.as_deref().map(CardNumber::move_from),
            )
            .field("tran_type_cd", &self.tran_type_cd)
            .field("tran_cat_cd", &self.tran_cat_cd)
            .field("tran_source", &self.tran_source)
            .field("tran_desc", &self.tran_desc)
            .field("tran_amt", &self.tran_amt)
            .field("tran_orig_date", &self.tran_orig_date)
            .field("tran_proc_date", &self.tran_proc_date)
            .field("tran_merchant_id", &self.tran_merchant_id)
            .field("tran_merchant_name", &self.tran_merchant_name)
            .field("tran_merchant_city", &self.tran_merchant_city)
            .field("tran_merchant_zip", &self.tran_merchant_zip)
            .field("confirm", &self.confirm)
            .finish()
    }
}

/// The 201 body of `POST /transactions`: the added transaction and the
/// `COTRN02C` success text, `Transaction added successfully.  Your Tran ID is
/// <TRAN-ID>.` (P-COTRN02C-024).
///
/// The transaction is always present: `COTRN02C` sets `WS-ERR-FLG` on every
/// other outcome, the `N` and blank confirmations included (V-COTRN02C-027),
/// so the add has no message-only 200 answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TransactionAddResponse {
    /// The transaction written.
    pub transaction: TransactionResponse,
    /// The success text, byte-identical to the COBOL message.
    pub message: String,
}

impl TransactionAddResponse {
    /// The answer to a committed add of `transaction`, with the success text
    /// `transaction_add::write_result` returned.
    #[must_use]
    pub fn added(transaction: &Transaction, message: String) -> Self {
        Self {
            transaction: TransactionResponse::from(transaction),
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::entities::{CardXref, Transaction};
    use carddemo_domain::error::DomainError;
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money, TranCategoryCode, TranId,
        TranTypeCode, TransactionRowId,
    };
    use carddemo_domain::ports::{Direction, PortError, PortResult, XrefReader};
    use carddemo_domain::rules::transaction_add::{self, TransactionAddInput};
    use carddemo_domain::rules::transaction_list::{self, TransactionListInput};
    use carddemo_domain::rules::{Confirm, transaction_view};
    use serde_json::{Map, Value, json};
    use utoipa::PartialSchema;

    use super::{
        TransactionAddField, TransactionAddRequest, TransactionAddResponse, TransactionListField,
        TransactionListQuery, TransactionResponse, TransactionViewField,
    };
    use crate::dto::BmsField;
    use crate::error::ApiError;
    use crate::pagination::{PageDirection, PageQuery};

    type TestResult = Result<(), Box<dyn Error>>;

    /// `(JSON member, BMS field, map width)` of every `COTRN2A` input field,
    /// in screen order [app/cpy-bms/COTRN02.CPY:L60-138].
    const ADD_MEMBERS: [(&str, &str, usize); 14] = [
        ("account_id", "ACTIDIN", 11),
        ("card_number", "CARDNIN", 16),
        ("tran_type_cd", "TTYPCD", 2),
        ("tran_cat_cd", "TCATCD", 4),
        ("tran_source", "TRNSRC", 10),
        ("tran_desc", "TDESC", 60),
        ("tran_amt", "TRNAMT", 12),
        ("tran_orig_date", "TORIGDT", 10),
        ("tran_proc_date", "TPROCDT", 10),
        ("tran_merchant_id", "MID", 9),
        ("tran_merchant_name", "MNAME", 30),
        ("tran_merchant_city", "MCITY", 25),
        ("tran_merchant_zip", "MZIP", 10),
        ("confirm", "CONFIRM", 1),
    ];

    /// The `pattern` of a free-text member: ASCII text without NUL characters,
    /// or NUL characters only, the `LOW-VALUES` the rule answers as blank.
    const STORABLE_TEXT: &str = r"^(?:\x00+|[\x01-\x7F]*)$";

    /// The `pattern` the API document publishes for each of the
    /// [`ADD_MEMBERS`], in the same order: the form the class or format edit
    /// accepts (V-COTRN02C-019 to V-COTRN02C-023, V-COTRN02C-026),
    /// [`STORABLE_TEXT`] for the free-text members, and none for the key
    /// fields and the confirmation, whose edits depend on another member or
    /// answer a blank or `N` with a message.
    const ADD_PATTERNS: [(&str, Option<&str>); 14] = [
        ("account_id", None),
        ("card_number", None),
        ("tran_type_cd", Some("^[0-9]{2}$")),
        ("tran_cat_cd", Some("^[0-9]{4}$")),
        ("tran_source", Some(STORABLE_TEXT)),
        ("tran_desc", Some(STORABLE_TEXT)),
        ("tran_amt", Some(r"^[+-][0-9]{8}\.[0-9]{2}$")),
        ("tran_orig_date", Some("^[0-9]{4}-[0-9]{2}-[0-9]{2}$")),
        ("tran_proc_date", Some("^[0-9]{4}-[0-9]{2}-[0-9]{2}$")),
        ("tran_merchant_id", Some("^[0-9]{9}$")),
        ("tran_merchant_name", Some(STORABLE_TEXT)),
        ("tran_merchant_city", Some(STORABLE_TEXT)),
        ("tran_merchant_zip", Some(STORABLE_TEXT)),
        ("confirm", None),
    ];

    /// The [`ADD_MEMBERS`] the rule moves byte for byte into `X(n)` record
    /// fields without a class edit, which the check keeps ASCII and free of
    /// NUL characters unless they are the whole entry.
    const ASCII_MEMBERS: [(&str, &str, usize); 5] = [
        ("tran_source", "TRNSRC", 10),
        ("tran_desc", "TDESC", 60),
        ("tran_merchant_name", "MNAME", 30),
        ("tran_merchant_city", "MCITY", 25),
        ("tran_merchant_zip", "MZIP", 10),
    ];

    /// Every add field with the domain constant naming its BMS field, in
    /// screen order.
    const ADD_FIELDS: [(TransactionAddField, &str); 14] = [
        (TransactionAddField::ActIdIn, transaction_add::FIELD_ACTIDIN),
        (TransactionAddField::CardNIn, transaction_add::FIELD_CARDNIN),
        (TransactionAddField::TTypCd, transaction_add::FIELD_TTYPCD),
        (TransactionAddField::TCatCd, transaction_add::FIELD_TCATCD),
        (TransactionAddField::TrnSrc, transaction_add::FIELD_TRNSRC),
        (TransactionAddField::TDesc, transaction_add::FIELD_TDESC),
        (TransactionAddField::TrnAmt, transaction_add::FIELD_TRNAMT),
        (TransactionAddField::TOrigDt, transaction_add::FIELD_TORIGDT),
        (TransactionAddField::TProcDt, transaction_add::FIELD_TPROCDT),
        (TransactionAddField::Mid, transaction_add::FIELD_MID),
        (TransactionAddField::MName, transaction_add::FIELD_MNAME),
        (TransactionAddField::MCity, transaction_add::FIELD_MCITY),
        (TransactionAddField::MZip, transaction_add::FIELD_MZIP),
        (TransactionAddField::Confirm, transaction_add::FIELD_CONFIRM),
    ];

    /// The full card number of [`stored`], which no `Debug` output may show.
    const CARD: &str = "4859452612877065";

    /// A stored transaction shaped as a CT02 add writes it: the dates of the
    /// map, the timestamps' trailing spaces included (P-COTRN02C-023).
    fn stored(amount_cents: i64) -> Result<Transaction, Box<dyn Error>> {
        Ok(Transaction {
            transaction_id: TransactionRowId::from(42),
            tran_id: TranId::move_from("0000000000683580"),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: TranCategoryCode::new(1)?,
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: Money::from_cents(amount_cents)?,
            tran_merchant_id: MerchantId::new(7)?,
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from(CARD),
            tran_orig_ts: Alnum::move_from("2022-06-10"),
            tran_proc_ts: Alnum::move_from("2022-07-01-12.00.00.000000"),
        })
    }

    /// The JSON of [`stored`] with amount `amount`.
    fn stored_json(amount: &str) -> Value {
        json!({
            "tran_id": "0000000000683580",
            "tran_type_cd": "01",
            "tran_cat_cd": "0001",
            "tran_source": "POS TERM",
            "tran_desc": "Purchase at Abshire-Lowe",
            "tran_amt": amount,
            "tran_merchant_id": "000000007",
            "tran_merchant_name": "Abshire-Lowe",
            "tran_merchant_city": "North Enoshaven",
            "tran_merchant_zip": "72112",
            "tran_card_num": CARD,
            "tran_orig_ts": "2022-06-10",
            "tran_proc_ts": "2022-07-01-12.00.00.000000",
        })
    }

    /// An add request whose members are `members`, each set to `value`.
    fn request_with(
        members: &[&str],
        value: &str,
    ) -> Result<TransactionAddRequest, Box<dyn Error>> {
        let body: Map<String, Value> = members
            .iter()
            .map(|member| ((*member).to_owned(), Value::from(value)))
            .collect();
        Ok(serde_json::from_value(Value::Object(body))?)
    }

    /// The error body of `error` as JSON, with its status.
    fn answer(error: &ApiError) -> Result<(StatusCode, Value), Box<dyn Error>> {
        Ok((error.status(), serde_json::to_value(error.body())?))
    }

    #[test]
    fn response_carries_the_13_record_fields_as_text() -> TestResult {
        // (cents, wire amount)
        let amounts = [
            (-1_234, "-12.34"),
            (0, "0.00"),
            (50_477, "504.77"),
            (99_999_999_999, "999999999.99"),
        ];
        for (cents, amount) in amounts {
            let response = TransactionResponse::from(&stored(cents)?);
            let value = serde_json::to_value(&response)?;
            assert_eq!(value, stored_json(amount), "{cents}");
            let members = value.as_object().map_or(0, Map::len);
            assert_eq!(members, 13);
            assert!(value.get("transaction_id").is_none());
            assert_eq!(
                serde_json::from_value::<TransactionResponse>(value)?,
                response
            );
        }
        Ok(())
    }

    #[test]
    fn added_response_carries_the_transaction_and_the_success_text() -> TestResult {
        let message = messages::transaction_added_message("0000000000683580");
        let response = TransactionAddResponse::added(&stored(1_250)?, message.clone());
        assert_eq!(
            serde_json::to_value(&response)?,
            json!({ "transaction": stored_json("12.50"), "message": message })
        );
        Ok(())
    }

    #[test]
    fn debug_masks_the_card_number() -> TestResult {
        let request = request_with(&["card_number"], CARD)?;
        let response = TransactionResponse::from(&stored(1)?);
        let added = TransactionAddResponse::added(&stored(1)?, String::new());
        for shown in [
            format!("{request:?}"),
            format!("{response:?}"),
            format!("{added:?}"),
        ] {
            assert!(!shown.contains(CARD), "{shown}");
            assert!(shown.contains("************7065"), "{shown}");
        }
        Ok(())
    }

    #[test]
    fn add_check_names_each_member_over_its_map_width() -> TestResult {
        for (member, field, width) in ADD_MEMBERS {
            assert!(request_with(&[member], &"9".repeat(width))?.check().is_ok());
            let error = request_with(&[member], &"9".repeat(width + 1))?
                .check()
                .expect_err(member);
            assert_eq!(
                answer(&error)?,
                (
                    StatusCode::BAD_REQUEST,
                    json!({
                        "code": "INVALID",
                        "message": "",
                        "fields": [{ "field": field, "message": "" }],
                    })
                ),
                "{member}"
            );
        }
        Ok(())
    }

    #[test]
    fn add_check_keeps_the_free_text_members_ascii_up_to_their_width() -> TestResult {
        for (member, _, width) in ASCII_MEMBERS {
            let printable: String = (b' '..=b'~').map(char::from).cycle().take(width).collect();
            for accepted in [printable, "\u{7f}".to_owned(), "Cafe".to_owned()] {
                assert!(
                    request_with(&[member], &accepted)?.check().is_ok(),
                    "{member} {accepted:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn add_check_names_each_free_text_member_holding_non_ascii_or_mixed_nul_text() -> TestResult {
        for (member, field, width) in ASCII_MEMBERS {
            // Accented Latin-1, the first non-ASCII code point, non-Latin-1,
            // outside the BMP, inside ASCII text, and a width of `é` that fits
            // the length check but not the field's bytes; then NUL characters
            // beside other text, which the rule would store and PostgreSQL
            // text cannot hold: inside, leading, trailing, before and after a
            // space, and a full width with one other character.
            let rejected = [
                "\u{e9}".to_owned(),
                "\u{80}".to_owned(),
                "\u{20ac}".to_owned(),
                "\u{1f600}".to_owned(),
                "Caf\u{e9}".to_owned(),
                "\u{e9}".repeat(width),
                "A\0B".to_owned(),
                "\0A".to_owned(),
                "A\0".to_owned(),
                "\0 ".to_owned(),
                " \0".to_owned(),
                format!("{}A", "\0".repeat(width - 1)),
            ];
            for entered in rejected {
                let error = request_with(&[member], &entered)?
                    .check()
                    .expect_err(member);
                assert_eq!(
                    answer(&error)?,
                    (
                        StatusCode::BAD_REQUEST,
                        json!({
                            "code": "INVALID",
                            "message": "",
                            "fields": [{ "field": field, "message": "" }],
                        })
                    ),
                    "{member} {entered:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn add_check_lists_non_ascii_and_over_length_fields_together_in_screen_order() -> TestResult {
        let request = TransactionAddRequest {
            account_id: Some("9".repeat(12)),
            tran_type_cd: Some("\u{e9}".to_owned()),
            tran_merchant_zip: Some("\u{e9}".to_owned()),
            tran_merchant_city: Some("\u{20ac}".to_owned()),
            tran_merchant_name: Some("\u{e9}".repeat(31)),
            tran_desc: Some("9".repeat(61)),
            tran_source: Some("\u{1f600}".to_owned()),
            ..TransactionAddRequest::default()
        };
        let error = request.check().expect_err("six members fail");
        let (status, body) = answer(&error)?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"], json!(""));
        // `TTYPCD` is not listed: its class edit answers "é" (V-COTRN02C-019).
        let expected: Vec<Value> = ["ACTIDIN", "TRNSRC", "TDESC", "MNAME", "MCITY", "MZIP"]
            .iter()
            .map(|field| json!({ "field": field, "message": "" }))
            .collect();
        assert_eq!(body["fields"], Value::Array(expected));
        Ok(())
    }

    #[test]
    fn add_check_passes_blank_free_text_members_to_the_rule() -> TestResult {
        for (member, _, width) in ASCII_MEMBERS {
            let blanks = [
                String::new(),
                " ".repeat(width),
                "\0".to_owned(),
                "\0\0".to_owned(),
                "\0".repeat(width),
            ];
            for blank in blanks {
                assert!(
                    request_with(&[member], &blank)?.check().is_ok(),
                    "{member} {blank:?}"
                );
            }
        }
        Ok(())
    }

    /// The body of an add of account 1 that every `COTRN02C` edit accepts.
    fn valid_add_body() -> Value {
        json!({
            "account_id": "00000000001",
            "tran_type_cd": "01",
            "tran_cat_cd": "0001",
            "tran_source": "POS TERM",
            "tran_desc": "Purchase at Abshire-Lowe",
            "tran_amt": "-00000012.34",
            "tran_orig_date": "2022-07-01",
            "tran_proc_date": "2022-07-02",
            "tran_merchant_id": "800000000",
            "tran_merchant_name": "Abshire-Lowe",
            "tran_merchant_city": "North Enoshaven",
            "tran_merchant_zip": "72112",
            "confirm": "Y",
        })
    }

    /// A cross-reference file of one record, the rule test's: account 1 and
    /// card [`CARD`].
    struct OneXref(CardXref);

    impl XrefReader for OneXref {
        async fn read_xref_by_card(&mut self, card: CardNumber) -> PortResult<CardXref> {
            if card == self.0.xref_card_num {
                Ok(self.0.clone())
            } else {
                Err(PortError::NotFound)
            }
        }

        async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
            if id == self.0.xref_acct_id {
                Ok(self.0.clone())
            } else {
                Err(PortError::NotFound)
            }
        }

        async fn xrefs_after(
            &mut self,
            _after: Option<CardNumber>,
            _limit: u16,
        ) -> PortResult<Vec<CardXref>> {
            Ok(vec![self.0.clone()])
        }
    }

    // V-COTRN02C-001, V-COTRN02C-002, V-COTRN02C-019 to V-COTRN02C-023,
    // V-COTRN02C-026, V-COTRN02C-028: the members with a class or format edit
    // need no ASCII check, because that edit rejects a non-ASCII entry.
    #[tokio::test]
    async fn add_rule_answers_non_ascii_format_members_with_their_audited_message() -> TestResult {
        let mut store = OneXref(CardXref {
            xref_card_num: CardNumber::move_from(CARD),
            xref_cust_id: CustomerId::new(1)?,
            xref_acct_id: AccountId::new(1)?,
        });
        let valid = valid_add_body();
        let request: TransactionAddRequest = serde_json::from_value(valid.clone())?;
        request.check()?;
        assert!(
            transaction_add::validate(&mut store, request.into_input())
                .await
                .is_ok()
        );
        // (member, value, message, BMS field)
        let cases = [
            (
                "account_id",
                Value::from("\u{e9}"),
                messages::ACCOUNT_ID_MUST_BE_NUMERIC,
                transaction_add::FIELD_ACTIDIN,
            ),
            (
                "card_number",
                Value::from("\u{20ac}"),
                messages::CARD_NUMBER_MUST_BE_NUMERIC,
                transaction_add::FIELD_CARDNIN,
            ),
            (
                "tran_type_cd",
                Value::from("\u{e9}"),
                messages::TYPE_CD_MUST_BE_NUMERIC,
                transaction_add::FIELD_TTYPCD,
            ),
            (
                "tran_cat_cd",
                Value::from("00\u{e9}"),
                messages::CATEGORY_CD_MUST_BE_NUMERIC,
                transaction_add::FIELD_TCATCD,
            ),
            (
                "tran_amt",
                Value::from("-0000001\u{e9}.34"),
                messages::AMOUNT_SHOULD_BE_IN_FORMAT,
                transaction_add::FIELD_TRNAMT,
            ),
            (
                "tran_orig_date",
                Value::from("2022-07-\u{e9}"),
                messages::ORIG_DATE_SHOULD_BE_IN_FORMAT,
                transaction_add::FIELD_TORIGDT,
            ),
            (
                "tran_proc_date",
                Value::from("\u{1f600}"),
                messages::PROC_DATE_SHOULD_BE_IN_FORMAT,
                transaction_add::FIELD_TPROCDT,
            ),
            (
                "tran_merchant_id",
                Value::from("80000000\u{e9}"),
                messages::MERCHANT_ID_MUST_BE_NUMERIC,
                transaction_add::FIELD_MID,
            ),
            (
                "confirm",
                Value::from("\u{e9}"),
                messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
                transaction_add::FIELD_CONFIRM,
            ),
        ];
        for (member, value, message, field) in cases {
            let mut body = valid.clone();
            body[member] = value;
            if member == "card_number" {
                // The card number is edited only without an account ID.
                body["account_id"] = Value::from("");
            }
            let request: TransactionAddRequest = serde_json::from_value(body)?;
            request.check()?;
            let error = transaction_add::validate(&mut store, request.into_input())
                .await
                .expect_err(member);
            assert_eq!(
                error,
                DomainError::invalid(message).with_field(field),
                "{member}"
            );
        }
        Ok(())
    }

    // V-COTRN02C-010, V-COTRN02C-011, V-COTRN02C-016 to V-COTRN02C-018: NUL
    // characters alone pass the check and reach the rule as `LOW-VALUES`,
    // which answers them with the field's empty-field message; NUL characters
    // beside other text fail the check, which the handler runs first, so they
    // never reach the rule.
    #[tokio::test]
    async fn add_rule_answers_all_nul_free_text_as_empty_and_the_check_stops_mixed_nul()
    -> TestResult {
        let mut store = OneXref(CardXref {
            xref_card_num: CardNumber::move_from(CARD),
            xref_cust_id: CustomerId::new(1)?,
            xref_acct_id: AccountId::new(1)?,
        });
        // (member, message, BMS field), in the order of `ASCII_MEMBERS`
        let empty_answers = [
            (
                "tran_source",
                messages::SOURCE_CAN_NOT_BE_EMPTY,
                transaction_add::FIELD_TRNSRC,
            ),
            (
                "tran_desc",
                messages::DESCRIPTION_CAN_NOT_BE_EMPTY,
                transaction_add::FIELD_TDESC,
            ),
            (
                "tran_merchant_name",
                messages::MERCHANT_NAME_CAN_NOT_BE_EMPTY,
                transaction_add::FIELD_MNAME,
            ),
            (
                "tran_merchant_city",
                messages::MERCHANT_CITY_CAN_NOT_BE_EMPTY,
                transaction_add::FIELD_MCITY,
            ),
            (
                "tran_merchant_zip",
                messages::MERCHANT_ZIP_CAN_NOT_BE_EMPTY,
                transaction_add::FIELD_MZIP,
            ),
        ];
        for ((member, field, width), (answered, message, rule_field)) in
            ASCII_MEMBERS.into_iter().zip(empty_answers)
        {
            assert_eq!(member, answered);
            for low_values in ["\0".to_owned(), "\0".repeat(width)] {
                let mut body = valid_add_body();
                body[member] = Value::from(low_values.as_str());
                let request: TransactionAddRequest = serde_json::from_value(body)?;
                request.check()?;
                let error = transaction_add::validate(&mut store, request.into_input())
                    .await
                    .expect_err(member);
                assert_eq!(
                    error,
                    DomainError::empty(message).with_field(rule_field),
                    "{member} {low_values:?}"
                );
            }
            let mut body = valid_add_body();
            body[member] = Value::from("A\0B");
            let request: TransactionAddRequest = serde_json::from_value(body)?;
            let error = request.check().expect_err(member);
            assert_eq!(
                answer(&error)?,
                (
                    StatusCode::BAD_REQUEST,
                    json!({
                        "code": "INVALID",
                        "message": "",
                        "fields": [{ "field": field, "message": "" }],
                    })
                ),
                "{member}"
            );
        }
        Ok(())
    }

    #[test]
    fn add_check_lists_every_over_length_field_in_screen_order() -> TestResult {
        let members: Vec<&str> = ADD_MEMBERS.iter().map(|(member, ..)| *member).collect();
        let error = request_with(&members, &"9".repeat(61))?
            .check()
            .expect_err("every member is too long");
        let (status, body) = answer(&error)?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let expected: Vec<Value> = ADD_MEMBERS
            .iter()
            .map(|(_, field, _)| json!({ "field": field, "message": "" }))
            .collect();
        assert_eq!(body["fields"], Value::Array(expected));
        assert!(TransactionAddRequest::default().check().is_ok());
        Ok(())
    }

    #[test]
    fn add_schema_publishes_each_map_width_and_requires_nothing() -> TestResult {
        let schema = serde_json::to_value(TransactionAddRequest::schema())?;
        for ((member, _, width), (patterned, pattern)) in ADD_MEMBERS.into_iter().zip(ADD_PATTERNS)
        {
            assert_eq!(patterned, member);
            let property = &schema["properties"][member];
            assert_eq!(property["maxLength"], json!(width), "{member}");
            assert_eq!(
                property.get("pattern"),
                pattern.map(Value::from).as_ref(),
                "{member}"
            );
        }
        let properties = schema["properties"].as_object().map_or(0, Map::len);
        assert_eq!(properties, ADD_MEMBERS.len());
        assert!(schema.get("required").is_none());
        Ok(())
    }

    #[test]
    fn add_into_input_moves_every_member_as_entered() {
        let request = TransactionAddRequest {
            account_id: Some("00000000001".to_owned()),
            card_number: None,
            tran_type_cd: Some("01".to_owned()),
            tran_cat_cd: Some("0001".to_owned()),
            tran_source: Some(" POS".to_owned()),
            tran_desc: Some("Purchase ".to_owned()),
            tran_amt: Some("-00000012.34".to_owned()),
            tran_orig_date: Some("2022-07-01".to_owned()),
            tran_proc_date: Some(String::new()),
            tran_merchant_id: Some("800000000".to_owned()),
            tran_merchant_name: Some("Abshire-Lowe".to_owned()),
            tran_merchant_city: None,
            tran_merchant_zip: Some("72112".to_owned()),
            confirm: Some("y".to_owned()),
        };
        assert_eq!(
            request.into_input(),
            TransactionAddInput {
                account_id: Some("00000000001".to_owned()),
                card_number: None,
                type_cd: "01".to_owned(),
                cat_cd: "0001".to_owned(),
                source: " POS".to_owned(),
                description: "Purchase ".to_owned(),
                amount: "-00000012.34".to_owned(),
                orig_date: "2022-07-01".to_owned(),
                proc_date: String::new(),
                merchant_id: "800000000".to_owned(),
                merchant_name: "Abshire-Lowe".to_owned(),
                merchant_city: String::new(),
                merchant_zip: "72112".to_owned(),
                confirm: Confirm::Yes,
            }
        );
    }

    #[test]
    fn add_into_input_classifies_the_confirmation_through_confirm_parse() {
        for entered in [
            None,
            Some(""),
            Some(" "),
            Some("Y"),
            Some("y"),
            Some("N"),
            Some("n"),
            Some("X"),
        ] {
            let request = TransactionAddRequest {
                confirm: entered.map(str::to_owned),
                ..TransactionAddRequest::default()
            };
            assert_eq!(
                request.into_input().confirm,
                Confirm::parse(entered),
                "{entered:?}"
            );
        }
        let other = TransactionAddRequest {
            confirm: Some("X".to_owned()),
            ..TransactionAddRequest::default()
        };
        assert_eq!(other.into_input().confirm, Confirm::Other("X".to_owned()));
    }

    #[test]
    fn list_filter_absent_or_blank_lists_every_transaction() -> TestResult {
        for entered in [
            None,
            Some(""),
            Some("   "),
            Some("\0\0"),
            Some("           "),
            Some("\0\0\0\0\0\0\0\0\0\0\0"),
        ] {
            let query = TransactionListQuery {
                account_id: entered.map(str::to_owned),
            };
            assert!(query.check().is_ok());
            let input =
                query.into_input(Some("0000000000000005".to_owned()), Direction::Backward, 5)?;
            assert_eq!(
                input,
                TransactionListInput {
                    start_key: Some("0000000000000005".to_owned()),
                    direction: Direction::Backward,
                    page_size: 5,
                    account_id: None,
                },
                "{entered:?}"
            );
        }
        let filtered = TransactionListQuery {
            account_id: Some("00000000001".to_owned()),
        }
        .into_input(None, Direction::Forward, transaction_list::SCREEN_ROWS)?;
        assert_eq!(filtered.account_id, Some(AccountId::new(1)?));
        assert_eq!(filtered.start_key, None);
        Ok(())
    }

    #[test]
    fn list_filter_not_eleven_digits_is_400_without_message() -> TestResult {
        let rejected = (
            StatusCode::BAD_REQUEST,
            json!({ "code": "INVALID", "message": "" }),
        );
        for entered in [
            "abc",
            "0000000000A",
            "1",
            "000000000001",
            " 0000000001",
            "0000000001 ",
            "+0000000001",
            "0000000000١",
            "            ",
            "\0\0\0\0\0\0\0\0\0\0\0\0",
        ] {
            let query = TransactionListQuery {
                account_id: Some(entered.to_owned()),
            };
            let error = query
                .into_input(None, Direction::Forward, transaction_list::SCREEN_ROWS)
                .expect_err(entered);
            assert_eq!(answer(&error)?, rejected, "{entered}");
        }
        let too_long = TransactionListQuery {
            account_id: Some("000000000001".to_owned()),
        };
        assert_eq!(answer(&too_long.check().expect_err("12 digits"))?, rejected);
        Ok(())
    }

    #[test]
    fn list_query_and_page_query_read_the_same_query_string() -> TestResult {
        let uri: Uri = concat!(
            "/transactions?account_id=00000000001",
            "&start_key=0000000000000005&direction=backward&page_size=1",
        )
        .parse()?;
        let Query(filter) = Query::<TransactionListQuery>::try_from_uri(&uri)?;
        let Query(page) = Query::<PageQuery>::try_from_uri(&uri)?;
        assert_eq!(filter.account_id.as_deref(), Some("00000000001"));
        assert_eq!(page.start_key.as_deref(), Some("0000000000000005"));
        assert_eq!(page.direction, Some(PageDirection::Backward));
        let Query(none) = Query::<TransactionListQuery>::try_from_uri(&"/transactions".parse()?)?;
        assert_eq!(none, TransactionListQuery::default());
        Ok(())
    }

    #[test]
    fn field_enums_name_the_audited_bms_fields_in_rank_order() -> TestResult {
        for (index, (field, name)) in ADD_FIELDS.into_iter().enumerate() {
            assert_eq!(field.as_str(), name);
            assert_eq!(BmsField::as_str(field), name);
            assert_eq!(ADD_MEMBERS[index].1, name);
            assert_eq!(serde_json::to_value(field)?, json!(name));
            assert_eq!(
                serde_json::from_value::<TransactionAddField>(json!(name))?,
                field
            );
            assert_eq!(BmsField::rank(field), u16::try_from(index + 1)?);
        }
        let names: Vec<&str> = ADD_FIELDS.iter().map(|(_, name)| *name).collect();
        assert_eq!(
            serde_json::to_value(TransactionAddField::schema())?["enum"],
            json!(names)
        );

        assert_eq!(TransactionListField::TrnIdIn.as_str(), "TRNIDIN");
        assert_eq!(
            serde_json::to_value(TransactionListField::TrnIdIn)?,
            json!("TRNIDIN")
        );
        assert_eq!(BmsField::rank(TransactionListField::TrnIdIn), 1);
        assert_eq!(
            TransactionViewField::TrnIdIn.as_str(),
            transaction_view::TRAN_ID_FIELD
        );
        assert_eq!(
            serde_json::to_value(TransactionViewField::TrnIdIn)?,
            json!("TRNIDIN")
        );
        assert_eq!(BmsField::rank(TransactionViewField::TrnIdIn), 1);
        Ok(())
    }
}
