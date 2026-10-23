//! COACTUPC, account update (transaction `CAUP`, map `CACTUPA`): the contracts of endpoint 5,
//! `GET /api/v1/accounts/{acct_id}` (ENTER fetch), and endpoint 6, `PUT
//! /api/v1/accounts/{acct_id}` (ENTER validate, F5 save). F3 and F12 send no request.
//!
//! # Wire contract
//!
//! - Endpoint 5 answers [`AccountUpdateView`]: the values `3202-SHOW-ORIGINAL-VALUES` shows and
//!   the [`AccountUpdateSnapshot`] of `ACUP-OLD-DETAILS` (COACTUPC-068, COACTUPC-074 to
//!   COACTUPC-077, COACTUPC-100).
//! - Endpoint 6 takes [`AccountUpdateRequest`]: the key, the raw text of every map field and
//!   the echoed snapshot. It answers [`AccountUpdateResult`]: the info line, the screen state,
//!   which keys apply and the amounts as shown (COACTUPC-078, COACTUPC-079, COACTUPC-082).
//! - Every failure is an `api::error::ScreenError`, whose `field` is the name of the
//!   [`AccountUpdateFields`] member, or `acct_id` for the path parameter. A 404 or 500 of
//!   endpoint 5 is that `ScreenError` as [`AccountUpdateViewError`], which adds the state `S`
//!   and the details the screen shows when the account read did not succeed and the customer
//!   read found the customer (COACTUPC-099), and nothing otherwise.
//!
//! # State
//!
//! The snapshot's `state` is `ACUP-CHANGE-ACTION`, which decides what a key does: F5 saves only
//! when the echoed state is `N` (COACTUPC-071). Endpoint 6 never refreshes the snapshot's
//! values, as the COBOL refreshes `ACUP-OLD-DETAILS` only at the fetch, so a 200 of endpoint 6
//! returns only the state it leaves, [`AccountUpdateResult::state`]. The client writes that code
//! into `snapshot.state` and echoes every other snapshot value unchanged (COACTUPC-100).
//!
//! # Raw input and typed output
//!
//! The request carries the date, phone and SSN parts and the amounts as the raw text of their
//! map fields, at most the map `LENGTH` (`modern/audit/screens.md` COACTUP-016 to COACTUP-120)
//! and fitting the field's `PIC X(n)` byte for byte as printable ASCII (X-COUSR01C-3):
//! COACTUPC's own edits word the messages for them, the raw-edit-input exception of Typed Dates
//! and Timestamps. Every date the API sends, in the view and in the snapshot, is a typed date,
//! `null` for a blank field.
//!
//! # Cardholder data
//!
//! The SSN leaves only as `***-**-` and its last four digits, through the redacted serialisation
//! of [`Ssn`] (COACTUPC-076), and the snapshot holds those four digits only. The SSN parts enter
//! only when re-entered, are never serialised and are printed as `***` by `Debug`; omitting all
//! three means unchanged (COACTUPC-064, E32).

use core::fmt;

use axum::Json;
use axum::response::{IntoResponse, Response};
use chrono::NaiveDate;
use domain::online::coactupc as rules;
use domain::sensitive::Ssn;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::{digits, full_text};
use crate::error::{ScreenError, ServiceError};

/// What `Debug` prints for an SSN part that is present.
const SSN_PART_MASK: &str = "***";

/// The key of an endpoint 6 request.
///
/// `ENTER` runs the change check and the edits; `PF5` saves, and only when the echoed
/// `snapshot.state` is `N`. Any other key text is read as `ENTER`, as `0000-MAIN` processes
/// every other key, `PF12` included, as ENTER (COACTUPC-004); a value that is not a string is a
/// 400.
// `#[serde(other)]` must sit on the last variant, so `Enter` follows `Pf5`. The schema name
// `CoactupcAid` keeps the program's key enum apart from the other programs' (Generated API Types
// Only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = CoactupcAid)]
pub enum Aid {
    /// F5: the save of changes validated in state `N`.
    #[serde(rename = "PF5")]
    Pf5,
    /// ENTER, and every key text other than `PF5`.
    #[serde(rename = "ENTER", other)]
    Enter,
}

impl From<Aid> for rules::Aid {
    fn from(aid: Aid) -> Self {
        match aid {
            Aid::Pf5 => Self::Pf5,
            Aid::Enter => Self::Enter,
        }
    }
}

/// `serde`'s `skip_serializing_if` predicate of the SSN parts: always skip.
///
/// A predicate rather than `skip_serializing`, which would also drop the members from the
/// `OpenAPI` schema, where they must appear as `writeOnly` request input.
const fn withheld<T>(_: &T) -> bool {
    true
}

/// The raw text of every enterable map field of an endpoint 6 request, each fitting its map
/// field's `PIC X(n)` byte for byte: at most its map `LENGTH` in bytes, each printable ASCII
/// (`0x20`–`0x7E`), or the field's `LOW-VALUES` (X-COUSR01C-3).
///
/// An absent member is the empty text, which the program reads as blank. The account number is
/// the path's `acct_id`. The SSN parts are present only when the operator re-entered them:
/// omitting all three means unchanged, a part omitted beside a present one is blank
/// (COACTUPC-064). They are never serialised, so a body carrying them is written as JSON, and
/// `Debug` prints them as `***`.
// The byte rule is the parent module's `pic_x` beside each `length`, whose rejection text is
// `FIELD_LENGTH_MESSAGE`, before COACTUPC's edits run. `length` counts characters, and the
// save writes each value into a `PicX` that holds printable ASCII only, so without `pic_x` a
// text such as `PASSPORTé` or one holding a TAB would pass the edits and fail every F5.
#[derive(Clone, Default, Serialize, Deserialize, ToSchema, Validate)]
#[serde(default)]
pub struct AccountUpdateFields {
    /// `ACSTTUS` (1): the active status.
    #[schema(max_length = 1)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    pub active_status: String,
    /// `OPNYEAR` (4): the open date's year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub open_year: String,
    /// `OPNMON` (2): the open date's month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub open_month: String,
    /// `OPNDAY` (2): the open date's day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub open_day: String,
    /// `ACRDLIM` (15): the credit limit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15), custom(function = "super::pic_x::<15>"))]
    pub credit_limit: String,
    /// `EXPYEAR` (4): the expiry date's year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub expiry_year: String,
    /// `EXPMON` (2): the expiry date's month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub expiry_month: String,
    /// `EXPDAY` (2): the expiry date's day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub expiry_day: String,
    /// `ACSHLIM` (15): the cash credit limit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15), custom(function = "super::pic_x::<15>"))]
    pub cash_credit_limit: String,
    /// `RISYEAR` (4): the reissue date's year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub reissue_year: String,
    /// `RISMON` (2): the reissue date's month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub reissue_month: String,
    /// `RISDAY` (2): the reissue date's day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub reissue_day: String,
    /// `ACURBAL` (15): the current balance.
    #[schema(max_length = 15)]
    #[validate(length(max = 15), custom(function = "super::pic_x::<15>"))]
    pub current_balance: String,
    /// `ACRCYCR` (15): the current cycle credit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15), custom(function = "super::pic_x::<15>"))]
    pub current_cycle_credit: String,
    /// `AADDGRP` (10): the account group.
    #[schema(max_length = 10)]
    #[validate(length(max = 10), custom(function = "super::pic_x::<10>"))]
    pub group_id: String,
    /// `ACRCYDB` (15): the current cycle debit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15), custom(function = "super::pic_x::<15>"))]
    pub current_cycle_debit: String,
    /// `ACSTNUM` (9): the customer ID, read-only once details are shown.
    #[schema(max_length = 9)]
    #[validate(length(max = 9), custom(function = "super::pic_x::<9>"))]
    pub customer_id: String,
    /// `ACTSSN1` (3): the first three SSN digits, only when re-entered.
    #[serde(skip_serializing_if = "withheld")]
    #[schema(write_only, max_length = 3, example = "***")]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub ssn_part1: Option<String>,
    /// `ACTSSN2` (2): the fourth and fifth SSN digits, only when re-entered.
    #[serde(skip_serializing_if = "withheld")]
    #[schema(write_only, max_length = 2, example = "**")]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub ssn_part2: Option<String>,
    /// `ACTSSN3` (4): the last four SSN digits, only when re-entered.
    #[serde(skip_serializing_if = "withheld")]
    #[schema(write_only, max_length = 4, example = "****")]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub ssn_part3: Option<String>,
    /// `DOBYEAR` (4): the date of birth's year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub dob_year: String,
    /// `DOBMON` (2): the date of birth's month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub dob_month: String,
    /// `DOBDAY` (2): the date of birth's day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub dob_day: String,
    /// `ACSTFCO` (3): the FICO score.
    #[schema(max_length = 3)]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub fico_score: String,
    /// `ACSFNAM` (25): the first name.
    #[schema(max_length = 25)]
    #[validate(length(max = 25), custom(function = "super::pic_x::<25>"))]
    pub first_name: String,
    /// `ACSMNAM` (25): the middle name.
    #[schema(max_length = 25)]
    #[validate(length(max = 25), custom(function = "super::pic_x::<25>"))]
    pub middle_name: String,
    /// `ACSLNAM` (25): the last name.
    #[schema(max_length = 25)]
    #[validate(length(max = 25), custom(function = "super::pic_x::<25>"))]
    pub last_name: String,
    /// `ACSADL1` (50): address line 1.
    #[schema(max_length = 50)]
    #[validate(length(max = 50), custom(function = "super::pic_x::<50>"))]
    pub address_line_1: String,
    /// `ACSSTTE` (2): the state code.
    #[schema(max_length = 2)]
    #[validate(length(max = 2), custom(function = "super::pic_x::<2>"))]
    pub state: String,
    /// `ACSADL2` (50): address line 2.
    #[schema(max_length = 50)]
    #[validate(length(max = 50), custom(function = "super::pic_x::<50>"))]
    pub address_line_2: String,
    /// `ACSZIPC` (5): the ZIP code.
    #[schema(max_length = 5)]
    #[validate(length(max = 5), custom(function = "super::pic_x::<5>"))]
    pub zip: String,
    /// `ACSCITY` (50): address line 3, the city.
    #[schema(max_length = 50)]
    #[validate(length(max = 50), custom(function = "super::pic_x::<50>"))]
    pub city: String,
    /// `ACSCTRY` (3): the country code, read-only once details are shown.
    #[schema(max_length = 3)]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub country: String,
    /// `ACSPH1A` (3): phone number 1's area code.
    #[schema(max_length = 3)]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub phone_1_area: String,
    /// `ACSPH1B` (3): phone number 1's prefix.
    #[schema(max_length = 3)]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub phone_1_prefix: String,
    /// `ACSPH1C` (4): phone number 1's line number.
    #[schema(max_length = 4)]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub phone_1_line: String,
    /// `ACSGOVT` (20): the government-issued ID.
    #[schema(max_length = 20)]
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    pub government_id: String,
    /// `ACSPH2A` (3): phone number 2's area code.
    #[schema(max_length = 3)]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub phone_2_area: String,
    /// `ACSPH2B` (3): phone number 2's prefix.
    #[schema(max_length = 3)]
    #[validate(length(max = 3), custom(function = "super::pic_x::<3>"))]
    pub phone_2_prefix: String,
    /// `ACSPH2C` (4): phone number 2's line number.
    #[schema(max_length = 4)]
    #[validate(length(max = 4), custom(function = "super::pic_x::<4>"))]
    pub phone_2_line: String,
    /// `ACSEFTC` (10): the EFT account ID.
    #[schema(max_length = 10)]
    #[validate(length(max = 10), custom(function = "super::pic_x::<10>"))]
    pub eft_account_id: String,
    /// `ACSPFLG` (1): the primary card holder indicator.
    #[schema(max_length = 1)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    pub primary_card_holder: String,
}

