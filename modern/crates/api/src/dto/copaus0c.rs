//! The contract of endpoint 21, `GET /api/v1/accounts/{acct_id}/authorizations` (transaction
//! `CPVS`, program COPAUS0C, map `COPAU0A`; AAP 0.3.4.1): the [`AuthSummaryQuery`] of ENTER, F7
//! and F8, and the [`AuthSummaryResponse`] with its [`AuthSummaryBlock`] and
//! [`AuthSummaryRow`]s.
//!
//! - **The account** travels as the raw `acct_id` path segment, which no DTO revalidates:
//!   `domain::online::copaus0c::edit_account` answers every value that is not 11 digits, a longer
//!   one included, with the program's own message (COPAUS0C-005, COPAUS0C-006), which is the
//!   server half of the map length of `ACCTID` (COPAU00-013).
//! - **Paging** is a Bounded Pagination collection of 5 rows by default. The query carries only
//!   `page_key`, `direction` and `page_size`; the page number, the first key of each page and
//!   the top-of-page and bottom-of-page decisions are the screen's paging state (COPAUS0C-039,
//!   COPAUS0C-040, COPAUS0C-045), so no page number is sent or returned.
//! - **Display values** are the strings the domain builds with the program's moves and
//!   pictures, at the map field widths and with their padding: the screen formats nothing, and
//!   no amount is computed or edited here (R1).
//! - **Stored request text**, the Transaction ID, Type and STS that CP00 keeps as the request
//!   sent them, passes through [`redact_free_text`] as endpoint 22 masks it, so one stored
//!   value shows one way on COPAU00 and COPAU01: `TX0000000000001` shows as `TX*********0001`
//!   and an SSN as `***-**-` and its last four digits (COPAUS0C-032, COPAUS1C-021, E32).
//! - **No card number**, full or redacted, is part of any schema: COPAU00 shows none
//!   (COPAUS0C-055, E32).
//!
//! A failed edit or read is no part of these types: it reaches the client as a `ScreenError`
//! through `api::error`.

use std::fmt;

use domain::copybook::PicU;
use domain::online::copaus0c as rules;
use domain::sensitive::redact_free_text;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError};

use super::{PageDirection, digits};

/// The `validator` code of a `page_key` that is not a rendered authorization key. It has no text
/// of its own, so `api::dto` answers it with the fixed text of an input the screen field would
/// not have admitted.
const PAGE_KEY_CODE: &str = "page_key";

/// The query of endpoint 21: absent members are ENTER or a first entry, which shows the first
/// page; `direction` with a `page_key` is F8 or F7 (COPAUS0C-008, COPAUS0C-041, COPAUS0C-047).
// The schema name `AuthSummaryQuery` is the frontend's `components["schemas"]["AuthSummaryQuery"]`
// (Generated API Types Only).
#[derive(Debug, Clone, ToSchema, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct AuthSummaryQuery {
    /// F8: the `last_key` of the page shown; F7: the `first_key` the screen saved for the page
    /// before it. The 16 digits of an authorization key exactly as a response renders them; any
    /// other text is rejected with 400 on field `page_key`.
    // No example: every valid key is a run of sixteen digits, which a card-number scan of the
    // emitted contract cannot tell from a card number (Cardholder Data Redaction).
    #[validate(custom(function = "valid_page_key"))]
    #[schema(
        min_length = 16,
        max_length = 16,
        pattern = "^[01][0-9]{5}[01][0-9]{9}$"
    )]
    #[param(
        min_length = 16,
        max_length = 16,
        pattern = "^[01][0-9]{5}[01][0-9]{9}$"
    )]
    pub page_key: Option<String>,
    /// `forward` (F8) pages after `page_key`, `backward` (F7) from it; absent with a `page_key`
    /// pages forward, and without one the first page is shown whatever the direction.
    pub direction: Option<PageDirection>,
    /// Rows per page, 1 to 100; absent, the 5 rows of the map. A value outside that range is
    /// rejected with 422 (Bounded Pagination). The screen never sends it.
    #[validate(range(min = 1, max = 100))]
    #[schema(minimum = 1, maximum = 100)]
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<i64>,
}

/// Every member in declaration order, an absent value as `null`.
impl Serialize for AuthSummaryQuery {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AuthSummaryQuery", 3)?;
        out.serialize_field("page_key", &self.page_key)?;
        out.serialize_field("direction", &self.direction)?;
        out.serialize_field("page_size", &self.page_size)?;
        out.end()
    }
}

/// The members of [`AuthSummaryQuery`], in declaration order.
const AUTH_SUMMARY_QUERY_MEMBERS: &[&str] = &["page_key", "direction", "page_size"];

/// Every member optional, absent or `null` as `None`; `page_size` read as `QueryPageSize`
/// reads it, so that every value reaches the range rule and its 422.
impl<'de> Deserialize<'de> for AuthSummaryQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "AuthSummaryQuery",
            AUTH_SUMMARY_QUERY_MEMBERS,
            AuthSummaryQueryVisitor,
        )
    }
}

/// The visitor of [`AuthSummaryQuery`]'s members.
struct AuthSummaryQueryVisitor;

impl<'de> Visitor<'de> for AuthSummaryQueryVisitor {
    type Value = AuthSummaryQuery;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct AuthSummaryQuery")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AuthSummaryQuery, A::Error> {
        let mut page_key = None;
        let mut direction = None;
        let mut page_size = None;
        while let Some(name) = map.next_key_seed(WireName(AUTH_SUMMARY_QUERY_MEMBERS))? {
            match name {
                Some("page_key") => member(&mut map, &mut page_key, "page_key")?,
                Some("direction") => member(&mut map, &mut direction, "direction")?,
                Some("page_size") => member(&mut map, &mut page_size, "page_size")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(AuthSummaryQuery {
            page_key: page_key.flatten(),
            direction: direction.flatten(),
            page_size: page_size.and_then(|QueryPageSize(size)| size),
        })
    }
}

/// The `page_key` rule: the text [`rules::AuthKey::parse`] accepts, the 16 digits
/// [`rules::AuthKey::render`] writes, and nothing else.
///
/// # Errors
///
/// A [`ValidationError`] with code [`PAGE_KEY_CODE`] and no message for any other text. The
/// error carries nothing of the rejected text.
fn valid_page_key(value: &str) -> Result<(), ValidationError> {
    rules::AuthKey::parse(value)
        .map(|_| ())
        .map_err(|_| ValidationError::new(PAGE_KEY_CODE))
}

/// The page a query asks for, as COPAUS0C reads it: [`rules::PageRequest::new`] over the query's
/// direction and its parsed `page_key`.
impl TryFrom<&AuthSummaryQuery> for rules::PageRequest {
    type Error = rules::AuthKeyError;

