//! The handlers of endpoints 26 and 27, transaction-type maintenance (transaction `CTTU`,
//! program COTRTUPC, map `CTRTUPA`, admin only; AAP 0.3.4.1):
//!
//! - [`fetch`], `GET /api/v1/transaction-types/{type_code}`: ENTER on the fresh screen, the read
//!   of the type the code names (COTRTUPC-004 to COTRTUPC-011).
//! - [`action`], `POST /api/v1/transaction-types/{type_code}/actions`: ENTER (validate), PF5
//!   (save: an update, or an insert when no row holds the code) and PF4 (the delete the screen
//!   has asked to confirm) (COTRTUPC-016 to COTRTUPC-034, COTRTUPC-043).
//!
//! F3, F12, the first F4 and F5 on a type no row holds are the screen's and send no request
//! (COTRTUPC-039 to COTRTUPC-041, COTRTUPC-044).
//!
//! Both endpoints read and write the Db2 data version only, `db2.transaction_type`. Nothing
//! copies it into the VSAM data version that `tranrept` reads, since TRANEXTR is out of scope, so
//! a maintenance change never reaches the report lookups (COTRTUPC-002; AAP 0.3.3.4, E23).
//!
//! # Thin Request Handlers
//!
//! Each handler extracts the [`AdminUser`] (the admin gate, COTRTUPC-001), the raw
//! `{type_code}` and, for [`action`], the validated body, calls one function of
//! `crate::services::cotrtupc`, and maps its answer through the `From` conversions of
//! `crate::dto::cotrtupc` and [`ScreenError`]. The key and description edits, the state served
//! for each key, the update-then-insert fallback and the reading of a delete refused by the
//! categories' `ON DELETE RESTRICT` are the service's and the domain's. The path parameter is
//! checked for its map length only ([`TypeCodePath`], `COTRTUP-013`) and otherwise passed on
//! unparsed, so a malformed code gets the program's own message. A handler never reads the key
//! or a state from the body to decide anything: the screen's state carries no authority, and the
//! service derives the state it serves from the key and the row it re-reads (AAP 0.5.1).
//!
//! # Statuses
//!
//! 400 for an edit, an unserved key, a code longer than its map field or a body the extractor
//! rejects; 401 without a valid token; 403 for a token whose user type is not `A`; 500 for every
//! store failure, the delete refused while categories exist (`Please delete associated child
//! records first:`) included. Its Db2 text after that prefix is `HALTED(H3)` (E19). Neither
//! endpoint answers 404 or 409: a code no row holds is a 200 offering the add path
//! (COTRTUPC-010), and the changed-by-another-user text is never set (COTRTUPC-025).
//!
//! No value these endpoints handle is protected, and nothing here logs (Cardholder Data
//! Redaction).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::ValidJson;
use crate::dto::cotrtupc::{
    TranTypeActionRequest, TranTypeActionResult, TranTypeView, TypeCodePath,
};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`fetch`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct FetchOperation;

