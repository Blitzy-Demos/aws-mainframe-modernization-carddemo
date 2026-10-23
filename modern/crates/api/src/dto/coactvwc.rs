//! The contract of endpoint 4, `GET /api/v1/accounts/{acct_id}/view` (transaction `CAVW`,
//! program COACTVWC, map `CACTVWA`; AAP 0.3.4.1): the [`AccountViewResponse`] schema and its
//! conversion from `domain::online::coactvwc::AccountView`, and the [`AccountViewError`] body of
//! its 404 and 500, from `domain::online::coactvwc::ViewFailure`.
//!
//! The endpoint has no request DTO. Its one input is the raw text of the 11-character `ACCTSID`
//! field (COACTVW-013), which the handler takes as the `acct_id` path `String` and which
//! `domain::online::coactvwc::edit_account` edits with the program's own messages (COACTVWC-029
//! to COACTVWC-032).
//!
//! # What a 200 carries
//!
//! The account and customer blocks of a view whose three reads all found their record, and
//! nothing else (COACTVWC-008):
//!
//! - **No message.** COACTVWC moves none to the error row of a successful view (COACTVWC-008),
//!   so the row stays empty (COACTVW-099).
//! - **No information message.** The information row always shows one constant prompt, which
//!   the screen renders itself; no endpoint 4 response carries it (COACTVWC-027, COACTVW-096).
//! - **The account number as a key only.** `acct_id` names the account the view describes, the
//!   eleven digits every read used; the screen keeps the text it sent in its own field and does
//!   not refill it from the response (COACTVWC-007).
//!
//! The two blocks are [`AccountViewAccountBlock`] and [`AccountViewCustomerBlock`], flattened
//! into the response, so its members sit side by side as the map shows them.
//!
//! # What an error carries
//!
//! Every other outcome is an error (400, 404 or 500, COACTVWC-030 to COACTVWC-047). A failed
//! edit, the 400, is a bare `api::error::ScreenError`. A 404 or 500 is [`AccountViewError`]: the
//! `ScreenError` members `message` and `field` unchanged, and beside them the blocks the COBOL
//! still shows with the message (COACTVWC-008, X-COACTVWC-6):
//!
//! - account found, customer not found (404, COACTVWC-044) or failed (500, COACTVWC-046):
//!   `account`, the account's values;
//! - account not found (404, COACTVWC-040) or failed (500, COACTVWC-042), customer found:
//!   `customer`, the customer's values, and `account`, whose values are those of halt candidate
//!   X-COACTVWC-H1, a placeholder that is no parity result (COACTVWC-009);
//! - every other read outcome, and a store failure with no audited message: neither.
//!
//! # Cardholder data
//!
//! COACTVWC shows the full SSN (`COACTVWC.cbl:495-504`). Here the field stays a
//! `domain::sensitive::Ssn`, moved from the domain view unchanged, whose `Serialize` writes only
//! `***-**-` and the last four digits, and the schema example is redacted too (COACTVWC-017,
//! E32). The customer block of an error body carries the same `Ssn`. No card number is part of
//! the view or of an error body (COACTVWC-034).

use axum::Json;
use axum::response::{IntoResponse, Response};
use chrono::NaiveDate;
use domain::online::coactvwc as rules;
use domain::sensitive::Ssn;
use serde::Serialize;
use utoipa::ToSchema;

use super::{digits, full_text};
use crate::error::{ScreenError, ServiceError};

/// The account view of endpoint 4 (CAVW, COACTVWC): the account and customer blocks of map
/// `CACTVWA` after all three reads found their record (COACTVWC-008).
///
/// Text fields hold every byte of their map field, trailing spaces included: the stored value
/// unchanged, or its first characters where the map field is narrower (ZIP code and phone
/// numbers). Numeric identifiers hold their digits with the leading zeros, and amounts the
/// 15-character text of the map's `+ZZZ,ZZZ,ZZZ.99` field, so the client formats no number.
/// Dates are typed, `null` for a blank field. The SSN is only ever its redacted form.
// The schema name `AccountViewResponse` is the frontend's
// `components["schemas"]["AccountViewResponse"]` (Generated API Types Only). There is no
// `Deserialize`: the SSN travels redacted and cannot be read back.
//
// No `message` or `info_message` member: COACTVWC sets neither on a successful view, and the
// information row is the screen's constant prompt that no response carries (COACTVWC-008,
// COACTVWC-027, COACTVW-096).
//
// The two blocks are flattened, so the body is one object whose members follow the map, as
// before the blocks had types of their own; the schema is `allOf` the two block schemas and
// `acct_id`. The same block types travel in `AccountViewError`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountViewResponse {
    /// The account number every read used: eleven digits, leading zeros included
    /// (COACTVWC-032).
    pub acct_id: String,
    /// The account block (COACTVWC-010 to COACTVWC-015).
    #[serde(flatten)]
    pub account: AccountViewAccountBlock,
    /// The customer block (COACTVWC-016 to COACTVWC-026).
    #[serde(flatten)]
    pub customer: AccountViewCustomerBlock,
}

