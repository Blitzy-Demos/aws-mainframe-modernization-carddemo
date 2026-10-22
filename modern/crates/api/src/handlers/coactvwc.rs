//! COACTVWC, account view: the handler of endpoint 4, `GET /api/v1/accounts/{acct_id}/view`
//! (transaction `CAVW`, map `CACTVWA`; AAP 0.3.4.1).
//!
//! COACTVWC serves one key, ENTER: it edits the typed account number, then reads the card
//! cross-reference, the account and the customer (COACTVWC-005, COACTVWC-029 to COACTVWC-047).
//! PF3 and the transfer back to the main menu are client-side and call no endpoint
//! (COACTVWC-004).
//!
//! [`view`] is a thin handler (Thin Request Handlers): it extracts the signed-on user and the
//! raw `acct_id` path text ([`RawPath`]), calls `services::coactvwc::view`, and maps the outcome
//! through `AccountViewResponse::from` and `ScreenError::from`. The path text goes to the service
//! unparsed and untrimmed, so the program's own edit answers a malformed account number with
//! its own message (COACTVWC-029 to COACTVWC-032).
//!
//! The SSN reaches the response only in its redacted form, `***-**-` and the last four digits,
//! through the `domain::sensitive::Ssn` the DTO carries; this module neither reads nor formats
//! it (COACTVWC-017, E32).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::RawPath;
use crate::dto::coactvwc::AccountViewResponse;
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`view`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ViewOperation;

/// View an account (CAVW / COACTVWC).
///
/// ENTER on the account view: the account number as typed is edited, then the card
/// cross-reference is read by account, the account by the same key and the customer by the
/// cross-reference's customer ID. The response holds the account and customer blocks of the
/// map, the SSN in redacted form only. PF3 is client-side and calls no endpoint.
///
/// # Errors
///
/// A `ScreenError` with the program's message and the field `acct_id`: 400 when the account
/// number is missing, not numeric or all zeros; 404 when the cross-reference, the account or
/// the customer record is not found; 500 when one of those reads fails. 401 when the request
/// carries no valid bearer token. A segment that is not UTF-8 once percent-decoded is a 400
/// with the API's `Invalid path parameters...` and no field, before the program's edit.
#[utoipa::path(
    impl_for = ViewOperation,
    get,
    path = "/api/v1/accounts/{acct_id}/view",
    operation_id = "coactvwc_view",
    tag = "COACTVWC",
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
            description = "The account and customer blocks of the view, the SSN redacted",
            body = AccountViewResponse
        ),
        (
            status = 400,
            description = "The account number is missing, not numeric or all zeros; or a path \
                           segment that is not UTF-8 once percent-decoded",
            body = ScreenError
        ),
        (
            status = 401,
            description = "The request carries no valid bearer token",
            body = ScreenError
        ),
        (
            status = 404,
            description = "The cross-reference, account or customer record was not found",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The cross-reference, account or customer read failed",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn view(
    State(state): State<AppState>,
    user: SignedOnUser,
    RawPath(acct_id): RawPath<String>,
) -> Result<Json<AccountViewResponse>, ScreenError> {
    services::coactvwc::view(&state, &user, &acct_id)
        .await
        .map(AccountViewResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
