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
//!   [`AccountUpdateFields`] member, or `acct_id` for the path parameter.
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
//! map fields, at most the map `LENGTH` (`modern/audit/screens.md` COACTUP-016 to COACTUP-120):
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

use chrono::NaiveDate;
use domain::online::coactupc as rules;
use domain::sensitive::Ssn;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::{digits, full_text};

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

/// The raw text of every enterable map field of an endpoint 6 request, at most its map
/// `LENGTH`.
///
/// An absent member is the empty text, which the program reads as blank. The account number is
/// the path's `acct_id`. The SSN parts are present only when the operator re-entered them:
/// omitting all three means unchanged, a part omitted beside a present one is blank
/// (COACTUPC-064). They are never serialised, so a body carrying them is written as JSON, and
/// `Debug` prints them as `***`.
#[derive(Clone, Default, Serialize, Deserialize, ToSchema, Validate)]
#[serde(default)]
pub struct AccountUpdateFields {
    /// `ACSTTUS` (1): the active status.
    #[validate(length(max = 1))]
    pub active_status: String,
    /// `OPNYEAR` (4): the open date's year.
    #[validate(length(max = 4))]
    pub open_year: String,
    /// `OPNMON` (2): the open date's month.
    #[validate(length(max = 2))]
    pub open_month: String,
    /// `OPNDAY` (2): the open date's day.
    #[validate(length(max = 2))]
    pub open_day: String,
    /// `ACRDLIM` (15): the credit limit.
    #[validate(length(max = 15))]
    pub credit_limit: String,
    /// `EXPYEAR` (4): the expiry date's year.
    #[validate(length(max = 4))]
    pub expiry_year: String,
    /// `EXPMON` (2): the expiry date's month.
    #[validate(length(max = 2))]
    pub expiry_month: String,
    /// `EXPDAY` (2): the expiry date's day.
    #[validate(length(max = 2))]
    pub expiry_day: String,
    /// `ACSHLIM` (15): the cash credit limit.
    #[validate(length(max = 15))]
    pub cash_credit_limit: String,
    /// `RISYEAR` (4): the reissue date's year.
    #[validate(length(max = 4))]
    pub reissue_year: String,
    /// `RISMON` (2): the reissue date's month.
    #[validate(length(max = 2))]
    pub reissue_month: String,
    /// `RISDAY` (2): the reissue date's day.
    #[validate(length(max = 2))]
    pub reissue_day: String,
    /// `ACURBAL` (15): the current balance.
    #[validate(length(max = 15))]
    pub current_balance: String,
    /// `ACRCYCR` (15): the current cycle credit.
    #[validate(length(max = 15))]
    pub current_cycle_credit: String,
    /// `AADDGRP` (10): the account group.
    #[validate(length(max = 10))]
    pub group_id: String,
    /// `ACRCYDB` (15): the current cycle debit.
    #[validate(length(max = 15))]
    pub current_cycle_debit: String,
    /// `ACSTNUM` (9): the customer ID, read-only once details are shown.
    #[validate(length(max = 9))]
    pub customer_id: String,
    /// `ACTSSN1` (3): the first three SSN digits, only when re-entered.
    #[serde(skip_serializing_if = "withheld")]
    #[schema(write_only, example = "***")]
    #[validate(length(max = 3))]
    pub ssn_part1: Option<String>,
    /// `ACTSSN2` (2): the fourth and fifth SSN digits, only when re-entered.
    #[serde(skip_serializing_if = "withheld")]
    #[schema(write_only, example = "**")]
    #[validate(length(max = 2))]
    pub ssn_part2: Option<String>,
    /// `ACTSSN3` (4): the last four SSN digits, only when re-entered.
    #[serde(skip_serializing_if = "withheld")]
    #[schema(write_only, example = "****")]
    #[validate(length(max = 4))]
    pub ssn_part3: Option<String>,
    /// `DOBYEAR` (4): the date of birth's year.
    #[validate(length(max = 4))]
    pub dob_year: String,
    /// `DOBMON` (2): the date of birth's month.
    #[validate(length(max = 2))]
    pub dob_month: String,
    /// `DOBDAY` (2): the date of birth's day.
    #[validate(length(max = 2))]
    pub dob_day: String,
    /// `ACSTFCO` (3): the FICO score.
    #[validate(length(max = 3))]
    pub fico_score: String,
    /// `ACSFNAM` (25): the first name.
    #[validate(length(max = 25))]
    pub first_name: String,
    /// `ACSMNAM` (25): the middle name.
    #[validate(length(max = 25))]
    pub middle_name: String,
    /// `ACSLNAM` (25): the last name.
    #[validate(length(max = 25))]
    pub last_name: String,
    /// `ACSADL1` (50): address line 1.
    #[validate(length(max = 50))]
    pub address_line_1: String,
    /// `ACSSTTE` (2): the state code.
    #[validate(length(max = 2))]
    pub state: String,
    /// `ACSADL2` (50): address line 2.
    #[validate(length(max = 50))]
    pub address_line_2: String,
    /// `ACSZIPC` (5): the ZIP code.
    #[validate(length(max = 5))]
    pub zip: String,
    /// `ACSCITY` (50): address line 3, the city.
    #[validate(length(max = 50))]
    pub city: String,
    /// `ACSCTRY` (3): the country code, read-only once details are shown.
    #[validate(length(max = 3))]
    pub country: String,
    /// `ACSPH1A` (3): phone number 1's area code.
    #[validate(length(max = 3))]
    pub phone_1_area: String,
    /// `ACSPH1B` (3): phone number 1's prefix.
    #[validate(length(max = 3))]
    pub phone_1_prefix: String,
    /// `ACSPH1C` (4): phone number 1's line number.
    #[validate(length(max = 4))]
    pub phone_1_line: String,
    /// `ACSGOVT` (20): the government-issued ID.
    #[validate(length(max = 20))]
    pub government_id: String,
    /// `ACSPH2A` (3): phone number 2's area code.
    #[validate(length(max = 3))]
    pub phone_2_area: String,
    /// `ACSPH2B` (3): phone number 2's prefix.
    #[validate(length(max = 3))]
    pub phone_2_prefix: String,
    /// `ACSPH2C` (4): phone number 2's line number.
    #[validate(length(max = 4))]
    pub phone_2_line: String,
    /// `ACSEFTC` (10): the EFT account ID.
    #[validate(length(max = 10))]
    pub eft_account_id: String,
    /// `ACSPFLG` (1): the primary card holder indicator.
    #[validate(length(max = 1))]
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

/// The body of endpoint 6: the key, the raw text of the map fields and the echoed snapshot,
/// whose `state` the last response set (COACTUPC-100).
///
/// `Debug` prints the SSN parts as `***`, through [`AccountUpdateFields`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Validate)]
pub struct AccountUpdateRequest {
    /// `ENTER` validates, `PF5` saves.
    pub aid: Aid,
    /// The raw text of every enterable map field.
    #[validate(nested)]
    pub fields: AccountUpdateFields,
    /// The snapshot of the fetch, with the state of the last response.
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
    use serde_json::{Value, json};
    use utoipa::{OpenApi, ToSchema};
    use validator::Validate;

    use super::{
        AccountUpdateAmounts, AccountUpdateFields, AccountUpdateRequest, AccountUpdateResult,
        AccountUpdateSnapshot, AccountUpdateView, Aid,
    };
    use crate::dto::{FIELD_LENGTH_MESSAGE, digits, validation_error};

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
}
