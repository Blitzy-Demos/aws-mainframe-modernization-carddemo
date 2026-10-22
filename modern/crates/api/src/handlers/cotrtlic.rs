//! The handlers of CTLI, COTRTLIC's transaction-type list, behind the admin gate (AAP 0.3.4.1):
//!
//! - endpoint 24, `GET /api/v1/transaction-types`: the screen load, and F7 or F8 under the filters
//!   on the screen;
//! - endpoint 25, `POST /api/v1/transaction-types/list-actions`: ENTER and F10, the program's
//!   whole selection and confirmation workflow.
//!
//! F2 (to CTTU) and F3 call no endpoint. Both operations read and write the Db2 data version of
//! the types, `db2.transaction_type`, never the VSAM version `tranrept` reads (E23).
//!
//! # Thin Request Handlers
//!
//! Each handler extracts the [`AdminUser`] first, so a missing or non-admin token is answered
//! 401 or 403 before any input is read, then the validated query or body, calls one function of
//! `services::cotrtlic` and maps what it returns through the `From` conversions of
//! `dto::cotrtlic` and [`ScreenError`]. The ENTER edit order, the F10 commit conditions and the
//! E28 selection handling are the service's and the domain's: a handler never reads `aid`,
//! `rows` or `state`.
//!
//! # State and paging
//!
//! `TranTypeListState` is echoed by the client unchanged, in the body of endpoint 25 and as the
//! opaque `page_key` of endpoint 24. It carries no authority: the acting identity is the
//! verified token, and the service re-runs every edit and re-reads every row it commits (AAP
//! 0.5.1). Both operations are Bounded Pagination collections: `page_size`, 1 to 100 and 7 when
//! absent, is checked by the extractor, which answers 422 outside that range, and reaches the
//! service untouched.
//!
//! # Status
//!
//! COTRTLIC re-sends the whole screen with every message (`COTRTLIC.cbl:699-720`), so an input
//! error, a row another user deleted and a Db2 failure still answer with the page and the
//! state. A request that gets no screen is answered with a [`ScreenError`] instead: 400 for a
//! query or body the extractor rejects and for a `page_key` or `state` no screen could hold;
//! 500 for a database the service cannot reach, the failed `9998-PRIMING-QUERY`, carrying
//! `Db2 access failure. ` in the Db2 format (COTRTLIC-053), and, on endpoint 25, for an F10
//! commit whose transaction cannot be opened, committed or rolled back or whose re-read of the
//! target row fails.
//!
//! `TranTypeListResponse` and `TranTypeListActionResult` carry the audited status and answer
//! with it through their own `IntoResponse`, so each handler returns the DTO itself and never
//! wraps it in `Json`, which would answer 200 for every outcome. 400 and 500 therefore carry
//! either the page or a [`ScreenError`], and each operation declares both statuses as `anyOf`
//! the two schemas, inline through the schema-only `TranTypeListErrorBody` and
//! `TranTypeListActionErrorBody`; `anyOf` rather than `oneOf`, because a page with its message
//! also satisfies the [`ScreenError`] schema. 200, and endpoint 25's 404, carry the page alone;
//! 401, 403 and 422 carry the [`ScreenError`] alone.

use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cotrtlic::{
    TranTypeListActionErrorBody, TranTypeListActionRequest, TranTypeListActionResult,
    TranTypeListErrorBody, TranTypeListQuery, TranTypeListResponse,
};
use crate::dto::{ValidJson, ValidQuery};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`list`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ListOperation;

