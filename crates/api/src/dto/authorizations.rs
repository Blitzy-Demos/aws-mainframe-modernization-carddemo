//! The wire types of CPVS and CPVD: the pending authorizations of an account
//! (`COPAUS0C`, map `COPAU0A` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU00.cpy])
//! and one pending authorization with its PF5 fraud toggle (`COPAUS1C` with
//! `COPAUS2C`, map `COPAU1A` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU01.cpy]).
//!
//! | Operation | Program | Request | Response |
//! |---|---|---|---|
//! | `GET /authorizations` | `COPAUS0C` | [`AuthSummaryQuery`] with [`AuthSummaryPageQuery`] | [`AuthSummaryResponse`] |
//! | `GET /authorizations/{id}` | `COPAUS1C` | the path `auth_id` ([`AuthIdPath`]), through [`parse_auth_id`] | [`AuthDetailResponse`] |
//! | `PUT /authorizations/{id}` | `COPAUS1C` PF5, `COPAUS2C` | the path `auth_id` ([`AuthIdPath`]); no body, PF5 acts on the record shown | [`FraudToggleResponse`] |
//!
//! The REST key of a detail is its row identity `auth_id`; the IMS key
//! `PA-AUTHORIZATION-KEY` stays in the row. Authorizations the CP00 consumer
//! writes are read through these resources.
//!
//! Stored fields are sent as stored: the `X(6)` original date `YYMMDD` and
//! time `HHMMSS` and the `X(4)` card expiry `MMYY`, which `POPULATE-AUTH-LIST`
//! and `POPULATE-AUTH-DETAILS` cut into `MM/DD/YY`, `HH:MM:SS` and `MM/YY`, are
//! formatted by the SPA; amounts are scale-2 decimal strings. The values the
//! programs compute rather than move are computed here: the approval flag
//! (`A`/`D`) and the decline-reason text.

use std::fmt;

use carddemo_domain::entities::{PendingAuthDetail, PendingAuthSummary};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_domain::numeric::{AuthCount, AuthId, CardNumber, Money};
use carddemo_domain::ports::Direction;
use carddemo_domain::rules::auth_detail::{self, DetailStep, FraudOutcome};
use carddemo_domain::rules::auth_summary::{AuthSummaryInput, AuthSummaryView};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{BmsField, bms_errors, id_string, money, reject_invalid, text};
use crate::error::ApiError;
use crate::pagination::{PageDirection, PageQuery, PageSize};

/// `WS-AUTH-APRV-STAT` and `AUTHRSPO` of an authorization whose
/// `PA-AUTH-RESP-CODE` is `00`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L536-540]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L311-317].
const APPROVED: &str = "A";

/// The approval flag of every other response code.
const DECLINED: &str = "D";

/// The text of a path that sets no message (V-COPAUS1C-001, E-49).
const NO_MESSAGE: &str = "";

/// The BMS field the errors of `GET /authorizations` name.
///
/// Every `COPAUS0C` message puts the cursor on the account ID,
/// `MOVE -1 TO ACCTIDL OF COPAU0AI` (V-COPAUS0C-001 to V-COPAUS0C-015).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AuthSummaryField {
    /// `ACCTIDI PIC X(11)` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU00.cpy:L60].
    #[serde(rename = "ACCTID")]
    AcctId,
}

impl AuthSummaryField {
    /// The BMS field name, the `fields[].field` value of the error body.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AcctId => "ACCTID",
        }
    }
}

impl BmsField for AuthSummaryField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The only field, so the only rank.
    fn rank(self) -> u16 {
        match self {
            Self::AcctId => 1,
        }
    }
}

/// The BMS field the errors of `GET` and `PUT /authorizations/{id}` name.
///
/// `SEND-AUTHVIEW-SCREEN` always puts the cursor on the card number,
/// `MOVE -1 TO CARDNUML` [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L373-378]
/// (V-COPAUS1C-001 to V-COPAUS1C-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum AuthDetailField {
    /// `CARDNUMI PIC X(16)` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU01.cpy:L60].
    #[serde(rename = "CARDNUM")]
    CardNum,
}

impl AuthDetailField {
    /// The BMS field name, the `fields[].field` value of the error body.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CardNum => "CARDNUM",
        }
    }
}

impl BmsField for AuthDetailField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The only field, so the only rank.
    fn rank(self) -> u16 {
        match self {
            Self::CardNum => 1,
        }
    }
}

/// COPAUS1C PROCESS-ENTER-KEY — V-COPAUS1C-001
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L208-227]: the path
/// `{id}` of `GET` and `PUT /authorizations/{id}` as the [`AuthId`] it names.
///
/// The COBOL reads nothing when the account is not numeric or no key was
/// selected, and sets its error flag with no message; E-49 maps that to an
/// identity that is not an integer. A non-positive integer parses here and
/// gets the same answer from `rules::auth_detail`.
///
/// # Errors
///
/// 400 with code `INVALID`, an empty message and the field `CARDNUM` when
/// `raw` is not a decimal `i64`.
pub fn parse_auth_id(raw: &str) -> Result<AuthId, ApiError> {
    raw.parse::<i64>().map(AuthId::from).map_err(|_| {
        ApiError::no_message(
            ErrorKind::Invalid,
            NO_MESSAGE,
            bms_errors([AuthDetailField::CardNum], NO_MESSAGE),
        )
    })
}

/// The path of `GET` and `PUT /authorizations/{id}`.
///
/// COPAUS1C PROCESS-ENTER-KEY — V-COPAUS1C-001 (E-49): the published pattern
/// admits exactly the text [`parse_auth_id`] reads as an `i64` and
/// `rules::auth_detail` then accepts as a positive identity. Any other text
/// is the COBOL path with no selected key: 400 with no message on `CARDNUM`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct AuthIdPath {
    /// The authorization's `auth_id`, as a summary page lists it: a positive
    /// integer up to 9223372036854775807, with an optional `+` and leading
    /// zeros.
    // `i64::from_str` text above zero: the 1- to 18-digit values, then each
    // 19-digit value up to `i64::MAX`, digit by digit.
    #[param(pattern = "^[+]?0*(?:[1-9][0-9]{0,17}|[1-8][0-9]{18}|9[01][0-9]{17}|\
                       92[01][0-9]{16}|922[0-2][0-9]{15}|9223[0-2][0-9]{14}|\
                       92233[0-6][0-9]{13}|922337[01][0-9]{12}|92233720[0-2][0-9]{10}|\
                       922337203[0-5][0-9]{9}|9223372036[0-7][0-9]{8}|\
                       92233720368[0-4][0-9]{7}|922337203685[0-3][0-9]{6}|\
                       9223372036854[0-6][0-9]{5}|92233720368547[0-6][0-9]{4}|\
                       922337203685477[0-4][0-9]{3}|9223372036854775[0-7][0-9]{2}|\
                       922337203685477580[0-7])$")]
    pub id: String,
}

/// The filter of `GET /authorizations`: the account whose authorizations are
/// listed, the `ACCTID` field of `COPAU0A`.
///
/// The page position is the separate [`AuthSummaryPageQuery`], which the
/// handler extracts beside this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct AuthSummaryQuery {
    /// The account ID as entered, 11 digits (`ACCTIDI PIC X(11)`). Sent to
    /// the rule unedited, so a blank or non-numeric value is answered with
    /// the `COPAUS0C` message.
    // The pattern is the class test `ACCTIDI IS NUMERIC` of
    // `rules::auth_summary::edit_account_id` (V-COPAUS0C-002); a value it
    // refuses still reaches the rule, which answers a blank one with
    // `Please enter Acct Id...` (V-COPAUS0C-001).
    #[serde(default)]
    #[param(max_length = 11, pattern = "^[0-9]{11}$")]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
}

