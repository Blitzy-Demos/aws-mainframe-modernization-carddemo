//! The bearer token of `POST /auth/login`, proved through the HTTP layer:
//! what sign-on issues, which tokens the API refuses, and that nothing but
//! the verified token names the actor of a request.
//!
//! COSGN00C hands the signed-on user to the next program in the COMMAREA
//! fields `CDEMO-USER-ID` and `CDEMO-USER-TYPE`, whose 88 levels are
//! `CDEMO-USRTYP-ADMIN` (`A`) and `CDEMO-USRTYP-USER` (`U`)
//! [app/cpy/COCOM01Y.cpy:L25-28], once `READ-USER-SEC-FILE` has matched the
//! password [app/cbl/COSGN00C.cbl:L209-240]. The API carries the two values in
//! the `sub` and `usr_type` claims of an `HS256` token (AAP 0.6.1):
//!
//! - The sign-on answer holds exactly the token, its RFC 3339 expiry
//!   (`CARDDEMO_JWT_TTL_SECS` after issue), the user ID, the user type and the
//!   landing route, and the token's claims repeat the user ID and type. The
//!   user ID, stored password and type expected of the seeded regular user
//!   are those its reviewed parity row records, read through
//!   `carddemo_audit`.
//! - A request without a valid token is 401 `Authentication required`: no
//!   `Authorization` header, another scheme, a value that is no JWT, a
//!   tampered signature, an algorithm other than `HS256`, another secret, or
//!   an expiry already passed. The scheme matches in any ASCII case.
//! - The COBOL keeps a `U` user out of admin functions only by menu routing
//!   [app/cbl/COMEN01C.cbl:L136-143]. Here a `U` token is 403
//!   `Admin access required` on `/admin/**` whatever the query string, the
//!   other headers or the body claim, and its add stores nothing. Without a
//!   token `/admin/**` is 401, not 403, and the `U` token still reaches the
//!   user functions.
//!
//! The 401 of a token whose `sub` names no stored user, and the
//! `WWW-Authenticate` challenge of every 401, are proved in `auth_subject.rs`.
//!
//! Docker must be running: the test starts one `postgres:16.15` container.

pub mod common;

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use carddemo_api::auth::{Claims, JwtKeys, UserType};
use carddemo_domain::messages;
use carddemo_runtime::config::Settings;
use carddemo_testkit::block_on;
use chrono::DateTime;
use common::{Resp, SeededUser, TestApp, assert_error, send};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

/// The user ID typed to sign the seeded regular user on.
const REGULAR_USER: &str = "USER0001";

/// The seeded regular user: the user ID, stored password and type the
/// sign-on answer and the token's claims must carry [app/jcl/DUSRSECJ.jcl:L40].
const REGULAR_USER_ROW: &str = "P-CSUSR01Y-009";

/// The seeded admin, type `A` [app/jcl/DUSRSECJ.jcl:L35].
const ADMIN_USER: &str = "ADMIN001";

/// The password of every seeded user, upper case as sign-on compares it.
const SEED_PASSWORD: &str = "PASSWORD";

/// The sign-on route, the only issuer of bearer tokens.
const LOGIN_PATH: &str = "/auth/login";

/// Account 1 of the fixture: a user function both user types reach.
const ACCOUNT_PATH: &str = "/accounts/00000000001";

/// The CU00 list and CU01 add route, behind the admin gate.
const ADMIN_USERS: &str = "/admin/users";

/// The `code` of a 401 for a request without a valid bearer token: none, a
/// value that is no `HS256` token signed with the API's secret, or one whose
/// expiry has passed.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The `code` of a 403 for a valid token that is not an admin's, on
/// `/admin/**`.
const FORBIDDEN: &str = "FORBIDDEN";

/// Every key of the sign-on answer, the members of `LoginResponse`.
const LOGIN_KEYS: [&str; 5] = ["expires_at", "landing", "token", "user_id", "usr_type"];

/// A signing secret other than the API's: obviously fake test data.
const OTHER_SECRET: &str = "not-the-api-signing-secret";

/// How long before now the expired token was issued.
const EXPIRED_ISSUED_AGO_SECS: u64 = 7200;

/// How long before now the expired token expired: far beyond the 60 s of
/// leeway `jsonwebtoken` allows by default, so its 401 does not rest on the
/// API's zero leeway alone.
const EXPIRED_AGO_SECS: u64 = 3600;

/// The user ID the refused add of a `U` token tries to store.
const ACTOR_OVERRIDE_ADD: &str = "QAACTR01";

