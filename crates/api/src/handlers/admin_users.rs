//! CU00 to CU03, user administration over the `USRSEC` file (`CSUSR01Y`
//! `SEC-USER-DATA` [app/cpy/CSUSR01Y.cpy:L17-23]): the `COUSR00C` list, the
//! `COUSR01C` add, the `COUSR02C` fetch and update and the `COUSR03C` delete.
//!
//! | Route | Handler | Transaction · program | Rule |
//! |---|---|---|---|
//! | `GET /admin/users` | [`list_users`] | CU00 · `COUSR00C` | `user_list::list` |
//! | `POST /admin/users` | [`create_user`] | CU01 · `COUSR01C` | `user_add::prepare`, `user_add::add` |
//! | `GET /admin/users/{id}` | [`get_user`] | CU02 and CU03 ENTER · `COUSR02C`, `COUSR03C` | `user_update::lookup` |
//! | `PUT /admin/users/{id}` | [`update_user`] | CU02 PF5 · `COUSR02C` | `user_update::update`, `UserUpdateCheck::compare`, `user_update::rewrite` |
//! | `DELETE /admin/users/{id}` | [`delete_user`] | CU03 PF5 · `COUSR03C` | `user_delete::delete` |
//!
//! # Access
//!
//! Every route takes [`AdminUser`]: a request without a valid bearer token is
//! 401 `Authentication required`, and a valid token whose `usr_type` is not
//! `A` is 403 `Admin access required`. The COBOL keeps regular users away
//! from these programs only through menu routing (`COSGN00C` sends a `U` user
//! to `COMEN01C`, which refuses the admin options
//! [app/cbl/COMEN01C.cbl:L136-143]); the 403 gate is a mandated deviation. The
//! acting admin is the verified token's `sub`, used only for the handler span;
//! no request field names the actor.
//!
//! # Units of work
//!
//! Each request opens one unit of work and hands its store to the rule.
//! `COUSR01C`, `COUSR02C` and `COUSR03C` each make a single `USRSEC` write
//! (section B of the parity audit: "n/a (single write)"), and a failed write
//! has already rolled back to its own savepoint. So a rule that succeeds is
//! committed, a rule that fails is rolled back, and nothing persists on any
//! failure. The list and the fetch write nothing and roll back. A failed
//! rollback is logged and never replaces the rule's answer: the server
//! discards the transaction when the connection closes.
//!
//! # Passwords (E-20, E-21)
//!
//! argon2id hashing and verification are memory-hard, CPU-bound and never
//! yield, so `user_add::prepare` (before the unit of work opens) and
//! `UserUpdateCheck::compare` (while the row read for update stays locked)
//! run on tokio's blocking pool, never on an executor thread. No response
//! type and no span field carries a password or its hash, and the request
//! bodies' `Debug` leaves the password out.

use std::future::Future;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use carddemo_domain::error::DomainError;
use carddemo_domain::rules::user_add::{self, UserAdded};
use carddemo_domain::rules::user_delete;
use carddemo_domain::rules::user_list::{self, UserListInput};
use carddemo_domain::rules::user_update::{self, UserUpdateOutcome};
use carddemo_persistence::UnitOfWork;
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::auth::AdminUser;
use crate::dto::admin_users::{
    UserAddRequest, UserDeleteField, UserListField, UserMessageResponse, UserSummary,
    UserUpdateField, UserUpdateRequest, check_user_id_key,
};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::{Page, PageQuery};
use crate::state::AppState;

/// The transaction ID of the user list, `COUSR00C`.
const CU00: &str = "CU00";

/// The transaction ID of the user add, `COUSR01C`.
const CU01: &str = "CU01";

/// The transaction ID of the user fetch and update, `COUSR02C`.
const CU02: &str = "CU02";

/// The transaction ID of the user delete, `COUSR03C`.
const CU03: &str = "CU03";

/// The handler span of a request for one user: the transaction, the acting
/// admin and the user the request names. `outcome` is recorded by
/// [`traced`] once the unit of work is committed or rolled back.
fn user_span(transaction: &'static str, admin: &AdminUser, target_user_id: Option<&str>) -> Span {
    tracing::info_span!(
        "admin_users",
        transaction,
        user_id = %admin.user_id,
        target_user_id,
        outcome = Empty,
    )
}

