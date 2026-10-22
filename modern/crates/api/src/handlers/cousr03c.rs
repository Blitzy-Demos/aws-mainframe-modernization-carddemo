//! COUSR03C, user delete: the handler of endpoint 20, `DELETE /api/v1/users/{user_id}`, admin
//! only (`[A]`; transaction `CU03`, map `COUSR3A`; AAP 0.3.4.1).
//!
//! The endpoint serves one key, F5 (`F5=Delete`), the `DELETE-USER-INFO` paragraph
//! (COUSR03C-016 to COUSR03C-023). ENTER's fetch is endpoint 18, the user read COUSR03C shares
//! with COUSR02C, served by `handlers::cousr02c::fetch` on the same path; F3, F4 and F12 send no
//! request and belong to the screen (COUSR03C-006 to COUSR03C-008). `api::app::router()`
//! registers this handler in the one `routes!` of `/api/v1/users/{user_id}`, beside the
//! `GET` and `PUT` of `handlers::cousr02c`.
//!
//! The handler extracts the admin gate and the raw path ID, calls
//! `services::cousr03c::delete` and maps its answer through `ScreenResult::from` and
//! `ScreenError::from` (Thin Request Handlers). Every edit, message and status is the domain's
//! and `crate::error`'s: a failed delete keeps the program's `Unable to Update User...`,
//! `Update` included (COUSR03C-022).

use axum::Json;
use axum::extract::{Path, State};

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::ScreenResult;
use crate::error::ScreenError;
use crate::services;

/// Delete a user (CU03 / COUSR03C)
///
/// F5 deletes the user whose ID is in the path, the ID the screen fetched through the shared
/// user read (endpoint 18) or typed afresh: the screen sends it as it stands and asks no other
/// confirmation (COUSR03C-023). The path `user_id` is the raw text of the user ID input; it is
/// padded to its 8-byte key, neither upper-cased nor trimmed, and a blank one is rejected before
/// any read (COUSR03C-016, COUSR03C-017). A deleted user answers with the program's message,
/// such as `User USER0001 has been deleted ...` (COUSR03C-019, COUSR03C-020).
///
/// F4, F12 and F3 are handled by the screen and call no endpoint. An administrator token is
/// required: the caller acts only as the verified `AdminUser`, and nothing compares it with the
/// target, so an administrator may delete any user, their own included (COUSR03C-002,
/// COUSR03C-024).
///
/// # Errors
///
/// A `ScreenError` with its audited status:
///
/// - 400 `User ID can NOT be empty...` on `user_id` for a blank path ID (COUSR03C-016);
/// - 401 without a valid bearer token and 403 for a token whose user type is not `A`, from the
///   `AdminUser` extractor, before the service runs (COUSR03C-001, COUSR03C-002);
/// - 404 `User ID NOT found...` on `user_id` for a delete that affects no row (COUSR03C-021);
/// - 500 `Unable to Update User...` on `first_name` for a failed delete, and for a path ID with
///   no row or a failed read, which leave nothing held for the delete (COUSR03C-018,
///   COUSR03C-022); 500 with the API's fixed text when the store transaction cannot begin,
///   commit or roll back.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{user_id}",
    operation_id = "cousr03c_delete",
    tag = "COUSR03C",
    params(
        ("user_id" = String, Path, description = "User ID as typed on the screen (8 characters)")
    ),
    responses(
        (
            status = 200,
            description = "User deleted; the message names the deleted user ID",
            body = ScreenResult,
            example = json!({"message": "User USER0001 has been deleted ..."})
        ),
        (
            status = 400,
            description = "Blank user ID: `User ID can NOT be empty...`",
            body = ScreenError,
            example = json!({"message": "User ID can NOT be empty...", "field": "user_id"})
        ),
        (
            status = 401,
            description = "No valid bearer token",
            body = ScreenError
        ),
        (
            status = 403,
            description = "The token's user type is not `A` (administrator)",
            body = ScreenError
        ),
        (
            status = 404,
            description = "The delete affected no row: `User ID NOT found...`",
            body = ScreenError,
            example = json!({"message": "User ID NOT found...", "field": "user_id"})
        ),
        (
            status = 500,
            description = "The delete failed, or found nothing held to delete: \
                           `Unable to Update User...`; or the store transaction failed",
            body = ScreenError,
            example = json!({"message": "Unable to Update User...", "field": "first_name"})
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn delete(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<String>,
) -> Result<Json<ScreenResult>, ScreenError> {
    services::cousr03c::delete(&state, &admin, &user_id)
        .await
        .map(ScreenResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
