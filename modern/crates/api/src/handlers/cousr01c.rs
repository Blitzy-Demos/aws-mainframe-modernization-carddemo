//! COUSR01C, user add: the handler of endpoint 17, `POST /api/v1/users`, admin only
//! (transaction `CU01`, map `COUSR1A`; AAP 0.3.4.1).
//!
//! ENTER is the only key the endpoint serves. F3 returns to the admin menu and F4 clears the
//! screen without a request, so both belong to the screen (COUSR01C-007, COUSR01C-008). `GET` on
//! the same path is `handlers::cousr00c::list`, and `api::app` registers the two on the same
//! path item.
//!
//! The handler extracts the [`AdminUser`] gate and the validated [`UserAddRequest`], hands both
//! to `services::cousr01c::add`, and maps its answer: the success message into a
//! [`ScreenResult`], a failure into a [`ScreenError`]. The empty-field edits, the argon2id hash
//! of the password as typed and the insert are the service's (Thin Request Handlers).
//!
//! # Identity
//!
//! The acting identity is the verified token's [`AdminUser`] alone. The body's `user_id` and
//! `user_type` describe the user being added and grant the caller nothing (AAP 0.5.1,
//! COUSR01C-026).
//!
//! # Redaction
//!
//! The body carries the new user's password. Nothing here formats, logs or returns it: the
//! request's only `Debug` prints it redacted, the success message names the user ID only, and
//! every rejection names a field, never a value (Cardholder Data Redaction).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cousr01c::UserAddRequest;
use crate::dto::{ScreenResult, ValidJson};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`add`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct AddOperation;

/// Add a user (CU01 / COUSR01C).
///
/// ENTER on `COUSR1A`: the five fields are edited in the program's order and, when none is
/// empty, the user is added and the message names its user ID, such as `User NEWUSR01 has been
/// added ...` (COUSR01C-010 to COUSR01C-014, COUSR01C-021). F4 and PF3 send no request. An
/// administrator token is required.
///
/// # Errors
///
/// A `ScreenError`, checked in this order:
///
/// - 401 without a valid bearer token (COUSR01C-002);
/// - 403 when the token's user type is not `A` (COUSR01C-001);
/// - 400 for a body that does not deserialise or a value its map field cannot hold, then for
///   the first empty field, and for a user ID already stored, `User ID already exist...`
///   (COUSR01C-010 to COUSR01C-014, COUSR01C-023);
/// - 500 `Unable to Add User...` when the user cannot be stored (COUSR01C-024).
// The taken user ID is a 400, not a 409: 409 is reserved for a record changed by another user,
// and the duplicate is input the operator must change (COUSR01C-023), so the document declares
// no 409. Success stays 200, not 201: the program answers the add with its message row.
#[utoipa::path(
    impl_for = AddOperation,
    post,
    path = "/api/v1/users",
    operation_id = "cousr01c_add",
    tag = "COUSR01C",
    request_body = UserAddRequest,
    responses(
        (
            status = 200,
            description = "The user is added; the message names its user ID (COUSR01C-021).",
            body = ScreenResult
        ),
        (
            status = 400,
            description = "A body that does not deserialise, a value its map field cannot hold, \
                the first empty field in the program's order, or a user ID already stored \
                (COUSR01C-010 to COUSR01C-014, COUSR01C-023).",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No valid bearer token (COUSR01C-002).",
            body = ScreenError
        ),
        (
            status = 403,
            description = "The token's user type is not `A` (COUSR01C-001).",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The user could not be stored (COUSR01C-024).",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn add(
    State(state): State<AppState>,
    admin: AdminUser,
    ValidJson(request): ValidJson<UserAddRequest>,
) -> Result<Json<ScreenResult>, ScreenError> {
    services::cousr01c::add(&state, &admin, request)
        .await
        .map(ScreenResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