/// Runs `work` inside `span` and records its `outcome`: the success label the
/// work returns, or the error body's `code` (for example `NOT_FOUND`).
async fn traced<T>(
    span: &Span,
    work: impl Future<Output = Result<(T, &'static str), ApiError>>,
) -> Result<T, ApiError> {
    match work.instrument(span.clone()).await {
        Ok((response, outcome)) => {
            span.record("outcome", outcome);
            Ok(response)
        }
        Err(error) => {
            span.record("outcome", error.body().code.as_str());
            Err(error)
        }
    }
}

/// Runs an argon2 step of a rule on tokio's blocking pool.
///
/// # Errors
///
/// - The rule's own [`DomainError`], mapped through the status table.
/// - [`ApiError::unexpected`] when the blocking task does not complete (it
///   panicked, or the runtime is shutting down); no COBOL path has such a
///   failure, so no program message applies. The detail is logged.
async fn off_executor<T, F>(step: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(step).await {
        Ok(result) => result.map_err(ApiError::from),
        Err(error) => {
            tracing::error!(error = %error, "argon2 step did not complete");
            Err(ApiError::unexpected())
        }
    }
}

/// Rolls `uow` back, logging a failure instead of returning it, so the
/// answer already decided (the rule's error, or the rows a read returned)
/// stands.
async fn roll_back(uow: UnitOfWork) {
    if let Err(error) = uow.rollback().await {
        tracing::error!(error = %error, "unit of work rollback failed");
    }
}

/// CU00: one page of the user list, in user-ID order.
///
/// COUSR00C PROCESS-ENTER-KEY, PROCESS-PF7-KEY, PROCESS-PF8-KEY,
/// PROCESS-PAGE-FORWARD, PROCESS-PAGE-BACKWARD — P-COUSR00C-003 to
/// P-COUSR00C-011; V-COUSR00C-002 to V-COUSR00C-009 (E-54, E-71).
///
/// `start_key` is the `USRIDIN` search field and the page keys a previous
/// page returned; `direction=forward` is ENTER and PF8, `direction=backward`
/// PF7; `page_size` defaults to the 10 rows of the `COUSR0A` screen. A
/// page-boundary text, which `COUSR00C` shows without its error flag, comes
/// back in `message` of the 200 page, and a start key with no user at or
/// after it is an empty page with `You are at the top of the page...`. Each
/// row is the user's ID, names and type; `SEC-USR-PWD` is never listed.
///
/// # Errors
///
/// - 400 `INVALID`, field `USRIDIN`, for a start key longer than `PIC X(8)`,
///   and for a query that does not deserialize (a `page_size` outside 1 to
///   100, an unknown `direction`).
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 500 `Unable to lookup User...`, field `USRIDIN`, when the user browse
///   fails (V-COUSR00C-004, V-COUSR00C-006, V-COUSR00C-009).
#[utoipa::path(
    get,
    path = "/admin/users",
    tag = "admin-users",
    params(PageQuery),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The page of users, with the page-boundary text COUSR00C shows, if any", body = Page<UserSummary>),
        (status = 400, description = "A start key longer than 8 characters, or a malformed query", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 500, description = "The user browse failed", body = ErrorBody),
    )
)]
pub async fn list_users(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> Result<Json<Page<UserSummary>>, ApiError> {
    let span = tracing::info_span!(
        "admin_users",
        transaction = CU00,
        user_id = %admin.user_id,
        start_key = page.start_key.as_deref(),
        outcome = Empty,
    );
    traced(&span, list(&state, page)).await
}

/// The CU00 browse: the start-key width check, then `user_list::list` in a
/// unit of work that is rolled back, since the list writes nothing.
async fn list(
    state: &AppState,
    page: PageQuery,
) -> Result<(Json<Page<UserSummary>>, &'static str), ApiError> {
    if let Some(key) = page.start_key.as_deref() {
        check_user_id_key(key, UserListField::UsrIdIn)?;
    }
    let (start_key, direction, page_size) = page.resolve(user_list::SCREEN_ROWS);
    let input = UserListInput {
        start_key,
        direction,
        page_size,
    };
    let mut uow = state.db().begin().await?;
    let listed = user_list::list(&mut uow.store(), input).await;
    roll_back(uow).await;
    Ok((Json(Page::from(listed?)), "listed"))
}

/// CU01: adds a user to `USRSEC`.
///
/// COUSR01C PROCESS-ENTER-KEY, WRITE-USER-SEC-FILE — P-COUSR01C-002 to
/// P-COUSR01C-005; V-COUSR01C-001 to V-COUSR01C-007 (E-20).
///
/// The five fields are edited for emptiness in screen order (first name, last
/// name, user ID, password, user type) and stored exactly as entered: no
/// upper-casing, no user-type check. The password is stored as the argon2id
/// hash of its 8 space-padded bytes, so a user ID or password added in lower
/// or mixed case never signs on, as in the COBOL (E-20). The 201 body carries
/// the user as written and `User <id> has been added ...`.
///
/// # Errors
///
/// - 400 `EMPTY` with the empty-field message of the first blank field
///   (`FNAME`, `LNAME`, `USERID`, `PASSWD`, `USRTYPE`), or 400 `INVALID` for a
///   field longer than its BMS field or a body that does not deserialize.
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 409 `User ID already exist...`, field `USERID` (V-COUSR01C-006).
/// - 500 `Unable to Add User...`, field `FNAME`, when the write fails or the
///   password cannot be hashed (V-COUSR01C-007).
#[utoipa::path(
    post,
    path = "/admin/users",
    tag = "admin-users",
    request_body = UserAddRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "The user was added", body = UserMessageResponse),
        (status = 400, description = "A blank or over-length field, or a malformed body", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 409, description = "The user ID already exists", body = ErrorBody),
        (status = 500, description = "The user could not be added", body = ErrorBody),
    )
)]
pub async fn create_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiJson(body): ApiJson<UserAddRequest>,
) -> Result<(StatusCode, Json<UserMessageResponse>), ApiError> {
    let span = user_span(CU01, &admin, body.user_id.as_deref());
    traced(&span, add(&state, body)).await
}

