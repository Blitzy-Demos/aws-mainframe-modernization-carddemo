//! CC00 · COSGN00C · COSGN00 — `POST /auth/login`, `POST /auth/logout`.
//!
//! The sign-on screen becomes two public operations. ENTER on the screen
//! (`PROCESS-ENTER-KEY` [app/cbl/COSGN00C.cbl:L108-140]) is
//! `POST /auth/login`; PF3, which answers with `CCDA-MSG-THANK-YOU` and ends
//! the conversation (`SEND-PLAIN-TEXT` [app/cbl/COSGN00C.cbl:L88-90,
//! L162-172]), is `POST /auth/logout`. The other 3270 mechanics of the program
//! (first-entry screen, invalid AID keys, header) have no endpoint.
//!
//! # Identity
//!
//! The actor of every other operation is only ever the verified bearer token's
//! `sub` and `usr_type`, read back by `crate::auth::AuthUser` and
//! `crate::auth::AdminUser`. This module is the only issuer of that token:
//! [`login`] issues it once the `COSGN00C` rules have signed the user on, for
//! the upper-cased user ID and the type the stored user type stands for, from
//! the values `COSGN00C` moves to `CDEMO-USER-ID` and `CDEMO-USER-TYPE`
//! [app/cbl/COSGN00C.cbl:L224-228]. The type is the 88-level value `A` or `U`:
//! a stored code other than exactly `A`, such as a legacy `X` or `a`, takes
//! the `ELSE` path of `IF CDEMO-USRTYP-ADMIN` [app/cbl/COSGN00C.cbl:L230-240]
//! and is issued as `U`. The `usr_type` and `landing` of the response carry
//! the same type and serve the SPA's navigation only. Neither operation takes
//! a token, and [`logout`] is stateless: no server-side session exists, so the
//! SPA discards its token.
//!
//! # Sign-on sequence
//!
//! 1. `LoginRequest::check`: the 8-character lengths of `USERIDI` and
//!    `PASSWDI`.
//! 2. `signon::edit_blank_fields`: the blank user ID, then the blank password
//!    (V-COSGN00C-001, V-COSGN00C-002). These edits read no store and run
//!    before the unit of work is opened, as `PROCESS-ENTER-KEY` rejects a
//!    blank field before any `USRSEC` I/O [app/cbl/COSGN00C.cbl:L117-130]: a
//!    blank field takes no pooled connection and keeps its 400 answer when no
//!    database connection can be had.
//! 3. `signon::sign_on` in a unit of work: the blank edits again, with the
//!    same answer, then the upper-casing and the `USRSEC` read, with no
//!    argon2 work.
//! 4. The unit of work is rolled back. Sign-on writes nothing, and the pooled
//!    connection is released before the slow step.
//! 5. `SignonCheck::verify` on tokio's blocking pool: the argon2id compare of
//!    the upper-cased password (E-20), which selects the outcome.
//! 6. The token is issued and the response lands an administrator on `/admin`
//!    and any other user on `/` [app/cbl/COSGN00C.cbl:L230-240].
//!
//! Every rejection is the rule's [`carddemo_domain::error::DomainError`],
//! whose status `crate::error` alone chooses.
//!
//! # Telemetry
//!
//! [`login`] runs in a span with `transaction` `CC00`, the `user_id` that
//! signed on (recorded on success only) and `outcome`: `ok` for an issued
//! token, `rejected` for a 4xx answer and `failed` for a 5xx one. Neither the
//! password, the stored hash nor the token is recorded or logged.

use std::future::{Ready, ready};

use axum::Json;
use axum::extract::State;
use carddemo_domain::messages;
use carddemo_domain::rules::signon::{self, SignonCheck, SignonInput, SignonOutcome};
use tracing::{Span, field};

use crate::auth::UserType;
use crate::dto::auth::{LoginRequest, LoginResponse, MessageResponse};
use crate::error::{ApiError, ApiJson, ErrorBody};
use crate::state::AppState;

