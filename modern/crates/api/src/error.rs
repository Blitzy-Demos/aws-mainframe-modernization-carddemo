//! The API's error contract: [`ScreenError`], the standard `{ message, field? }` failure body of
//! the request layer, the token gate and the programs, with the HTTP status the owning program's
//! audit assigns (AAP 0.3.4.1, 0.5.6). Two exceptions answer without it. Endpoints 24 and 25
//! answer COTRTLIC's own errors (400 and 500, and 404 on endpoint 25) with the page,
//! `dto::cotrtlic::TranTypeListResponse` or `TranTypeListActionResult`; their request-layer
//! rejections, an unissued `page_key` or `state`, an unreachable database and a failed commit
//! transaction keep the [`ScreenError`]. An unmatched path (404) and a method its route does not
//! serve (405) get axum's empty body.
//!
//! # Where a status comes from
//!
//! - **Domain failures.** Each `domain::online` program module returns its own failure type,
//!   and each failure names its status class; the class's `http_status()` is the audited
//!   status. One `From` conversion per failure type copies the message literal and the field
//!   unchanged and takes the status from that class, never from the text. A message on a
//!   `HALTED(H3)` or `HALTED(H4)` path, whose Db2, CICS or IMS codes have no counterpart, is
//!   passed through exactly as the domain renders it.
//! - **Store errors without an audited message.** A [`PersistenceError`] that reaches this
//!   module, through [`ServiceError::Store`], answers 500 with [`UNABLE_TO_PROCESS`], and its
//!   conversion logs it, once, as the only record of its cause. A store error the COBOL does
//!   answer with a text of its own (`Unable to lookup User...`, for instance) is logged by the
//!   service and mapped into the domain's lookup enum, and arrives as a domain failure.
//! - **The request layer.** `api::dto`'s extractors reject a body or query with
//!   [`ScreenError::input`] and an out-of-range `page_size` with [`ScreenError::page_size`];
//!   `api::auth::extract` rejects a missing or invalid token with [`ScreenError::unauthorized`]
//!   and a non-admin token on an admin endpoint with [`ScreenError::forbidden`].
//!
//! Statuses are 400 input, 401 sign-on or token, 403 admin gate, 404 not found, 409 changed by
//! another user and 500 store failure. 422 is reserved for `page_size` (Bounded Pagination), and
//! only [`ScreenError::page_size`] produces it.
//!
//! # Thin Request Handlers
//!
//! The outcome-to-status mapping lives here, so a handler ends in
//! `.map_err(ScreenError::from)` and holds no `match` or `if` on a domain outcome.
//!
//! # Redaction
//!
//! A [`ScreenError`] carries only a domain message literal, which already renders any protected
//! value redacted, or a fixed text of this crate. It never carries the `Display` of a
//! [`PersistenceError`], an SQLSTATE, a constraint or column name, query text or a request value
//! (Cardholder Data Redaction, AAP 0.3.4.8).

use core::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use domain::online::{
    coactupc, coactvwc, cobil00c, cocrdlic, cocrdslc, cocrdupc, copaus0c, copaus1c, corpt00c,
    cosgn00c, cotrn00c, cotrn01c, cotrn02c, cotrtlic, cotrtupc, cousr00c, cousr01c, cousr02c,
};
use persistence::PersistenceError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The message of a 500 that no COBOL program words: a store error with no audited message,
/// such as a failed `begin` or `commit`, and an internal failure, such as token signing.
///
/// A fixed text of this crate, not a COBOL literal. It names no value, no SQLSTATE and no
/// constraint.
pub const UNABLE_TO_PROCESS: &str = "Unable to process request...";

/// The request field every [`ScreenError::page_size`] names.
const PAGE_SIZE_FIELD: &str = "page_size";

/// The standard body of an error response: the message row of the screen and the request field
/// the program puts the cursor on, with the HTTP status the response carries. Endpoints 24 and 25
/// may answer a program error with their page instead.
///
/// Serialised as `{"message": …, "field": …}`, `field` omitted when absent. The status travels
/// as the response status only and is no part of the body or of the schema. A deserialised
/// value, as a test reads one back, has status 500, since the body does not carry it.
///
/// The schema name is `ScreenError`: the frontend's generated types refer to
/// `components["schemas"]["ScreenError"]` (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({"message": "Please enter User ID ...", "field": "user_id"}))]
pub struct ScreenError {
    /// The message row, exactly as the program words it; it never holds a full card number,
    /// SSN, card verification code or password.
    message: String,
    /// The request field the message names and the screen puts the cursor on, as its wire
    /// name, such as `user_id` or `rows[3].description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    /// The HTTP status of the response.
    #[serde(skip, default = "status_not_carried")]
    #[schema(ignore)]
    status: StatusCode,
}

