//! The wire types of `GET /accounts/{id}` (CAVW, `COACTVWC`, map `COACTVW`)
//! and `PUT /accounts/{id}` (CAUP, `COACTUPC`, map `COACTUP`).
//!
//! [`AccountResponse`] carries what `COACTVWC` `1200-SETUP-SCREEN-VARS` moves
//! to the `COACTVW` map [app/cbl/COACTVWC.cbl:L466-522]: the account and
//! customer records under their entity field names, plus the account's
//! cross-referenced card number for navigation and the version token that
//! replaces the COMMAREA copy of the fetched records (V-COACTUPC-001, E-24).
//! The SSN and amounts are sent unedited; the SPA formats them.
//!
//! [`AccountUpdateRequest`] carries the unprotected `COACTUP` input fields
//! [app/bms/COACTUP.bms] as entered, so a blank or missing field reaches
//! `carddemo_domain::rules::account_update` and gets its audited COBOL
//! message. The only edit made here is the BMS field length, of the body
//! fields and of the `{id}` path segment that stands for `ACCTSID`.
//!
//! [`AccountViewField`] and [`AccountUpdateField`] list the BMS fields the
//! error bodies of the two operations can name: the distinct `BMS field`
//! values of the `COACTVWC` and `COACTUPC` rows of
//! `docs/parity/validation-audit.md`, plus the four `COACTUP` input fields no
//! edit checks, which only the length check reports.

use std::fmt;

use carddemo_domain::rules::VersionToken;
use carddemo_domain::rules::account_update::AccountUpdateInput;
use carddemo_domain::rules::account_view::AccountView;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

use super::{BmsField, id_string, money, reject_invalid, text};
use crate::error::ApiError;

/// The `message` of a request whose fields exceed their BMS lengths.
///
/// The 3270 field length bounds every `COACTUP` entry, so no COBOL path, and
/// no `COACTUPC` row of the validation audit, exists for a longer one. The
/// answer is 400 `Invalid` with no message text, the treatment the audit gives
/// a REST input with no COBOL counterpart (E-49), and `fields` naming each
/// over-length field.
const OVER_LENGTH_MESSAGE: &str = "";

/// `ACCTSIDI PIC X(11)` [app/cpy-bms/COACTUP.CPY:L60], the bound of the
/// `{id}` path segment of `PUT /accounts/{id}`.
const ACCOUNT_ID_LEN: usize = 11;

/// The name the `{id}` path segment of `PUT /accounts/{id}` takes among the
/// length errors of [`AccountUpdateRequest::check`].
const PATH_ACCOUNT_ID: &str = "account_id";

/// The `validator` code of a failed length check.
const LENGTH_CODE: &str = "length";

/// A withheld value, which `Debug` shows as `<redacted>`: the social security
/// number and the card number, which `CustSsn` and `CardNumber` keep out of
/// diagnostics.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// A BMS field the errors of `GET /accounts/{id}` name: the `COACTVW` map
/// [app/cpy-bms/COACTVW.CPY].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AccountViewField {
    /// `ACCTSID`, the account number: every `COACTVWC` outcome places the
    /// cursor here (V-COACTVWC-001 to V-COACTVWC-008).
    #[serde(rename = "ACCTSID")]
    Acctsid,
}

impl AccountViewField {
    /// The BMS field name, as `fields[].field` carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acctsid => "ACCTSID",
        }
    }
}

impl BmsField for AccountViewField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The `Order` of the field's first `COACTVWC` row: 1 (V-COACTVWC-001).
    fn rank(self) -> u16 {
        match self {
            Self::Acctsid => 1,
        }
    }
}

/// A BMS field the errors of `PUT /accounts/{id}` name: the `COACTUP` map
/// [app/cpy-bms/COACTUP.CPY], in map order.
///
/// Each variant is either a distinct `BMS field` of the `COACTUPC` rows of the
/// validation audit or one of the four input fields no edit checks
/// (`AADDGRP`, `ACSTNUM`, `ACSADL2`, `ACSGOVT`), which only
/// [`AccountUpdateRequest::check`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AccountUpdateField {
    /// `ACCTSID`, the account number (the `{id}` path segment): the key edit,
    /// the read, lock and rewrite failures, the record-changed check and the
    /// `CSUTLDTC` rejections (V-COACTUPC-002 to V-COACTUPC-009,
    /// V-COACTUPC-023, V-COACTUPC-036, V-COACTUPC-049, V-COACTUPC-076,
    /// V-COACTUPC-124 to V-COACTUPC-128), and a segment longer than the
    /// field's 11 characters, which only [`AccountUpdateRequest::check`]
    /// reports.
    #[serde(rename = "ACCTSID")]
    Acctsid,
    /// `ACSTTUS`, Account Status (V-COACTUPC-010 to V-COACTUPC-012).
    #[serde(rename = "ACSTTUS")]
    Acsttus,
    /// `OPNYEAR`, Open Date year (V-COACTUPC-013 to V-COACTUPC-015,
    /// V-COACTUPC-022).
    #[serde(rename = "OPNYEAR")]
    Opnyear,
    /// `OPNMON`, Open Date month (V-COACTUPC-016, V-COACTUPC-017,
    /// V-COACTUPC-020, V-COACTUPC-021).
    #[serde(rename = "OPNMON")]
    Opnmon,
    /// `OPNDAY`, Open Date day (V-COACTUPC-018, V-COACTUPC-019).
    #[serde(rename = "OPNDAY")]
    Opnday,
    /// `ACRDLIM`, Credit Limit (V-COACTUPC-024, V-COACTUPC-025).
    #[serde(rename = "ACRDLIM")]
    Acrdlim,
    /// `EXPYEAR`, Expiry Date year (V-COACTUPC-026 to V-COACTUPC-028,
    /// V-COACTUPC-035).
    #[serde(rename = "EXPYEAR")]
    Expyear,
    /// `EXPMON`, Expiry Date month (V-COACTUPC-029, V-COACTUPC-030,
    /// V-COACTUPC-033, V-COACTUPC-034).
    #[serde(rename = "EXPMON")]
    Expmon,
    /// `EXPDAY`, Expiry Date day (V-COACTUPC-031, V-COACTUPC-032).
    #[serde(rename = "EXPDAY")]
    Expday,
    /// `ACSHLIM`, Cash Credit Limit (V-COACTUPC-037, V-COACTUPC-038).
    #[serde(rename = "ACSHLIM")]
    Acshlim,
    /// `RISYEAR`, Reissue Date year (V-COACTUPC-039 to V-COACTUPC-041,
    /// V-COACTUPC-048).
    #[serde(rename = "RISYEAR")]
    Risyear,
    /// `RISMON`, Reissue Date month (V-COACTUPC-042, V-COACTUPC-043,
    /// V-COACTUPC-046, V-COACTUPC-047).
    #[serde(rename = "RISMON")]
    Rismon,
    /// `RISDAY`, Reissue Date day (V-COACTUPC-044, V-COACTUPC-045).
    #[serde(rename = "RISDAY")]
    Risday,
    /// `ACURBAL`, Current Balance (V-COACTUPC-050, V-COACTUPC-051).
    #[serde(rename = "ACURBAL")]
    Acurbal,
    /// `ACRCYCR`, Current Cycle Credit (V-COACTUPC-052, V-COACTUPC-053).
    #[serde(rename = "ACRCYCR")]
    Acrcycr,
    /// `AADDGRP`, Account Group: no edit checks it.
    #[serde(rename = "AADDGRP")]
    Aaddgrp,
    /// `ACRCYDB`, Current Cycle Debit (V-COACTUPC-054, V-COACTUPC-055).
    #[serde(rename = "ACRCYDB")]
    Acrcydb,
    /// `ACSTNUM`, Customer id: no edit checks it.
    #[serde(rename = "ACSTNUM")]
    Acstnum,
    /// `ACTSSN1`, SSN first 3 digits (V-COACTUPC-056 to V-COACTUPC-059).
    #[serde(rename = "ACTSSN1")]
    Actssn1,
    /// `ACTSSN2`, SSN 4th and 5th digits (V-COACTUPC-060 to
    /// V-COACTUPC-062).
    #[serde(rename = "ACTSSN2")]
    Actssn2,
    /// `ACTSSN3`, SSN last 4 digits (V-COACTUPC-063 to V-COACTUPC-065).
    #[serde(rename = "ACTSSN3")]
    Actssn3,
    /// `DOBYEAR`, Date of Birth year (V-COACTUPC-066 to V-COACTUPC-068,
    /// V-COACTUPC-075, V-COACTUPC-077).
    #[serde(rename = "DOBYEAR")]
    Dobyear,
    /// `DOBMON`, Date of Birth month (V-COACTUPC-069, V-COACTUPC-070,
    /// V-COACTUPC-073, V-COACTUPC-074).
    #[serde(rename = "DOBMON")]
    Dobmon,
    /// `DOBDAY`, Date of Birth day (V-COACTUPC-071, V-COACTUPC-072).
    #[serde(rename = "DOBDAY")]
    Dobday,
    /// `ACSTFCO`, FICO Score (V-COACTUPC-078 to V-COACTUPC-081).
    #[serde(rename = "ACSTFCO")]
    Acstfco,
    /// `ACSFNAM`, First Name (V-COACTUPC-082, V-COACTUPC-083).
    #[serde(rename = "ACSFNAM")]
    Acsfnam,
    /// `ACSMNAM`, Middle Name (V-COACTUPC-084).
    #[serde(rename = "ACSMNAM")]
    Acsmnam,
    /// `ACSLNAM`, Last Name (V-COACTUPC-085, V-COACTUPC-086).
    #[serde(rename = "ACSLNAM")]
    Acslnam,
    /// `ACSADL1`, Address line 1 (V-COACTUPC-087).
    #[serde(rename = "ACSADL1")]
    Acsadl1,
    /// `ACSSTTE`, State (V-COACTUPC-088 to V-COACTUPC-090, V-COACTUPC-123).
    #[serde(rename = "ACSSTTE")]
    Acsstte,
    /// `ACSADL2`, Address line 2: no edit checks it.
    #[serde(rename = "ACSADL2")]
    Acsadl2,
    /// `ACSZIPC`, Zip (V-COACTUPC-091 to V-COACTUPC-093).
    #[serde(rename = "ACSZIPC")]
    Acszipc,
    /// `ACSCITY`, City, stored as address line 3 (V-COACTUPC-094,
    /// V-COACTUPC-095).
    #[serde(rename = "ACSCITY")]
    Acscity,
    /// `ACSCTRY`, Country (V-COACTUPC-096, V-COACTUPC-097).
    #[serde(rename = "ACSCTRY")]
    Acsctry,
    /// `ACSPH1A`, Phone 1 area code (V-COACTUPC-098 to V-COACTUPC-101).
    #[serde(rename = "ACSPH1A")]
    Acsph1a,
    /// `ACSPH1B`, Phone 1 prefix (V-COACTUPC-102 to V-COACTUPC-104).
    #[serde(rename = "ACSPH1B")]
    Acsph1b,
    /// `ACSPH1C`, Phone 1 line number (V-COACTUPC-105 to V-COACTUPC-107).
    #[serde(rename = "ACSPH1C")]
    Acsph1c,
    /// `ACSGOVT`, Government Issued Id: no edit checks it.
    #[serde(rename = "ACSGOVT")]
    Acsgovt,
    /// `ACSPH2A`, Phone 2 area code (V-COACTUPC-108 to V-COACTUPC-111).
    #[serde(rename = "ACSPH2A")]
    Acsph2a,
    /// `ACSPH2B`, Phone 2 prefix (V-COACTUPC-112 to V-COACTUPC-114).
    #[serde(rename = "ACSPH2B")]
    Acsph2b,
    /// `ACSPH2C`, Phone 2 line number (V-COACTUPC-115 to V-COACTUPC-117).
    #[serde(rename = "ACSPH2C")]
    Acsph2c,
    /// `ACSEFTC`, EFT Account Id (V-COACTUPC-118 to V-COACTUPC-120).
    #[serde(rename = "ACSEFTC")]
    Acseftc,
    /// `ACSPFLG`, Primary Card Holder (V-COACTUPC-121, V-COACTUPC-122).
    #[serde(rename = "ACSPFLG")]
    Acspflg,
}