/// The span `outcome` of a sign-on that issued a token.
const OUTCOME_OK: &str = "ok";

/// The span `outcome` of a sign-on answered with a 4xx error body.
const OUTCOME_REJECTED: &str = "rejected";

/// The span `outcome` of a sign-on answered with a 5xx error body.
const OUTCOME_FAILED: &str = "failed";

/// COSGN00C PROCESS-ENTER-KEY / READ-USER-SEC-FILE — P-COSGN00C-002,
/// P-COSGN00C-005, P-COSGN00C-006, P-COSGN00C-007, P-COSGN00C-008,
/// P-COSGN00C-009, P-COSGN00C-010, P-COSGN00C-011, P-COSGN00C-012,
/// P-COSGN00C-013, P-COSGN00C-015, V-COSGN00C-001, V-COSGN00C-002,
/// V-COSGN00C-003, V-COSGN00C-004, V-COSGN00C-005.
///
/// `POST /auth/login`: signs the user on with the `COSGN00C` rules and issues
/// the bearer token. The user ID and password are upper-cased and padded to 8
/// before the user is read and the password verified, so `admin001` and
/// `password` sign on as `ADMIN001`, while a user ID or password stored in
/// lower or mixed case never signs on (E-20).
///
/// Answers 200, not 201: sign-on writes nothing.
///
/// # Errors
///
/// The error body `{code, message, fields}`, with the audited `COSGN00C`
/// message and the BMS field the COBOL puts the cursor on:
///
/// - 400: a field longer than its 8-character map field, with an empty
///   message (`USERID`, `PASSWD`); 'Please enter User ID ...' (`USERID`,
///   V-COSGN00C-001); 'Please enter Password ...' (`PASSWD`, V-COSGN00C-002).
///   The user ID is edited first.
/// - 401: 'User not found. Try again ...' (`USERID`, V-COSGN00C-003); 'Wrong
///   Password. Try again ...' (`PASSWD`, V-COSGN00C-005).
/// - 500: 'Unable to verify the User ...' (`USERID`, V-COSGN00C-004) when the
///   user read fails or the stored hash is unusable; 'Unexpected error' when
///   the database transaction cannot be opened or ended, the verification
///   task fails, or the token cannot be signed.
#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (
            status = 200,
            description = "Signed on: the bearer token and the landing route",
            body = LoginResponse
        ),
        (
            status = 400,
            description = "A blank or over-length user ID or password",
            body = ErrorBody
        ),
        (status = 401, description = "Unknown user ID or wrong password", body = ErrorBody),
        (status = 500, description = "The user could not be verified", body = ErrorBody),
    )
)]
#[tracing::instrument(
    skip_all,
    fields(transaction = "CC00", user_id = field::Empty, outcome = field::Empty)
)]
pub async fn login(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let result = sign_on_user(&state, body).await;
    let span = Span::current();
    if let Ok(response) = &result {
        span.record("user_id", response.user_id.as_str());
    }
    span.record("outcome", outcome(&result));
    result.map(Json)
}

/// The span `outcome` of a sign-on `result`: [`OUTCOME_OK`] for an issued
/// token, [`OUTCOME_FAILED`] for a 5xx error and [`OUTCOME_REJECTED`] for any
/// other.
fn outcome<T>(result: &Result<T, ApiError>) -> &'static str {
    match result {
        Ok(_) => OUTCOME_OK,
        Err(error) if error.status().is_server_error() => OUTCOME_FAILED,
        Err(_) => OUTCOME_REJECTED,
    }
}

/// CSMSG01Y CCDA-MSG-THANK-YOU — P-COSGN00C-003, P-COSGN00C-016.
///
/// `POST /auth/logout`: PF3 on the sign-on screen, answered with the
/// `CCDA-MSG-THANK-YOU` text of `CSMSG01Y` byte for byte, trailing spaces
/// included [app/cpy/CSMSG01Y.cpy:L18-19]. Public and stateless: no token is
/// read and no session ends, because none exists on the server; the SPA
/// discards its token and shows `/login`.
// A plain function returning a ready future rather than an `async fn`: it
// awaits nothing, and axum takes any function returning a future as a handler.
#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "auth",
    responses(
        (
            status = 200,
            description = "Signed off: the CardDemo thank-you text",
            body = MessageResponse
        ),
    )
)]
pub fn logout() -> Ready<Json<MessageResponse>> {
    ready(Json(MessageResponse::new(messages::CCDA_MSG_THANK_YOU)))
}