/// The status of a [`ScreenError`] read back from a body, which does not carry one.
const fn status_not_carried() -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// The [`StatusCode`] of an audited status class, from the code its `http_status()` returns.
///
/// Only the statuses of the error contract map to themselves. Any other code, which no domain
/// failure class returns, is a 500: never a success status, and never the 422 reserved for
/// `page_size`.
const fn audited_status(http_status: u16) -> StatusCode {
    match http_status {
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        404 => StatusCode::NOT_FOUND,
        409 => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl ScreenError {
    /// An error with `status`, `message` and `field`.
    fn new(status: StatusCode, message: impl Into<String>, field: Option<String>) -> Self {
        Self {
            message: message.into(),
            field,
            status,
        }
    }

    /// An error with the status a domain failure class assigns, its message and its field.
    fn audited(http_status: u16, message: impl Into<String>, field: Option<&'static str>) -> Self {
        Self::new(
            audited_status(http_status),
            message,
            field.map(str::to_owned),
        )
    }

    /// 400: the request's input was rejected, by a program edit or by request validation.
    #[must_use]
    pub fn input(message: impl Into<String>, field: Option<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, field)
    }

    /// 401: the credentials did not sign on, or the request carries no valid token.
    #[must_use]
    pub fn unauthorized(message: impl Into<String>, field: Option<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message, field)
    }

    /// 403: the admin gate refused a token whose user type is not `A`. The gate names no field.
    #[must_use]
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message, None)
    }

    /// 404: a read found no record.
    #[must_use]
    pub fn not_found(message: impl Into<String>, field: Option<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message, field)
    }

    /// 409: the stored record changed after it was fetched, by another user.
    #[must_use]
    pub fn conflict(message: impl Into<String>, field: Option<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message, field)
    }

    /// 422: `page_size` lies outside 1 to 100 on a collection endpoint (Bounded Pagination).
    ///
    /// The only producer of 422, used only by `api::dto`'s extractors; the field is always
    /// `page_size`.
    #[must_use]
    pub fn page_size(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            message,
            Some(PAGE_SIZE_FIELD.to_owned()),
        )
    }

    /// 500: the store failed.
    #[must_use]
    pub fn store(message: impl Into<String>, field: Option<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message, field)
    }

    /// The HTTP status of the response.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// The message row.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The request field the message names, if any.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }
}

/// The response: the status, and the body as JSON without the status.
impl IntoResponse for ScreenError {
    fn into_response(self) -> Response {
        let status = self.status;
        (status, Json(self)).into_response()
    }
}

/// The message only.
impl fmt::Display for ScreenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ScreenError {}

/// The error of an `api::services` function: `Result<Outcome, ServiceError<ProgramFailure>>`,
/// where `ProgramFailure` is the failure type of the program's `domain::online` module.
///
/// Converted into the response by `ScreenError::from`.
///
/// # Errors
///
/// - [`ServiceError::Rule`]: the program's own answer, with its audited message, field and
///   status class.
/// - [`ServiceError::Store`]: a store error with no audited COBOL message, answered 500 with
///   [`UNABLE_TO_PROCESS`]. `?` on a repository call produces it.
/// - [`ServiceError::Internal`]: an internal failure outside the store, answered 500 with
///   [`UNABLE_TO_PROCESS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError<F> {
    /// The domain failure, carrying its audited message literal.
    Rule(F),
    /// A store error that has no audited message, such as a failed `begin` or `commit`. A store
    /// error the program answers with a text of its own is mapped into the domain's lookup or
    /// result enum by the service and arrives as [`ServiceError::Rule`].
    Store(PersistenceError),
    /// An internal failure outside the store, such as token signing. It carries no payload, so
    /// it can disclose nothing.
    Internal,
}

impl<F> From<PersistenceError> for ServiceError<F> {
    fn from(error: PersistenceError) -> Self {
        Self::Store(error)
    }
}

/// The failure's own `Display`, the store error's SQLSTATE and constraint name, or
/// [`UNABLE_TO_PROCESS`]: no stored or request value.
impl<F: fmt::Display> fmt::Display for ServiceError<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rule(failure) => failure.fmt(f),
            Self::Store(error) => error.fmt(f),
            Self::Internal => f.write_str(UNABLE_TO_PROCESS),
        }
    }
}

/// The source is the wrapped domain failure or [`PersistenceError`], so a walk of the error
/// chain reaches it and can downcast to it; [`ServiceError::Internal`] wraps nothing and has
/// none.
impl<F: std::error::Error + 'static> std::error::Error for ServiceError<F> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rule(failure) => Some(failure),
            Self::Store(error) => Some(error),
            Self::Internal => None,
        }
    }
}

impl<F> From<ServiceError<F>> for ScreenError
where
    ScreenError: From<F>,
{
    fn from(error: ServiceError<F>) -> Self {
        match error {
            ServiceError::Rule(failure) => Self::from(failure),
            // Named in full: the bound `ScreenError: From<F>` would otherwise claim the call.
            ServiceError::Store(error) => <Self as From<PersistenceError>>::from(error),
            ServiceError::Internal => Self::store(UNABLE_TO_PROCESS, None),
        }
    }
}

/// 500 with [`UNABLE_TO_PROCESS`] and no field, whatever the variant: the error's SQLSTATE and
/// constraint name stay out of the body.
///
/// A store error with no audited message, such as a failed `begin`, `commit`, `rollback` or
/// savepoint, ends here and nowhere else, so this conversion logs it once, as an `ERROR` event,
/// before answering. The event carries the error's `Display`, an SQLSTATE and a constraint or
/// column name at most, never a stored or request value (Cardholder Data Redaction); the
/// request span it falls in names the route. A service that answers a store error with the
/// program's own message logs it itself and never returns it as [`ServiceError::Store`].
impl From<PersistenceError> for ScreenError {
    fn from(error: PersistenceError) -> Self {
        tracing::error!(
            error = %error,
            "store failure without an audited message; answering 500"
        );
        Self::store(UNABLE_TO_PROCESS, None)
    }
}