impl AccountUpdateField {
    /// The BMS field name, as `fields[].field` carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acctsid => "ACCTSID",
            Self::Acsttus => "ACSTTUS",
            Self::Opnyear => "OPNYEAR",
            Self::Opnmon => "OPNMON",
            Self::Opnday => "OPNDAY",
            Self::Acrdlim => "ACRDLIM",
            Self::Expyear => "EXPYEAR",
            Self::Expmon => "EXPMON",
            Self::Expday => "EXPDAY",
            Self::Acshlim => "ACSHLIM",
            Self::Risyear => "RISYEAR",
            Self::Rismon => "RISMON",
            Self::Risday => "RISDAY",
            Self::Acurbal => "ACURBAL",
            Self::Acrcycr => "ACRCYCR",
            Self::Aaddgrp => "AADDGRP",
            Self::Acrcydb => "ACRCYDB",
            Self::Acstnum => "ACSTNUM",
            Self::Actssn1 => "ACTSSN1",
            Self::Actssn2 => "ACTSSN2",
            Self::Actssn3 => "ACTSSN3",
            Self::Dobyear => "DOBYEAR",
            Self::Dobmon => "DOBMON",
            Self::Dobday => "DOBDAY",
            Self::Acstfco => "ACSTFCO",
            Self::Acsfnam => "ACSFNAM",
            Self::Acsmnam => "ACSMNAM",
            Self::Acslnam => "ACSLNAM",
            Self::Acsadl1 => "ACSADL1",
            Self::Acsstte => "ACSSTTE",
            Self::Acsadl2 => "ACSADL2",
            Self::Acszipc => "ACSZIPC",
            Self::Acscity => "ACSCITY",
            Self::Acsctry => "ACSCTRY",
            Self::Acsph1a => "ACSPH1A",
            Self::Acsph1b => "ACSPH1B",
            Self::Acsph1c => "ACSPH1C",
            Self::Acsgovt => "ACSGOVT",
            Self::Acsph2a => "ACSPH2A",
            Self::Acsph2b => "ACSPH2B",
            Self::Acsph2c => "ACSPH2C",
            Self::Acseftc => "ACSEFTC",
            Self::Acspflg => "ACSPFLG",
        }
    }
}

impl BmsField for AccountUpdateField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The lowest `Order` among the field's `COACTUPC` rows: the rank at which
    /// `1200-EDIT-MAP-INPUTS` first reaches it, so the fields of one error body
    /// come in the order the COBOL edits them and the first is the one the
    /// cursor goes to.
    ///
    /// The four fields no edit checks have no row. They rank after the last
    /// audited `Order` (124), in map order, because only the length check,
    /// which has no COBOL counterpart, can report them.
    fn rank(self) -> u16 {
        match self {
            Self::Acctsid => 2,
            Self::Acsttus => 7,
            Self::Opnyear => 10,
            Self::Opnmon => 13,
            Self::Opnday => 15,
            Self::Acrdlim => 21,
            Self::Expyear => 23,
            Self::Expmon => 26,
            Self::Expday => 28,
            Self::Acshlim => 34,
            Self::Risyear => 36,
            Self::Rismon => 39,
            Self::Risday => 41,
            Self::Acurbal => 47,
            Self::Acrcycr => 49,
            Self::Acrcydb => 51,
            Self::Actssn1 => 53,
            Self::Actssn2 => 57,
            Self::Actssn3 => 60,
            Self::Dobyear => 63,
            Self::Dobmon => 66,
            Self::Dobday => 68,
            Self::Acstfco => 75,
            Self::Acsfnam => 79,
            Self::Acsmnam => 81,
            Self::Acslnam => 82,
            Self::Acsadl1 => 84,
            Self::Acsstte => 85,
            Self::Acszipc => 88,
            Self::Acscity => 91,
            Self::Acsctry => 93,
            Self::Acsph1a => 95,
            Self::Acsph1b => 99,
            Self::Acsph1c => 102,
            Self::Acsph2a => 105,
            Self::Acsph2b => 109,
            Self::Acsph2c => 112,
            Self::Acseftc => 115,
            Self::Acspflg => 118,
            Self::Aaddgrp => 125,
            Self::Acstnum => 126,
            Self::Acsadl2 => 127,
            Self::Acsgovt => 128,
        }
    }
}