    /// # Errors
    ///
    /// [`rules::AuthKeyError`] for a `page_key` that is not a rendered authorization key, which
    /// the query's own validation has already rejected with 400.
    fn try_from(query: &AuthSummaryQuery) -> Result<Self, Self::Error> {
        let page_key = query
            .page_key
            .as_deref()
            .map(rules::AuthKey::parse)
            .transpose()?;
        Ok(Self::new(
            query.direction.map(rules::Direction::from),
            page_key,
        ))
    }
}

/// The customer and summary block of `COPAU0A`, every value as its map field shows it, padding
/// included (COPAUS0C-020 to COPAUS0C-025, COPAUS0C-027 to COPAUS0C-029).
///
/// The map's one-byte `Acct Status` field has no member: COPAUS0C never fills it, so it is
/// always blank and endpoint 21 returns no account status (COPAUS0C-026).
// The schema name `AuthSummaryBlock` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
#[schema(example = json!({
    "customer_id": "000000123",
    "customer_name": "ANN L DOE                ",
    "address_line_1": "12 MAIN STREET,BLDG 7    ",
    "address_line_2": "SPRINGFIELD,IL,62701     ",
    "phone": "(217)555-0100",
    "credit_limit": "     1234.56",
    "cash_limit": "  1234.56",
    "summary_found": true,
    "approved_count": "001",
    "declined_count": "000",
    "credit_balance": "      100.00",
    "cash_balance": "     0.00",
    "approved_amount": "   100.00 ",
    "declined_amount": "     0.00 "
}))]
pub struct AuthSummaryBlock {
    /// Customer Id, `CUSTID` (9): the customer's 9 digits with their leading zeros
    /// (COPAUS0C-020).
    pub customer_id: String,
    /// Name, `CNAME` (25): first name, middle initial and last name (COPAUS0C-021).
    pub customer_name: String,
    /// The first address row, `ADDR001` (25): address lines 1 and 2 (COPAUS0C-022).
    pub address_line_1: String,
    /// The second address row, `ADDR002` (25): address line 3, state and ZIP (COPAUS0C-023).
    pub address_line_2: String,
    /// PH, `PHONE1` (13): the first 13 characters of phone 1 (COPAUS0C-024).
    pub phone: String,
    /// Credit Lim, `CREDLIM` (12): the account's credit limit through `-zzzzzzz9.99`
    /// (COPAUS0C-025).
    pub credit_limit: String,
    /// Cash Lim, `CASHLIM` (9): the account's cash credit limit through `-zzzz9.99`
    /// (COPAUS0C-025).
    pub cash_limit: String,
    /// Whether the account has a pending-authorization summary. Without one the six figures
    /// below are `0` characters filling each field and the list is empty, a normal outcome
    /// (COPAUS0C-028, COPAUS0C-029).
    pub summary_found: bool,
    /// Approval #, `APPRCNT` (3): the approved count as 3 digits (COPAUS0C-028).
    pub approved_count: String,
    /// Decline #, `DECLCNT` (3): the declined count as 3 digits (COPAUS0C-028).
    pub declined_count: String,
    /// Credit Bal, `CREDBAL` (12): through `-zzzzzzz9.99` (COPAUS0C-028).
    pub credit_balance: String,
    /// Cash Bal, `CASHBAL` (9): through `-zzzz9.99` (COPAUS0C-028).
    pub cash_balance: String,
    /// Appr Amt, `APPRAMT` (10): through `-zzzz9.99` and one space (COPAUS0C-028).
    pub approved_amount: String,
    /// Decl Amt, `DECLAMT` (10): through `-zzzz9.99` and one space (COPAUS0C-028).
    pub declined_amount: String,
}

/// Every member in declaration order.
impl Serialize for AuthSummaryBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AuthSummaryBlock", 14)?;
        out.serialize_field("customer_id", &self.customer_id)?;
        out.serialize_field("customer_name", &self.customer_name)?;
        out.serialize_field("address_line_1", &self.address_line_1)?;
        out.serialize_field("address_line_2", &self.address_line_2)?;
        out.serialize_field("phone", &self.phone)?;
        out.serialize_field("credit_limit", &self.credit_limit)?;
        out.serialize_field("cash_limit", &self.cash_limit)?;
        out.serialize_field("summary_found", &self.summary_found)?;
        out.serialize_field("approved_count", &self.approved_count)?;
        out.serialize_field("declined_count", &self.declined_count)?;
        out.serialize_field("credit_balance", &self.credit_balance)?;
        out.serialize_field("cash_balance", &self.cash_balance)?;
        out.serialize_field("approved_amount", &self.approved_amount)?;
        out.serialize_field("declined_amount", &self.declined_amount)?;
        out.end()
    }
}

/// The members of [`AuthSummaryBlock`], in declaration order.
const AUTH_SUMMARY_BLOCK_MEMBERS: &[&str] = &[
    "customer_id",
    "customer_name",
    "address_line_1",
    "address_line_2",
    "phone",
    "credit_limit",
    "cash_limit",
    "summary_found",
    "approved_count",
    "declined_count",
    "credit_balance",
    "cash_balance",
    "approved_amount",
    "declined_amount",
];

/// Every member required.
impl<'de> Deserialize<'de> for AuthSummaryBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "AuthSummaryBlock",
            AUTH_SUMMARY_BLOCK_MEMBERS,
            AuthSummaryBlockVisitor,
        )
    }
}

/// The visitor of [`AuthSummaryBlock`]'s members.
struct AuthSummaryBlockVisitor;