// One conversion per `domain::online` failure type. Each takes the message and the field from
// the failure and the status from its class's `http_status()`, the status the program's audit
// assigns.

impl From<cosgn00c::SignonFailure> for ScreenError {
    fn from(failure: cosgn00c::SignonFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(failure.field().name()),
        )
    }
}

impl From<coactvwc::ViewFailure> for ScreenError {
    fn from(failure: coactvwc::ViewFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(coactvwc::ViewField::name),
        )
    }
}

/// Also the 409 of `check_change_in_rec`, an [`coactupc::UpdateFailure`] of class `Changed`.
impl From<coactupc::UpdateFailure> for ScreenError {
    fn from(failure: coactupc::UpdateFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(account_update_field),
        )
    }
}

impl From<cocrdlic::ListFailure> for ScreenError {
    fn from(failure: cocrdlic::ListFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(card_list_field),
        )
    }
}

impl From<cocrdslc::DetailFailure> for ScreenError {
    fn from(failure: cocrdslc::DetailFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(failure.field().name()),
        )
    }
}

/// Also the 409 of `9300-CHECK-CHANGE-IN-REC`, a [`cocrdupc::CardUpdateFailure`] of class
/// `Conflict`.
impl From<cocrdupc::CardUpdateFailure> for ScreenError {
    fn from(failure: cocrdupc::CardUpdateFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(cocrdupc::CardUpdateField::name),
        )
    }
}

impl From<cotrn00c::TranListFailure> for ScreenError {
    fn from(failure: cotrn00c::TranListFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(tran_list_field),
        )
    }
}

impl From<cotrn01c::TranViewFailure> for ScreenError {
    fn from(failure: cotrn01c::TranViewFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(failure.field().name()),
        )
    }
}

/// Also the field-less 400 of an unserved key, `AddFailure::invalid_key` (COTRN02C-007).
impl From<cotrn02c::AddFailure> for ScreenError {
    fn from(failure: cotrn02c::AddFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(tran_add_field),
        )
    }
}

impl From<corpt00c::ReportFailure> for ScreenError {
    fn from(failure: corpt00c::ReportFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(report_field(failure.field())),
        )
    }
}

impl From<cobil00c::BillPayFailure> for ScreenError {
    fn from(failure: cobil00c::BillPayFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(bill_pay_field(failure.field())),
        )
    }
}

/// COUSR00C's only failure names no field (COUSR00C-026).
impl From<cousr00c::UserListFailure> for ScreenError {
    fn from(failure: cousr00c::UserListFailure) -> Self {
        Self::audited(failure.class().http_status(), failure.message(), None)
    }
}

impl From<cousr01c::UserAddFailure> for ScreenError {
    fn from(failure: cousr01c::UserAddFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(failure.field().name()),
        )
    }
}

/// The failures of COUSR02C and of COUSR03C, which shares the type.
impl From<cousr02c::UserMaintFailure> for ScreenError {
    fn from(failure: cousr02c::UserMaintFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            Some(failure.field().name()),
        )
    }
}

/// The message may embed `HALTED(H4)` code positions; it is passed through as rendered.
impl From<copaus0c::AuthSummaryFailure> for ScreenError {
    fn from(failure: copaus0c::AuthSummaryFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(auth_summary_field),
        )
    }
}

/// COPAUS1C's response carries no detail field, so the failure names none. The message may be
/// empty, as the COBOL sets none, or embed `HALTED(H3)` or `HALTED(H4)` code positions; it is
/// passed through as rendered.
impl From<copaus1c::AuthDetailFailure> for ScreenError {
    fn from(failure: copaus1c::AuthDetailFailure) -> Self {
        Self::audited(failure.class().http_status(), failure.message(), None)
    }
}

/// 400 with the failure's fixed text and no field.
///
/// The domain gives this type no status class. Every variant is an echoed `TranTypeListState`,
/// filter or shown description that no COTRTLIC screen could hold: request input the client
/// sent, so the input status. The text names the defect and never the value.
impl From<cotrtlic::ListFailure> for ScreenError {
    fn from(failure: cotrtlic::ListFailure) -> Self {
        Self::input(failure.to_string(), None)
    }
}

/// The message may be the fixed prefix of a `HALTED(H3)` Db2 text, or blank for COTRTUPC-038;
/// it is passed through as rendered.
impl From<cotrtupc::MaintFailure> for ScreenError {
    fn from(failure: cotrtupc::MaintFailure) -> Self {
        Self::audited(
            failure.class().http_status(),
            failure.message(),
            failure.field().map(type_maint_field),
        )
    }
}

// The request field names of the failure fields whose domain enum names a map field only. Each
// is the wire name of the endpoint's request member: a path parameter, a query parameter or a
// body member.