/// The account `GET /accounts/{id}` shows (CAVW), and the record part of the
/// `PUT /accounts/{id}` answer.
///
/// `COACTVWC` `1200-SETUP-SCREEN-VARS` [app/cbl/COACTVWC.cbl:L466-522]
/// (P-COACTVWC-014, P-COACTVWC-015) moves these fields, in this order, to the
/// `COACTVW` map; the `ACCT-ADDR-ZIP` of the account record is not shown and
/// not sent. Text fields are right-trimmed, `9(n)` fields are their
/// zero-padded digits and amounts are scale-2 decimal strings.
#[derive(Clone, Serialize, ToSchema)]
pub struct AccountResponse {
    /// `ACCTSID`: `ACCT-ID 9(11)`, 11 digits.
    pub account_id: String,
    /// `ACSTTUS`: `ACCT-ACTIVE-STATUS X(01)`.
    pub acct_active_status: String,
    /// `ACURBAL`: `ACCT-CURR-BAL S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_curr_bal: Decimal,
    /// `ACRDLIM`: `ACCT-CREDIT-LIMIT S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_credit_limit: Decimal,
    /// `ACSHLIM`: `ACCT-CASH-CREDIT-LIMIT S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_cash_credit_limit: Decimal,
    /// `ACRCYCR`: `ACCT-CURR-CYC-CREDIT S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_curr_cyc_credit: Decimal,
    /// `ACRCYDB`: `ACCT-CURR-CYC-DEBIT S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_curr_cyc_debit: Decimal,
    /// `ADTOPEN`: `ACCT-OPEN-DATE X(10)`, the stored text.
    pub acct_open_date: String,
    /// `AEXPDT`: `ACCT-EXPIRAION-DATE X(10)`, the copybook's spelling kept.
    pub acct_expiraion_date: String,
    /// `AREISDT`: `ACCT-REISSUE-DATE X(10)`.
    pub acct_reissue_date: String,
    /// `AADDGRP`: `ACCT-GROUP-ID X(10)`.
    pub acct_group_id: String,
    /// `ACSTNUM`: `CUST-ID 9(09)`, 9 digits.
    pub customer_id: String,
    /// `ACSTSSN`: `CUST-SSN 9(09)`, 9 digits; the map shows them as
    /// `999-99-9999`.
    pub cust_ssn: String,
    /// `ACSTDOB`: `CUST-DOB-YYYY-MM-DD X(10)`.
    pub cust_dob_yyyy_mm_dd: String,
    /// `ACSTFCO`: `CUST-FICO-CREDIT-SCORE 9(03)`, 3 digits.
    pub cust_fico_credit_score: String,
    /// `ACSFNAM`: `CUST-FIRST-NAME X(25)`.
    pub cust_first_name: String,
    /// `ACSMNAM`: `CUST-MIDDLE-NAME X(25)`.
    pub cust_middle_name: String,
    /// `ACSLNAM`: `CUST-LAST-NAME X(25)`.
    pub cust_last_name: String,
    /// `ACSADL1`: `CUST-ADDR-LINE-1 X(50)`.
    pub cust_addr_line_1: String,
    /// `ACSADL2`: `CUST-ADDR-LINE-2 X(50)`.
    pub cust_addr_line_2: String,
    /// `ACSCITY`: `CUST-ADDR-LINE-3 X(50)`, which the map shows as the city.
    pub cust_addr_line_3: String,
    /// `ACSSTTE`: `CUST-ADDR-STATE-CD X(02)`.
    pub cust_addr_state_cd: String,
    /// `ACSZIPC`: `CUST-ADDR-ZIP X(10)`.
    pub cust_addr_zip: String,
    /// `ACSCTRY`: `CUST-ADDR-COUNTRY-CD X(03)`.
    pub cust_addr_country_cd: String,
    /// `ACSPHN1`: `CUST-PHONE-NUM-1 X(15)`.
    pub cust_phone_num_1: String,
    /// `ACSPHN2`: `CUST-PHONE-NUM-2 X(15)`.
    pub cust_phone_num_2: String,
    /// `ACSGOVT`: `CUST-GOVT-ISSUED-ID X(20)`.
    pub cust_govt_issued_id: String,
    /// `ACSEFTC`: `CUST-EFT-ACCOUNT-ID X(10)`.
    pub cust_eft_account_id: String,
    /// `ACSPFLG`: `CUST-PRI-CARD-HOLDER-IND X(01)`.
    pub cust_pri_card_holder_ind: String,
    /// `XREF-CARD-NUM X(16)` of the `CXACAIX` record the customer was found
    /// through (P-COACTVWC-007): the account's first card, for navigation to
    /// the card screens. No map field shows it.
    pub card_number: String,
    /// The token `PUT /accounts/{id}` echoes back: 16 lower-case hex digits
    /// standing for the fields `9700-CHECK-CHANGE-IN-REC` compares
    /// (V-COACTUPC-126).
    #[schema(pattern = "^[0-9a-f]{16}$")]
    pub version: String,
}

impl AccountResponse {
    /// COACTVWC 1200-SETUP-SCREEN-VARS — P-COACTVWC-011, P-COACTVWC-014,
    /// P-COACTVWC-015
    ///
    /// The response for the records `account_view::view` read, or for the
    /// records an update left, with the token of those records.
    #[must_use]
    pub fn from_view(view: &AccountView) -> Self {
        let AccountView {
            account,
            customer,
            card_xref,
            version,
        } = view;
        Self {
            account_id: id_string(&account.acct_id),
            acct_active_status: text(&account.acct_active_status),
            acct_curr_bal: money(account.acct_curr_bal),
            acct_credit_limit: money(account.acct_credit_limit),
            acct_cash_credit_limit: money(account.acct_cash_credit_limit),
            acct_curr_cyc_credit: money(account.acct_curr_cyc_credit),
            acct_curr_cyc_debit: money(account.acct_curr_cyc_debit),
            acct_open_date: text(&account.acct_open_date),
            acct_expiraion_date: text(&account.acct_expiraion_date),
            acct_reissue_date: text(&account.acct_reissue_date),
            acct_group_id: id_string(&account.acct_group_id),
            customer_id: id_string(&customer.cust_id),
            cust_ssn: id_string(&customer.cust_ssn),
            cust_dob_yyyy_mm_dd: text(&customer.cust_dob_yyyy_mm_dd),
            cust_fico_credit_score: id_string(&customer.cust_fico_credit_score),
            cust_first_name: text(&customer.cust_first_name),
            cust_middle_name: text(&customer.cust_middle_name),
            cust_last_name: text(&customer.cust_last_name),
            cust_addr_line_1: text(&customer.cust_addr_line_1),
            cust_addr_line_2: text(&customer.cust_addr_line_2),
            cust_addr_line_3: text(&customer.cust_addr_line_3),
            cust_addr_state_cd: text(&customer.cust_addr_state_cd),
            cust_addr_zip: text(&customer.cust_addr_zip),
            cust_addr_country_cd: text(&customer.cust_addr_country_cd),
            cust_phone_num_1: text(&customer.cust_phone_num_1),
            cust_phone_num_2: text(&customer.cust_phone_num_2),
            cust_govt_issued_id: text(&customer.cust_govt_issued_id),
            cust_eft_account_id: text(&customer.cust_eft_account_id),
            cust_pri_card_holder_ind: text(&customer.cust_pri_card_holder_ind),
            card_number: id_string(&card_xref.xref_card_num),
            version: version.as_str().to_owned(),
        }
    }
}

/// Every field but the social security number and the card number, which it
/// withholds as the domain's `CustSsn` and `CardNumber` do: a handler span
/// that records its return value must not log them.
impl fmt::Debug for AccountResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountResponse")
            .field("account_id", &self.account_id)
            .field("acct_active_status", &self.acct_active_status)
            .field("acct_curr_bal", &self.acct_curr_bal)
            .field("acct_credit_limit", &self.acct_credit_limit)
            .field("acct_cash_credit_limit", &self.acct_cash_credit_limit)
            .field("acct_curr_cyc_credit", &self.acct_curr_cyc_credit)
            .field("acct_curr_cyc_debit", &self.acct_curr_cyc_debit)
            .field("acct_open_date", &self.acct_open_date)
            .field("acct_expiraion_date", &self.acct_expiraion_date)
            .field("acct_reissue_date", &self.acct_reissue_date)
            .field("acct_group_id", &self.acct_group_id)
            .field("customer_id", &self.customer_id)
            .field("cust_ssn", &Redacted)
            .field("cust_dob_yyyy_mm_dd", &self.cust_dob_yyyy_mm_dd)
            .field("cust_fico_credit_score", &self.cust_fico_credit_score)
            .field("cust_first_name", &self.cust_first_name)
            .field("cust_middle_name", &self.cust_middle_name)
            .field("cust_last_name", &self.cust_last_name)
            .field("cust_addr_line_1", &self.cust_addr_line_1)
            .field("cust_addr_line_2", &self.cust_addr_line_2)
            .field("cust_addr_line_3", &self.cust_addr_line_3)
            .field("cust_addr_state_cd", &self.cust_addr_state_cd)
            .field("cust_addr_zip", &self.cust_addr_zip)
            .field("cust_addr_country_cd", &self.cust_addr_country_cd)
            .field("cust_phone_num_1", &self.cust_phone_num_1)
            .field("cust_phone_num_2", &self.cust_phone_num_2)
            .field("cust_govt_issued_id", &self.cust_govt_issued_id)
            .field("cust_eft_account_id", &self.cust_eft_account_id)
            .field("cust_pri_card_holder_ind", &self.cust_pri_card_holder_ind)
            .field("card_number", &Redacted)
            .field("version", &self.version)
            .finish()
    }
}