/// The CU01 add: the width check, `user_add::prepare` on the blocking pool,
/// then the single `WRITE` of `user_add::add`, committed when it succeeds.
async fn add(
    state: &AppState,
    body: UserAddRequest,
) -> Result<((StatusCode, Json<UserMessageResponse>), &'static str), ApiError> {
    body.check()?;
    let input = body.into_input();
    let prepared = off_executor(move || user_add::prepare(input)).await?;
    let mut uow = state.db().begin().await?;
    let written = user_add::add(&mut uow.store(), prepared).await;
    match written {
        Ok(UserAdded { user, message }) => {
            uow.commit().await?;
            let response = Json(UserMessageResponse::new(&user, message));
            Ok(((StatusCode::CREATED, response), "added"))
        }
        Err(error) => {
            roll_back(uow).await;
            Err(error.into())
        }
    }
}

/// CU02 and CU03 ENTER: fetches one user for the edit form or the delete
/// dialog.
///
/// COUSR02C PROCESS-ENTER-KEY, READ-USER-SEC-FILE — P-COUSR02C-002,
/// P-COUSR02C-003, P-COUSR02C-007; V-COUSR02C-001 to V-COUSR02C-003, and the
/// identical `COUSR03C` fetch rows V-COUSR03C-001 to V-COUSR03C-003 (E-21,
/// E-58).
///
/// The body is the user's ID, names and type. `COUSR02C` also moves the
/// stored password into its edit field; a stored argon2id hash cannot be
/// shown, so no password is ever returned (E-21, a known difference).
///
/// # Errors
///
/// - 400 `EMPTY` `User ID can NOT be empty...`, field `USRIDIN`, for a blank
///   ID, or 400 `INVALID` for an ID longer than `PIC X(8)`.
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 404 `User ID NOT found...`, field `USRIDIN` (V-COUSR02C-002).
/// - 500 `Unable to lookup User...`, field `FNAME`, when the read fails
///   (V-COUSR02C-003).
#[utoipa::path(
    get,
    path = "/admin/users/{id}",
    tag = "admin-users",
    params(("id" = String, Path, description = "The user ID, `SEC-USR-ID`, as stored (case kept)")),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The user, without its password", body = UserSummary),
        (status = 400, description = "A blank or over-length user ID", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 404, description = "No user has the ID", body = ErrorBody),
        (status = 500, description = "The user could not be read", body = ErrorBody),
    )
)]
pub async fn get_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<UserSummary>, ApiError> {
    let span = user_span(CU02, &admin, Some(&id));
    traced(&span, fetch(&state, id)).await
}

