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
//! run on tokio's blocking pool, never on an executor thread. Each runs under
//! a permit of the per-process argon2 limit (`AppState::admit_argon2`), shared
//! with sign-on and waited for before the unit of work opens. Only the
//! update's `READ … UPDATE` reads the stored hash, for its compare; the list,
//! the fetch and the delete read the record without it. No response type and
//! no span field carries a password or its hash, and the request bodies'
//! `Debug` leaves the password out.

use std::future::Future;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use carddemo_domain::error::DomainError;
use carddemo_domain::rules::user_add::{self, UserAdded};
use carddemo_domain::rules::user_delete;
use carddemo_domain::rules::user_list::{self, UserListInput, UserProfile};
use carddemo_domain::rules::user_update::{self, UserUpdateOutcome};
use carddemo_persistence::tx::UnitOfWork;
use tokio::sync::OwnedSemaphorePermit;
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::auth::AdminUser;
use crate::dto::admin_users::{
    UserAddField, UserAddRequest, UserDeleteField, UserListQuery, UserMessageResponse, UserSummary,
    UserUpdateField, UserUpdateRequest, check_user_id_key, check_user_list_start_key,
};
use crate::dto::{BmsField, KeySegment};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::{Page, PageQuery};
use crate::state::{AppState, run_argon2};

/// The transaction ID of the user list, `COUSR00C`.
const CU00: &str = "CU00";

/// The transaction ID of the user add, `COUSR01C`.
const CU01: &str = "CU01";

/// The transaction ID of the user fetch and update, `COUSR02C`.
const CU02: &str = "CU02";

/// The transaction ID of the user delete, `COUSR03C`.
const CU03: &str = "CU03";

/// The span field of the user ID a request for one user names.
const TARGET_USER_ID: &str = "target_user_id";

/// The span field of the user ID a CU00 page starts from.
const START_KEY: &str = "start_key";

/// The handler span of a request for one user: the transaction and the
/// acting admin. `target_user_id`, the user the request names, is recorded
/// by [`record_key`] once [`check_user_id_key`] has passed it; a key the
/// check rejects leaves it empty and fills `key_rejected` and `key_len`
/// instead. `outcome` is recorded by [`traced`] once the unit of work is
/// committed or rolled back.
fn user_span(transaction: &'static str, admin: &AdminUser) -> Span {
    tracing::info_span!(
        "admin_users",
        transaction,
        user_id = %admin.user_id,
        target_user_id = Empty,
        key_rejected = Empty,
        key_len = Empty,
        outcome = Empty,
    )
}

/// The CU00 handler span: as [`user_span`], with the `start_key` the page
/// starts from in place of `target_user_id`.
fn list_span(admin: &AdminUser) -> Span {
    tracing::info_span!(
        "admin_users",
        transaction = CU00,
        user_id = %admin.user_id,
        start_key = Empty,
        key_rejected = Empty,
        key_len = Empty,
        outcome = Empty,
    )
}

/// Records on `span` the user ID key `key`, which [`check_user_id_key`] (or,
/// for the CU00 start key, [`check_user_list_start_key`]) answered `checked`
/// for. A key that passed is recorded under `field` as
/// entered, at most `PIC X(8)`. Of a rejected key only the error's `code`
/// (`key_rejected`) and the key's length in characters (`key_len`) are
/// recorded, never its text: a rejected key can be as long as the request
/// allows and hold anything.
fn record_key(span: &Span, field: &'static str, key: &str, checked: &Result<(), ApiError>) {
    match checked {
        Ok(()) => span.record(field, key),
        Err(error) => span
            .record("key_rejected", error.body().code.as_str())
            .record("key_len", key.chars().count()),
    };
}

/// [`check_user_id_key`] of `key` on the BMS field `bms_field`, the check
/// that decides the request, with its result recorded on `span` under
/// `field` by [`record_key`].
///
/// # Errors
///
/// The check's 400 `INVALID`, unchanged.
fn checked_key(
    span: &Span,
    field: &'static str,
    key: &str,
    bms_field: impl BmsField,
) -> Result<(), ApiError> {
    let checked = check_user_id_key(key, bms_field);
    record_key(span, field, key, &checked);
    checked
}