impl<'de> Visitor<'de> for AuthSummaryBlockVisitor {
    type Value = AuthSummaryBlock;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct AuthSummaryBlock")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AuthSummaryBlock, A::Error> {
        let mut customer_id = None;
        let mut customer_name = None;
        let mut address_line_1 = None;
        let mut address_line_2 = None;
        let mut phone = None;
        let mut credit_limit = None;
        let mut cash_limit = None;
        let mut summary_found = None;
        let mut approved_count = None;
        let mut declined_count = None;
        let mut credit_balance = None;
        let mut cash_balance = None;
        let mut approved_amount = None;
        let mut declined_amount = None;
        while let Some(name) = map.next_key_seed(WireName(AUTH_SUMMARY_BLOCK_MEMBERS))? {
            match name {
                Some("customer_id") => member(&mut map, &mut customer_id, "customer_id")?,
                Some("customer_name") => member(&mut map, &mut customer_name, "customer_name")?,
                Some("address_line_1") => member(&mut map, &mut address_line_1, "address_line_1")?,
                Some("address_line_2") => member(&mut map, &mut address_line_2, "address_line_2")?,
                Some("phone") => member(&mut map, &mut phone, "phone")?,
                Some("credit_limit") => member(&mut map, &mut credit_limit, "credit_limit")?,
                Some("cash_limit") => member(&mut map, &mut cash_limit, "cash_limit")?,
                Some("summary_found") => member(&mut map, &mut summary_found, "summary_found")?,
                Some("approved_count") => member(&mut map, &mut approved_count, "approved_count")?,
                Some("declined_count") => member(&mut map, &mut declined_count, "declined_count")?,
                Some("credit_balance") => member(&mut map, &mut credit_balance, "credit_balance")?,
                Some("cash_balance") => member(&mut map, &mut cash_balance, "cash_balance")?,
                Some("approved_amount") => {
                    member(&mut map, &mut approved_amount, "approved_amount")?;
                }
                Some("declined_amount") => {
                    member(&mut map, &mut declined_amount, "declined_amount")?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(AuthSummaryBlock {
            customer_id: customer_id.ok_or_else(|| de::Error::missing_field("customer_id"))?,
            customer_name: customer_name
                .ok_or_else(|| de::Error::missing_field("customer_name"))?,
            address_line_1: address_line_1
                .ok_or_else(|| de::Error::missing_field("address_line_1"))?,
            address_line_2: address_line_2
                .ok_or_else(|| de::Error::missing_field("address_line_2"))?,
            phone: phone.ok_or_else(|| de::Error::missing_field("phone"))?,
            credit_limit: credit_limit.ok_or_else(|| de::Error::missing_field("credit_limit"))?,
            cash_limit: cash_limit.ok_or_else(|| de::Error::missing_field("cash_limit"))?,
            summary_found: summary_found
                .ok_or_else(|| de::Error::missing_field("summary_found"))?,
            approved_count: approved_count
                .ok_or_else(|| de::Error::missing_field("approved_count"))?,
            declined_count: declined_count
                .ok_or_else(|| de::Error::missing_field("declined_count"))?,
            credit_balance: credit_balance
                .ok_or_else(|| de::Error::missing_field("credit_balance"))?,
            cash_balance: cash_balance.ok_or_else(|| de::Error::missing_field("cash_balance"))?,
            approved_amount: approved_amount
                .ok_or_else(|| de::Error::missing_field("approved_amount"))?,
            declined_amount: declined_amount
                .ok_or_else(|| de::Error::missing_field("declined_amount"))?,
        })
    }
}

/// The block exactly as the domain renders it, its summary state as `summary_found`.
impl From<rules::SummaryBlock> for AuthSummaryBlock {
    fn from(block: rules::SummaryBlock) -> Self {
        Self {
            customer_id: block.customer_id,
            customer_name: block.name,
            address_line_1: block.address_line_1,
            address_line_2: block.address_line_2,
            phone: block.phone,
            credit_limit: block.credit_limit,
            cash_limit: block.cash_limit,
            summary_found: block.summary == rules::SummaryState::Found,
            approved_count: block.approved_count,
            declined_count: block.declined_count,
            credit_balance: block.credit_balance,
            cash_balance: block.cash_balance,
            approved_amount: block.approved_amount,
            declined_amount: block.declined_amount,
        }
    }
}

/// One row of the list, as `POPULATE-AUTH-LIST` fills it, and the key that selects it
/// (COPAUS0C-032 to COPAUS0C-034). It holds no card number (COPAUS0C-055).
///
/// `date` and `time` are the display strings the program builds, not typed values: they come
/// from `PA-AUTH-ORIG-DATE` and `PA-AUTH-ORIG-TIME`, `PIC X(06)` fields outside the `X(10)` dates
/// and `X(26)` timestamps that Typed Dates and Timestamps covers, copied with no calendar check,
/// so a stored `241399` shows `13/99/24`, which no date type holds (COPAUS0C-034).
// The schema name `AuthSummaryRow` is fixed (Generated API Types Only). The example leaves out
// `auth_key`: a key is sixteen digits, which a card-number scan of the emitted contract cannot
// tell from a card number (Cardholder Data Redaction).
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
#[schema(example = json!({
    "transaction_id": "TX*********0001 ",
    "date": "01/31/24",
    "time": "13:05:09",
    "auth_type": "PURC",
    "approval": "A",
    "match_status": "P",
    "amount": "      100.00"
}))]
pub struct AuthSummaryRow {
    /// The row's authorization key as 16 digits: the `{auth_key}` path segment of endpoints 22
    /// and 23 for an `S` on this row, and the `page_key` of the next F7 or F8 when it is the
    /// page's first or last row (COPAUS0C-010, COPAUS0C-034).
    pub auth_key: String,
    /// Transaction ID, `TRNIDnn` (16): the 15 stored characters and one space, masked by
    /// [`redact_free_text`] as stored request text, as endpoint 22 masks it: `TX0000000000001`
    /// shows as `TX*********0001`, a 9-digit SSN as `***-**-` and its last four digits, and a
    /// run of fewer than nine digits as stored. A Tran Id whose SSN fills its last nine bytes is
    /// one character longer than the 16 of the map field: the redacted form adds two, the one
    /// trailing space gives back one (COPAUS0C-032, COPAUS1C-021, E32).
    pub transaction_id: String,
    /// Date, `PDATEnn` (8): `MM/DD/YY` from the stored `YYMMDD` (COPAUS0C-034).
    pub date: String,
    /// Time, `PTIMEnn` (8): `HH:MM:SS` from the stored `HHMMSS` (COPAUS0C-034).
    pub time: String,
    /// Type, `PTYPEnn` (4): the stored authorization type, masked as the transaction ID is; its
    /// 4 bytes hold no written number of nine digits and show as stored (COPAUS0C-032, E32).
    pub auth_type: String,
    /// A/D, `PAPRVnn` (1): `A` for response code `00`, `D` otherwise (COPAUS0C-033).
    pub approval: String,
    /// STS, `PSTATnn` (1): the stored match status, masked as the transaction ID is; its one
    /// byte shows as stored (COPAUS0C-032, E32).
    pub match_status: String,
    /// Amount, `PAMTnnn` (12): the approved amount through `-zzzzzzz9.99` (COPAUS0C-033).
    pub amount: String,
}