impl AuthSummaryQuery {
    /// The BMS length of `ACCTID`, eleven characters.
    ///
    /// The screen field cannot hold a twelfth character; a longer value is
    /// "account ID not 11 digits", the class test of `PROCESS-ENTER-KEY`
    /// (V-COPAUS0C-002), which the rule would give it too.
    ///
    /// # Errors
    ///
    /// 400 with code `INVALID`, `Acct Id must be Numeric ...` and the field
    /// `ACCTID` when `account_id` is longer than eleven characters.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate().map_err(|errors| {
            reject_invalid(&errors, summary_field, messages::ACCT_ID_MUST_BE_NUMERIC)
        })
    }

    /// The rule input of the request: the account ID as entered, and the
    /// browse position `crate::pagination::PageQuery::resolve` gave.
    ///
    /// `start_key` is the `auth_id` of the last authorization shown (forward,
    /// PF8) or of the first one shown (backward, PF7), as a previous page's
    /// `next_key` or `prev_key` carried it. The COBOL keeps that position in
    /// its COMMAREA, where no operator can type it, so a key that is not an
    /// identity is a malformed request rather than a `COPAUS0C` edit.
    ///
    /// # Errors
    ///
    /// 400 with code `INVALID` and no fields when `start_key` is not a
    /// decimal `i64`.
    pub fn into_input(
        self,
        start_key: Option<String>,
        direction: Direction,
        page_size: u16,
    ) -> Result<AuthSummaryInput, ApiError> {
        let start_key = start_key.map(|raw| parse_start_key(&raw)).transpose()?;
        Ok(AuthSummaryInput {
            account_id: self.account_id,
            start_key,
            direction,
            page_size,
        })
    }
}

/// The BMS field of a validated `AuthSummaryQuery` field, by its JSON name.
fn summary_field(name: &str) -> Option<AuthSummaryField> {
    (name == "account_id").then_some(AuthSummaryField::AcctId)
}

/// The page position `start_key` as the [`AuthId`] it names.
fn parse_start_key(raw: &str) -> Result<AuthId, ApiError> {
    raw.parse::<i64>().map(AuthId::from).map_err(|error| {
        ApiError::rejection(format!(
            "Failed to deserialize query string: start_key: {error}"
        ))
    })
}

/// The page position of `GET /authorizations`: `start_key`, `direction` and
/// `page_size`, the parameters of the shared
/// [`PageQuery`](crate::pagination::PageQuery), into which the handler
/// converts it, so [`PageQuery::resolve`] applies the same defaults.
///
/// COPAUS0C PROCESS-PF7-KEY, PROCESS-PF8-KEY — P-COPAUS0C-010 to
/// P-COPAUS0C-012: the list declares its own query so that `start_key`
/// publishes the grammar of the text [`AuthSummaryQuery::into_input`]
/// accepts, a decimal `i64`; the other lists' keys have grammars of their
/// own. `direction` and `page_size` are published as the shared query
/// publishes them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuthSummaryPageQuery {
    /// The `auth_id` the page continues from, a previous page's `next_key` or
    /// `prev_key`: forward (PF8) the last authorization shown, which the page
    /// skips; backward (PF7) the first one shown, before which the page ends.
    /// Absent, the page is the first one, the newest authorizations, forward
    /// (ENTER), and the last one, the oldest, backward. A decimal integer
    /// from -9223372036854775808 to 9223372036854775807, with an optional
    /// sign and leading zeros.
    // `i64::from_str` text: zero, the 1- to 18-digit values, each 19-digit
    // value up to `i64::MAX` digit by digit, all signed either way, and
    // `i64::MIN`.
    #[param(
        pattern = "^(?:[+-]?0*(?:0|[1-9][0-9]{0,17}|[1-8][0-9]{18}|9[01][0-9]{17}|\
                   92[01][0-9]{16}|922[0-2][0-9]{15}|9223[0-2][0-9]{14}|\
                   92233[0-6][0-9]{13}|922337[01][0-9]{12}|92233720[0-2][0-9]{10}|\
                   922337203[0-5][0-9]{9}|9223372036[0-7][0-9]{8}|\
                   92233720368[0-4][0-9]{7}|922337203685[0-3][0-9]{6}|\
                   9223372036854[0-6][0-9]{5}|92233720368547[0-6][0-9]{4}|\
                   922337203685477[0-4][0-9]{3}|9223372036854775[0-7][0-9]{2}|\
                   922337203685477580[0-7])|-0*9223372036854775808)$"
    )]
    pub start_key: Option<String>,
    /// `forward` (next page, the default) or `backward` (previous page).
    // Inlined: query parameters register no component schema, so a reference
    // to `PageDirection` would dangle in the OpenAPI document.
    #[param(inline)]
    pub direction: Option<PageDirection>,
    /// Rows on the page, from 1 to 100. Defaults to the 5 rows of the
    /// `COPAU00` list.
    #[param(value_type = Option<u16>, minimum = 1, maximum = 100)]
    pub page_size: Option<PageSize>,
}

impl From<AuthSummaryPageQuery> for PageQuery {
    fn from(query: AuthSummaryPageQuery) -> Self {
        Self {
            start_key: query.start_key,
            direction: query.direction,
            page_size: query.page_size,
        }
    }
}

/// `WS-AUTH-APRV-STAT`: `A` when `PA-AUTH-RESP-CODE = '00'`, else `D`.
fn approval(detail: &PendingAuthDetail) -> String {
    if detail.is_approved() {
        APPROVED
    } else {
        DECLINED
    }
    .to_owned()
}

/// One line of the `COPAU0A` list: `POPULATE-AUTH-LIST`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L522-560]
/// (P-COPAUS0C-013).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AuthSummaryItem {
    /// The row identity: the `{id}` of the detail and the `start_key` of a
    /// page that continues from this line.
    pub auth_id: String,
    /// `TRNID0nI`: `PA-TRANSACTION-ID PIC X(15)`.
    pub pa_transaction_id: String,
    /// `PDATE0nI`: `PA-AUTH-ORIG-DATE PIC X(06)` as stored, `YYMMDD`.
    pub pa_auth_orig_date: String,
    /// `PTIME0nI`: `PA-AUTH-ORIG-TIME PIC X(06)` as stored, `HHMMSS`.
    pub pa_auth_orig_time: String,
    /// `PTYPE0nI`: `PA-AUTH-TYPE PIC X(04)`.
    pub pa_auth_type: String,
    /// `PAPRV0nI`: `A` when `PA-AUTH-RESP-CODE` is `00`, else `D`.
    pub approved: String,
    /// `PSTAT0nI`: `PA-MATCH-STATUS PIC X(01)`.
    pub pa_match_status: String,
    /// `PAMT00nI`: `PA-APPROVED-AMT PIC S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub pa_approved_amt: Decimal,
}

impl From<&PendingAuthDetail> for AuthSummaryItem {
    fn from(detail: &PendingAuthDetail) -> Self {
        Self {
            auth_id: id_string(&detail.auth_id),
            pa_transaction_id: text(&detail.pa_transaction_id),
            pa_auth_orig_date: text(&detail.pa_auth_orig_date),
            pa_auth_orig_time: text(&detail.pa_auth_orig_time),
            pa_auth_type: text(&detail.pa_auth_type),
            approved: approval(detail),
            pa_match_status: text(&detail.pa_match_status.to_alnum()),
            pa_approved_amt: money(detail.pa_approved_amt),
        }
    }
}