/// [`check_user_list_start_key`] of the CU00 start key `key`, the check that
/// decides the list request, with its result recorded on `span` under
/// `start_key` by [`record_key`].
///
/// # Errors
///
/// The check's 400 `INVALID`, unchanged.
fn checked_start_key(span: &Span, key: &str) -> Result<(), ApiError> {
    let checked = check_user_list_start_key(key);
    record_key(span, START_KEY, key, &checked);
    checked
}

/// Records the new user's ID of a CU01 body on `span` as `target_user_id`,
/// through [`check_user_id_key`] on `USERID` alone. That check only bounds
/// the telemetry: `UserAddRequest::check` edits every member and answers
/// the request, so the error recorded here is dropped. An absent ID records
/// nothing.
fn record_new_user_id(span: &Span, user_id: Option<&str>) {
    if let Some(user_id) = user_id {
        let checked = check_user_id_key(user_id, UserAddField::UserId);
        record_key(span, TARGET_USER_ID, user_id, &checked);
    }
}

/// Runs `work` inside `span` and records its `outcome`: the success label the
/// work returns, or the error body's `code` (for example `NOT_FOUND`), or
/// `cancelled` when the request is dropped before `work` completes.
async fn traced<T>(
    span: &Span,
    work: impl Future<Output = Result<(T, &'static str), ApiError>>,
) -> Result<T, ApiError> {
    match crate::cancellable(span.clone(), work.instrument(span.clone())).await {
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

/// Runs an argon2 step of a rule on tokio's blocking pool under `permit`, an
/// `AppState::admit_argon2` permit the caller waited for.
///
/// The permit moves into the blocking task through
/// `crate::state::run_argon2` and is released when the step returns, so the
/// step stays counted against the per-process argon2 limit even when the
/// request is dropped while it runs.
///
/// # Errors
///
/// - The rule's own [`DomainError`], mapped through the status table.
/// - [`ApiError::unexpected`] when the blocking task does not complete (it
///   panicked, or the runtime is shutting down); no COBOL path has such a
///   failure, so no program message applies. The detail is logged.
async fn off_executor<T, F>(permit: OwnedSemaphorePermit, step: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Result<T, DomainError> + Send + 'static,
    T: Send + 'static,
{
    match run_argon2(permit, step).await {
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
/// COUSR00C SEND-USRLST-SCREEN — P-COUSR00C-013: the 200 page, its
/// `message` included, replaces the `COUSR0A` map.
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
///   holding a character outside ASCII, or holding a NUL character without
///   being the 8 of `LOW-VALUES` (E-49), and for a query that does not
///   deserialize (a `page_size` outside 1 to 100, an unknown `direction`).
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 500 `Unable to lookup User...`, field `USRIDIN`, when the user browse
///   fails (V-COUSR00C-004, V-COUSR00C-006, V-COUSR00C-009).
#[utoipa::path(
    get,
    path = "/admin/users",
    tag = "admin-users",
    params(UserListQuery),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The page of users, with the page-boundary text COUSR00C shows, if any", body = Page<UserSummary>),
        (status = 400, description = "A start key longer than 8 characters, not ASCII, or holding a NUL character other than the 8 of LOW-VALUES, or a malformed query", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 500, description = "The user browse failed", body = ErrorBody),
    )
)]
pub async fn list_users(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiQuery(query): ApiQuery<UserListQuery>,
) -> Result<Json<Page<UserSummary>>, ApiError> {
    let span = list_span(&admin);
    traced(&span, list(&state, &span, query)).await
}

/// The CU00 browse: the start-key width, ASCII and NUL check, recorded on
/// `span`, then
/// `user_list::list` in a unit of work that is rolled back, since the list
/// writes nothing.
async fn list(
    state: &AppState,
    span: &Span,
    query: UserListQuery,
) -> Result<(Json<Page<UserSummary>>, &'static str), ApiError> {
    if let Some(key) = query.start_key.as_deref() {
        checked_start_key(span, key)?;
    }
    let (start_key, direction, page_size) = PageQuery::from(query).resolve(user_list::SCREEN_ROWS);
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
/// COUSR01C SEND-USRADD-SCREEN — P-COUSR01C-007: the response body and the
/// form's field errors replace the `COUSR1A` map.
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
///   field longer than its BMS field or holding a character outside ASCII,
///   or a body that does not deserialize.
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
        (status = 400, description = "A blank, over-length or non-ASCII field, or a malformed body", body = ErrorBody),
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
    let span = user_span(CU01, &admin);
    traced(&span, add(&state, &span, body)).await
}

/// The CU01 add: the width and encoding check, `user_add::prepare` on the
/// blocking pool once the argon2 limit admits it, then the single `WRITE` of
/// `user_add::add`, committed when it succeeds. The permit is waited for and
/// released before the unit of work opens.
async fn add(
    state: &AppState,
    span: &Span,
    body: UserAddRequest,
) -> Result<((StatusCode, Json<UserMessageResponse>), &'static str), ApiError> {
    record_new_user_id(span, body.user_id.as_deref());
    body.check()?;
    let input = body.into_input();
    let argon2 = state.admit_argon2().await?;
    let prepared = off_executor(argon2, move || user_add::prepare(input)).await?;
    let mut uow = state.db().begin().await?;
    let written = user_add::add(&mut uow.store(), prepared).await;
    match written {
        Ok(UserAdded { user, message }) => {
            uow.commit().await?;
            let response = Json(UserMessageResponse::new(&UserProfile::from(&user), message));
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
/// COUSR02C SEND-USRUPD-SCREEN — P-COUSR02C-009: the response body replaces
/// the `COUSR2A` map.
/// COUSR03C SEND-USRDEL-SCREEN — P-COUSR03C-007: the delete dialog, filled
/// from the response body, replaces the `COUSR3A` map.
///
/// The body is the user's ID, names and type. `COUSR02C` also moves the
/// stored password into its edit field; a stored argon2id hash cannot be
/// shown, so no password is ever returned (E-21, a known difference).
///
/// # Errors
///
/// - 400 `EMPTY` `User ID can NOT be empty...`, field `USRIDIN`, for a blank
///   ID, or 400 `INVALID` for an ID longer than `PIC X(8)` or holding a
///   character outside ASCII, and for a bare `.` or `..` segment, which
///   names no user.
/// - 401 without a valid bearer token; 403 for a token that is not an
///   administrator's.
/// - 404 `User ID NOT found...`, field `USRIDIN` (V-COUSR02C-002).
/// - 500 `Unable to lookup User...`, field `FNAME`, when the read fails
///   (V-COUSR02C-003).
#[utoipa::path(
    get,
    path = "/admin/users/{id}",
    tag = "admin-users",
    params(("id" = String, Path, description = "The user ID, `SEC-USR-ID` `X(08)`, as stored (case kept): at most 8 ASCII characters. A user ID that is `.` or `..` after zero or more `~` is sent with one more leading `~` (`.` as `~.`), because URLs drop dot segments; every other ID is sent as stored", max_length = 9, pattern = "^[\\x00-\\x7F]*$")),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The user, without its password", body = UserSummary),
        (status = 400, description = "A blank, over-length or non-ASCII user ID, or a bare `.` or `..` segment", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 404, description = "No user has the ID", body = ErrorBody),
        (status = 500, description = "The user could not be read", body = ErrorBody),
    )
)]
pub async fn get_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(KeySegment(id)): ApiPath<KeySegment>,
) -> Result<Json<UserSummary>, ApiError> {
    let span = user_span(CU02, &admin);
    traced(&span, fetch(&state, &span, id)).await
}

/// The ENTER fetch: the ID width check, recorded on `span`, then
/// `user_update::lookup` in a unit of work that is rolled back, releasing the
/// row its `READ … UPDATE` locked.
async fn fetch(
    state: &AppState,
    span: &Span,
    id: String,
) -> Result<(Json<UserSummary>, &'static str), ApiError> {
    checked_key(span, TARGET_USER_ID, &id, UserUpdateField::UsrIdIn)?;
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
/// COUSR02C SEND-USRUPD-SCREEN — P-COUSR02C-009: the response body replaces
/// the `COUSR2A` map.
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
///   a field or ID longer than its BMS field or holding a character outside
///   ASCII, a bare `.` or `..` segment, which names no user, or a body that
///   does not deserialize.
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
    params(("id" = String, Path, description = "The user ID, `SEC-USR-ID` `X(08)`, as stored (case kept): at most 8 ASCII characters. A user ID that is `.` or `..` after zero or more `~` is sent with one more leading `~` (`.` as `~.`), because URLs drop dot segments; every other ID is sent as stored", max_length = 9, pattern = "^[\\x00-\\x7F]*$")),
    request_body = UserUpdateRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The user was updated, or no field differed and nothing was written", body = UserMessageResponse),
        (status = 400, description = "A blank, over-length or non-ASCII field or user ID, a bare `.` or `..` segment, or a malformed body", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 404, description = "The rewrite found no user", body = ErrorBody),
        (status = 500, description = "The user could not be updated", body = ErrorBody),
    )
)]
pub async fn update_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(KeySegment(id)): ApiPath<KeySegment>,
    ApiJson(body): ApiJson<UserUpdateRequest>,
) -> Result<Json<UserMessageResponse>, ApiError> {
    let span = user_span(CU02, &admin);
    traced(&span, update(&state, &span, id, body)).await
}

