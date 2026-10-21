//! The contract of endpoint 4, `GET /api/v1/accounts/{acct_id}/view` (transaction `CAVW`,
//! program COACTVWC, map `CACTVWA`; AAP 0.3.4.1): the [`AccountViewResponse`] schema and its
//! conversion from `domain::online::coactvwc::AccountView`.
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
//! Every other outcome is a `ScreenError` through `api::error` (400, 404 or 500, COACTVWC-030 to
//! COACTVWC-047). The account or customer block that the COBOL still shows beside some of those
//! messages (COACTVWC-008, escalation candidate X-COACTVWC-6) is not part of that body.
//!
//! # Cardholder data
//!
//! COACTVWC shows the full SSN (`COACTVWC.cbl:495-504`). Here the field stays a
//! `domain::sensitive::Ssn`, moved from the domain view unchanged, whose `Serialize` writes only
//! `***-**-` and the last four digits, and the schema example is redacted too (COACTVWC-017,
//! E32). No card number is part of the view (COACTVWC-034).

use chrono::NaiveDate;
use domain::online::coactvwc as rules;
use domain::sensitive::Ssn;
use serde::Serialize;
use utoipa::ToSchema;

use super::{digits, text};

/// The account view of endpoint 4 (CAVW, COACTVWC): the account and customer blocks of map
/// `CACTVWA` after all three reads found their record (COACTVWC-008).
///
/// Text fields hold the stored value without its trailing spaces, numeric identifiers their
/// digits with the leading zeros, and amounts the 15-character text of the map's
/// `+ZZZ,ZZZ,ZZZ.99` field, so the client formats no number. Dates are typed, `null` for a blank
/// field. The SSN is only ever its redacted form.
// The schema name `AccountViewResponse` is the frontend's
// `components["schemas"]["AccountViewResponse"]` (Generated API Types Only). There is no
// `Deserialize`: the SSN travels redacted and cannot be read back.
//
// No `message` or `info_message` member: COACTVWC sets neither on a successful view, and the
// information row is the screen's constant prompt that no response carries (COACTVWC-008,
// COACTVWC-027, COACTVW-096). The blocks the COBOL shows beside some failure messages
// (X-COACTVWC-6) have no member either, because every error body is `ScreenError` with only a
// message and a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountViewResponse {
    /// The account number every read used: eleven digits, leading zeros included
    /// (COACTVWC-032).
    pub acct_id: String,
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