/// The `COPAU0A` screen: the header `GATHER-ACCOUNT-DETAILS` fills
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L750-805] and one page
/// of authorizations, with the paging contract of `crate::pagination::Page`.
///
/// The header carries the fields the COBOL builds its screen text from:
/// `CNAMEO`, `ADDR001O` and `ADDR002O` are `STRING`s of the customer name and
/// address fields, which the SPA composes. `ACCSTAT` is not sent: no
/// `COPAUS0C` statement moves a value to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AuthSummaryResponse {
    /// `ACCTID`: `ACCT-ID 9(11)`, zero-padded.
    pub account_id: String,
    /// `CUSTIDO`: `CUST-ID 9(09)`, zero-padded.
    pub customer_id: String,
    /// `CUST-FIRST-NAME`, the first word of `CNAMEO`.
    pub cust_first_name: String,
    /// `CUST-MIDDLE-NAME`, whose first letter `CNAMEO` shows.
    pub cust_middle_name: String,
    /// `CUST-LAST-NAME`, the last word of `CNAMEO`.
    pub cust_last_name: String,
    /// `CUST-ADDR-LINE-1`, the start of `ADDR001O`.
    pub cust_addr_line_1: String,
    /// `CUST-ADDR-LINE-2`, the end of `ADDR001O`.
    pub cust_addr_line_2: String,
    /// `CUST-ADDR-LINE-3`, the start of `ADDR002O`.
    pub cust_addr_line_3: String,
    /// `CUST-ADDR-STATE-CD`, in `ADDR002O`.
    pub cust_addr_state_cd: String,
    /// `CUST-ADDR-ZIP`, whose first five characters `ADDR002O` shows.
    pub cust_addr_zip: String,
    /// `PHONE1O`: `CUST-PHONE-NUM-1`.
    pub cust_phone_num_1: String,
    /// `CREDLIMO`: `ACCT-CREDIT-LIMIT PIC S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_credit_limit: Decimal,
    /// `CASHLIMO`: `ACCT-CASH-CREDIT-LIMIT PIC S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub acct_cash_credit_limit: Decimal,
    /// `APPRCNTO`: `PA-APPROVED-AUTH-CNT PIC S9(04) COMP`; 0 when the
    /// account has no summary segment.
    pub pa_approved_auth_cnt: i16,
    /// `DECLCNTO`: `PA-DECLINED-AUTH-CNT PIC S9(04) COMP`; 0 without a
    /// summary.
    pub pa_declined_auth_cnt: i16,
    /// `CREDBALO`: `PA-CREDIT-BALANCE PIC S9(09)V99`; `0.00` without a
    /// summary.
    #[serde(with = "rust_decimal::serde::str")]
    pub pa_credit_balance: Decimal,
    /// `CASHBALO`: `PA-CASH-BALANCE PIC S9(09)V99`; `0.00` without a summary.
    #[serde(with = "rust_decimal::serde::str")]
    pub pa_cash_balance: Decimal,
    /// `APPRAMTO`: `PA-APPROVED-AUTH-AMT PIC S9(09)V99`; `0.00` without a
    /// summary.
    #[serde(with = "rust_decimal::serde::str")]
    pub pa_approved_auth_amt: Decimal,
    /// `DECLAMTO`: `PA-DECLINED-AUTH-AMT PIC S9(09)V99`; `0.00` without a
    /// summary.
    #[serde(with = "rust_decimal::serde::str")]
    pub pa_declined_auth_amt: Decimal,
    /// The authorizations of the page, newest first (P-COPAUS0C-010). Empty
    /// when the account has no summary or the request browsed nothing.
    pub items: Vec<AuthSummaryItem>,
    /// The `start_key` of the next page, read with `direction=forward`; `null`
    /// when no page follows.
    // Required although nullable, as `crate::pagination::Page` declares it:
    // the key is always serialized.
    #[schema(required = true)]
    pub next_key: Option<String>,
    /// The `start_key` of the previous page, read with `direction=backward`;
    /// `null` when no page precedes it.
    #[schema(required = true)]
    pub prev_key: Option<String>,
    /// The page-boundary text of a request that browsed nothing,
    /// `You are already at the top of the page...` or
    /// `You are already at the bottom of the page...` (V-COPAUS0C-013,
    /// V-COPAUS0C-014). Omitted otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl AuthSummaryResponse {
    /// COPAUS0C GATHER-ACCOUNT-DETAILS, POPULATE-AUTH-LIST — P-COPAUS0C-006,
    /// P-COPAUS0C-009, P-COPAUS0C-013: the screen of `view`.
    ///
    /// Without a `PAUTSUM0` segment the counts and amounts are zero, as
    /// `MOVE ZERO TO APPRCNTO … DECLAMTO`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L797-803] shows them.
    #[must_use]
    pub fn from_view(view: &AuthSummaryView) -> Self {
        let segment = view.summary.as_ref();
        let count = |field: fn(&PendingAuthSummary) -> AuthCount| {
            segment.map_or(AuthCount::ZERO, field).get()
        };
        let amount =
            |field: fn(&PendingAuthSummary) -> Money<9>| money(segment.map_or(Money::ZERO, field));
        let customer = &view.customer;
        Self {
            account_id: id_string(&view.account.acct_id),
            customer_id: id_string(&customer.cust_id),
            cust_first_name: text(&customer.cust_first_name),
            cust_middle_name: text(&customer.cust_middle_name),
            cust_last_name: text(&customer.cust_last_name),
            cust_addr_line_1: text(&customer.cust_addr_line_1),
            cust_addr_line_2: text(&customer.cust_addr_line_2),
            cust_addr_line_3: text(&customer.cust_addr_line_3),
            cust_addr_state_cd: text(&customer.cust_addr_state_cd),
            cust_addr_zip: text(&customer.cust_addr_zip),
            cust_phone_num_1: text(&customer.cust_phone_num_1),
            acct_credit_limit: money(view.account.acct_credit_limit),
            acct_cash_credit_limit: money(view.account.acct_cash_credit_limit),
            pa_approved_auth_cnt: count(|segment| segment.pa_approved_auth_cnt),
            pa_declined_auth_cnt: count(|segment| segment.pa_declined_auth_cnt),
            pa_credit_balance: amount(|segment| segment.pa_credit_balance),
            pa_cash_balance: amount(|segment| segment.pa_cash_balance),
            pa_approved_auth_amt: amount(|segment| segment.pa_approved_auth_amt),
            pa_declined_auth_amt: amount(|segment| segment.pa_declined_auth_amt),
            items: view.details.iter().map(AuthSummaryItem::from).collect(),
            next_key: view.next_key.as_ref().map(id_string),
            prev_key: view.prev_key.as_ref().map(id_string),
            message: view.message.map(str::to_owned),
        }
    }
}

