//! The signed-on user of a request: the [`AuthUser`] and [`AdminUser`]
//! extractors.
//!
//! COSGN00C hands the signed-on user to every later program in the COMMAREA
//! fields `CDEMO-USER-ID PIC X(08)` and `CDEMO-USER-TYPE PIC X(01)`
//! [app/cpy/COCOM01Y.cpy:L25-28]. Here the bearer token that
//! `POST /auth/login` issues carries the two values as its `sub` and
//! `usr_type` claims, and these extractors read them back.
//!
//! - The actor comes only from the verified `Authorization: Bearer <token>`
//!   header. No query parameter, path segment, body field or other header
//!   ever names the acting user or a role.
//! - [`AuthUser`] answers 401 ([`ApiError::unauthenticated`]) for a request
//!   without a valid token: no `Authorization` header, more than one, another
//!   scheme, or a token that is malformed, tampered with, signed with another
//!   key or algorithm, expired, or whose `usr_type` claim is not exactly `A`
//!   or `U`. A verified token is then held to `USRSEC` as COSGN00C
//!   `READ-USER-SEC-FILE` reads it [app/cbl/COSGN00C.cbl:L209-219]: a token
//!   whose `sub` names no stored user, one deleted since sign-on included, is
//!   401 too, and a failed read is 500. This read precedes every handler, so
//!   a protected route whose database cannot be reached answers that 500
//!   before any of its own edits, store-free ones included.
//! - [`AdminUser`] authenticates first and checks the role second. A request
//!   without a valid token, or whose token names no stored user, is therefore
//!   401 on `/admin/**` routes too, and only a valid token whose `usr_type` is
//!   not `A` is 403 ([`ApiError::forbidden`]).
//! - [`BearerUser`] serves the public `POST /auth/logout`: the user of a
//!   bearer token that verifies, if one is sent, for its span only. It reads
//!   no store, logs nothing and never refuses a request.
//!
//! The COBOL keeps users out of admin functions only through menu routing:
//! COSGN00C transfers an `A` user to COADM01C and every other user to
//! COMEN01C [app/cbl/COSGN00C.cbl:L230-240], and COMEN01C refuses an
//! admin-only option to a `U` user [app/cbl/COMEN01C.cbl:L136-143]. The 403
//! gate takes the place of that routing on the server; the SPA's
//! `RequireAdmin` guard only decides what navigation it shows.
//!
//! No resource-ownership check is made here. `USRSEC` links a user to no
//! account, so every signed-on user may address every account, as on the
//! 3270 screens.
//!
//! Every rejection is logged at `warn` level, so the default `RUST_LOG=info`
//! keeps it. A 401 is the event `bearer token rejected` with a fixed `reason`:
//! `missing_or_malformed_header`, the token's
//! [`JwtError::reason`](crate::auth::JwtError::reason), or `unknown_subject`
//! for a verified token whose `sub` names no stored user. A 403 is the event
//! `admin access denied` with the verified `user_id`. No event carries the
//! token, the header value, a claim value the token was rejected for, the
//! decoder's message or the secret.
//!
//! A refused request never reaches its handler, so no handler span opens for
//! it. Before either event, the refusal is recorded on the request span
//! through `crate::record_refusal`: the `transaction` of the operation the
//! request asks for, `outcome` `rejected`, and for a 403 the verified
//! `user_id`. Each event therefore carries them in its span context. A 500 of
//! the `USRSEC` read, or of its unit of work, is recorded the same way
//! through `crate::record_failure`, with `outcome` `failed` and the verified
//! token's `sub` as `user_id`, before its detail is logged. The 401 of a
//! `sub` that names no stored user records no user.
//!
//! A verified user is also kept in the request's extensions, where
//! `verified_user` finds it. Every handler takes its [`AuthUser`] or
//! [`AdminUser`] before its body, query and path extractors, so when one of
//! those refuses a request whose token verified, its refusal records that
//! `user_id` as well. A request whose token did not verify or names no stored
//! user, and a request to the public `POST /auth/login`, keep no user, so
//! their refusals record none.

use std::convert::Infallible;
use std::ops::Deref;
use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap};
use carddemo_domain::numeric::UserId;
use carddemo_domain::rules::PortError;
use carddemo_domain::rules::signon::UserStore;
use carddemo_persistence::tx::PersistenceError;

use crate::auth::jwt::{Claims, JwtKeys, UserType};
use crate::error::ApiError;
use crate::state::AppState;

/// The authentication scheme of the `Authorization` header, compared without
/// regard to ASCII case.
const BEARER_SCHEME: &str = "Bearer";

/// The logged `reason` of a request without a single well-formed
/// `Authorization: Bearer <token>` header, the cases [`bearer_token`] refuses.
const REASON_MISSING_OR_MALFORMED_HEADER: &str = "missing_or_malformed_header";

/// The logged `reason` of a verified token whose `sub` names no stored user,
/// the case [`require_signed_on_user`] refuses.
const REASON_UNKNOWN_SUBJECT: &str = "unknown_subject";

/// The signed-on user of a request, from its verified bearer token.
///
/// It stands for the COMMAREA fields `CDEMO-USER-ID` and `CDEMO-USER-TYPE`
/// [app/cpy/COCOM01Y.cpy:L25-28]. Any route that takes it answers 401 to a
/// request without a valid token, or whose token names no stored user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUser {
    /// The token's `sub` claim verbatim: the upper-cased user ID the sign-on
    /// handler issued the token for.
    pub user_id: String,
    /// The type of the token's `usr_type` claim, `A` ([`UserType::Admin`]) or
    /// `U` ([`UserType::User`]); a token with any other claim value is
    /// rejected before it reaches here.
    pub usr_type: UserType,
}

impl AuthUser {
    /// Whether the user is an admin, the 88-level test `CDEMO-USRTYP-ADMIN`.
    #[must_use]
    pub const fn is_admin(&self) -> bool {
        self.usr_type.is_admin()
    }