impl AccountUpdateFields {
    /// The fields as COACTUPC's input, `acct_id` standing for the read-only `ACCTSID` field:
    /// each member is the raw text the program receives (COACTUPC-007), and the SSN parts are
    /// `None` when all three are omitted (COACTUPC-064).
    #[must_use]
    pub fn input<'a>(&'a self, acct_id: &'a str) -> rules::AccountUpdateInput<'a> {
        rules::AccountUpdateInput {
            account_id: acct_id,
            active_status: &self.active_status,
            open_date: date_parts(&self.open_year, &self.open_month, &self.open_day),
            credit_limit: &self.credit_limit,
            expiry_date: date_parts(&self.expiry_year, &self.expiry_month, &self.expiry_day),
            cash_credit_limit: &self.cash_credit_limit,
            reissue_date: date_parts(&self.reissue_year, &self.reissue_month, &self.reissue_day),
            current_balance: &self.current_balance,
            current_cycle_credit: &self.current_cycle_credit,
            group_id: &self.group_id,
            current_cycle_debit: &self.current_cycle_debit,
            customer_id: &self.customer_id,
            ssn: self.ssn_parts(),
            dob: date_parts(&self.dob_year, &self.dob_month, &self.dob_day),
            fico_score: &self.fico_score,
            first_name: &self.first_name,
            middle_name: &self.middle_name,
            last_name: &self.last_name,
            address_line_1: &self.address_line_1,
            state: &self.state,
            address_line_2: &self.address_line_2,
            zip: &self.zip,
            city: &self.city,
            country: &self.country,
            phone_1: phone_parts(&self.phone_1_area, &self.phone_1_prefix, &self.phone_1_line),
            government_id: &self.government_id,
            phone_2: phone_parts(&self.phone_2_area, &self.phone_2_prefix, &self.phone_2_line),
            eft_account_id: &self.eft_account_id,
            primary_card_holder: &self.primary_card_holder,
        }
    }

    /// The re-entered SSN parts: `None` when all three are omitted, which means unchanged;
    /// otherwise each part, an omitted one as the empty text the edit reads as blank
    /// (COACTUPC-064).
    fn ssn_parts(&self) -> Option<rules::SsnParts<'_>> {
        let entered = [&self.ssn_part1, &self.ssn_part2, &self.ssn_part3];
        if entered.iter().all(|part| part.is_none()) {
            return None;
        }
        let [part1, part2, part3] = entered.map(|part| part.as_deref().unwrap_or_default());
        Some(rules::SsnParts {
            part1,
            part2,
            part3,
        })
    }
}

/// A date's three map fields as COACTUPC's input.
const fn date_parts<'a>(year: &'a str, month: &'a str, day: &'a str) -> rules::DateParts<'a> {
    rules::DateParts { year, month, day }
}

/// A phone number's three map fields as COACTUPC's input.
const fn phone_parts<'a>(area: &'a str, prefix: &'a str, line: &'a str) -> rules::PhoneParts<'a> {
    rules::PhoneParts { area, prefix, line }
}

/// `***` for an SSN part that is present, `None` for an omitted one.
fn masked(part: Option<&String>) -> Option<&'static str> {
    part.map(|_| SSN_PART_MASK)
}

/// Every member, the SSN parts as `***` when present: they are cardholder data.
impl fmt::Debug for AccountUpdateFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountUpdateFields")
            .field("active_status", &self.active_status)
            .field("open_year", &self.open_year)
            .field("open_month", &self.open_month)
            .field("open_day", &self.open_day)
            .field("credit_limit", &self.credit_limit)
            .field("expiry_year", &self.expiry_year)
            .field("expiry_month", &self.expiry_month)
            .field("expiry_day", &self.expiry_day)
            .field("cash_credit_limit", &self.cash_credit_limit)
            .field("reissue_year", &self.reissue_year)
            .field("reissue_month", &self.reissue_month)
            .field("reissue_day", &self.reissue_day)
            .field("current_balance", &self.current_balance)
            .field("current_cycle_credit", &self.current_cycle_credit)
            .field("group_id", &self.group_id)
            .field("current_cycle_debit", &self.current_cycle_debit)
            .field("customer_id", &self.customer_id)
            .field("ssn_part1", &masked(self.ssn_part1.as_ref()))
            .field("ssn_part2", &masked(self.ssn_part2.as_ref()))
            .field("ssn_part3", &masked(self.ssn_part3.as_ref()))
            .field("dob_year", &self.dob_year)
            .field("dob_month", &self.dob_month)
            .field("dob_day", &self.dob_day)
            .field("fico_score", &self.fico_score)
            .field("first_name", &self.first_name)
            .field("middle_name", &self.middle_name)
            .field("last_name", &self.last_name)
            .field("address_line_1", &self.address_line_1)
            .field("state", &self.state)
            .field("address_line_2", &self.address_line_2)
            .field("zip", &self.zip)
            .field("city", &self.city)
            .field("country", &self.country)
            .field("phone_1_area", &self.phone_1_area)
            .field("phone_1_prefix", &self.phone_1_prefix)
            .field("phone_1_line", &self.phone_1_line)
            .field("government_id", &self.government_id)
            .field("phone_2_area", &self.phone_2_area)
            .field("phone_2_prefix", &self.phone_2_prefix)
            .field("phone_2_line", &self.phone_2_line)
            .field("eft_account_id", &self.eft_account_id)
            .field("primary_card_holder", &self.primary_card_holder)
            .finish()
    }
}