/// The PF5 update in three steps, inside one unit of work: `user_update::update`
/// (the empty checks and the `READ … UPDATE`), `UserUpdateCheck::compare` on
/// the blocking pool with the row still locked, then `user_update::rewrite`,
/// committed when it rewrote the record. The ID width check before them is
/// recorded on `span`.
///
/// The argon2 permit `compare` runs under is waited for before the unit of
/// work opens, so an update queued behind other argon2 work holds neither a
/// pooled connection nor the `USRSEC` row lock.
async fn update(
    state: &AppState,
    span: &Span,
    id: String,
    body: UserUpdateRequest,
) -> Result<(Json<UserMessageResponse>, &'static str), ApiError> {
    checked_key(span, TARGET_USER_ID, &id, UserUpdateField::UsrIdIn)?;
    body.check()?;
    let input = body.into_input(id.clone());
    let argon2 = state.admit_argon2().await?;
    let mut uow = state.db().begin().await?;
    let read = user_update::update(&mut uow.store(), input).await;
    let check = match read {
        Ok(check) => check,
        Err(error) => {
            roll_back(uow).await;
            return Err(error.into());
        }
    };
    let rewrite = match off_executor(argon2, move || check.compare()).await {
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
            let response = Json(UserMessageResponse::new(&UserProfile::from(&user), message));
            Ok((response, "updated"))
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
/// COUSR03C SEND-USRDEL-SCREEN — P-COUSR03C-007: the delete dialog and the
/// `DELETE` response replace the `COUSR3A` map.
///
/// The user is read for update and deleted. The body carries the user as
/// read before the delete and `User <id> has been deleted ...`.
///
/// # Errors
///
/// - 400 `EMPTY` `User ID can NOT be empty...`, field `USRIDIN`, for a blank
///   ID, or 400 `INVALID` for an ID longer than `PIC X(8)` or holding a
///   character outside ASCII, and for a bare `.` or `..` segment, which
///   names no user.
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
    params(("id" = String, Path, description = "The user ID, `SEC-USR-ID` `X(08)`, as stored (case kept): at most 8 ASCII characters. A user ID that is `.` or `..` after zero or more `~` is sent with one more leading `~` (`.` as `~.`), because URLs drop dot segments; every other ID is sent as stored", max_length = 9, pattern = "^[\\x00-\\x7F]*$")),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "The user was deleted", body = UserMessageResponse),
        (status = 400, description = "A blank, over-length or non-ASCII user ID, or a bare `.` or `..` segment", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 403, description = "The token is not an administrator's", body = ErrorBody),
        (status = 404, description = "The delete found no user", body = ErrorBody),
        (status = 500, description = "The user could not be deleted", body = ErrorBody),
    )
)]
pub async fn delete_user(
    State(state): State<AppState>,
    admin: AdminUser,
    ApiPath(KeySegment(id)): ApiPath<KeySegment>,
) -> Result<Json<UserMessageResponse>, ApiError> {
    let span = user_span(CU03, &admin);
    traced(&span, delete(&state, &span, id)).await
}

