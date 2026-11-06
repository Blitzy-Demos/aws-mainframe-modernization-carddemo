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
//! the same type and serve the SPA's navigation only. Neither operation
//! requires a token: [`login`] reads none, and [`logout`] answers alike with
//! or without one. [`logout`] is stateless: no server-side session exists, so
//! the SPA discards its token. A bearer token the SPA still sends there only
//! names its user in the logout span when it verifies; no store is read and
//! nothing is refused.
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
//! 5. `SignonCheck::verify` on tokio's blocking pool, once the per-process
//!    argon2 limit (`AppState::admit_argon2`) admits it: the argon2id compare
//!    of the upper-cased password (E-20), which selects the outcome. Sign-ons
//!    beyond the limit queue for a permit, holding no connection, and the
//!    blocking task keeps its permit until the compare returns, even when the
//!    client has gone, so concurrent sign-ons cannot multiply the memory each
//!    compare holds.
//! 6. The token is issued and the response lands an administrator on `/admin`
//!    and any other user on `/` [app/cbl/COSGN00C.cbl:L230-240].
//!
//! Every rejection is the rule's [`carddemo_domain::error::DomainError`],
//! whose status `crate::error` alone chooses.
//!
//! # Telemetry
//!
//! [`login`] runs in a span with `transaction` `CC00`, `user_id` and
//! `outcome`. `user_id` is the user the sign-on is attempted for, recorded
//! once, before the sign-on runs, when the user ID passes its length and
//! blank edits: the upper-cased `USRSEC` key the rule reads by, which is
//! also the `user_id` of a successful response. A wrong password, an unknown
//! user, a password refused by its own edits and a failure to read the user
//! are therefore all attributed; an over-length or blank user ID records
//! none. `outcome` is `ok` for an issued token, `rejected` for a 4xx answer
//! and `failed` for a 5xx one, or `cancelled` for a sign-on dropped before
//! its answer, its client gone.
//!
//! [`logout`] runs in a span with `transaction` `CC00`, `outcome` `ok` and,
//! when the request's one bearer token verifies, that token's `sub` as
//! `user_id`. A request without a token, or whose token does not verify,
//! records no user, and no event is logged for it.
//!
//! Neither the password, the stored hash nor the token is recorded or logged.

use std::future::{Ready, ready};

use axum::Json;
use axum::extract::State;
use carddemo_domain::messages;
use carddemo_domain::rules::signon::{self, SignonCheck, SignonInput, SignonOutcome};
use tracing::{Span, field};

use crate::auth::UserType;
use crate::auth::extract::BearerUser;
use crate::dto::auth::{LoginField, LoginRequest, LoginResponse, MessageResponse};
use crate::dto::id_string;
use crate::error::{ApiError, ApiJson, ErrorBody};
use crate::state::{AppState, run_argon2};

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
    let span = Span::current();
    record_attempted_user(&span, &body);
    let result = crate::cancellable(span.clone(), sign_on_user(&state, body)).await;
    span.record("outcome", outcome(&result));
    result.map(Json)
}