    /// The user a verified token's claims name.
    fn from_claims(claims: Claims) -> Self {
        let Claims { sub, usr_type, .. } = claims;
        Self {
            user_id: sub,
            usr_type,
        }
    }
}

/// A signed-on admin: an [`AuthUser`] whose type is [`UserType::Admin`].
///
/// Every `/admin/**` route takes it. A request without a valid token, or
/// whose token names no stored user, is 401, and a valid token of any other
/// type is 403. It dereferences to the [`AuthUser`], so a handler reads the
/// admin's ID as `admin.user_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUser(pub AuthUser);

impl Deref for AdminUser {
    type Target = AuthUser;

    fn deref(&self) -> &AuthUser {
        &self.0
    }
}

/// The token of the request's one `Authorization: Bearer <token>` header.
///
/// `None` when there is no `Authorization` header or more than one, when the
/// value is not visible ASCII, when the scheme is not `Bearer` (in any ASCII
/// case), or when the scheme is not followed by exactly one space and a
/// non-empty token without spaces. Two headers are refused rather than one of
/// them chosen, because a proxy in front of the API may have read the other.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    (scheme.eq_ignore_ascii_case(BEARER_SCHEME) && !token.is_empty() && !token.contains(' '))
        .then_some(token)
}

/// The signed-on user of the request `parts` head, whose bearer token `keys`
/// verify.
///
/// This checks the token alone. Whether its `sub` still names a stored user
/// is [`require_signed_on_user`]'s check, which [`AuthUser`] makes next.
///
/// The user is also kept in the head's extensions, where [`verified_user`]
/// finds it. axum hands the same extensions to every later extractor of the
/// request, so the extractor of its body, query or path can name the user
/// when it refuses the request. [`AuthUser`] removes it again when the
/// subject check refuses the request.
///
/// # Errors
///
/// [`ApiError::unauthenticated`] (401) when the request has no single bearer
/// token, or when `keys` reject the token: malformed, signed with another key
/// or algorithm, expired, missing a required claim, or with a `usr_type`
/// claim other than `A` or `U`. No user is kept then. The refusal is recorded
/// on the request span (`crate::record_refusal`, no user), then logged at
/// `warn` level with its fixed `reason` only: `missing_or_malformed_header`,
/// or [`JwtError::reason`](crate::auth::JwtError::reason).
pub(crate) fn authenticate(parts: &mut Parts, keys: &JwtKeys) -> Result<AuthUser, ApiError> {
    let user = bearer_token(&parts.headers)
        .ok_or(REASON_MISSING_OR_MALFORMED_HEADER)
        .and_then(|token| keys.verify(token).map_err(|err| err.reason()))
        .map(AuthUser::from_claims)
        .map_err(|reason| {
            crate::record_refusal(
                crate::transaction_of(&parts.method, &parts.uri, &parts.extensions),
                None,
            );
            tracing::warn!(reason, "bearer token rejected");
            ApiError::unauthenticated()
        })?;
    parts.extensions.insert(user.clone());
    Ok(user)
}

/// The user the request's bearer token names, when `keys` verify the token:
/// [`bearer_token`] and [`JwtKeys::verify`] as [`authenticate`] runs them,
/// without its refusal.
///
/// `None` when the request has no single bearer token or its token does not
/// verify. No store is read, nothing is kept in the extensions, recorded or
/// logged, and the token's text goes nowhere.
pub(crate) fn bearer_user(headers: &HeaderMap, keys: &JwtKeys) -> Option<AuthUser> {
    let token = bearer_token(headers)?;
    keys.verify(token).ok().map(AuthUser::from_claims)
}

/// The user a request's bearer token names when the token verifies, or
/// `None`: [`bearer_user`] as an extractor that never refuses a request.
///
/// For the public `POST /auth/logout`, which requires no token and names the
/// user of one that verifies in its span only. Unlike [`AuthUser`], it reads
/// no store, keeps nothing in the request's extensions, records nothing on
/// the request span and logs nothing. The keys are the state's
/// `Arc<JwtKeys>`, so a handler that takes only this extractor reaches no
/// other part of the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerUser(pub Option<AuthUser>);

impl<S> FromRequestParts<S> for BearerUser
where
    Arc<JwtKeys>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Infallible;

    /// Verifies the request's bearer token, if any, with the state's keys.
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let keys = Arc::<JwtKeys>::from_ref(state);
        Ok(Self(bearer_user(&parts.headers, &keys)))
    }
}

/// The user [`authenticate`] verified for the request whose `extensions`
/// these are, or `None` when no extractor has authenticated it yet, when its
/// token did not verify or names no stored user, or when its route is public.
pub(crate) fn verified_user(extensions: &Extensions) -> Option<&AuthUser> {
    extensions.get::<AuthUser>()
}

