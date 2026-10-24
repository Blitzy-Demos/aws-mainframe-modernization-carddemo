//! The identity extractors: the acting user of a request, from its verified token only, and the
//! admin gate (AAP 0.3.4.6, 0.5.1).
//!
//! In `CardDemo` every online program receives the signed-on identity in its COMMAREA:
//! `CDEMO-USER-ID PIC X(08)` and `CDEMO-USER-TYPE PIC X(01)`, with the 88-levels
//! `CDEMO-USRTYP-ADMIN VALUE 'A'` and `CDEMO-USRTYP-USER VALUE 'U'` (COCOM01Y-005,
//! COCOM01Y-006). COSGN00C sets it after the password check (COSGN00C-014), and no program takes
//! it from screen input. A menu started without a COMMAREA (`EIBCALEN = 0`) has no single
//! result: `MAIN-PARA` moves `'COSGN00C'` into `CDEMO-FROM-PROGRAM`, while
//! `RETURN-TO-SIGNON-SCREEN` tests and transfers to `CDEMO-TO-PROGRAM`, which nothing on that
//! path sets. The target is COSGN00C only when that field's initial storage is low-values or
//! spaces, and otherwise whatever program its bytes name (`COMEN01C.cbl:82-84,196-203`,
//! `COADM01C.cbl:86-88,163-170`). That transfer is halted, COMEN01C-003 as
//! HALTED(X-COMEN01C-2) and COADM01C-002 as HALTED(X-COADM01C-H1), and this module reproduces
//! none of it.
//!
//! The replacement:
//!
//! - [`SignedOnUser`] is the COMMAREA identity of a request. It is rebuilt from the claims
//!   [`JwtKeys::verify`] accepts and from nothing else: never from a body, a query string, a
//!   cookie, a path segment or any header other than one `Authorization: Bearer` header. A request
//!   without a valid token is refused with 401 and [`SIGNON_REQUIRED`] (COMEN01C-002), whose
//!   response carries the challenge `WWW-Authenticate: Bearer`, as every 401 does
//!   (`crate::error`).
//! - [`AdminUser`] is the admin gate: a valid token whose user type is not `A` is refused with 403
//!   and [`ADMIN_REQUIRED`], before any service runs (COADM01C-001). The frontend's route guard
//!   only hides the admin screens; this extractor is the control.
//!
//! Handlers receive identity only as one of these two arguments and never read a header or verify
//! a token themselves (Thin Request Handlers).
//!
//! # Session lifetime
//!
//! The extractors read no store. A token stays valid until its `exp`, as the COMMAREA did for a
//! session, even when the user is changed or deleted meanwhile: COSGN00C reads `USRSEC` once, at
//! sign-on, and no program reads it again for the identity it passes on.
//!
//! # Redaction and totality
//!
//! A rejection carries only one of the two fixed texts above: never the token, a header value, a
//! claim or the reason verification failed, which [`JwtError`](crate::auth::jwt::JwtError)
//! states and this module drops. Nothing here logs or traces (Cardholder Data Redaction). The
//! header is untrusted input, so it is parsed without indexing, unwrapping or panicking (Total
//! Parsers Over Untrusted Input). The keys reach the extractors only through the application
//! state, by [`FromRef`], never from the environment (Local-Only Runtime).