/// The sign-on sequence of the module documentation, from the request body to
/// the response body.
async fn sign_on_user(state: &AppState, body: LoginRequest) -> Result<LoginResponse, ApiError> {
    let input = signon_input(body)?;

    let mut uow = state.db().begin().await?;
    let looked_up = signon::sign_on(&mut uow.store(), input).await;
    // Sign-on reads USRSEC and writes nothing, so the unit of work ends here,
    // whatever the rule answered, and its connection goes back to the pool
    // before the argon2 work.
    uow.rollback().await?;

    let outcome = verify(looked_up?).await?;
    let user_id = outcome.user_id.trimmed();
    let usr_type = UserType::from_code(&outcome.usr_type.trimmed());
    let issued = state.jwt().issue(&user_id, usr_type)?;
    Ok(LoginResponse::new(
        issued.token,
        issued.expires_at,
        &outcome,
    ))
}

/// COSGN00C PROCESS-ENTER-KEY — P-COSGN00C-005, P-COSGN00C-006,
/// V-COSGN00C-001, V-COSGN00C-002 [app/cbl/COSGN00C.cbl:L117-130]: the
/// sign-on request as the rule input, edited before the unit of work is
/// opened.
///
/// `PROCESS-ENTER-KEY` rejects a blank user ID, then a blank password, before
/// any `USRSEC` I/O. The length check of [`LoginRequest::check`] and
/// [`signon::edit_blank_fields`] read no store, so their rejections come
/// before any failure to open the unit of work, even when no connection can
/// be had, and take no pooled connection. The input keeps the fields as
/// typed, and the rule edits them again with the same answer.
///
/// # Errors
///
/// 400 with code `INVALID`, an empty message and `USERID` then `PASSWD` for
/// each field longer than 8 characters; otherwise 400 with code `EMPTY` and
/// [`messages::PLEASE_ENTER_USER_ID`] on `USERID` for a blank user ID, or
/// [`messages::PLEASE_ENTER_PASSWORD`] on `PASSWD` for a blank password.
fn signon_input(body: LoginRequest) -> Result<SignonInput, ApiError> {
    body.check()?;
    let input = body.into_input();
    signon::edit_blank_fields(&input).map_err(ApiError::from)?;
    Ok(input)
}

