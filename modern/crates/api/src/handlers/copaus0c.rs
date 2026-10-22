//! COPAUS0C, the pending-authorization summary: the handler of endpoint 21,
//! `GET /api/v1/accounts/{acct_id}/authorizations` (transaction `CPVS`, map `COPAU0A`; AAP
//! 0.3.4.1).
//!
//! ENTER with an account ID returns the customer block, the pending-authorization summary block
//! and the first page of the account's pending authorization details; F8 pages forward from the
//! displayed page's last key and F7 back from the first key the screen saved for the page before
//! (COPAUS0C-008, COPAUS0C-041, COPAUS0C-047). The `S` selection that transfers to COPAU01 and
//! PF3 run in the screen and make no request.
//!
//! A Bounded Pagination collection over the pending authorization details: `page_size` from 1
//! to 100, 5 rows when absent, a value outside that range refused with 422 by [`ValidQuery`]
//! before the service runs. The handler passes the validated query on untouched.
//!
//! The account travels as the raw path segment: the service's domain edit answers a blank or
//! non-numeric value with the program's own message, and the domain, never this handler,
//! converts the edited `PicU<11>` account into the signed `PicS<11>` summary key (COPAUS0C-005,
//! COPAUS0C-006, COPAUS0C-027).
//!
//! The handler extracts the identity, the path segment and the validated query, calls
//! `services::copaus0c::summary` and maps its outcome into [`AuthSummaryResponse`] or a
//! [`ScreenError`] (Thin Request Handlers). [`SignedOnUser`] is extracted before the path and the
//! query, so a request without a valid token is refused with 401 before any input is judged
//! (n5). No response holds a card number, full or redacted (COPAUS0C-055).

use axum::Json;
use axum::extract::{Path, State};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidQuery;
use crate::dto::copaus0c::{AuthSummaryQuery, AuthSummaryResponse};
use crate::error::ScreenError;
use crate::services;

/// List pending authorizations for an account (CPVS / COPAUS0C)
///
/// ENTER, a request without `page_key`, shows the account's customer, its pending-authorization
/// summary block and the first page of its pending authorizations in key order. F8 sends the
/// displayed page's `last_key` with `direction` forward, F7 the first key saved for the page
/// before with `direction` backward. Without a summary the block reports `summary_found` false
/// and the list is empty. The page holds `page_size` rows, 5 when absent. The row selection and
/// PF3 run in the screen.
///
/// # Errors
///
/// A `ScreenError` with 400 for a blank or non-numeric account ID, a malformed query or a
/// `page_key` that is not an authorization key; 401 without a valid bearer token; 404 when the
/// account has no cross-reference, account or customer record; 422 for a `page_size` outside 1
/// to 100; 500 when a read fails, with the program's system-error text.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{acct_id}/authorizations",
    operation_id = "copaus0c_summary",
    tag = "COPAUS0C",
    params(
        (
            "acct_id" = String,
            Path,
            description = "Account ID as typed on the screen (11 characters)"
        ),
        AuthSummaryQuery,
    ),
    responses(
        (
            status = 200,
            description = "The customer and summary block and one page of the account's pending \
                           authorizations",
            body = AuthSummaryResponse
        ),
        (
            status = 400,
            description = "A blank or non-numeric account ID, a malformed query or a `page_key` \
                           that is not an authorization key",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 404,
            description = "The account has no cross-reference, account or customer record",
            body = ScreenError
        ),
        (
            status = 422,
            description = "`page_size` outside 1 to 100",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A read failed: the program's system-error text",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn summary(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path(acct_id): Path<String>,
    ValidQuery(query): ValidQuery<AuthSummaryQuery>,
) -> Result<Json<AuthSummaryResponse>, ScreenError> {
    services::copaus0c::summary(&state, &user, &acct_id, query)
        .await
        .map(AuthSummaryResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