use axum::extract::{FromRef, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use domain::copybook::PicX;
use domain::online::cosgn00c::UserType;

use crate::auth::jwt::{Claims, JwtKeys};
use crate::error::ScreenError;

/// The message of the 401 every endpoint except sign-on answers when the request carries no
/// valid token: no `Authorization` header, a malformed one, a bad signature, another algorithm,
/// an expired token, a token that is not yet valid (its `nbf` in the future) or claims that hold
/// no user ID and user type.
///
/// A fixed text of this crate, not a COBOL literal. The 401 and this text are this replacement's
/// identity gate (AAP 0.5.1, n5; COMEN01C-002), not a reproduction of a program started without
/// a COMMAREA, whose transfer target is halted (X-COMEN01C-2, X-COADM01C-H1). It names no
/// token, header or claim.
pub const SIGNON_REQUIRED: &str = "Sign-on required. Please sign on ...";

/// The message of the 403 every admin endpoint answers to a valid token whose user type is not
/// `A`.
///
/// A fixed text of this crate, not a COBOL literal: COADM01C tests no user type and is reached
/// only from sign-on's admin branch (COADM01C-001). It names no token, header or claim.
pub const ADMIN_REQUIRED: &str = "No access - Admin Only function ...";

/// The authentication scheme of the `Authorization` header, compared without regard to ASCII
/// case, as HTTP scheme names are.
const BEARER: &str = "Bearer";

/// The signed-on user a request acts for: the COMMAREA identity `CDEMO-USER-ID` and
/// `CDEMO-USER-TYPE` (COCOM01Y-005, COCOM01Y-006), from the verified token only.
///
/// As an extractor it guards every endpoint except 1, `POST /api/v1/signon`: 2
/// `GET /menus/main`, 4 to 15 (accounts, cards, transactions, the report request and bill
/// payment) and 21 to 23 (pending authorizations). Every failure is a 401 with
/// [`SIGNON_REQUIRED`].
///
/// `api::services::cocrdslc` decides endpoint 8's card-number disclosure from
/// [`SignedOnUser::user_type`] alone, so no request field can ask for it.
///
/// A user ID and a user type are no protected values, so the derived `Debug` shows both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedOnUser {
    /// `CDEMO-USER-ID`: the user ID sign-on read, space-padded to 8 bytes, as the `sub` claim
    /// carries it without its pad spaces.
    pub user_id: PicX<8>,
    /// `CDEMO-USER-TYPE`: the stored `SEC-USR-TYPE` byte, unchanged, as the `usr_type` claim
    /// carries it. A type that is neither `A` nor `U` is kept and counts as no administrator, as
    /// COSGN00C sends every type other than `A` to the main menu (COSGN00C-016).
    pub user_type: UserType,
}

impl SignedOnUser {
    /// `CDEMO-USRTYP-ADMIN`: the user type is `A`, the only type the admin gate passes
    /// (COSGN00C-015).
    #[must_use]
    pub const fn is_admin(&self) -> bool {
        self.user_type.is_admin()
    }
}

/// A signed-on user whose user type is `A`: the admin gate.
///
/// As an extractor it guards every `[A]` endpoint: 3 `GET /menus/admin`, 16 to 20 (`/users`
/// and `/users/{user_id}`) and 24 to 27 (`/transaction-types`). A request without a valid token
/// is refused with 401 and [`SIGNON_REQUIRED`], exactly as [`SignedOnUser`] refuses it; a valid
/// token of any other user type is refused with 403 and [`ADMIN_REQUIRED`] (COADM01C-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUser(pub SignedOnUser);

/// The token of the request's one `Authorization: Bearer <token>` header.
///
/// `None` when the request carries no such header or several, when the value holds a byte that is
/// not visible ASCII, when the scheme is not `Bearer` (in any ASCII case), and when the token is
/// empty or holds whitespace.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    let well_formed = scheme.eq_ignore_ascii_case(BEARER)
        && !token.is_empty()
        && !token.contains(char::is_whitespace);
    well_formed.then_some(token)
}

/// The identity verified `claims` carry, rebuilt as the COMMAREA fields.
///
/// `None` unless `sub` is a user ID of 1 to 8 printable ASCII bytes that is not all spaces and
/// `usr_type` is exactly one printable ASCII byte. Sign-on issues no other shape
/// (COSGN00C-014): it refuses a blank user ID and reads 8-byte fields only.
fn from_claims(claims: &Claims) -> Option<SignedOnUser> {
    let user_id = PicX::<8>::from_text(&claims.sub).ok()?;
    if user_id.is_spaces() || claims.usr_type.len() != 1 {
        return None;
    }
    let code = PicX::<1>::from_text(&claims.usr_type).ok()?;
    Some(SignedOnUser {
        user_id,
        user_type: UserType::from_code(code),
    })
}

/// The 401 of a request without a valid token; its response carries `WWW-Authenticate: Bearer`.
fn signon_required() -> ScreenError {
    ScreenError::unauthorized(SIGNON_REQUIRED, None)
}

/// The identity of the request's bearer token, verified with the state's [`JwtKeys`].
///
/// Every failure, whatever its cause, is the same 401 [`ScreenError`] with [`SIGNON_REQUIRED`]
/// and no field, so the response reveals nothing about the token it refuses.
impl<S> FromRequestParts<S> for SignedOnUser
where
    JwtKeys: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let keys = JwtKeys::from_ref(state);
        bearer_token(&parts.headers)
            .and_then(|token| keys.verify(token).ok())
            .as_ref()
            .and_then(from_claims)
            .ok_or_else(signon_required)
    }
}

