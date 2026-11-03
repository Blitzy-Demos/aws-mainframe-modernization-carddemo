//! The 401 path of the bearer token, proved through the HTTP layer: a
//! verified token whose `sub` names no stored user, and the challenge every
//! 401 carries.
//!
//! `POST /auth/login` issues a token only after COSGN00C `READ-USER-SEC-FILE`
//! has read the user [app/cbl/COSGN00C.cbl:L209-219], but the token verifies
//! until it expires. The extractors therefore repeat that read: a token
//! signed with the key for a user `USRSEC` never held, and the token of an
//! admin deleted since sign-on, are both 401 `Authentication required` on a
//! user route and on `/admin/**` alike, and the forged add writes nothing.
//! The seeded users keep the admin gate of their token's `usr_type`:
//! `ADMIN001` passes and `USER0001` is 403.
//!
//! Every 401 carries `WWW-Authenticate: Bearer` (RFC 9110 §15.5.2): the
//! unknown subject, a request without a token on the CT00 and CR00 routes,
//! and a failed sign-on. The 2xx and 403 answers carry no challenge.
//!
//! Docker must be running: the test starts one `postgres:16.15` container.

pub mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use carddemo_domain::messages;
use carddemo_runtime::config::Settings;
use carddemo_testkit::block_on;
use common::{Resp, TestApp, assert_error};
use jsonwebtoken::{EncodingKey, Header};
use reqwest::header::WWW_AUTHENTICATE;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// The one challenge every 401 carries.
const BEARER: &str = "Bearer";

/// The seeded admin, `ADMIN001`, type `A` [app/jcl/DUSRSECJ.jcl:L35].
const SEEDED_ADMIN: &str = "ADMIN001";

/// A user ID no seeded `USRSEC` row holds.
const UNKNOWN_USER: &str = "NOBODY99";

/// The admin the forged token tries to add.
const FORGED_ADD: &str = "QAFORGE1";

/// The admin added, signed on and deleted again.
const DELETED_ADMIN: &str = "GHOSTA01";

/// The password of the added admins, upper case as sign-on compares it.
const NEW_PASSWORD: &str = "PASSWORD";

/// Account 1 of the fixture, a user route.
const ACCOUNT_PATH: &str = "/accounts/00000000001";

/// The CU00 list and CU01 add route.
const ADMIN_USERS: &str = "/admin/users";

/// The lifetime of a forged token, as the default `CARDDEMO_JWT_TTL_SECS`.
const FORGED_TTL_SECS: u64 = 3600;

/// One answer: the status, JSON and raw body as the harness reads them, and
/// every `WWW-Authenticate` value.
struct Answer {
    resp: Resp,
    challenges: Vec<String>,
}

/// Sends `method path`, with `token` as the bearer token and `body` as the
/// JSON body when given, through the bounded client of the harness.
async fn call(
    app: &TestApp,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Answer {
    let mut builder = app.client().request(method, app.url(path));
    if let Some(token) = token {
        builder = builder.bearer_auth(token);
    }
    if let Some(body) = body {
        builder = builder.json(body);
    }
    let response = builder
        .send()
        .await
        .unwrap_or_else(|error| panic!("the request to {path} failed: {error}"));
    let status = response.status();
    let challenges = response
        .headers()
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .unwrap_or_else(|error| panic!("{path}: a non-text challenge: {error}"))
                .to_owned()
        })
        .collect();
    let raw = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("reading the answer of {path} failed: {error}"));
    let json = serde_json::from_str(&raw).unwrap_or(Value::Null);
    Answer {
        resp: (status, json, raw),
        challenges,
    }
}

/// A token for `sub` and `usr_type`, signed HS256 with the default
/// `CARDDEMO_JWT_SECRET` the test app runs with: what a holder of the key
/// signs without `POST /auth/login`.
fn forged_token(sub: &str, usr_type: &str) -> String {
    let iat = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is after the epoch")
        .as_secs();
    let exp = iat
        .checked_add(FORGED_TTL_SECS)
        .expect("the expiry fits in u64");
    let claims = json!({"sub": sub, "usr_type": usr_type, "iat": iat, "exp": exp});
    let key = EncodingKey::from_secret(Settings::default().jwt_secret.as_bytes());
    jsonwebtoken::encode(&Header::default(), &claims, &key).expect("the claims sign")
}

/// The CU01 body of a new admin `user_id`.
fn new_admin(user_id: &str) -> Value {
    json!({
        "first_name": "GHOST",
        "last_name": "ADMIN",
        "user_id": user_id,
        "password": NEW_PASSWORD,
        "usr_type": "A",
    })
}

/// Asserts that `answer` is 401 `UNAUTHENTICATED` `Authentication required`
/// with the one `Bearer` challenge.
fn assert_unauthenticated(answer: &Answer, case: &str) {
    let (status, body, raw) = &answer.resp;
    assert_eq!(*status, StatusCode::UNAUTHORIZED, "{case}: {raw}");
    assert_eq!(body["code"], "UNAUTHENTICATED", "{case}: {raw}");
    assert_error(
        &answer.resp,
        StatusCode::UNAUTHORIZED,
        messages::AUTHENTICATION_REQUIRED,
        &[],
    );
    assert_eq!(answer.challenges, [BEARER], "{case}: the 401 challenge");
}

/// Asserts that `answer` has `status` and no `WWW-Authenticate` header.
fn assert_unchallenged(answer: &Answer, status: StatusCode, case: &str) {
    assert_eq!(answer.resp.0, status, "{case}: {}", answer.resp.2);
    assert!(
        answer.challenges.is_empty(),
        "{case}: no challenge expected, got {:?}",
        answer.challenges
    );
}