/// The body of `PUT /accounts/{id}` (CAUP): the `COACTUP` input fields
/// `1100-RECEIVE-MAP` receives into `ACUP-NEW-DETAILS`
/// [app/cbl/COACTUPC.cbl:L757-860], in map order, each as entered.
///
/// The account number is the `{id}` path segment (`ACCTSID`, 11 characters),
/// not a body field; [`AccountUpdateRequest::check`] bounds it with the body
/// fields. A missing or blank field is passed on as empty text,
/// which the rule receives as `LOW-VALUES` and answers with its audited
/// message, so no field is required here. Each field is bounded by its BMS
/// length [app/bms/COACTUP.bms], checked by [`AccountUpdateRequest::check`].
#[derive(Clone, Default, Deserialize, ToSchema, Validate)]
#[serde(default)]
pub struct AccountUpdateRequest {
    /// `ACSTTUS X(1)`: Account Status, `Y` or `N`.
    #[schema(max_length = 1)]
    #[validate(length(max = 1))]
    pub acct_active_status: Option<String>,
    /// `OPNYEAR X(4)`: Open Date year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub acct_open_year: Option<String>,
    /// `OPNMON X(2)`: Open Date month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub acct_open_mon: Option<String>,
    /// `OPNDAY X(2)`: Open Date day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub acct_open_day: Option<String>,
    /// `ACRDLIM X(15)`: Credit Limit, the amount text `NUMVAL-C` reads.
    #[schema(max_length = 15)]
    #[validate(length(max = 15))]
    pub acct_credit_limit: Option<String>,
    /// `EXPYEAR X(4)`: Expiry Date year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub acct_exp_year: Option<String>,
    /// `EXPMON X(2)`: Expiry Date month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub acct_exp_mon: Option<String>,
    /// `EXPDAY X(2)`: Expiry Date day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub acct_exp_day: Option<String>,
    /// `ACSHLIM X(15)`: Cash Credit Limit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15))]
    pub acct_cash_credit_limit: Option<String>,
    /// `RISYEAR X(4)`: Reissue Date year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub acct_reissue_year: Option<String>,
    /// `RISMON X(2)`: Reissue Date month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub acct_reissue_mon: Option<String>,
    /// `RISDAY X(2)`: Reissue Date day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub acct_reissue_day: Option<String>,
    /// `ACURBAL X(15)`: Current Balance.
    #[schema(max_length = 15)]
    #[validate(length(max = 15))]
    pub acct_curr_bal: Option<String>,
    /// `ACRCYCR X(15)`: Current Cycle Credit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15))]
    pub acct_curr_cyc_credit: Option<String>,
    /// `AADDGRP X(10)`: Account Group, which no edit checks.
    #[schema(max_length = 10)]
    #[validate(length(max = 10))]
    pub acct_group_id: Option<String>,
    /// `ACRCYDB X(15)`: Current Cycle Debit.
    #[schema(max_length = 15)]
    #[validate(length(max = 15))]
    pub acct_curr_cyc_debit: Option<String>,
    /// `ACSTNUM X(9)`: Customer id, sent back as fetched.
    #[schema(max_length = 9)]
    #[validate(length(max = 9))]
    pub customer_id: Option<String>,
    /// `ACTSSN1 X(3)`: the first three SSN digits.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_ssn_1: Option<String>,
    /// `ACTSSN2 X(2)`: the fourth and fifth SSN digits.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub cust_ssn_2: Option<String>,
    /// `ACTSSN3 X(4)`: the last four SSN digits.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub cust_ssn_3: Option<String>,
    /// `DOBYEAR X(4)`: Date of Birth year.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub cust_dob_year: Option<String>,
    /// `DOBMON X(2)`: Date of Birth month.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub cust_dob_mon: Option<String>,
    /// `DOBDAY X(2)`: Date of Birth day.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub cust_dob_day: Option<String>,
    /// `ACSTFCO X(3)`: FICO Score.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_fico_credit_score: Option<String>,
    /// `ACSFNAM X(25)`: First Name.
    #[schema(max_length = 25)]
    #[validate(length(max = 25))]
    pub cust_first_name: Option<String>,
    /// `ACSMNAM X(25)`: Middle Name.
    #[schema(max_length = 25)]
    #[validate(length(max = 25))]
    pub cust_middle_name: Option<String>,
    /// `ACSLNAM X(25)`: Last Name.
    #[schema(max_length = 25)]
    #[validate(length(max = 25))]
    pub cust_last_name: Option<String>,
    /// `ACSADL1 X(50)`: Address line 1.
    #[schema(max_length = 50)]
    #[validate(length(max = 50))]
    pub cust_addr_line_1: Option<String>,
    /// `ACSSTTE X(2)`: State.
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub cust_addr_state_cd: Option<String>,
    /// `ACSADL2 X(50)`: Address line 2, which no edit checks.
    #[schema(max_length = 50)]
    #[validate(length(max = 50))]
    pub cust_addr_line_2: Option<String>,
    /// `ACSZIPC X(5)`: Zip.
    #[schema(max_length = 5)]
    #[validate(length(max = 5))]
    pub cust_addr_zip: Option<String>,
    /// `ACSCITY X(50)`: City, stored as address line 3.
    #[schema(max_length = 50)]
    #[validate(length(max = 50))]
    pub cust_addr_line_3: Option<String>,
    /// `ACSCTRY X(3)`: Country, sent back as fetched.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_addr_country_cd: Option<String>,
    /// `ACSPH1A X(3)`: Phone 1 area code.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_phone_num_1a: Option<String>,
    /// `ACSPH1B X(3)`: Phone 1 prefix.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_phone_num_1b: Option<String>,
    /// `ACSPH1C X(4)`: Phone 1 line number.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub cust_phone_num_1c: Option<String>,
    /// `ACSGOVT X(20)`: Government Issued Id, which no edit checks.
    #[schema(max_length = 20)]
    #[validate(length(max = 20))]
    pub cust_govt_issued_id: Option<String>,
    /// `ACSPH2A X(3)`: Phone 2 area code.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_phone_num_2a: Option<String>,
    /// `ACSPH2B X(3)`: Phone 2 prefix.
    #[schema(max_length = 3)]
    #[validate(length(max = 3))]
    pub cust_phone_num_2b: Option<String>,
    /// `ACSPH2C X(4)`: Phone 2 line number.
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub cust_phone_num_2c: Option<String>,
    /// `ACSEFTC X(10)`: EFT Account Id.
    #[schema(max_length = 10)]
    #[validate(length(max = 10))]
    pub cust_eft_account_id: Option<String>,
    /// `ACSPFLG X(1)`: Primary Card Holder, `Y` or `N`.
    #[schema(max_length = 1)]
    #[validate(length(max = 1))]
    pub cust_pri_card_holder_ind: Option<String>,
    /// The `version` of the `GET /accounts/{id}` response the client edited,
    /// standing for `ACUP-OLD-DETAILS`. A missing or malformed token is
    /// answered in `crate::handlers::accounts` with 400 and no message
    /// (V-COACTUPC-001, E-24) before any COBOL edit, so it has no length rule
    /// here.
    pub version: Option<String>,
}

impl AccountUpdateRequest {
    /// COACTUPC 1100-RECEIVE-MAP, 1210-EDIT-ACCOUNT — P-COACTUPC-018,
    /// V-COACTUPC-002, V-COACTUPC-003 (field lengths only)
    ///
    /// The BMS field lengths of the `COACTUP` input fields [app/bms/COACTUP.bms]:
    /// the body fields, and `account_id`, the `{id}` path segment the handler
    /// passes for `ACCTSIDI X(11)`.
    ///
    /// The path is checked here, with the body, because `1210-EDIT-ACCOUNT`
    /// edits the key only once `1100-RECEIVE-MAP` has moved it into
    /// `CC-ACCT-ID X(11)`, cut at the field's width: a longer segment would
    /// otherwise address the account its first 11 characters name. A segment
    /// of 11 characters or fewer, blank included, passes, and the key edit
    /// answers it (V-COACTUPC-002, V-COACTUPC-003).
    ///
    /// A blank, missing or otherwise invalid value passes: the rule edits it.
    /// Lengths count characters, as the 3270 field positions do.
    ///
    /// # Errors
    ///
    /// 400 `Invalid` with an empty message when any value is longer than its
    /// BMS field, listing each such field once in `fields`, `ACCTSID` for the
    /// path segment, in the order `COACTUPC` edits them
    /// ([`AccountUpdateField`]'s rank).
    pub fn check(&self, account_id: &str) -> Result<(), ApiError> {
        let mut errors = self.validate().err().unwrap_or_default();
        // `validator` counts characters, and so does this bound.
        if account_id.chars().count() > ACCOUNT_ID_LEN {
            errors.add(PATH_ACCOUNT_ID, ValidationError::new(LENGTH_CODE));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(reject_invalid(&errors, update_field, OVER_LENGTH_MESSAGE))
        }
    }