/// The account block of map `CACTVWA`, as `1200-SETUP-SCREEN-VARS` moves it from
/// `ACCOUNT-RECORD` (COACTVWC-010 to COACTVWC-015): part of every [`AccountViewResponse`], and of
/// an [`AccountViewError`] when COACTVWC shows it beside the message (COACTVWC-008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountViewAccountBlock {
    /// `ACCT-ACTIVE-STATUS`, the one-character active flag of `ACSTTUS` (COACTVWC-010,
    /// COACTVW-016).
    pub active_status: String,
    /// `ACCT-OPEN-DATE`, shown in `ADTOPEN`; `null` when blank (COACTVWC-014, COACTVW-019).
    pub open_date: Option<NaiveDate>,
    /// `ACCT-EXPIRAION-DATE`, shown in `AEXPDT`; `null` when blank (COACTVWC-014, COACTVW-025).
    pub expiration_date: Option<NaiveDate>,
    /// `ACCT-REISSUE-DATE`, shown in `AREISDT`; `null` when blank (COACTVWC-014, COACTVW-031).
    pub reissue_date: Option<NaiveDate>,
    /// `ACCT-CREDIT-LIMIT` as the 15-character `ACRDLIM` field edits it (COACTVWC-012,
    /// COACTVW-022).
    pub credit_limit: String,
    /// `ACCT-CASH-CREDIT-LIMIT` as the 15-character `ACSHLIM` field edits it (COACTVWC-012,
    /// COACTVW-028).
    pub cash_credit_limit: String,
    /// `ACCT-CURR-BAL` as the 15-character `ACURBAL` field edits it (COACTVWC-011,
    /// COACTVW-034).
    pub current_balance: String,
    /// `ACCT-CURR-CYC-CREDIT` as the 15-character `ACRCYCR` field edits it (COACTVWC-013,
    /// COACTVW-037).
    pub current_cycle_credit: String,
    /// `ACCT-CURR-CYC-DEBIT` as the 15-character `ACRCYDB` field edits it (COACTVWC-013,
    /// COACTVW-043).
    pub current_cycle_debit: String,
    /// `ACCT-GROUP-ID`, the 10-character `AADDGRP` field (COACTVWC-015, COACTVW-040).
    pub group_id: String,
}