/// `ACUP-OLD-DETAILS` and `ACUP-CHANGE-ACTION`: the values endpoint 5 fetched, which the client
/// echoes in every endpoint 6 request (COACTUPC-100).
///
/// Text members hold the stored field in full, trailing spaces included; IDs and the FICO score
/// are their digits; amounts are plain decimal strings such as `-1940.00`; dates are typed,
/// `null` for a blank field. The SSN is only its last four digits (E32). A value the client
/// alters is read as unknown and equals nothing, so it counts as a change and, at a save, as a
/// record changed by another user: the snapshot grants nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct AccountUpdateSnapshot {
    /// `ACUP-CHANGE-ACTION`: a space before a fetch, `S` details shown, `E` changes not OK,
    /// `N` changes validated, `C` saved, `L` lock failed, `F` rewrite failed.
    #[validate(length(max = 1))]
    pub state: String,
    /// `ACUP-OLD-ACCT-ID`, eleven digits.
    #[validate(length(max = 11))]
    pub account_id: String,
    /// `ACUP-OLD-ACTIVE-STATUS`.
    #[validate(length(max = 1))]
    pub active_status: String,
    /// `ACUP-OLD-CURR-BAL`.
    #[validate(length(max = 14))]
    pub current_balance: String,
    /// `ACUP-OLD-CREDIT-LIMIT`.
    #[validate(length(max = 14))]
    pub credit_limit: String,
    /// `ACUP-OLD-CASH-CREDIT-LIMIT`.
    #[validate(length(max = 14))]
    pub cash_credit_limit: String,
    /// `ACUP-OLD-OPEN-DATE`.
    pub open_date: Option<NaiveDate>,
    /// `ACUP-OLD-EXPIRAION-DATE`.
    pub expiry_date: Option<NaiveDate>,
    /// `ACUP-OLD-REISSUE-DATE`.
    pub reissue_date: Option<NaiveDate>,
    /// `ACUP-OLD-CURR-CYC-CREDIT`.
    #[validate(length(max = 14))]
    pub current_cycle_credit: String,
    /// `ACUP-OLD-CURR-CYC-DEBIT`.
    #[validate(length(max = 14))]
    pub current_cycle_debit: String,
    /// `ACUP-OLD-GROUP-ID`.
    #[validate(length(max = 10))]
    pub group_id: String,
    /// `ACUP-OLD-CUST-ID`, nine digits.
    #[validate(length(max = 9))]
    pub customer_id: String,
    /// `ACUP-OLD-CUST-FIRST-NAME`.
    #[validate(length(max = 25))]
    pub first_name: String,
    /// `ACUP-OLD-CUST-MIDDLE-NAME`.
    #[validate(length(max = 25))]
    pub middle_name: String,
    /// `ACUP-OLD-CUST-LAST-NAME`.
    #[validate(length(max = 25))]
    pub last_name: String,
    /// `ACUP-OLD-CUST-ADDR-LINE-1`.
    #[validate(length(max = 50))]
    pub address_line_1: String,
    /// `ACUP-OLD-CUST-ADDR-LINE-2`.
    #[validate(length(max = 50))]
    pub address_line_2: String,
    /// `ACUP-OLD-CUST-ADDR-LINE-3`, the city.
    #[validate(length(max = 50))]
    pub city: String,
    /// `ACUP-OLD-CUST-ADDR-STATE-CD`.
    #[validate(length(max = 2))]
    pub state_code: String,
    /// `ACUP-OLD-CUST-ADDR-COUNTRY-CD`.
    #[validate(length(max = 3))]
    pub country: String,
    /// `ACUP-OLD-CUST-ADDR-ZIP`, all ten characters.
    #[validate(length(max = 10))]
    pub zip: String,
    /// `ACUP-OLD-CUST-PHONE-NUM-1`, all fifteen characters.
    #[validate(length(max = 15))]
    pub phone_1: String,
    /// `ACUP-OLD-CUST-PHONE-NUM-2`, all fifteen characters.
    #[validate(length(max = 15))]
    pub phone_2: String,
    /// The last four digits of `ACUP-OLD-CUST-SSN`, never the full SSN (E32).
    #[schema(example = "1234")]
    #[validate(length(max = 4))]
    pub ssn_last4: String,
    /// `ACUP-OLD-CUST-GOVT-ISSUED-ID`.
    #[validate(length(max = 20))]
    pub government_id: String,
    /// `ACUP-OLD-CUST-DOB-YYYY-MM-DD`.
    pub dob: Option<NaiveDate>,
    /// `ACUP-OLD-CUST-EFT-ACCOUNT-ID`.
    #[validate(length(max = 10))]
    pub eft_account_id: String,
    /// `ACUP-OLD-CUST-PRI-HOLDER-IND`.
    #[validate(length(max = 1))]
    pub primary_card_holder: String,
    /// `ACUP-OLD-CUST-FICO-SCORE`, three digits.
    #[validate(length(max = 3))]
    pub fico_score: String,
}

/// The snapshot as the text the client echoes, from `rules::Snapshot::fields`.
impl From<rules::Snapshot> for AccountUpdateSnapshot {
    fn from(snapshot: rules::Snapshot) -> Self {
        let rules::SnapshotFields {
            state,
            account_id,
            active_status,
            current_balance,
            credit_limit,
            cash_credit_limit,
            open_date,
            expiry_date,
            reissue_date,
            current_cycle_credit,
            current_cycle_debit,
            group_id,
            customer_id,
            first_name,
            middle_name,
            last_name,
            address_line_1,
            address_line_2,
            city,
            state_code,
            country,
            zip,
            phone_1,
            phone_2,
            ssn_last4,
            government_id,
            dob,
            eft_account_id,
            primary_card_holder,
            fico_score,
        } = snapshot.fields();
        Self {
            state,
            account_id,
            active_status,
            current_balance,
            credit_limit,
            cash_credit_limit,
            open_date,
            expiry_date,
            reissue_date,
            current_cycle_credit,
            current_cycle_debit,
            group_id,
            customer_id,
            first_name,
            middle_name,
            last_name,
            address_line_1,
            address_line_2,
            city,
            state_code,
            country,
            zip,
            phone_1,
            phone_2,
            ssn_last4,
            government_id,
            dob,
            eft_account_id,
            primary_card_holder,
            fico_score,
        }
    }
}

/// The echoed snapshot as COACTUPC's `ACUP-OLD-DETAILS`, through the domain's total
/// `rules::Snapshot::from_fields`: any text yields a snapshot.
impl From<&AccountUpdateSnapshot> for rules::Snapshot {
    fn from(snapshot: &AccountUpdateSnapshot) -> Self {
        Self::from_fields(&rules::SnapshotFields {
            state: snapshot.state.as_str(),
            account_id: snapshot.account_id.as_str(),
            active_status: snapshot.active_status.as_str(),
            current_balance: snapshot.current_balance.as_str(),
            credit_limit: snapshot.credit_limit.as_str(),
            cash_credit_limit: snapshot.cash_credit_limit.as_str(),
            open_date: snapshot.open_date,
            expiry_date: snapshot.expiry_date,
            reissue_date: snapshot.reissue_date,
            current_cycle_credit: snapshot.current_cycle_credit.as_str(),
            current_cycle_debit: snapshot.current_cycle_debit.as_str(),
            group_id: snapshot.group_id.as_str(),
            customer_id: snapshot.customer_id.as_str(),
            first_name: snapshot.first_name.as_str(),
            middle_name: snapshot.middle_name.as_str(),
            last_name: snapshot.last_name.as_str(),
            address_line_1: snapshot.address_line_1.as_str(),
            address_line_2: snapshot.address_line_2.as_str(),
            city: snapshot.city.as_str(),
            state_code: snapshot.state_code.as_str(),
            country: snapshot.country.as_str(),
            zip: snapshot.zip.as_str(),
            phone_1: snapshot.phone_1.as_str(),
            phone_2: snapshot.phone_2.as_str(),
            ssn_last4: snapshot.ssn_last4.as_str(),
            government_id: snapshot.government_id.as_str(),
            dob: snapshot.dob,
            eft_account_id: snapshot.eft_account_id.as_str(),
            primary_card_holder: snapshot.primary_card_holder.as_str(),
            fico_score: snapshot.fico_score.as_str(),
        })
    }
}

/// The five amounts as the screen shows them after endpoint 6 validated or saved them: each the
/// 15-character `+ZZZ,ZZZ,ZZZ.99` string of its value (COACTUPC-078, COACTUPC-109).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountUpdateAmounts {
    /// `ACRDLIM`.
    pub credit_limit: String,
    /// `ACSHLIM`.
    pub cash_credit_limit: String,
    /// `ACURBAL`.
    pub current_balance: String,
    /// `ACRCYCR`.
    pub current_cycle_credit: String,
    /// `ACRCYDB`.
    pub current_cycle_debit: String,
}

impl From<rules::ShownAmounts> for AccountUpdateAmounts {
    fn from(amounts: rules::ShownAmounts) -> Self {
        let rules::ShownAmounts {
            current_balance,
            credit_limit,
            cash_credit_limit,
            current_cycle_credit,
            current_cycle_debit,
        } = amounts;
        Self {
            credit_limit,
            cash_credit_limit,
            current_balance,
            current_cycle_credit,
            current_cycle_debit,
        }
    }
}