/// Endpoints 5 and 6: `acct_id` is the path parameter, every other field a member of
/// `AccountUpdateFields`.
const fn account_update_field(field: coactupc::UpdateField) -> &'static str {
    use coactupc::UpdateField as F;
    match field {
        F::AccountId => "acct_id",
        F::ActiveStatus => "active_status",
        F::OpenYear => "open_year",
        F::OpenMonth => "open_month",
        F::OpenDay => "open_day",
        F::CreditLimit => "credit_limit",
        F::ExpiryYear => "expiry_year",
        F::ExpiryMonth => "expiry_month",
        F::ExpiryDay => "expiry_day",
        F::CashCreditLimit => "cash_credit_limit",
        F::ReissueYear => "reissue_year",
        F::ReissueMonth => "reissue_month",
        F::ReissueDay => "reissue_day",
        F::CurrentBalance => "current_balance",
        F::CurrentCycleCredit => "current_cycle_credit",
        F::CurrentCycleDebit => "current_cycle_debit",
        F::SsnPart1 => "ssn_part1",
        F::SsnPart2 => "ssn_part2",
        F::SsnPart3 => "ssn_part3",
        F::DobYear => "dob_year",
        F::DobMonth => "dob_month",
        F::DobDay => "dob_day",
        F::FicoScore => "fico_score",
        F::FirstName => "first_name",
        F::MiddleName => "middle_name",
        F::LastName => "last_name",
        F::AddressLine1 => "address_line_1",
        F::State => "state",
        F::Zip => "zip",
        F::City => "city",
        F::Country => "country",
        F::Phone1Area => "phone_1_area",
        F::Phone1Prefix => "phone_1_prefix",
        F::Phone1Line => "phone_1_line",
        F::Phone2Area => "phone_2_area",
        F::Phone2Prefix => "phone_2_prefix",
        F::Phone2Line => "phone_2_line",
        F::EftAccountId => "eft_account_id",
        F::PrimaryCardHolder => "primary_card_holder",
    }
}

/// Endpoint 7: the two filters of `CardListQuery`.
const fn card_list_field(field: cocrdlic::ListField) -> &'static str {
    match field {
        cocrdlic::ListField::Account => "acct_id",
        cocrdlic::ListField::Card => "card_num",
    }
}

/// Endpoint 11: the search field of `TranListQuery`.
const fn tran_list_field(field: cotrn00c::TranListField) -> &'static str {
    match field {
        cotrn00c::TranListField::StartTranId => "start_tran_id",
    }
}

/// Endpoint 13: the keys and `confirm` of `TranAddRequest`, every other field a member of
/// `TranAddFields`.
const fn tran_add_field(field: cotrn02c::AddField) -> &'static str {
    use cotrn02c::AddField as F;
    match field {
        F::AcctId => "acct_id",
        F::CardNum => "card_num",
        F::TypeCd => "type_cd",
        F::CatCd => "cat_cd",
        F::Source => "source",
        F::Description => "description",
        F::Amount => "amount",
        F::OrigDate => "orig_date",
        F::ProcDate => "proc_date",
        F::MerchantId => "merchant_id",
        F::MerchantName => "merchant_name",
        F::MerchantCity => "merchant_city",
        F::MerchantZip => "merchant_zip",
        F::Confirm => "confirm",
    }
}

/// Endpoint 14: the members of `ReportRequest`.
const fn report_field(field: corpt00c::ReportField) -> &'static str {
    use corpt00c::ReportField as F;
    match field {
        F::Monthly => "monthly",
        F::StartMonth => "start_month",
        F::StartDay => "start_day",
        F::StartYear => "start_year",
        F::EndMonth => "end_month",
        F::EndDay => "end_day",
        F::EndYear => "end_year",
        F::Confirm => "confirm",
    }
}

/// Endpoint 15: the members of `BillPaymentRequest`.
const fn bill_pay_field(field: cobil00c::BillPayField) -> &'static str {
    match field {
        cobil00c::BillPayField::AcctId => "acct_id",
        cobil00c::BillPayField::Confirm => "confirm",
    }
}

/// Endpoint 21: the `acct_id` path parameter.
const fn auth_summary_field(field: copaus0c::AuthSummaryField) -> &'static str {
    match field {
        copaus0c::AuthSummaryField::AcctId => "acct_id",
    }
}