/// The customer block of map `CACTVWA`, as `1200-SETUP-SCREEN-VARS` moves it from
/// `CUSTOMER-RECORD` (COACTVWC-016 to COACTVWC-026): part of every [`AccountViewResponse`], and
/// of an [`AccountViewError`] when COACTVWC shows it beside the message (COACTVWC-008). The SSN
/// is only ever its redacted form.
// No `Deserialize`: the SSN travels redacted and cannot be read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountViewCustomerBlock {
    /// `CUST-ID`: nine digits, leading zeros included (COACTVWC-016, COACTVW-047).
    pub customer_id: String,
    /// `CUST-SSN`, redacted: `***-**-` followed by its last four digits, never the full
    /// number (COACTVWC-017, COACTVW-050).
    #[schema(value_type = String, example = "***-**-1234")]
    pub ssn: Ssn,
    /// `CUST-DOB-YYYY-MM-DD`, shown in `ACSTDOB`; `null` when blank (COACTVWC-019, COACTVW-053).
    pub date_of_birth: Option<NaiveDate>,
    /// `CUST-FICO-CREDIT-SCORE`: three digits, leading zeros included (COACTVWC-018,
    /// COACTVW-056).
    pub fico_score: String,
    /// `CUST-FIRST-NAME`, the 25-character `ACSFNAM` field (COACTVWC-020, COACTVW-061).
    pub first_name: String,
    /// `CUST-MIDDLE-NAME`, the 25-character `ACSMNAM` field (COACTVWC-020, COACTVW-063).
    pub middle_name: String,
    /// `CUST-LAST-NAME`, the 25-character `ACSLNAM` field (COACTVWC-020, COACTVW-065).
    pub last_name: String,
    /// `CUST-ADDR-LINE-1`, the 50-character `ACSADL1` field (COACTVWC-021, COACTVW-067).
    pub address_line_1: String,
    /// `CUST-ADDR-LINE-2`, the 50-character `ACSADL2` field (COACTVWC-021, COACTVW-072).
    pub address_line_2: String,
    /// `CUST-ADDR-LINE-3`, the 50-character field the map labels `City` (COACTVWC-021,
    /// COACTVW-078).
    pub city: String,
    /// `CUST-ADDR-STATE-CD`, the 2-character `ACSSTTE` field (COACTVWC-022, COACTVW-070).
    pub state: String,
    /// `CUST-ADDR-ZIP`: the first five characters only, as the 5-character `ACSZIPC` field
    /// holds them (COACTVWC-023, COACTVW-075).
    pub zip: String,
    /// `CUST-ADDR-COUNTRY-CD`, the 3-character `ACSCTRY` field (COACTVWC-022, COACTVW-081).
    pub country: String,
    /// `CUST-PHONE-NUM-1`: the first 13 characters only, as `ACSPHN1` holds them
    /// (COACTVWC-024, COACTVW-084).
    pub phone_1: String,
    /// `CUST-PHONE-NUM-2`: the first 13 characters only, as `ACSPHN2` holds them
    /// (COACTVWC-024, COACTVW-089).
    pub phone_2: String,
    /// `CUST-GOVT-ISSUED-ID`, the 20-character `ACSGOVT` field (COACTVWC-025, COACTVW-086).
    pub government_id: String,
    /// `CUST-EFT-ACCOUNT-ID`, the 10-character `ACSEFTC` field (COACTVWC-025, COACTVW-091).
    pub eft_account_id: String,
    /// `CUST-PRI-CARD-HOLDER-IND`, the one-character `ACSPFLG` flag (COACTVWC-026,
    /// COACTVW-094).
    pub primary_card_holder: String,
}

/// The view as `1200-SETUP-SCREEN-VARS` fills the map: the key with its leading zeros and the
/// two blocks (COACTVWC-010 to COACTVWC-026, COACTVWC-032).
impl From<rules::AccountView> for AccountViewResponse {
    fn from(view: rules::AccountView) -> Self {
        let rules::AccountView {
            acct_id,
            account,
            customer,
        } = view;
        Self {
            acct_id: digits(acct_id),
            account: AccountViewAccountBlock::from(account),
            customer: AccountViewCustomerBlock::from(customer),
        }
    }
}

/// The account block field for field: text as all the bytes of its map field and the domain's
/// edited amounts and typed dates unchanged (COACTVWC-010 to COACTVWC-015).
impl From<rules::AccountBlock> for AccountViewAccountBlock {
    fn from(account: rules::AccountBlock) -> Self {
        Self {
            active_status: full_text(&account.active_status),
            open_date: account.open_date,
            expiration_date: account.expiration_date,
            reissue_date: account.reissue_date,
            credit_limit: account.credit_limit,
            cash_credit_limit: account.cash_credit_limit,
            current_balance: account.current_balance,
            current_cycle_credit: account.current_cycle_credit,
            current_cycle_debit: account.current_cycle_debit,
            group_id: full_text(&account.group_id),
        }
    }
}

/// The customer block field for field: text as all the bytes of its map field, identifiers
/// with their leading zeros, the typed date unchanged and the [`Ssn`] moved as it is
/// (COACTVWC-016 to COACTVWC-026).
impl From<rules::CustomerBlock> for AccountViewCustomerBlock {
    fn from(customer: rules::CustomerBlock) -> Self {
        Self {
            customer_id: digits(customer.customer_id),
            ssn: customer.ssn,
            date_of_birth: customer.date_of_birth,
            fico_score: digits(customer.fico_score),
            first_name: full_text(&customer.first_name),
            middle_name: full_text(&customer.middle_name),
            last_name: full_text(&customer.last_name),
            address_line_1: full_text(&customer.address_line_1),
            address_line_2: full_text(&customer.address_line_2),
            city: full_text(&customer.city),
            state: full_text(&customer.state),
            zip: full_text(&customer.zip),
            country: full_text(&customer.country),
            phone_1: full_text(&customer.phone_1),
            phone_2: full_text(&customer.phone_2),
            government_id: full_text(&customer.government_id),
            eft_account_id: full_text(&customer.eft_account_id),
            primary_card_holder: full_text(&customer.primary_card_holder),
        }
    }
}

