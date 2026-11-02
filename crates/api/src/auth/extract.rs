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
//!   or `U`.
//! - [`AdminUser`] authenticates first and checks the role second. A request
//!   without a valid token is therefore 401 on `/admin/**` routes too, and
//!   only a valid token whose `usr_type` is not `A` is 403
//!   ([`ApiError::forbidden`]).
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
//! `missing_or_malformed_header`, or the token's
//! [`JwtError::reason`](crate::auth::JwtError::reason). A 403 is the event
//! `admin access denied` with the verified `user_id`. No event carries the
//! token, the header value, a claim value the token was rejected for, the
//! decoder's message or the secret.

use std::ops::Deref;

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::auth::jwt::{Claims, JwtKeys, UserType};
use crate::error::ApiError;
use crate::state::AppState;

/// The authentication scheme of the `Authorization` header, compared without
/// regard to ASCII case.
const BEARER_SCHEME: &str = "Bearer";

/// The logged `reason` of a request without a single well-formed
/// `Authorization: Bearer <token>` header, the cases [`bearer_token`] refuses.
const REASON_MISSING_OR_MALFORMED_HEADER: &str = "missing_or_malformed_header";

/// The signed-on user of a request, from its verified bearer token.
///
/// It stands for the COMMAREA fields `CDEMO-USER-ID` and `CDEMO-USER-TYPE`
/// [app/cpy/COCOM01Y.cpy:L25-28]. Any route that takes it answers 401 to a
/// request without a valid token.
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
/// Every `/admin/**` route takes it. A request without a valid token is 401,
/// and a valid token of any other type is 403. It dereferences to the
/// [`AuthUser`], so a handler reads the admin's ID as `admin.user_id`.
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

/// The signed-on user of a request with `headers`, whose bearer token `keys`
/// verify.
///
/// # Errors
///
/// [`ApiError::unauthenticated`] (401) when the request has no single bearer
/// token, or when `keys` reject the token: malformed, signed with another key
/// or algorithm, expired, missing a required claim, or with a `usr_type`
/// claim other than `A` or `U`. Each is logged at `warn` level with its fixed
/// `reason` only: `missing_or_malformed_header`, or
/// [`JwtError::reason`](crate::auth::JwtError::reason).
pub(crate) fn authenticate(headers: &HeaderMap, keys: &JwtKeys) -> Result<AuthUser, ApiError> {
    let Some(token) = bearer_token(headers) else {
        tracing::warn!(
            reason = REASON_MISSING_OR_MALFORMED_HEADER,
            "bearer token rejected"
        );
        return Err(ApiError::unauthenticated());
    };
    keys.verify(token)
        .map(AuthUser::from_claims)
        .map_err(|err| {
            tracing::warn!(reason = err.reason(), "bearer token rejected");
            ApiError::unauthenticated()
        })
}

/// `user` as an [`AdminUser`] when its type is [`UserType::Admin`].
///
/// # Errors
///
/// [`ApiError::forbidden`] (403) for every other type, logged at `warn` level
/// with the user ID: the verified `sub` claim, not a secret.
pub(crate) fn require_admin(user: AuthUser) -> Result<AdminUser, ApiError> {
    if user.is_admin() {
        Ok(AdminUser(user))
    } else {
        tracing::warn!(user_id = %user.user_id, "admin access denied");
        Err(ApiError::forbidden())
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(&parts.headers, state.jwt())
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    /// Authenticates before it checks the role, so a request without a valid
    /// token is 401, never 403.
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        require_admin(user)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::to_bytes;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use carddemo_domain::messages;
    use carddemo_runtime::telemetry::subscriber_with_writer;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde::Serialize;
    use serde_json::{Value, json};

    use super::{AdminUser, AuthUser, authenticate, bearer_token, require_admin};
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
            assert_unauthenticated(authenticate(&map, &keys), case);
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
            assert_unauthenticated(authenticate(&map, &keys), case);
        }

        let map = authorization(&format!("Bearer {}", sign(Algorithm::HS256, &current)));
        assert!(
            authenticate(&map, &keys).is_ok(),
            "the control token verifies"
        );
    }

    #[test]
    fn admin_token_passes_the_admin_check_with_any_scheme_case() {
        let keys = keys();
        let token = token(&keys, "ADMIN001", UserType::Admin);
        for scheme in ["Bearer", "bearer"] {
            let map = authorization(&format!("{scheme} {token}"));
            let user = authenticate(&map, &keys).expect("an admin token authenticates");
            assert_eq!(
                user,
                AuthUser {
                    user_id: "ADMIN001".to_owned(),
                    usr_type: UserType::Admin,
                },
                "{scheme}"
            );
            assert!(user.is_admin(), "{scheme}");

            let admin = require_admin(user.clone()).expect("an admin passes");
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
            let map = authorization(&format!("Bearer {issued}"));
            let user = authenticate(&map, &keys).expect("a valid token authenticates");
            assert_eq!(user.user_id, user_id, "the sub claim verbatim for {code:?}");
            assert_eq!(user.usr_type, UserType::User, "{code:?}");
            assert!(!user.is_admin(), "{code:?}");
            assert_forbidden(require_admin(user), code);
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
            let map = value.as_deref().map_or_else(HeaderMap::new, authorization);
            let (result, lines) = logged(|| authenticate(&map, &keys));
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
        let user_map = authorization(&format!("Bearer {user_token}"));
        let (denied, lines) = logged(|| authenticate(&user_map, &keys).and_then(require_admin));
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

        let admin_map = authorization(&format!(
            "Bearer {}",
            token(&keys, "ADMIN001", UserType::Admin)
        ));
        let (admitted, lines) = logged(|| authenticate(&admin_map, &keys).and_then(require_admin));
        assert!(admitted.is_ok(), "an admin token passes");
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[tokio::test]
    async fn rejections_carry_the_catalog_messages() {
        let keys = keys();
        let missing = authenticate(&HeaderMap::new(), &keys).expect_err("no token");
        assert_eq!(
            message_of(missing).await,
            (
                StatusCode::UNAUTHORIZED,
                Value::from(messages::AUTHENTICATION_REQUIRED)
            )
        );

        let map = authorization(&format!(
            "Bearer {}",
            token(&keys, "USER0001", UserType::User)
        ));
        let user = authenticate(&map, &keys).expect("a user token authenticates");
        let denied = require_admin(user).expect_err("a user is not an admin");
        assert_eq!(
            message_of(denied).await,
            (
                StatusCode::FORBIDDEN,
                Value::from(messages::ADMIN_ACCESS_REQUIRED)
            )
        );
    }
}