/// The 200 body of endpoint 5: the account and customer as `CACTUPA` shows them in state `S`,
/// with the snapshot the client echoes (COACTUPC-068, COACTUPC-074 to COACTUPC-077).
///
/// Every member but the dates, the SSN and the snapshot carries the name of the
/// [`AccountUpdateFields`] member the screen sends it back in. Text is every byte of its map
/// field as stored, trailing spaces included; amounts are the 15-character `+ZZZ,ZZZ,ZZZ.99`
/// strings; dates are typed, `null` for a blank one, and the screen splits them into the map's
/// parts; the ZIP is the first five stored characters and each phone number its three parts,
/// characters 2-4, 6-8 and 10-13 of the stored number (COACTUPC-074, COACTUPC-075,
/// COACTUPC-077). The SSN is `***-**-` and its last four digits (COACTUPC-076); no card number
/// is held.
// No error-message member: a shown view has none (COACTUPC-068), so only the info line travels.
// No `Deserialize`: `Ssn` serialises redacted only and cannot be read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountUpdateView {
    /// `ACCTSID`: the account number, eleven digits, read-only once details are shown.
    pub account_id: String,
    /// `ACSTTUS`: the active status.
    pub active_status: String,
    /// `OPNYEAR`, `OPNMON`, `OPNDAY`: the open date.
    pub open_date: Option<NaiveDate>,
    /// `ACRDLIM`: the credit limit.
    pub credit_limit: String,
    /// `EXPYEAR`, `EXPMON`, `EXPDAY`: the expiry date.
    pub expiry_date: Option<NaiveDate>,
    /// `ACSHLIM`: the cash credit limit.
    pub cash_credit_limit: String,
    /// `RISYEAR`, `RISMON`, `RISDAY`: the reissue date.
    pub reissue_date: Option<NaiveDate>,
    /// `ACURBAL`: the current balance.
    pub current_balance: String,
    /// `ACRCYCR`: the current cycle credit.
    pub current_cycle_credit: String,
    /// `AADDGRP`: the account group.
    pub group_id: String,
    /// `ACRCYDB`: the current cycle debit.
    pub current_cycle_debit: String,
    /// `ACSTNUM`: the customer ID, nine digits, read-only once details are shown.
    pub customer_id: String,
    /// `ACTSSN1` to `ACTSSN3`: the SSN, redacted; the screen shows `***`, `**` and the last four
    /// digits.
    #[schema(value_type = String, example = "***-**-1234")]
    pub ssn: Ssn,
    /// `DOBYEAR`, `DOBMON`, `DOBDAY`: the date of birth.
    pub dob: Option<NaiveDate>,
    /// `ACSTFCO`: the FICO score, three digits.
    pub fico_score: String,
    /// `ACSFNAM`: the first name.
    pub first_name: String,
    /// `ACSMNAM`: the middle name.
    pub middle_name: String,
    /// `ACSLNAM`: the last name.
    pub last_name: String,
    /// `ACSADL1`: address line 1.
    pub address_line_1: String,
    /// `ACSSTTE`: the state code.
    pub state: String,
    /// `ACSADL2`: address line 2.
    pub address_line_2: String,
    /// `ACSZIPC`: the first five characters of the stored ZIP code (COACTUPC-077).
    pub zip: String,
    /// `ACSCITY`: address line 3, the city.
    pub city: String,
    /// `ACSCTRY`: the country code, read-only once details are shown.
    pub country: String,
    /// `ACSPH1A`: phone number 1's area code.
    pub phone_1_area: String,
    /// `ACSPH1B`: phone number 1's prefix.
    pub phone_1_prefix: String,
    /// `ACSPH1C`: phone number 1's line number.
    pub phone_1_line: String,
    /// `ACSGOVT`: the government-issued ID.
    pub government_id: String,
    /// `ACSPH2A`: phone number 2's area code.
    pub phone_2_area: String,
    /// `ACSPH2B`: phone number 2's prefix.
    pub phone_2_prefix: String,
    /// `ACSPH2C`: phone number 2's line number.
    pub phone_2_line: String,
    /// `ACSEFTC`: the EFT account ID.
    pub eft_account_id: String,
    /// `ACSPFLG`: the primary card holder indicator.
    pub primary_card_holder: String,
    /// `INFOMSG`: `Update account details presented above.`, the info line of state `S`
    /// (COACTUPC-079).
    pub info_message: String,
    /// The fetched values and state `S`, echoed in every endpoint 6 request.
    pub snapshot: AccountUpdateSnapshot,
}

/// The domain's view as `3202-SHOW-ORIGINAL-VALUES` fills the map: each text member all the
/// bytes of its map field, identifiers with their leading zeros, the shown amounts, the typed
/// dates and the [`Ssn`] unchanged (COACTUPC-074 to COACTUPC-077).
impl From<rules::AccountUpdateView> for AccountUpdateView {
    fn from(view: rules::AccountUpdateView) -> Self {
        let info_message = view.info_message().to_owned();
        let rules::AccountUpdateView {
            account_id,
            active_status,
            amounts,
            open_date,
            expiry_date,
            reissue_date,
            group_id,
            customer_id,
            ssn,
            dob,
            fico_score,
            first_name,
            middle_name,
            last_name,
            address_line_1,
            address_line_2,
            city,
            state,
            zip,
            country,
            phone_1,
            phone_2,
            government_id,
            eft_account_id,
            primary_card_holder,
            snapshot,
        } = view;
        let amounts = AccountUpdateAmounts::from(amounts);
        Self {
            account_id: digits(account_id),
            active_status: full_text(&active_status),
            open_date,
            credit_limit: amounts.credit_limit,
            expiry_date,
            cash_credit_limit: amounts.cash_credit_limit,
            reissue_date,
            current_balance: amounts.current_balance,
            current_cycle_credit: amounts.current_cycle_credit,
            group_id: full_text(&group_id),
            current_cycle_debit: amounts.current_cycle_debit,
            customer_id: digits(customer_id),
            ssn,
            dob,
            fico_score: digits(fico_score),
            first_name: full_text(&first_name),
            middle_name: full_text(&middle_name),
            last_name: full_text(&last_name),
            address_line_1: full_text(&address_line_1),
            state: full_text(&state),
            address_line_2: full_text(&address_line_2),
            zip: full_text(&zip),
            city: full_text(&city),
            country: full_text(&country),
            phone_1_area: full_text(&phone_1.area),
            phone_1_prefix: full_text(&phone_1.prefix),
            phone_1_line: full_text(&phone_1.line),
            government_id: full_text(&government_id),
            phone_2_area: full_text(&phone_2.area),
            phone_2_prefix: full_text(&phone_2.prefix),
            phone_2_line: full_text(&phone_2.line),
            eft_account_id: full_text(&eft_account_id),
            primary_card_holder: full_text(&primary_card_holder),
            info_message,
            snapshot: AccountUpdateSnapshot::from(snapshot),
        }
    }
}

/// The body of an endpoint 5 error: the `ScreenError` of the message and, when the account read
/// did not succeed and the customer read found the customer, the state `S` and the details the
/// screen then shows with its fields enterable (COACTUPC-099).
///
/// Serialised as `{"message": …, "field": …, "state": …, "details": …}`, each of the last three
/// omitted when absent. Every other failure of the fetch leaves the state not fetched and shows
/// no value (COACTUPC-068, COACTUPC-095, COACTUPC-097), so its body is exactly its
/// `ScreenError`. The status is the `ScreenError`'s and travels as the response status only.
// Errors keep the `ScreenError { message, field? }` shape: those two members are the flattened
// `ScreenError`, unchanged, and the state and details are optional members beside them, which
// the `ScreenError` schema, requiring only `message`, leaves open. A bare `ScreenError` would
// drop the customer block and the state `S` the COBOL shows with these messages, which the
// parity row COACTUPC-099 requires. The schema is `allOf` `ScreenError` and the two optional
// members. Endpoint 6 answers its re-read's failures as bare `ScreenError`s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AccountUpdateViewError {
    /// The message and its field, as the `ScreenError` whose status the response carries.
    #[serde(flatten)]
    pub error: ScreenError,
    /// `ACUP-CHANGE-ACTION` after the response: `S`, present only beside `details`
    /// (COACTUPC-099).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "S")]
    pub state: Option<String>,
    /// The values the screen shows in state `S` and the snapshot the client echoes in its next
    /// endpoint 6 request: the customer block as stored (COACTUPC-075, COACTUPC-076) and an
    /// account block, and its part of the snapshot, that no read filled, a placeholder of halt
    /// candidate X-COACTUPC-H1 that is no parity result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<AccountUpdateView>,
}

/// The program's failure: its `ScreenError`, through `api::error`, and the details and state
/// the domain set beside it.
impl From<rules::UpdateFailure> for AccountUpdateViewError {
    fn from(failure: rules::UpdateFailure) -> Self {
        let details = failure.details().cloned().map(AccountUpdateView::from);
        // The state travels only with the details it leaves shown: every other failure of the
        // fetch leaves the screen not fetched (COACTUPC-068).
        let state = details
            .is_some()
            .then(|| failure.state().code().to_string());
        Self {
            error: ScreenError::from(failure),
            state,
            details,
        }
    }
}

/// The service's error: the program's failure with its details, or the `ScreenError` of a store
/// or internal failure with no audited message, which shows nothing. Endpoint 5 runs no
/// password work, so `ServiceError::Saturated` never reaches it; it would show nothing either.
impl From<ServiceError<rules::UpdateFailure>> for AccountUpdateViewError {
    fn from(error: ServiceError<rules::UpdateFailure>) -> Self {
        match error {
            ServiceError::Rule(failure) => Self::from(failure),
            other @ (ServiceError::Store(_) | ServiceError::Internal | ServiceError::Saturated) => {
                Self {
                    error: ScreenError::from(other),
                    state: None,
                    details: None,
                }
            }
        }
    }
}

/// The `ScreenError`'s status, then the body as JSON without it.
impl IntoResponse for AccountUpdateViewError {
    fn into_response(self) -> Response {
        let status = self.error.status();
        (status, Json(self)).into_response()
    }
}

/// The body of endpoint 6: the key, the raw text of the map fields and the echoed snapshot,
/// whose `state` the last response set (COACTUPC-100).
///
/// `Debug` prints the SSN parts as `***`, through [`AccountUpdateFields`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
pub struct AccountUpdateRequest {
    /// `ENTER` validates, `PF5` saves.
    pub aid: Aid,
    /// The raw text of every enterable map field.
    #[serde(deserialize_with = "super::object")]
    #[validate(nested)]
    pub fields: AccountUpdateFields,
    /// The snapshot of the fetch, with the state of the last response.
    #[serde(deserialize_with = "super::object")]
    #[validate(nested)]
    pub snapshot: AccountUpdateSnapshot,
}