/// The body of an endpoint 4 error: the `ScreenError` of the message, and the blocks COACTVWC
/// still shows beside it (COACTVWC-008, X-COACTVWC-6).
///
/// Serialised as `{"message": …, "field": …, "account": …, "customer": …}`, each of the last
/// three omitted when absent, so an error without a block is exactly its `ScreenError`. The
/// status is the `ScreenError`'s and travels as the response status only.
///
/// - `account`: after the account read found the account and the customer read did not
///   (COACTVWC-044, COACTVWC-046), and after the account read did not succeed and the customer
///   read found the customer, when its values are halt candidate X-COACTVWC-H1's placeholder
///   (COACTVWC-009).
/// - `customer`: only when the customer read found the customer and the account read did not
///   succeed (COACTVWC-040, COACTVWC-042).
// Errors keep the `ScreenError { message, field? }` shape: those two members are the flattened
// `ScreenError`, unchanged, and the blocks are optional members beside them, which the
// `ScreenError` schema, requiring only `message`, leaves open. A bare `ScreenError` would drop
// the blocks the COBOL shows with these messages, which the parity rows COACTVWC-008 and
// COACTVWC-009 require. The schema is `allOf` `ScreenError` and the two optional blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountViewError {
    /// The message and its field, as the `ScreenError` whose status the response carries.
    #[serde(flatten)]
    pub error: ScreenError,
    /// The account block shown beside the message, if any (COACTVWC-008, COACTVWC-009).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountViewAccountBlock>,
    /// The customer block shown beside the message, if any (COACTVWC-008).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<AccountViewCustomerBlock>,
}

/// The program's failure: its `ScreenError`, through `api::error`, and the blocks the domain
/// set beside it.
impl From<rules::ViewFailure> for AccountViewError {
    fn from(failure: rules::ViewFailure) -> Self {
        let account = failure
            .account()
            .cloned()
            .map(AccountViewAccountBlock::from);
        let customer = failure
            .customer()
            .cloned()
            .map(AccountViewCustomerBlock::from);
        Self {
            error: ScreenError::from(failure),
            account,
            customer,
        }
    }
}

/// The service's error: the program's failure with its blocks, or the `ScreenError` of a store
/// or internal failure with no audited message, which shows no block. Endpoint 4 runs no
/// password work, so `ServiceError::Saturated` never reaches it; it would show no block either.
impl From<ServiceError<rules::ViewFailure>> for AccountViewError {
    fn from(error: ServiceError<rules::ViewFailure>) -> Self {
        match error {
            ServiceError::Rule(failure) => Self::from(failure),
            other @ (ServiceError::Store(_) | ServiceError::Internal | ServiceError::Saturated) => {
                Self {
                    error: ScreenError::from(other),
                    account: None,
                    customer: None,
                }
            }
        }
    }
}