/// Reads the stored row of [`ACTOR_OVERRIDE_ADD`]. The admin control add at
/// the end of the actor checks finds the row through it, which proves the
/// statement names that ID.
const STORED_ADD_SQL: &str = "SELECT user_id FROM users WHERE user_id = 'QAACTR01'";

/// Every case runs on one seeded database, so the file starts a single
/// container.
#[test]
fn bearer_tokens_are_issued_verified_and_name_the_only_actor() {
    let regular = common::seeded_user(&common::parity_audit(), REGULAR_USER_ROW);
    let app = TestApp::start();
    let settings = Settings::default();
    let keys = JwtKeys::new(&settings.jwt_secret, settings.jwt_ttl_secs);
    block_on(async {
        let user = login_answer_carries_the_token(&app, &keys, &regular).await;
        invalid_tokens_are_401(&app, &keys, &settings.jwt_secret, &user).await;
        the_token_alone_names_the_actor(&app, &user).await;
    });
}

/// Signs `USER0001` on and checks the answer: exactly the [`LOGIN_KEYS`], an
/// expiry [`JwtKeys::ttl_secs`] after the moment of issue, and a compact
/// three-segment token whose claims, verified with the API's own keys, repeat
/// the answer's user ID, type and expiry. The typed credentials are the
/// `seeded` user's ID and stored password, and the answer and the claims
/// carry its user ID and type. Returns the token.
async fn login_answer_carries_the_token(
    app: &TestApp,
    keys: &JwtKeys,
    seeded: &SeededUser,
) -> String {
    assert_eq!(
        REGULAR_USER, seeded.user_id,
        "{REGULAR_USER_ROW}: the typed user ID is the seeded one"
    );
    assert_eq!(
        SEED_PASSWORD, seeded.password,
        "{REGULAR_USER_ROW}: the typed password is the stored one"
    );
    let ttl = keys.ttl_secs();
    let body = json!({ "user_id": REGULAR_USER, "password": SEED_PASSWORD });
    let before = now_secs();
    let (status, answer, raw) = app.post(LOGIN_PATH, None, &body).await;
    let after = now_secs();
    assert_eq!(status, StatusCode::OK, "sign-on of {REGULAR_USER}: {raw}");

    let keys_seen: BTreeSet<&str> = answer
        .as_object()
        .unwrap_or_else(|| panic!("the sign-on answer is not a JSON object: {raw}"))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys_seen,
        BTreeSet::from(LOGIN_KEYS),
        "the keys of the sign-on answer: {raw}"
    );

    let token = string_at(&answer, "token", &raw);
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(segments.len(), 3, "the token is `header.payload.signature`");
    assert!(
        segments.iter().all(|segment| !segment.is_empty()),
        "every segment of the token holds text"
    );

    let expires_at = DateTime::parse_from_rfc3339(string_at(&answer, "expires_at", &raw))
        .unwrap_or_else(|error| panic!("`expires_at` is not RFC 3339 ({error}): {raw}"));
    let exp = u64::try_from(expires_at.timestamp())
        .unwrap_or_else(|error| panic!("`expires_at` lies before the Unix epoch ({error}): {raw}"));
    let earliest = before
        .checked_add(ttl)
        .expect("the earliest expiry fits u64");
    let latest = after.checked_add(ttl).expect("the latest expiry fits u64");
    assert!(
        (earliest..=latest).contains(&exp),
        "`expires_at` is {ttl} s after the sign-on, between {earliest} and {latest}: {raw}"
    );

    let claims = keys
        .verify(token)
        .unwrap_or_else(|error| panic!("the API's keys reject the issued token: {error}"));
    assert_eq!(
        claims.sub, seeded.user_id,
        "the `sub` claim is the user ID of {REGULAR_USER_ROW}"
    );
    assert_eq!(
        string_at(&answer, "user_id", &raw),
        seeded.user_id,
        "the answer's user ID is that of {REGULAR_USER_ROW}: {raw}"
    );
    assert_eq!(
        claims.usr_type.as_code(),
        seeded.usr_type,
        "the `usr_type` claim is the type of {REGULAR_USER_ROW}"
    );
    assert_eq!(
        string_at(&answer, "usr_type", &raw),
        seeded.usr_type,
        "the answer's user type is that of {REGULAR_USER_ROW}: {raw}"
    );
    assert_eq!(claims.exp, exp, "the `exp` claim is `expires_at`");
    assert_eq!(claims.exp.checked_sub(claims.iat), Some(ttl), "{claims:?}");
    token.to_owned()
}