/// Every case runs on one seeded database, so the file starts a single
/// container.
#[test]
fn a_token_whose_user_is_not_stored_is_401_with_the_bearer_challenge() {
    let app = TestApp::start();
    block_on(async {
        let admin = app.admin_token().await;
        let user = app.user_token().await;
        forged_subject_is_refused(&app, &admin).await;
        deleted_admin_token_is_refused(&app, &admin).await;
        stored_subjects_keep_their_gate(&app, &admin, &user).await;
        every_401_carries_the_challenge(&app).await;
    });
}

/// A forged `A` token for a user `USRSEC` never held is 401 on a user route
/// and on `/admin/**`, and its add stores nothing; the same forgery for the
/// seeded admin passes, so only the subject decides.
async fn forged_subject_is_refused(app: &TestApp, admin: &str) {
    let forged = forged_token(UNKNOWN_USER, "A");
    let add = new_admin(FORGED_ADD);
    let cases = [
        ("forged GET account", Method::GET, ACCOUNT_PATH, None),
        ("forged GET users", Method::GET, ADMIN_USERS, None),
        ("forged POST user", Method::POST, ADMIN_USERS, Some(&add)),
    ];
    for (case, method, path, body) in cases {
        let answer = call(app, method, path, Some(&forged), body).await;
        assert_unauthenticated(&answer, case);
    }

    // The forged add stored no user: the CU02 fetch of its ID finds none.
    let fetch = format!("{ADMIN_USERS}/{FORGED_ADD}");
    let fetched = call(app, Method::GET, &fetch, Some(admin), None).await;
    assert_error(
        &fetched.resp,
        StatusCode::NOT_FOUND,
        messages::USER_ID_NOT_FOUND,
        &["USRIDIN"],
    );

    let control = forged_token(SEEDED_ADMIN, "A");
    let admitted = call(app, Method::GET, ADMIN_USERS, Some(&control), None).await;
    assert_unchallenged(&admitted, StatusCode::OK, "forged token of ADMIN001");
}

/// The token of an admin deleted after sign-on worked before the delete and
/// is 401 after it, on `/admin/**` and on a user route.
async fn deleted_admin_token_is_refused(app: &TestApp, admin: &str) {
    let body = new_admin(DELETED_ADMIN);
    let created = call(app, Method::POST, ADMIN_USERS, Some(admin), Some(&body)).await;
    assert_unchallenged(&created, StatusCode::CREATED, "the add");
    let token = app.login(DELETED_ADMIN, NEW_PASSWORD).await;
    let before = call(app, Method::GET, ADMIN_USERS, Some(&token), None).await;
    assert_unchallenged(&before, StatusCode::OK, "the new admin before the delete");

    let path = format!("{ADMIN_USERS}/{DELETED_ADMIN}");
    let deleted = call(app, Method::DELETE, &path, Some(admin), None).await;
    assert_unchallenged(&deleted, StatusCode::OK, "the delete");

    for (case, path) in [
        ("deleted admin GET users", ADMIN_USERS),
        ("deleted admin GET account", ACCOUNT_PATH),
    ] {
        let answer = call(app, Method::GET, path, Some(&token), None).await;
        assert_unauthenticated(&answer, case);
    }
}

/// The seeded users still pass the subject check, and their tokens keep the
/// admin gate of their `usr_type`.
async fn stored_subjects_keep_their_gate(app: &TestApp, admin: &str, user: &str) {
    let admitted = call(app, Method::GET, ADMIN_USERS, Some(admin), None).await;
    assert_unchallenged(&admitted, StatusCode::OK, "ADMIN001 on GET users");

    let account = call(app, Method::GET, ACCOUNT_PATH, Some(user), None).await;
    assert_unchallenged(&account, StatusCode::OK, "USER0001 on GET account");

    let refused = call(app, Method::GET, ADMIN_USERS, Some(user), None).await;
    assert_unchallenged(&refused, StatusCode::FORBIDDEN, "USER0001 on GET users");
    assert_error(
        &refused.resp,
        StatusCode::FORBIDDEN,
        messages::ADMIN_ACCESS_REQUIRED,
        &[],
    );
}

/// The challenge on the other 401s: no token on the CT00, CT01, CT02 and
/// CR00 routes, and a failed sign-on.
async fn every_401_carries_the_challenge(app: &TestApp) {
    let empty = json!({});
    let cases = [
        (Method::GET, "/transactions", None),
        (Method::GET, "/transactions/0000000000683580", None),
        (Method::POST, "/transactions", Some(&empty)),
        (
            Method::GET,
            "/reports/transactions?report_type=monthly&confirm=Y",
            None,
        ),
    ];
    for (method, path, body) in cases {
        let case = format!("{method} {path} without a token");
        let answer = call(app, method, path, None, body).await;
        assert_unauthenticated(&answer, &case);
    }

    let wrong = json!({"user_id": SEEDED_ADMIN, "password": "WRONGPWD"});
    let signon = call(app, Method::POST, "/auth/login", None, Some(&wrong)).await;
    assert_error(
        &signon.resp,
        StatusCode::UNAUTHORIZED,
        messages::WRONG_PASSWORD_TRY_AGAIN,
        &["PASSWD"],
    );
    assert_eq!(signon.challenges, [BEARER], "the sign-on 401 challenge");
}