/// The `COPAU1A` screen: one pending authorization as
/// `POPULATE-AUTH-DETAILS` shows it
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L290-357]
/// (P-COPAUS1C-005).
#[derive(Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AuthDetailResponse {
    /// The row identity, the `{id}` of this resource.
    pub auth_id: String,
    /// `PA-ACCT-ID`, the account of the summary the detail sits under,
    /// zero-padded.
    pub account_id: String,
    /// `CARDNUMO`: `PA-CARD-NUM PIC X(16)`.
    pub card_number: String,
    /// `AUTHDTO`: `PA-AUTH-ORIG-DATE PIC X(06)` as stored, `YYMMDD`.
    pub pa_auth_orig_date: String,
    /// `AUTHTMO`: `PA-AUTH-ORIG-TIME PIC X(06)` as stored, `HHMMSS`.
    pub pa_auth_orig_time: String,
    /// `AUTHRSPO`: `A` when `PA-AUTH-RESP-CODE` is `00`, else `D`.
    pub approved: String,
    /// `PA-AUTH-RESP-CODE PIC X(02)`.
    pub pa_auth_resp_code: String,
    /// `PA-AUTH-RESP-REASON PIC X(04)`.
    pub pa_auth_resp_reason: String,
    /// `AUTHRSNO`: the reason code, `-` and its `WS-DECLINE-REASON-TABLE`
    /// text, such as `4100-INSUFFICNT FUND`, or `9999-ERROR` for a code the
    /// table lacks.
    pub decline_reason: String,
    /// `AUTHCDO`: `PA-PROCESSING-CODE PIC 9(06)`, zero-padded.
    pub pa_processing_code: String,
    /// `AUTHAMTO`: `PA-APPROVED-AMT PIC S9(10)V99`.
    #[serde(with = "rust_decimal::serde::str")]
    pub pa_approved_amt: Decimal,
    /// `POSEMDO`: `PA-POS-ENTRY-MODE PIC 9(02)`, zero-padded.
    pub pa_pos_entry_mode: String,
    /// `AUTHSRCO`: `PA-MESSAGE-SOURCE PIC X(06)`.
    pub pa_message_source: String,
    /// `MCCCDO`: `PA-MERCHANT-CATAGORY-CODE PIC X(04)`, spelled as the
    /// copybook spells it.
    pub pa_merchant_catagory_code: String,
    /// `CRDEXPO`: `PA-CARD-EXPIRY-DATE PIC X(04)` as stored, `MMYY`.
    pub pa_card_expiry_date: String,
    /// `AUTHTYPO`: `PA-AUTH-TYPE PIC X(04)`.
    pub pa_auth_type: String,
    /// `TRNIDO`: `PA-TRANSACTION-ID PIC X(15)`.
    pub pa_transaction_id: String,
    /// `AUTHMTCO`: `PA-MATCH-STATUS PIC X(01)`.
    pub pa_match_status: String,
    /// `PA-AUTH-FRAUD PIC X(01)`: `F` reported, `R` removed, empty when
    /// never marked. `AUTHFRDO` shows it with the report date for `F` and
    /// `R`, and `-` otherwise.
    pub pa_auth_fraud: String,
    /// `PA-FRAUD-RPT-DATE PIC X(08)`, `mm/dd/yy` once fraud was marked.
    pub pa_fraud_rpt_date: String,
    /// `MERNAMEO`: `PA-MERCHANT-NAME PIC X(22)`.
    pub pa_merchant_name: String,
    /// `MERIDO`: `PA-MERCHANT-ID PIC X(15)`.
    pub pa_merchant_id: String,
    /// `MERCITYO`: `PA-MERCHANT-CITY PIC X(13)`.
    pub pa_merchant_city: String,
    /// `MERSTO`: `PA-MERCHANT-STATE PIC X(02)`.
    pub pa_merchant_state: String,
    /// `MERZIPO`: `PA-MERCHANT-ZIP PIC X(09)`.
    pub pa_merchant_zip: String,
    /// The `auth_id` of the next authorization under the same summary, the
    /// one PF8 shows (P-COPAUS1C-014); `null` when none follows or none was
    /// read.
    #[schema(required = true)]
    pub next_id: Option<String>,
    /// The text `COPAUS1C` shows with the record, such as
    /// `Already at the last Authorization...` (V-COPAUS1C-012). Omitted when
    /// it shows none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl AuthDetailResponse {
    /// COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: the screen of
    /// `detail`, with the PF8 neighbour `next_id` and the text `message`.
    #[must_use]
    pub fn from_detail(
        detail: &PendingAuthDetail,
        next_id: Option<AuthId>,
        message: Option<String>,
    ) -> Self {
        Self {
            auth_id: id_string(&detail.auth_id),
            account_id: id_string(&detail.pa_acct_id),
            card_number: id_string(&detail.pa_card_num),
            pa_auth_orig_date: text(&detail.pa_auth_orig_date),
            pa_auth_orig_time: text(&detail.pa_auth_orig_time),
            approved: approval(detail),
            pa_auth_resp_code: text(&detail.pa_auth_resp_code),
            pa_auth_resp_reason: text(&detail.pa_auth_resp_reason),
            decline_reason: text(&auth_detail::decline_reason(detail)),
            pa_processing_code: id_string(&detail.pa_processing_code),
            pa_approved_amt: money(detail.pa_approved_amt),
            pa_pos_entry_mode: id_string(&detail.pa_pos_entry_mode),
            pa_message_source: text(&detail.pa_message_source),
            pa_merchant_catagory_code: text(&detail.pa_merchant_catagory_code),
            pa_card_expiry_date: text(&detail.pa_card_expiry_date),
            pa_auth_type: text(&detail.pa_auth_type),
            pa_transaction_id: text(&detail.pa_transaction_id),
            pa_match_status: text(&detail.pa_match_status.to_alnum()),
            pa_auth_fraud: text(&detail.pa_auth_fraud.to_alnum()),
            pa_fraud_rpt_date: text(&detail.pa_fraud_rpt_date),
            pa_merchant_name: text(&detail.pa_merchant_name),
            pa_merchant_id: text(&detail.pa_merchant_id),
            pa_merchant_city: text(&detail.pa_merchant_city),
            pa_merchant_state: text(&detail.pa_merchant_state),
            pa_merchant_zip: text(&detail.pa_merchant_zip),
            next_id: next_id.as_ref().map(id_string),
            message,
        }
    }

    /// COPAUS1C PROCESS-PF8-KEY — P-COPAUS1C-014, V-COPAUS1C-012: the
    /// screen of `detail` with the outcome of the PF8 read after it, the next
    /// authorization's `auth_id` or the last-authorization text.
    #[must_use]
    pub fn from_step(detail: &PendingAuthDetail, step: &DetailStep) -> Self {
        match step {
            DetailStep::Next(next) => Self::from_detail(detail, Some(next.auth_id), None),
            DetailStep::AtEnd { message } => {
                Self::from_detail(detail, None, Some((*message).to_owned()))
            }
        }
    }
}

/// The card number is shown masked, as `CardNumber`'s `Debug` shows it, so
/// diagnostics never carry a full card number.
impl fmt::Debug for AuthDetailResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthDetailResponse")
            .field("auth_id", &self.auth_id)
            .field("account_id", &self.account_id)
            .field("card_number", &CardNumber::move_from(&self.card_number))
            .field("pa_auth_orig_date", &self.pa_auth_orig_date)
            .field("pa_auth_orig_time", &self.pa_auth_orig_time)
            .field("approved", &self.approved)
            .field("pa_auth_resp_code", &self.pa_auth_resp_code)
            .field("pa_auth_resp_reason", &self.pa_auth_resp_reason)
            .field("decline_reason", &self.decline_reason)
            .field("pa_processing_code", &self.pa_processing_code)
            .field("pa_approved_amt", &self.pa_approved_amt)
            .field("pa_pos_entry_mode", &self.pa_pos_entry_mode)
            .field("pa_message_source", &self.pa_message_source)
            .field("pa_merchant_catagory_code", &self.pa_merchant_catagory_code)
            .field("pa_card_expiry_date", &self.pa_card_expiry_date)
            .field("pa_auth_type", &self.pa_auth_type)
            .field("pa_transaction_id", &self.pa_transaction_id)
            .field("pa_match_status", &self.pa_match_status)
            .field("pa_auth_fraud", &self.pa_auth_fraud)
            .field("pa_fraud_rpt_date", &self.pa_fraud_rpt_date)
            .field("pa_merchant_name", &self.pa_merchant_name)
            .field("pa_merchant_id", &self.pa_merchant_id)
            .field("pa_merchant_city", &self.pa_merchant_city)
            .field("pa_merchant_state", &self.pa_merchant_state)
            .field("pa_merchant_zip", &self.pa_merchant_zip)
            .field("next_id", &self.next_id)
            .field("message", &self.message)
            .finish()
    }
}

/// The answer to PF5: the detail as `UPDATE-AUTH-DETAILS` replaced it and
/// the text `MARK-AUTH-FRAUD` shows
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L230-266]
/// (P-COPAUS1C-006 to P-COPAUS1C-008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FraudToggleResponse {
    /// The replaced detail: the toggled `PA-AUTH-FRAUD` and the
    /// `PA-FRAUD-RPT-DATE` `COPAUS2C` stamped.
    pub detail: AuthDetailResponse,
    /// `AUTH MARKED FRAUD...` when the flag became `F`,
    /// `AUTH FRAUD REMOVED...` when it became `R`.
    pub message: String,
}