/// The signed-on user, when the user type is `A`.
///
/// The token is verified first, so a request without a valid token is a 401, never a 403; a
/// valid token of any other user type is a 403 [`ScreenError`] with [`ADMIN_REQUIRED`].
impl<S> FromRequestParts<S> for AdminUser
where
    JwtKeys: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = SignedOnUser::from_request_parts(parts, state).await?;
        if user.is_admin() {
            Ok(Self(user))
        } else {
            Err(ScreenError::forbidden(ADMIN_REQUIRED))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::{FromRef, FromRequestParts};
    use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
    use axum::http::request::Parts;
    use axum::http::{HeaderValue, Request, StatusCode};
    use axum::routing::get;
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{ADMIN_REQUIRED, AdminUser, SIGNON_REQUIRED, SignedOnUser};
    use crate::auth::jwt::{Claims, JwtKeys};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A test-only signing secret.
    const SECRET: &str = "extract-unit-test-key";

    /// The default `CARDDEMO_JWT_TTL_SECONDS`.
    const TTL: u64 = 3_600;

    /// Header `{"alg":"none","typ":"JWT"}`, payload
    /// `{"sub":"ADMIN001","usr_type":"A","iat":1700000000,"exp":4102444800}`, empty signature.
    const ALG_NONE: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJBRE1JTjAwMSIsInVzcl90eXBlIjoiQSIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjo0MTAyNDQ0ODAwfQ.";

    fn keys() -> JwtKeys {
        JwtKeys::new(SECRET, TTL)
    }

    fn user_id(text: &str) -> PicX<8> {
        PicX::from_text(text).expect("a user ID of at most 8 printable ASCII bytes")
    }

    fn user_type(code: u8) -> UserType {
        UserType::from_code(PicX::from_array([code]))
    }

    fn now() -> u64 {
        jsonwebtoken::get_current_timestamp()
    }

    /// A token from [`keys`] for `id` and `kind`, issued now.
    fn token(id: &str, kind: &UserType) -> String {
        keys().issue(&user_id(id), kind).expect("a signed token")
    }

    /// `sub` and `usr_type` signed with [`SECRET`] under `algorithm`, issued now.
    fn crafted(algorithm: Algorithm, sub: &str, usr_type: &str) -> String {
        let iat = now();
        let claims = Claims {
            sub: sub.to_owned(),
            usr_type: usr_type.to_owned(),
            iat,
            exp: iat.checked_add(TTL).expect("a lifetime within u64"),
        };
        jsonwebtoken::encode(
            &Header::new(algorithm),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("encodable claims")
    }

    /// The request parts of `GET /api/v1/users` with one `Authorization` header per value.
    fn parts(values: &[HeaderValue]) -> Parts {
        values
            .iter()
            .fold(Request::builder().uri("/api/v1/users"), |builder, value| {
                builder.header(AUTHORIZATION, value.clone())
            })
            .body(())
            .expect("a valid request")
            .into_parts()
            .0
    }

    fn bearer(token: &str) -> HeaderValue {
        HeaderValue::from_str(&format!("Bearer {token}")).expect("a visible-ASCII header value")
    }

    async fn signed_on(values: &[HeaderValue]) -> Result<SignedOnUser, ScreenError> {
        SignedOnUser::from_request_parts(&mut parts(values), &keys()).await
    }

    async fn admin(values: &[HeaderValue]) -> Result<AdminUser, ScreenError> {
        AdminUser::from_request_parts(&mut parts(values), &keys()).await
    }

    /// Asserts the 401 of a request without a valid token, and that it quotes nothing sent.
    fn assert_signon_required(rejection: &ScreenError, sent: &str, case: &str) {
        assert_eq!(rejection.status(), StatusCode::UNAUTHORIZED, "{case}");
        assert_eq!(rejection.message(), SIGNON_REQUIRED, "{case}");
        assert_eq!(rejection.field(), None, "{case}");
        if !sent.is_empty() {
            assert!(!rejection.to_string().contains(sent), "{case}");
            assert!(!format!("{rejection:?}").contains(sent), "{case}");
        }
    }

    /// Runs `test` to completion on a current-thread runtime with I/O and time enabled, the
    /// runtime tokio's test attribute builds. It is built here because that attribute's expansion
    /// carries lint-suppression attributes, an `expect` and unchecked pinning, which R15 admits
    /// nowhere; a runtime that cannot be built fails the test with its error, never a panic.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    #[test]
    fn a_valid_token_yields_its_commarea_identity() -> TestResult {
        block_on(async {
            // audit: COSGN00C-014
            let admin_user = signed_on(&[bearer(&token("ADMIN001", &UserType::ADMIN))])
                .await
                .expect("a signed-on admin");
            assert_eq!(admin_user.user_id, PicX::from_array(*b"ADMIN001"));
            assert_eq!(admin_user.user_type, UserType::ADMIN);
            assert!(admin_user.is_admin());

            let user = signed_on(&[bearer(&token("USER0001", &UserType::USER))])
                .await
                .expect("a signed-on user");
            assert_eq!(user.user_id, user_id("USER0001"));
            assert_eq!(user.user_type, UserType::USER);
            assert!(!user.is_admin());

            // A short user ID loses its pad spaces in `sub` and gets them back here.
            let short = signed_on(&[bearer(&token("USER1", &UserType::USER))])
                .await
                .expect("a signed-on user");
            assert_eq!(short.user_id, PicX::from_array(*b"USER1   "));
            Ok(())
        })
    }

    #[test]
    fn the_scheme_matches_in_any_ascii_case() -> TestResult {
        block_on(async {
            let token = token("ADMIN001", &UserType::ADMIN);
            for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
                let value = HeaderValue::from_str(&format!("{scheme} {token}")).expect("a header");
                let user = signed_on(&[value]).await.expect(scheme);
                assert_eq!(user.user_id, user_id("ADMIN001"), "{scheme}");
            }
            Ok(())
        })
    }

    #[test]
    fn the_admin_gate_passes_type_a_only() -> TestResult {
        block_on(async {
            // audit: COADM01C-001
            let passed = admin(&[bearer(&token("ADMIN001", &UserType::ADMIN))])
                .await
                .expect("an admin");
            assert_eq!(
                passed,
                AdminUser(SignedOnUser {
                    user_id: user_id("ADMIN001"),
                    user_type: UserType::ADMIN,
                })
            );

            // `U`, and every other stored type, is signed on but is no administrator.
            for code in [b'U', b' ', b'a', b'X'] {
                let kind = user_type(code);
                let sent = token("USER0001", &kind);
                let user = signed_on(&[bearer(&sent)]).await.expect("a signed-on user");
                assert_eq!(user.user_type, kind);
                assert!(!user.is_admin());

                let rejection = admin(&[bearer(&sent)])
                    .await
                    .expect_err("refused by the gate");
                assert_eq!(rejection.status(), StatusCode::FORBIDDEN);
                assert_eq!(rejection.message(), ADMIN_REQUIRED);
                assert_eq!(rejection.field(), None);
                assert!(!rejection.to_string().contains(&sent));
            }
            Ok(())
        })
    }

    #[test]
    fn every_request_without_a_valid_token_is_401_on_both_extractors() -> TestResult {
        let valid = token("ADMIN001", &UserType::ADMIN);
        let expired_at = now()
            .checked_sub(TTL.checked_add(1).expect("a small offset"))
            .expect("a time after the epoch");
        let expired = keys()
            .issue_at(&user_id("ADMIN001"), &UserType::ADMIN, expired_at)
            .expect("a signed token");
        let wrong_secret = JwtKeys::new("another-secret", TTL)
            .issue(&user_id("ADMIN001"), &UserType::ADMIN)
            .expect("a signed token");
        let hs384 = crafted(Algorithm::HS384, "ADMIN001", "A");
        let mut cases: Vec<(&str, Vec<HeaderValue>, String)> = vec![
            ("no header", vec![], String::new()),
            (
                "basic scheme",
                vec![HeaderValue::from_static("Basic abc")],
                "abc".to_owned(),
            ),
            (
                "basic scheme, valid token",
                vec![HeaderValue::from_str(&format!("Basic {valid}")).expect("a header")],
                valid.clone(),
            ),
            (
                "scheme only",
                vec![HeaderValue::from_static("Bearer")],
                String::new(),
            ),
            (
                "empty token",
                vec![HeaderValue::from_static("Bearer ")],
                String::new(),
            ),
            (
                "token with a space",
                vec![HeaderValue::from_static("Bearer a b")],
                "a b".to_owned(),
            ),
            (
                "two spaces",
                vec![HeaderValue::from_str(&format!("Bearer  {valid}")).expect("a header")],
                valid.clone(),
            ),
            (
                "tab separator",
                vec![HeaderValue::from_str(&format!("Bearer\t{valid}")).expect("a header")],
                valid.clone(),
            ),
            (
                "token only",
                vec![HeaderValue::from_str(&valid).expect("a header")],
                valid.clone(),
            ),
            (
                "two headers",
                vec![bearer(&valid), bearer(&valid)],
                valid.clone(),
            ),
            (
                "not visible ASCII",
                vec![HeaderValue::from_bytes(b"Bearer \xff").expect("an obs-text header value")],
                String::new(),
            ),
            (
                "garbage token",
                vec![bearer("not.a.token")],
                "not.a.token".to_owned(),
            ),
            ("expired", vec![bearer(&expired)], expired.clone()),
            (
                "wrong secret",
                vec![bearer(&wrong_secret)],
                wrong_secret.clone(),
            ),
            ("alg none", vec![bearer(ALG_NONE)], ALG_NONE.to_owned()),
            ("HS384", vec![bearer(&hs384)], hs384.clone()),
        ];
        let claim_cases = [
            ("sub longer than 8 bytes", "TOOLONGUSER", "A"),
            ("usr_type of 2 bytes", "ADMIN001", "AU"),
            ("empty usr_type", "ADMIN001", ""),
            ("empty sub", "", "A"),
            ("blank sub", "        ", "A"),
            ("non-ASCII sub", "ADM\u{cd}N01", "A"),
            ("control byte in sub", "ADMIN\t01", "A"),
            ("non-ASCII usr_type", "ADMIN001", "\u{c1}"),
        ];
        cases.extend(claim_cases.into_iter().map(|(case, sub, usr_type)| {
            let sent = crafted(Algorithm::HS256, sub, usr_type);
            (case, vec![bearer(&sent)], sent)
        }));

        block_on(async {
            for (case, values, sent) in &cases {
                let rejection = signed_on(values).await.expect_err(case);
                assert_signon_required(&rejection, sent, case);
                let rejection = admin(values).await.expect_err(case);
                assert_signon_required(&rejection, sent, case);
            }
            Ok(())
        })
    }

    /// An application state other than `JwtKeys` itself, as `api::app::AppState` is.
    #[derive(Clone)]
    struct GatedState {
        keys: JwtKeys,
    }

    impl FromRef<GatedState> for JwtKeys {
        fn from_ref(state: &GatedState) -> Self {
            state.keys.clone()
        }
    }

    async fn admin_only(AdminUser(user): AdminUser) -> String {
        user.user_id.to_text()
    }

    async fn any_user(user: SignedOnUser) -> String {
        user.user_id.to_text()
    }

    /// The status, every `WWW-Authenticate` value and the body of `GET path` with
    /// `authorization`, through a router whose state is a [`GatedState`].
    async fn call(
        path: &str,
        authorization: Option<HeaderValue>,
    ) -> (StatusCode, Vec<HeaderValue>, Vec<u8>) {
        let router = Router::new()
            .route("/admin", get(admin_only))
            .route("/menu", get(any_user))
            .with_state(GatedState { keys: keys() });
        let mut request = Request::builder().uri(path);
        if let Some(value) = authorization {
            request = request.header(AUTHORIZATION, value);
        }
        let response = router
            .oneshot(request.body(Body::empty()).expect("a valid request"))
            .await
            .expect("an infallible router");
        let status = response.status();
        let challenges = response
            .headers()
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .cloned()
            .collect();
        let body = axum::body::to_bytes(response.into_body(), 4_096)
            .await
            .expect("a small body");
        (status, challenges, body.to_vec())
    }

    fn json_body(body: &[u8]) -> Value {
        serde_json::from_slice(body).expect("a JSON body")
    }

    #[test]
    fn the_rejections_become_screen_error_responses() -> TestResult {
        block_on(async {
            let admin_token = token("ADMIN001", &UserType::ADMIN);
            let user_token = token("USER0001", &UserType::USER);

            // Only a 401 challenges: a served request and the gate's 403 carry no challenge.
            let (status, challenges, body) = call("/admin", Some(bearer(&admin_token))).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, b"ADMIN001");
            assert!(challenges.is_empty(), "{challenges:?}");
            let (status, challenges, body) = call("/menu", Some(bearer(&user_token))).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, b"USER0001");
            assert!(challenges.is_empty(), "{challenges:?}");

            let (status, challenges, body) = call("/admin", Some(bearer(&user_token))).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(json_body(&body), json!({ "message": ADMIN_REQUIRED }));
            assert!(challenges.is_empty(), "{challenges:?}");

            for path in ["/admin", "/menu"] {
                for (label, authorization) in
                    [("no token", None), ("alg none", Some(bearer(ALG_NONE)))]
                {
                    let case = format!("{path} with {label}");
                    let (status, challenges, body) = call(path, authorization).await;
                    assert_eq!(status, StatusCode::UNAUTHORIZED, "{case}");
                    assert_eq!(
                        json_body(&body),
                        json!({ "message": SIGNON_REQUIRED }),
                        "{case}"
                    );
                    assert_eq!(challenges, ["Bearer"], "{case}");
                }
            }
            Ok(())
        })
    }
}