/// COSGN00C READ-USER-SEC-FILE [app/cbl/COSGN00C.cbl:L209-219], repeated for
/// a verified token: `user` when `USRSEC` still holds a user with its ID.
///
/// A token verifies until it expires, while the user its `sub` names may have
/// been deleted since sign-on (`DELETE /admin/users/{id}`), or may never have
/// existed when the token was signed with the key outside `POST /auth/login`.
/// The ID is read back by the key sign-on reads, byte for byte, through
/// [`UserStore::read_user_key`], which selects the key alone: nothing else of
/// the stored record is compared, so neither the names, the type nor the
/// password hash is read. The actor's type stays the verified token's
/// `usr_type`.
///
/// `transaction` is the code of the operation the request asks for
/// (`crate::transaction_of`). A refusal is recorded on the request span with
/// it before it is logged: `crate::record_refusal` for the 401, with no user,
/// and `crate::record_failure` for a failed read, with the token's `sub`.
///
/// # Errors
///
/// - [`ApiError::unauthenticated`] (401) when no user has the ID
///   ([`PortError::NotFound`]), or when the `sub` claim cannot be a
///   `SEC-USR-ID PIC X(08)` (more than 8 characters, or one above `U+00FF`)
///   and so is never read. Each is logged at `warn` level with the reason
///   `unknown_subject` only, never the user ID.
/// - The 500 of [`ApiError::from`] a [`PortError`] for any other failure of
///   the read, whose detail goes to the log only.
pub(crate) async fn require_signed_on_user<S: UserStore>(
    store: &mut S,
    user: AuthUser,
    transaction: Option<&'static str>,
) -> Result<AuthUser, ApiError> {
    let Ok(id) = UserId::from_trimmed(&user.user_id) else {
        return Err(unknown_subject(transaction));
    };
    match store.read_user_key(id).await {
        Ok(_) => Ok(user),
        Err(PortError::NotFound) => Err(unknown_subject(transaction)),
        Err(err) => {
            crate::record_failure(transaction, &user.user_id);
            Err(ApiError::from(err))
        }
    }
}

/// The 401 of a verified token whose `sub` names no stored user, recorded on
/// the request span for `transaction`, then logged with its fixed reason
/// only.
fn unknown_subject(transaction: Option<&'static str>) -> ApiError {
    crate::record_refusal(transaction, None);
    tracing::warn!(reason = REASON_UNKNOWN_SUBJECT, "bearer token rejected");
    ApiError::unauthenticated()
}

/// `user`, signed on for the request `parts` heads, as an [`AdminUser`] when
/// its type is [`UserType::Admin`].
///
/// # Errors
///
/// [`ApiError::forbidden`] (403) for every other type. The refusal is
/// recorded on the request span with the user ID (`crate::record_refusal`),
/// then logged at `warn` level with it: the verified `sub` claim, not a
/// secret.
pub(crate) fn require_admin(parts: &Parts, user: AuthUser) -> Result<AdminUser, ApiError> {
    if user.is_admin() {
        Ok(AdminUser(user))
    } else {
        crate::record_refusal(
            crate::transaction_of(&parts.method, &parts.uri, &parts.extensions),
            Some(&user.user_id),
        );
        tracing::warn!(user_id = %user.user_id, "admin access denied");
        Err(ApiError::forbidden())
    }
}