/// Every member in declaration order.
impl Serialize for AuthSummaryRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AuthSummaryRow", 8)?;
        out.serialize_field("auth_key", &self.auth_key)?;
        out.serialize_field("transaction_id", &self.transaction_id)?;
        out.serialize_field("date", &self.date)?;
        out.serialize_field("time", &self.time)?;
        out.serialize_field("auth_type", &self.auth_type)?;
        out.serialize_field("approval", &self.approval)?;
        out.serialize_field("match_status", &self.match_status)?;
        out.serialize_field("amount", &self.amount)?;
        out.end()
    }
}

/// The members of [`AuthSummaryRow`], in declaration order.
const AUTH_SUMMARY_ROW_MEMBERS: &[&str] = &[
    "auth_key",
    "transaction_id",
    "date",
    "time",
    "auth_type",
    "approval",
    "match_status",
    "amount",
];

/// Every member required.
impl<'de> Deserialize<'de> for AuthSummaryRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "AuthSummaryRow",
            AUTH_SUMMARY_ROW_MEMBERS,
            AuthSummaryRowVisitor,
        )
    }
}

/// The visitor of [`AuthSummaryRow`]'s members.
struct AuthSummaryRowVisitor;

impl<'de> Visitor<'de> for AuthSummaryRowVisitor {
    type Value = AuthSummaryRow;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct AuthSummaryRow")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AuthSummaryRow, A::Error> {
        let mut auth_key = None;
        let mut transaction_id = None;
        let mut date = None;
        let mut time = None;
        let mut auth_type = None;
        let mut approval = None;
        let mut match_status = None;
        let mut amount = None;
        while let Some(name) = map.next_key_seed(WireName(AUTH_SUMMARY_ROW_MEMBERS))? {
            match name {
                Some("auth_key") => member(&mut map, &mut auth_key, "auth_key")?,
                Some("transaction_id") => member(&mut map, &mut transaction_id, "transaction_id")?,
                Some("date") => member(&mut map, &mut date, "date")?,
                Some("time") => member(&mut map, &mut time, "time")?,
                Some("auth_type") => member(&mut map, &mut auth_type, "auth_type")?,
                Some("approval") => member(&mut map, &mut approval, "approval")?,
                Some("match_status") => member(&mut map, &mut match_status, "match_status")?,
                Some("amount") => member(&mut map, &mut amount, "amount")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(AuthSummaryRow {
            auth_key: auth_key.ok_or_else(|| de::Error::missing_field("auth_key"))?,
            transaction_id: transaction_id
                .ok_or_else(|| de::Error::missing_field("transaction_id"))?,
            date: date.ok_or_else(|| de::Error::missing_field("date"))?,
            time: time.ok_or_else(|| de::Error::missing_field("time"))?,
            auth_type: auth_type.ok_or_else(|| de::Error::missing_field("auth_type"))?,
            approval: approval.ok_or_else(|| de::Error::missing_field("approval"))?,
            match_status: match_status.ok_or_else(|| de::Error::missing_field("match_status"))?,
            amount: amount.ok_or_else(|| de::Error::missing_field("amount"))?,
        })
    }
}

/// The row as the domain renders it, its key through [`rules::AuthKey::render`] and its stored
/// request text through [`redact_free_text`].
impl From<rules::AuthRow> for AuthSummaryRow {
    fn from(row: rules::AuthRow) -> Self {
        Self {
            auth_key: row.key.render(),
            // The Transaction ID, Type and STS are stored text of the CP00 request, kept as the
            // request sent it, so the 15 bytes of the ID can hold an SSN. They leave through
            // `redact_free_text` with no exemption, exactly as endpoint 22 masks the same
            // members, so one stored value shows one way on COPAU00 and COPAU01 and no stored
            // text reaches the body or the derived `Debug` of this type unmasked; the 4 bytes of
            // the type and the 1 of the status hold no written number of nine digits and pass
            // unchanged (E32). The values the program builds stay as built.
            transaction_id: redact_free_text(&row.transaction_id),
            date: row.date,
            time: row.time,
            auth_type: redact_free_text(&row.auth_type),
            approval: row.approval.as_str().to_owned(),
            match_status: redact_free_text(&row.match_status),
            amount: row.amount,
        }
    }
}

/// The 200 of endpoint 21: the account, its block and one page of its pending authorizations,
/// with the keys and the next-page flag the screen keeps in its paging state (COPAUS0C-008,
/// COPAUS0C-031, COPAUS0C-036, COPAUS0C-039). A successful read carries no message
/// (COPAUS0C-052).
// The schema name `AuthSummaryResponse` is the frontend's
// `components["schemas"]["AuthSummaryResponse"]` (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
pub struct AuthSummaryResponse {
    /// The account read, 11 digits with their leading zeros, which the account field shows.
    pub acct_id: String,
    /// The customer and summary block, returned whether or not a summary exists
    /// (COPAUS0C-028, COPAUS0C-029).
    pub summary: AuthSummaryBlock,
    /// Up to `page_size` rows in key order; empty without a summary, or when no detail has the
    /// requested `page_key` (COPAUS0C-029, COPAUS0C-041, COPAUS0C-047).
    pub rows: Vec<AuthSummaryRow>,
    /// The first row's `auth_key`, which the screen saves as this page's first key for F7;
    /// `null` with no rows (COPAUS0C-039).
    pub first_key: Option<String>,
    /// The last row's `auth_key`, the `page_key` of the next F8; `null` with no rows
    /// (COPAUS0C-039).
    pub last_key: Option<String>,
    /// The next-page flag after a full page: `true` when another authorization follows it,
    /// `false` when none does. `null` when the page holds fewer than `page_size` rows: the
    /// program leaves the flag unset, and the screen keeps the value an earlier page gave it
    /// (COPAUS0C-036).
    pub has_next_page: Option<bool>,
}

/// Every member in declaration order, an absent key or flag as `null`.
impl Serialize for AuthSummaryResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AuthSummaryResponse", 6)?;
        out.serialize_field("acct_id", &self.acct_id)?;
        out.serialize_field("summary", &self.summary)?;
        out.serialize_field("rows", &self.rows)?;
        out.serialize_field("first_key", &self.first_key)?;
        out.serialize_field("last_key", &self.last_key)?;
        out.serialize_field("has_next_page", &self.has_next_page)?;
        out.end()
    }
}