/// Endpoints 26 and 27: the `type_code` path parameter and the `description` member of
/// `TranTypeActionRequest`.
const fn type_maint_field(field: cotrtupc::MaintField) -> &'static str {
    match field {
        cotrtupc::MaintField::TypeCode => "type_code",
        cotrtupc::MaintField::Description => "description",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;

    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use axum::response::IntoResponse;
    use domain::online::{
        coactupc, coactvwc, cobil00c, cocrdlic, cocrdslc, cocrdupc, copaus0c, copaus1c, corpt00c,
        cosgn00c, cotrn00c, cotrn01c, cotrn02c, cotrtlic, cotrtupc, cousr00c, cousr01c, cousr02c,
    };
    use persistence::{DecodeKind, PersistenceError};
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};

    use super::{
        ScreenError, ServiceError, UNABLE_TO_PROCESS, account_update_field, audited_status,
        auth_summary_field, bill_pay_field, card_list_field, report_field, tran_add_field,
        tran_list_field, type_maint_field,
    };

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Every persistence error variant, each carrying an SQLSTATE or a constraint name where the
    /// variant can hold one.
    fn persistence_errors() -> [PersistenceError; 7] {
        [
            PersistenceError::NotFound,
            PersistenceError::Duplicate {
                constraint: Some("users_pkey".to_owned()),
            },
            PersistenceError::Restrict {
                constraint: Some("trc_type_code".to_owned()),
            },
            PersistenceError::Check {
                constraint: Some("accounts_acct_id_check".to_owned()),
            },
            PersistenceError::Database {
                sqlstate: Some("40P01".to_owned()),
                constraint: Some("xtran_type".to_owned()),
            },
            PersistenceError::Unavailable,
            PersistenceError::decode("card_num", DecodeKind::NotNumeric),
        ]
    }

    /// The status, message and field of `error`.
    fn parts(error: &ScreenError) -> (u16, &str, Option<&str>) {
        (error.status().as_u16(), error.message(), error.field())
    }

    #[test]
    fn constructors_set_their_status_message_and_field() {
        let field = || Some("acct_id".to_owned());
        let cases = [
            (ScreenError::input("in", field()), 400, Some("acct_id")),
            (ScreenError::input("in", None), 400, None),
            (
                ScreenError::unauthorized("in", field()),
                401,
                Some("acct_id"),
            ),
            (ScreenError::forbidden("in"), 403, None),
            (ScreenError::not_found("in", field()), 404, Some("acct_id")),
            (ScreenError::conflict("in", None), 409, None),
            (ScreenError::page_size("in"), 422, Some("page_size")),
            (ScreenError::store("in", field()), 500, Some("acct_id")),
        ];
        for (error, status, expected_field) in cases {
            assert_eq!(parts(&error), (status, "in", expected_field), "{error:?}");
            assert_eq!(error.to_string(), "in");
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn audited_statuses_keep_the_error_contract_and_never_reach_422() {
        for code in [400, 401, 403, 404, 409, 500] {
            assert_eq!(audited_status(code).as_u16(), code);
        }
        for code in [0, 200, 302, 422, 503, 999] {
            assert_eq!(audited_status(code), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[test]
    fn every_domain_failure_class_maps_to_its_own_status() {
        let codes = [
            cosgn00c::FailureClass::Input.http_status(),
            cosgn00c::FailureClass::Signon.http_status(),
            cosgn00c::FailureClass::Store.http_status(),
            coactvwc::FailureClass::Input.http_status(),
            coactvwc::FailureClass::NotFound.http_status(),
            coactvwc::FailureClass::Store.http_status(),
            coactupc::FailureClass::Input.http_status(),
            coactupc::FailureClass::NotFound.http_status(),
            coactupc::FailureClass::Changed.http_status(),
            coactupc::FailureClass::Store.http_status(),
            coactupc::FailureClass::Abend.http_status(),
            cocrdlic::FailureClass::Input.http_status(),
            cocrdlic::FailureClass::Store.http_status(),
            cocrdslc::FailureClass::Input.http_status(),
            cocrdslc::FailureClass::NotFound.http_status(),
            cocrdslc::FailureClass::Store.http_status(),
            cocrdupc::FailureClass::Input.http_status(),
            cocrdupc::FailureClass::NotFound.http_status(),
            cocrdupc::FailureClass::Conflict.http_status(),
            cocrdupc::FailureClass::Store.http_status(),
            cotrn00c::FailureClass::Input.http_status(),
            cotrn00c::FailureClass::Lookup.http_status(),
            cotrn01c::FailureClass::Input.http_status(),
            cotrn01c::FailureClass::NotFound.http_status(),
            cotrn01c::FailureClass::Lookup.http_status(),
            cotrn02c::FailureClass::Input.http_status(),
            cotrn02c::FailureClass::NotFound.http_status(),
            cotrn02c::FailureClass::Store.http_status(),
            corpt00c::FailureClass::Input.http_status(),
            corpt00c::FailureClass::Write.http_status(),
            cobil00c::FailureClass::Input.http_status(),
            cobil00c::FailureClass::NotFound.http_status(),
            cobil00c::FailureClass::Store.http_status(),
            cousr00c::FailureClass::Store.http_status(),
            cousr01c::FailureClass::Input.http_status(),
            cousr01c::FailureClass::Duplicate.http_status(),
            cousr01c::FailureClass::Store.http_status(),
            cousr02c::FailureClass::Input.http_status(),
            cousr02c::FailureClass::NotFound.http_status(),
            cousr02c::FailureClass::Store.http_status(),
            copaus0c::FailureClass::Input.http_status(),
            copaus0c::FailureClass::NotFound.http_status(),
            copaus0c::FailureClass::Store.http_status(),
            cotrtupc::FailureClass::Input.http_status(),
            cotrtupc::FailureClass::Store.http_status(),
        ];
        for code in codes {
            assert_eq!(audited_status(code).as_u16(), code);
        }
    }

    #[test]
    fn persistence_errors_answer_500_without_any_detail() {
        for store in persistence_errors() {
            let detail: Vec<String> = [store.sqlstate(), store.constraint()]
                .into_iter()
                .flatten()
                .map(str::to_owned)
                .collect();
            let error = ScreenError::from(store);
            assert_eq!(parts(&error), (500, UNABLE_TO_PROCESS, None));
            for text in &detail {
                assert!(!error.message().contains(text.as_str()), "{text}");
            }
            assert!(!error.message().contains("card_num"));
        }
    }

    #[test]
    fn a_store_error_without_an_audited_message_is_logged_once_with_its_cause() -> TestResult {
        let traces = test_support::trace_capture::capture();
        for store in persistence_errors() {
            traces.clear();
            let cause = store.to_string();
            let service = ServiceError::<cotrn01c::TranViewFailure>::Store(store);
            let error = ScreenError::from(service);
            assert_eq!(parts(&error), (500, UNABLE_TO_PROCESS, None));
            let lines = traces.lines();
            let [line] = lines.as_slice() else {
                return Err(format!("{cause}: {} lines", lines.len()).into());
            };
            for expected in [
                r#""level":"ERROR""#,
                "store failure without an audited message",
                cause.as_str(),
            ] {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
        }

        // A domain answer or an internal failure is not a store error, and logs nothing here.
        traces.clear();
        let rule = ServiceError::Rule(cotrn01c::TranViewFailure::LookupFailed);
        assert_eq!(ScreenError::from(rule).status().as_u16(), 500);
        let internal = ServiceError::<cotrn01c::TranViewFailure>::Internal;
        assert_eq!(
            parts(&ScreenError::from(internal)),
            (500, UNABLE_TO_PROCESS, None)
        );
        assert!(traces.lines().is_empty(), "{}", traces.text());
        Ok(())
    }

    #[test]
    fn service_errors_convert_by_variant() {
        let rule: ServiceError<cotrn01c::TranViewFailure> =
            ServiceError::Rule(cotrn01c::TranViewFailure::NotFound);
        let error = ScreenError::from(rule);
        assert_eq!(
            parts(&error),
            (
                404,
                cotrn01c::TranViewFailure::NotFound.message(),
                Some("tran_id")
            )
        );
        for store in persistence_errors() {
            let service = ServiceError::<cotrn01c::TranViewFailure>::from(store.clone());
            assert_eq!(service, ServiceError::Store(store));
            let error = ScreenError::from(service);
            assert_eq!(parts(&error), (500, UNABLE_TO_PROCESS, None));
        }
        let internal = ScreenError::from(ServiceError::<cousr00c::UserListFailure>::Internal);
        assert_eq!(parts(&internal), (500, UNABLE_TO_PROCESS, None));
    }

    #[test]
    fn service_errors_display_no_value_and_chain_to_the_wrapped_error() {
        let failure = cousr00c::UserListFailure::LookupFailed;
        let rule = ServiceError::<cousr00c::UserListFailure>::Rule(failure);
        assert_eq!(rule.to_string(), failure.message());
        assert_eq!(
            rule.source()
                .and_then(|source| source.downcast_ref::<cousr00c::UserListFailure>()),
            Some(&failure)
        );
        for store in persistence_errors() {
            let error = ServiceError::<cousr00c::UserListFailure>::Store(store.clone());
            assert_eq!(error.to_string(), store.to_string());
            assert_eq!(
                error
                    .source()
                    .and_then(|source| source.downcast_ref::<PersistenceError>()),
                Some(&store)
            );
        }
        let internal = ServiceError::<cousr00c::UserListFailure>::Internal;
        assert_eq!(internal.to_string(), UNABLE_TO_PROCESS);
        assert!(internal.source().is_none());
    }

    #[tokio::test]
    async fn responses_carry_the_status_and_a_body_without_it() -> TestResult {
        let cases = [
            (
                ScreenError::input("Please enter User ID ...", Some("user_id".to_owned())),
                json!({"message": "Please enter User ID ...", "field": "user_id"}),
            ),
            (ScreenError::forbidden("gate"), json!({"message": "gate"})),
            (
                ScreenError::page_size("size"),
                json!({"message": "size", "field": "page_size"}),
            ),
        ];
        for (error, body) in cases {
            let status = error.status();
            let response = error.into_response();
            assert_eq!(response.status(), status);
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .ok_or("no content type")?;
            assert_eq!(content_type, "application/json");
            let bytes = axum::body::to_bytes(response.into_body(), 4096).await?;
            let sent: Value = serde_json::from_slice(&bytes)?;
            assert_eq!(sent, body);
        }
        Ok(())
    }

    #[test]
    fn bodies_read_back_without_a_status() -> TestResult {
        let read: ScreenError = serde_json::from_value(json!({"message": "m", "field": "f"}))?;
        assert_eq!(parts(&read), (500, "m", Some("f")));
        let bare: ScreenError = serde_json::from_value(json!({"message": "m"}))?;
        assert_eq!(parts(&bare), (500, "m", None));
        Ok(())
    }

    #[test]
    fn schema_is_screen_error_with_message_and_optional_field_only() -> TestResult {
        assert_eq!(ScreenError::name(), "ScreenError");
        let schema = serde_json::to_value(ScreenError::schema())?;
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("no properties")?;
        let names: Vec<&str> = properties.keys().map(String::as_str).collect();
        assert_eq!(names, ["field", "message"]);
        assert_eq!(schema.get("required"), Some(&json!(["message"])));
        assert_eq!(
            schema.get("example"),
            Some(&json!({"message": "Please enter User ID ...", "field": "user_id"}))
        );
        Ok(())
    }

    #[test]
    fn signon_and_view_failures_keep_literal_field_and_class() -> TestResult {
        for failure in [
            cosgn00c::SignonFailure::UserIdBlank,
            cosgn00c::SignonFailure::WrongPassword,
            cosgn00c::SignonFailure::UserNotFound,
            cosgn00c::SignonFailure::UnableToVerify,
        ] {
            let error = ScreenError::from(failure);
            assert_eq!(
                parts(&error),
                (
                    failure.class().http_status(),
                    failure.message(),
                    Some(failure.field().name())
                )
            );
        }
        let wrong = ScreenError::from(cosgn00c::SignonFailure::WrongPassword);
        assert_eq!(
            parts(&wrong),
            (401, cosgn00c::WRONG_PASSWORD, Some("password"))
        );

        let view = coactvwc::edit_account("")
            .err()
            .ok_or("blank account accepted")?;
        let expected = view.message().to_owned();
        assert_eq!(
            parts(&ScreenError::from(view)),
            (400, expected.as_str(), Some("acct_id"))
        );

        let update = coactupc::account_key("")
            .err()
            .ok_or("blank account accepted")?;
        let expected = update.message().to_owned();
        assert_eq!(
            parts(&ScreenError::from(update)),
            (400, expected.as_str(), Some("acct_id"))
        );
        Ok(())
    }

    #[test]
    fn card_failures_keep_literal_field_and_class() -> TestResult {
        let read = cocrdlic::ListFailure::file_error();
        assert_eq!(parts(&ScreenError::from(read)), (500, read.message(), None));

        let detail = cocrdslc::edit("", None, None)
            .err()
            .ok_or("blank keys accepted")?;
        assert_eq!(
            parts(&ScreenError::from(detail)),
            (400, detail.message(), Some(detail.field().name()))
        );

        for failure in [
            cocrdupc::CardUpdateFailure::lock_failed(),
            cocrdupc::CardUpdateFailure::rewrite_failed(),
        ] {
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (500, failure.message(), None)
            );
        }
        Ok(())
    }

    #[test]
    fn transaction_failures_keep_literal_field_and_class() -> TestResult {
        let lookup = cotrn00c::lookup_failure();
        assert_eq!(
            parts(&ScreenError::from(lookup)),
            (500, lookup.message(), None)
        );
        let search = cotrn00c::edit_start_tran_id("12AB")
            .err()
            .ok_or("non-numeric search accepted")?;
        assert_eq!(
            parts(&ScreenError::from(search)),
            (400, search.message(), Some("start_tran_id"))
        );

        for failure in [
            cotrn01c::TranViewFailure::Blank,
            cotrn01c::TranViewFailure::NotFound,
            cotrn01c::TranViewFailure::LookupFailed,
        ] {
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (
                    failure.class().http_status(),
                    failure.message(),
                    Some("tran_id")
                )
            );
        }

        let cases = [
            (cotrn02c::AddFailure::transaction_lookup(), 500, "acct_id"),
            (cotrn02c::AddFailure::last_not_found(), 404, "acct_id"),
            (cotrn02c::AddFailure::duplicate_id(), 500, "acct_id"),
            (cotrn02c::AddFailure::write_failed(), 500, "acct_id"),
            (cotrn02c::AddFailure::invalid_confirmation(), 400, "confirm"),
        ];
        for (failure, status, field) in cases {
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (status, failure.message(), Some(field))
            );
        }
        // COTRN02C-007: the whole 50-byte text, trailing spaces kept, and no `field` member.
        let invalid_key = ScreenError::from(cotrn02c::AddFailure::invalid_key());
        assert_eq!(
            parts(&invalid_key),
            (400, domain::messages::CCDA_MSG_INVALID_KEY, None)
        );
        assert_eq!(invalid_key.message().len(), 50);
        assert_eq!(
            serde_json::to_value(&invalid_key)?,
            json!({"message": domain::messages::CCDA_MSG_INVALID_KEY})
        );
        Ok(())
    }

    #[test]
    fn report_and_bill_payment_failures_keep_literal_field_and_class() -> TestResult {
        let deck = corpt00c::tdq_write_failure();
        let expected = deck.message().to_owned();
        assert_eq!(
            parts(&ScreenError::from(deck)),
            (500, expected.as_str(), Some("monthly"))
        );

        let blank = cobil00c::edit_account("")
            .err()
            .ok_or("blank account accepted")?;
        let cases = [
            (blank, 400, "acct_id"),
            (cobil00c::account_not_found(), 404, "acct_id"),
            (cobil00c::account_lookup_failed(), 500, "acct_id"),
            (cobil00c::transaction_lookup_failed(), 500, "acct_id"),
        ];
        for (failure, status, field) in cases {
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (status, failure.message(), Some(field))
            );
        }
        Ok(())
    }

    #[test]
    fn user_failures_keep_literal_field_and_class() {
        let list = cousr00c::UserListFailure::LookupFailed;
        assert_eq!(parts(&ScreenError::from(list)), (500, list.message(), None));

        for (failure, status) in [
            (cousr01c::duplicate_user_id(), 400),
            (cousr01c::write_failure(), 500),
        ] {
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (status, failure.message(), Some(failure.field().name()))
            );
        }

        for (failure, status, field) in [
            (cousr02c::UserMaintFailure::not_found(), 404, "user_id"),
            (
                cousr02c::UserMaintFailure::update_failed(),
                500,
                "first_name",
            ),
        ] {
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (status, failure.message(), Some(field))
            );
        }
    }

    #[test]
    fn authorization_failures_pass_their_rendered_message_through() -> TestResult {
        let summary = copaus0c::edit_account("")
            .err()
            .ok_or("blank account accepted")?;
        let expected = summary.message().to_owned();
        assert_eq!(
            parts(&ScreenError::from(summary)),
            (400, expected.as_str(), Some("acct_id"))
        );

        let detail = copaus1c::edit("", "").err().ok_or("blank keys accepted")?;
        let expected = detail.message().to_owned();
        assert_eq!(
            parts(&ScreenError::from(detail)),
            (400, expected.as_str(), None)
        );
        Ok(())
    }

    #[test]
    fn transaction_type_failures_keep_literal_field_and_class() -> TestResult {
        for failure in [
            cotrtlic::ListFailure::TooManyRows,
            cotrtlic::ListFailure::RowOutOfRange,
            cotrtlic::ListFailure::PageOutOfRange,
            cotrtlic::ListFailure::FieldTooLong,
            cotrtlic::ListFailure::InvalidKey,
            cotrtlic::ListFailure::InvalidRow,
        ] {
            let text = failure.to_string();
            assert_eq!(
                parts(&ScreenError::from(failure)),
                (400, text.as_str(), None)
            );
        }

        let code = cotrtupc::edit_type_code("")
            .err()
            .ok_or("blank code accepted")?;
        assert_eq!(
            parts(&ScreenError::from(code)),
            (400, code.message(), Some("type_code"))
        );
        Ok(())
    }

    /// `names` are snake-case wire names, none repeated.
    fn assert_wire_names(names: &[&str]) {
        let distinct: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(distinct.len(), names.len(), "{names:?}");
        for name in names {
            assert!(
                !name.is_empty()
                    && name.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'),
                "{name}"
            );
        }
    }

    #[test]
    fn field_names_are_distinct_wire_names() {
        use coactupc::UpdateField as U;
        use corpt00c::ReportField as R;
        use cotrn02c::AddField as A;

        assert_wire_names(
            &[
                U::AccountId,
                U::ActiveStatus,
                U::OpenYear,
                U::OpenMonth,
                U::OpenDay,
                U::CreditLimit,
                U::ExpiryYear,
                U::ExpiryMonth,
                U::ExpiryDay,
                U::CashCreditLimit,
                U::ReissueYear,
                U::ReissueMonth,
                U::ReissueDay,
                U::CurrentBalance,
                U::CurrentCycleCredit,
                U::CurrentCycleDebit,
                U::SsnPart1,
                U::SsnPart2,
                U::SsnPart3,
                U::DobYear,
                U::DobMonth,
                U::DobDay,
                U::FicoScore,
                U::FirstName,
                U::MiddleName,
                U::LastName,
                U::AddressLine1,
                U::State,
                U::Zip,
                U::City,
                U::Country,
                U::Phone1Area,
                U::Phone1Prefix,
                U::Phone1Line,
                U::Phone2Area,
                U::Phone2Prefix,
                U::Phone2Line,
                U::EftAccountId,
                U::PrimaryCardHolder,
            ]
            .map(account_update_field),
        );
        assert_wire_names(
            &[
                A::AcctId,
                A::CardNum,
                A::TypeCd,
                A::CatCd,
                A::Source,
                A::Description,
                A::Amount,
                A::OrigDate,
                A::ProcDate,
                A::MerchantId,
                A::MerchantName,
                A::MerchantCity,
                A::MerchantZip,
                A::Confirm,
            ]
            .map(tran_add_field),
        );
        assert_wire_names(
            &[
                R::Monthly,
                R::StartMonth,
                R::StartDay,
                R::StartYear,
                R::EndMonth,
                R::EndDay,
                R::EndYear,
                R::Confirm,
            ]
            .map(report_field),
        );

        assert_wire_names(&[
            card_list_field(cocrdlic::ListField::Account),
            card_list_field(cocrdlic::ListField::Card),
        ]);
        assert_wire_names(&[
            bill_pay_field(cobil00c::BillPayField::AcctId),
            bill_pay_field(cobil00c::BillPayField::Confirm),
        ]);
        assert_wire_names(&[
            type_maint_field(cotrtupc::MaintField::TypeCode),
            type_maint_field(cotrtupc::MaintField::Description),
        ]);
        assert_wire_names(&[tran_list_field(cotrn00c::TranListField::StartTranId)]);
        assert_wire_names(&[auth_summary_field(copaus0c::AuthSummaryField::AcctId)]);
    }
}
