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
//!   key or algorithm, or expired.
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
//! A rejection is logged at `debug` level with the failure kind or the user
//! ID, never with the token or the header value.

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
    /// The type the token's `usr_type` claim stands for, read by
    /// [`UserType::from_code`]: only exactly `A` is [`UserType::Admin`].
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
            usr_type: UserType::from_code(&usr_type),
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
/// or algorithm, expired, or missing a required claim. Only the failure kind
/// is logged.
pub(crate) fn authenticate(headers: &HeaderMap, keys: &JwtKeys) -> Result<AuthUser, ApiError> {
    let Some(token) = bearer_token(headers) else {
        tracing::debug!("bearer token missing or malformed");
        return Err(ApiError::unauthenticated());
    };
    keys.verify(token)
        .map(AuthUser::from_claims)
        .map_err(|err| {
            tracing::debug!(error = %err, "bearer token rejected");
            ApiError::unauthenticated()
        })
}

/// `user` as an [`AdminUser`] when its type is [`UserType::Admin`].
///
/// # Errors
///
/// [`ApiError::forbidden`] (403) for every other type. The user ID is logged;
/// it is not a secret.
pub(crate) fn require_admin(user: AuthUser) -> Result<AdminUser, ApiError> {
    if user.is_admin() {
        Ok(AdminUser(user))
    } else {
        tracing::debug!(user_id = %user.user_id, "admin access denied");
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::to_bytes;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use carddemo_domain::messages;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde_json::Value;

    use super::{AdminUser, AuthUser, authenticate, bearer_token, require_admin};
    use crate::auth::jwt::{Claims, JwtKeys, UserType};
    use crate::error::ApiError;

    const SECRET: &str = "test-secret";
    const TTL_SECS: u64 = 3600;

    fn keys() -> JwtKeys {
        JwtKeys::new(SECRET, TTL_SECS)
    }

    fn token(keys: &JwtKeys, user_id: &str, usr_type: &str) -> String {
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
    fn sign(algorithm: Algorithm, claims: &Claims) -> String {
        jsonwebtoken::encode(
            &Header::new(algorithm),
            claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("signs")
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
        let token = token(&keys, "ADMIN001", "A");
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
            usr_type: "A".to_owned(),
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
                token(&JwtKeys::new("other-secret", TTL_SECS), "ADMIN001", "A"),
            ),
            ("expired 10 s ago", sign(Algorithm::HS256, &expired)),
            ("HS384", sign(Algorithm::HS384, &current)),
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
        let token = token(&keys, "ADMIN001", "A");
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
        for (user_id, code) in [
            ("USER0001", "U"),
            ("user0001", "X"),
            ("ADMIN001", "a"),
            ("ADMIN001", ""),
            ("ADMIN001", "A "),
            ("ADMIN001", "AA"),
        ] {
            let map = authorization(&format!("Bearer {}", token(&keys, user_id, code)));
            let user = authenticate(&map, &keys).expect("a valid token authenticates");
            assert_eq!(user.user_id, user_id, "the sub claim verbatim for {code:?}");
            assert_eq!(user.usr_type, UserType::User, "{code:?}");
            assert!(!user.is_admin(), "{code:?}");
            assert_forbidden(require_admin(user), code);
        }
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

        let map = authorization(&format!("Bearer {}", token(&keys, "USER0001", "U")));
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