/// List transaction types (CTLI / COTRTLIC).
///
/// Endpoint 24: seven rows of the Db2 data version of the transaction types,
/// `db2.transaction_type`, with the page number, the messages, the `state` the screen echoes with
/// ENTER and F10, and the opaque `page_key` it echoes with F7 and F8. An administrator token is
/// required.
///
/// Without `direction` the request is the screen load: page 1 under no filter, every other member
/// ignored. With `direction` and the `page_key` of the previous response it is F7 (`backward`) or
/// F8 (`forward`) under `type_filter` and `desc_filter`, the filters on the screen, whose edits run
/// as on ENTER, so a changed filter moves to page 1 of it. F2 (add, to CTTU) and F3 (exit) call no
/// endpoint.
///
/// # Errors
///
/// 400 with the page for a filter in error, or with a `ScreenError` for a query the request rules
/// reject or a `page_key` this API did not issue; 401 without a valid token; 403 for a token whose
/// user type is not `A`; 422 for a `page_size` outside 1 to 100; 500 with the page and its Db2
/// message when the count or the page read fails, or with a `ScreenError` carrying
/// `Db2 access failure. ` when the database cannot be reached (COTRTLIC-053).
#[utoipa::path(
    impl_for = ListOperation,
    get,
    path = "/api/v1/transaction-types",
    operation_id = "cotrtlic_list",
    tag = "COTRTLIC",
    params(TranTypeListQuery),
    responses(
        (
            status = 200,
            description = "The page: rows, page number, messages, `state` and `page_key`",
            body = TranTypeListResponse
        ),
        (
            status = 400,
            description = "Either body: the page with a filter in error and its message, or a \
                `ScreenError` for a query the request rules reject or a `page_key` this API did \
                not issue",
            body = inline(TranTypeListErrorBody)
        ),
        (
            status = 401,
            description = "No bearer token, or one that is invalid or expired",
            body = ScreenError
        ),
        (
            status = 403,
            description = "The token's user type is not `A` (admin gate)",
            body = ScreenError
        ),
        (
            status = 422,
            description = "`page_size` outside 1 to 100 (Bounded Pagination)",
            body = ScreenError
        ),
        (
            status = 500,
            description = "Either body: the page, with the rows read before the failure and the \
                program's Db2 message, when the count or the page read failed, or a \
                `ScreenError` carrying `Db2 access failure. ` when the database cannot be reached",
            body = inline(TranTypeListErrorBody)
        ),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn list(
    State(state): State<AppState>,
    admin: AdminUser,
    ValidQuery(query): ValidQuery<TranTypeListQuery>,
) -> Result<TranTypeListResponse, ScreenError> {
    // The DTO answers with its own audited status; `Json` would make every outcome a 200.
    services::cotrtlic::list(&state, &admin, query)
        .await
        .map(TranTypeListResponse::from)
        .map_err(ScreenError::from)
}

/// The `utoipa::Path` of [`list_actions`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct ListActionsOperation;

/// Validate or confirm list actions (CTLI / COTRTLIC).
///
/// Endpoint 25: the key, the two filters, the seven selections and descriptions as typed, and the
/// `state` of the previous response echoed unchanged. It answers with the page as re-read, or the
/// state's rows where nothing was read, the per-row errors, the messages, what was committed and
/// the new `state`. An administrator token is required.
///
/// ENTER (`aid` `ENTER`) edits the selections against the state as received, then the
/// description filter, then the type filter. A valid single `U` or `D` returns the confirmation
/// prompt with the action pending in `state`; a changed filter re-reads page 1 of it and keeps
/// every selection at its row position.
///
/// F10 (`aid` `PF10`) commits the pending update or delete only when one is pending and the type
/// filter, the description filter and the selected row are all unchanged; otherwise it is
/// processed exactly as ENTER. F2 (add, to CTTU) and F3 (exit) call no endpoint.
///
/// # Errors
///
/// 400 with the page and the per-row errors for an input error or a delete the type's categories
/// restrict, or with a `ScreenError` for a body the request rules reject or a `state` no screen
/// could hold; 401 without a valid token; 403 for a token whose user type is not `A`; 404 with the
/// page when the row to commit was deleted by another user; 422 for a `page_size` outside 1 to
/// 100; 500 with the page and its Db2 message when a Db2 statement fails, with a `ScreenError`
/// carrying `Db2 access failure. ` when the database cannot be reached (COTRTLIC-053), or with a
/// `ScreenError` when the commit's transaction cannot be opened, committed or rolled back or its
/// re-read of the target row fails.
// The static `list-actions` segment coexists with endpoint 26's `{type_code}` parameter: axum's
// matcher prefers the static segment, so this POST never reaches COTRTUPC.
#[utoipa::path(
    impl_for = ListActionsOperation,
    post,
    path = "/api/v1/transaction-types/list-actions",
    operation_id = "cotrtlic_list_actions",
    tag = "COTRTLIC",
    request_body = TranTypeListActionRequest,
    responses(
        (
            status = 200,
            description = "The page, the messages, what was committed and the new `state`",
            body = TranTypeListActionResult
        ),
        (
            status = 400,
            description = "Either body: the page with the per-row errors and the message of an \
                input error or of a delete the type's categories restrict, or a `ScreenError` for \
                a body the request rules reject or a `state` no screen could hold",
            body = inline(TranTypeListActionErrorBody)
        ),
        (
            status = 401,
            description = "No bearer token, or one that is invalid or expired",
            body = ScreenError
        ),
        (
            status = 403,
            description = "The token's user type is not `A` (admin gate)",
            body = ScreenError
        ),
        (
            status = 404,
            description = "The page and the message of a row another user deleted before the \
                commit",
            body = TranTypeListActionResult
        ),
        (
            status = 422,
            description = "`page_size` outside 1 to 100 (Bounded Pagination)",
            body = ScreenError
        ),
        (
            status = 500,
            description = "Either body: the page and the program's Db2 message of a failed \
                statement, or a `ScreenError` carrying `Db2 access failure. ` when the database \
                cannot be reached, or `Unable to process request...` when the commit's \
                transaction cannot be opened, committed or rolled back or its re-read of the \
                target row fails",
            body = inline(TranTypeListActionErrorBody)
        ),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn list_actions(
    State(state): State<AppState>,
    admin: AdminUser,
    ValidJson(request): ValidJson<TranTypeListActionRequest>,
) -> Result<TranTypeListActionResult, ScreenError> {
    // The DTO answers with its own audited status; `Json` would make every outcome a 200.
    services::cotrtlic::list_actions(&state, &admin, request)
        .await
        .map(TranTypeListActionResult::from)
        .map_err(ScreenError::from)
}