/// Fetch a transaction type (CTTU / COTRTUPC).
///
/// ENTER on the fresh screen: `1000-PROCESS-INPUTS` stores and edits the code typed in
/// `TRTYPCDI`, and `9000-READ-TRANTYPE` reads the row it names from `db2.transaction_type`. A row
/// found is shown with its description in state `show_details` (COTRTUPC-009); a valid code no
/// row holds answers 200 in state `details_not_found` with the error row `No record found for
/// this key in database` and the prompt to add it with F5 (COTRTUPC-010).
///
/// Requires the bearer token of an administrator, user type `A` (COTRTUPC-001).
///
/// # Errors
///
/// - 400 naming `type_code` when the code is longer than its two-character map field
///   (`COTRTUP-013`) or the key edits refuse it: blank, not numeric or zero (COTRTUPC-005 to
///   COTRTUPC-007). Nothing is read.
/// - 401 without a valid token; 403 for a token whose user type is not `A`.
/// - 500 when the read fails, with the program's fixed message prefix (COTRTUPC-011).
// The responses are exactly the statuses COTRTUPC's audit assigns: a code no row holds is a 200
// that starts the add path, never a 404 (COTRTUPC-010).
#[utoipa::path(
    impl_for = FetchOperation,
    get,
    path = "/api/v1/transaction-types/{type_code}",
    operation_id = "cotrtupc_fetch",
    tag = "COTRTUPC",
    params(
        (
            "type_code" = String,
            Path,
            max_length = 2,
            description = "Transaction type code as typed on the screen (2 characters)"
        )
    ),
    responses(
        (
            status = 200,
            description = "The type shown, or the add prompt for a code no row holds",
            body = TranTypeView
        ),
        (
            status = 400,
            description = "The code is longer than 2 characters, or the key edits refused it",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No valid bearer token",
            body = ScreenError
        ),
        (
            status = 403,
            description = "The token's user type is not A",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The read of the transaction type failed",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn fetch(
    State(state): State<AppState>,
    admin: AdminUser,
    path: TypeCodePath,
) -> Result<Json<TranTypeView>, ScreenError> {
    services::cotrtupc::fetch(&state, &admin, &path.type_code)
        .await
        .map(TranTypeView::from)
        .map(Json)
        .map_err(ScreenError::from)
}

/// The `utoipa::Path` of [`action`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ActionOperation;

/// Save or delete a transaction type (CTTU / COTRTUPC).
///
/// `{type_code}` is the code the screen shows, and `aid` the key pressed:
///
/// - `ENTER`: `1200-EDIT-MAP-INPUTS` edits the description typed in `TRTYDSCI`. A valid change
///   answers state `changes_ok_not_confirmed` with the prompt to save it with F5; nothing is
///   written (COTRTUPC-016 to COTRTUPC-021).
/// - `PF5`: `9600-WRITE-PROCESSING` saves the validated description, an update of the row, or
///   the insert of `9700-INSERT-RECORD` when no row holds the code, and commits it; the answer is
///   state `changes_okayed_and_done` (COTRTUPC-023 to COTRTUPC-026).
/// - `PF4`: the second F4, after the screen asked to confirm the delete: `9800-DELETE-PROCESSING`
///   deletes the row and commits; the answer is state `delete_done` (COTRTUPC-032).
///
/// Any other key is answered `Invalid key pressed`, with nothing read or written (COTRTUPC-043).
/// The service re-reads the row for `{type_code}` and derives the state it serves from it; no
/// state the screen holds is sent or trusted.
///
/// Requires the bearer token of an administrator, user type `A` (COTRTUPC-001).
///
/// # Errors
///
/// - 400 for an unserved key, a code longer than 2 or a description longer than 50 characters,
///   a body that does not deserialise, and the key, description and no-change edits
///   (COTRTUPC-005, COTRTUPC-017 to COTRTUPC-019, COTRTUPC-043). Nothing is written.
/// - 401 without a valid token; 403 for a token whose user type is not `A`.
/// - 500 when a store statement fails, with the program's fixed message prefix and nothing
///   written: the update, insert, lock and delete failures, the delete refused while categories
///   exist (COTRTUPC-027 to COTRTUPC-029, COTRTUPC-033, COTRTUPC-034), and the failed re-read
///   (COTRTUPC-038).
// No 404 and no 409: an update finding no row falls back to the insert (COTRTUPC-026), and the
// changed-by-another-user text is never set (COTRTUPC-025).
#[utoipa::path(
    impl_for = ActionOperation,
    post,
    path = "/api/v1/transaction-types/{type_code}/actions",
    operation_id = "cotrtupc_action",
    tag = "COTRTUPC",
    params(
        (
            "type_code" = String,
            Path,
            max_length = 2,
            description = "Transaction type code as typed on the screen (2 characters)"
        )
    ),
    request_body = TranTypeActionRequest,
    responses(
        (
            status = 200,
            description = "The screen the step leaves: validated, saved or deleted",
            body = TranTypeActionResult
        ),
        (
            status = 400,
            description = "An edit refused the input, or the key is not served",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No valid bearer token",
            body = ScreenError
        ),
        (
            status = 403,
            description = "The token's user type is not A",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A store statement failed, the delete refused by categories included",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn action(
    State(state): State<AppState>,
    admin: AdminUser,
    path: TypeCodePath,
    ValidJson(request): ValidJson<TranTypeActionRequest>,
) -> Result<Json<TranTypeActionResult>, ScreenError> {
    services::cotrtupc::act(&state, &admin, &path.type_code, request)
        .await
        .map(TranTypeActionResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