/// The ENTER fetch: the ID width check, then `user_update::lookup` in a unit
/// of work that is rolled back, releasing the row its `READ … UPDATE` locked.
async fn fetch(
    state: &AppState,
    id: String,
) -> Result<(Json<UserSummary>, &'static str), ApiError> {
    check_user_id_key(&id, UserUpdateField::UsrIdIn)?;
    let mut uow = state.db().begin().await?;
    let found = user_update::lookup(&mut uow.store(), Some(id)).await;
    roll_back(uow).await;
    Ok((Json(UserSummary::from(&found?)), "fetched"))
}

/// CU02 PF5: updates a user's names, password and type.
///
/// COUSR02C UPDATE-USER-INFO, READ-USER-SEC-FILE, UPDATE-USER-SEC-FILE —
/// P-COUSR02C-004 to P-COUSR02C-007; V-COUSR02C-004 to V-COUSR02C-012 (E-21,
/// E-46).
///
/// The fields are edited for emptiness in screen order (user ID, first name,
/// last name, password, user type); the user is then read for update and
/// each entered field compared with the stored value. An absent `password`
/// stands for the current one: unchanged and never re-hashed. A present one
/// is compared by argon2 verification, case kept, and re-hashed when it
/// differs (E-21). When a field differs the record is rewritten and the body
/// carries `User <id> has been updated ...`. When none does, nothing is
/// written and the answer is still 200, with `Please modify to update ...`:
/// `COUSR02C` sets that text without its error flag (V-COUSR02C-010).
///
/// # Errors
///
/// - 400 `EMPTY` with the empty-field message of the first blank field
///   (`USRIDIN`, `FNAME`, `LNAME`, `PASSWD`, `USRTYPE`), or 400 `INVALID` for
///   a field or ID longer than its BMS field or a body that does not
///   deserialize.
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 404 `User ID NOT found...`, field `USRIDIN`, when the rewrite finds no
///   user (V-COUSR02C-011).
/// - 500 `Unable to Update User...`, field `FNAME`, for an unknown user, whose
///   failed read the COBOL follows with a failing rewrite (V-COUSR02C-009,
///   E-46), for a failed rewrite (V-COUSR02C-012), and when the password
///   cannot be verified or hashed. Nothing is written on any error.
#[utoipa::path(
    put,
    path = "/admin/users/{id}",
    tag = "admin-users",
    params(("id" = String, Path, description = "The user ID, `SEC-USR-ID`, as stored (case kept)")),
    request_body = UserUpdateRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The user was updated, or no field differed and nothing was written", body = UserMessageResponse),
        (status = 400, description = "A blank or over-length field, or a malformed body", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 404, description = "The rewrite found no user", body = ErrorBody),
        (status = 500, description = "The user could not be updated", body = ErrorBody),
    )
)]
pub async fn update_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<UserUpdateRequest>,
) -> Result<Json<UserMessageResponse>, ApiError> {
    let span = user_span(CU02, &admin, Some(&id));
    traced(&span, update(&state, id, body)).await
}

/// The PF5 update in three steps, inside one unit of work: `user_update::update`
/// (the empty checks and the `READ … UPDATE`), `UserUpdateCheck::compare` on
/// the blocking pool with the row still locked, then `user_update::rewrite`,
/// committed when it rewrote the record.
async fn update(
    state: &AppState,
    id: String,
    body: UserUpdateRequest,
) -> Result<(Json<UserMessageResponse>, &'static str), ApiError> {
    check_user_id_key(&id, UserUpdateField::UsrIdIn)?;
    body.check()?;
    let input = body.into_input(id.clone());
    let mut uow = state.db().begin().await?;
    let read = user_update::update(&mut uow.store(), input).await;
    let check = match read {
        Ok(check) => check,
        Err(error) => {
            roll_back(uow).await;
            return Err(error.into());
        }
    };
    let rewrite = match off_executor(move || check.compare()).await {
        Ok(rewrite) => rewrite,
        Err(error) => {
            roll_back(uow).await;
            return Err(error);
        }
    };
    let written = user_update::rewrite(&mut uow.store(), rewrite).await;
    match written {
        Ok(UserUpdateOutcome::Updated { user, message }) => {
            uow.commit().await?;
            Ok((Json(UserMessageResponse::new(&user, message)), "updated"))
        }
        Ok(UserUpdateOutcome::NotModified { message }) => {
            // The outcome carries no record, and the response shows the user
            // as stored, as the COBOL screen still shows the fields. The row
            // is locked by this unit of work since the READ UPDATE, so the
            // re-read returns exactly the record the fields were compared
            // with; it can fail only on a database failure, answered with the
            // read's own audited text (V-COUSR02C-003).
            let shown = user_update::lookup(&mut uow.store(), Some(id)).await;
            roll_back(uow).await;
            Ok((
                Json(UserMessageResponse::new(&shown?, message)),
                "not_modified",
            ))
        }
        Err(error) => {
            roll_back(uow).await;
            Err(error.into())
        }
    }
}