/// The view field for field, as `1200-SETUP-SCREEN-VARS` fills the map: text without its
/// trailing spaces, identifiers with their leading zeros, the domain's edited amounts and typed
/// dates unchanged, and the [`Ssn`] moved as it is (COACTVWC-010 to COACTVWC-026).
impl From<rules::AccountView> for AccountViewResponse {
    fn from(view: rules::AccountView) -> Self {
        let rules::AccountView {
            acct_id,
            account,
            customer,
        } = view;
        Self {
            acct_id: digits(acct_id),
            active_status: text(&account.active_status),
            open_date: account.open_date,
            expiration_date: account.expiration_date,
            reissue_date: account.reissue_date,
            credit_limit: account.credit_limit,
            cash_credit_limit: account.cash_credit_limit,
            current_balance: account.current_balance,
            current_cycle_credit: account.current_cycle_credit,
            current_cycle_debit: account.current_cycle_debit,
            group_id: text(&account.group_id),
            customer_id: digits(customer.customer_id),
            ssn: customer.ssn,
            date_of_birth: customer.date_of_birth,
            fico_score: digits(customer.fico_score),
            first_name: text(&customer.first_name),
            middle_name: text(&customer.middle_name),
            last_name: text(&customer.last_name),
            address_line_1: text(&customer.address_line_1),
            address_line_2: text(&customer.address_line_2),
            city: text(&customer.city),
            state: text(&customer.state),
            zip: text(&customer.zip),
            country: text(&customer.country),
            phone_1: text(&customer.phone_1),
            phone_2: text(&customer.phone_2),
            government_id: text(&customer.government_id),
            eft_account_id: text(&customer.eft_account_id),
            primary_card_holder: text(&customer.primary_card_holder),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use domain::CardNumber;
    use domain::copybook::{FixedRecord, PicU, PicX};
    use domain::records::account::AccountRecord;
    use domain::records::card_xref::CardXrefRecord;
    use domain::records::customer::CustomerRecord;
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};

    use super::{AccountViewResponse, rules};

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

    /// The domain view of the synthetic account and customer, all three reads found.
    fn domain_view() -> Result<rules::AccountView, Box<dyn StdError>> {
        let account = AccountRecord::decode(account_line().as_bytes())?;
        let customer = CustomerRecord::decode(customer_line().as_bytes())?;
        let xref = CardXrefRecord {
            card_num: CardNumber::parse("4111111111111111")?,
            cust_id: customer.id,
            acct_id: account.id,
            filler: PicX::spaces(),
        };
        let key = PicU::<11>::new(42)?;
        Ok(rules::view(
            key,
            rules::Lookup::Found(&xref),
            rules::Lookup::Found(&account),
            rules::Lookup::Found(&customer),
        )?)
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
        assert_eq!(response.active_status, "Y");
        assert_eq!(response.open_date, Some(date(2014, 11, 20)?));
        assert_eq!(response.expiration_date, Some(date(2025, 5, 20)?));
        assert_eq!(response.reissue_date, None);
        assert_eq!(response.group_id, "GRP01");
        // The domain's edited amounts pass through unchanged, each to its own member.
        let amounts = [
            (&response.current_balance, &edited.current_balance),
            (&response.credit_limit, &edited.credit_limit),
            (&response.cash_credit_limit, &edited.cash_credit_limit),
            (&response.current_cycle_credit, &edited.current_cycle_credit),
            (&response.current_cycle_debit, &edited.current_cycle_debit),
        ];
        for (sent, shown) in amounts {
            assert_eq!(sent, shown);
            assert_eq!(sent.chars().count(), 15, "{sent:?}");
        }
        assert_eq!(response.current_balance, "+      1,234.56");
        assert_eq!(response.current_cycle_debit, "-         25.50");
        Ok(())
    }

    /// The customer block, COACTVWC-016 and COACTVWC-018 to COACTVWC-026.
    #[test]
    fn the_customer_block_maps_field_for_field() -> TestResult {
        let response = AccountViewResponse::from(domain_view()?);
        assert_eq!(response.customer_id, "000000007");
        assert_eq!(response.fico_score, "075");
        assert_eq!(response.date_of_birth, Some(date(1980, 1, 15)?));
        assert_eq!(
            [
                response.first_name.as_str(),
                &response.middle_name,
                &response.last_name,
                &response.address_line_1,
                &response.address_line_2,
                &response.city,
                &response.state,
                &response.country,
                &response.government_id,
                &response.eft_account_id,
                &response.primary_card_holder,
            ],
            [
                "JOHN",
                "Q",
                "PUBLIC",
                "1 MAIN ST",
                "APT 2",
                "SPRINGFIELD",
                "IL",
                "USA",
                "GOVT-ID-0001",
                "0000000042",
                "Y",
            ]
        );
        // The narrower map fields keep the leftmost characters.
        assert_eq!(response.zip, "62701");
        assert_eq!(response.phone_1, "(555)555-0100");
        assert_eq!(response.phone_2, "(555)555-0199");
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

    /// The schema the frontend's generated types read: its name, the redacted SSN example and the
    /// `format: date` of the four dates.
    #[test]
    fn the_schema_types_the_ssn_as_a_redacted_string_and_the_dates_as_dates() -> TestResult {
        assert_eq!(AccountViewResponse::name(), "AccountViewResponse");
        let schema = serde_json::to_value(AccountViewResponse::schema())?;
        let properties = &schema["properties"];
        assert_eq!(properties["ssn"]["type"], json!("string"));
        assert_eq!(properties["ssn"]["example"], json!("***-**-1234"));
        for name in [
            "open_date",
            "expiration_date",
            "reissue_date",
            "date_of_birth",
        ] {
            assert_eq!(properties[name]["format"], json!("date"), "{name}");
        }
        let required = schema["required"].as_array().ok_or("no required list")?;
        assert!(required.contains(&json!("ssn")));
        assert!(!required.contains(&json!("open_date")));
        assert!(properties.get("message").is_none() && properties.get("info_message").is_none());
        Ok(())
    }
}