/// The 200 body of endpoint 6: the screen `3000-SEND-MAP` sends after a validation, a save or
/// a restart, which has no error message (COACTUPC-014, COACTUPC-108).
///
/// The fields keep the text the request sent, except the amounts, which `amounts` gives as
/// shown. `state` is the code the client writes into `snapshot.state` for its next request.
// `rules::UpdateOutcome` carries the shown amounts and the state, never the account's values,
// so the result holds those and no view: a 200 of endpoint 6 changes no fetched value, and F5
// saves only once the client echoes `state` `N`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[schema(example = json!({
    "message": "Changes validated.Press F5 to save",
    "state": "N",
    "save_enabled": true,
    "cancel_enabled": true,
    "committed": false,
    "amounts": {
        "credit_limit": "+     20,200.00",
        "cash_credit_limit": "+     10,200.00",
        "current_balance": "+        194.00",
        "current_cycle_credit": "+           .00",
        "current_cycle_debit": "+           .00"
    }
}))]
pub struct AccountUpdateResult {
    /// `INFOMSG`: the info line of the state the response leaves, `Changes validated.Press F5 to
    /// save`, `Changes committed to database` or `Enter or update id of account to update`
    /// (COACTUPC-079).
    pub message: String,
    /// `ACUP-CHANGE-ACTION` after the response: `N` after a validation, `C` after a save, a
    /// space after a restart.
    pub state: String,
    /// Whether `F5=Save` is shown: in state `N` only (COACTUPC-082).
    pub save_enabled: bool,
    /// Whether `F12=Cancel` is shown: in states `E`, `N`, `L` and `F` (COACTUPC-082).
    pub cancel_enabled: bool,
    /// Whether both records were rewritten (COACTUPC-108).
    pub committed: bool,
    /// The five amounts as shown after a validation or a save; `null` after a restart.
    pub amounts: Option<AccountUpdateAmounts>,
}

/// The outcome's state decides the info line, the state code and the keys; its amounts are the
/// ones the screen shows. Every text is the domain's.
impl From<rules::UpdateOutcome> for AccountUpdateResult {
    fn from(outcome: rules::UpdateOutcome) -> Self {
        let state = outcome.state();
        let (committed, amounts) = match outcome {
            rules::UpdateOutcome::ChangesValidated { shown } => (false, Some(shown.into())),
            rules::UpdateOutcome::Committed { shown } => (true, Some(shown.into())),
            rules::UpdateOutcome::Restart => (false, None),
        };
        Self {
            message: state.info_message().to_owned(),
            state: state.code().to_string(),
            save_enabled: state.save_key_shown(),
            cancel_enabled: state.cancel_key_shown(),
            committed,
            amounts,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::PathBuf;

    use axum::http::StatusCode;
    use chrono::{Datelike, NaiveDate};
    use domain::FixedClock;
    use domain::copybook::framing::decode_all;
    use domain::copybook::{FixedRecord, PicX};
    use domain::online::coactupc as rules;
    use domain::records::account::AccountRecord;
    use domain::records::card_xref::CardXrefRecord;
    use domain::records::customer::CustomerRecord;
    use serde_json::{Map, Value, json};
    use utoipa::{OpenApi, ToSchema};
    use validator::{Validate, ValidationErrors};

    use super::{
        AccountUpdateAmounts, AccountUpdateFields, AccountUpdateRequest, AccountUpdateResult,
        AccountUpdateSnapshot, AccountUpdateView, Aid,
    };
    use crate::dto::{FIELD_LENGTH_MESSAGE, PIC_X_CODE, digits, validation_error};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The fixture records of `file` in `app/data/ASCII`, read in place.
    fn fixture<R: FixedRecord>(file: &str) -> Result<Vec<R>, Box<dyn StdError>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file);
        let records = decode_all::<R>(&std::fs::read(path)?)?;
        Ok(records.into_iter().map(|framed| framed.record).collect())
    }

    /// The first cross-reference of `cardxref.txt`, its account and its customer.
    fn records() -> Result<(CardXrefRecord, AccountRecord, CustomerRecord), Box<dyn StdError>> {
        let xref = fixture::<CardXrefRecord>("cardxref.txt")?
            .into_iter()
            .next()
            .ok_or("no cross-reference")?;
        let account = fixture::<AccountRecord>("acctdata.txt")?
            .into_iter()
            .find(|account| account.id == xref.acct_id)
            .ok_or("no account")?;
        let customer = fixture::<CustomerRecord>("custdata.txt")?
            .into_iter()
            .find(|customer| customer.id == xref.cust_id)
            .ok_or("no customer")?;
        Ok((xref, account, customer))
    }

    /// Endpoint 5's domain answer for the first cross-reference's account, and its customer.
    fn fetched() -> Result<(rules::AccountUpdateView, CustomerRecord), Box<dyn StdError>> {
        let (xref, account, customer) = records()?;
        let view = rules::fetch(
            &digits(account.id),
            rules::Lookup::Found(&xref),
            rules::Lookup::Found(&account),
            rules::Lookup::Found(&customer),
        )?;
        Ok((view, customer))
    }

    /// A date as the screen's year, month and day parts, empty for a blank date.
    fn date_parts(date: Option<NaiveDate>) -> [String; 3] {
        date.map_or_else(Default::default, |day| {
            [
                format!("{:04}", day.year()),
                format!("{:02}", day.month()),
                format!("{:02}", day.day()),
            ]
        })
    }

    /// The fields a screen sends back unchanged after showing `view`, the SSN parts omitted.
    fn echoed(view: &AccountUpdateView) -> AccountUpdateFields {
        let [open_year, open_month, open_day] = date_parts(view.open_date);
        let [expiry_year, expiry_month, expiry_day] = date_parts(view.expiry_date);
        let [reissue_year, reissue_month, reissue_day] = date_parts(view.reissue_date);
        let [dob_year, dob_month, dob_day] = date_parts(view.dob);
        let view = view.clone();
        AccountUpdateFields {
            active_status: view.active_status,
            open_year,
            open_month,
            open_day,
            credit_limit: view.credit_limit,
            expiry_year,
            expiry_month,
            expiry_day,
            cash_credit_limit: view.cash_credit_limit,
            reissue_year,
            reissue_month,
            reissue_day,
            current_balance: view.current_balance,
            current_cycle_credit: view.current_cycle_credit,
            group_id: view.group_id,
            current_cycle_debit: view.current_cycle_debit,
            customer_id: view.customer_id,
            dob_year,
            dob_month,
            dob_day,
            fico_score: view.fico_score,
            first_name: view.first_name,
            middle_name: view.middle_name,
            last_name: view.last_name,
            address_line_1: view.address_line_1,
            state: view.state,
            address_line_2: view.address_line_2,
            zip: view.zip,
            city: view.city,
            country: view.country,
            phone_1_area: view.phone_1_area,
            phone_1_prefix: view.phone_1_prefix,
            phone_1_line: view.phone_1_line,
            government_id: view.government_id,
            phone_2_area: view.phone_2_area,
            phone_2_prefix: view.phone_2_prefix,
            phone_2_line: view.phone_2_line,
            eft_account_id: view.eft_account_id,
            primary_card_holder: view.primary_card_holder,
            ..AccountUpdateFields::default()
        }
    }

    /// The nine SSN digits of `customer` and their dashed form, which no body may hold.
    fn ssn_forms(customer: &CustomerRecord) -> Result<[String; 2], Box<dyn StdError>> {
        let plain = digits(*customer.ssn.expose());
        let dashed = format!(
            "{}-{}-{}",
            plain.get(..3).ok_or("short SSN")?,
            plain.get(3..5).ok_or("short SSN")?,
            plain.get(5..).ok_or("short SSN")?
        );
        Ok([plain, dashed])
    }

    #[test]
    fn aid_travels_as_enter_and_pf5_and_reads_every_other_key_as_enter() -> TestResult {
        for (aid, wire) in [(Aid::Enter, "\"ENTER\""), (Aid::Pf5, "\"PF5\"")] {
            assert_eq!(serde_json::to_string(&aid)?, wire);
            assert_eq!(serde_json::from_str::<Aid>(wire)?, aid);
        }
        // COACTUPC-004: every other key is processed as ENTER.
        for other in ["\"PF12\"", "\"PF3\"", "\"enter\"", "\"\""] {
            assert_eq!(serde_json::from_str::<Aid>(other)?, Aid::Enter, "{other}");
        }
        assert!(serde_json::from_str::<Aid>("5").is_err());
        assert_eq!(rules::Aid::from(Aid::Enter), rules::Aid::Enter);
        assert_eq!(rules::Aid::from(Aid::Pf5), rules::Aid::Pf5);
        Ok(())
    }

