//! COTRN01C, the transaction view: the handler of endpoint 12,
//! `GET /api/v1/transactions/{tran_id}` (transaction `CT01`, map `COTRN1A`; AAP 0.3.4.1).
//!
//! [`view`] serves ENTER, the program's only key that reads: it extracts the signed-on user and
//! the raw `tran_id` path segment, calls `services::cotrn01c::view` and maps the answer through
//! `TranViewResponse::from` or `ScreenError::from` (Thin Request Handlers). F4 (clear), F5 (to
//! the transaction list) and PF3 (back) are served by the screen alone and reach no endpoint
//! (COTRN01C-004 to COTRN01C-006).
//!
//! The path segment stays a `String`, never parsed here: the domain's total edit, not an
//! extractor, answers a blank Tran ID with the program's own message (COTRN01C-009). The card
//! number leaves only in its redacted form, because the response holds it as a `CardNumber`
//! (COTRN01C-017, E32, Cardholder Data Redaction), and the two timestamps are typed in the
//! response (COTRN01C-023, COTRN01C-024, Typed Dates and Timestamps).

use axum::Json;
use axum::extract::{Path, State};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cotrn01c::TranViewResponse;
use crate::error::ScreenError;
use crate::services;

/// View a transaction (CT01 / COTRN01C).
///
/// ENTER reads the transaction whose ID the Tran ID input holds, sent as the `tran_id` path
/// segment exactly as typed, and returns the detail fields of the `COTRN1A` map. Every signed-on
/// user gets the same view, whatever the user type. The card number is returned redacted to its
/// last four characters, and the original and processing timestamps are typed, `null` when
/// blank. F4, F5 and PF3 send no request. Nothing is locked or written.
///
/// # Errors
///
/// A `ScreenError` naming field `tran_id`: 400 `Tran ID can NOT be empty...` when the Tran ID
/// is blank, 404 `Transaction ID NOT found...` when no transaction has the key, and 500
/// `Unable to lookup Transaction...` when the read fails. A request without a valid bearer
/// token is refused with 401 before anything is read.
#[utoipa::path(
    get,
    path = "/api/v1/transactions/{tran_id}",
    operation_id = "cotrn01c_view",
    tag = "COTRN01C",
    params(
        (
            "tran_id" = String,
            Path,
            description = "Transaction ID as typed on the screen (16 characters)"
        )
    ),
    responses(
        (
            status = 200,
            description = "The transaction as the `COTRN1A` map shows it, with the card number \
                redacted and no message (COTRN01C-015 to COTRN01C-028)",
            body = TranViewResponse
        ),
        (
            status = 400,
            description = "`Tran ID can NOT be empty...`: the Tran ID is blank, and nothing is \
                read (COTRN01C-009)",
            body = ScreenError
        ),
        (
            status = 401,
            description = "The request carries no valid bearer token",
            body = ScreenError
        ),
        (
            status = 404,
            description = "`Transaction ID NOT found...`: no transaction has the key \
                (COTRN01C-012)",
            body = ScreenError
        ),
        (
            status = 500,
            description = "`Unable to lookup Transaction...`: the read failed (COTRN01C-013)",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn view(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path(tran_id): Path<String>,
) -> Result<Json<TranViewResponse>, ScreenError> {
    services::cotrn01c::view(&state, &user, &tran_id)
        .await
        .map(TranViewResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
