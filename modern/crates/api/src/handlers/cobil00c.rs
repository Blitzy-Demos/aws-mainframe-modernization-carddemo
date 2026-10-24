//! Endpoint 15, `POST /api/v1/bill-payments`: the handler of COBIL00C (transaction `CB00`, map
//! `COBIL0A`; AAP 0.3.4.1).
//!
//! ENTER sends the account ID and the confirmation. A blank confirmation shows the balance
//! with the prompt `Confirm to make a bill payment...` (COBIL00C-019), `Y` or `y` pays
//! (COBIL00C-028) and `N` or `n` clears the screen (COBIL00C-010). F4 (clear) and F3 (back) send
//! no request.
//!
//! The handler only extracts the identity and the validated body, calls
//! `services::cobil00c::pay` and maps what it returns (Thin Request Handlers). It never reads
//! `confirm`: the edits, the balance test and the payment are decided by
//! `domain::online::cobil00c` and carried out by the service. The payment arithmetic, the
//! balance moved into `TRAN-AMT PIC S9(09)V99` and then subtracted (ARITH-020 to ARITH-022,
//! E11), is `domain::money` inside the service, and the balance arrives already edited as text
//! (ARITH-019), so no amount is handled here.
//!
//! Option 10 of the main menu is open to `U` and `A` users alike, so the endpoint is guarded by
//! `SignedOnUser`, never by the admin gate (COBIL00C-003). The identity is extracted before the
//! body, so a request without a valid token is a 401 whatever its body holds (COBIL00C-038, n5).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidJson;
use crate::dto::cobil00c::{BillPaymentRequest, BillPaymentResult};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`pay`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct PayOperation;

/// Pay the account balance (CB00 / COBIL00C).
///
/// ENTER on the bill-payment screen. With a blank confirmation it shows the current balance of
/// the account and asks for confirmation; with `Y` or `y` it records the payment transaction
/// and updates the balance; with `N` or `n` it clears the screen. F4 (clear) and F3 (back) are
/// handled by the screen and send no request.
///
/// # Errors
///
/// A `ScreenError` with the program's message and the field the screen focuses:
///
/// - 400: a blank account ID, a confirmation other than `Y`, `y`, `N`, `n` or blank, a balance
///   of zero or less (`You have nothing to pay...`), or a body that does not fit
///   `BillPaymentRequest`.
/// - 401: no valid bearer token.
/// - 404: no account with the ID.
/// - 500: the account, the transactions or the store could not be read or written.
#[utoipa::path(
    impl_for = PayOperation,
    post,
    path = "/api/v1/bill-payments",
    operation_id = "cobil00c_pay",
    tag = "COBIL00C",
    request_body = BillPaymentRequest,
    responses(
        (
            status = 200,
            description = "The screen after ENTER: the balance with the confirmation prompt, the \
                cleared screen after `N`, or the cleared screen with the new transaction ID \
                after a payment",
            body = BillPaymentResult
        ),
        (
            status = 400,
            description = "A blank account ID, an invalid confirmation, a balance of zero or \
                less, or a body that does not fit `BillPaymentRequest`",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No valid bearer token",
            body = ScreenError
        ),
        (
            status = 404,
            description = "No account with the ID",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The account, the transactions or the store could not be read or \
                written",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn pay(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidJson(request): ValidJson<BillPaymentRequest>,
) -> Result<Json<BillPaymentResult>, ScreenError> {
    services::cobil00c::pay(&state, &user, request)
        .await
        .map(BillPaymentResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