impl From<&FraudOutcome> for FraudToggleResponse {
    fn from(outcome: &FraudOutcome) -> Self {
        Self {
            detail: AuthDetailResponse::from_detail(&outcome.detail, None, None),
            message: outcome.message.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::entities::{
        Account, CardXref, Customer, FraudFlag, MatchStatus, NewPendingAuthDetail,
        PA_ACCOUNT_STATUS_OCCURS, PendingAuthDetail, PendingAuthSummary,
    };
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, Alnum, AuthCount, AuthId, CardNumber, CustSsn, CustomerId,
        FicoScore, Money, NumericCode,
    };
    use carddemo_domain::ports::Direction;
    use carddemo_domain::rules::auth_detail::{self, DetailStep, FraudOutcome};
    use carddemo_domain::rules::auth_summary::{self, AuthSummaryView};
    use serde_json::{Value, json};
    use utoipa::{IntoParams, PartialSchema};

    use super::{
        AuthDetailField, AuthDetailResponse, AuthIdPath, AuthSummaryField, AuthSummaryItem,
        AuthSummaryPageQuery, AuthSummaryQuery, AuthSummaryResponse, FraudToggleResponse,
        parse_auth_id,
    };
    use crate::dto::BmsField;
    use crate::error::ApiError;
    use crate::pagination::PageQuery;

    const CARD: &str = "4111000000000001";

    /// The alternatives that together match the decimal values with as many
    /// digits as `max`, from the lowest such value to `max`: for each digit of
    /// `max`, the digits before it, a lower digit in its place (at most that
    /// digit in the last place), and any digits after it.
    fn same_width_up_to(max: &str) -> Vec<String> {
        let digits = max.as_bytes();
        let last = digits.len() - 1;
        let mut alternatives = Vec::new();
        for (place, &digit) in digits.iter().enumerate() {
            let low = if place == 0 { b'1' } else { b'0' };
            let high = if place == last { digit } else { digit - 1 };
            if high < low {
                continue;
            }
            let class = match high - low {
                0 => char::from(low).to_string(),
                1 => format!("[{}{}]", char::from(low), char::from(high)),
                _ => format!("[{}-{}]", char::from(low), char::from(high)),
            };
            let rest = last - place;
            let tail = if rest == 0 {
                String::new()
            } else {
                format!("[0-9]{{{rest}}}")
            };
            alternatives.push(format!("{}{class}{tail}", &max[..place]));
        }
        alternatives
    }

    /// The magnitudes from 1 to `i64::MAX` without leading zeros: the values
    /// of up to 18 digits, then the 19-digit ones up to `i64::MAX`.
    fn positive_i64_magnitude() -> String {
        let max = i64::MAX.to_string();
        let shorter = max.len() - 2;
        format!(
            "[1-9][0-9]{{0,{shorter}}}|{}",
            same_width_up_to(&max).join("|")
        )
    }

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn customer_id(value: u32) -> CustomerId {
        CustomerId::new(value).expect("a customer number of at most 9 digits")
    }

    fn cents<const D: u8>(value: i64) -> Money<D> {
        Money::from_cents(value).expect("an amount within the field")
    }

    fn account() -> Account {
        Account {
            acct_id: account_id(1),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(19_400),
            acct_credit_limit: cents(2_020_000),
            acct_cash_credit_limit: cents(1_020_000),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn customer() -> Customer {
        Customer {
            cust_id: customer_id(7),
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
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
    }

    fn pending_summary() -> PendingAuthSummary {
        PendingAuthSummary {
            pa_acct_id: account_id(1),
            pa_cust_id: customer_id(7),
            pa_auth_status: Alnum::move_from("A"),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(2_020_000),
            pa_cash_limit: cents(1_020_000),
            pa_credit_balance: cents(12_000),
            pa_cash_balance: cents(-5),
            pa_approved_auth_cnt: AuthCount::new(6).expect("a count"),
            pa_declined_auth_cnt: AuthCount::new(1).expect("a count"),
            pa_approved_auth_amt: cents(12_000),
            pa_declined_auth_amt: cents(75_000),
        }
    }

    /// A stored detail of account 1 with response code `resp_code`.
    fn detail(auth_id: i64, resp_code: &str) -> PendingAuthDetail {
        PendingAuthDetail::from_new(
            AuthId::from(auth_id),
            NewPendingAuthDetail {
                pa_acct_id: account_id(1),
                pa_auth_date_9c: 77_817,
                pa_auth_time_9c: 879_999_999,
                pa_auth_orig_date: Alnum::move_from("220701"),
                pa_auth_orig_time: Alnum::move_from("120000"),
                pa_card_num: CardNumber::move_from(CARD),
                pa_auth_type: Alnum::move_from("SALE"),
                pa_card_expiry_date: Alnum::move_from("0225"),
                pa_message_type: Alnum::move_from("0100"),
                pa_message_source: Alnum::move_from("POS"),
                pa_auth_id_code: Alnum::move_from("123456"),
                pa_auth_resp_code: Alnum::move_from(resp_code),
                pa_auth_resp_reason: Alnum::move_from("0000"),
                pa_processing_code: NumericCode::new(30).expect("a six-digit code"),
                pa_transaction_amt: cents(2_100),
                pa_approved_amt: cents(2_000),
                pa_merchant_catagory_code: Alnum::move_from("5411"),
                pa_acqr_country_code: Alnum::move_from("840"),
                pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
                pa_merchant_id: Alnum::move_from("800000000"),
                pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
                pa_merchant_city: Alnum::move_from("Enoshaven"),
                pa_merchant_state: Alnum::move_from("NC"),
                pa_merchant_zip: Alnum::move_from("72112"),
                pa_transaction_id: Alnum::move_from("000000000000001"),
                pa_match_status: MatchStatus::Pending,
                pa_auth_fraud: FraudFlag::Blank,
                pa_fraud_rpt_date: Alnum::spaces(),
            },
        )
    }

    fn view(summary: Option<PendingAuthSummary>) -> AuthSummaryView {
        AuthSummaryView {
            account: account(),
            customer: customer(),
            card_xref: CardXref {
                xref_card_num: CardNumber::move_from(CARD),
                xref_cust_id: customer_id(7),
                xref_acct_id: account_id(1),
            },
            summary,
            details: Vec::new(),
            next_key: None,
            prev_key: None,
            message: None,
        }
    }

    fn to_json(value: &impl serde::Serialize) -> Value {
        serde_json::to_value(value).expect("serializes")
    }

    fn body(error: &ApiError) -> Value {
        to_json(error.body())
    }

    #[test]
    fn summary_item_marks_only_response_code_00_approved() {
        // COPAUS0C POPULATE-AUTH-LIST L536-540, P-COPAUS0C-013
        let cases = [
            ("00", "A"),
            ("05", "D"),
            ("01", "D"),
            ("0 ", "D"),
            (" 0", "D"),
            ("  ", "D"),
        ];
        for (resp_code, expected) in cases {
            let item = AuthSummaryItem::from(&detail(7, resp_code));
            assert_eq!(item.approved, expected, "{resp_code:?}");
        }
        assert_eq!(
            to_json(&AuthSummaryItem::from(&detail(7, "00"))),
            json!({
                "auth_id": "7",
                "pa_transaction_id": "000000000000001",
                "pa_auth_orig_date": "220701",
                "pa_auth_orig_time": "120000",
                "pa_auth_type": "SALE",
                "approved": "A",
                "pa_match_status": "P",
                "pa_approved_amt": "20.00",
            })
        );
    }

    #[test]
    fn summary_response_without_a_segment_shows_zero_counts_and_amounts() {
        // COPAUS0C GATHER-ACCOUNT-DETAILS L797-803, P-COPAUS0C-006, P-COPAUS0C-009
        let header = json!({
            "account_id": "00000000001",
            "customer_id": "000000007",
            "cust_first_name": "Immanuel",
            "cust_middle_name": "Madeline",
            "cust_last_name": "Kessler",
            "cust_addr_line_1": "618 Deshaun Route",
            "cust_addr_line_2": "Apt. 802",
            "cust_addr_line_3": "Altenwerthshire",
            "cust_addr_state_cd": "NC",
            "cust_addr_zip": "12546",
            "cust_phone_num_1": "(908)119-8310",
            "acct_credit_limit": "20200.00",
            "acct_cash_credit_limit": "10200.00",
            "items": [],
            "next_key": null,
            "prev_key": null,
        });
        let cases = [
            (None, json!([0, 0, "0.00", "0.00", "0.00", "0.00"])),
            (
                Some(pending_summary()),
                json!([6, 1, "120.00", "-0.05", "120.00", "750.00"]),
            ),
        ];
        for (segment, totals) in cases {
            let mut expected = header.clone();
            let names = [
                "pa_approved_auth_cnt",
                "pa_declined_auth_cnt",
                "pa_credit_balance",
                "pa_cash_balance",
                "pa_approved_auth_amt",
                "pa_declined_auth_amt",
            ];
            for (name, value) in names.into_iter().zip(totals.as_array().expect("an array")) {
                expected[name] = value.clone();
            }
            let response = AuthSummaryResponse::from_view(&view(segment));
            assert_eq!(to_json(&response), expected);
        }
    }

    #[test]
    fn summary_response_carries_the_page_rows_keys_and_boundary_text() {
        // COPAUS0C PROCESS-PAGE-FORWARD, PROCESS-PF8-KEY, PROCESS-PF7-KEY:
        // P-COPAUS0C-010 to P-COPAUS0C-012, V-COPAUS0C-013, V-COPAUS0C-014
        let mut page = view(Some(pending_summary()));
        page.details = vec![detail(2, "00"), detail(4, "05")];
        page.next_key = Some(AuthId::from(4));
        page.prev_key = Some(AuthId::from(2));
        let json = to_json(&AuthSummaryResponse::from_view(&page));
        let listed: Vec<(&str, &str)> = json["items"]
            .as_array()
            .expect("an array")
            .iter()
            .map(|item| {
                (
                    item["auth_id"].as_str().expect("a key"),
                    item["approved"].as_str().expect("a flag"),
                )
            })
            .collect();
        assert_eq!(listed, [("2", "A"), ("4", "D")]);
        assert_eq!(json["next_key"], "4");
        assert_eq!(json["prev_key"], "2");
        assert!(json.get("message").is_none());

        for text in [
            messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE,
            messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE,
        ] {
            let mut boundary = view(Some(pending_summary()));
            boundary.message = Some(text);
            let json = to_json(&AuthSummaryResponse::from_view(&boundary));
            assert_eq!(json["message"], text);
            assert_eq!(json["items"], json!([]));
            assert_eq!(json["next_key"], Value::Null);
        }
    }

    #[test]
    fn detail_response_maps_every_copau01_field() {
        // COPAUS1C POPULATE-AUTH-DETAILS L290-357, P-COPAUS1C-005
        let mut stored = detail(9, "05");
        stored.pa_auth_resp_reason = Alnum::move_from("4100");
        stored.pa_auth_fraud = FraudFlag::Confirmed;
        stored.pa_fraud_rpt_date = Alnum::move_from("07/01/22");
        assert_eq!(
            to_json(&AuthDetailResponse::from_detail(&stored, None, None)),
            json!({
                "auth_id": "9",
                "account_id": "00000000001",
                "card_number": CARD,
                "pa_auth_orig_date": "220701",
                "pa_auth_orig_time": "120000",
                "approved": "D",
                "pa_auth_resp_code": "05",
                "pa_auth_resp_reason": "4100",
                "decline_reason": "4100-INSUFFICNT FUND",
                "pa_processing_code": "000030",
                "pa_approved_amt": "20.00",
                "pa_pos_entry_mode": "01",
                "pa_message_source": "POS",
                "pa_merchant_catagory_code": "5411",
                "pa_card_expiry_date": "0225",
                "pa_auth_type": "SALE",
                "pa_transaction_id": "000000000000001",
                "pa_match_status": "P",
                "pa_auth_fraud": "F",
                "pa_fraud_rpt_date": "07/01/22",
                "pa_merchant_name": "Abshire-Lowe",
                "pa_merchant_id": "800000000",
                "pa_merchant_city": "Enoshaven",
                "pa_merchant_state": "NC",
                "pa_merchant_zip": "72112",
                "next_id": null,
            })
        );

        // The stored flag bytes, the space of a detail never marked included.
        let flags = [
            (FraudFlag::Blank, MatchStatus::Pending, "", "P"),
            (FraudFlag::Removed, MatchStatus::AuthDeclined, "R", "D"),
            (FraudFlag::Other(b'X'), MatchStatus::Other(b'Q'), "X", "Q"),
        ];
        for (fraud, status, fraud_text, status_text) in flags {
            let mut flagged = detail(9, "00");
            flagged.pa_auth_fraud = fraud;
            flagged.pa_match_status = status;
            let response = AuthDetailResponse::from_detail(&flagged, None, None);
            assert_eq!(response.pa_auth_fraud, fraud_text, "{fraud:?}");
            assert_eq!(response.pa_match_status, status_text, "{status:?}");
            assert_eq!(response.approved, "A");
            assert_eq!(response.decline_reason, "0000-APPROVED");
        }
    }

    #[test]
    fn detail_response_amounts_are_two_place_strings() {
        // PA-APPROVED-AMT PIC S9(10)V99, sent as a decimal string.
        let cases = [
            (2_000, "20.00"),
            (5, "0.05"),
            (-1_234, "-12.34"),
            (0, "0.00"),
            (999_999_999_999, "9999999999.99"),
        ];
        for (amount, expected) in cases {
            let mut stored = detail(1, "00");
            stored.pa_approved_amt = cents(amount);
            let detail_json = to_json(&AuthDetailResponse::from_detail(&stored, None, None));
            let item_json = to_json(&AuthSummaryItem::from(&stored));
            assert_eq!(detail_json["pa_approved_amt"], expected, "{amount}");
            assert_eq!(item_json["pa_approved_amt"], expected, "{amount}");
        }
    }

    #[test]
    fn from_step_gives_the_next_id_or_the_last_authorization_text() {
        // COPAUS1C PROCESS-PF8-KEY: P-COPAUS1C-014, V-COPAUS1C-012
        let current = detail(3, "00");
        let next =
            AuthDetailResponse::from_step(&current, &DetailStep::Next(Box::new(detail(8, "00"))));
        assert_eq!(next.next_id.as_deref(), Some("8"));
        assert_eq!(next.message, None);
        assert_eq!(next.auth_id, "3");

        let at_end = AuthDetailResponse::from_step(
            &current,
            &DetailStep::AtEnd {
                message: messages::ALREADY_AT_THE_LAST_AUTHORIZATION,
            },
        );
        assert_eq!(at_end.next_id, None);
        let json = to_json(&at_end);
        assert_eq!(json["message"], messages::ALREADY_AT_THE_LAST_AUTHORIZATION);
        assert_eq!(json["next_id"], Value::Null);
        assert_eq!(
            at_end,
            AuthDetailResponse::from_detail(
                &current,
                None,
                Some(messages::ALREADY_AT_THE_LAST_AUTHORIZATION.to_owned()),
            )
        );
    }

    #[test]
    fn fraud_toggle_response_carries_the_replaced_detail_and_the_pf5_text() {
        // COPAUS1C MARK-AUTH-FRAUD: P-COPAUS1C-006 to P-COPAUS1C-008
        let cases = [
            (FraudFlag::Confirmed, messages::AUTH_MARKED_FRAUD, "F"),
            (FraudFlag::Removed, messages::AUTH_FRAUD_REMOVED, "R"),
        ];
        for (flag, message, flag_text) in cases {
            let mut replaced = detail(5, "00");
            replaced.pa_auth_fraud = flag;
            replaced.pa_fraud_rpt_date = Alnum::move_from("07/01/22");
            let response = FraudToggleResponse::from(&FraudOutcome {
                detail: replaced.clone(),
                message,
            });
            assert_eq!(response.message, message);
            assert_eq!(
                response.detail,
                AuthDetailResponse::from_detail(&replaced, None, None)
            );
            let json = to_json(&response);
            assert_eq!(json["detail"]["pa_auth_fraud"], flag_text);
            assert_eq!(json["detail"]["pa_fraud_rpt_date"], "07/01/22");
            assert!(json["detail"].get("message").is_none());
        }
    }

    #[test]
    fn debug_masks_the_card_number_that_json_sends_in_full() {
        let response = AuthDetailResponse::from_detail(&detail(5, "00"), None, None);
        let toggle = FraudToggleResponse::from(&FraudOutcome {
            detail: detail(5, "00"),
            message: messages::AUTH_MARKED_FRAUD,
        });
        let cases = [
            (
                format!("{response:?}"),
                to_json(&response)["card_number"].clone(),
            ),
            (
                format!("{toggle:?}"),
                to_json(&toggle)["detail"]["card_number"].clone(),
            ),
        ];
        for (shown, sent) in cases {
            assert!(!shown.contains(CARD), "{shown}");
            assert!(shown.contains("************0001"), "{shown}");
            assert_eq!(sent, CARD);
        }
    }

    #[test]
    fn an_account_id_over_eleven_characters_fails_on_acctid() {
        // ACCTIDI PIC X(11); V-COPAUS0C-002
        let rejected = AuthSummaryQuery {
            account_id: Some("000000000012".to_owned()),
        };
        let error = rejected.check().expect_err("twelve characters");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body(&error),
            json!({
                "code": ErrorKind::Invalid.code(),
                "message": messages::ACCT_ID_MUST_BE_NUMERIC,
                "fields": [{"field": "ACCTID", "message": messages::ACCT_ID_MUST_BE_NUMERIC}],
            })
        );
        // The rule gives the same answer to the same text.
        let rule_error =
            auth_summary::edit_account_id(Some("000000000012")).expect_err("not 11 digits");
        assert_eq!(body(&error), body(&ApiError::from(rule_error)));

        // Within the field length the rule edits the value, blank included.
        for accepted in [None, Some(""), Some("00000000001"), Some("0000000000A")] {
            let query = AuthSummaryQuery {
                account_id: accepted.map(str::to_owned),
            };
            assert!(query.check().is_ok(), "{accepted:?}");
        }
    }

    #[test]
    fn the_account_id_pattern_admits_exactly_what_edit_account_id_accepts() {
        // ACCTIDI IS NUMERIC on X(11): V-COPAUS0C-001, V-COPAUS0C-002.
        for accepted in ["00000000001", "99999999999", "00000000000"] {
            let query = AuthSummaryQuery {
                account_id: Some(accepted.to_owned()),
            };
            assert!(query.check().is_ok(), "{accepted:?}");
            assert!(
                auth_summary::edit_account_id(Some(accepted)).is_ok(),
                "{accepted:?}"
            );
        }
        // Outside the pattern: the check or the rule answers 400 on ACCTID.
        let rejected = [
            ("", messages::PLEASE_ENTER_ACCT_ID),
            ("           ", messages::PLEASE_ENTER_ACCT_ID),
            ("\0\0\0", messages::PLEASE_ENTER_ACCT_ID),
            ("0000000001", messages::ACCT_ID_MUST_BE_NUMERIC),
            ("0000000000A", messages::ACCT_ID_MUST_BE_NUMERIC),
            ("+0000000001", messages::ACCT_ID_MUST_BE_NUMERIC),
            (" 0000000001", messages::ACCT_ID_MUST_BE_NUMERIC),
            ("000000000012", messages::ACCT_ID_MUST_BE_NUMERIC),
        ];
        for (text, message) in rejected {
            let query = AuthSummaryQuery {
                account_id: Some(text.to_owned()),
            };
            let error = query
                .check()
                .and_then(|()| auth_summary::edit_account_id(Some(text)).map_err(ApiError::from))
                .expect_err("outside the pattern");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{text:?}");
            let json = body(&error);
            assert_eq!(json["message"], message, "{text:?}");
            assert_eq!(json["fields"][0]["field"], "ACCTID", "{text:?}");
        }
    }

    #[test]
    fn into_input_passes_the_account_raw_and_parses_the_start_key() {
        // Every form the `start_key` pattern admits: a sign, leading zeros,
        // and both ends of the i64 range.
        let cases: [(Option<&str>, Option<i64>); 11] = [
            (None, None),
            (Some("42"), Some(42)),
            (Some("9007199254740993"), Some(9_007_199_254_740_993)),
            (Some("-1"), Some(-1)),
            (Some("0"), Some(0)),
            (Some("-0"), Some(0)),
            (Some("+0042"), Some(42)),
            (Some("9223372036854775807"), Some(i64::MAX)),
            (Some("+09223372036854775807"), Some(i64::MAX)),
            (Some("-9223372036854775808"), Some(i64::MIN)),
            (Some("-0009223372036854775808"), Some(i64::MIN)),
        ];
        for (start_key, expected) in cases {
            for direction in [Direction::Forward, Direction::Backward] {
                let query = AuthSummaryQuery {
                    account_id: Some(" 1".to_owned()),
                };
                let input = query
                    .into_input(start_key.map(str::to_owned), direction, 5)
                    .expect("a key that is an identity");
                assert_eq!(input.account_id.as_deref(), Some(" 1"));
                assert_eq!(input.start_key, expected.map(AuthId::from), "{start_key:?}");
                assert_eq!(input.direction, direction);
                assert_eq!(input.page_size, 5);
            }
        }
    }

    #[test]
    fn a_start_key_that_is_not_an_identity_is_a_400() {
        // Each lies outside the `start_key` pattern.
        let rejected = [
            "abc",
            "",
            "1.5",
            " 1",
            "1 ",
            "+",
            "-",
            "+-1",
            "99999999999999999999",
            "9223372036854775808",
            "-9223372036854775809",
        ];
        for start_key in rejected {
            let error = AuthSummaryQuery::default()
                .into_input(Some(start_key.to_owned()), Direction::Forward, 5)
                .expect_err("not an identity");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{start_key:?}");
            let json = body(&error);
            assert_eq!(json["code"], ErrorKind::Invalid.code(), "{start_key:?}");
            assert!(json.get("fields").is_none(), "{start_key:?}");
            let message = json["message"].as_str().expect("a message");
            assert!(message.contains("start_key"), "{message}");
        }
    }

    #[test]
    fn the_query_deserializes_from_the_query_string() {
        let cases = [
            ("/authorizations", None),
            ("/authorizations?account_id=", Some("")),
            (
                "/authorizations?account_id=00000000001&start_key=3&direction=backward&page_size=1",
                Some("00000000001"),
            ),
        ];
        for (uri, expected) in cases {
            let uri: Uri = uri.parse().expect("a valid URI");
            let Query(query) = Query::<AuthSummaryQuery>::try_from_uri(&uri).expect("parses");
            assert_eq!(query.account_id.as_deref(), expected, "{uri}");
        }
    }

    #[test]
    fn the_query_parameter_declares_the_bms_length() {
        let params = AuthSummaryQuery::into_params(|| None);
        let json = to_json(&params);
        assert_eq!(json[0]["name"], "account_id");
        assert_eq!(json[0]["in"], "query");
        assert_eq!(json[0]["required"], false);
        assert_eq!(json[0]["schema"]["maxLength"], 11);
        assert_eq!(json[0]["schema"]["pattern"], "^[0-9]{11}$");
    }

    #[test]
    fn the_path_and_page_parameters_publish_the_identity_grammars() {
        let magnitude = positive_i64_magnitude();
        // V-COPAUS1C-001 (E-49): a positive i64, as `parse_auth_id` and the
        // rule's positive edit take it.
        let path = to_json(&AuthIdPath::into_params(|| None));
        assert_eq!(path.as_array().map(Vec::len), Some(1), "{path}");
        assert_eq!(path[0]["name"], "id");
        assert_eq!(path[0]["in"], "path");
        assert_eq!(path[0]["required"], true);
        assert_eq!(path[0]["schema"]["type"], "string");
        assert_eq!(
            path[0]["schema"]["pattern"],
            format!("^[+]?0*(?:{magnitude})$")
        );
        // Leading zeros make the text unbounded, so no maximum length holds.
        assert!(path[0]["schema"].get("maxLength").is_none(), "{path}");

        // Any i64, as `AuthSummaryQuery::into_input` parses the page key.
        let page = to_json(&AuthSummaryPageQuery::into_params(|| None));
        let names: Vec<&str> = page
            .as_array()
            .expect("an array")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, ["start_key", "direction", "page_size"]);
        let min = i64::MIN.unsigned_abs();
        assert_eq!(
            page[0]["schema"]["pattern"],
            format!("^(?:[+-]?0*(?:0|{magnitude})|-0*{min})$")
        );
        // `direction` and `page_size` are the shared query's schemas.
        let shared = to_json(&PageQuery::into_params(|| None));
        for (index, param) in page.as_array().expect("an array").iter().enumerate() {
            assert_eq!(param["in"], "query", "{param}");
            assert_eq!(param["required"], false, "{param}");
            if index > 0 {
                assert_eq!(param["schema"], shared[index]["schema"], "{param}");
            }
        }
        assert!(!page.to_string().contains("$ref"), "{page}");
    }

    #[test]
    fn the_page_query_converts_to_the_shared_query_unchanged() {
        // (query text, start key, direction, rows) after `resolve` with the
        // five COPAU00 rows.
        let cases = [
            ("", None, Direction::Forward, auth_summary::SCREEN_ROWS),
            (
                "account_id=00000000001&start_key=3&direction=backward&page_size=1",
                Some("3"),
                Direction::Backward,
                1,
            ),
            (
                "start_key=-0042&page_size=100",
                Some("-0042"),
                Direction::Forward,
                100,
            ),
        ];
        for (text, start_key, direction, rows) in cases {
            let uri: Uri = format!("/authorizations?{text}")
                .parse()
                .expect("a valid URI");
            let Query(page) = Query::<AuthSummaryPageQuery>::try_from_uri(&uri).expect("parses");
            let Query(shared) = Query::<PageQuery>::try_from_uri(&uri).expect("parses");
            let converted = PageQuery::from(page);
            assert_eq!(converted, shared, "{text}");
            assert_eq!(
                converted.resolve(auth_summary::SCREEN_ROWS),
                (start_key.map(str::to_owned), direction, rows),
                "{text}"
            );
        }
        // Both refuse the same malformed parameters with the same text.
        for text in ["page_size=0", "page_size=101", "direction=sideways"] {
            let uri: Uri = format!("/authorizations?{text}")
                .parse()
                .expect("a valid URI");
            let own = Query::<AuthSummaryPageQuery>::try_from_uri(&uri).expect_err("rejected");
            let shared = Query::<PageQuery>::try_from_uri(&uri).expect_err("rejected");
            assert_eq!(own.body_text(), shared.body_text(), "{text}");
        }
    }

    #[test]
    fn the_response_schemas_send_keys_always_and_amounts_as_strings() {
        let summary = to_json(&AuthSummaryResponse::schema());
        let required = summary["required"].as_array().expect("required fields");
        for key in ["next_key", "prev_key", "items", "pa_credit_balance"] {
            assert!(required.contains(&json!(key)), "{key}");
        }
        assert!(!required.contains(&json!("message")));
        assert_eq!(summary["properties"]["pa_credit_balance"]["type"], "string");

        let detail = to_json(&AuthDetailResponse::schema());
        let required = detail["required"].as_array().expect("required fields");
        assert!(required.contains(&json!("next_id")));
        assert!(!required.contains(&json!("message")));
        assert_eq!(detail["properties"]["pa_approved_amt"]["type"], "string");
    }

    #[test]
    fn parse_auth_id_rejects_a_path_that_is_not_an_integer() {
        // COPAUS1C PROCESS-ENTER-KEY, V-COPAUS1C-001 (E-49)
        assert_eq!(parse_auth_id("12").expect("an integer"), AuthId::from(12));
        assert_eq!(parse_auth_id("0").expect("an integer"), AuthId::from(0));
        // Every form the path pattern admits reads as a positive identity,
        // which the rule's positive edit (`auth_id.get() <= 0`) passes.
        let admitted = [
            ("1", 1),
            ("+1", 1),
            ("0001", 1),
            ("+0009", 9),
            ("999999999999999999", 999_999_999_999_999_999),
            ("1000000000000000000", 1_000_000_000_000_000_000),
            ("9223372036854775807", i64::MAX),
            ("+09223372036854775807", i64::MAX),
        ];
        for (raw, expected) in admitted {
            let id = parse_auth_id(raw).expect("a positive identity");
            assert_eq!(id, AuthId::from(expected), "{raw:?}");
            assert!(id.get() > 0, "{raw:?}");
        }
        // Outside the pattern and still integers: zero and the negatives,
        // which the rule refuses before any read with the answer below.
        for raw in ["0", "+0", "000", "-0", "-1"] {
            let id = parse_auth_id(raw).expect("an integer");
            assert!(id.get() <= 0, "{raw:?}");
        }
        // The rule's answer to a non-positive identity, as the API sends it.
        let rule = body(&ApiError::from(
            DomainError::invalid("").with_field(auth_detail::CARD_NUM_FIELD),
        ));
        let not_integers = [
            "abc",
            "",
            "1e3",
            "12 ",
            " 12",
            "1.0",
            "+",
            "++1",
            "9223372036854775808",
            "10000000000000000000",
        ];
        for raw in not_integers {
            let error = parse_auth_id(raw).expect_err("not an integer");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{raw:?}");
            assert_eq!(body(&error), rule, "{raw:?}");
        }
        assert_eq!(
            rule,
            json!({
                "code": ErrorKind::Invalid.code(),
                "message": "",
                "fields": [{"field": "CARDNUM", "message": ""}],
            })
        );
    }

    #[test]
    fn the_field_enums_name_the_fields_the_rules_set() {
        let rule_field = auth_summary::edit_account_id(None)
            .expect_err("blank")
            .first_field();
        assert_eq!(rule_field, Some(AuthSummaryField::AcctId.as_str()));
        assert_eq!(
            AuthDetailField::CardNum.as_str(),
            auth_detail::CARD_NUM_FIELD
        );

        assert_eq!(BmsField::as_str(AuthSummaryField::AcctId), "ACCTID");
        assert_eq!(BmsField::as_str(AuthDetailField::CardNum), "CARDNUM");
        assert_eq!(BmsField::rank(AuthSummaryField::AcctId), 1);
        assert_eq!(BmsField::rank(AuthDetailField::CardNum), 1);

        assert_eq!(to_json(&AuthSummaryField::AcctId), json!("ACCTID"));
        assert_eq!(to_json(&AuthDetailField::CardNum), json!("CARDNUM"));
        let parsed: AuthSummaryField =
            serde_json::from_value(json!("ACCTID")).expect("the BMS name");
        assert_eq!(parsed, AuthSummaryField::AcctId);
        let parsed: AuthDetailField =
            serde_json::from_value(json!("CARDNUM")).expect("the BMS name");
        assert_eq!(parsed, AuthDetailField::CardNum);
        assert_eq!(
            to_json(&AuthSummaryField::schema())["enum"],
            json!(["ACCTID"])
        );
        assert_eq!(
            to_json(&AuthDetailField::schema())["enum"],
            json!(["CARDNUM"])
        );
    }
}