/// The subject check of `user` ([`require_signed_on_user`]) for the
/// operation `transaction`, in a unit of work of its own.
///
/// The read writes nothing, so the unit of work is rolled back once it has
/// answered, whatever it answered, and its connection goes back to the pool
/// before the handler opens its own.
///
/// # Errors
///
/// The subject check's error, or 500 when the unit of work cannot be opened
/// or rolled back. Each 500 is recorded on the request span with the token's
/// `sub` (`crate::record_failure`) before its detail is logged.
async fn signed_on_user(
    state: &AppState,
    user: AuthUser,
    transaction: Option<&'static str>,
) -> Result<AuthUser, ApiError> {
    // The check takes the user by value, and a rollback that fails after it
    // still names the token's `sub`.
    let user_id = user.user_id.clone();
    let failed = |err: PersistenceError| {
        crate::record_failure(transaction, &user_id);
        ApiError::from(err)
    };
    let mut uow = state.db().begin().await.map_err(failed)?;
    let signed_on = require_signed_on_user(&mut uow.store(), user, transaction).await;
    uow.rollback().await.map_err(failed)?;
    signed_on
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    /// Verifies the bearer token (`authenticate`), then reads its user
    /// (`require_signed_on_user`) in a unit of work of its own
    /// (`signed_on_user`). A request the subject check refuses keeps no user
    /// in its extensions.
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = authenticate(parts, state.jwt())?;
        let transaction = crate::transaction_of(&parts.method, &parts.uri, &parts.extensions);
        let signed_on = signed_on_user(state, user, transaction).await;
        if signed_on.is_err() {
            parts.extensions.remove::<Self>();
        }
        signed_on
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    /// Authenticates before it checks the role, so a request without a valid
    /// token, or whose token names no stored user (a deleted admin's
    /// included), is 401, never 403.
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        require_admin(parts, user)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::State;
    use axum::http::header::AUTHORIZATION;
    use axum::http::request::Parts;
    use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use carddemo_domain::entities::{UserProfile, UserSecurity};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{Alnum, UserId};
    use carddemo_domain::ports::{Browse, PortError, PortResult, UserStore};
    use carddemo_runtime::telemetry::subscriber_with_writer;
    use carddemo_testkit::block_on;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde::Serialize;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{
        AdminUser, AuthUser, authenticate, bearer_token, require_admin, require_signed_on_user,
        verified_user,
    };
    use crate::auth::jwt::{Claims, JwtKeys, UserType};
    use crate::error::ApiError;

    const SECRET: &str = "test-secret";
    const TTL_SECS: u64 = 3600;

    fn keys() -> JwtKeys {
        JwtKeys::new(SECRET, TTL_SECS)
    }

    fn token(keys: &JwtKeys, user_id: &str, usr_type: UserType) -> String {
        keys.issue(user_id, usr_type).expect("issues").token
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock is after the epoch")
            .as_secs()
    }

    /// `claims` signed under `algorithm` with the test secret, without
    /// `JwtKeys::issue`, so the claims and the algorithm can be anything.
    fn sign(algorithm: Algorithm, claims: &impl Serialize) -> String {
        jsonwebtoken::encode(
            &Header::new(algorithm),
            claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("signs")
    }

    /// `claims` as JSON with the `usr_type` claim replaced by `role`, which
    /// [`Claims`] cannot hold when it is not the string `A` or `U`.
    fn with_role(claims: &Claims, role: Value) -> Value {
        let mut value = serde_json::to_value(claims).expect("serializes");
        value["usr_type"] = role;
        value
    }

    /// One `Authorization` header per value, in order.
    fn headers(values: &[HeaderValue]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append(AUTHORIZATION, value.clone());
        }
        map
    }

    fn authorization(text: &str) -> HeaderMap {
        headers(&[HeaderValue::from_str(text).expect("a header value")])
    }

    /// The head of a `GET /` request with `headers`, as an extractor gets it
    /// outside a router: no matched route, so no transaction code.
    fn head(headers: HeaderMap) -> Parts {
        let (mut parts, ()) = Request::new(()).into_parts();
        parts.headers = headers;
        parts
    }

    fn assert_unauthenticated(result: Result<AuthUser, ApiError>, case: &str) {
        let err = result.expect_err(case);
        assert_eq!(err.body(), ApiError::unauthenticated().body(), "{case}");
        assert_eq!(
            err.into_response().status(),
            StatusCode::UNAUTHORIZED,
            "{case}"
        );
    }

    fn assert_forbidden(result: Result<AdminUser, ApiError>, case: &str) {
        let err = result.expect_err(case);
        assert_eq!(err.body(), ApiError::forbidden().body(), "{case}");
        assert_eq!(
            err.into_response().status(),
            StatusCode::FORBIDDEN,
            "{case}"
        );
    }

    async fn message_of(err: ApiError) -> (StatusCode, Value) {
        let response = err.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is buffered");
        let body: Value = serde_json::from_slice(&bytes).expect("a JSON body");
        (status, body["message"].clone())
    }

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

    /// What `run` returns, and the JSON log lines it writes through the API's
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

    #[test]
    fn bearer_token_takes_the_single_bearer_credential() {
        for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
            let map = authorization(&format!("{scheme} a.b.c"));
            assert_eq!(bearer_token(&map), Some("a.b.c"), "{scheme}");
        }

        let malformed = [
            "",
            "a.b.c",
            "Bearer",
            "Bearer ",
            "Bearer  a.b.c",
            " Bearer a.b.c",
            "Bearer a.b.c ",
            "Bearer a.b c",
            "Bearer\ta.b.c",
            "Bearer: a.b.c",
            "Bearers a.b.c",
            "Token a.b.c",
            "Basic dXNlcjpwYXNz",
        ];
        for text in malformed {
            assert_eq!(bearer_token(&authorization(text)), None, "{text:?}");
        }

        assert_eq!(bearer_token(&HeaderMap::new()), None);
        let valid = HeaderValue::from_static("Bearer a.b.c");
        assert_eq!(bearer_token(&headers(&[valid.clone(), valid])), None);
        for bytes in [&b"\xFF"[..], &b"Bearer a.b.\xFF"[..]] {
            let value = HeaderValue::from_bytes(bytes).expect("obs-text is a header value");
            assert_eq!(bearer_token(&headers(&[value])), None, "{bytes:?}");
        }
    }

    #[test]
    fn valid_token_in_a_malformed_header_is_401() {
        let keys = keys();
        let token = token(&keys, "ADMIN001", UserType::Admin);
        let bearer = HeaderValue::from_str(&format!("Bearer {token}")).expect("a header value");
        let mut non_ascii = format!("Bearer {token}").into_bytes();
        non_ascii.push(0xFF);

        let cases = [
            ("no header", HeaderMap::new()),
            ("bare token", authorization(&token)),
            ("basic scheme", authorization(&format!("Basic {token}"))),
            ("scheme only", authorization("Bearer")),
            ("empty token", authorization("Bearer ")),
            ("two spaces", authorization(&format!("Bearer  {token}"))),
            ("trailing space", authorization(&format!("Bearer {token} "))),
            ("two headers", headers(&[bearer.clone(), bearer])),
            (
                "non-ASCII byte",
                headers(&[HeaderValue::from_bytes(&non_ascii).expect("a header value")]),
            ),
        ];
        for (case, map) in cases {
            assert_unauthenticated(authenticate(&mut head(map), &keys), case);
        }
    }

    #[test]
    fn rejected_token_is_401() {
        let keys = keys();
        let now = now();
        let expired = Claims {
            sub: "ADMIN001".to_owned(),
            usr_type: UserType::Admin,
            iat: now - TTL_SECS - 10,
            exp: now - 10,
        };
        let current = Claims {
            iat: now,
            exp: now + 60,
            ..expired.clone()
        };
        let cases = [
            ("garbage", "not-a-jwt".to_owned()),
            ("three garbage segments", "a.b.c".to_owned()),
            (
                "another secret",
                token(
                    &JwtKeys::new("other-secret", TTL_SECS),
                    "ADMIN001",
                    UserType::Admin,
                ),
            ),
            ("expired 10 s ago", sign(Algorithm::HS256, &expired)),
            ("HS384", sign(Algorithm::HS384, &current)),
            (
                "role X",
                sign(Algorithm::HS256, &with_role(&current, json!("X"))),
            ),
            (
                "role a",
                sign(Algorithm::HS256, &with_role(&current, json!("a"))),
            ),
            (
                "role A as an enum map",
                sign(Algorithm::HS256, &with_role(&current, json!({"A": null}))),
            ),
        ];
        for (case, token) in cases {
            let map = authorization(&format!("Bearer {token}"));
            assert_unauthenticated(authenticate(&mut head(map), &keys), case);
        }

        let map = authorization(&format!("Bearer {}", sign(Algorithm::HS256, &current)));
        assert!(
            authenticate(&mut head(map), &keys).is_ok(),
            "the control token verifies"
        );
    }

    #[test]
    fn admin_token_passes_the_admin_check_with_any_scheme_case() {
        let keys = keys();
        let token = token(&keys, "ADMIN001", UserType::Admin);
        for scheme in ["Bearer", "bearer"] {
            let mut parts = head(authorization(&format!("{scheme} {token}")));
            let user = authenticate(&mut parts, &keys).expect("an admin token authenticates");
            assert_eq!(
                user,
                AuthUser {
                    user_id: "ADMIN001".to_owned(),
                    usr_type: UserType::Admin,
                },
                "{scheme}"
            );
            assert!(user.is_admin(), "{scheme}");

            let admin = require_admin(&parts, user.clone()).expect("an admin passes");
            assert_eq!(admin, AdminUser(user), "{scheme}");
            assert_eq!(admin.user_id, "ADMIN001", "{scheme}");
            assert!(admin.is_admin(), "{scheme}");
        }
    }

    #[test]
    fn every_type_but_a_is_a_user_and_403_on_the_admin_check() {
        let keys = keys();
        // (user ID, stored `SEC-USR-TYPE`), issued as sign-on issues it.
        for (user_id, code) in [
            ("USER0001", "U"),
            ("user0001", "X"),
            ("ADMIN001", "a"),
            ("ADMIN001", ""),
            ("ADMIN001", "A "),
            ("ADMIN001", "AA"),
        ] {
            let issued = token(&keys, user_id, UserType::from_code(code));
            let mut parts = head(authorization(&format!("Bearer {issued}")));
            let user = authenticate(&mut parts, &keys).expect("a valid token authenticates");
            assert_eq!(user.user_id, user_id, "the sub claim verbatim for {code:?}");
            assert_eq!(user.usr_type, UserType::User, "{code:?}");
            assert!(!user.is_admin(), "{code:?}");
            assert_forbidden(require_admin(&parts, user), code);
        }
    }

    #[test]
    fn authentication_keeps_the_verified_user_in_the_extensions_and_a_refusal_keeps_none() {
        let keys = keys();
        let user = AuthUser {
            user_id: "USER0001".to_owned(),
            usr_type: UserType::User,
        };
        let admin = AuthUser {
            user_id: "ADMIN001".to_owned(),
            usr_type: UserType::Admin,
        };
        let forged = token(
            &JwtKeys::new("other-secret", TTL_SECS),
            "ADMIN001",
            UserType::Admin,
        );
        // (case, the `Authorization` value if any, the user kept, if any)
        let cases = [
            (
                "user token",
                Some(format!(
                    "Bearer {}",
                    token(&keys, "USER0001", UserType::User)
                )),
                Some(&user),
            ),
            (
                "admin token",
                Some(format!(
                    "bearer {}",
                    token(&keys, "ADMIN001", UserType::Admin)
                )),
                Some(&admin),
            ),
            ("no header", None, None),
            ("bad signature", Some(format!("Bearer {forged}")), None),
            ("garbage token", Some("Bearer not-a-jwt".to_owned()), None),
            ("basic auth", Some(format!("Basic {forged}")), None),
        ];
        for (case, value, kept) in cases {
            let mut parts = head(value.as_deref().map_or_else(HeaderMap::new, authorization));
            assert_eq!(verified_user(&parts.extensions), None, "{case}: before");
            let result = authenticate(&mut parts, &keys);
            assert_eq!(result.as_ref().ok(), kept, "{case}");
            assert_eq!(verified_user(&parts.extensions), kept, "{case}: after");
        }
    }

    #[test]
    fn a_401_is_logged_at_warn_with_its_reason_only() {
        let keys = keys();
        let now = now();
        let current = Claims {
            sub: "ADMIN001".to_owned(),
            usr_type: UserType::Admin,
            iat: now,
            exp: now + 60,
        };
        let expired = Claims {
            iat: now - TTL_SECS - 10,
            exp: now - 10,
            ..current.clone()
        };
        let other_secret = JwtKeys::new("other-secret", TTL_SECS);
        let admin = token(&keys, "ADMIN001", UserType::Admin);
        let bearer = |token: &str| Some(format!("Bearer {token}"));
        // (case, the `Authorization` value if any, the logged reason)
        let cases = [
            ("no header", None, "missing_or_malformed_header"),
            (
                "basic scheme",
                Some(format!("Basic {admin}")),
                "missing_or_malformed_header",
            ),
            ("garbage", bearer("not-a-jwt"), "malformed"),
            (
                "another secret",
                bearer(&token(&other_secret, "ADMIN001", UserType::Admin)),
                "bad_signature",
            ),
            (
                "HS384",
                bearer(&sign(Algorithm::HS384, &current)),
                "wrong_algorithm",
            ),
            (
                "expired",
                bearer(&sign(Algorithm::HS256, &expired)),
                "expired",
            ),
            (
                "role X",
                bearer(&sign(Algorithm::HS256, &with_role(&current, json!("X")))),
                "invalid_claims",
            ),
        ];
        for (case, value, reason) in cases {
            let mut parts = head(value.as_deref().map_or_else(HeaderMap::new, authorization));
            let (result, lines) = logged(|| authenticate(&mut parts, &keys));
            assert_unauthenticated(result, case);
            let [line] = lines.as_slice() else {
                panic!("{case}: one event expected, got {lines:?}");
            };
            assert_eq!(line["level"], "WARN", "{case}");
            assert_eq!(line["target"], "carddemo_api::auth::extract", "{case}");
            assert_eq!(
                line["fields"],
                json!({"message": "bearer token rejected", "reason": reason}),
                "{case}"
            );
            let text = line.to_string();
            for withheld in [
                SECRET,
                "other-secret",
                "JSON error",
                "unknown variant",
                "invalid value",
            ] {
                assert!(!text.contains(withheld), "{case}: {text}");
            }
            if let Some(value) = &value {
                let presented = value.split_once(' ').map_or(value.as_str(), |(_, t)| t);
                assert!(!text.contains(presented), "{case}: {text}");
            }
        }
    }

    #[test]
    fn a_403_is_logged_at_warn_with_the_user_id_and_a_pass_is_not_logged() {
        let keys = keys();
        let user_token = token(&keys, "USER0001", UserType::User);
        let mut user_parts = head(authorization(&format!("Bearer {user_token}")));
        let (denied, lines) = logged(|| {
            authenticate(&mut user_parts, &keys).and_then(|user| require_admin(&user_parts, user))
        });
        assert_forbidden(denied, "a user token");
        let [line] = lines.as_slice() else {
            panic!("one event expected, got {lines:?}");
        };
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["target"], "carddemo_api::auth::extract");
        assert_eq!(
            line["fields"],
            json!({"message": "admin access denied", "user_id": "USER0001"})
        );
        assert!(!line.to_string().contains(&user_token), "{line}");

        let mut admin_parts = head(authorization(&format!(
            "Bearer {}",
            token(&keys, "ADMIN001", UserType::Admin)
        )));
        let (admitted, lines) = logged(|| {
            authenticate(&mut admin_parts, &keys).and_then(|user| require_admin(&admin_parts, user))
        });
        assert!(admitted.is_ok(), "an admin token passes");
        assert!(lines.is_empty(), "{lines:?}");
    }

    /// The `GET /accounts/{id}` stand-in: [`AuthUser`]'s extraction as its
    /// `FromRequestParts` impl runs it, on a router without an `AppState`.
    async fn signed_on(
        State(keys): State<Arc<JwtKeys>>,
        mut parts: Parts,
    ) -> Result<String, ApiError> {
        authenticate(&mut parts, &keys).map(|user| user.user_id)
    }

    /// The `GET /admin/users` stand-in: [`AdminUser`]'s extraction as its
    /// `FromRequestParts` impl runs it.
    async fn signed_on_admin(
        State(keys): State<Arc<JwtKeys>>,
        mut parts: Parts,
    ) -> Result<String, ApiError> {
        let user = authenticate(&mut parts, &keys)?;
        require_admin(&parts, user).map(|admin| admin.0.user_id)
    }

    /// The status of `GET uri` with the `Authorization` value `credential`,
    /// if any, through the API's layers over the two stand-ins, and the JSON
    /// lines the API's telemetry subscriber writes meanwhile.
    fn served(uri: &str, credential: Option<&str>) -> (StatusCode, Vec<Value>) {
        let routes = Router::new()
            .route("/accounts/{id}", get(signed_on))
            .route("/admin/users", get(signed_on_admin))
            .with_state(Arc::new(keys()));
        let app = crate::with_layers(routes, HeaderValue::from_static("http://localhost:5173"));
        let request = credential
            .into_iter()
            .fold(Request::get(uri), |builder, value| {
                builder.header(AUTHORIZATION, value)
            })
            .body(Body::empty())
            .expect("a valid request");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        logged(|| {
            runtime.block_on(async {
                app.oneshot(request)
                    .await
                    .expect("the router is infallible")
                    .status()
            })
        })
    }

    /// A refused request's `transaction` and verified `user_id`, if any.
    type Refusal = (&'static str, Option<&'static str>);

    /// Asserts that `lines`, written for one request, hold one request span
    /// line. For a `refusal` it carries the refusal's `transaction`, `outcome`
    /// `rejected` and `user_id`, if any, and the one warning carries the same
    /// in its span context. For `None` it carries none of the three, and no
    /// warning is written.
    fn assert_refusal_recorded(case: &str, lines: &[Value], refusal: Option<Refusal>) {
        let spans: Vec<&Value> = lines
            .iter()
            .filter(|line| line["type"] == "span" && line["name"] == "request")
            .collect();
        let [span] = spans.as_slice() else {
            panic!("{case}: one request span expected, got {lines:?}");
        };
        let attributes = &span["attributes"];
        let warnings: Vec<&Value> = lines
            .iter()
            .filter(|line| line["level"] == "WARN")
            .collect();
        let Some((transaction, user_id)) = refusal else {
            for field in ["transaction", "user_id", "outcome"] {
                assert!(attributes.get(field).is_none(), "{case}: {span}");
            }
            assert!(warnings.is_empty(), "{case}: {warnings:?}");
            return;
        };
        let [warning] = warnings.as_slice() else {
            panic!("{case}: one warning expected, got {lines:?}");
        };
        let context = &warning["span"];
        assert_eq!(context["name"], "request", "{case}: {warning}");
        for recorded in [attributes, context] {
            assert_eq!(recorded["transaction"], transaction, "{case}: {recorded}");
            assert_eq!(recorded["outcome"], "rejected", "{case}: {recorded}");
            assert_eq!(
                recorded.get("user_id").and_then(Value::as_str),
                user_id,
                "{case}: {recorded}"
            );
        }
    }

    #[test]
    fn a_refusal_is_on_the_request_span_and_in_its_event_s_span_context() {
        const ACCOUNT: &str = "/accounts/00000000001";
        const ADMIN_USERS: &str = "/admin/users";
        const CAVW: Option<Refusal> = Some(("CAVW", None));
        let keys = keys();
        let user = format!("Bearer {}", token(&keys, "USER0001", UserType::User));
        let admin = format!("Bearer {}", token(&keys, "ADMIN001", UserType::Admin));
        let other_secret = JwtKeys::new("other-secret", TTL_SECS);
        let forged = format!(
            "Bearer {}",
            token(&other_secret, "ADMIN001", UserType::Admin)
        );
        let garbage = "Bearer not-a-jwt".to_owned();
        let basic = "Basic QURNSU4wMDE6UEFTU1dPUkQ=".to_owned();
        let denied = Some(("CU00", Some("USER0001")));
        // (case, URI, `Authorization`, status, the refused request's
        // `transaction` and `user_id`, or `None` when the request passes)
        let cases = [
            ("no token", ACCOUNT, None, 401, CAVW),
            ("bad signature", ACCOUNT, Some(&forged), 401, CAVW),
            ("garbage token", ACCOUNT, Some(&garbage), 401, CAVW),
            ("basic auth", ACCOUNT, Some(&basic), 401, CAVW),
            (
                "admin, no token",
                ADMIN_USERS,
                None,
                401,
                Some(("CU00", None)),
            ),
            ("admin, user token", ADMIN_USERS, Some(&user), 403, denied),
            ("user token", ACCOUNT, Some(&user), 200, None),
            ("admin, admin token", ADMIN_USERS, Some(&admin), 200, None),
        ];
        for (case, uri, credential, status, refusal) in cases {
            let (answered, lines) = served(uri, credential.map(String::as_str));
            assert_eq!(answered.as_u16(), status, "{case}");
            assert_refusal_recorded(case, &lines, refusal);

            for line in &lines {
                let text = line.to_string();
                for withheld in [SECRET, "other-secret", "00000000001"] {
                    assert!(!text.contains(withheld), "{case}: {text}");
                }
                for value in [&user, &admin, &forged, &garbage, &basic] {
                    let presented = value.split_once(' ').map_or(value.as_str(), |(_, t)| t);
                    assert!(!text.contains(presented), "{case}: {text}");
                }
            }
        }
    }

    #[tokio::test]
    async fn rejections_carry_the_catalog_messages() {
        let keys = keys();
        let missing = authenticate(&mut head(HeaderMap::new()), &keys).expect_err("no token");
        assert_eq!(
            message_of(missing).await,
            (
                StatusCode::UNAUTHORIZED,
                Value::from(messages::AUTHENTICATION_REQUIRED)
            )
        );

        let mut parts = head(authorization(&format!(
            "Bearer {}",
            token(&keys, "USER0001", UserType::User)
        )));
        let user = authenticate(&mut parts, &keys).expect("a user token authenticates");
        let denied = require_admin(&parts, user).expect_err("a user is not an admin");
        assert_eq!(
            message_of(denied).await,
            (
                StatusCode::FORBIDDEN,
                Value::from(messages::ADMIN_ACCESS_REQUIRED)
            )
        );
    }

    /// The adapter detail of a failed `USRSEC` read, which must reach the log
    /// only.
    const READ_DETAIL: &str = "connection reset by 10.0.0.7:5432 while reading users";

    /// A `USRSEC` holding at most one user, which records the key of every
    /// `read_user_key` and fails every other call, since the subject check
    /// makes none: it reads neither the record nor its hash.
    #[derive(Default)]
    struct OneUserStore {
        /// The stored user, if any.
        user: Option<UserSecurity>,
        /// The failure every read answers instead, if any.
        failure: Option<PortError>,
        /// The key of each `read_user_key`, in order.
        reads: Vec<UserId>,
    }

    /// The `SEC-USR-ID` key of `user_id`, as persistence builds it from the
    /// stored text.
    fn key(user_id: &str) -> UserId {
        UserId::from_trimmed(user_id).expect("an X(08) user ID")
    }

    impl OneUserStore {
        /// A store holding the user `user_id` with the `SEC-USR-TYPE` code.
        fn holding(user_id: &str, code: &str) -> Self {
            Self {
                user: Some(UserSecurity {
                    sec_usr_id: key(user_id),
                    sec_usr_fname: Alnum::move_from("FIRST"),
                    sec_usr_lname: Alnum::move_from("LAST"),
                    password_hash: String::new(),
                    sec_usr_type: Alnum::move_from(code),
                }),
                ..Self::default()
            }
        }

        /// A store whose every read fails with `failure`.
        fn failing(failure: PortError) -> Self {
            Self {
                failure: Some(failure),
                ..Self::default()
            }
        }
    }

    /// The failure of a call the subject check never makes.
    fn not_made(call: &str) -> PortError {
        PortError::Unavailable(format!("the subject check makes no {call}"))
    }

    impl UserStore for OneUserStore {
        async fn read_user(&mut self, _id: UserId) -> PortResult<UserSecurity> {
            Err(not_made("read_user"))
        }

        async fn read_user_key(&mut self, id: UserId) -> PortResult<UserId> {
            self.reads.push(id);
            if let Some(failure) = &self.failure {
                return Err(failure.clone());
            }
            self.user
                .as_ref()
                .map(|user| user.sec_usr_id)
                .filter(|stored| *stored == id)
                .ok_or(PortError::NotFound)
        }

        async fn read_user_for_update(&mut self, _id: UserId) -> PortResult<UserSecurity> {
            Err(not_made("read_user_for_update"))
        }

        async fn read_user_profile_for_update(&mut self, _id: UserId) -> PortResult<UserProfile> {
            Err(not_made("read_user_profile_for_update"))
        }

        async fn browse_users(&mut self, _browse: Browse<UserId>) -> PortResult<Vec<UserProfile>> {
            Err(not_made("browse_users"))
        }

        async fn insert_user(&mut self, _user: UserSecurity) -> PortResult<()> {
            Err(not_made("insert_user"))
        }

        async fn update_user(&mut self, _user: UserSecurity) -> PortResult<()> {
            Err(not_made("update_user"))
        }

        async fn delete_user(&mut self, _id: UserId) -> PortResult<()> {
            Err(not_made("delete_user"))
        }
    }

    fn token_user(user_id: &str, usr_type: UserType) -> AuthUser {
        AuthUser {
            user_id: user_id.to_owned(),
            usr_type,
        }
    }

    /// The subject check of `user` against `store`, and the log lines it
    /// writes.
    fn checked(
        store: &mut OneUserStore,
        user: AuthUser,
    ) -> (Result<AuthUser, ApiError>, Vec<Value>) {
        logged(|| block_on(require_signed_on_user(store, user, None)))
    }

    #[test]
    fn a_stored_subject_passes_unchanged_with_the_type_of_its_token() {
        // (user ID, stored `SEC-USR-TYPE`, the token's type). The stored type
        // is never compared: a demoted admin keeps the token's `A`.
        for (user_id, code, usr_type) in [
            ("ADMIN001", "A", UserType::Admin),
            ("USER0001", "U", UserType::User),
            ("ADMIN002", "U", UserType::Admin),
            ("ADMéN01", "A", UserType::Admin),
        ] {
            let mut store = OneUserStore::holding(user_id, code);
            let user = token_user(user_id, usr_type);
            let (result, lines) = checked(&mut store, user.clone());
            assert_eq!(result.expect(user_id), user, "{user_id}");
            assert!(lines.is_empty(), "{user_id}: {lines:?}");
            assert_eq!(store.reads, [key(user_id)], "{user_id}");
        }
    }

    #[test]
    fn an_unknown_subject_is_401_logged_with_its_reason_only() {
        let admin = || OneUserStore::holding("ADMIN001", "A");
        // (case, store, the token's `sub`, whether the ID is read)
        let cases = [
            ("deleted", OneUserStore::default(), "ADMIN001", true),
            ("never stored", admin(), "NOBODY99", true),
            ("another case", admin(), "admin001", true),
            ("9 characters", admin(), "ADMIN0012", false),
            ("above U+00FF", admin(), "ADM€N001", false),
        ];
        for (case, mut store, sub, read) in cases {
            let (result, lines) = checked(&mut store, token_user(sub, UserType::Admin));
            let err = result.expect_err(case);
            assert_eq!(err.status(), StatusCode::UNAUTHORIZED, "{case}");
            assert_eq!(
                serde_json::to_value(err.body()).expect("the body serializes"),
                json!({"code": "UNAUTHENTICATED", "message": messages::AUTHENTICATION_REQUIRED}),
                "{case}"
            );
            let expected_reads = if read { vec![key(sub)] } else { Vec::new() };
            assert_eq!(store.reads, expected_reads, "{case}");
            let [line] = lines.as_slice() else {
                panic!("{case}: one event expected, got {lines:?}");
            };
            assert_eq!(line["level"], "WARN", "{case}");
            assert_eq!(line["target"], "carddemo_api::auth::extract", "{case}");
            assert_eq!(
                line["fields"],
                json!({"message": "bearer token rejected", "reason": "unknown_subject"}),
                "{case}"
            );
            assert!(!line.to_string().contains(sub), "{case}: {line}");
        }
    }

    #[test]
    fn a_failed_user_read_is_500_without_its_detail() {
        for failure in [
            PortError::Unavailable(READ_DETAIL.to_owned()),
            PortError::Contention,
        ] {
            let case = failure.to_string();
            let mut store = OneUserStore::failing(failure);
            let (result, lines) = checked(&mut store, token_user("ADMIN001", UserType::Admin));
            let err = result.expect_err(&case);
            assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR, "{case}");
            assert_eq!(err.body(), ApiError::unexpected().body(), "{case}");
            let body = serde_json::to_string(err.body()).expect("the body serializes");
            assert!(!body.contains(READ_DETAIL), "{case}: {body}");
            assert_eq!(store.reads, [key("ADMIN001")], "{case}");
            // The detail is logged once, as an error: a failed read is not an
            // unknown subject.
            let [line] = lines.as_slice() else {
                panic!("{case}: one event expected, got {lines:?}");
            };
            assert_eq!(line["level"], "ERROR", "{case}");
            assert_eq!(line["fields"]["error"], case.as_str(), "{case}");
        }
    }

    /// The subject check of `user` against `store` for a `CAVW` request,
    /// inside a stand-in for the request span with its three empty refusal
    /// fields, and the log lines it writes: the events, then the span line.
    fn checked_in_request_span(
        store: &mut OneUserStore,
        user: AuthUser,
    ) -> (Result<AuthUser, ApiError>, Vec<Value>) {
        logged(|| {
            tracing::info_span!(
                "request",
                transaction = tracing::field::Empty,
                user_id = tracing::field::Empty,
                outcome = tracing::field::Empty,
            )
            .in_scope(|| block_on(require_signed_on_user(store, user, Some("CAVW"))))
        })
    }

    #[test]
    fn a_subject_check_refusal_is_on_the_request_span_before_its_event() {
        // (case, store, the request span's `outcome`, the event's level, the
        // `user_id` recorded)
        let cases = [
            (
                "unknown subject",
                OneUserStore::default(),
                "rejected",
                "WARN",
                None,
            ),
            (
                "failed read",
                OneUserStore::failing(PortError::Unavailable(READ_DETAIL.to_owned())),
                "failed",
                "ERROR",
                Some("ADMIN001"),
            ),
        ];
        for (case, mut store, outcome, level, user_id) in cases {
            let (result, lines) =
                checked_in_request_span(&mut store, token_user("ADMIN001", UserType::Admin));
            assert!(result.is_err(), "{case}");
            let [event, span] = lines.as_slice() else {
                panic!("{case}: one event and one span line expected, got {lines:?}");
            };
            assert_eq!(event["level"], level, "{case}");
            // The event carries the refusal in its span context: a 500 names
            // the verified token's `sub`, and the 401 of a `sub` that names
            // no stored user records it nowhere.
            let mut context = json!({"name": "request", "transaction": "CAVW", "outcome": outcome});
            if let Some(user_id) = user_id {
                context["user_id"] = json!(user_id);
            }
            assert_eq!(event["span"], context, "{case}");
            assert_eq!(span["type"], "span", "{case}");
            assert_eq!(span["attributes"]["transaction"], "CAVW", "{case}");
            assert_eq!(span["attributes"]["outcome"], outcome, "{case}");
            assert_eq!(
                span["attributes"].get("user_id").and_then(Value::as_str),
                user_id,
                "{case}: {span}"
            );
            if user_id.is_none() {
                assert!(!event.to_string().contains("ADMIN001"), "{case}: {event}");
            }
        }
    }
}