/// Every request without a valid token is 401 [`UNAUTHENTICATED`]
/// `Authentication required` on a user function. Each forged token is first
/// shown to fail the API's own keys for the defect it stands for, so its 401
/// is that defect's. The login token `user`, sent with the scheme in either
/// case, is the 200 control.
async fn invalid_tokens_are_401(app: &TestApp, keys: &JwtKeys, secret: &str, user: &str) {
    let unauthorized = StatusCode::UNAUTHORIZED;
    let required = messages::AUTHENTICATION_REQUIRED;

    let missing = app.get(ACCOUNT_PATH, None).await;
    assert_refused(
        &missing,
        unauthorized,
        UNAUTHENTICATED,
        required,
        "no Authorization header",
    );

    for (case, authorization) in [
        ("a non-Bearer scheme", "Basic dXNlcjpwYXNz"),
        ("a value that is no JWT", "Bearer not-a-jwt"),
    ] {
        let resp = get_with_headers(
            app,
            ACCOUNT_PATH,
            &[(AUTHORIZATION.as_str(), authorization)],
        )
        .await;
        assert_refused(&resp, unauthorized, UNAUTHENTICATED, required, case);
    }

    for (case, forged, defect) in forged_tokens(secret, keys.ttl_secs(), user) {
        let rejection = keys
            .verify(&forged)
            .expect_err("the API's keys reject the forged token");
        assert_eq!(rejection.kind(), &defect, "{case}: the defect of the token");
        let resp = app.get(ACCOUNT_PATH, Some(&forged)).await;
        assert_refused(&resp, unauthorized, UNAUTHENTICATED, required, case);
    }

    let (status, _, raw) = app.get(ACCOUNT_PATH, Some(user)).await;
    assert_eq!(status, StatusCode::OK, "the login token: {raw}");
    let lower = format!("bearer {user}");
    let (status, _, raw) =
        get_with_headers(app, ACCOUNT_PATH, &[(AUTHORIZATION.as_str(), &lower)]).await;
    assert_eq!(status, StatusCode::OK, "the scheme in lower case: {raw}");
}

/// The forged tokens of `USER0001`, each with the [`ErrorKind`] the API's
/// keys reject it for: the login token `user` with its signature tampered,
/// valid claims signed `HS384` with the API's `secret`, a token the API's
/// issuer signs with [`OTHER_SECRET`], and claims whose `exp` passed
/// [`EXPIRED_AGO_SECS`] ago, signed `HS256` with `secret`.
fn forged_tokens(secret: &str, ttl: u64, user: &str) -> [(&'static str, String, ErrorKind); 4] {
    let key = EncodingKey::from_secret(secret.as_bytes());
    let now = now_secs();
    let current = Claims {
        sub: REGULAR_USER.to_owned(),
        usr_type: UserType::User,
        iat: now,
        exp: now.checked_add(ttl).expect("the expiry fits u64"),
    };
    let expired = Claims {
        iat: now
            .checked_sub(EXPIRED_ISSUED_AGO_SECS)
            .expect("the clock is past the expired token's issue"),
        exp: now
            .checked_sub(EXPIRED_AGO_SECS)
            .expect("the clock is past the expired token's expiry"),
        ..current.clone()
    };
    let other_secret = JwtKeys::new(OTHER_SECRET, ttl)
        .issue(REGULAR_USER, UserType::User)
        .unwrap_or_else(|error| panic!("issuing with another secret failed: {error}"))
        .token;
    [
        (
            "a tampered signature",
            tamper_signature(user),
            ErrorKind::InvalidSignature,
        ),
        (
            "an HS384 token",
            sign(Algorithm::HS384, &current, &key),
            ErrorKind::InvalidAlgorithm,
        ),
        (
            "a token signed with another secret",
            other_secret,
            ErrorKind::InvalidSignature,
        ),
        (
            "an expired token",
            sign(Algorithm::HS256, &expired, &key),
            ErrorKind::ExpiredSignature,
        ),
    ]
}

/// A `U` token is 403 [`FORBIDDEN`] `Admin access required` on `/admin/**`
/// whatever else the request names as the actor: an admin in the query
/// string, admin role headers, or an admin `sub` and type `A` in the add body,
/// which stores nothing. Role headers do not demote an admin token either.
/// Without a token the admin route is 401 [`UNAUTHENTICATED`], and the `U`
/// token reaches a user function.
async fn the_token_alone_names_the_actor(app: &TestApp, user: &str) {
    let forbidden = StatusCode::FORBIDDEN;
    let admin_only = messages::ADMIN_ACCESS_REQUIRED;

    let query = format!("{ADMIN_USERS}?usr_type=A&user_id={ADMIN_USER}");
    let resp = app.get(&query, Some(user)).await;
    assert_refused(
        &resp,
        forbidden,
        FORBIDDEN,
        admin_only,
        "a U token with an admin query",
    );

    let bearer = format!("Bearer {user}");
    let spoofed = [
        (AUTHORIZATION.as_str(), bearer.as_str()),
        ("X-User-Id", ADMIN_USER),
        ("X-Usr-Type", "A"),
        ("X-Role", "admin"),
    ];
    let resp = get_with_headers(app, ADMIN_USERS, &spoofed).await;
    assert_refused(
        &resp,
        forbidden,
        FORBIDDEN,
        admin_only,
        "a U token with admin headers",
    );

    let add = json!({
        "first_name": "ACTOR",
        "last_name": "OVERRIDE",
        "user_id": ACTOR_OVERRIDE_ADD,
        "password": SEED_PASSWORD,
        "usr_type": "A",
        "sub": ADMIN_USER,
    });
    let resp = app.post(ADMIN_USERS, Some(user), &add).await;
    assert_refused(
        &resp,
        forbidden,
        FORBIDDEN,
        admin_only,
        "a U token adding with admin claims",
    );
    let rows = app.stack().fetch_text_rows(STORED_ADD_SQL).await;
    assert!(rows.is_empty(), "the refused add stored a user: {rows:?}");

    let resp = app.get(ADMIN_USERS, None).await;
    assert_refused(
        &resp,
        StatusCode::UNAUTHORIZED,
        UNAUTHENTICATED,
        messages::AUTHENTICATION_REQUIRED,
        "no token on /admin/users",
    );

    let (status, _, raw) = app.get(ACCOUNT_PATH, Some(user)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a U token on a user function: {raw}"
    );

    let admin = app.admin_token().await;
    let admin_bearer = format!("Bearer {admin}");
    let demoting = [
        (AUTHORIZATION.as_str(), admin_bearer.as_str()),
        ("X-Usr-Type", "U"),
        ("X-Role", "user"),
    ];
    let (status, _, raw) = get_with_headers(app, ADMIN_USERS, &demoting).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an A token with user headers: {raw}"
    );

    // The same body under the admin token is a valid add that the read finds,
    // so the empty read above is the refusal's doing.
    let (status, _, raw) = app.post(ADMIN_USERS, Some(&admin), &add).await;
    assert_eq!(status, StatusCode::CREATED, "the admin add: {raw}");
    let rows = app.stack().fetch_text_rows(STORED_ADD_SQL).await;
    assert_eq!(rows, vec![vec![Some(ACTOR_OVERRIDE_ADD.to_owned())]]);
}

