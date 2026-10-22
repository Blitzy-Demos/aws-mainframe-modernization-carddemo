//! Endpoint 7, `GET /api/v1/cards`: the card list of transaction `CCLI`, program COCRDLIC, map
//! `CCRDLIA`, screen route `/cards` (AAP 0.3.4.1).
//!
//! ENTER sends the account and card filters; F7 and F8 page backward and forward, the screen
//! echoing the paging state of the previous response. A row selection (`S` for detail, `U` for
//! update) calls no endpoint: the screen navigates to `/cards/view` or `/cards/update` with the
//! row's `acct_id` and `card_ref`, and those screens call endpoints 8 and 9 with that handle.
//!
//! The handler follows Thin Request Handlers: it extracts the verified identity and the
//! validated query, calls `services::cocrdlic::list` once and maps the outcome through
//! `CardListResponse::from` and `ScreenError::from`. Every filter edit, read, paging decision
//! and message belongs to the service and to `domain::online::cocrdlic`.
//!
//! # Bounded Pagination
//!
//! A collection endpoint: `page_size` admits 1 to 100 and defaults to 7, the program's screen
//! size. `ValidQuery` answers any other value with 422 before the service runs, and an admitted
//! value reaches the service unchanged.
//!
//! # Cardholder Data Redaction
//!
//! Every row carries the card's `cards.card_ref` handle and its card number redacted to the last
//! four digits, and the paging keys are handles too (E32). The card filter is request input: the
//! handler neither logs nor echoes it.

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidQuery;
use crate::dto::cocrdlic::{CardListQuery, CardListResponse};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`list`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ListOperation;

/// List cards (CCLI / COCRDLIC).
///
/// One page of the card list in ascending card-number order. ENTER applies the account and card
/// filters (`acct_id`, `card_num`); F7 and F8 page through `direction`, with the paging state of
/// the previous response echoed unchanged (`page_no`, `first_key`, `last_key`, `has_next_page`,
/// `last_page_shown`). Row selections are client-side and call no endpoint. Card numbers are
/// redacted to their last four digits, and every row carries its `card_ref` handle. A search
/// that matches no card is a 200 page with the program's no-records message.
///
/// # Errors
///
/// - 400: the query does not deserialise or breaks a field rule, or a filter fails the
///   program's edit, with the program's message and the field it names.
/// - 401: the request carries no valid bearer token.
/// - 422: `page_size` lies outside 1 to 100.
/// - 500: a card read failed, answered with the program's file-error text, or the request failed
///   internally.
#[utoipa::path(
    impl_for = ListOperation,
    get,
    path = "/api/v1/cards",
    operation_id = "cocrdlic_list",
    tag = "COCRDLIC",
    params(CardListQuery),
    responses(
        (
            status = 200,
            description = "One page of the card list: at most `page_size` rows, each with its \
                `card_ref` handle and redacted card number, the page number, the paging state \
                to echo and both message lines.",
            body = CardListResponse
        ),
        (
            status = 400,
            description = "The query does not deserialise or breaks a field rule, or a filter \
                is neither blank nor all digits; the body names the field.",
            body = ScreenError
        ),
        (
            status = 401,
            description = "The request carries no valid bearer token.",
            body = ScreenError
        ),
        (
            status = 422,
            description = "Returned only when `page_size` is outside 1 to 100 \
                (Bounded Pagination).",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A card read failed, answered with the program's file-error text, \
                or the request failed internally.",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
// The identity extractor precedes the query, so a request without a valid token is answered
// 401 before its query is deserialised or validated.
pub(crate) async fn list(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidQuery(query): ValidQuery<CardListQuery>,
) -> Result<Json<CardListResponse>, ScreenError> {
    services::cocrdlic::list(&state, &user, query)
        .await
        .map(CardListResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