/// The PF5 delete: the ID width check, recorded on `span`, the record the
/// response shows, then `user_delete::delete`, committed when it deleted the
/// user.
async fn delete(
    state: &AppState,
    span: &Span,
    id: String,
) -> Result<(Json<UserMessageResponse>, &'static str), ApiError> {
    checked_key(span, TARGET_USER_ID, &id, UserDeleteField::UsrIdIn)?;
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

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Span, Subscriber};
    use utoipa::IntoParams;
    use utoipa_axum::router::OpenApiRouter;
    use utoipa_axum::routes;

    use super::{
        CU00, CU01, CU02, CU03, START_KEY, TARGET_USER_ID, checked_key, checked_start_key,
        list_span, record_new_user_id, user_span,
    };
    use crate::auth::{AdminUser, AuthUser, UserType};
    use crate::dto::admin_users::{UserDeleteField, UserUpdateField};
    use crate::error::ApiError;
    use crate::pagination::PageQuery;
    use crate::state::AppState;

    /// The ASCII pattern of every user key.
    const ASCII: &str = "^[\\x00-\\x7F]*$";

    /// The pattern of the CU00 start key: ASCII, with NUL characters only as
    /// the 8 of `LOW-VALUES`.
    const LIST_START_KEY: &str = "^(?:\\x00{8}|[\\x01-\\x7F]*)$";

    /// The `OpenAPI` document of the five operations, registered as the route
    /// registry registers them: one `routes!` per path.
    fn document() -> Value {
        let (_router, api) = OpenApiRouter::<AppState>::new()
            .routes(routes!(super::list_users, super::create_user))
            .routes(routes!(
                super::get_user,
                super::update_user,
                super::delete_user
            ))
            .split_for_parts();
        serde_json::to_value(api).expect("the document serializes")
    }

    /// The parameters of the operation `method` of `path`.
    fn parameters<'a>(document: &'a Value, path: &str, method: &str) -> &'a [Value] {
        document["paths"][path][method]["parameters"]
            .as_array()
            .map_or(&[], Vec::as_slice)
    }

    /// The `name` and `in` of each parameter.
    fn names(parameters: &[Value]) -> Vec<(&str, &str)> {
        parameters
            .iter()
            .map(|parameter| {
                (
                    parameter["name"].as_str().expect("a name"),
                    parameter["in"].as_str().expect("a location"),
                )
            })
            .collect()
    }

    /// The longest `{id}` segment: an 8-character user ID that is `.` or `..`
    /// after `~`s, such as `~~~~~~~.`, travels behind one more `~`
    /// (`KeySegment`).
    const ESCAPED_KEY_LEN: u8 = 9;

    #[test]
    fn every_user_key_publishes_the_ascii_limit_of_its_escaped_segment() {
        let document = document();
        for method in ["get", "put", "delete"] {
            let parameters = parameters(&document, "/admin/users/{id}", method);
            assert_eq!(names(parameters), [("id", "path")], "{method}");
            let id = &parameters[0];
            assert_eq!(id["required"], json!(true), "{method}");
            assert_eq!(id["schema"]["type"], json!("string"), "{method}");
            assert_eq!(
                id["schema"]["maxLength"],
                json!(ESCAPED_KEY_LEN),
                "{method}"
            );
            assert_eq!(id["schema"]["pattern"], json!(ASCII), "{method}");
            let description = id["description"].as_str().expect("a description");
            assert!(description.contains("(`.` as `~.`)"), "{method}");
        }

        let list = parameters(&document, "/admin/users", "get");
        assert_eq!(
            names(list),
            [
                ("start_key", "query"),
                ("direction", "query"),
                ("page_size", "query")
            ]
        );
        assert_eq!(list[0]["required"], json!(false));
        assert_eq!(list[0]["schema"]["type"], json!("string"));
        assert_eq!(list[0]["schema"]["maxLength"], json!(8));
        assert_eq!(list[0]["schema"]["pattern"], json!(LIST_START_KEY));
        // The shared paging parameters keep the schemas `PageQuery` gives them.
        let shared = serde_json::to_value(PageQuery::into_params(|| None)).expect("serializes");
        let shared = &shared.as_array().expect("an array")[1..];
        for (published, page_query) in list[1..].iter().zip(shared) {
            assert_eq!(published["name"], page_query["name"]);
            assert_eq!(published["required"], page_query["required"]);
            assert_eq!(published["schema"], page_query["schema"], "{published}");
        }
        assert!(!Value::from(list.to_vec()).to_string().contains("$ref"));

        assert!(parameters(&document, "/admin/users", "post").is_empty());
    }

    /// Text no valid key holds, so finding it in telemetry means a rejected
    /// key leaked.
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

    /// One handler's key: the span it opens, the field the key is recorded
    /// under, and the key step it runs on that span first. `gates` is whether
    /// the step's result answers the request.
    struct Site {
        route: &'static str,
        transaction: &'static str,
        span: fn(&AdminUser) -> Span,
        field: &'static str,
        step: fn(&Span, &str) -> Result<(), ApiError>,
        gates: bool,
    }

    fn sites() -> [Site; 5] {
        [
            Site {
                route: "GET /admin/users?start_key",
                transaction: CU00,
                span: list_span,
                field: START_KEY,
                step: checked_start_key,
                gates: true,
            },
            Site {
                route: "POST /admin/users",
                transaction: CU01,
                span: |admin| user_span(CU01, admin),
                field: TARGET_USER_ID,
                step: |span, key| {
                    record_new_user_id(span, Some(key));
                    Ok(())
                },
                gates: false,
            },
            Site {
                route: "GET /admin/users/{id}",
                transaction: CU02,
                span: |admin| user_span(CU02, admin),
                field: TARGET_USER_ID,
                step: |span, key| checked_key(span, TARGET_USER_ID, key, UserUpdateField::UsrIdIn),
                gates: true,
            },
            Site {
                route: "PUT /admin/users/{id}",
                transaction: CU02,
                span: |admin| user_span(CU02, admin),
                field: TARGET_USER_ID,
                step: |span, key| checked_key(span, TARGET_USER_ID, key, UserUpdateField::UsrIdIn),
                gates: true,
            },
            Site {
                route: "DELETE /admin/users/{id}",
                transaction: CU03,
                span: |admin| user_span(CU03, admin),
                field: TARGET_USER_ID,
                step: |span, key| checked_key(span, TARGET_USER_ID, key, UserDeleteField::UsrIdIn),
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
    fn a_user_key_is_recorded_once_checked_and_a_rejected_one_only_by_code_and_length() {
        let overlong = format!("USER0001{MARKER}");
        let overlong_len = overlong.chars().count().to_string();
        // (key, recorded as the key, rejected with this length): an ASCII
        // key of `PIC X(8)` or less, blank included, is kept as entered; a
        // longer one, counted in characters, or one holding a character
        // outside ASCII, is not.
        let cases = [
            ("USER0001", Some("USER0001"), None),
            ("", Some(""), None),
            (overlong.as_str(), None, Some(overlong_len.as_str())),
            ("ÜSER0001X", None, Some("9")),
            ("ÜSER0001", None, Some("8")),
        ];
        for site in sites() {
            for (key, recorded, rejected_len) in cases {
                let capture = Capture::default();
                let result = tracing::subscriber::with_default(capture.clone(), || {
                    let span = (site.span)(&admin());
                    (site.step)(&span, key)
                });
                let case = format!("{} with {key:?}", site.route);
                // Each field holds exactly the values recorded under it, so a
                // second recording of any of them fails the case.
                assert_eq!(
                    capture.values_of("transaction"),
                    [site.transaction],
                    "{case}"
                );
                assert_eq!(capture.values_of("user_id"), ["ADMIN001"], "{case}");
                assert_eq!(capture.values_of(site.field), recorded.as_slice(), "{case}");
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
                    other => panic!("{case}: the key step answered {other:?}"),
                }
            }
        }
    }

    #[test]
    fn an_absent_new_user_id_records_no_key() {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            record_new_user_id(&user_span(CU01, &admin()), None);
        });
        for field in [TARGET_USER_ID, "key_rejected", "key_len"] {
            assert_eq!(capture.values_of(field), Vec::<String>::new(), "{field}");
        }
        assert_eq!(capture.values_of("transaction"), [CU01]);
    }
}
