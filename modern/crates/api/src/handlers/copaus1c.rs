//! The handlers of transaction `CPVD`, program COPAUS1C and map `COPAU1A` (AAP 0.3.4.1):
//!
//! - endpoint 22, `GET /api/v1/authorizations/{acct_id}/{auth_key}`: ENTER, a refresh or the
//!   first entry from COPAUS0C's selection, and F8 with `direction=forward`, the next
//!   authorization of the account ([`detail`]);
//! - endpoint 23, `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud`: F5, mark or unmark
//!   the authorization as fraud ([`toggle_fraud`]). COPAUS1C `LINK`s COPAUS2C for the `AUTHFRDS`
//!   write, and COPAUS2C's rules run inside `services::copaus1c::toggle_fraud`, in COPAUS1C's unit
//!   of work, so COPAUS2C has no handler of its own.
//!
//! F3 back to `/authorizations` is the screen's own and calls no endpoint (COPAUS1C-004).
//!
//! # Thin Request Handlers
//!
//! Each handler extracts, delegates and maps, in one expression: the verified identity and the
//! request, one call of `services::copaus1c`, and the `From` conversions of `dto::copaus1c` and
//! [`ScreenError`]. The two path segments reach the service as the raw text the client sent:
//! `domain::online::copaus1c::edit` and the total `AuthKey::parse` decode them there, so a
//! malformed account or key is answered with the program's own status and message (Total
//! Parsers Over Untrusted Input). Whether a request is ENTER or F8 is the service's dispatch on
//! `direction`, never a branch here. Handlers never reference the storage crate or SQL.
//!
//! # Identity and redaction
//!
//! [`SignedOnUser`] is the first extractor after the state, so a request without a valid token
//! is a 401 before its path or query is read (AAP 0.5.1). CPVD checks no user type, so any
//! signed-on user may view or flag an authorization. The response's card number is a
//! `CardNumber`, serialised as twelve `*` and the card's last four digits, never the full number,
//! and nothing here logs or traces a value (Cardholder Data Redaction, E32).

use axum::Json;
use axum::extract::{Path, State};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidQuery;
use crate::dto::copaus1c::{AuthDetailQuery, AuthDetailResponse, FraudToggleResult};
use crate::error::ScreenError;
use crate::services;

/// View an authorization (CPVD / COPAUS1C).
///
/// The pending authorization the path names, as `POPULATE-AUTH-DETAILS` fills `COPAU1A`: card
/// number, date and time, response and reason, amount, merchant and fraud status. Without
/// `direction` this is ENTER, `PROCESS-ENTER-KEY`: the named authorization is read and shown.
/// With `direction=forward` this is F8, `PROCESS-PF8-KEY`: the account's next authorization is
/// shown with its own `auth_key`, or the same one with `Already at the last Authorization...`.
/// `backward` is no key of this screen and is answered as ENTER, as the program's `WHEN OTHER`
/// branch does. The card number is redacted to its last four digits.
///
/// # Errors
///
/// A `ScreenError` body: 400 when the account is not eleven digits, the key is blank or the
/// query does not deserialise; 401 without a valid bearer token; 404 when no authorization of
/// the account has the key; 500 when the summary, detail or next read fails, or the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/authorizations/{acct_id}/{auth_key}",
    operation_id = "copaus1c_detail",
    tag = "COPAUS1C",
    params(
        ("acct_id" = String, Path, description = "Account ID"),
        ("auth_key" = String, Path, description = "Authorization key from a summary row"),
        AuthDetailQuery
    ),
    responses(
        (
            status = 200,
            description = "The authorization as the detail screen shows it",
            body = AuthDetailResponse
        ),
        (
            status = 400,
            description = "Malformed account, blank key or invalid query",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 404,
            description = "No authorization of the account has the key",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A summary, detail or next read failed, or the store failed",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn detail(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path((acct_id, auth_key)): Path<(String, String)>,
    ValidQuery(query): ValidQuery<AuthDetailQuery>,
) -> Result<Json<AuthDetailResponse>, ScreenError> {
    services::copaus1c::detail(&state, &user, &acct_id, &auth_key, query)
        .await
        .map(AuthDetailResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}

/// Mark or unmark an authorization as fraud (CPVD / COPAUS1C, COPAUS2C).
///
/// F5, `MARK-AUTH-FRAUD`: the authorization the path names is read, its fraud flag flipped, and
/// COPAUS2C records the report in `AUTHFRDS`, inserting the row or updating it when one is
/// already stored. Only after that succeeds is the detail replaced, and both writes commit
/// together. The answer carries `AUTH MARKED FRAUD...` or `AUTH FRAUD REMOVED...` and the
/// authorization as replaced, its key unchanged and its card number redacted to the last four
/// digits. The request has no body.
///
/// # Errors
///
/// A `ScreenError` body: 400 when the account is not eleven digits or the key is blank; 401
/// without a valid bearer token; 404 when no authorization of the account has the key; 500 when
/// a read fails, when COPAUS2C's insert or update or the replace of the detail fails and both
/// writes are rolled back, or when the store fails. Nothing is written on any error.
#[utoipa::path(
    post,
    path = "/api/v1/authorizations/{acct_id}/{auth_key}/fraud",
    operation_id = "copaus1c_toggle_fraud",
    tag = "COPAUS1C",
    params(
        ("acct_id" = String, Path, description = "Account ID"),
        ("auth_key" = String, Path, description = "Authorization key from a summary row")
    ),
    responses(
        (
            status = 200,
            description = "The fraud flag was flipped and committed",
            body = FraudToggleResult
        ),
        (status = 400, description = "Malformed account or blank key", body = ScreenError),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 404,
            description = "No authorization of the account has the key",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A read or write failed and nothing was committed",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn toggle_fraud(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path((acct_id, auth_key)): Path<(String, String)>,
) -> Result<Json<FraudToggleResult>, ScreenError> {
    services::copaus1c::toggle_fraud(&state, &user, &acct_id, &auth_key)
        .await
        .map(FraudToggleResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