/// The `ScreenError`'s status, then the body as JSON without it.
impl IntoResponse for AccountViewError {
    fn into_response(self) -> Response {
        let status = self.error.status();
        (status, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use chrono::NaiveDate;
    use domain::CardNumber;
    use domain::copybook::{FixedRecord, PicU, PicX};
    use domain::records::account::AccountRecord;
    use domain::records::card_xref::CardXrefRecord;
    use domain::records::customer::CustomerRecord;
    use persistence::PersistenceError;
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};

    use crate::error::{ServiceError, UNABLE_TO_PROCESS};

    use super::{
        AccountViewAccountBlock, AccountViewCustomerBlock, AccountViewError, AccountViewResponse,
        rules,
    };

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A synthetic SSN that no fixture holds, in its two plain forms.
    const SSN_DIGITS: &str = "123456789";
    const SSN_DASHED: &str = "123-45-6789";

    /// `text` left-justified in a field of `width` characters.
    fn field(text: &str, width: usize) -> String {
        format!("{text:<width$}")
    }

    /// A 300-byte `CVACT01Y` line: account 42, balance 1,234.56, limits 5,000.00 and 1,500.00,
    /// cycle credit zero and cycle debit -25.50 in trailing overpunch, a blank reissue date and
    /// a group ID with trailing spaces.
    fn account_line() -> String {
        [
            "00000000042",
            "Y",
            "00000012345F",
            "00000050000{",
            "00000015000{",
            "2014-11-20",
            "2025-05-20",
            &field("", 10),
            "00000000000{",
            "00000000255}",
            "A000000000",
            &field("GRP01", 10),
            &field("", 178),
        ]
        .concat()
    }

    /// A 500-byte `CVCUS01Y` line for customer 7, with a 10-character ZIP code and 15-character
    /// phone numbers that the map fields narrow.
    fn customer_line() -> String {
        [
            "000000007",
            &field("JOHN", 25),
            &field("Q", 25),
            &field("PUBLIC", 25),
            &field("1 MAIN ST", 50),
            &field("APT 2", 50),
            &field("SPRINGFIELD", 50),
            "IL",
            "USA",
            "62701-1234",
            "(555)555-0100XY",
            "(555)555-0199  ",
            SSN_DIGITS,
            &field("GOVT-ID-0001", 20),
            "1980-01-15",
            "0000000042",
            "Y",
            "075",
            &field("", 168),
        ]
        .concat()
    }

    /// How a read of the chain ends, without its record.
    #[derive(Debug, Clone, Copy)]
    enum Read {
        Found,
        NotFound,
        Failed,
    }

    impl Read {
        /// The read's outcome over `record`.
        const fn of<T>(self, record: &T) -> rules::Lookup<&T> {
            match self {
                Self::Found => rules::Lookup::Found(record),
                Self::NotFound => rules::Lookup::NotFound,
                Self::Failed => rules::Lookup::Error,
            }
        }
    }

    /// The domain's answer for the synthetic account 42 and customer 7 when the cross-reference
    /// read finds its row and the account and customer reads end as `account` and `customer`.
    fn outcome(
        account: Read,
        customer: Read,
    ) -> Result<Result<rules::AccountView, rules::ViewFailure>, Box<dyn StdError>> {
        let stored_account = AccountRecord::decode(account_line().as_bytes())?;
        let stored_customer = CustomerRecord::decode(customer_line().as_bytes())?;
        let xref = CardXrefRecord {
            card_num: CardNumber::parse("4111111111111111")?,
            cust_id: stored_customer.id,
            acct_id: stored_account.id,
            filler: PicX::spaces(),
        };
        let key = PicU::<11>::new(42)?;
        Ok(rules::view(
            key,
            rules::Lookup::Found(&xref),
            account.of(&stored_account),
            customer.of(&stored_customer),
        ))
    }

    /// The domain view of the synthetic account and customer, all three reads found.
    fn domain_view() -> Result<rules::AccountView, Box<dyn StdError>> {
        Ok(outcome(Read::Found, Read::Found)??)
    }

    /// The domain's failure when the account and customer reads end as `account` and `customer`.
    fn domain_failure(
        account: Read,
        customer: Read,
    ) -> Result<rules::ViewFailure, Box<dyn StdError>> {
        match outcome(account, customer)? {
            Ok(_) => {
                Err(format!("{account:?} account and {customer:?} customer gave a view").into())
            }
            Err(failure) => Ok(failure),
        }
    }

    fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, &'static str> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or("invalid test date")
    }

    /// The account block, COACTVWC-010 to COACTVWC-015, and the key, COACTVWC-032.
    #[test]
    fn the_account_block_maps_field_for_field() -> TestResult {
        let view = domain_view()?;
        let edited = view.account.clone();
        let response = AccountViewResponse::from(view);
        assert_eq!(response.acct_id, "00000000042");
        let account = &response.account;
        assert_eq!(account.active_status, "Y");
        assert_eq!(account.open_date, Some(date(2014, 11, 20)?));
        assert_eq!(account.expiration_date, Some(date(2025, 5, 20)?));
        assert_eq!(account.reissue_date, None);
        // COACTVWC-015: the stored 10 characters, trailing spaces included.
        assert_eq!(account.group_id, field("GRP01", 10));
        // The domain's edited amounts pass through unchanged, each to its own member.
        let amounts = [
            (&account.current_balance, &edited.current_balance),
            (&account.credit_limit, &edited.credit_limit),
            (&account.cash_credit_limit, &edited.cash_credit_limit),
            (&account.current_cycle_credit, &edited.current_cycle_credit),
            (&account.current_cycle_debit, &edited.current_cycle_debit),
        ];
        for (sent, shown) in amounts {
            assert_eq!(sent, shown);
            assert_eq!(sent.chars().count(), 15, "{sent:?}");
        }
        assert_eq!(account.current_balance, "+      1,234.56");
        assert_eq!(account.current_cycle_debit, "-         25.50");
        Ok(())
    }