    #[test]
    fn the_view_shows_the_fetched_values_and_never_the_full_ssn() -> TestResult {
        let (view, customer) = fetched()?;
        let view = AccountUpdateView::from(view);
        assert_eq!(view.account_id, "00000000050");
        assert_eq!(view.info_message, "Update account details presented above.");
        assert_eq!(view.snapshot.state, "S");
        assert_eq!(view.current_balance.chars().count(), 15);
        let body = serde_json::to_value(&view)?;
        // COACTUPC-076: the SSN leaves redacted only.
        assert_eq!(body["ssn"], json!(customer.ssn.redacted()));
        assert_eq!(
            view.snapshot.ssn_last4.as_bytes(),
            customer.ssn.last4().as_slice()
        );
        let text = body.to_string();
        for form in ssn_forms(&customer)? {
            assert!(!text.contains(&form), "the view carries the full SSN");
        }
        let open = view.open_date.ok_or("blank open date")?;
        assert_eq!(
            body["open_date"],
            json!(open.format("%Y-%m-%d").to_string())
        );
        assert_eq!(body["snapshot"]["open_date"], body["open_date"]);
        Ok(())
    }

    // audit: COACTUPC-074
    // audit: COACTUPC-075
    // audit: COACTUPC-077
    #[test]
    fn the_view_carries_every_byte_of_each_map_field() -> TestResult {
        let (xref, mut account, mut customer) = records()?;
        account.active_status = PicX::spaces();
        account.group_id = PicX::from_text("GRP01")?;
        customer.first_name = PicX::from_text("JOHN")?;
        customer.middle_name = PicX::spaces();
        customer.addr_zip = PicX::from_text("123")?;
        customer.phone_num_1 = PicX::from_text("(201)   -0100")?;
        let view = AccountUpdateView::from(rules::fetch(
            &digits(account.id),
            rules::Lookup::Found(&xref),
            rules::Lookup::Found(&account),
            rules::Lookup::Found(&customer),
        )?);
        // A space-only field stays its spaces; a padded one keeps its trailing spaces.
        assert_eq!(view.active_status, " ");
        assert_eq!(view.group_id, "GRP01     ");
        assert_eq!(view.first_name, format!("{:<25}", "JOHN"));
        assert_eq!(view.middle_name, " ".repeat(25));
        // COACTUPC-077: the first five ZIP characters and the phone digits at positions 2-4,
        // 6-8 and 10-13, whatever sits there.
        assert_eq!(view.zip, "123  ");
        assert_eq!(
            [
                view.phone_1_area.as_str(),
                &view.phone_1_prefix,
                &view.phone_1_line
            ],
            ["201", "   ", "0100"]
        );
        // Every text member is exactly as wide as its map field.
        for (member, width) in [
            (&view.active_status, 1),
            (&view.group_id, 10),
            (&view.first_name, 25),
            (&view.middle_name, 25),
            (&view.last_name, 25),
            (&view.address_line_1, 50),
            (&view.state, 2),
            (&view.address_line_2, 50),
            (&view.zip, 5),
            (&view.city, 50),
            (&view.country, 3),
            (&view.phone_1_area, 3),
            (&view.phone_1_prefix, 3),
            (&view.phone_1_line, 4),
            (&view.government_id, 20),
            (&view.phone_2_area, 3),
            (&view.phone_2_prefix, 3),
            (&view.phone_2_line, 4),
            (&view.eft_account_id, 10),
            (&view.primary_card_holder, 1),
        ] {
            assert_eq!(member.len(), width, "{member:?}");
        }
        let body = serde_json::to_value(&view)?;
        assert_eq!(body["first_name"], json!(view.first_name));
        assert_eq!(body["phone_1_prefix"], json!("   "));
        Ok(())
    }

    #[test]
    fn the_snapshot_echo_rebuilds_the_fetched_snapshot() -> TestResult {
        let (view, _) = fetched()?;
        let echoed = AccountUpdateSnapshot::from(view.snapshot.clone());
        assert!(echoed.validate().is_ok());
        let wire: AccountUpdateSnapshot = serde_json::from_value(serde_json::to_value(&echoed)?)?;
        assert_eq!(wire, echoed);
        // COACTUPC-100: the echo rebuilds `ACUP-OLD-DETAILS` and its state.
        assert_eq!(rules::Snapshot::from(&wire), view.snapshot);
        let mut altered = wire;
        altered.current_balance = "not an amount".to_owned();
        assert_ne!(rules::Snapshot::from(&altered), view.snapshot);
        Ok(())
    }

    #[test]
    fn fields_become_the_programs_input_with_omitted_ssn_parts_unchanged() {
        let fields = AccountUpdateFields {
            active_status: "Y".to_owned(),
            open_year: "2014".to_owned(),
            open_month: "11".to_owned(),
            open_day: "20".to_owned(),
            credit_limit: "1,000.00".to_owned(),
            phone_2_line: "0123".to_owned(),
            primary_card_holder: "N".to_owned(),
            ..AccountUpdateFields::default()
        };
        let input = fields.input("00000000050");
        assert_eq!(input.account_id, "00000000050");
        assert_eq!(input.active_status, "Y");
        assert_eq!(
            input.open_date,
            rules::DateParts {
                year: "2014",
                month: "11",
                day: "20"
            }
        );
        assert_eq!(input.credit_limit, "1,000.00");
        assert_eq!(input.phone_2.line, "0123");
        assert_eq!(input.primary_card_holder, "N");
        assert_eq!(input.city, "");
        // COACTUPC-064: all three parts omitted means unchanged.
        assert!(input.ssn.is_none());

        let partial = AccountUpdateFields {
            ssn_part3: Some("6789".to_owned()),
            ..AccountUpdateFields::default()
        };
        assert_eq!(
            partial.input("").ssn,
            Some(rules::SsnParts {
                part1: "",
                part2: "",
                part3: "6789"
            })
        );
    }

    #[test]
    fn ssn_parts_are_read_but_never_serialised_or_printed() -> TestResult {
        let fields: AccountUpdateFields = serde_json::from_value(json!({
            "active_status": "Y",
            "ssn_part1": "123",
            "ssn_part2": "45",
            "ssn_part3": "6789"
        }))?;
        assert_eq!(fields.ssn_part3.as_deref(), Some("6789"));
        assert_eq!(fields.zip, "");
        let wire = serde_json::to_value(&fields)?;
        for part in ["ssn_part1", "ssn_part2", "ssn_part3"] {
            assert!(wire.get(part).is_none(), "{part} serialised");
        }
        assert_eq!(wire["active_status"], json!("Y"));
        let printed = format!("{fields:?}");
        assert!(printed.contains("ssn_part1: Some(\"***\")"));
        assert!(!printed.contains("123") && !printed.contains("6789"));

        let omitted: AccountUpdateFields = serde_json::from_value(json!({"ssn_part1": null}))?;
        assert!(omitted.input("").ssn.is_none());
        assert!(format!("{omitted:?}").contains("ssn_part1: None"));
        Ok(())
    }

