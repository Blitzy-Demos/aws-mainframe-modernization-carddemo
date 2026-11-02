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
//! the upper-cased user ID and the stored user type, the values `COSGN00C`
//! moves to `CDEMO-USER-ID` and `CDEMO-USER-TYPE` [app/cbl/COSGN00C.cbl:L224-228].
//! The `usr_type` and `landing` of the response serve the SPA's navigation
//! only. Neither operation takes a token, and [`logout`] is stateless: no
//! server-side session exists, so the SPA discards its token.
//!
//! # Sign-on sequence
//!
//! 1. `LoginRequest::check`: the 8-character lengths of `USERIDI` and
//!    `PASSWDI`.
//! 2. `signon::sign_on` in a unit of work: the blank edits, the upper-casing
//!    and the `USRSEC` read, with no argon2 work.
//! 3. The unit of work is rolled back. Sign-on writes nothing, and the pooled
//!    connection is released before the slow step.
//! 4. `SignonCheck::verify` on tokio's blocking pool: the argon2id compare of
//!    the upper-cased password (E-20), which selects the outcome.
//! 5. The token is issued and the response lands an administrator on `/admin`
//!    and any other user on `/` [app/cbl/COSGN00C.cbl:L230-240].
//!
//! Every rejection is the rule's [`carddemo_domain::error::DomainError`],
//! whose status `crate::error` alone chooses.
//!
//! # Telemetry
//!
//! [`login`] runs in a span with `transaction` `CC00`, the `user_id` that
//! signed on (recorded on success only) and `outcome` (`ok` or `rejected`).
//! Neither the password, the stored hash nor the token is recorded or logged.

use std::future::{Ready, ready};

use axum::Json;
use axum::extract::State;
use carddemo_domain::messages;
use carddemo_domain::rules::signon::{self, SignonCheck, SignonOutcome};
use tracing::{Span, field};

use crate::dto::auth::{LoginRequest, LoginResponse, MessageResponse};
use crate::error::{ApiError, ApiJson, ErrorBody};
use crate::state::AppState;

/// The span `outcome` of a sign-on that issued a token.
const OUTCOME_OK: &str = "ok";

/// The span `outcome` of a sign-on answered with an error body.
const OUTCOME_REJECTED: &str = "rejected";

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
    match &result {
        Ok(response) => {
            span.record("user_id", response.user_id.as_str());
            span.record("outcome", OUTCOME_OK);
        }
        Err(_) => {
            span.record("outcome", OUTCOME_REJECTED);
        }
    }
    result.map(Json)
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
    body.check()?;
    let input = body.into_input();

    let mut uow = state.db().begin().await?;
    let looked_up = signon::sign_on(&mut uow.store(), input).await;
    // Sign-on reads USRSEC and writes nothing, so the unit of work ends here,
    // whatever the rule answered, and its connection goes back to the pool
    // before the argon2 work.
    uow.rollback().await?;

    let outcome = verify(looked_up?).await?;
    let user_id = outcome.user_id.trimmed();
    let usr_type = outcome.usr_type.trimmed();
    let issued = state.jwt().issue(&user_id, &usr_type)?;
    Ok(LoginResponse::new(
        issued.token,
        issued.expires_at,
        &outcome,
    ))
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
