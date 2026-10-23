//! Endpoints 18 and 19 of CU02 / COUSR02C, both admin-only (`[A]`):
//!
//! - Endpoint 18, `GET /api/v1/users/{user_id}` ([`fetch`]): ENTER, the user read. CU03 /
//!   COUSR03C shares it, because the two programs' read paragraphs carry identical messages
//!   (COUSR02C-010 to COUSR02C-016, COUSR03C-010 to COUSR03C-014).
//! - Endpoint 19, `PUT /api/v1/users/{user_id}` ([`update`]): PF5, and the save half of F3
//!   (`F3=Save&Exit`), the rewrite (COUSR02C-006, COUSR02C-018 to COUSR02C-032).
//!
//! F4 (clear), F12 (cancel) and the navigation after F3 send no request. The delete on the same
//! path is `handlers::cousr03c::delete`.
//!
//! Each handler extracts, calls its one `services::cousr02c` function and maps the outcome
//! through the `From` conversions of `crate::dto` and `crate::error` (Thin Request Handlers).
//! The path `user_id` is a `UserIdPath`, revalidated against the map `LENGTH` of `USRIDIN`
//! (COUSR02-013) and passed on as typed: the service's domain edit answers a blank one with the
//! program's own message. The caller's authority comes from the `AdminUser` token alone; the
//! path and the body only name the target user (AAP 0.5.1).
//!
//! No password leaves either endpoint: `UserView` has no password property, and the optional
//! write-only password of `UserUpdateRequest` reaches the service unformatted, which verifies it
//! against the stored hash and re-hashes it only when it differs (`E27`, Cardholder Data
//! Redaction).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cousr02c::{UserIdPath, UserUpdateRequest, UserView};
use crate::dto::{ScreenResult, ValidJson, ValidPath};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`fetch`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct FetchOperation;

/// Fetch a user (CU02 / COUSR02C and CU03 / COUSR03C)
///
/// ENTER on the update screen and on the delete screen: the ID, names and type stored for the
/// path `user_id`, with the prompt each screen shows after the read (`update_prompt` for
/// COUSR02C, `delete_prompt` for COUSR03C). The password and its hash are never returned. An
/// administrator token (`usr_type` `A`) is required.
///
/// # Errors
///
/// - 400 `User ID can NOT be empty...` (`field` `user_id`) for a blank user ID, and the API's
///   fixed text naming `user_id` for one longer than 8 bytes or not printable ASCII.
/// - 401 without a valid bearer token; 403 for a token whose user type is not `A`.
/// - 404 `User ID NOT found...` (`field` `user_id`) when no user has the ID.
/// - 500 `Unable to lookup User...` (`field` `first_name`) when the read fails.
#[utoipa::path(
    impl_for = FetchOperation,
    get,
    path = "/api/v1/users/{user_id}",
    operation_id = "cousr02c_fetch",
    tag = "COUSR02C",
    params(UserIdPath),
    responses(
        (status = 200, description = "The stored user, without its password", body = UserView),
        (
            status = 400,
            description = "Blank user ID, or one that does not fit the screen field",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (status = 403, description = "The token's user type is not A", body = ScreenError),
        (status = 404, description = "No user has this ID", body = ScreenError),
        (status = 500, description = "The read failed", body = ScreenError)
    ),
    security(("bearer_auth" = []))
)]
// `AdminUser` precedes the path so the gate answers 401 or 403 before any input is examined.
pub(crate) async fn fetch(
    State(state): State<AppState>,
    admin: AdminUser,
    ValidPath(path): ValidPath<UserIdPath>,
) -> Result<Json<UserView>, ScreenError> {
    services::cousr02c::fetch(&state, &admin, &path.user_id)
        .await
        .map(UserView::from)
        .map(Json)
        .map_err(ScreenError::from)
}

/// The `utoipa::Path` of [`update`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct UpdateOperation;

/// Update a user (CU02 / COUSR02C)
///
/// PF5 (`F5=Save`), and the save F3 (`F3=Save&Exit`) performs before the screen leaves: the
/// names and type of the user under the path `user_id` are rewritten when any of them, or the
/// password, differs from the stored row. An omitted or null `password` keeps the stored one; a
/// supplied one is checked against the stored hash and replaces it only when it differs. ENTER
/// is the fetch, `GET /api/v1/users/{user_id}`; F4 (clear) and F12 (cancel) send no request.
/// No password is returned. An administrator token (`usr_type` `A`) is required.
///
/// # Errors
///
/// - 400 for a path user ID or a body that does not fit the screen fields, for the first
///   empty-field edit that fails, in the program's order (user ID, first name, last name, a
///   supplied password, user type), and `Please modify to update ...` when nothing differs from
///   the stored row.
/// - 401 without a valid bearer token; 403 for a token whose user type is not `A`.
/// - 404 `User ID NOT found...` (`field` `user_id`) when no user has the ID.
/// - 500 `Unable to lookup User...` or `Unable to Update User...` (`field` `first_name`) when
///   the read, the password check or the rewrite fails.
/// - 503, with `Retry-After` in seconds, when the server's password checking is saturated, so
///   nothing was rewritten; the same request may be sent again after that delay.
#[utoipa::path(
    impl_for = UpdateOperation,
    put,
    path = "/api/v1/users/{user_id}",
    operation_id = "cousr02c_update",
    tag = "COUSR02C",
    params(UserIdPath),
    request_body = UserUpdateRequest,
    responses(
        (status = 200, description = "The user was rewritten", body = ScreenResult),
        (
            status = 400,
            description = "Invalid path or body, an empty field, or nothing modified",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (status = 403, description = "The token's user type is not A", body = ScreenError),
        (status = 404, description = "No user has this ID", body = ScreenError),
        (
            status = 500,
            description = "The read, the password check or the rewrite failed",
            body = ScreenError
        ),
        (
            status = 503,
            description = "Password checking is saturated; nothing was rewritten",
            body = ScreenError,
            headers(
                (
                    "Retry-After" = u64,
                    description = "Seconds to wait before sending the request again"
                )
            )
        )
    ),
    security(("bearer_auth" = []))
)]
// The body comes last, as axum requires, so the gate answers 401 or 403 before it is read (n5).
pub(crate) async fn update(
    State(state): State<AppState>,
    admin: AdminUser,
    ValidPath(path): ValidPath<UserIdPath>,
    ValidJson(request): ValidJson<UserUpdateRequest>,
) -> Result<Json<ScreenResult>, ScreenError> {
    services::cousr02c::update(&state, &admin, &path.user_id, request)
        .await
        .map(ScreenResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