    /// The rule input for the account `account_id` (the `{id}` path segment
    /// as received, which [`AccountUpdateRequest::check`] has bounded) whose
    /// fetched records the client saw under `version`.
    ///
    /// Every value goes on as entered, and a missing one as empty text: the
    /// field `1100-RECEIVE-MAP` reads as not entered.
    #[must_use]
    pub fn into_input(self, account_id: String, version: VersionToken) -> AccountUpdateInput {
        // Destructured in full so that a field added to the request fails to
        // compile until it is mapped. The body's own token is parsed by the
        // handler, which passes the result as `version`.
        let Self {
            acct_active_status,
            acct_open_year,
            acct_open_mon,
            acct_open_day,
            acct_credit_limit,
            acct_exp_year,
            acct_exp_mon,
            acct_exp_day,
            acct_cash_credit_limit,
            acct_reissue_year,
            acct_reissue_mon,
            acct_reissue_day,
            acct_curr_bal,
            acct_curr_cyc_credit,
            acct_group_id,
            acct_curr_cyc_debit,
            customer_id,
            cust_ssn_1,
            cust_ssn_2,
            cust_ssn_3,
            cust_dob_year,
            cust_dob_mon,
            cust_dob_day,
            cust_fico_credit_score,
            cust_first_name,
            cust_middle_name,
            cust_last_name,
            cust_addr_line_1,
            cust_addr_state_cd,
            cust_addr_line_2,
            cust_addr_zip,
            cust_addr_line_3,
            cust_addr_country_cd,
            cust_phone_num_1a,
            cust_phone_num_1b,
            cust_phone_num_1c,
            cust_govt_issued_id,
            cust_phone_num_2a,
            cust_phone_num_2b,
            cust_phone_num_2c,
            cust_eft_account_id,
            cust_pri_card_holder_ind,
            version: _,
        } = self;
        AccountUpdateInput {
            account_id,
            active_status: entered(acct_active_status),
            open_year: entered(acct_open_year),
            open_month: entered(acct_open_mon),
            open_day: entered(acct_open_day),
            credit_limit: entered(acct_credit_limit),
            expiry_year: entered(acct_exp_year),
            expiry_month: entered(acct_exp_mon),
            expiry_day: entered(acct_exp_day),
            cash_credit_limit: entered(acct_cash_credit_limit),
            reissue_year: entered(acct_reissue_year),
            reissue_month: entered(acct_reissue_mon),
            reissue_day: entered(acct_reissue_day),
            current_balance: entered(acct_curr_bal),
            current_cycle_credit: entered(acct_curr_cyc_credit),
            group_id: entered(acct_group_id),
            current_cycle_debit: entered(acct_curr_cyc_debit),
            customer_id: entered(customer_id),
            ssn_part_1: entered(cust_ssn_1),
            ssn_part_2: entered(cust_ssn_2),
            ssn_part_3: entered(cust_ssn_3),
            dob_year: entered(cust_dob_year),
            dob_month: entered(cust_dob_mon),
            dob_day: entered(cust_dob_day),
            fico_score: entered(cust_fico_credit_score),
            first_name: entered(cust_first_name),
            middle_name: entered(cust_middle_name),
            last_name: entered(cust_last_name),
            address_line_1: entered(cust_addr_line_1),
            state: entered(cust_addr_state_cd),
            address_line_2: entered(cust_addr_line_2),
            zip: entered(cust_addr_zip),
            city: entered(cust_addr_line_3),
            country: entered(cust_addr_country_cd),
            phone_1_area: entered(cust_phone_num_1a),
            phone_1_prefix: entered(cust_phone_num_1b),
            phone_1_line: entered(cust_phone_num_1c),
            govt_issued_id: entered(cust_govt_issued_id),
            phone_2_area: entered(cust_phone_num_2a),
            phone_2_prefix: entered(cust_phone_num_2b),
            phone_2_line: entered(cust_phone_num_2c),
            eft_account_id: entered(cust_eft_account_id),
            primary_card_holder: entered(cust_pri_card_holder_ind),
            version,
        }
    }
}

/// A body field as the rule receives it: the text entered, or empty text for
/// a field not sent.
fn entered(value: Option<String>) -> String {
    value.unwrap_or_default()
}

/// The BMS field of the [`AccountUpdateRequest`] field whose JSON name is
/// `name`, or of the `{id}` segment for [`PATH_ACCOUNT_ID`], for the length
/// errors `validator` reports by that name. `None` for `version`, which has
/// no length rule, and for any other name.
fn update_field(name: &str) -> Option<AccountUpdateField> {
    use AccountUpdateField as F;
    let field = match name {
        PATH_ACCOUNT_ID => F::Acctsid,
        "acct_active_status" => F::Acsttus,
        "acct_open_year" => F::Opnyear,
        "acct_open_mon" => F::Opnmon,
        "acct_open_day" => F::Opnday,
        "acct_credit_limit" => F::Acrdlim,
        "acct_exp_year" => F::Expyear,
        "acct_exp_mon" => F::Expmon,
        "acct_exp_day" => F::Expday,
        "acct_cash_credit_limit" => F::Acshlim,
        "acct_reissue_year" => F::Risyear,
        "acct_reissue_mon" => F::Rismon,
        "acct_reissue_day" => F::Risday,
        "acct_curr_bal" => F::Acurbal,
        "acct_curr_cyc_credit" => F::Acrcycr,
        "acct_group_id" => F::Aaddgrp,
        "acct_curr_cyc_debit" => F::Acrcydb,
        "customer_id" => F::Acstnum,
        "cust_ssn_1" => F::Actssn1,
        "cust_ssn_2" => F::Actssn2,
        "cust_ssn_3" => F::Actssn3,
        "cust_dob_year" => F::Dobyear,
        "cust_dob_mon" => F::Dobmon,
        "cust_dob_day" => F::Dobday,
        "cust_fico_credit_score" => F::Acstfco,
        "cust_first_name" => F::Acsfnam,
        "cust_middle_name" => F::Acsmnam,
        "cust_last_name" => F::Acslnam,
        "cust_addr_line_1" => F::Acsadl1,
        "cust_addr_state_cd" => F::Acsstte,
        "cust_addr_line_2" => F::Acsadl2,
        "cust_addr_zip" => F::Acszipc,
        "cust_addr_line_3" => F::Acscity,
        "cust_addr_country_cd" => F::Acsctry,
        "cust_phone_num_1a" => F::Acsph1a,
        "cust_phone_num_1b" => F::Acsph1b,
        "cust_phone_num_1c" => F::Acsph1c,
        "cust_govt_issued_id" => F::Acsgovt,
        "cust_phone_num_2a" => F::Acsph2a,
        "cust_phone_num_2b" => F::Acsph2b,
        "cust_phone_num_2c" => F::Acsph2c,
        "cust_eft_account_id" => F::Acseftc,
        "cust_pri_card_holder_ind" => F::Acspflg,
        _ => return None,
    };
    Some(field)
}

/// Every field but the three social security number parts, which it
/// withholds as `AccountUpdateInput` does: a handler span that records its
/// arguments must not log them.
impl fmt::Debug for AccountUpdateRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountUpdateRequest")
            .field("acct_active_status", &self.acct_active_status)
            .field("acct_open_year", &self.acct_open_year)
            .field("acct_open_mon", &self.acct_open_mon)
            .field("acct_open_day", &self.acct_open_day)
            .field("acct_credit_limit", &self.acct_credit_limit)
            .field("acct_exp_year", &self.acct_exp_year)
            .field("acct_exp_mon", &self.acct_exp_mon)
            .field("acct_exp_day", &self.acct_exp_day)
            .field("acct_cash_credit_limit", &self.acct_cash_credit_limit)
            .field("acct_reissue_year", &self.acct_reissue_year)
            .field("acct_reissue_mon", &self.acct_reissue_mon)
            .field("acct_reissue_day", &self.acct_reissue_day)
            .field("acct_curr_bal", &self.acct_curr_bal)
            .field("acct_curr_cyc_credit", &self.acct_curr_cyc_credit)
            .field("acct_group_id", &self.acct_group_id)
            .field("acct_curr_cyc_debit", &self.acct_curr_cyc_debit)
            .field("customer_id", &self.customer_id)
            .field("cust_ssn_1", &Redacted)
            .field("cust_ssn_2", &Redacted)
            .field("cust_ssn_3", &Redacted)
            .field("cust_dob_year", &self.cust_dob_year)
            .field("cust_dob_mon", &self.cust_dob_mon)
            .field("cust_dob_day", &self.cust_dob_day)
            .field("cust_fico_credit_score", &self.cust_fico_credit_score)
            .field("cust_first_name", &self.cust_first_name)
            .field("cust_middle_name", &self.cust_middle_name)
            .field("cust_last_name", &self.cust_last_name)
            .field("cust_addr_line_1", &self.cust_addr_line_1)
            .field("cust_addr_state_cd", &self.cust_addr_state_cd)
            .field("cust_addr_line_2", &self.cust_addr_line_2)
            .field("cust_addr_zip", &self.cust_addr_zip)
            .field("cust_addr_line_3", &self.cust_addr_line_3)
            .field("cust_addr_country_cd", &self.cust_addr_country_cd)
            .field("cust_phone_num_1a", &self.cust_phone_num_1a)
            .field("cust_phone_num_1b", &self.cust_phone_num_1b)
            .field("cust_phone_num_1c", &self.cust_phone_num_1c)
            .field("cust_govt_issued_id", &self.cust_govt_issued_id)
            .field("cust_phone_num_2a", &self.cust_phone_num_2a)
            .field("cust_phone_num_2b", &self.cust_phone_num_2b)
            .field("cust_phone_num_2c", &self.cust_phone_num_2c)
            .field("cust_eft_account_id", &self.cust_eft_account_id)
            .field("cust_pri_card_holder_ind", &self.cust_pri_card_holder_ind)
            .field("version", &self.version)
            .finish()
    }
}