    /// The customer block, COACTVWC-016 and COACTVWC-018 to COACTVWC-026.
    #[test]
    fn the_customer_block_maps_field_for_field() -> TestResult {
        let customer = AccountViewResponse::from(domain_view()?).customer;
        assert_eq!(customer.customer_id, "000000007");
        assert_eq!(customer.fico_score, "075");
        assert_eq!(customer.date_of_birth, Some(date(1980, 1, 15)?));
        // Each the stored value unchanged, every byte of its map field, trailing spaces
        // included (COACTVWC-020 to COACTVWC-022, COACTVWC-025, COACTVWC-026).
        assert_eq!(
            [
                customer.first_name.as_str(),
                &customer.middle_name,
                &customer.last_name,
                &customer.address_line_1,
                &customer.address_line_2,
                &customer.city,
                &customer.state,
                &customer.country,
                &customer.government_id,
                &customer.eft_account_id,
                &customer.primary_card_holder,
            ],
            [
                field("JOHN", 25),
                field("Q", 25),
                field("PUBLIC", 25),
                field("1 MAIN ST", 50),
                field("APT 2", 50),
                field("SPRINGFIELD", 50),
                field("IL", 2),
                field("USA", 3),
                field("GOVT-ID-0001", 20),
                field("0000000042", 10),
                field("Y", 1),
            ]
        );
        // The narrower map fields keep the leftmost characters (COACTVWC-023, COACTVWC-024).
        assert_eq!(customer.zip, "62701");
        assert_eq!(customer.phone_1, "(555)555-0100");
        assert_eq!(customer.phone_2, "(555)555-0199");
        Ok(())
    }