    #[test]
    fn an_over_long_member_is_a_400_naming_its_path() -> TestResult {
        let (view, _) = fetched()?;
        let view = AccountUpdateView::from(view);
        let valid = AccountUpdateRequest {
            aid: Aid::Enter,
            fields: echoed(&view),
            snapshot: view.snapshot.clone(),
        };
        assert!(valid.validate().is_ok());

        let mut zip = valid.clone();
        zip.fields.zip = "123456".to_owned();
        let mut ssn = valid.clone();
        ssn.fields.ssn_part1 = Some("1234".to_owned());
        let mut last4 = valid;
        last4.snapshot.ssn_last4 = "12345".to_owned();
        for (request, field) in [
            (zip, "fields.zip"),
            (ssn, "fields.ssn_part1"),
            (last4, "snapshot.ssn_last4"),
        ] {
            let errors = request
                .validate()
                .err()
                .ok_or("over-long member accepted")?;
            let error = validation_error(&errors);
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(field))
            );
            assert!(!error.message().contains("1234"));
        }
        Ok(())
    }

    /// Every member of [`AccountUpdateFields`] and its map `LENGTH`, the `maxLength` of its
    /// schema property.
    fn member_widths() -> Result<Vec<(String, usize)>, Box<dyn StdError>> {
        let document = serde_json::to_value(Contract::openapi())?;
        let properties = document["components"]["schemas"]["AccountUpdateFields"]["properties"]
            .as_object()
            .ok_or("the schema has no AccountUpdateFields properties")?;
        properties
            .iter()
            .map(|(member, property)| {
                let width = property["maxLength"]
                    .as_u64()
                    .ok_or("a member has no maxLength")?;
                Ok((member.clone(), usize::try_from(width)?))
            })
            .collect()
    }

    /// The fields of a body that holds only `members`, each with its text.
    fn fields_of<'a>(
        members: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<AccountUpdateFields, Box<dyn StdError>> {
        let body: Map<String, Value> = members
            .into_iter()
            .map(|(member, text)| (member.to_owned(), Value::from(text)))
            .collect();
        Ok(serde_json::from_value(Value::Object(body))?)
    }

    /// The codes of the rules `member` fails in `errors`, empty when it fails none.
    fn codes<'a>(errors: &'a ValidationErrors, member: &str) -> Vec<&'a str> {
        errors
            .field_errors()
            .get(member)
            .copied()
            .map(|list| list.iter().map(|error| error.code.as_ref()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn printable_ascii_at_full_width_low_values_and_omitted_ssn_parts_pass() -> TestResult {
        let widths = member_widths()?;
        assert_eq!(widths.len(), 42, "every enterable map field is a member");
        let printable: Vec<(&str, String)> = widths
            .iter()
            .map(|(member, width)| {
                let text: String = (b' '..=b'~').cycle().take(*width).map(char::from).collect();
                (member.as_str(), text)
            })
            .collect();
        let low_values: Vec<(&str, String)> = widths
            .iter()
            .map(|(member, width)| (member.as_str(), "\0".repeat(*width)))
            .collect();
        for members in [&printable, &low_values] {
            let fields = fields_of(
                members
                    .iter()
                    .map(|(member, text)| (*member, text.as_str())),
            )?;
            assert!(fields.ssn_part1.is_some() && fields.ssn_part3.is_some());
            assert!(fields.validate().is_ok(), "{fields:?}");
        }
        // COACTUPC-064: SSN parts omitted, or `null`, mean unchanged and pass.
        let without_ssn = fields_of(
            printable
                .iter()
                .filter(|(member, _)| !member.starts_with("ssn_part"))
                .map(|(member, text)| (*member, text.as_str())),
        )?;
        let nulls: AccountUpdateFields = serde_json::from_value(json!({
            "ssn_part1": null,
            "ssn_part2": null,
            "ssn_part3": null
        }))?;
        for fields in [without_ssn, nulls, AccountUpdateFields::default()] {
            assert!(fields.input("").ssn.is_none());
            assert!(fields.validate().is_ok(), "{fields:?}");
        }
        Ok(())
    }

    #[test]
    fn a_character_outside_printable_ascii_fails_pic_x_on_its_member() -> TestResult {
        let (view, _) = fetched()?;
        let snapshot = AccountUpdateView::from(view).snapshot;
        let request = |fields: AccountUpdateFields| AccountUpdateRequest {
            aid: Aid::Enter,
            fields,
            snapshot: snapshot.clone(),
        };
        let accents = "é".repeat(25);
        // Within `length` by characters, beyond the field by bytes or by character class.
        let inputs = [
            ("government_id", "PASSPORTé"),
            ("address_line_2", "Café 9"),
            ("group_id", "GRPé"),
            ("address_line_1", "Emoji😀"),
            ("city", "SPRING\tFIELD"),
            ("ssn_part3", "12\t4"),
            ("middle_name", "\0"),
            ("first_name", accents.as_str()),
        ];
        let widths = member_widths()?;
        let every_member = widths.iter().flat_map(|(member, _)| {
            ["é", "😀", "\t", "\n", "\u{7f}"].map(|text| (member.as_str(), text))
        });
        for (member, text) in inputs.into_iter().chain(every_member) {
            let errors = fields_of([(member, text)])?
                .validate()
                .err()
                .ok_or("a text outside printable ASCII was accepted")?;
            assert_eq!(codes(&errors, member), [PIC_X_CODE], "{member} {text:?}");
            assert_eq!(errors.field_errors().len(), 1, "{member} {text:?}");

            // The 400 names the member as its over-long rejection does, and never the text.
            let width = widths
                .iter()
                .find(|(name, _)| name == member)
                .map(|&(_, width)| width)
                .ok_or("no such member")?;
            let over_long = "X".repeat(width + 1);
            let too_long = request(fields_of([(member, over_long.as_str())])?)
                .validate()
                .err()
                .ok_or("an over-long member was accepted")?;
            let unstorable = request(fields_of([(member, text)])?)
                .validate()
                .err()
                .ok_or("a text outside printable ASCII was accepted")?;
            let (length, error) = (validation_error(&too_long), validation_error(&unstorable));
            assert!(length.field().is_some_and(|field| field.ends_with(member)));
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (
                    StatusCode::BAD_REQUEST,
                    FIELD_LENGTH_MESSAGE,
                    length.field()
                ),
                "{member} {text:?}"
            );
            let wire = serde_json::to_string(&error)?;
            assert!(!wire.contains(text) && !format!("{error:?}").contains(text));
        }
        Ok(())
    }

    #[test]
    fn a_result_carries_the_state_its_keys_and_the_amounts_shown() {
        let shown = rules::ShownAmounts {
            current_balance: "a".to_owned(),
            credit_limit: "b".to_owned(),
            cash_credit_limit: "c".to_owned(),
            current_cycle_credit: "d".to_owned(),
            current_cycle_debit: "e".to_owned(),
        };
        let amounts = AccountUpdateAmounts {
            credit_limit: "b".to_owned(),
            cash_credit_limit: "c".to_owned(),
            current_balance: "a".to_owned(),
            current_cycle_credit: "d".to_owned(),
            current_cycle_debit: "e".to_owned(),
        };
        // COACTUPC-014, COACTUPC-079, COACTUPC-082: validated, state `N`, F5 and F12 shown.
        let validated = AccountUpdateResult::from(rules::UpdateOutcome::ChangesValidated {
            shown: shown.clone(),
        });
        assert_eq!(
            validated,
            AccountUpdateResult {
                message: "Changes validated.Press F5 to save".to_owned(),
                state: "N".to_owned(),
                save_enabled: true,
                cancel_enabled: true,
                committed: false,
                amounts: Some(amounts.clone()),
            }
        );
        // COACTUPC-108: committed, state `C`, neither key shown.
        let committed = AccountUpdateResult::from(rules::UpdateOutcome::Committed { shown });
        assert_eq!(
            committed,
            AccountUpdateResult {
                message: "Changes committed to database".to_owned(),
                state: "C".to_owned(),
                save_enabled: false,
                cancel_enabled: false,
                committed: true,
                amounts: Some(amounts),
            }
        );
        // COACTUPC-006: the empty screen of a new search.
        let restart = AccountUpdateResult::from(rules::UpdateOutcome::Restart);
        assert_eq!(
            (restart.message.as_str(), restart.state.as_str()),
            ("Enter or update id of account to update", " ")
        );
        assert!(!restart.save_enabled && !restart.cancel_enabled && !restart.committed);
        assert!(restart.amounts.is_none());
    }

    /// What `rules::decide` makes of `request` for the account `view` shows.
    fn decide(
        request: &AccountUpdateRequest,
        customer: &CustomerRecord,
        view: &AccountUpdateView,
    ) -> Result<rules::Decision, Box<dyn StdError>> {
        let day = NaiveDate::from_ymd_opt(2022, 7, 18).ok_or("invalid date")?;
        let clock = FixedClock::new(day.and_hms_opt(10, 30, 0).ok_or("invalid time")?);
        Ok(rules::decide(
            request.aid.into(),
            &request.fields.input(&view.account_id),
            &rules::Snapshot::from(&request.snapshot),
            &rules::Fetched::new(customer),
            &clock,
        )?)
    }

    #[test]
    fn f5_saves_once_the_returned_state_is_echoed() -> TestResult {
        let (view, customer) = fetched()?;
        let view = AccountUpdateView::from(view);
        // Values every edit of COACTUPC-015 to COACTUPC-038 and COACTUPC-067 accepts, where the
        // fixture's own fail one: a re-entered SSN (COACTUPC-064), a general-purpose area code
        // and a state and ZIP that agree.
        let fields = AccountUpdateFields {
            ssn_part1: Some("123".to_owned()),
            ssn_part2: Some("45".to_owned()),
            ssn_part3: Some("6789".to_owned()),
            phone_1_area: "201".to_owned(),
            phone_2_area: "201".to_owned(),
            state: "NJ".to_owned(),
            zip: "70001".to_owned(),
            ..echoed(&view)
        };
        let mut request = AccountUpdateRequest {
            aid: Aid::Enter,
            fields,
            snapshot: view.snapshot.clone(),
        };
        let rules::Decision::Respond(outcome) = decide(&request, &customer, &view)? else {
            return Err("ENTER asked for a save".into());
        };
        let result = AccountUpdateResult::from(outcome);
        assert_eq!((result.state.as_str(), result.save_enabled), ("N", true));

        // COACTUPC-004: F5 with the fetch's state `S` is processed as ENTER.
        request.aid = Aid::Pf5;
        assert!(matches!(
            decide(&request, &customer, &view)?,
            rules::Decision::Respond(rules::UpdateOutcome::ChangesValidated { .. })
        ));
        // COACTUPC-071: F5 with the returned state `N` asks for the save.
        request.snapshot.state.clone_from(&result.state);
        assert!(matches!(
            decide(&request, &customer, &view)?,
            rules::Decision::Save(_)
        ));
        Ok(())
    }

    #[derive(OpenApi)]
    #[openapi(components(schemas(
        Aid,
        AccountUpdateFields,
        AccountUpdateSnapshot,
        AccountUpdateAmounts,
        AccountUpdateView,
        AccountUpdateRequest,
        AccountUpdateResult
    )))]
    struct Contract;

    #[test]
    fn schemas_keep_their_names_and_take_the_ssn_parts_write_only() -> TestResult {
        assert_eq!(Aid::name(), "CoactupcAid");
        assert_eq!(AccountUpdateFields::name(), "AccountUpdateFields");
        assert_eq!(AccountUpdateSnapshot::name(), "AccountUpdateSnapshot");
        assert_eq!(AccountUpdateView::name(), "AccountUpdateView");
        assert_eq!(AccountUpdateRequest::name(), "AccountUpdateRequest");
        assert_eq!(AccountUpdateResult::name(), "AccountUpdateResult");

        let document = serde_json::to_value(Contract::openapi())?;
        let schemas = &document["components"]["schemas"];
        assert_eq!(schemas["CoactupcAid"]["enum"], json!(["PF5", "ENTER"]));
        let fields = &schemas["AccountUpdateFields"];
        for (part, example) in [
            ("ssn_part1", "***"),
            ("ssn_part2", "**"),
            ("ssn_part3", "****"),
        ] {
            assert_eq!(fields["properties"][part]["writeOnly"], Value::Bool(true));
            assert_eq!(fields["properties"][part]["example"], json!(example));
        }
        assert!(
            fields.get("required").is_none(),
            "every field defaults to blank"
        );
        assert_eq!(
            schemas["AccountUpdateRequest"]["properties"]["aid"]["$ref"],
            json!("#/components/schemas/CoactupcAid")
        );
        let view = &schemas["AccountUpdateView"]["properties"];
        assert_eq!(view["ssn"]["example"], json!("***-**-1234"));
        assert_eq!(view["open_date"]["format"], json!("date"));
        assert_eq!(
            schemas["AccountUpdateSnapshot"]["properties"]["dob"]["format"],
            json!("date")
        );
        Ok(())
    }

    /// How a master read of the fetch chain ends, without its record.
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
                Self::Failed => rules::Lookup::Failed,
            }
        }
    }

    /// Endpoint 5's error body for the first cross-reference when its account and customer
    /// reads end as `account` and `customer`.
    fn fetch_error(
        account: Read,
        customer: Read,
    ) -> Result<super::AccountUpdateViewError, Box<dyn StdError>> {
        let (xref, stored_account, stored_customer) = records()?;
        let failure = rules::fetch(
            &digits(stored_account.id),
            rules::Lookup::Found(&xref),
            account.of(&stored_account),
            customer.of(&stored_customer),
        )
        .err()
        .ok_or_else(|| format!("{account:?} account and {customer:?} customer gave a view"))?;
        Ok(super::AccountUpdateViewError::from(failure))
    }

    /// The error body of every outcome of the account and customer reads after a found
    /// cross-reference: the state `S` and the details only when the account read did not succeed
    /// and the customer was found (COACTUPC-091, COACTUPC-093, COACTUPC-095, COACTUPC-097,
    /// COACTUPC-099). The details' customer values are the fetched customer's; their account
    /// values, halt candidate X-COACTUPC-H1's placeholder, are not asserted.
    #[test]
    fn a_fetch_error_carries_the_state_and_details_only_beside_a_found_customer() -> TestResult {
        // (account read, customer read, status, field, state and details)
        let cases = [
            (Read::NotFound, Read::Found, 404, Some("acct_id"), true),
            (Read::Failed, Read::Found, 500, Some("acct_id"), true),
            (Read::Found, Read::NotFound, 404, None, false),
            (Read::Found, Read::Failed, 500, None, false),
            (Read::NotFound, Read::NotFound, 404, Some("acct_id"), false),
            (Read::NotFound, Read::Failed, 500, Some("acct_id"), false),
            (Read::Failed, Read::NotFound, 500, Some("acct_id"), false),
        ];
        let (fetched_view, _) = fetched()?;
        let shown = AccountUpdateView::from(fetched_view);
        for (account, customer, status, field, with_details) in cases {
            let label = format!("{account:?} account, {customer:?} customer");
            let body = fetch_error(account, customer)?;
            assert_eq!(body.error.status().as_u16(), status, "{label}");
            assert_eq!(body.error.field(), field, "{label}");
            assert_eq!(body.state.is_some(), with_details, "{label}");
            assert_eq!(body.details.is_some(), with_details, "{label}");
            let Some(details) = body.details else {
                continue;
            };
            assert_eq!(body.state.as_deref(), Some("S"), "{label}");
            assert_eq!(
                details.info_message,
                "Update account details presented above."
            );
            assert_eq!(details.snapshot.state, "S", "{label}");
            assert_eq!(details.account_id, shown.account_id, "{label}");
            assert_eq!(
                [
                    &details.customer_id,
                    &details.fico_score,
                    &details.first_name,
                    &details.middle_name,
                    &details.last_name,
                    &details.address_line_1,
                    &details.address_line_2,
                    &details.city,
                    &details.state,
                    &details.zip,
                    &details.country,
                    &details.phone_1_area,
                    &details.phone_1_prefix,
                    &details.phone_1_line,
                    &details.phone_2_area,
                    &details.phone_2_prefix,
                    &details.phone_2_line,
                    &details.government_id,
                    &details.eft_account_id,
                    &details.primary_card_holder,
                ],
                [
                    &shown.customer_id,
                    &shown.fico_score,
                    &shown.first_name,
                    &shown.middle_name,
                    &shown.last_name,
                    &shown.address_line_1,
                    &shown.address_line_2,
                    &shown.city,
                    &shown.state,
                    &shown.zip,
                    &shown.country,
                    &shown.phone_1_area,
                    &shown.phone_1_prefix,
                    &shown.phone_1_line,
                    &shown.phone_2_area,
                    &shown.phone_2_prefix,
                    &shown.phone_2_line,
                    &shown.government_id,
                    &shown.eft_account_id,
                    &shown.primary_card_holder,
                ],
                "{label}"
            );
            assert_eq!(details.ssn, shown.ssn, "{label}");
            assert_eq!(details.dob, shown.dob, "{label}");
            assert_eq!(
                details.snapshot.ssn_last4, shown.snapshot.ssn_last4,
                "{label}"
            );
        }
        Ok(())
    }

    /// The wire form of a fetch error with details: the `ScreenError` members at the top level,
    /// the state and the details beside them, the SSN redacted in the details and only its last
    /// four digits in their snapshot (COACTUPC-076, COACTUPC-100); a bare error is exactly its
    /// `ScreenError`.
    #[test]
    fn a_fetch_error_body_is_its_screen_error_with_the_state_and_details_beside_it() -> TestResult {
        use axum::response::IntoResponse as _;

        let (_, _, customer) = records()?;
        let full_ssn = digits(*customer.ssn.expose());
        let shown = fetch_error(Read::NotFound, Read::Found)?;
        let body = serde_json::to_value(&shown)?;
        let mut keys: Vec<&str> = body
            .as_object()
            .ok_or("the body is not an object")?
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["details", "field", "message", "state"]);
        assert_eq!(body["state"], json!("S"));
        assert_eq!(body["details"]["snapshot"]["state"], json!("S"));
        let ssn = body["details"]["ssn"].as_str().ok_or("no SSN")?;
        assert!(ssn.starts_with("***-**-") && ssn.len() == 11, "{ssn}");
        assert!(body["details"]["snapshot"].get("ssn").is_none());
        assert!(!body.to_string().contains(&full_ssn), "the full SSN left");

        let bare = fetch_error(Read::Found, Read::NotFound)?;
        assert_eq!(
            serde_json::to_value(&bare)?,
            serde_json::to_value(&bare.error)?
        );
        assert_eq!(shown.into_response().status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    /// An edit failure and a cross-reference failure leave the state not fetched with nothing
    /// shown (COACTUPC-068, COACTUPC-085); a store or internal failure with no audited message
    /// is the API's 500 with nothing beside it.
    #[test]
    fn a_fetch_error_before_the_master_reads_shows_nothing() -> TestResult {
        use persistence::PersistenceError;

        use crate::error::{ServiceError, UNABLE_TO_PROCESS};

        let (_, account, customer) = records()?;
        let edit = rules::fetch(
            "",
            rules::Lookup::NotReached,
            rules::Lookup::NotReached,
            rules::Lookup::NotReached,
        )
        .err()
        .ok_or("a blank account number passed the edit")?;
        let no_xref = rules::fetch(
            &digits(account.id),
            rules::Lookup::NotFound,
            rules::Lookup::Found(&account),
            rules::Lookup::Found(&customer),
        )
        .err()
        .ok_or("a missing cross-reference gave a view")?;
        let bodies = [
            super::AccountUpdateViewError::from(ServiceError::Rule(edit)),
            super::AccountUpdateViewError::from(ServiceError::Rule(no_xref)),
            super::AccountUpdateViewError::from(ServiceError::Store(PersistenceError::Unavailable)),
            super::AccountUpdateViewError::from(ServiceError::<rules::UpdateFailure>::Internal),
        ];
        let statuses: Vec<u16> = bodies
            .iter()
            .map(|body| body.error.status().as_u16())
            .collect();
        assert_eq!(statuses, [400, 404, 500, 500]);
        for body in &bodies {
            assert!(body.state.is_none() && body.details.is_none(), "{body:?}");
        }
        for body in &bodies[2..] {
            assert_eq!(body.error.message(), UNABLE_TO_PROCESS);
            assert_eq!(body.error.field(), None);
        }
        Ok(())
    }

    /// The fetch error schema: `allOf` `ScreenError` and the optional state and details.
    #[test]
    fn the_fetch_error_schema_is_the_screen_error_with_optional_state_and_details() -> TestResult {
        use utoipa::PartialSchema as _;

        assert_eq!(
            super::AccountUpdateViewError::name(),
            "AccountUpdateViewError"
        );
        let schema = serde_json::to_value(super::AccountUpdateViewError::schema())?;
        let parts = schema["allOf"].as_array().ok_or("the error is not allOf")?;
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["$ref"], json!("#/components/schemas/ScreenError"));
        let members = &parts[1];
        assert!(members.get("required").is_none(), "a member is required");
        assert!(
            members["properties"]["details"]
                .to_string()
                .contains("#/components/schemas/AccountUpdateView")
        );
        assert!(members["properties"]["state"].is_object());
        Ok(())
    }
}