/// The 200 answer of `PUT /accounts/{id}`: the records as they now stand and
/// the information text `COACTUPC` shows, which the SPA shows as a success
/// notification.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountUpdateResponse {
    /// The account and customer after the request: rewritten, or as fetched
    /// when nothing changed, with their new token.
    pub account: AccountResponse,
    /// `messages::CHANGES_COMMITTED_TO_DATABASE` after both rewrites
    /// (P-COACTUPC-015), or
    /// `messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED` when the
    /// input equals the fetched values (V-COACTUPC-010).
    pub message: String,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use axum::http::StatusCode;
    use carddemo_audit::{Document, Repo, VALIDATION_AUDIT_PATH};
    use carddemo_domain::entities::{Account, CardXref, Customer};
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustSsn, CustomerId, FicoScore, Money,
        NumericError,
    };
    use carddemo_domain::rules::VersionToken;
    use carddemo_domain::rules::account_update::AccountUpdateInput;
    use carddemo_domain::rules::account_view::AccountView;
    use serde_json::{Map, Value, json};
    use utoipa::PartialSchema;

    use super::{
        AccountResponse, AccountUpdateField, AccountUpdateRequest, AccountUpdateResponse,
        AccountViewField, update_field,
    };
    use crate::dto::BmsField;

    /// A well-formed token; the tests never compute one, so no key is needed.
    const TOKEN: &str = "0123456789abcdef";

    /// An `{id}` path segment of the full 11 characters of `ACCTSIDI X(11)`.
    const ACCOUNT: &str = "00000000001";

    /// (JSON name, BMS field, BMS length) of every length-checked request
    /// field, in `COACTUP` map order [app/bms/COACTUP.bms].
    const REQUEST_FIELDS: [(&str, &str, usize); 42] = [
        ("acct_active_status", "ACSTTUS", 1),
        ("acct_open_year", "OPNYEAR", 4),
        ("acct_open_mon", "OPNMON", 2),
        ("acct_open_day", "OPNDAY", 2),
        ("acct_credit_limit", "ACRDLIM", 15),
        ("acct_exp_year", "EXPYEAR", 4),
        ("acct_exp_mon", "EXPMON", 2),
        ("acct_exp_day", "EXPDAY", 2),
        ("acct_cash_credit_limit", "ACSHLIM", 15),
        ("acct_reissue_year", "RISYEAR", 4),
        ("acct_reissue_mon", "RISMON", 2),
        ("acct_reissue_day", "RISDAY", 2),
        ("acct_curr_bal", "ACURBAL", 15),
        ("acct_curr_cyc_credit", "ACRCYCR", 15),
        ("acct_group_id", "AADDGRP", 10),
        ("acct_curr_cyc_debit", "ACRCYDB", 15),
        ("customer_id", "ACSTNUM", 9),
        ("cust_ssn_1", "ACTSSN1", 3),
        ("cust_ssn_2", "ACTSSN2", 2),
        ("cust_ssn_3", "ACTSSN3", 4),
        ("cust_dob_year", "DOBYEAR", 4),
        ("cust_dob_mon", "DOBMON", 2),
        ("cust_dob_day", "DOBDAY", 2),
        ("cust_fico_credit_score", "ACSTFCO", 3),
        ("cust_first_name", "ACSFNAM", 25),
        ("cust_middle_name", "ACSMNAM", 25),
        ("cust_last_name", "ACSLNAM", 25),
        ("cust_addr_line_1", "ACSADL1", 50),
        ("cust_addr_state_cd", "ACSSTTE", 2),
        ("cust_addr_line_2", "ACSADL2", 50),
        ("cust_addr_zip", "ACSZIPC", 5),
        ("cust_addr_line_3", "ACSCITY", 50),
        ("cust_addr_country_cd", "ACSCTRY", 3),
        ("cust_phone_num_1a", "ACSPH1A", 3),
        ("cust_phone_num_1b", "ACSPH1B", 3),
        ("cust_phone_num_1c", "ACSPH1C", 4),
        ("cust_govt_issued_id", "ACSGOVT", 20),
        ("cust_phone_num_2a", "ACSPH2A", 3),
        ("cust_phone_num_2b", "ACSPH2B", 3),
        ("cust_phone_num_2c", "ACSPH2C", 4),
        ("cust_eft_account_id", "ACSEFTC", 10),
        ("cust_pri_card_holder_ind", "ACSPFLG", 1),
    ];

    /// The BMS fields no `COACTUPC` edit checks, in map order.
    const UNEDITED_FIELDS: [&str; 4] = ["AADDGRP", "ACSTNUM", "ACSADL2", "ACSGOVT"];

    fn token() -> VersionToken {
        VersionToken::parse(TOKEN).expect("16 lower-case hex digits")
    }

    /// Account 00000000001 [app/data/ASCII/acctdata.txt:L1] (P-CVACT01Y-015),
    /// its customer [app/data/ASCII/custdata.txt:L1] (P-CVCUS01Y-021) and its
    /// card [app/data/ASCII/cardxref.txt:L49], as the codec decodes them.
    fn fixture_view() -> Result<AccountView, NumericError> {
        Ok(AccountView {
            account: Account {
                acct_id: AccountId::new(1)?,
                acct_active_status: Alnum::move_from("Y"),
                acct_curr_bal: Money::<10>::from_cents(19_400)?,
                acct_credit_limit: Money::<10>::from_cents(202_000)?,
                acct_cash_credit_limit: Money::<10>::from_cents(102_000)?,
                acct_open_date: Alnum::move_from("2014-11-20"),
                acct_expiraion_date: Alnum::move_from("2025-05-20"),
                acct_reissue_date: Alnum::move_from("2025-05-20"),
                acct_curr_cyc_credit: Money::<10>::ZERO,
                acct_curr_cyc_debit: Money::<10>::ZERO,
                acct_addr_zip: Alnum::move_from("A000000000"),
                acct_group_id: AccountGroupId::move_from(""),
            },
            customer: Customer {
                cust_id: CustomerId::new(1)?,
                cust_first_name: Alnum::move_from("Immanuel"),
                cust_middle_name: Alnum::move_from("Madeline"),
                cust_last_name: Alnum::move_from("Kessler"),
                cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
                cust_addr_line_2: Alnum::move_from("Apt. 802"),
                cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
                cust_addr_state_cd: Alnum::move_from("NC"),
                cust_addr_country_cd: Alnum::move_from("USA"),
                cust_addr_zip: Alnum::move_from("12546"),
                cust_phone_num_1: Alnum::move_from("(908)119-8310"),
                cust_phone_num_2: Alnum::move_from("(373)693-8684"),
                cust_ssn: CustSsn::new(20_973_888)?,
                cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
                cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
                cust_eft_account_id: Alnum::move_from("0053581756"),
                cust_pri_card_holder_ind: Alnum::move_from("Y"),
                cust_fico_credit_score: FicoScore::new(274)?,
            },
            card_xref: CardXref {
                xref_card_num: CardNumber::from_trimmed("9680294154603697")?,
                xref_cust_id: CustomerId::new(1)?,
                xref_acct_id: AccountId::new(1)?,
            },
            version: token(),
        })
    }

    /// A request whose only field is `name`, holding `value`.
    fn request_with(name: &str, value: &str) -> AccountUpdateRequest {
        let mut body = Map::new();
        body.insert(name.to_owned(), Value::String(value.to_owned()));
        serde_json::from_value(Value::Object(body)).expect("a request body")
    }

    /// A request whose every length-checked field holds `value(max)`.
    fn request_of_all(value: impl Fn(usize) -> String) -> AccountUpdateRequest {
        let body: Map<String, Value> = REQUEST_FIELDS
            .iter()
            .map(|&(name, _, max)| (name.to_owned(), Value::String(value(max))))
            .collect();
        serde_json::from_value(Value::Object(body)).expect("a request body")
    }

    /// The BMS fields of the error body `check` answers for `request` under
    /// the path segment `account_id`, which must be the 400 `Invalid` length
    /// answer with no message.
    fn rejected_fields(request: &AccountUpdateRequest, account_id: &str) -> Vec<String> {
        let error = request.check(account_id).expect_err("a value is too long");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        let body = error.body();
        assert_eq!(body.code, ErrorKind::Invalid.code());
        assert_eq!(body.message, "");
        assert!(body.fields.iter().all(|field| field.message.is_empty()));
        body.fields
            .iter()
            .map(|field| field.field.clone())
            .collect()
    }

    #[test]
    fn from_view_shows_the_coactvw_fields() -> Result<(), NumericError> {
        let response = AccountResponse::from_view(&fixture_view()?);
        let json = serde_json::to_value(&response).expect("serializes");
        assert_eq!(
            json,
            json!({
                "account_id": "00000000001",
                "acct_active_status": "Y",
                "acct_curr_bal": "194.00",
                "acct_credit_limit": "2020.00",
                "acct_cash_credit_limit": "1020.00",
                "acct_curr_cyc_credit": "0.00",
                "acct_curr_cyc_debit": "0.00",
                "acct_open_date": "2014-11-20",
                "acct_expiraion_date": "2025-05-20",
                "acct_reissue_date": "2025-05-20",
                "acct_group_id": "",
                "customer_id": "000000001",
                "cust_ssn": "020973888",
                "cust_dob_yyyy_mm_dd": "1961-06-08",
                "cust_fico_credit_score": "274",
                "cust_first_name": "Immanuel",
                "cust_middle_name": "Madeline",
                "cust_last_name": "Kessler",
                "cust_addr_line_1": "618 Deshaun Route",
                "cust_addr_line_2": "Apt. 802",
                "cust_addr_line_3": "Altenwerthshire",
                "cust_addr_state_cd": "NC",
                "cust_addr_zip": "12546",
                "cust_addr_country_cd": "USA",
                "cust_phone_num_1": "(908)119-8310",
                "cust_phone_num_2": "(373)693-8684",
                "cust_govt_issued_id": "00000000000049368437",
                "cust_eft_account_id": "0053581756",
                "cust_pri_card_holder_ind": "Y",
                "card_number": "9680294154603697",
                "version": TOKEN,
            })
        );
        // Not on the COACTVW map: the account record's zip and the card's
        // verification code.
        let text = json.to_string();
        assert!(!text.contains("A000000000"), "{text}");
        assert!(!text.contains("cvv"), "{text}");
        Ok(())
    }

    #[test]
    fn a_negative_cycle_debit_keeps_its_sign_below_one_unit() -> Result<(), NumericError> {
        // Synthetic: the fixture account's ACCT-CURR-CYC-DEBIT is +0.00, so
        // this S9(10)V99 value is set by hand to a negative under one dollar,
        // whose sign a zero integer part must not drop.
        let fixture = fixture_view()?;
        let view = AccountView {
            account: Account {
                acct_curr_cyc_debit: Money::<10>::from_cents(-5)?,
                ..fixture.account
            },
            ..fixture
        };
        let json = serde_json::to_value(AccountResponse::from_view(&view)).expect("serializes");
        assert_eq!(json["acct_curr_cyc_debit"], "-0.05");
        Ok(())
    }

    #[test]
    fn update_response_nests_the_account_beside_the_message() -> Result<(), NumericError> {
        let response = AccountUpdateResponse {
            account: AccountResponse::from_view(&fixture_view()?),
            message: messages::CHANGES_COMMITTED_TO_DATABASE.to_owned(),
        };
        let json = serde_json::to_value(&response).expect("serializes");
        assert_eq!(json["message"], messages::CHANGES_COMMITTED_TO_DATABASE);
        assert_eq!(json["account"]["account_id"], "00000000001");
        assert_eq!(json["account"]["version"], TOKEN);
        Ok(())
    }

    #[test]
    fn debug_withholds_the_ssn_and_the_card_number() -> Result<(), NumericError> {
        let response = AccountResponse::from_view(&fixture_view()?);
        let request = AccountUpdateRequest {
            cust_ssn_1: Some("020".to_owned()),
            cust_ssn_2: Some("97".to_owned()),
            cust_ssn_3: Some("3888".to_owned()),
            cust_last_name: Some("Kessler".to_owned()),
            ..AccountUpdateRequest::default()
        };
        let update = AccountUpdateResponse {
            account: response.clone(),
            message: String::new(),
        };
        for shown in [
            format!("{response:?}"),
            format!("{update:?}"),
            format!("{request:?}"),
        ] {
            assert!(shown.contains("Kessler"), "{shown}");
            assert!(shown.contains("<redacted>"), "{shown}");
            for secret in ["020973888", "9680294154603697", "\"020\"", "\"97\"", "3888"] {
                assert!(!shown.contains(secret), "{secret} in {shown}");
            }
        }
        Ok(())
    }

    #[test]
    fn the_schema_bounds_each_field_by_its_bms_length() {
        let schema = serde_json::to_value(AccountUpdateRequest::schema()).expect("serializes");
        let properties = schema["properties"].as_object().expect("an object schema");
        let names: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = REQUEST_FIELDS
            .iter()
            .map(|&(name, _, _)| name)
            .chain(["version"])
            .collect();
        assert_eq!(names, expected);
        for (name, _, max) in REQUEST_FIELDS {
            assert_eq!(properties[name]["maxLength"], json!(max), "{name}");
        }
        assert!(properties["version"].get("maxLength").is_none());
        assert!(schema.get("required").is_none(), "{schema}");

        let response = serde_json::to_value(AccountResponse::schema()).expect("serializes");
        assert_eq!(
            response["properties"]["version"]["pattern"],
            "^[0-9a-f]{16}$"
        );
        assert_eq!(response["properties"]["acct_curr_bal"]["type"], "string");
    }

    #[test]
    fn a_field_over_its_length_is_rejected_at_its_bms_field() {
        for (name, bms, max) in REQUEST_FIELDS {
            assert_eq!(
                update_field(name).map(AccountUpdateField::as_str),
                Some(bms)
            );
            let over = request_with(name, &"9".repeat(max + 1));
            assert_eq!(rejected_fields(&over, ACCOUNT), [bms], "{name}");
            let at_limit = request_with(name, &"9".repeat(max));
            assert!(at_limit.check(ACCOUNT).is_ok(), "{name}");
        }
        assert_eq!(update_field("version"), None);
        // The name of the `{id}` segment's length error, not a body field.
        assert_eq!(
            update_field("account_id"),
            Some(AccountUpdateField::Acctsid)
        );
    }

    #[test]
    fn the_path_segment_is_bounded_by_acctsid() {
        // ACSTTUS ranks first among the body fields, so a body over-length in
        // it and in ACSPFLG shows where ACCTSID falls.
        let over_status_and_flag = AccountUpdateRequest {
            acct_active_status: Some("YY".to_owned()),
            cust_pri_card_holder_ind: Some("YY".to_owned()),
            ..AccountUpdateRequest::default()
        };
        let none = AccountUpdateRequest::default();
        // (segment, body, the BMS fields of the length answer, empty when the
        // check passes)
        let cases: [(&str, &AccountUpdateRequest, &[&str]); 7] = [
            // Over 11 characters: refused before 1100-RECEIVE-MAP would cut
            // them to the account their first 11 characters name.
            ("00000000001X", &none, &["ACCTSID"]),
            ("000000000012", &none, &["ACCTSID"]),
            ("00000000001 ", &none, &["ACCTSID"]),
            // Reported with the body's over-length fields, in COACTUPC order.
            (
                "00000000001X",
                &over_status_and_flag,
                &["ACCTSID", "ACSTTUS", "ACSPFLG"],
            ),
            // 11 characters or fewer, blank included: left to the key edit
            // (V-COACTUPC-002, V-COACTUPC-003).
            ("0000000000A", &none, &[]),
            ("           ", &none, &[]),
            ("", &none, &[]),
        ];
        for (segment, body, expected) in cases {
            if expected.is_empty() {
                assert!(body.check(segment).is_ok(), "{segment:?}");
            } else {
                assert_eq!(rejected_fields(body, segment), expected, "{segment:?}");
            }
        }
    }

    #[test]
    fn every_over_length_field_is_listed_in_coactupc_edit_order() {
        let all_over = request_of_all(|max| "X".repeat(max + 1));
        let expected = [
            "ACSTTUS", "OPNYEAR", "OPNMON", "OPNDAY", "ACRDLIM", "EXPYEAR", "EXPMON", "EXPDAY",
            "ACSHLIM", "RISYEAR", "RISMON", "RISDAY", "ACURBAL", "ACRCYCR", "ACRCYDB", "ACTSSN1",
            "ACTSSN2", "ACTSSN3", "DOBYEAR", "DOBMON", "DOBDAY", "ACSTFCO", "ACSFNAM", "ACSMNAM",
            "ACSLNAM", "ACSADL1", "ACSSTTE", "ACSZIPC", "ACSCITY", "ACSCTRY", "ACSPH1A", "ACSPH1B",
            "ACSPH1C", "ACSPH2A", "ACSPH2B", "ACSPH2C", "ACSEFTC", "ACSPFLG", "AADDGRP", "ACSTNUM",
            "ACSADL2", "ACSGOVT",
        ];
        // `validator` reports through a hash map; the body must not vary.
        for _ in 0..8 {
            assert_eq!(rejected_fields(&all_over, ACCOUNT), expected);
        }

        let two = AccountUpdateRequest {
            cust_pri_card_holder_ind: Some("YY".to_owned()),
            acct_active_status: Some("YY".to_owned()),
            ..AccountUpdateRequest::default()
        };
        assert_eq!(rejected_fields(&two, ACCOUNT), ["ACSTTUS", "ACSPFLG"]);
    }

    #[test]
    fn blank_and_missing_fields_pass_unchanged() {
        let blank = request_of_all(|_| String::new());
        let spaces = request_of_all(|max| " ".repeat(max));
        for request in [AccountUpdateRequest::default(), blank, spaces.clone()] {
            assert!(request.check(ACCOUNT).is_ok(), "{request:?}");
        }
        let missing = AccountUpdateRequest::default().into_input(String::new(), token());
        let empty = request_of_all(|_| String::new()).into_input(String::new(), token());
        assert_eq!(missing, empty);
        assert_eq!(missing.first_name, "");
        assert_eq!(missing.version, token());
        let nulls: AccountUpdateRequest =
            serde_json::from_value(json!({"cust_first_name": null, "version": null}))
                .expect("nulls deserialize");
        assert_eq!(nulls.cust_first_name, None);
        assert_eq!(nulls.version, None);
        assert_eq!(
            spaces.into_input(String::new(), token()).address_line_2,
            " ".repeat(50)
        );
    }

    #[test]
    fn into_input_maps_each_field_as_entered() {
        // Each field holds its own JSON name, so a crossed mapping shows.
        let mut body: Map<String, Value> = REQUEST_FIELDS
            .iter()
            .map(|&(name, _, _)| (name.to_owned(), Value::String(name.to_owned())))
            .collect();
        body.insert("version".to_owned(), json!("ffffffffffffffff"));
        let request: AccountUpdateRequest =
            serde_json::from_value(Value::Object(body)).expect("a request body");
        let input = request.into_input("00000000001".to_owned(), token());
        let entered = str::to_owned;
        let expected = AccountUpdateInput {
            account_id: entered("00000000001"),
            active_status: entered("acct_active_status"),
            open_year: entered("acct_open_year"),
            open_month: entered("acct_open_mon"),
            open_day: entered("acct_open_day"),
            credit_limit: entered("acct_credit_limit"),
            expiry_year: entered("acct_exp_year"),
            expiry_month: entered("acct_exp_mon"),
            expiry_day: entered("acct_exp_day"),
            cash_credit_limit: entered("acct_cash_credit_limit"),
            reissue_year: entered("acct_reissue_year"),
            reissue_month: entered("acct_reissue_mon"),
            reissue_day: entered("acct_reissue_day"),
            current_balance: entered("acct_curr_bal"),
            current_cycle_credit: entered("acct_curr_cyc_credit"),
            group_id: entered("acct_group_id"),
            current_cycle_debit: entered("acct_curr_cyc_debit"),
            customer_id: entered("customer_id"),
            ssn_part_1: entered("cust_ssn_1"),
            ssn_part_2: entered("cust_ssn_2"),
            ssn_part_3: entered("cust_ssn_3"),
            dob_year: entered("cust_dob_year"),
            dob_month: entered("cust_dob_mon"),
            dob_day: entered("cust_dob_day"),
            fico_score: entered("cust_fico_credit_score"),
            first_name: entered("cust_first_name"),
            middle_name: entered("cust_middle_name"),
            last_name: entered("cust_last_name"),
            address_line_1: entered("cust_addr_line_1"),
            state: entered("cust_addr_state_cd"),
            address_line_2: entered("cust_addr_line_2"),
            zip: entered("cust_addr_zip"),
            city: entered("cust_addr_line_3"),
            country: entered("cust_addr_country_cd"),
            phone_1_area: entered("cust_phone_num_1a"),
            phone_1_prefix: entered("cust_phone_num_1b"),
            phone_1_line: entered("cust_phone_num_1c"),
            govt_issued_id: entered("cust_govt_issued_id"),
            phone_2_area: entered("cust_phone_num_2a"),
            phone_2_prefix: entered("cust_phone_num_2b"),
            phone_2_line: entered("cust_phone_num_2c"),
            eft_account_id: entered("cust_eft_account_id"),
            primary_card_holder: entered("cust_pri_card_holder_ind"),
            // The parsed token passed in, not the body's text.
            version: token(),
        };
        assert_eq!(input, expected);
    }

    /// Every `AccountUpdateField`: the request fields' variants and `ACCTSID`.
    fn update_fields() -> Vec<AccountUpdateField> {
        REQUEST_FIELDS
            .iter()
            .filter_map(|&(name, _, _)| update_field(name))
            .chain([AccountUpdateField::Acctsid])
            .collect()
    }

    #[test]
    fn field_enums_serialize_as_their_bms_names() {
        let update: Vec<(&str, u16)> = update_fields()
            .into_iter()
            .map(|field| {
                let name = field.as_str();
                assert_eq!(serde_json::to_value(field).expect("serializes"), name);
                let back: AccountUpdateField =
                    serde_json::from_value(json!(name)).expect("deserializes");
                assert_eq!(back, field);
                assert_eq!(BmsField::as_str(field), name);
                (name, field.rank())
            })
            .collect();
        assert_eq!(update.len(), 43);
        let view = AccountViewField::Acctsid;
        assert_eq!(serde_json::to_value(view).expect("serializes"), "ACCTSID");
        assert_eq!(BmsField::as_str(view), view.as_str());

        let schema = serde_json::to_value(AccountUpdateField::schema()).expect("serializes");
        let listed: BTreeSet<&str> = schema["enum"]
            .as_array()
            .expect("a string enum")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let names: BTreeSet<&str> = update.iter().map(|&(name, _)| name).collect();
        assert_eq!(listed, names);
    }

    /// The lowest `Order` of each BMS field among `program`'s rows of the
    /// validation audit; rows marking no field (`—`) are left out.
    fn first_orders(program: &str) -> BTreeMap<String, u16> {
        let repo = Repo::from_manifest_dir(env!("CARGO_MANIFEST_DIR"));
        let audit =
            Document::load(&repo.root.join(VALIDATION_AUDIT_PATH)).expect("the validation audit");
        let mut orders = BTreeMap::new();
        let tables = audit
            .tables()
            .iter()
            .filter(|table| table.is_data_table() && table.section.as_deref() == Some(program));
        for table in tables {
            for row in &table.rows {
                let field = table.cell(row, "BMS field").expect("a BMS field cell");
                if field == "—" {
                    continue;
                }
                let order: u16 = table
                    .cell(row, "Order")
                    .and_then(|order| order.parse().ok())
                    .expect("a numeric Order");
                orders
                    .entry(field.to_owned())
                    .and_modify(|first: &mut u16| *first = (*first).min(order))
                    .or_insert(order);
            }
        }
        assert!(!orders.is_empty(), "no {program} rows");
        orders
    }

    #[test]
    fn ranks_are_the_first_audit_order_of_each_field() {
        let audited = first_orders("COACTUPC");
        let last_order = audited.values().copied().max().expect("audited rows");
        for field in update_fields() {
            let name = field.as_str();
            if let Some(&order) = audited.get(name) {
                assert_eq!(field.rank(), order, "{name}");
            } else {
                assert!(UNEDITED_FIELDS.contains(&name), "{name} has no audit row");
                assert!(field.rank() > last_order, "{name}");
            }
        }
        for name in audited.keys() {
            let field: AccountUpdateField =
                serde_json::from_value(json!(name)).expect("an audited field is a variant");
            assert_eq!(field.as_str(), name);
        }
        let unedited: Vec<u16> = UNEDITED_FIELDS
            .iter()
            .map(|name| {
                serde_json::from_value::<AccountUpdateField>(json!(name))
                    .expect("a variant")
                    .rank()
            })
            .collect();
        assert!(unedited.is_sorted(), "{unedited:?}");

        let viewed = first_orders("COACTVWC");
        assert_eq!(
            viewed,
            BTreeMap::from([(AccountViewField::Acctsid.as_str().to_owned(), 1)])
        );
        assert_eq!(AccountViewField::Acctsid.rank(), 1);
    }
}