/// Asserts that `resp`, the answer to the request `case` describes, is the
/// refusal `status` with the wire `code`, the catalogue text `message` and no
/// fields.
fn assert_refused(resp: &Resp, status: StatusCode, code: &str, message: &str, case: &str) {
    let (actual_status, body, raw) = resp;
    assert_eq!(
        (*actual_status, body["code"].as_str()),
        (status, Some(code)),
        "{case}: the status and `code` of the refusal: {raw}"
    );
    assert_error(resp, status, code, message, &[]);
}

/// Sends `GET path` with exactly the `(name, value)` pairs of `headers` and
/// no bearer token of its own, through the bounded client and the
/// diagnostics of the harness.
async fn get_with_headers(app: &TestApp, path: &str, headers: &[(&str, &str)]) -> Resp {
    let builder = headers.iter().fold(
        app.client().get(app.url(path)),
        |builder, &(name, value)| builder.header(name, value),
    );
    send(builder, None).await
}

/// `token` with the first character of its signature segment replaced.
///
/// The first character carries six bits of the signature, so the segment
/// still decodes as base64url but no longer matches the signed part.
fn tamper_signature(token: &str) -> String {
    let (signed, signature) = token
        .rsplit_once('.')
        .expect("a compact `header.payload.signature` token");
    let mut chars = signature.chars();
    let first = chars.next().expect("a non-empty signature segment");
    let replacement = if first == 'A' { 'B' } else { 'A' };
    format!("{signed}.{replacement}{}", chars.as_str())
}

/// `claims` signed under `algorithm` with `key`.
fn sign(algorithm: Algorithm, claims: &Claims, key: &EncodingKey) -> String {
    jsonwebtoken::encode(&Header::new(algorithm), claims, key)
        .unwrap_or_else(|error| panic!("signing the test claims failed: {error}"))
}

/// The string at `key` of the JSON `answer`, whose raw text is `raw`.
fn string_at<'a>(answer: &'a Value, key: &str, raw: &str) -> &'a str {
    answer[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` of the answer is not a string: {raw}"))
}

/// System time in whole seconds since the Unix epoch: the clock the API
/// stamps `iat` with and checks `exp` against.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is after the Unix epoch")
        .as_secs()
}