/// COSGN00C READ-USER-SEC-FILE — P-COSGN00C-009, P-COSGN00C-010,
/// P-COSGN00C-013, V-COSGN00C-004, V-COSGN00C-005: the password compare.
///
/// argon2id verification is memory-hard and CPU-bound and never yields, so it
/// runs on tokio's blocking pool, where it holds no executor worker that other
/// requests need, as the sign-on rule requires of an asynchronous adapter.
///
/// # Errors
///
/// The rule's rejection, or [`messages::UNEXPECTED_ERROR`] when the
/// verification task panics or is cancelled; the task's failure goes to the
/// log only.
async fn verify(check: SignonCheck) -> Result<SignonOutcome, ApiError> {
    let verified = tokio::task::spawn_blocking(move || check.verify())
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "sign-on verification task failed");
            ApiError::unexpected()
        })?;
    verified.map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::rules::signon::SignonInput;
    use serde_json::{Value, json};

    use super::{OUTCOME_FAILED, OUTCOME_OK, OUTCOME_REJECTED, outcome, signon_input};
    use crate::dto::auth::LoginRequest;
    use crate::error::ApiError;

    #[test]
    fn a_sign_on_is_rejected_for_a_4xx_answer_and_failed_for_a_5xx_one() {
        // (sign-on result, `outcome`) — V-COSGN00C-001, V-COSGN00C-003 to
        // V-COSGN00C-005.
        let cases = [
            (Ok(()), OUTCOME_OK),
            (
                Err(DomainError::empty(messages::PLEASE_ENTER_USER_ID).with_field("USERID")),
                OUTCOME_REJECTED,
            ),
            (
                Err(
                    DomainError::signon_failed(messages::USER_NOT_FOUND_TRY_AGAIN)
                        .with_field("USERID"),
                ),
                OUTCOME_REJECTED,
            ),
            (
                Err(
                    DomainError::signon_failed(messages::WRONG_PASSWORD_TRY_AGAIN)
                        .with_field("PASSWD"),
                ),
                OUTCOME_REJECTED,
            ),
            (
                Err(DomainError::unexpected(messages::UNABLE_TO_VERIFY_THE_USER)
                    .with_field("USERID")),
                OUTCOME_FAILED,
            ),
        ];
        for (result, expected) in cases {
            let result = result.map_err(ApiError::from);
            assert_eq!(outcome(&result), expected, "{result:?}");
        }
        // No unit of work, verification task or token signature.
        assert_eq!(outcome::<()>(&Err(ApiError::unexpected())), OUTCOME_FAILED);
    }

    /// The status and JSON body an error answers with.
    fn answer(error: &ApiError) -> (StatusCode, Value) {
        let body = serde_json::to_value(error.body()).expect("serializes");
        (error.status(), body)
    }

    /// The 400 answer of an edit: `kind` with `message` on each BMS field of
    /// `fields`, in order.
    fn rejection(kind: ErrorKind, message: &str, fields: &[&str]) -> (StatusCode, Value) {
        let fields: Vec<Value> = fields
            .iter()
            .map(|field| json!({"field": field, "message": message}))
            .collect();
        let body = json!({"code": kind.code(), "message": message, "fields": fields});
        (StatusCode::BAD_REQUEST, body)
    }

    #[test]
    fn the_cc00_blank_edits_answer_without_opening_the_store() {
        // COSGN00C PROCESS-ENTER-KEY: the map-field lengths, then
        // V-COSGN00C-001 (order 1) and V-COSGN00C-002 (order 2), all before
        // the USRSEC read (V-COSGN00C-003, order 3), so the edits need no
        // state and no store. A passing request is the rule input as typed.
        let no_id = Err(rejection(
            ErrorKind::Empty,
            messages::PLEASE_ENTER_USER_ID,
            &["USERID"],
        ));
        let no_password = Err(rejection(
            ErrorKind::Empty,
            messages::PLEASE_ENTER_PASSWORD,
            &["PASSWD"],
        ));
        let too_long = Err(rejection(ErrorKind::Invalid, "", &["USERID"]));
        let cases = [
            (None, Some("x"), no_id.clone()),
            (Some(""), Some("PASSWORD"), no_id.clone()),
            (Some("        "), Some("PASSWORD"), no_id.clone()),
            (Some("\0\0\0\0\0\0\0\0"), Some("PASSWORD"), no_id.clone()),
            (None, None, no_id.clone()),
            (Some("        "), Some(""), no_id),
            (Some("ADMIN001"), None, no_password.clone()),
            (Some("ADMIN001"), Some("        "), no_password.clone()),
            (Some("USER0099"), Some("\0\0\0"), no_password),
            (Some("ABCDEFGHI"), Some(""), too_long),
            (
                Some("admin001"),
                Some("password"),
                Ok(("admin001", "password")),
            ),
        ];
        for (user_id, password, expected) in cases {
            let request = LoginRequest {
                user_id: user_id.map(str::to_owned),
                password: password.map(str::to_owned),
            };
            let outcome = signon_input(request).map_err(|error| answer(&error));
            let expected = expected.map(|(user_id, password)| SignonInput {
                user_id: user_id.to_owned(),
                password: password.to_owned(),
            });
            assert_eq!(outcome, expected, "{user_id:?} {password:?}");
        }
    }
}
