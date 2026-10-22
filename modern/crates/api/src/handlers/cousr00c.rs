//! Endpoint 16, `GET /api/v1/users` \[A\]: the user list of transaction CU00, program COUSR00C,
//! map `COUSR0A` (AAP 0.3.4.1).
//!
//! ENTER lists from the search field and F8 and F7 page forward and backward, ten rows by
//! default. The `U` and `D` row selections, which transfer to COUSR02C and COUSR03C, and F3 only
//! navigate and are the screen's, so they reach no endpoint. `POST` on the same path is
//! `handlers::cousr01c::add`, registered with this handler on the same path item.
//!
//! - **Admin only.** The `AdminUser` extractor is the gate: 401 without a valid bearer token,
//!   403 for a user type other than `A`, before the query is read (COUSR00C-001). The search
//!   value and the echoed paging state are request data and never identity (AAP 0.5.1).
//! - **Bounded Pagination.** `page_size` is validated by `ValidQuery` (1 to 100, 422 outside)
//!   and reaches the service untouched; the service defaults it to the map's ten rows.
//! - **No passwords.** A row carries the user ID, names and type only (COUSR00C-028), and this
//!   module logs nothing (Cardholder Data Redaction).
//! - **Thin Request Handlers.** The handler extracts, calls `services::cousr00c::list` once and
//!   maps the page through `UserListResponse::from` and a failure through `ScreenError::from`.

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::ValidQuery;
use crate::dto::cousr00c::{UserListQuery, UserListResponse};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`list`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ListOperation;

/// List users (CU00 / COUSR00C)
///
/// ENTER lists from `start_user_id`, or from the first user ID when it is absent or blank. F8
/// (`direction` forward) and F7 (`direction` backward) page from the state the previous
/// response returned, echoed as `page_key`, `first_key`, `last_key`, `page_no` and
/// `has_next_page`. A page holds `page_size` row positions, 10 by default; a row carries the
/// user ID, first and last name and user type, never a password. The `U` and `D` row selections
/// and F3 are handled by the screen and send no request. An administrator token is required.
///
/// # Errors
///
/// A `ScreenError` with status 400 when a query parameter does not parse or breaks its length
/// or range rule; 401 without a valid bearer token; 403 when the token's user type is not `A`;
/// 422 when `page_size` lies outside 1 to 100; 500 when the users cannot be read.
#[utoipa::path(
    impl_for = ListOperation,
    get,
    path = "/api/v1/users",
    operation_id = "cousr00c_list",
    tag = "COUSR00C",
    params(UserListQuery),
    responses(
        (
            status = 200,
            description = "The page of users with its paging state and the program's message",
            body = UserListResponse
        ),
        (
            status = 400,
            description = "A query parameter does not parse or breaks its length or range rule",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 403,
            description = "A valid token whose user type is not `A`: the admin gate refuses it",
            body = ScreenError
        ),
        (status = 422, description = "`page_size` outside 1 to 100", body = ScreenError),
        (status = 500, description = "The users could not be read", body = ScreenError),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn list(
    State(state): State<AppState>,
    admin: AdminUser,
    ValidQuery(query): ValidQuery<UserListQuery>,
) -> Result<Json<UserListResponse>, ScreenError> {
    services::cousr00c::list(&state, &admin, query)
        .await
        .map(UserListResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