/// The members of [`AuthSummaryResponse`], in declaration order.
const AUTH_SUMMARY_RESPONSE_MEMBERS: &[&str] = &[
    "acct_id",
    "summary",
    "rows",
    "first_key",
    "last_key",
    "has_next_page",
];

/// `acct_id`, `summary` and `rows` required; the keys and the flag absent or `null` as `None`.
impl<'de> Deserialize<'de> for AuthSummaryResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "AuthSummaryResponse",
            AUTH_SUMMARY_RESPONSE_MEMBERS,
            AuthSummaryResponseVisitor,
        )
    }
}

/// The visitor of [`AuthSummaryResponse`]'s members.
struct AuthSummaryResponseVisitor;

impl<'de> Visitor<'de> for AuthSummaryResponseVisitor {
    type Value = AuthSummaryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct AuthSummaryResponse")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AuthSummaryResponse, A::Error> {
        let mut acct_id = None;
        let mut summary = None;
        let mut rows = None;
        let mut first_key = None;
        let mut last_key = None;
        let mut has_next_page = None;
        while let Some(name) = map.next_key_seed(WireName(AUTH_SUMMARY_RESPONSE_MEMBERS))? {
            match name {
                Some("acct_id") => member(&mut map, &mut acct_id, "acct_id")?,
                Some("summary") => member(&mut map, &mut summary, "summary")?,
                Some("rows") => member(&mut map, &mut rows, "rows")?,
                Some("first_key") => member(&mut map, &mut first_key, "first_key")?,
                Some("last_key") => member(&mut map, &mut last_key, "last_key")?,
                Some("has_next_page") => member(&mut map, &mut has_next_page, "has_next_page")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(AuthSummaryResponse {
            acct_id: acct_id.ok_or_else(|| de::Error::missing_field("acct_id"))?,
            summary: summary.ok_or_else(|| de::Error::missing_field("summary"))?,
            rows: rows.ok_or_else(|| de::Error::missing_field("rows"))?,
            first_key: first_key.flatten(),
            last_key: last_key.flatten(),
            has_next_page: has_next_page.flatten(),
        })
    }
}

/// The response for the edited account, the block [`rules::summary_block`] built and the page
/// [`rules::page`] built, or [`rules::AuthPage::without_details`] when no summary exists.
impl From<(PicU<11>, rules::SummaryBlock, rules::AuthPage)> for AuthSummaryResponse {
    fn from((acct_id, block, page): (PicU<11>, rules::SummaryBlock, rules::AuthPage)) -> Self {
        Self {
            acct_id: digits(acct_id),
            summary: AuthSummaryBlock::from(block),
            first_key: page.first_key().map(rules::AuthKey::render),
            last_key: page.last_key().map(rules::AuthKey::render),
            has_next_page: next_page_flag(page.next_page),
            rows: page.rows.into_iter().map(AuthSummaryRow::from).collect(),
        }
    }
}

/// `CDEMO-CPVS-NEXT-PAGE-FLG` as the response reports it: `Y` and `N` after a full page, and
/// nothing when the page read left the flag unset (COPAUS0C-036).
const fn next_page_flag(next_page: rules::NextPage) -> Option<bool> {
    match next_page {
        rules::NextPage::Yes => Some(true),
        rules::NextPage::No => Some(false),
        rules::NextPage::Unreported => None,
    }
}

/// A member or variant name of this module's types, read as the entry of its list that it
/// equals, or `None` for a name the list does not hold.
#[derive(Clone, Copy)]
struct WireName(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for WireName {
    type Value = Option<&'static str>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}

impl Visitor<'_> for WireName {
    type Value = Option<&'static str>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a member or variant name")
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
        Ok(self.0.iter().copied().find(|wire| *wire == name))
    }
}

/// Reads the value of `name` into `slot`, refusing a member the object repeats.
fn member<'de, A, T>(map: &mut A, slot: &mut Option<T>, name: &'static str) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

/// A `page_size` as [`super::page_size_from_query`] reads it from a query string.
struct QueryPageSize(Option<i64>);