/// CU03 PF5: deletes a user, the Confirm of the delete dialog.
///
/// COUSR03C DELETE-USER-INFO, READ-USER-SEC-FILE, DELETE-USER-SEC-FILE —
/// P-COUSR03C-004, P-COUSR03C-005, P-COUSR03C-011; V-COUSR03C-004 to
/// V-COUSR03C-007 (E-46, E-58).
///
/// The user is read for update and deleted. The body carries the user as
/// read before the delete and `User <id> has been deleted ...`.
///
/// # Errors
///
/// - 400 `EMPTY` `User ID can NOT be empty...`, field `USRIDIN`, for a blank
///   ID, or 400 `INVALID` for an ID longer than `PIC X(8)`.
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 404 `User ID NOT found...`, field `USRIDIN`, when the delete finds no
///   user after the read found it (V-COUSR03C-006).
/// - 500 `Unable to Update User...`, field `FNAME` (the COBOL's own text,
///   reproduced): for an unknown user, whose failed read the COBOL follows
///   with a failing delete (V-COUSR03C-005, E-46), and for a failed delete
///   (V-COUSR03C-007). Nothing is deleted on any error.
#[utoipa::path(
    delete,
    path = "/admin/users/{id}",
    tag = "admin-users",
    params(("id" = String, Path, description = "The user ID, `SEC-USR-ID`, as stored (case kept)")),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The user was deleted", body = UserMessageResponse),
        (status = 400, description = "A blank or over-length user ID", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 404, description = "The delete found no user", body = ErrorBody),
        (status = 500, description = "The user could not be deleted", body = ErrorBody),
    )
)]
pub async fn delete_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<UserMessageResponse>, ApiError> {
    let span = user_span(CU03, &admin, Some(&id));
    traced(&span, delete(&state, id)).await
}

/// The PF5 delete: the ID width check, the record the response shows, then
/// `user_delete::delete`, committed when it deleted the user.
async fn delete(
    state: &AppState,
    id: String,
) -> Result<(Json<UserMessageResponse>, &'static str), ApiError> {
    check_user_id_key(&id, UserDeleteField::UsrIdIn)?;
    let mut uow = state.db().begin().await?;
    // `user_delete::delete` answers only the deleted ID, while the response
    // shows the user as read before the delete, so the record is read for
    // update first in the same unit of work. Its error is not the answer: the
    // rule's own READ UPDATE and DELETE decide that (V-COUSR03C-004 to
    // V-COUSR03C-007), so an unknown user still ends with the COBOL's 500,
    // not the fetch's 404. The lock it takes keeps the row the rule deletes
    // the one read here.
    let shown = user_update::lookup(&mut uow.store(), Some(id.clone())).await;
    let deleted = user_delete::delete(&mut uow.store(), Some(id)).await;
    match (deleted, shown) {
        (Ok(deleted), Ok(user)) => {
            uow.commit().await?;
            let response = Json(UserMessageResponse::new(&user, deleted.message));
            Ok((response, "deleted"))
        }
        // The first alternative is the rule's error, which takes precedence.
        // The second is a delete that succeeded after the first read failed:
        // a transient read failure, or a user added between the two reads.
        // The record to show was never read, so the delete is rolled back and
        // the read's own error answers.
        (Err(error), _) | (Ok(_), Err(error)) => {
            roll_back(uow).await;
            Err(error.into())
        }
    }
}
