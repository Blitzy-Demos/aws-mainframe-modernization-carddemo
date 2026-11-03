//! CTLI and CTTU: the list, add, update and delete of the Db2 transaction
//! type master `CARDDEMO.TRANSACTION_TYPE`
//! [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L1-4], table
//! `transaction_types`.
//!
//! | Operation | Transaction | Program | Rule |
//! |---|---|---|---|
//! | `GET /admin/transaction-types` | CTLI | `COTRTLIC` | `tran_type_list::list` |
//! | `POST /admin/transaction-types` | CTTU | `COTRTUPC` | `tran_type_update::add` |
//! | `PUT /admin/transaction-types/{id}` | CTTU | `COTRTUPC` | `tran_type_update::update` |
//! | `PUT /admin/transaction-types/{id}?origin=list` | CTLI | `COTRTLIC` | `tran_type_list::update_description` |
//! | `DELETE /admin/transaction-types/{id}` | CTLI | `COTRTLIC` | `tran_type_list::delete` |
//!
//! # One update, two programs
//!
//! CTTU and CTLI share `PUT /admin/transaction-types/{id}`. Without `origin`
//! the request is the `COTRTUP` maintenance screen: the key edits, the read,
//! the no-change compare, the description edits and the F5 write of
//! `COTRTUPC`, with its texts. With `origin=list` it is the inline update of
//! a listed row, the `U` selector and its F10 save in `COTRTLIC`
//! (`1211-EDIT-ARRAY-DESC`, `9200-UPDATE-RECORD`), with that program's texts.
//!
//! # What these resources leave out
//!
//! They leave out only the alternate `COTRTUPC` delete message set:
//! `Delete successful.` (P-COTRTUPC-004) and its failure texts
//! (P-COTRTUPC-005). The `COTRTUP` F4 delete is served by the one `DELETE`,
//! with the `COTRTLIC` `9300-DELETE-RECORD` texts (P-COTRTUPC-028); which set
//! that `DELETE` answers with is the user's decision under E-23 (open), so no
//! second delete endpoint exists here. The add is `POST`, which `COTRTUPC`
//! reaches with F5 on a key not found. The map's `F6=Add` legend has no
//! executable path: it stays dark and `0001-CHECK-PFKEYS` never accepts F6
//! (P-COTRTUPC-001, P-COTRTUPC-026).
//!
//! # Units of work
//!
//! Every write is one SQL statement on the Db2 master, as each COBOL write is
//! (section B of `docs/parity/parity-oracle-audit.md` records each as a
//! single write). The unit of work commits when the rule succeeds, the
//! COBOL's `EXEC CICS SYNCPOINT` after `SQLCODE` 0, and rolls back when it
//! fails, which undoes nothing: the failed statement changed no row. The list
//! reads through the cursors and writes nothing, so its unit of work is
//! always rolled back.
//!
//! Only `transaction_types` is written. The reports read the processing copy
//! `trantype_file`, which shows an edit made here only after
//! `batch seed --only tranextr` (TRANEXTR).
//!
//! # Identity and telemetry
//!
//! Every route takes [`AdminUser`]: no token is 401 and a `usr_type` `U`
//! token 403, before any input is read. The COBOL kept users out of these
//! programs only through the admin menu. The admin's ID from the verified
//! token is used for telemetry only. Each request runs in a `handler` span
//! with `transaction` (`CTLI` or `CTTU`), `user_id`, `type_code`, `origin`
//! and, once the unit of work has ended, `outcome`: `ok`, or the error code
//! of the response. `type_code` is the path code, the code of an add's body
//! or the type code filter of a list, recorded only once
//! [`check_type_code_key`] has passed it, so it is at most `X(2)`. A code
//! that check rejects is never recorded: `key_rejected` holds the error's
//! code and `key_len` its length in characters instead.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use carddemo_domain::error::DomainError;
use carddemo_domain::numeric::TranTypeCode;
use carddemo_domain::rules::{tran_type_list, tran_type_update};
use carddemo_persistence::UnitOfWork;
use tracing::{Instrument, Span};

use crate::auth::AdminUser;
use crate::dto::admin_transaction_types::{
    TranTypeCreateRequest, TranTypeListPageQuery, TranTypeListQuery, TranTypeMessageResponse,
    TranTypeOrigin, TranTypeResponse, TranTypeUpdateQuery, TranTypeUpdateRequest,
    check_type_code_key,
};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::{Page, PageQuery};
use crate::state::AppState;

/// The CICS transaction of the type list, `COTRTLIC`.
const CTLI: &str = "CTLI";

/// The CICS transaction of the type maintenance screen, `COTRTUPC`.
const CTTU: &str = "CTTU";