impl<'de> Deserialize<'de> for QueryPageSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        super::page_size_from_query(deserializer).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;
    use std::fmt::Debug;

    use axum::extract::FromRequestParts;
    use axum::http::{Request, StatusCode};
    use domain::copybook::{PicS, PicU};
    use domain::online::copaus0c as rules;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use utoipa::{IntoParams, PartialSchema, ToSchema};

    use super::{
        AUTH_SUMMARY_BLOCK_MEMBERS, AUTH_SUMMARY_RESPONSE_MEMBERS, AUTH_SUMMARY_ROW_MEMBERS,
        AuthSummaryBlock, AuthSummaryQuery, AuthSummaryResponse, AuthSummaryRow, PageDirection,
    };
    use crate::dto::{
        FIELD_LENGTH_MESSAGE, INVALID_QUERY, PAGE_SIZE_FIELD, PAGE_SIZE_MESSAGE, ValidQuery,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// The rendered key of an invented authorization: date part 75968, time part 869490879.
    const KEY: &str = "0759680869490879";

    /// The rendered key of a second invented authorization, after [`KEY`] in key order.
    const NEXT_KEY: &str = "0759680869490999";

    fn key(date: i64, time: i64) -> Result<rules::AuthKey, Box<dyn StdError>> {
        Ok(rules::AuthKey::new(PicS::new(date)?, PicS::new(time)?))
    }

    fn block(summary: rules::SummaryState) -> rules::SummaryBlock {
        rules::SummaryBlock {
            customer_id: "000000123".to_owned(),
            name: format!("{:<25}", "ANN L DOE"),
            address_line_1: format!("{:<25}", "12 MAIN STREET,BLDG 7"),
            address_line_2: format!("{:<25}", "SPRINGFIELD,IL,62701"),
            phone: "(217)555-0100".to_owned(),
            credit_limit: "     1234.56".to_owned(),
            cash_limit: "  1234.56".to_owned(),
            summary,
            approved_count: "001".to_owned(),
            declined_count: "000".to_owned(),
            credit_balance: "      100.00".to_owned(),
            cash_balance: "     0.00".to_owned(),
            approved_amount: "   100.00 ".to_owned(),
            declined_amount: "     0.00 ".to_owned(),
        }
    }

    fn row(key: rules::AuthKey, approval: rules::Approval) -> rules::AuthRow {
        rules::AuthRow {
            key,
            transaction_id: "TX0000000000001 ".to_owned(),
            date: "13/99/24".to_owned(),
            time: "13:05:09".to_owned(),
            auth_type: "PURC".to_owned(),
            approval,
            match_status: "P".to_owned(),
            amount: "      100.00".to_owned(),
        }
    }

    fn account() -> Result<PicU<11>, Box<dyn StdError>> {
        Ok(PicU::new(123)?)
    }

    /// The member names of a JSON object.
    fn members(value: &Value) -> BTreeSet<&str> {
        value
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    async fn query(uri: &str) -> Result<Result<AuthSummaryQuery, ScreenError>, axum::http::Error> {
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(
            ValidQuery::<AuthSummaryQuery>::from_request_parts(&mut head, &())
                .await
                .map(|ValidQuery(value)| value),
        )
    }

    /// Holds the JSON text of `value` to `wire`, its member names in declaration order, and to
    /// its schema: the members leave in that order and are the schema's properties; read back,
    /// a member no schema names is skipped, one named twice is refused, one the schema requires
    /// is refused when absent, and any other reads absent as it reads `null`.
    fn holds_its_members<T>(value: &T, wire: &[&str]) -> TestResult
    where
        T: Serialize + DeserializeOwned + PartialSchema + PartialEq + Debug,
    {
        let text = serde_json::to_string(value)?;
        let object = serde_json::from_str::<Value>(&text)?
            .as_object()
            .cloned()
            .ok_or("not a JSON object")?;
        let schema = serde_json::to_value(T::schema())?;
        assert_eq!(
            members(&schema["properties"]),
            wire.iter().copied().collect::<BTreeSet<_>>()
        );
        let positions = wire
            .iter()
            .map(|name| {
                text.find(&format!("\"{name}\":"))
                    .ok_or("member not written")
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert!(positions.is_sorted(), "{text}");

        let rest = text.get(1..).ok_or("empty JSON text")?;
        let unknown = format!("{{\"unknown_member\":{{\"nested\":[1,\"two\",null]}},{rest}");
        assert_eq!(serde_json::from_str::<T>(&unknown)?, *value);

        let required: BTreeSet<&str> = schema["required"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for name in wire {
            let held = serde_json::to_string(object.get(*name).ok_or("member not written")?)?;
            let repeated = format!("{{\"{name}\":{held},{rest}");
            let error = serde_json::from_str::<T>(&repeated)
                .err()
                .ok_or("repeated member accepted")?;
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("duplicate field `{name}`")),
                "{error}"
            );

            let mut absent = object.clone();
            absent.remove(*name);
            let read = serde_json::from_value::<T>(Value::Object(absent));
            if required.contains(name) {
                let error = read.err().ok_or("missing member accepted")?;
                assert_eq!(error.to_string(), format!("missing field `{name}`"));
            } else {
                let mut null = object.clone();
                null.insert((*name).to_owned(), Value::Null);
                assert_eq!(
                    read?,
                    serde_json::from_value::<T>(Value::Object(null))?,
                    "{name}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn the_block_carries_every_rendered_field_and_the_summary_state() {
        for (state, found) in [
            (rules::SummaryState::Found, true),
            (rules::SummaryState::NotFound, false),
        ] {
            let source = block(state);
            let dto = AuthSummaryBlock::from(source.clone());
            assert_eq!(
                dto,
                AuthSummaryBlock {
                    customer_id: source.customer_id,
                    customer_name: source.name,
                    address_line_1: source.address_line_1,
                    address_line_2: source.address_line_2,
                    phone: source.phone,
                    credit_limit: source.credit_limit,
                    cash_limit: source.cash_limit,
                    summary_found: found,
                    approved_count: source.approved_count,
                    declined_count: source.declined_count,
                    credit_balance: source.credit_balance,
                    cash_balance: source.cash_balance,
                    approved_amount: source.approved_amount,
                    declined_amount: source.declined_amount,
                }
            );
        }
    }

    #[test]
    fn a_row_carries_its_rendered_key_and_approval_and_the_display_strings() -> TestResult {
        for (approval, shown) in [
            (rules::Approval::Approved, "A"),
            (rules::Approval::Declined, "D"),
        ] {
            let dto = AuthSummaryRow::from(row(key(75_968, 869_490_879)?, approval));
            assert_eq!(
                dto,
                AuthSummaryRow {
                    auth_key: KEY.to_owned(),
                    transaction_id: "TX*********0001 ".to_owned(),
                    date: "13/99/24".to_owned(),
                    time: "13:05:09".to_owned(),
                    auth_type: "PURC".to_owned(),
                    approval: shown.to_owned(),
                    match_status: "P".to_owned(),
                    amount: "      100.00".to_owned(),
                }
            );
        }
        // A negative key keeps a text of its own, which parses back to it.
        let negative = key(-1, -2)?;
        let text = AuthSummaryRow::from(row(negative, rules::Approval::Declined)).auth_key;
        assert_eq!(text, "1000011000000002");
        assert_eq!(rules::AuthKey::parse(&text), Ok(negative));
        Ok(())
    }

    // audit: COPAUS0C-032
    #[test]
    fn stored_request_text_leaves_masked_as_endpoint_22_shows_it() -> TestResult {
        // An invented SSN, assembled from its groups: no fixture value appears in this module.
        let ssn = ["123", "45", "6789"].concat();
        let dashed = ["123", "45", "6789"].join("-");
        let redacted = "***-**-6789";
        // The stored Transaction ID with its blank column, the text endpoint 21 shows for it and
        // that text's width: the 13-digit run of an ordinary ID with `*` for each digit but its
        // last four; runs of six and five digits as stored; a plain SSN in its redacted form,
        // kept at 16 through its trailing spaces; an SSN filling the last nine bytes, whose
        // redacted form is one character longer than the map field.
        let cases = [
            (
                "an ordinary ID",
                "TX0000000000001 ".to_owned(),
                "TX*********0001 ".to_owned(),
                16,
            ),
            (
                "runs of fewer than nine digits",
                "PTX220718A00002 ".to_owned(),
                "PTX220718A00002 ".to_owned(),
                16,
            ),
            (
                "a plain SSN",
                format!("{ssn:<16}"),
                format!("{redacted:<16}"),
                16,
            ),
            (
                "an SSN filling the last nine bytes",
                format!("ABCDEF{ssn} "),
                format!("ABCDEF{redacted}"),
                17,
            ),
        ];
        for (case, stored, shown, width) in cases {
            let dto = AuthSummaryRow::from(rules::AuthRow {
                transaction_id: stored,
                ..row(key(75_968, 869_490_879)?, rules::Approval::Approved)
            });
            assert_eq!(dto.transaction_id, shown, "{case}");
            assert_eq!(dto.transaction_id.chars().count(), width, "{case}");
            // The 4 bytes of the type and the 1 of the status hold no written number of nine
            // digits, so they leave as stored.
            assert_eq!(
                (dto.auth_type.as_str(), dto.match_status.as_str()),
                ("PURC", "P"),
                "{case}"
            );
            for egress in [format!("{dto:?}"), serde_json::to_string(&dto)?] {
                assert!(
                    !egress.contains(&ssn) && !egress.contains(&dashed),
                    "{case}: the stored SSN left unmasked"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn the_response_carries_the_account_the_rows_their_keys_and_the_flag() -> TestResult {
        let first = key(75_968, 869_490_879)?;
        let last = key(75_968, 869_490_999)?;
        for (next_page, flag) in [
            (rules::NextPage::Yes, Some(true)),
            (rules::NextPage::No, Some(false)),
            (rules::NextPage::Unreported, None),
        ] {
            let page = rules::AuthPage {
                rows: vec![
                    row(first, rules::Approval::Approved),
                    row(last, rules::Approval::Declined),
                ],
                next_page,
            };
            let response =
                AuthSummaryResponse::from((account()?, block(rules::SummaryState::Found), page));
            assert_eq!(response.acct_id, "00000000123");
            assert_eq!(
                response.summary,
                AuthSummaryBlock::from(block(rules::SummaryState::Found))
            );
            let keys: Vec<&str> = response.rows.iter().map(|r| r.auth_key.as_str()).collect();
            assert_eq!(keys, [KEY, NEXT_KEY]);
            assert_eq!(
                response
                    .rows
                    .iter()
                    .map(|r| r.approval.as_str())
                    .collect::<Vec<_>>(),
                ["A", "D"]
            );
            assert_eq!(response.first_key.as_deref(), Some(KEY));
            assert_eq!(response.last_key.as_deref(), Some(NEXT_KEY));
            assert_eq!(response.has_next_page, flag, "{next_page:?}");
        }
        Ok(())
    }

    #[test]
    fn without_a_summary_the_block_stays_and_the_list_is_empty() -> TestResult {
        let response = AuthSummaryResponse::from((
            account()?,
            block(rules::SummaryState::NotFound),
            rules::AuthPage::without_details(),
        ));
        let json = serde_json::to_value(&response)?;
        assert_eq!(
            members(&json),
            BTreeSet::from([
                "acct_id",
                "first_key",
                "has_next_page",
                "last_key",
                "rows",
                "summary",
            ])
        );
        assert_eq!(json["acct_id"], "00000000123");
        assert_eq!(json["rows"], Value::Array(Vec::new()));
        for absent in ["first_key", "last_key", "has_next_page"] {
            assert_eq!(json[absent], Value::Null, "{absent}");
        }
        assert_eq!(json["summary"]["summary_found"], false);
        assert_eq!(
            json["summary"]["customer_name"],
            "ANN L DOE                "
        );
        assert_eq!(
            members(&json["summary"]),
            BTreeSet::from([
                "address_line_1",
                "address_line_2",
                "approved_amount",
                "approved_count",
                "cash_balance",
                "cash_limit",
                "credit_balance",
                "credit_limit",
                "customer_id",
                "customer_name",
                "declined_amount",
                "declined_count",
                "phone",
                "summary_found",
            ])
        );
        assert_eq!(
            serde_json::from_value::<AuthSummaryResponse>(json)?,
            response
        );
        Ok(())
    }

    /// The response, its block and a row, each held to [`holds_its_members`].
    #[test]
    fn the_response_types_read_their_members_as_their_schemas_declare_them() -> TestResult {
        let page = rules::AuthPage {
            rows: vec![
                row(key(75_968, 869_490_879)?, rules::Approval::Approved),
                row(key(75_968, 869_490_999)?, rules::Approval::Declined),
            ],
            next_page: rules::NextPage::Yes,
        };
        let response =
            AuthSummaryResponse::from((account()?, block(rules::SummaryState::Found), page));
        holds_its_members(&response, AUTH_SUMMARY_RESPONSE_MEMBERS)?;
        holds_its_members(&response.summary, AUTH_SUMMARY_BLOCK_MEMBERS)?;
        let first = response.rows.first().ok_or("no row")?;
        holds_its_members(first, AUTH_SUMMARY_ROW_MEMBERS)
    }

    #[test]
    fn the_query_string_is_parsed_and_validated() -> TestResult {
        block_on(async {
            let parsed = query(&format!(
                "/authorizations?page_key={KEY}&direction=backward&page_size=5"
            ))
            .await?
            .map_err(|error| error.to_string())?;
            assert_eq!(parsed.page_key.as_deref(), Some(KEY));
            assert_eq!(parsed.direction, Some(PageDirection::Backward));
            assert_eq!(parsed.page_size, Some(5));

            let empty = query("/authorizations")
                .await?
                .map_err(|error| error.to_string())?;
            assert_eq!(
                (empty.page_key, empty.direction, empty.page_size),
                (None, None, None)
            );

            for uri in [
                "/authorizations?page_size=101",
                "/authorizations?page_size=0",
                "/authorizations?page_size=-5",
                "/authorizations?page_key=x&page_size=101",
                "/authorizations?page_size=five",
                "/authorizations?page_size=",
                "/authorizations?page_size=99999999999999999999",
            ] {
                let error = query(uri).await?.err().ok_or("page_size accepted")?;
                assert_eq!(
                    (error.status(), error.message(), error.field()),
                    (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        PAGE_SIZE_MESSAGE,
                        Some(PAGE_SIZE_FIELD)
                    ),
                    "{uri}"
                );
            }

            // Too short, too long, a letter, a sign digit other than 0 or 1, and a negative zero.
            for page_key in [
                "",
                "075968086949087",
                "07596808694908790",
                "075968086949087A",
                "2759680869490879",
                "1000000869490879",
            ] {
                let error = query(&format!("/authorizations?page_key={page_key}"))
                    .await?
                    .err()
                    .ok_or("malformed page_key accepted")?;
                assert_eq!(
                    (error.status(), error.message(), error.field()),
                    (
                        StatusCode::BAD_REQUEST,
                        FIELD_LENGTH_MESSAGE,
                        Some("page_key")
                    ),
                    "{page_key:?}"
                );
                assert!(page_key.is_empty() || !error.message().contains(page_key));
            }

            let uri = "/authorizations?direction=sideways";
            let error = query(uri).await?.err().ok_or("unreadable query accepted")?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, INVALID_QUERY, None),
                "{uri}"
            );
            Ok(())
        })
    }

    /// A query member no schema names is skipped, and one named twice is the 400 of a query
    /// that does not deserialise; the query writes every member, an absent one as `null`.
    #[test]
    fn the_query_skips_an_unknown_member_and_refuses_a_repeated_one() -> TestResult {
        block_on(async {
            let parsed = query(&format!(
                "/authorizations?page=2&page_key={KEY}&acct_id=1&page_size=5"
            ))
            .await?
            .map_err(|error| error.to_string())?;
            assert_eq!(
                (
                    parsed.page_key.as_deref(),
                    parsed.direction,
                    parsed.page_size
                ),
                (Some(KEY), None, Some(5))
            );
            assert_eq!(
                serde_json::to_string(&parsed)?,
                format!(r#"{{"page_key":"{KEY}","direction":null,"page_size":5}}"#)
            );

            for uri in [
                format!("/authorizations?page_key={KEY}&page_key={NEXT_KEY}"),
                "/authorizations?direction=forward&direction=forward".to_owned(),
                "/authorizations?page_size=5&page_size=5".to_owned(),
            ] {
                let error = query(&uri).await?.err().ok_or("repeated member accepted")?;
                assert_eq!(
                    (error.status(), error.message(), error.field()),
                    (StatusCode::BAD_REQUEST, INVALID_QUERY, None),
                    "{uri}"
                );
            }
            Ok(())
        })
    }

    #[test]
    fn the_query_names_the_page_request() -> TestResult {
        let anchor = key(75_968, 869_490_879)?;
        let cases = [
            (None, None, rules::PageRequest::First),
            (
                Some(PageDirection::Forward),
                None,
                rules::PageRequest::First,
            ),
            (
                Some(PageDirection::Backward),
                None,
                rules::PageRequest::First,
            ),
            (None, Some(KEY), rules::PageRequest::After(anchor)),
            (
                Some(PageDirection::Forward),
                Some(KEY),
                rules::PageRequest::After(anchor),
            ),
            (
                Some(PageDirection::Backward),
                Some(KEY),
                rules::PageRequest::From(anchor),
            ),
        ];
        for (direction, page_key, expected) in cases {
            let query = AuthSummaryQuery {
                page_key: page_key.map(str::to_owned),
                direction,
                page_size: None,
            };
            assert_eq!(rules::PageRequest::try_from(&query), Ok(expected));
        }
        let malformed = AuthSummaryQuery {
            page_key: Some("ABC".to_owned()),
            direction: Some(PageDirection::Forward),
            page_size: Some(5),
        };
        assert_eq!(
            rules::PageRequest::try_from(&malformed),
            Err(rules::AuthKeyError)
        );
        Ok(())
    }

    #[test]
    fn the_contract_names_and_bounds_its_schemas_and_parameters() -> TestResult {
        assert_eq!(AuthSummaryQuery::name(), "AuthSummaryQuery");
        assert_eq!(AuthSummaryBlock::name(), "AuthSummaryBlock");
        assert_eq!(AuthSummaryRow::name(), "AuthSummaryRow");
        assert_eq!(AuthSummaryResponse::name(), "AuthSummaryResponse");

        let params = serde_json::to_value(AuthSummaryQuery::into_params(|| None))?;
        let params = params.as_array().ok_or("parameters are not a list")?;
        let names: Vec<&str> = params
            .iter()
            .filter_map(|param| param["name"].as_str())
            .collect();
        assert_eq!(names, ["page_key", "direction", "page_size"]);
        for param in params {
            assert_eq!(param["in"], "query", "{param}");
            assert_ne!(param["required"], true, "{param}");
        }
        // TRUNCATION: checked, never lossy. The rendered key is 16 characters wide and `usize` is
        // at most 64 bits, so `u64` holds it; `?` refuses any other value, never wrapping it.
        let width = u64::try_from(rules::AuthKey::RENDERED_WIDTH)?;
        let page_key = &params.first().ok_or("no page_key parameter")?["schema"];
        assert_eq!(page_key["minLength"], width);
        assert_eq!(page_key["maxLength"], width);
        let page_size = &params.get(2).ok_or("no page_size parameter")?["schema"];
        assert_eq!(page_size["minimum"], 1);
        assert_eq!(page_size["maximum"], 100);

        let schema = serde_json::to_value(AuthSummaryQuery::schema())?;
        assert_eq!(schema["properties"]["page_key"]["maxLength"], width);
        assert_eq!(schema["properties"]["page_size"]["maximum"], 100);

        // No schema names a card number, a CVV or an SSN, nor the never-filled account status.
        for schema in [
            AuthSummaryBlock::schema(),
            AuthSummaryRow::schema(),
            AuthSummaryResponse::schema(),
        ] {
            let json = serde_json::to_value(schema)?;
            for property in members(&json["properties"]) {
                for word in ["card", "cvv", "ssn", "status"] {
                    assert!(
                        !property.contains(word) || property == "match_status",
                        "{property}"
                    );
                }
            }
        }
        Ok(())
    }
}