/// COSGN00C PROCESS-ENTER-KEY — P-COSGN00C-005, P-COSGN00C-007,
/// V-COSGN00C-001: records on `span`, the [`login`] span, the `user_id` the
/// sign-on of `body` is attempted for, before the sign-on runs.
///
/// It is recorded once the user ID passes its own edits: the map-field
/// length of [`LoginRequest::check`], which then names no over-length
/// `USERID`, and the blank edit of V-COSGN00C-001. The value is the `USRSEC`
/// key the rule reads by ([`signon::entered_user_key`]), upper-cased and
/// without trailing spaces: for a failed sign-on the user it was attempted
/// for, and for a successful one the user that signed on, the `user_id` of
/// its response. An over-length or blank user ID records nothing. The
/// password's edits do not decide it, and the password is never recorded.
fn record_attempted_user(span: &Span, body: &LoginRequest) {
    let user_id_too_long = body.check().is_err_and(|error| {
        error
            .body()
            .fields
            .iter()
            .any(|entry| entry.field == LoginField::UserId.as_str())
    });
    if user_id_too_long {
        return;
    }
    if let Some(key) = signon::entered_user_key(body.user_id.as_deref().unwrap_or_default()) {
        span.record("user_id", id_string(&key).as_str());
    }
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
/// required and no session ends, because none exists on the server; the SPA
/// discards its token and shows `/login`. The answer is the same with or
/// without a bearer token, valid or not: a token that verifies only names
/// its user in the server's trace span, and none is refused.
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
#[tracing::instrument(
    skip_all,
    fields(transaction = "CC00", user_id = field::Empty, outcome = field::Empty)
)]
pub fn logout(BearerUser(user): BearerUser) -> Ready<Json<MessageResponse>> {
    let span = Span::current();
    if let Some(user) = user {
        span.record("user_id", user.user_id.as_str());
    }
    span.record("outcome", OUTCOME_OK);
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

    let outcome = verify(state, looked_up?).await?;
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
/// requests need, as the sign-on rule requires of an asynchronous adapter. It
/// first waits for a permit of the argon2 limit of `state`, which
/// `crate::state::run_argon2` moves into the blocking task: the permit is
/// released when the compare returns, not when this request is dropped.
///
/// # Errors
///
/// The rule's rejection, or [`messages::UNEXPECTED_ERROR`] when no permit can
/// be had or the verification task panics or is cancelled; the failure goes
/// to the log only.
async fn verify(state: &AppState, check: SignonCheck) -> Result<SignonOutcome, ApiError> {
    let permit = state.admit_argon2().await?;
    let verified = run_argon2(permit, move || check.verify())
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "sign-on verification task failed");
            ApiError::unexpected()
        })?;
    verified.map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header::AUTHORIZATION;
    use axum::http::{HeaderValue, Request, StatusCode};
    use axum::routing::post;
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::rules::signon::SignonInput;
    use carddemo_runtime::telemetry::subscriber_with_writer;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use tracing::field;

    use super::{
        OUTCOME_FAILED, OUTCOME_OK, OUTCOME_REJECTED, logout, outcome, record_attempted_user,
        signon_input,
    };
    use crate::auth::{JwtKeys, UserType};
    use crate::dto::auth::LoginRequest;
    use crate::error::ApiError;

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// What `run` returns, and the JSON lines it writes through the API's
    /// telemetry subscriber under the default filter `info`.
    fn logged<T>(run: impl FnOnce() -> T) -> (T, Vec<Value>) {
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) =
            subscriber_with_writer("api", "info", move || sink.clone()).expect("valid directives");
        let result = tracing::subscriber::with_default(subscriber, run);
        guard.shutdown().expect("the tracer provider shuts down");
        let bytes = capture.0.lock().expect("capture buffer").clone();
        let text = String::from_utf8(bytes).expect("JSON lines are UTF-8");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect();
        (result, lines)
    }

    /// One sign-on of [`ATTEMPTS`]: (case, user ID typed, password typed, the
    /// `user_id` its span records).
    type Attempt = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    );

    /// The sign-ons of
    /// `a_sign_on_names_the_user_it_is_attempted_for_once_the_user_id_passes_its_edits`:
    /// P-COSGN00C-005 and V-COSGN00C-001 (blank), P-COSGN00C-007 (the
    /// upper-cased key). Each case names the answer its request gets; the
    /// user ID is recorded before the sign-on runs, so that answer does not
    /// change it.
    const ATTEMPTS: [Attempt; 13] = [
        (
            "wrong password, 401",
            Some("ADMIN001"),
            Some("WRONGPW1"),
            Some("ADMIN001"),
        ),
        (
            "unknown user, 401",
            Some("user0099"),
            Some("PASSWORD"),
            Some("USER0099"),
        ),
        (
            "over-long password, 400",
            Some("admin001"),
            Some("WRONGPW123"),
            Some("ADMIN001"),
        ),
        (
            "blank password, 400",
            Some("ADMIN001"),
            Some("        "),
            Some("ADMIN001"),
        ),
        (
            "absent password, 400",
            Some("uSeR0001"),
            None,
            Some("USER0001"),
        ),
        // The query string is not read: the body's password decides.
        (
            "password in the query string, 401",
            Some("ADMIN001"),
            Some("QAQUERY1"),
            Some("ADMIN001"),
        ),
        (
            "database down, 500",
            Some("admin001"),
            Some("PASSWORD"),
            Some("ADMIN001"),
        ),
        (
            "short user ID",
            Some("user1"),
            Some("PASSW0RD"),
            Some("USER1"),
        ),
        (
            "over-long user ID, 400",
            Some("ADMIN0011"),
            Some("PASSWORD"),
            None,
        ),
        (
            "both over-long, 400",
            Some("ADMIN0011"),
            Some("WRONGPW123"),
            None,
        ),
        ("blank user ID, 400", Some("   "), Some("PASSWORD"), None),
        (
            "LOW-VALUES user ID, 400",
            Some("\0\0\0\0\0\0\0\0"),
            Some("PASSWORD"),
            None,
        ),
        ("absent user ID, 400", None, Some("PASSWORD"), None),
    ];

    #[test]
    fn a_sign_on_names_the_user_it_is_attempted_for_once_the_user_id_passes_its_edits() {
        for (case, user_id, password, expected) in ATTEMPTS {
            let body = LoginRequest {
                user_id: user_id.map(str::to_owned),
                password: password.map(str::to_owned),
            };
            let ((), lines) = logged(|| {
                let span = tracing::info_span!(
                    "login",
                    transaction = "CC00",
                    user_id = field::Empty,
                    outcome = field::Empty,
                );
                record_attempted_user(&span, &body);
            });
            let [span] = lines.as_slice() else {
                panic!("{case}: one span line expected, got {lines:?}");
            };
            assert_eq!(span["type"], "span", "{case}");
            assert_eq!(span["name"], "login", "{case}");
            let attributes = &span["attributes"];
            assert_eq!(attributes["transaction"], "CC00", "{case}: {span}");
            assert_eq!(
                attributes.get("user_id").and_then(Value::as_str),
                expected,
                "{case}: {span}"
            );
            assert!(attributes.get("password").is_none(), "{case}: {span}");
            if let Some(password) = password.filter(|typed| !typed.trim().is_empty()) {
                let text = span.to_string();
                assert!(!text.contains(password), "{case}: {text}");
                let upper = password.to_ascii_uppercase();
                assert!(!text.contains(&upper), "{case}: {text}");
            }
        }
    }

    /// The secret of the token keys the logout tests verify with.
    const SECRET: &str = "test-secret";

    /// `POST /auth/logout` with the `Authorization` value `credential`, if
    /// any, through the API's layers over [`logout`] with `keys`: its status
    /// and JSON body, and the JSON lines the API's telemetry subscriber writes
    /// meanwhile.
    fn logged_logout(
        keys: Arc<JwtKeys>,
        credential: Option<&str>,
    ) -> ((StatusCode, Value), Vec<Value>) {
        let routes = Router::new()
            .route("/auth/logout", post(logout))
            .with_state(keys);
        let app = crate::with_layers(routes, HeaderValue::from_static("http://localhost:5173"));
        let request = credential
            .into_iter()
            .fold(Request::post("/auth/logout"), |builder, value| {
                builder.header(AUTHORIZATION, value)
            })
            .body(Body::empty())
            .expect("a valid request");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        logged(|| {
            runtime.block_on(async {
                let response = app
                    .oneshot(request)
                    .await
                    .expect("the router is infallible");
                let status = response.status();
                let bytes = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("the body is buffered");
                (status, serde_json::from_slice(&bytes).expect("a JSON body"))
            })
        })
    }

    /// The span line named `name` among `lines`, the only one.
    fn span_named<'a>(case: &str, lines: &'a [Value], name: &str) -> &'a Value {
        let spans: Vec<&Value> = lines
            .iter()
            .filter(|line| line["type"] == "span" && line["name"] == name)
            .collect();
        let [span] = spans.as_slice() else {
            panic!("{case}: one {name} span expected, got {lines:?}");
        };
        span
    }

    #[test]
    fn logout_names_the_user_of_a_verified_token_in_its_span_and_answers_alike() {
        // P-COSGN00C-003, P-COSGN00C-016: the thank-you text for every
        // request, and a CC00 span under the request span.
        let keys = Arc::new(JwtKeys::new(SECRET, 3600));
        let issue = |keys: &JwtKeys, user_id: &str, usr_type: UserType| {
            keys.issue(user_id, usr_type).expect("issues").token
        };
        let admin = issue(&keys, "ADMIN001", UserType::Admin);
        let user = issue(&keys, "USER0001", UserType::User);
        let other = JwtKeys::new("other-secret", 3600);
        let forged = issue(&other, "ADMIN001", UserType::Admin);
        // (case, `Authorization`, the span's `user_id`)
        let cases = [
            ("no token", None, None),
            (
                "the ADMIN001 token",
                Some(format!("Bearer {admin}")),
                Some("ADMIN001"),
            ),
            (
                "the USER0001 token",
                Some(format!("bearer {user}")),
                Some("USER0001"),
            ),
            (
                "another key's token",
                Some(format!("Bearer {forged}")),
                None,
            ),
            (
                "a bearer that does not verify",
                Some("Bearer not-a-token".to_owned()),
                None,
            ),
            ("another scheme", Some(format!("Basic {admin}")), None),
        ];
        let thank_you = json!({"message": messages::CCDA_MSG_THANK_YOU});
        for (case, credential, user_id) in cases {
            let (answer, lines) = logged_logout(Arc::clone(&keys), credential.as_deref());
            assert_eq!(answer, (StatusCode::OK, thank_you.clone()), "{case}");
            let handler = span_named(case, &lines, "logout");
            let request = span_named(case, &lines, "request");
            assert_eq!(handler["parent_span_id"], request["span_id"], "{case}");
            assert_eq!(handler["trace_id"], request["trace_id"], "{case}");
            let attributes = &handler["attributes"];
            assert_eq!(attributes["transaction"], "CC00", "{case}: {handler}");
            assert_eq!(attributes["outcome"], "ok", "{case}: {handler}");
            assert_eq!(
                attributes.get("user_id").and_then(Value::as_str),
                user_id,
                "{case}: {handler}"
            );
            // Logout refuses nothing, so the request span records no refusal
            // and no warning or error is logged.
            for field in ["transaction", "user_id", "outcome"] {
                assert!(
                    request["attributes"].get(field).is_none(),
                    "{case}: {request}"
                );
            }
            for line in &lines {
                assert!(
                    line["level"] != "WARN" && line["level"] != "ERROR",
                    "{case}: {line}"
                );
                let text = line.to_string();
                for withheld in [&admin, &user, &forged] {
                    assert!(!text.contains(withheld.as_str()), "{case}: {text}");
                }
                for withheld in ["not-a-token", SECRET, "other-secret"] {
                    assert!(!text.contains(withheld), "{case}: {text}");
                }
            }
        }
    }

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