/// The `outcome` a span records for a request that succeeded.
const OUTCOME_OK: &str = "ok";

/// Lists the transaction types that pass the filters: one page from
/// `start_key` in `direction` (CTLI ENTER, F8 and F7).
///
/// COTRTLIC 1200-EDIT-INPUTS, 1290-CROSS-EDITS, 8000-READ-FORWARD,
/// 8100-READ-BACKWARDS, 2500-SETUP-MESSAGE — P-COTRTLIC-003, P-COTRTLIC-004,
/// P-COTRTLIC-022, P-COTRTLIC-030 to P-COTRTLIC-034, P-COTRTLIC-036,
/// V-COTRTLIC-001, V-COTRTLIC-002, V-COTRTLIC-004, V-COTRTLIC-005,
/// V-COTRTLIC-008, V-COTRTLIC-009, V-COTRTLIC-024, V-COTRTLIC-026,
/// V-COTRTLIC-028; 2600-SEND-SCREEN — P-COTRTLIC-029: the HTTP response
/// replaces the 3270 send of the `CTRTLIA` map, a mandated deviation.
///
/// A page holds `page_size` rows, seven (`WS-MAX-SCREEN-LINES`) when absent,
/// in ascending type code order. Without `start_key` both directions read
/// the first page from the start of the table: backward is F7 on the first
/// page, which re-reads that page with `No previous pages to display`
/// (P-COTRTLIC-004, V-COTRTLIC-009). An empty result and a page boundary are
/// 200 with the COBOL text in `message` (E-54).
///
/// # Errors
///
/// - 400 with an empty message for a filter longer than its BMS field, on
///   `TRTYPE` or `TRDESC`, and for a `start_key` longer than two characters
///   or not in ASCII, on no field; 400 for a malformed query; and 400 for a
///   type code filter that is not two digits, on `TRTYPE` (V-COTRTLIC-001).
///   The filters are checked first, then the start key, before the master
///   is read.
/// - 401 without a valid token; 403 for a token that is not an admin's.
/// - 404 when no type passes the filters (V-COTRTLIC-002), a description
///   filter holding a character outside ASCII included, which is searched as
///   entered.
/// - 500 with the cursor's Db2 text when the read fails (V-COTRTLIC-024,
///   V-COTRTLIC-026), with the no-records text when the filter count fails
///   (V-COTRTLIC-028), and with `Unexpected error` when the database cannot
///   be reached.
#[utoipa::path(
    get,
    path = "/admin/transaction-types",
    tag = "admin-transaction-types",
    params(TranTypeListQuery, TranTypeListPageQuery),
    responses(
        (status = 200, description = "One page of transaction types, with the page-boundary text in `message` where the COBOL shows one", body = Page<TranTypeResponse>),
        (status = 400, description = "A filter is longer than its field, the start key is longer than its field or not ASCII, the type code filter is not two digits, or the query is malformed", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 404, description = "No transaction type passes the filters", body = ErrorBody),
        (status = 500, description = "The Db2 master could not be read", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_tran_types(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiQuery(filters): ApiQuery<TranTypeListQuery>,
    ApiQuery(page): ApiQuery<TranTypeListPageQuery>,
) -> Result<Json<Page<TranTypeResponse>>, ApiError> {
    let span = handler_span(CTLI, &admin, None);
    record_entered_type_code(&span, filters.type_code.as_deref());
    traced(span, async move {
        filters.check()?;
        page.check()?;
        let (start_key, direction, page_size) =
            PageQuery::from(page).resolve(tran_type_list::SCREEN_ROWS);
        let input = filters.into_input(start_key, direction, page_size);
        let mut uow = state.db().begin().await?;
        let result = tran_type_list::list(&mut uow.store(), input).await;
        roll_back(uow, result)
            .await
            .map(|listed| Json(listed.into()))
    })
    .await
}

/// Adds a transaction type: the `COTRTUP` F5 on a key not found, with the
/// edits of the new details (CTTU).
///
/// COTRTUPC 1200-EDIT-MAP-INPUTS, 9000-READ-TRANTYPE, 9600-WRITE-PROCESSING,
/// 9700-INSERT-RECORD — P-COTRTUPC-006, P-COTRTUPC-007, P-COTRTUPC-009,
/// P-COTRTUPC-031, V-COTRTUPC-012 to V-COTRTUPC-021
///
/// The key is edited first, then read: a code already on file is refused.
/// The description is edited next, and the write updates first and inserts
/// on `SQLCODE` +100, as `9600-WRITE-PROCESSING` does. A stored type is
/// committed and answered 201 with `Changes committed to database`.
///
/// # Errors
///
/// - 400 for a member longer than its BMS field or a malformed body, and for
///   the key and description edits, on `TRTYPCD` or `TRTYDSC`
///   (V-COTRTUPC-012 to V-COTRTUPC-014, V-COTRTUPC-017, V-COTRTUPC-018).
/// - 401 without a valid token; 403 for a token that is not an admin's.
/// - 409 with no message when a type already has the code (V-COTRTUPC-015,
///   E-51).
/// - 500 with the Db2 text when the read or the write fails (V-COTRTUPC-016,
///   V-COTRTUPC-019 to V-COTRTUPC-021); nothing is written.
#[utoipa::path(
    post,
    path = "/admin/transaction-types",
    tag = "admin-transaction-types",
    request_body = TranTypeCreateRequest,
    responses(
        (status = 201, description = "The type was added; `message` is the COBOL commit text", body = TranTypeMessageResponse),
        (status = 400, description = "The type code or the description fails its edit", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 409, description = "A transaction type already has the code", body = ErrorBody),
        (status = 500, description = "The Db2 master could not be read or written", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_tran_type(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiJson(body): ApiJson<TranTypeCreateRequest>,
) -> Result<(StatusCode, Json<TranTypeMessageResponse>), ApiError> {
    let span = handler_span(CTTU, &admin, None);
    record_entered_type_code(&span, body.type_code.as_deref());
    traced(span, async move {
        body.check()?;
        let input = body.into_input();
        let mut uow = state.db().begin().await?;
        // `add` rather than `edit` then `upsert`: the audit ranks the read
        // between the key edit and the description edit, and only the rule
        // that also reads can keep that order.
        let result = tran_type_update::add(&mut uow.store(), &input).await;
        let saved = commit_or_roll_back(uow, result).await?;
        Ok((StatusCode::CREATED, Json(saved.into())))
    })
    .await
}

/// Updates the description of the type `{id}`: without `origin` the
/// `COTRTUP` maintenance screen (CTTU), with `origin=list` the inline update
/// of a listed row (CTLI).
///
/// Without `origin`: COTRTUPC 9000-READ-TRANTYPE, 1200-EDIT-MAP-INPUTS,
/// 1205-COMPARE-OLD-NEW, 9600-WRITE-PROCESSING — P-COTRTUPC-007,
/// P-COTRTUPC-008, P-COTRTUPC-015 to P-COTRTUPC-021, V-COTRTUPC-001 to
/// V-COTRTUPC-011. A request that repeats the stored values writes nothing
/// and is 200 with `No change detected with respect to values fetched.`
/// (V-COTRTUPC-006); a change is committed with `Changes committed to
/// database`.
///
/// With `origin=list`: COTRTLIC 1211-EDIT-ARRAY-DESC, 1240-EDIT-ALPHANUM-REQD,
/// 9200-UPDATE-RECORD — P-COTRTLIC-009, P-COTRTLIC-010, P-COTRTLIC-013,
/// P-COTRTLIC-017, P-COTRTLIC-021, V-COTRTLIC-012 to V-COTRTLIC-014,
/// V-COTRTLIC-016 to V-COTRTLIC-018. The update is committed with
/// `HIGHLIGHTED row was updated`, or with `No change detected with respect to
/// database values.` when only the case changed, which `COTRTLIC` still
/// writes (V-COTRTLIC-012, E-73). Either answer also carries `info`, the info
/// line `HIGHLIGHTED row was updated` that `9200-UPDATE-RECORD` sets once the
/// update is stored (P-COTRTLIC-009); without `origin` there is no `info`.
///
/// # Errors
///
/// - 400 for a key or description longer than its BMS field, a malformed
///   query or body, and the key and description edits: on `TRTYPCD` or
///   `TRTYDSC` without `origin` (V-COTRTUPC-001 to V-COTRTUPC-003,
///   V-COTRTUPC-007, V-COTRTUPC-008), on no field with `origin=list`
///   (V-COTRTLIC-013, V-COTRTLIC-014), where a key holding a character
///   outside ASCII is also refused with an empty message. A description
///   holding one gets the alphanumeric edit's text with either origin.
/// - 401 without a valid token; 403 for a token that is not an admin's.
/// - 404 when no type has the code (V-COTRTUPC-004, V-COTRTLIC-016).
/// - 500 with the Db2 or lock text when the read or the update fails
///   (V-COTRTUPC-005, V-COTRTUPC-009 to V-COTRTUPC-011, V-COTRTLIC-017,
///   V-COTRTLIC-018); nothing is written.
#[utoipa::path(
    put,
    path = "/admin/transaction-types/{id}",
    tag = "admin-transaction-types",
    params(
        ("id" = String, Path, description = "The type code, `TR_TYPE`: two ASCII characters", max_length = 2, pattern = "^[\\x00-\\x7F]*$"),
        TranTypeUpdateQuery,
    ),
    request_body = TranTypeUpdateRequest,
    responses(
        (status = 200, description = "The description was stored, or nothing changed; `message` is the COBOL text, and with `origin=list` `info` is the COTRTLIC info line", body = TranTypeMessageResponse),
        (status = 400, description = "The type code or the description fails its edit", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 404, description = "No transaction type has the code", body = ErrorBody),
        (status = 500, description = "The Db2 master could not be read or updated", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_tran_type(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<TranTypeUpdateQuery>,
    ApiJson(body): ApiJson<TranTypeUpdateRequest>,
) -> Result<Json<TranTypeMessageResponse>, ApiError> {
    let transaction = match query.origin {
        None => CTTU,
        Some(TranTypeOrigin::List) => CTLI,
    };
    let span = handler_span(transaction, &admin, query.origin);
    traced(span.clone(), async move {
        let response = match query.origin {
            None => update_via_cttu(&state, &span, id, body).await?,
            Some(TranTypeOrigin::List) => update_via_ctli(&state, &span, &id, body).await?,
        };
        Ok(Json(response))
    })
    .await
}

/// `PUT /admin/transaction-types/{id}` without `origin`: the `COTRTUPC`
/// fetch, edits and F5 write in one request.
///
/// COTRTUPC 9000-READ-TRANTYPE, 1200-EDIT-MAP-INPUTS, 9600-WRITE-PROCESSING —
/// P-COTRTUPC-007, P-COTRTUPC-015 to P-COTRTUPC-021, V-COTRTUPC-001 to
/// V-COTRTUPC-011
async fn update_via_cttu(
    state: &AppState,
    span: &Span,
    id: String,
    body: TranTypeUpdateRequest,
) -> Result<TranTypeMessageResponse, ApiError> {
    checked_type_code(span, &id, None)?;
    body.check(None)?;
    let input = body.into_upsert_input(id);
    let mut uow = state.db().begin().await?;
    // `update` rather than `edit` then `upsert`, as in `create_tran_type`:
    // the read and the no-change compare come between the key edit and the
    // description edit.
    let result = tran_type_update::update(&mut uow.store(), &input).await;
    commit_or_roll_back(uow, result).await.map(Into::into)
}

/// `PUT /admin/transaction-types/{id}?origin=list`: the `COTRTLIC` inline
/// description update of the listed row `{id}`.
///
/// COTRTLIC 1211-EDIT-ARRAY-DESC, 9200-UPDATE-RECORD — P-COTRTLIC-009,
/// P-COTRTLIC-010, V-COTRTLIC-012 to V-COTRTLIC-018
async fn update_via_ctli(
    state: &AppState,
    span: &Span,
    id: &str,
    body: TranTypeUpdateRequest,
) -> Result<TranTypeMessageResponse, ApiError> {
    let origin = Some(TranTypeOrigin::List);
    checked_type_code(span, id, origin)?;
    body.check(origin)?;
    let row = body.into_list_update(id);
    let mut uow = state.db().begin().await?;
    let result =
        tran_type_list::update_description(&mut uow.store(), row.code, &row.description).await;
    commit_or_roll_back(uow, result).await.map(Into::into)
}

/// Deletes the listed type `{id}`: the `D` selector of the `COTRTLI` list,
/// confirmed with F10 (CTLI).
///
/// COTRTLIC 9300-DELETE-RECORD — P-COTRTLIC-007, P-COTRTLIC-008,
/// V-COTRTLIC-021 to V-COTRTLIC-023
///
/// The confirmation is the dialog of the SPA. A deleted type is committed
/// and answered with `HIGHLIGHTED row deleted.Hit Enter to continue` and no
/// type.
///
/// # Errors
///
/// - 400 for a key longer than the type code `X(2)` or holding a character
///   outside ASCII, or a malformed path.
/// - 401 without a valid token; 403 for a token that is not an admin's.
/// - 404 with the delete-failed Db2 text when no type has the code
///   (V-COTRTLIC-022).
/// - 409 with the child-records Db2 text when categories still reference the
///   type (SQLCODE -532, V-COTRTLIC-021); the type stays.
/// - 500 with the delete-failed Db2 text on any other failure
///   (V-COTRTLIC-023).
#[utoipa::path(
    delete,
    path = "/admin/transaction-types/{id}",
    tag = "admin-transaction-types",
    params(
        ("id" = String, Path, description = "The type code, `TR_TYPE`: two ASCII characters", max_length = 2, pattern = "^[\\x00-\\x7F]*$"),
    ),
    responses(
        (status = 200, description = "The type was deleted; `message` is the COBOL text and `transaction_type` is null", body = TranTypeMessageResponse),
        (status = 400, description = "The type code is longer than two characters or not ASCII", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an admin's", body = ErrorBody),
        (status = 404, description = "No transaction type has the code", body = ErrorBody),
        (status = 409, description = "Transaction categories still reference the type", body = ErrorBody),
        (status = 500, description = "The Db2 master could not be updated", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_tran_type(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<TranTypeMessageResponse>, ApiError> {
    let span = handler_span(CTLI, &admin, None);
    traced(span.clone(), async move {
        checked_type_code(&span, &id, Some(TranTypeOrigin::List))?;
        // The `COTRTLIC` delete, not the `COTRTUPC` F4 delete with its own
        // texts: which of the two message sets this endpoint answers with is
        // open for the user (E-23), and the list's delete is the one the
        // CTLI resource carries.
        let code = TranTypeCode::move_from(&id);
        let mut uow = state.db().begin().await?;
        let result = tran_type_list::delete(&mut uow.store(), code).await;
        let deleted = commit_or_roll_back(uow, result).await?;
        Ok(Json(deleted.into()))
    })
    .await
}

/// The span of one request: its CICS `transaction`, the admin's ID and the
/// `origin` of an update. `type_code`, or `key_rejected` and `key_len` in
/// its place, is recorded by [`record_type_code`], and `outcome` by
/// [`traced`].
fn handler_span(
    transaction: &'static str,
    admin: &AdminUser,
    origin: Option<TranTypeOrigin>,
) -> Span {
    tracing::info_span!(
        "handler",
        transaction,
        user_id = admin.user_id.as_str(),
        type_code = tracing::field::Empty,
        key_rejected = tracing::field::Empty,
        key_len = tracing::field::Empty,
        origin = origin.map(origin_name),
        outcome = tracing::field::Empty,
    )
}

/// Records on `span` the type code `code`, which [`check_type_code_key`]
/// answered `checked` for. A code that passed is recorded as `type_code` as
/// entered, at most `X(2)`. Of a rejected code only the error's `code`
/// (`key_rejected`) and the code's length in characters (`key_len`) are
/// recorded, never its text: a rejected code can be as long as the request
/// allows and hold anything.
fn record_type_code(span: &Span, code: &str, checked: &Result<(), ApiError>) {
    match checked {
        Ok(()) => span.record("type_code", code),
        Err(error) => span
            .record("key_rejected", error.body().code.as_str())
            .record("key_len", code.chars().count()),
    };
}

/// [`check_type_code_key`] of the `{id}` path code `code` for `origin`, the
/// check that decides the request, with its result recorded on `span` by
/// [`record_type_code`].
///
/// # Errors
///
/// The check's 400, unchanged.
fn checked_type_code(
    span: &Span,
    code: &str,
    origin: Option<TranTypeOrigin>,
) -> Result<(), ApiError> {
    let checked = check_type_code_key(code, origin);
    record_type_code(span, code, &checked);
    checked
}

/// Records the type code of a list filter or of an add's body on `span`,
/// through [`check_type_code_key`] on that member alone, which holds it to
/// the same `X(2)` as the filter and body checks. That check only bounds the
/// telemetry: the request's own check edits every member and answers, so
/// the error recorded here is dropped. An absent code records nothing.
fn record_entered_type_code(span: &Span, code: Option<&str>) {
    if let Some(code) = code {
        record_type_code(span, code, &check_type_code_key(code, None));
    }
}

/// The `origin` query value, as the span records it.
const fn origin_name(origin: TranTypeOrigin) -> &'static str {
    match origin {
        TranTypeOrigin::List => "list",
    }
}

/// Runs `work` inside `span`, then records its outcome on the span: `ok`, or
/// the `code` of the error body. The work ends its own unit of work, so the
/// outcome is recorded after the commit or rollback.
async fn traced<T>(
    span: Span,
    work: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    let result = work.instrument(span.clone()).await;
    match &result {
        Ok(_) => span.record("outcome", OUTCOME_OK),
        Err(error) => span.record("outcome", error.body().code.as_str()),
    };
    result
}

/// Ends the unit of work of a write: commits it when the rule succeeded, the
/// COBOL `SYNCPOINT`, and rolls it back when the rule failed.
///
/// # Errors
///
/// The rule's error, or a 500 when the commit fails, in which case nothing
/// was stored.
async fn commit_or_roll_back<T>(
    uow: UnitOfWork,
    result: Result<T, DomainError>,
) -> Result<T, ApiError> {
    match result {
        Ok(value) => {
            uow.commit().await?;
            Ok(value)
        }
        Err(error) => {
            discard(uow).await;
            Err(error.into())
        }
    }
}

/// Ends the unit of work of a read, which wrote nothing: rolls it back
/// whatever the rule answered.
///
/// # Errors
///
/// The rule's error.
async fn roll_back<T>(uow: UnitOfWork, result: Result<T, DomainError>) -> Result<T, ApiError> {
    discard(uow).await;
    result.map_err(Into::into)
}

/// Rolls `uow` back. A failed `ROLLBACK` is only logged: the unit of work
/// holds no write to keep, the server rolls the transaction back when its
/// connection closes, and the response is the rule's answer either way.
async fn discard(uow: UnitOfWork) {
    if let Err(error) = uow.rollback().await {
        tracing::warn!(error = %error, "rollback of the unit of work failed");
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use axum::http::StatusCode;
    use serde_json::Value;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Span, Subscriber};
    use utoipa::IntoParams;
    use utoipa_axum::router::OpenApiRouter;
    use utoipa_axum::routes;

    use super::{CTLI, CTTU, checked_type_code, handler_span, record_entered_type_code};
    use crate::auth::{AdminUser, AuthUser, UserType};
    use crate::dto::admin_transaction_types::TranTypeOrigin;
    use crate::error::ApiError;
    use crate::pagination::PageQuery;
    use crate::state::AppState;

    /// The `OpenAPI` document of the four operations, registered as the route
    /// registry registers them: one `routes!` per path. Registering them also
    /// proves each function is an axum handler over [`AppState`].
    fn document() -> Value {
        let (_router, api) = OpenApiRouter::<AppState>::new()
            .routes(routes!(super::list_tran_types, super::create_tran_type))
            .routes(routes!(super::update_tran_type, super::delete_tran_type))
            .split_for_parts();
        serde_json::to_value(api).expect("the document serializes")
    }

    /// The operation `method` of `path`.
    fn operation<'a>(document: &'a Value, path: &str, method: &str) -> &'a Value {
        let operation = &document["paths"][path][method];
        assert!(operation.is_object(), "{method} {path} is declared");
        operation
    }

    /// The response statuses an operation declares, in ascending order.
    fn statuses(operation: &Value) -> Vec<String> {
        let mut statuses: Vec<String> = operation["responses"]
            .as_object()
            .expect("responses")
            .keys()
            .cloned()
            .collect();
        statuses.sort();
        statuses
    }

    /// The names and locations of an operation's parameters.
    fn parameters(operation: &Value) -> Vec<(String, String)> {
        operation["parameters"]
            .as_array()
            .map(|parameters| {
                parameters
                    .iter()
                    .map(|parameter| {
                        (
                            parameter["name"].as_str().expect("name").to_owned(),
                            parameter["in"].as_str().expect("in").to_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_four_operations_are_declared_on_their_two_paths() {
        let document = document();
        let paths = document["paths"].as_object().expect("paths");
        assert_eq!(
            paths.keys().map(String::as_str).collect::<Vec<_>>(),
            ["/admin/transaction-types", "/admin/transaction-types/{id}"]
        );
        for (path, method, operation_id, expected) in [
            (
                "/admin/transaction-types",
                "get",
                "list_tran_types",
                &["200", "400", "401", "403", "404", "500"][..],
            ),
            (
                "/admin/transaction-types",
                "post",
                "create_tran_type",
                &["201", "400", "401", "403", "409", "500"][..],
            ),
            (
                "/admin/transaction-types/{id}",
                "put",
                "update_tran_type",
                &["200", "400", "401", "403", "404", "500"][..],
            ),
            (
                "/admin/transaction-types/{id}",
                "delete",
                "delete_tran_type",
                &["200", "400", "401", "403", "404", "409", "500"][..],
            ),
        ] {
            let operation = operation(&document, path, method);
            assert_eq!(operation["operationId"], operation_id);
            assert_eq!(
                operation["tags"],
                serde_json::json!(["admin-transaction-types"])
            );
            assert_eq!(
                operation["security"],
                serde_json::json!([{"bearer_auth": []}])
            );
            assert_eq!(statuses(operation), expected, "{method} {path}");
        }
    }

    #[test]
    fn every_error_response_is_the_error_body() {
        let document = document();
        for (path, item) in document["paths"].as_object().expect("paths") {
            for (method, operation) in item.as_object().expect("operations") {
                for (status, response) in operation["responses"].as_object().expect("responses") {
                    if status.starts_with('2') {
                        continue;
                    }
                    assert_eq!(
                        response["content"]["application/json"]["schema"]["$ref"],
                        "#/components/schemas/ErrorBody",
                        "{method} {path} {status}"
                    );
                }
            }
        }
    }

    #[test]
    fn parameters_and_bodies_are_the_dto_types() {
        let document = document();
        let list = operation(&document, "/admin/transaction-types", "get");
        assert_eq!(
            parameters(list),
            [
                ("type_code", "query"),
                ("description", "query"),
                ("start_key", "query"),
                ("direction", "query"),
                ("page_size", "query"),
            ]
            .map(|(name, place)| (name.to_owned(), place.to_owned()))
        );
        // The description filter, fifty characters of any text the search
        // takes as entered; the start key, two characters in ASCII.
        let list_parameters = list["parameters"].as_array().expect("parameters");
        let description = &list_parameters[1]["schema"];
        assert_eq!(description["maxLength"], 50);
        assert!(description.get("pattern").is_none(), "{description}");
        let start_key = &list_parameters[2];
        assert_eq!(start_key["required"], false);
        assert_eq!(start_key["schema"]["type"], "string");
        assert_eq!(start_key["schema"]["maxLength"], 2);
        assert_eq!(start_key["schema"]["pattern"], "^[\\x00-\\x7F]*$");
        // `direction` and `page_size` keep the schemas `PageQuery` gives them.
        let shared = serde_json::to_value(PageQuery::into_params(|| None)).expect("serializes");
        let shared = &shared.as_array().expect("an array")[1..];
        for (published, page_query) in list_parameters[3..].iter().zip(shared) {
            assert_eq!(published["name"], page_query["name"]);
            assert_eq!(published["required"], page_query["required"]);
            assert_eq!(published["schema"], page_query["schema"], "{published}");
        }

        let create = operation(&document, "/admin/transaction-types", "post");
        assert!(parameters(create).is_empty());
        assert_eq!(
            create["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/TranTypeCreateRequest"
        );
        assert_eq!(
            create["responses"]["201"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/TranTypeMessageResponse"
        );

        let update = operation(&document, "/admin/transaction-types/{id}", "put");
        assert_eq!(
            parameters(update),
            [("id", "path"), ("origin", "query")]
                .map(|(name, place)| (name.to_owned(), place.to_owned()))
        );
        assert_eq!(
            update["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/TranTypeUpdateRequest"
        );
        // The answer of both origins, `info` included with `origin=list`.
        assert_eq!(
            update["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/TranTypeMessageResponse"
        );

        let delete = operation(&document, "/admin/transaction-types/{id}", "delete");
        assert_eq!(parameters(delete), [("id".to_owned(), "path".to_owned())]);
        // The {id} of both operations: two characters, in ASCII.
        for operation in [update, delete] {
            let id = &operation["parameters"][0];
            assert_eq!(id["required"], true);
            assert_eq!(id["schema"]["type"], "string");
            assert_eq!(id["schema"]["maxLength"], 2);
            assert_eq!(id["schema"]["pattern"], "^[\\x00-\\x7F]*$");
        }
        assert!(delete.get("requestBody").is_none());
        assert_eq!(
            delete["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/TranTypeMessageResponse"
        );
    }

    /// Text no valid code holds, so finding it in telemetry means a rejected
    /// code leaked.
    const MARKER: &str = "SPAN-LEAK-MARKER";

    /// A subscriber that keeps every value recorded on its spans and events,
    /// as text, in the order it was recorded.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<(&'static str, String)>>>);

    impl Capture {
        fn lock(&self) -> MutexGuard<'_, Vec<(&'static str, String)>> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Every value recorded so far.
        fn values(&self) -> Vec<(&'static str, String)> {
            self.lock().clone()
        }

        /// Every value recorded under `field`, in the order it was recorded,
        /// so a field recorded twice shows twice.
        fn values_of(&self, field: &str) -> Vec<String> {
            self.lock()
                .iter()
                .filter(|(name, _)| *name == field)
                .map(|(_, value)| value.clone())
                .collect()
        }
    }

    /// Writes each value a span or event carries into the capture.
    struct Collect<'a>(&'a mut Vec<(&'static str, String)>);

    impl Visit for Collect<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push((field.name(), value.to_owned()));
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.push((field.name(), value.to_string()));
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0.push((field.name(), format!("{value:?}")));
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            span.record(&mut Collect(&mut self.lock()));
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, values: &Record<'_>) {
            values.record(&mut Collect(&mut self.lock()));
        }

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            event.record(&mut Collect(&mut self.lock()));
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    /// One handler's type code: the span it opens and the code step it runs
    /// on that span first. `gates` is whether the step's result answers the
    /// request.
    struct Site {
        route: &'static str,
        transaction: &'static str,
        origin: Option<&'static str>,
        span: fn(&AdminUser) -> Span,
        step: fn(&Span, &str) -> Result<(), ApiError>,
        gates: bool,
    }

    fn sites() -> [Site; 5] {
        [
            Site {
                route: "GET /admin/transaction-types?type_code",
                transaction: CTLI,
                origin: None,
                span: |admin| handler_span(CTLI, admin, None),
                step: |span, code| {
                    record_entered_type_code(span, Some(code));
                    Ok(())
                },
                gates: false,
            },
            Site {
                route: "POST /admin/transaction-types",
                transaction: CTTU,
                origin: None,
                span: |admin| handler_span(CTTU, admin, None),
                step: |span, code| {
                    record_entered_type_code(span, Some(code));
                    Ok(())
                },
                gates: false,
            },
            Site {
                route: "PUT /admin/transaction-types/{id}",
                transaction: CTTU,
                origin: None,
                span: |admin| handler_span(CTTU, admin, None),
                step: |span, code| checked_type_code(span, code, None),
                gates: true,
            },
            Site {
                route: "PUT /admin/transaction-types/{id}?origin=list",
                transaction: CTLI,
                origin: Some("list"),
                span: |admin| handler_span(CTLI, admin, Some(TranTypeOrigin::List)),
                step: |span, code| checked_type_code(span, code, Some(TranTypeOrigin::List)),
                gates: true,
            },
            Site {
                route: "DELETE /admin/transaction-types/{id}",
                transaction: CTLI,
                origin: None,
                span: |admin| handler_span(CTLI, admin, None),
                step: |span, code| checked_type_code(span, code, Some(TranTypeOrigin::List)),
                gates: true,
            },
        ]
    }

    fn admin() -> AdminUser {
        AdminUser(AuthUser {
            user_id: "ADMIN001".to_owned(),
            usr_type: UserType::Admin,
        })
    }

    #[test]
    fn a_type_code_is_recorded_once_checked_and_a_rejected_one_only_by_code_and_length() {
        let overlong = format!("01{MARKER}");
        let overlong_len = overlong.chars().count().to_string();
        // (code, recorded as `type_code`, rejected with this length): a code
        // of `X(2)` or less, blank included, is kept as entered; a longer one,
        // counted in characters, is not.
        let cases = [
            ("01", Some("01"), None),
            ("", Some(""), None),
            (overlong.as_str(), None, Some(overlong_len.as_str())),
            ("ÄB1", None, Some("3")),
        ];
        for site in sites() {
            for (code, recorded, rejected_len) in cases {
                let capture = Capture::default();
                let result = tracing::subscriber::with_default(capture.clone(), || {
                    let span = (site.span)(&admin());
                    (site.step)(&span, code)
                });
                let case = format!("{} with {code:?}", site.route);
                // Each field holds exactly the values recorded under it, so a
                // second recording of any of them fails the case.
                assert_eq!(
                    capture.values_of("transaction"),
                    [site.transaction],
                    "{case}"
                );
                assert_eq!(capture.values_of("user_id"), ["ADMIN001"], "{case}");
                assert_eq!(
                    capture.values_of("origin"),
                    site.origin.as_slice(),
                    "{case}"
                );
                assert_eq!(
                    capture.values_of("type_code"),
                    recorded.as_slice(),
                    "{case}"
                );
                assert_eq!(
                    capture.values_of("key_rejected"),
                    rejected_len.map(|_| "INVALID").as_slice(),
                    "{case}"
                );
                assert_eq!(
                    capture.values_of("key_len"),
                    rejected_len.as_slice(),
                    "{case}"
                );
                assert!(
                    capture
                        .values()
                        .iter()
                        .all(|(_, value)| !value.contains(MARKER)),
                    "{case}: {:?}",
                    capture.values()
                );
                match result {
                    Err(error) if site.gates && rejected_len.is_some() => {
                        assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{case}");
                        assert_eq!(error.body().code, "INVALID", "{case}");
                    }
                    Ok(()) if !site.gates || rejected_len.is_none() => {}
                    other => panic!("{case}: the code step answered {other:?}"),
                }
            }
        }
    }

    #[test]
    fn an_absent_entered_type_code_records_no_key() {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            record_entered_type_code(&handler_span(CTLI, &admin(), None), None);
        });
        for field in ["type_code", "key_rejected", "key_len"] {
            assert_eq!(capture.values_of(field), Vec::<String>::new(), "{field}");
        }
        assert_eq!(capture.values_of("transaction"), [CTLI]);
    }
}
