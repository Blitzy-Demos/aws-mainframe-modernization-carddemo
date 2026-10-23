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

use domain::copybook::PicU;
use domain::online::copaus0c as rules;
use domain::sensitive::redact_free_text;
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, IntoParams, Validate)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;

    use axum::extract::FromRequestParts;
    use axum::http::{Request, StatusCode};
    use domain::copybook::{PicS, PicU};
    use domain::online::copaus0c as rules;
    use serde_json::Value;
    use utoipa::{IntoParams, PartialSchema, ToSchema};

    use super::{
        AuthSummaryBlock, AuthSummaryQuery, AuthSummaryResponse, AuthSummaryRow, PageDirection,
    };
    use crate::dto::{
        FIELD_LENGTH_MESSAGE, INVALID_QUERY, PAGE_SIZE_FIELD, PAGE_SIZE_MESSAGE, ValidQuery,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

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

    #[tokio::test]
    async fn the_query_string_is_parsed_and_validated() -> TestResult {
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

        for uri in [
            "/authorizations?direction=sideways",
            "/authorizations?page_size=five",
        ] {
            let error = query(uri).await?.err().ok_or("unreadable query accepted")?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, INVALID_QUERY, None),
                "{uri}"
            );
        }
        Ok(())
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