    /// The wire form: the SSN redacted (COACTVWC-017, E32), no card number (COACTVWC-034),
    /// typed dates (COACTVWC-014, COACTVWC-019) and no message member (COACTVWC-008,
    /// COACTVWC-027).
    #[test]
    fn the_body_carries_the_redacted_ssn_and_typed_dates_only() -> TestResult {
        let body = serde_json::to_value(AccountViewResponse::from(domain_view()?))?;
        assert_eq!(body["ssn"], json!("***-**-6789"));
        assert_eq!(body["open_date"], json!("2014-11-20"));
        assert_eq!(body["date_of_birth"], json!("1980-01-15"));
        assert_eq!(body["reissue_date"], Value::Null);
        let text = body.to_string();
        assert!(!text.contains(SSN_DIGITS) && !text.contains(SSN_DASHED));
        assert!(!text.contains("4111111111111111"));
        let mut keys: Vec<&str> = body
            .as_object()
            .ok_or("the body is not an object")?
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "acct_id",
                "active_status",
                "address_line_1",
                "address_line_2",
                "cash_credit_limit",
                "city",
                "country",
                "credit_limit",
                "current_balance",
                "current_cycle_credit",
                "current_cycle_debit",
                "customer_id",
                "date_of_birth",
                "eft_account_id",
                "expiration_date",
                "fico_score",
                "first_name",
                "government_id",
                "group_id",
                "last_name",
                "middle_name",
                "open_date",
                "phone_1",
                "phone_2",
                "primary_card_holder",
                "reissue_date",
                "ssn",
                "state",
                "zip",
            ]
        );
        Ok(())
    }

    /// The member order of the 200 body: the map's, as before the blocks had types of their
    /// own, so flattening them changed no byte of the body.
    #[test]
    fn the_body_keeps_the_member_order_of_the_map() -> TestResult {
        let text = serde_json::to_string(&AccountViewResponse::from(domain_view()?))?;
        let order = [
            "acct_id",
            "active_status",
            "open_date",
            "expiration_date",
            "reissue_date",
            "credit_limit",
            "cash_credit_limit",
            "current_balance",
            "current_cycle_credit",
            "current_cycle_debit",
            "group_id",
            "customer_id",
            "ssn",
            "date_of_birth",
            "fico_score",
            "first_name",
            "middle_name",
            "last_name",
            "address_line_1",
            "address_line_2",
            "city",
            "state",
            "zip",
            "country",
            "phone_1",
            "phone_2",
            "government_id",
            "eft_account_id",
            "primary_card_holder",
        ];
        let positions = order
            .iter()
            .map(|key| {
                text.find(&format!("\"{key}\":"))
                    .ok_or_else(|| format!("no member {key}"))
            })
            .collect::<Result<Vec<usize>, String>>()?;
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "members out of map order: {text}"
        );
        Ok(())
    }

    /// The schemas the frontend's generated types read: the names, the view as `allOf` its two
    /// blocks and `acct_id`, the redacted SSN example and the `format: date` of the four dates.
    #[test]
    fn the_schema_types_the_ssn_as_a_redacted_string_and_the_dates_as_dates() -> TestResult {
        assert_eq!(AccountViewResponse::name(), "AccountViewResponse");
        assert_eq!(AccountViewAccountBlock::name(), "AccountViewAccountBlock");
        assert_eq!(AccountViewCustomerBlock::name(), "AccountViewCustomerBlock");
        let schema = serde_json::to_value(AccountViewResponse::schema())?;
        let parts = schema["allOf"].as_array().ok_or("the view is not allOf")?;
        let references: Vec<&Value> = parts.iter().filter_map(|part| part.get("$ref")).collect();
        assert_eq!(
            references,
            [
                &json!("#/components/schemas/AccountViewAccountBlock"),
                &json!("#/components/schemas/AccountViewCustomerBlock"),
            ]
        );
        assert!(
            parts
                .iter()
                .any(|part| part["required"] == json!(["acct_id"])
                    && part["properties"]["acct_id"]["type"] == json!("string")),
            "the view's own part requires acct_id"
        );

        let account = serde_json::to_value(AccountViewAccountBlock::schema())?;
        for name in ["open_date", "expiration_date", "reissue_date"] {
            assert_eq!(
                account["properties"][name]["format"],
                json!("date"),
                "{name}"
            );
        }
        let account_required = account["required"].as_array().ok_or("no required list")?;
        assert!(account_required.contains(&json!("active_status")));
        assert!(!account_required.contains(&json!("open_date")));

        let customer = serde_json::to_value(AccountViewCustomerBlock::schema())?;
        let properties = &customer["properties"];
        assert_eq!(properties["ssn"]["type"], json!("string"));
        assert_eq!(properties["ssn"]["example"], json!("***-**-1234"));
        assert_eq!(properties["date_of_birth"]["format"], json!("date"));
        let required = customer["required"].as_array().ok_or("no required list")?;
        assert!(required.contains(&json!("ssn")));
        assert!(!required.contains(&json!("date_of_birth")));
        for block in [&account, &customer] {
            let members = &block["properties"];
            assert!(members.get("message").is_none() && members.get("info_message").is_none());
        }
        Ok(())
    }

    /// The account block's members, as the 200 body of the same records carries them.
    fn expected_account() -> Result<AccountViewAccountBlock, Box<dyn StdError>> {
        Ok(AccountViewResponse::from(domain_view()?).account)
    }

    /// The customer block's members, as the 200 body of the same records carries them.
    fn expected_customer() -> Result<AccountViewCustomerBlock, Box<dyn StdError>> {
        Ok(AccountViewResponse::from(domain_view()?).customer)
    }

    /// The error body of every outcome of the account and customer reads after a found
    /// cross-reference: the status, the field and which blocks travel beside the message
    /// (COACTVWC-008, COACTVWC-009, COACTVWC-033, COACTVWC-040, COACTVWC-042, COACTVWC-044,
    /// COACTVWC-046). The account block of an account that was not read is present, and its
    /// values, halt candidate X-COACTVWC-H1's, are not asserted.
    #[test]
    fn an_error_carries_the_blocks_the_program_shows_beside_its_message() -> TestResult {
        // (account read, customer read, status, field, account block, customer block)
        let cases = [
            (Read::Found, Read::NotFound, 404, None, true, false),
            (Read::Found, Read::Failed, 500, None, true, false),
            (
                Read::NotFound,
                Read::Found,
                404,
                Some("acct_id"),
                true,
                true,
            ),
            (Read::Failed, Read::Found, 500, Some("acct_id"), true, true),
            (
                Read::NotFound,
                Read::NotFound,
                404,
                Some("acct_id"),
                false,
                false,
            ),
            (
                Read::NotFound,
                Read::Failed,
                500,
                Some("acct_id"),
                false,
                false,
            ),
            (
                Read::Failed,
                Read::NotFound,
                500,
                Some("acct_id"),
                false,
                false,
            ),
            (
                Read::Failed,
                Read::Failed,
                500,
                Some("acct_id"),
                false,
                false,
            ),
        ];
        let account = expected_account()?;
        let customer = expected_customer()?;
        for (account_read, customer_read, status, field, with_account, with_customer) in cases {
            let label = format!("{account_read:?} account, {customer_read:?} customer");
            let failure = domain_failure(account_read, customer_read)?;
            let message = failure.message().to_owned();
            let body = AccountViewError::from(failure);
            assert_eq!(body.error.status().as_u16(), status, "{label}");
            assert_eq!(body.error.message(), message, "{label}");
            assert_eq!(body.error.field(), field, "{label}");
            assert_eq!(body.account.is_some(), with_account, "{label}");
            assert_eq!(
                body.customer.as_ref(),
                with_customer.then_some(&customer),
                "{label}"
            );
            if matches!(account_read, Read::Found) {
                assert_eq!(body.account.as_ref(), Some(&account), "{label}");
            }
        }
        Ok(())
    }

    /// The wire form of an error with both blocks: the `ScreenError` members at the top level,
    /// each block a nested object, the SSN redacted (COACTVWC-017) and no card number
    /// (COACTVWC-034); an error without a block is exactly its `ScreenError`.
    #[test]
    fn an_error_body_is_its_screen_error_with_the_blocks_beside_it() -> TestResult {
        let both = AccountViewError::from(domain_failure(Read::NotFound, Read::Found)?);
        let body = serde_json::to_value(&both)?;
        let mut keys: Vec<&str> = body
            .as_object()
            .ok_or("the body is not an object")?
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["account", "customer", "field", "message"]);
        assert_eq!(body["field"], json!("acct_id"));
        assert_eq!(body["customer"]["ssn"], json!("***-**-6789"));
        assert_eq!(body["customer"]["customer_id"], json!("000000007"));
        assert_eq!(body["customer"]["date_of_birth"], json!("1980-01-15"));
        assert!(body["account"].is_object());
        let text = body.to_string();
        assert!(!text.contains(SSN_DIGITS) && !text.contains(SSN_DASHED));
        assert!(!text.contains("4111111111111111"));

        let bare = AccountViewError::from(domain_failure(Read::NotFound, Read::NotFound)?);
        assert_eq!(
            serde_json::to_value(&bare)?,
            serde_json::to_value(&bare.error)?
        );
        let response = both.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    /// An edit failure and a cross-reference failure carry no block (COACTVWC-008); a store or
    /// internal failure with no audited message is the API's 500 with no block.
    #[test]
    fn an_error_before_the_master_reads_carries_no_block() -> TestResult {
        let edit = AccountViewError::from(ServiceError::Rule(
            rules::edit_account("")
                .err()
                .ok_or("a blank account number passed the edit")?,
        ));
        assert_eq!(edit.error.status(), StatusCode::BAD_REQUEST);
        let stored_customer = CustomerRecord::decode(customer_line().as_bytes())?;
        let stored_account = AccountRecord::decode(account_line().as_bytes())?;
        let key = PicU::<11>::new(42)?;
        let no_xref = AccountViewError::from(ServiceError::Rule(
            rules::view(
                key,
                rules::Lookup::NotFound,
                rules::Lookup::Found(&stored_account),
                rules::Lookup::Found(&stored_customer),
            )
            .err()
            .ok_or("a missing cross-reference gave a view")?,
        ));
        assert_eq!(no_xref.error.status(), StatusCode::NOT_FOUND);
        let store = AccountViewError::from(ServiceError::Store(PersistenceError::Unavailable));
        let internal = AccountViewError::from(ServiceError::<rules::ViewFailure>::Internal);
        for (label, body) in [
            ("edit", &edit),
            ("cross-reference", &no_xref),
            ("store", &store),
            ("internal", &internal),
        ] {
            assert!(
                body.account.is_none() && body.customer.is_none(),
                "{label}: a block"
            );
        }
        for body in [&store, &internal] {
            assert_eq!(body.error.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body.error.message(), UNABLE_TO_PROCESS);
            assert_eq!(body.error.field(), None);
        }
        Ok(())
    }

    /// The error schema: `allOf` `ScreenError` and the two blocks, neither block required.
    #[test]
    fn the_error_schema_is_the_screen_error_and_two_optional_blocks() -> TestResult {
        assert_eq!(AccountViewError::name(), "AccountViewError");
        let schema = serde_json::to_value(AccountViewError::schema())?;
        let parts = schema["allOf"].as_array().ok_or("the error is not allOf")?;
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["$ref"], json!("#/components/schemas/ScreenError"));
        let blocks = &parts[1];
        assert!(blocks.get("required").is_none(), "a block is required");
        let refs = |name: &str| blocks["properties"][name].to_string();
        assert!(refs("account").contains("#/components/schemas/AccountViewAccountBlock"));
        assert!(refs("customer").contains("#/components/schemas/AccountViewCustomerBlock"));
        Ok(())
    }
}
