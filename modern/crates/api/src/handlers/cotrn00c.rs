//! Endpoint 11, `GET /api/v1/transactions`: the transaction list of COTRN00C (transaction
//! `CT00`, map `COTRN0A`; AAP 0.3.4.1), a paginated collection (Bounded Pagination).
//!
//! The handler extracts the signed-on user and the validated `TranListQuery`, calls
//! `services::cotrn00c::list` once, and maps a page through `TranListResponse::from` and a
//! failure through `ScreenError::from` (Thin Request Handlers). `page_size`, 1 to 100 with the
//! ten rows of `COTRN0A` as its default, is checked by `ValidQuery`, which answers 422 outside
//! that range, and reaches the service untouched.
//!
//! ENTER, F7, F8 and the transfer key of a first entry are served here. The `S` selection to
//! COTRN01 and F3 to the main menu are client-side and send no request. `POST` on the same path
//! is COTRN02C's add, `handlers::cotrn02c::add`, which `app::router` registers beside this
//! handler on the same path item.

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidQuery;
use crate::dto::cotrn00c::{TranListQuery, TranListResponse};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`list`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ListOperation;

/// List transactions (CT00 / COTRN00C).
///
/// ENTER searches from `start_tran_id`, or from the lowest transaction when it is blank. F8
/// (`direction` `forward`) and F7 (`backward`) page from the `last_key` and `first_key` the
/// previous response returned, which the screen echoes with `page_no` and `has_next_page`;
/// `entry_key` names the transfer that brought the screen up. Every page is re-read from
/// `transactions`, so the echoed state grants nothing. A page holds `page_size` row slots, ten
/// by default, and no row carries a card number. The `S` selection to the transaction view and
/// F3 to the main menu are client-side.
///
/// # Errors
///
/// - 401 without a valid bearer token, answered before the query is read.
/// - 400 `Tran ID must be Numeric ...` on `start_tran_id` for an ENTER whose search value is
///   neither blank nor 16 digits, and 400 for a query that does not deserialise or breaks a
///   field rule.
/// - 422 for a `page_size` outside 1 to 100.
/// - 500 `Unable to lookup transaction...` when the page cannot be read, and 500 for a store
///   failure without an audited message.
#[utoipa::path(
    impl_for = ListOperation,
    get,
    path = "/api/v1/transactions",
    operation_id = "cotrn00c_list",
    tag = "COTRN00C",
    params(TranListQuery),
    responses(
        (
            status = 200,
            description = "The page read, or the F7 or F8 boundary answer that keeps the rows \
                shown, with the paging state to echo and the message the program moved last",
            body = TranListResponse
        ),
        (
            status = 400,
            description = "`Tran ID must be Numeric ...` on `start_tran_id`, or a query that \
                does not deserialise or breaks a field rule",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No valid bearer token",
            body = ScreenError
        ),
        (
            status = 422,
            description = "`page_size` outside 1 to 100",
            body = ScreenError
        ),
        (
            status = 500,
            description = "`Unable to lookup transaction...` when the page cannot be read, or a \
                store failure without an audited message",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn list(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidQuery(query): ValidQuery<TranListQuery>,
) -> Result<Json<TranListResponse>, ScreenError> {
    services::cotrn00c::list(&state, &user, query)
        .await
        .map(TranListResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
