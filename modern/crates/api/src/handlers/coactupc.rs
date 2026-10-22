//! COACTUPC, account update (transaction `CAUP`, map `CACTUPA`): the handlers of endpoint 5,
//! `GET /api/v1/accounts/{acct_id}` (ENTER fetch), and endpoint 6, `PUT
//! /api/v1/accounts/{acct_id}` (ENTER validate, F5 save). F3 and F12 send no request.
//!
//! Each handler extracts the verified identity, the raw path account number and, for endpoint
//! 6, the validated body, calls one function of `crate::services::coactupc` and maps what it
//! returns through the `From` conversions of `crate::dto::coactupc` and `crate::error`. Which
//! key does what is the service's and the domain's decision, never the handler's: the handler
//! reads no member of the request (Thin Request Handlers).
//!
//! The SSN leaves only redacted, through the DTO's serialisation; its parts enter only when the
//! operator re-entered them and are never logged here (Cardholder Data Redaction). The date,
//! phone and SSN parts and the amounts travel as the raw map text, because COACTUPC's own edits
//! word the messages for them; the handler passes them on unchanged, the raw-edit-input
//! exception of Typed Dates and Timestamps.

use axum::Json;
use axum::extract::{Path, State};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidJson;
use crate::dto::coactupc::{AccountUpdateRequest, AccountUpdateResult, AccountUpdateView};
use crate::error::ScreenError;
use crate::services;

/// Fetch an account for update (CAUP / COACTUPC).
///
/// Endpoint 5, ENTER while no details are shown. The account number is the raw path text, which
/// the program's account edit checks before any read; the cross-reference, the account and the
/// customer are then read, and the response carries the values the map shows with the snapshot
/// the client echoes in every endpoint 6 request. Dates are typed; the SSN is only `***-**-` and
/// its last four digits.
///
/// # Errors
///
/// A `ScreenError` with the program's message: 400 when the account number fails its edit, 401
/// without a valid bearer token, 404 when the cross-reference, the account or the customer is not
/// found, and 500 when a read fails.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{acct_id}",
    operation_id = "coactupc_fetch",
    tag = "COACTUPC",
    params(
        (
            "acct_id" = String,
            Path,
            description = "Account number as typed on the screen (11 characters)"
        ),
    ),
    responses(
        (
            status = 200,
            description = "The account and customer as shown, with the snapshot to echo",
            body = AccountUpdateView
        ),
        (
            status = 400,
            description = "The account number failed the program's edit",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 404,
            description = "No cross-reference, account or customer was found",
            body = ScreenError
        ),
        (status = 500, description = "A read failed", body = ScreenError),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn fetch(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path(acct_id): Path<String>,
) -> Result<Json<AccountUpdateView>, ScreenError> {
    services::coactupc::fetch(&state, &user, &acct_id)
        .await
        .map(AccountUpdateView::from)
        .map(Json)
        .map_err(ScreenError::from)
}

/// Validate or save an account update (CAUP / COACTUPC).
///
/// Endpoint 6, on the details endpoint 5 showed. `aid` `ENTER` runs the change check and the
/// program's edits and, when the changes pass, answers state `N` with `F5=Save` shown; `PF5` with
/// the echoed state `N` locks and rewrites the account and the customer, answering state `C`.
/// Any other key text is read as `ENTER`; F3 and F12 send no request. The date, phone and SSN
/// parts and the amounts are the raw map text the program's edits check. The SSN parts are
/// write-only: omitting all three leaves the stored SSN unchanged.
///
/// # Errors
///
/// A `ScreenError` with the program's message: 400 when the body fails validation, nothing
/// changed or an edit fails; 401 without a valid bearer token; 404 when the re-read finds no
/// cross-reference, account or customer; 409 when the stored records changed after the fetch; 500
/// when a read, a lock or a rewrite fails, or the store fails.
#[utoipa::path(
    put,
    path = "/api/v1/accounts/{acct_id}",
    operation_id = "coactupc_save",
    tag = "COACTUPC",
    params(
        (
            "acct_id" = String,
            Path,
            description = "Account number as typed on the screen (11 characters)"
        ),
    ),
    request_body = AccountUpdateRequest,
    responses(
        (
            status = 200,
            description = "The changes were validated or saved, or the screen restarted",
            body = AccountUpdateResult
        ),
        (
            status = 400,
            description = "The body failed validation, nothing changed, or an edit failed",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 404,
            description = "The re-read found no cross-reference, account or customer",
            body = ScreenError
        ),
        (
            status = 409,
            description = "The stored records changed after the fetch",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A read, a lock, a rewrite or the store failed",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn save(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path(acct_id): Path<String>,
    ValidJson(request): ValidJson<AccountUpdateRequest>,
) -> Result<Json<AccountUpdateResult>, ScreenError> {
    services::coactupc::save(&state, &user, &acct_id, request)
        .await
        .map(AccountUpdateResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
