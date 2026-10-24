//! Integration tests of endpoints 1 to 3: sign-on (`POST /api/v1/signon`, CC00 / COSGN00C), the
//! main menu (`GET /api/v1/menus/main`, CM00 / COMEN01C) and the admin menu
//! (`GET /api/v1/menus/admin`, CA00 / COADM01C). They are binding g1b for every `server` row of
//! `modern/audit/online/COSGN00C.md`, `COMEN01C.md` and `COADM01C.md`, and for the server half of
//! the two `both` rows of the sign-on map in `modern/audit/screens.md` (`COSGN00-027`,
//! `COSGN00-031`). The main and admin maps have no `both` row: their option input is never sent.
//!
//! They also bind the COMMAREA rows COCOM01Y-001 to COCOM01Y-007, COCOM01Y-015 and COCOM01Y-016
//! of `modern/audit/copybooks.md`: `cocom01y_commarea_rows` proves that the user ID and type
//! COSGN00C moves into the COMMAREA travel only as the verified claims `sub` and `usr_type`, that
//! the transfer target is the route `next_route`, and that the from-transaction, from-program,
//! program context, map and mapset travel in no answer, no token and no `OpenAPI` name.
//!
//! Each test drives the whole router in process (`tower::ServiceExt::oneshot`) over its own
//! `postgres:16` testcontainer, which the `test-support` harness brings up holding the schema and,
//! except in the store-failure test, loaded with the fixtures and the ten DUSRSECJ users. Every
//! bearer token comes from endpoint 1; users the fixtures do not hold are added through endpoint
//! 17 with an admin token. Expected texts are copied from the audit rows, trailing spaces
//! included, and each proving assertion carries the rule IDs it proves.
//!
//! No halt and no halt candidate covers a `server` row of the three programs: the candidates
//! `X-COSGN00C-H1`, `X-COMEN01C-1`, `X-COMEN01C-2`, `X-COADM01C-H1` and `X-COADM01C-H2` block
//! `client` rows only, and `COSGN00C-020` embeds no CICS code, so halt H4 does not apply. Nothing
//! here is therefore reported blocked.
//!
//! Cardholder Data Redaction: sign-on responses, successful or not, are checked to hold no
//! `password` key, no submitted password and no argon2id hash. A failing assertion names the user
//! ID or the case, never a password.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::app::{AppState, DOCUMENT_ROUTE, app};
use api::auth::extract::{ADMIN_REQUIRED, SIGNON_REQUIRED};
use api::dto::{FIELD_LENGTH_MESSAGE, INVALID_BODY};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, header};
use chrono::NaiveDate;
use domain::clock::{Clock, FixedClock};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use persistence::PgStore;
use runtime::config::Config;
use serde_json::{Map, Value, json};
use test_support::fixtures::start_loaded_postgres;
use test_support::postgres::{PostgresGuard, start_postgres};
use tower::ServiceExt as _;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// A local-only signing secret for the API under test.
const SECRET: &str = "endpoints-signon-menus-local-secret";

/// A second local-only secret: a token it signs is not one of the API's.
const OTHER_SECRET: &str = "endpoints-signon-menus-other-secret";

/// Endpoint 1.
const SIGNON: &str = "/api/v1/signon";

/// Endpoint 2.
const MAIN_MENU: &str = "/api/v1/menus/main";

/// Endpoint 3.
const ADMIN_MENU: &str = "/api/v1/menus/admin";

/// Endpoint 17, which adds the users the fixtures do not hold.
const USERS: &str = "/api/v1/users";

/// The DUSRSECJ password of every in-stream user (`DUSRSECJ.jcl:35-44`, `README.md:203-204`).
const DUSRSECJ_PASSWORD: &str = "PASSWORD";

/// A password no user is stored with.
const WRONG_PASSWORD: &str = "WRONGPW1";

/// The token lifetime when `CARDDEMO_JWT_TTL_SECONDS` is unset (COSGN00C-014).
const DEFAULT_TTL_SECONDS: u64 = 3600;

/// COSGN00C-007.
const ENTER_USER_ID: &str = "Please enter User ID ...";

/// COSGN00C-008.
const ENTER_PASSWORD: &str = "Please enter Password ...";

/// COSGN00C-018.
const WRONG_PASSWORD_MESSAGE: &str = "Wrong Password. Try again ...";

/// COSGN00C-019.
const USER_NOT_FOUND: &str = "User not found. Try again ...";

/// COSGN00C-020.
const UNABLE_TO_VERIFY: &str = "Unable to verify the User ...";

/// COMEN01C-011 and COADM01C-014: 37 characters.
const INVALID_OPTION: &str = "Please enter a valid option number...";

/// COMEN01C-011: 33 characters, the last a space.
const ADMIN_ONLY: &str = "No access - Admin Only option... ";

/// COMEN01C-015: option 11's not-installed text, 58 characters.
const OPTION_11_NOT_INSTALLED: &str = "This option Pending Authorization View is not installed...";

/// COADM01C-014: the not-installed text of every admin option.
const NOT_INSTALLED: &str = "This option is not installed ...";

/// The main menu of `COMEN02Y` in table order: number, the 35-byte name as declared and the
/// program; every option's user type is `U` (COMEN01C-020). Option `08` is the name of line 70;
/// line 69's admin-only variant is a comment.
const MAIN_OPTIONS: [(&str, &str, &str); 11] = [
    ("01", "Account View                       ", "COACTVWC"),
    ("02", "Account Update                     ", "COACTUPC"),
    ("03", "Credit Card List                   ", "COCRDLIC"),
    ("04", "Credit Card View                   ", "COCRDSLC"),
    ("05", "Credit Card Update                 ", "COCRDUPC"),
    ("06", "Transaction List                   ", "COTRN00C"),
    ("07", "Transaction View                   ", "COTRN01C"),
    ("08", "Transaction Add                    ", "COTRN02C"),
    ("09", "Transaction Reports                ", "CORPT00C"),
    ("10", "Bill Payment                       ", "COBIL00C"),
    ("11", "Pending Authorization View         ", "COPAUS0C"),
];

/// The admin menu of `COADM02Y` in table order: number, the 35-byte name as declared and the
/// program (COADM01C-006 to COADM01C-011).
const ADMIN_OPTIONS: [(&str, &str, &str); 6] = [
    ("01", "User List (Security)               ", "COUSR00C"),
    ("02", "User Add (Security)                ", "COUSR01C"),
    ("03", "User Update (Security)             ", "COUSR02C"),
    ("04", "User Delete (Security)             ", "COUSR03C"),
    ("05", "Transaction Type List/Update (Db2) ", "COTRTLIC"),
    ("06", "Transaction Type Maintenance (Db2) ", "COTRTUPC"),
];

/// The local configuration of the API under test: `secret`, the default token lifetime and a
/// job-deck directory under the integration-test scratch directory.
fn config(secret: &str) -> Config {
    Config {
        jwt_secret: secret.to_owned(),
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints_signon_menus-jobs"),
        ..Config::default()
    }
}

/// The test clock: 2022-07-18 10:30:00.000.
fn clock() -> TestResult<Arc<dyn Clock>> {
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or("an invalid test instant")?;
    Ok(Arc::new(FixedClock::new(at)))
}

/// The API over the database of `guard`, configured by `config`, with the state it serves.
fn app_over(guard: &PostgresGuard, config: &Config) -> TestResult<(Router, AppState)> {
    let state = AppState::from_config(PgStore::new(guard.pool().clone()), config, clock()?)?;
    Ok((app(state.clone()), state))
}

/// The status and the JSON body (`null` when empty) of one request to `app`, with `authorization`
/// as the raw `Authorization` header when one is given.
async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    authorization: Option<&str>,
    body: Option<&Value>,
) -> TestResult<(u16, Value)> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(authorization) = authorization {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }
    let request = match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))?,
        None => builder.body(Body::empty())?,
    };
    let response = app.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, value))
}

/// `Bearer <token>`.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// A `GET` of `uri`, with `token` as bearer token when one is given.
async fn get(app: &Router, uri: &str, token: Option<&str>) -> TestResult<(u16, Value)> {
    let authorization = token.map(bearer);
    call(app, Method::GET, uri, authorization.as_deref(), None).await
}

/// Endpoint 1 with `body` as the request body.
async fn post_signon(app: &Router, body: &Value) -> TestResult<(u16, Value)> {
    call(app, Method::POST, SIGNON, None, Some(body)).await
}

/// Endpoint 1 with the user ID and password as typed.
async fn sign_on(app: &Router, user_id: &str, password: &str) -> TestResult<(u16, Value)> {
    post_signon(app, &json!({"user_id": user_id, "password": password})).await
}

/// The bearer token endpoint 1 issues to `user_id` signing on with `password`.
async fn token(app: &Router, user_id: &str, password: &str) -> TestResult<String> {
    let (status, body) = sign_on(app, user_id, password).await?;
    if status != 200 {
        return Err(format!("the sign-on of {user_id} answered {status}").into());
    }
    body.get("token")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("the sign-on of {user_id} returned no token").into())
}

/// Adds a user through endpoint 17 with `admin_token`; COUSR01C stores every field as typed.
async fn add_user(
    app: &Router,
    admin_token: &str,
    user_id: &str,
    password: &str,
    user_type: &str,
) -> TestResult {
    let body = json!({
        "first_name": "Signon",
        "last_name": "Fixture",
        "user_id": user_id,
        "password": password,
        "user_type": user_type,
    });
    let authorization = bearer(admin_token);
    let (status, _) = call(app, Method::POST, USERS, Some(&authorization), Some(&body)).await?;
    if status == 200 {
        Ok(())
    } else {
        Err(format!("adding {user_id} answered {status}").into())
    }
}

/// The `ScreenError` body `{message, field}`, with `field` left out when `None`.
fn screen_error(message: &str, field: Option<&str>) -> Value {
    match field {
        Some(field) => json!({"message": message, "field": field}),
        None => json!({"message": message}),
    }
}

/// The member names of a JSON object, in sorted order; empty for any other value.
fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// Whether `value` or any value nested in it is an object with a member named `key`.
fn has_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(key) || object.values().any(|nested| has_key(nested, key))
        }
        Value::Array(items) => items.iter().any(|nested| has_key(nested, key)),
        _ => false,
    }
}

/// Cardholder Data Redaction for a sign-on response: no `password` key, not the submitted
/// password, and no argon2id hash.
fn assert_redacted(body: &Value, submitted_password: &str, case: &str) {
    let text = body.to_string();
    assert!(!has_key(body, "password"), "{case}: a password key");
    assert!(
        !text.contains(submitted_password),
        "{case}: the submitted password"
    );
    assert!(!text.contains("$argon2"), "{case}: a password hash");
}

/// The current time in seconds since the Unix epoch, the clock the token's `iat` is read from.
fn unix_now() -> TestResult<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// The names of every claim of `token`, verified with `secret`.
fn claim_names(token: &str, secret: &str) -> TestResult<BTreeSet<String>> {
    let data = jsonwebtoken::decode::<Map<String, Value>>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )?;
    Ok(data.claims.keys().cloned().collect())
}

/// The header and payload of `payload_from` with the signature of `signature_from`: a token whose
/// signature does not match its claims.
fn spliced(payload_from: &str, signature_from: &str) -> TestResult<String> {
    let mut parts = payload_from.split('.');
    let (Some(head), Some(payload)) = (parts.next(), parts.next()) else {
        return Err("a token without a payload".into());
    };
    let signature = signature_from
        .split('.')
        .nth(2)
        .ok_or("a token without a signature")?;
    Ok(format!("{head}.{payload}.{signature}"))
}

/// The 40-byte display text `BUILD-MENU-OPTIONS` strings: the two digits, `. `, the 35-byte name
/// and one trailing space (COMEN01C-021, COADM01C-006 to COADM01C-011).
fn option_text(number: &str, name: &str) -> String {
    format!("{number}. {name} ")
}

/// Whether the operation at `pointer` of the `OpenAPI` document declares a `page_size`
/// parameter. The operation must exist, so the answer is never vacuous.
fn declares_page_size(document: &Value, pointer: &str) -> TestResult<bool> {
    let operation = document
        .pointer(pointer)
        .ok_or_else(|| format!("the document has no operation at {pointer}"))?;
    Ok(operation
        .get("parameters")
        .and_then(Value::as_array)
        .is_some_and(|parameters| {
            parameters
                .iter()
                .any(|parameter| parameter.get("name") == Some(&json!("page_size")))
        }))
}

/// The main menu body without its `user_type`, the one member that differs between callers.
fn without_user_type(mut menu: Value) -> Value {
    if let Some(object) = menu.as_object_mut() {
        object.remove("user_type");
    }
    menu
}

/// Success for the two DUSRSECJ users R8 names (COSGN00C-013 to COSGN00C-016), then the
/// upper-casing of the user ID and the password (COSGN00C-009).
async fn successful_signons(app: &Router, state: &AppState) -> TestResult {
    let claims_named = BTreeSet::from(["exp", "iat", "sub", "usr_type"].map(str::to_owned));
    for (user_id, user_type, next_route) in
        [("ADMIN001", "A", "/admin"), ("USER0001", "U", "/menu")]
    {
        let before = unix_now()?;
        let (status, body) = sign_on(app, user_id, DUSRSECJ_PASSWORD).await?;
        let after = unix_now()?;
        // audit: COSGN00C-013
        assert_eq!(status, 200, "{user_id}");
        // audit: COSGN00C-014
        assert_eq!(
            keys(&body),
            BTreeSet::from(["next_route", "token", "user_id", "user_type"]),
            "{user_id}"
        );
        assert_eq!(body.get("user_id"), Some(&json!(user_id)));
        assert_eq!(body.get("user_type"), Some(&json!(user_type)));
        // audit: COSGN00C-015
        // audit: COSGN00C-016
        assert_eq!(
            body.get("next_route"),
            Some(&json!(next_route)),
            "{user_id}"
        );
        let token = body
            .get("token")
            .and_then(Value::as_str)
            .ok_or("no token")?;
        assert!(!token.is_empty(), "{user_id}: an empty token");
        let claims = state.jwt.verify(token)?;
        // audit: COSGN00C-014
        assert_eq!(
            (claims.sub.as_str(), claims.usr_type.as_str()),
            (user_id, user_type)
        );
        assert!((before..=after).contains(&claims.iat), "{user_id}: iat");
        assert_eq!(
            claims.exp.checked_sub(claims.iat),
            Some(DEFAULT_TTL_SECONDS)
        );
        assert_eq!(claim_names(token, SECRET)?, claims_named, "{user_id}");
        // audit: COSGN00C-013
        assert_redacted(&body, DUSRSECJ_PASSWORD, user_id);
    }
    let typed = [
        ("user0001", "password", "USER0001", "U"),
        ("Admin001", "PassWord", "ADMIN001", "A"),
    ];
    for (typed_id, typed_password, user_id, user_type) in typed {
        let (status, body) = sign_on(app, typed_id, typed_password).await?;
        // audit: COSGN00C-009
        assert_eq!(
            (status, body.get("user_id"), body.get("user_type")),
            (200, Some(&json!(user_id)), Some(&json!(user_type))),
            "{typed_id}"
        );
        assert_redacted(&body, typed_password, typed_id);
    }
    Ok(())
}

/// Each sign-on is decided from its own credentials (COSGN00C-006): failures leave no count or
/// lockout behind, and endpoint 1 reads no token it is sent.
async fn no_state_between_signons(app: &Router) -> TestResult {
    for attempt in 1..=3 {
        let (status, _) = sign_on(app, "USER0001", WRONG_PASSWORD).await?;
        // audit: COSGN00C-006
        assert_eq!(status, 401, "failed attempt {attempt}");
    }
    let admin_token = token(app, "ADMIN001", DUSRSECJ_PASSWORD).await?;
    let body = json!({"user_id": "USER0001", "password": DUSRSECJ_PASSWORD});
    let sent = [bearer(&admin_token), "Bearer not-a-token".to_owned()];
    for (case, authorization) in sent.iter().enumerate() {
        let (status, response) =
            call(app, Method::POST, SIGNON, Some(authorization), Some(&body)).await?;
        // audit: COSGN00C-006
        assert_eq!(
            (status, response.get("user_id"), response.get("user_type")),
            (200, Some(&json!("USER0001")), Some(&json!("U"))),
            "authorization case {case}"
        );
    }
    Ok(())
}

/// The blank-field edits in program order (COSGN00C-007, COSGN00C-008, COSGN00C-010) and the
/// input rejections of the 8-position map fields (COSGN00C-003, COSGN00-027, COSGN00-031).
async fn blank_and_length_edits(app: &Router) -> TestResult {
    let user_id_blank = [
        json!({}),
        json!({"password": DUSRSECJ_PASSWORD}),
        json!({"user_id": "", "password": DUSRSECJ_PASSWORD}),
        json!({"user_id": "        ", "password": DUSRSECJ_PASSWORD}),
        json!({"user_id": "", "password": ""}),
    ];
    for (case, body) in user_id_blank.iter().enumerate() {
        // audit: COSGN00C-007
        // audit: COSGN00C-010
        assert_eq!(
            post_signon(app, body).await?,
            (400, screen_error(ENTER_USER_ID, Some("user_id"))),
            "blank user ID case {case}"
        );
    }
    let password_blank = [
        json!({"user_id": "USER0001"}),
        json!({"user_id": "USER0001", "password": ""}),
        json!({"user_id": "USER0001", "password": "        "}),
        // An unknown user: the blank password is answered before any read.
        json!({"user_id": "NOSUCHUS", "password": ""}),
    ];
    for (case, body) in password_blank.iter().enumerate() {
        // audit: COSGN00C-008
        // audit: COSGN00C-010
        assert_eq!(
            post_signon(app, body).await?,
            (400, screen_error(ENTER_PASSWORD, Some("password"))),
            "blank password case {case}"
        );
    }
    // A trailing space is part of the value and is never trimmed away. The rejection text is the
    // request DTO's own; no COBOL literal applies.
    for (case, user_id) in ["USER00011", "USER0001 "].iter().enumerate() {
        // audit: COSGN00-027
        // audit: COSGN00C-003
        assert_eq!(
            sign_on(app, user_id, DUSRSECJ_PASSWORD).await?,
            (400, screen_error(FIELD_LENGTH_MESSAGE, Some("user_id"))),
            "nine-character user ID case {case}"
        );
    }
    for (case, password) in ["PASSWORD1", "PASSWORD "].iter().enumerate() {
        // audit: COSGN00-031
        // audit: COSGN00C-003
        assert_eq!(
            sign_on(app, "USER0001", password).await?,
            (400, screen_error(FIELD_LENGTH_MESSAGE, Some("password"))),
            "nine-character password case {case}"
        );
    }
    // Eight characters no 3270 field carries: `X-COSGN00C-3` proposes this rejection, which is no
    // COBOL result, so only the status and the field are asserted.
    let unprintable = [
        ("\u{c4}BCDEFGH", DUSRSECJ_PASSWORD, "user_id"),
        ("USER\u{0}001", DUSRSECJ_PASSWORD, "user_id"),
        ("USER0001", "PASSW\u{d6}RD", "password"),
    ];
    for (case, (user_id, password, field)) in unprintable.iter().enumerate() {
        let (status, body) = sign_on(app, user_id, password).await?;
        // audit: COSGN00C-003
        assert_eq!(
            (status, body.get("field")),
            (400, Some(&json!(field))),
            "unprintable case {case}"
        );
    }
    Ok(())
}

/// A body whose top-level value is not a JSON object, an array whose elements would fill the
/// members in order included, is the request layer's 400 with no field: no edit runs, nothing is
/// read and no token is issued.
async fn non_object_bodies(app: &Router) -> TestResult {
    let bodies = [
        json!(["ADMIN001", DUSRSECJ_PASSWORD]),
        json!(["USER0001", DUSRSECJ_PASSWORD]),
        json!([]),
        Value::Null,
        json!("ADMIN001"),
        json!(1),
        json!(true),
    ];
    for (case, body) in bodies.iter().enumerate() {
        let (status, response) = post_signon(app, body).await?;
        // The request layer's text: no COBOL literal and no audit row applies.
        assert_eq!(
            (status, &response),
            (400, &screen_error(INVALID_BODY, None)),
            "non-object body case {case}"
        );
        assert_redacted(&response, DUSRSECJ_PASSWORD, "non-object body");
    }
    Ok(())
}

/// A wrong password and an unknown user (COSGN00C-018, COSGN00C-019).
async fn failed_signons(app: &Router) -> TestResult {
    let (status, body) = sign_on(app, "USER0001", WRONG_PASSWORD).await?;
    // audit: COSGN00C-018
    assert_eq!(
        (status, &body),
        (401, &screen_error(WRONG_PASSWORD_MESSAGE, Some("password")))
    );
    assert_redacted(&body, WRONG_PASSWORD, "wrong password");
    // The short ID is read space-padded, the leading space kept.
    for user_id in ["NOSUCHUS", "USER01", " USER001"] {
        let (status, body) = sign_on(app, user_id, DUSRSECJ_PASSWORD).await?;
        // audit: COSGN00C-019
        assert_eq!(
            (status, &body),
            (401, &screen_error(USER_NOT_FOUND, Some("user_id"))),
            "{user_id}"
        );
        assert_redacted(&body, DUSRSECJ_PASSWORD, user_id);
    }
    Ok(())
}

/// What COUSR01C stores as typed while sign-on upper-cases: a stored lower-case password never
/// verifies (E26) and a stored lower-case user ID is never found (`X-COSGN00C-1`), both
/// COSGN00C-009; and a stored user type other than `A` or `U` (COSGN00C-014, COSGN00C-016).
async fn stored_as_typed(app: &Router, state: &AppState) -> TestResult {
    let admin_token = token(app, "ADMIN001", DUSRSECJ_PASSWORD).await?;
    add_user(app, &admin_token, "LOWERPW1", "lowerpw1", "U").await?;
    add_user(app, &admin_token, "lowerid1", "IDPASSW1", "U").await?;
    add_user(app, &admin_token, "TYPEX001", "TYPEXPW1", "X").await?;
    for (case, password) in ["lowerpw1", "LOWERPW1"].iter().enumerate() {
        // audit: COSGN00C-009
        // audit: COSGN00C-018
        assert_eq!(
            sign_on(app, "LOWERPW1", password).await?,
            (401, screen_error(WRONG_PASSWORD_MESSAGE, Some("password"))),
            "stored lower-case password, attempt {case}"
        );
    }
    for typed_id in ["lowerid1", "LOWERID1"] {
        // audit: COSGN00C-009
        // audit: COSGN00C-019
        assert_eq!(
            sign_on(app, typed_id, "IDPASSW1").await?,
            (401, screen_error(USER_NOT_FOUND, Some("user_id"))),
            "{typed_id}"
        );
    }
    let (status, body) = sign_on(app, "TYPEX001", "TYPEXPW1").await?;
    // audit: COSGN00C-016
    assert_eq!(
        (status, body.get("user_type"), body.get("next_route")),
        (200, Some(&json!("X")), Some(&json!("/menu")))
    );
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .ok_or("no token")?;
    // audit: COSGN00C-014
    assert_eq!(state.jwt.verify(token)?.usr_type, "X");
    Ok(())
}

/// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
/// without the lint attributes that macro's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

#[test]
fn cosgn00c_signon_rows() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let (app, state) = app_over(&guard, &config(SECRET))?;
        successful_signons(&app, &state).await?;
        no_state_between_signons(&app).await?;
        blank_and_length_edits(&app).await?;
        non_object_bodies(&app).await?;
        failed_signons(&app).await?;
        stored_as_typed(&app, &state).await
    })
}

/// COSGN00C-020 over a dedicated database whose pool is closed, so that every read fails; the
/// blank-field edits still answer first, because they read nothing (COSGN00C-010).
#[test]
fn cosgn00c_store_failure_rows() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let (app, _state) = app_over(&guard, &config(SECRET))?;
        guard.pool().close().await;
        // audit: COSGN00C-010
        assert_eq!(
            sign_on(&app, "", DUSRSECJ_PASSWORD).await?,
            (400, screen_error(ENTER_USER_ID, Some("user_id")))
        );
        // audit: COSGN00C-010
        assert_eq!(
            sign_on(&app, "ADMIN001", "").await?,
            (400, screen_error(ENTER_PASSWORD, Some("password")))
        );
        for user_id in ["ADMIN001", "NOSUCHUS"] {
            let (status, body) = sign_on(&app, user_id, DUSRSECJ_PASSWORD).await?;
            // audit: COSGN00C-020
            assert_eq!(
                (status, &body),
                (500, &screen_error(UNABLE_TO_VERIFY, Some("user_id"))),
                "{user_id}"
            );
            assert_redacted(&body, DUSRSECJ_PASSWORD, user_id);
        }
        Ok(())
    })
}

/// Endpoint 2's body for a caller whose `usr_type` claim is `user_type` (COMEN01C-004,
/// COMEN01C-011, COMEN01C-013, COMEN01C-015, COMEN01C-017, COMEN01C-020, COMEN01C-021).
fn assert_main_menu(body: &Value, user_type: &str) -> TestResult {
    // audit: COMEN01C-004
    assert_eq!(body.get("user_type"), Some(&json!(user_type)));
    assert_eq!(
        keys(body),
        BTreeSet::from([
            "admin_only_message",
            "invalid_option_message",
            "options",
            "user_type"
        ])
    );
    let options = body
        .get("options")
        .and_then(Value::as_array)
        .ok_or("no option table")?;
    // audit: COMEN01C-020
    assert_eq!(options.len(), 11);
    for (option, (number, name, program)) in options.iter().zip(MAIN_OPTIONS) {
        let mut expected = json!({
            "number": number,
            "name": name,
            "text": option_text(number, name),
            "program": program,
            "user_type": "U",
            "admin_only": false,
            "installed": true,
        });
        if number == "11" {
            expected["unavailable_message"] = json!(OPTION_11_NOT_INSTALLED);
        }
        // audit: COMEN01C-013
        // audit: COMEN01C-015
        // audit: COMEN01C-020
        // audit: COMEN01C-021
        assert_eq!(option, &expected, "main option {number}");
        // audit: COMEN01C-020
        assert_eq!(
            option.get("name").and_then(Value::as_str).map(str::len),
            Some(35),
            "main option {number}"
        );
        // audit: COMEN01C-021
        assert_eq!(
            option.get("text").and_then(Value::as_str).map(str::len),
            Some(40),
            "main option {number}"
        );
    }
    // audit: COMEN01C-015
    assert_eq!(OPTION_11_NOT_INSTALLED.chars().count(), 58);
    // audit: COMEN01C-017
    assert!(options.iter().all(|option| {
        let program = option.get("program").and_then(Value::as_str);
        let message = option.get("unavailable_message").and_then(Value::as_str);
        program.is_some_and(|program| !program.starts_with("DUMMY"))
            && message.is_none_or(|message| !message.contains("coming soon"))
    }));
    // audit: COMEN01C-011
    assert_eq!(
        (
            body.get("invalid_option_message"),
            body.get("admin_only_message")
        ),
        (Some(&json!(INVALID_OPTION)), Some(&json!(ADMIN_ONLY)))
    );
    // audit: COMEN01C-011
    assert_eq!(
        (INVALID_OPTION.chars().count(), ADMIN_ONLY.chars().count()),
        (37, 33)
    );
    Ok(())
}

/// Endpoint 2 without a valid token: no header, a malformed header, another scheme, a payload
/// under another token's signature, a token of another secret and an expired token
/// (COMEN01C-002).
async fn main_menu_refuses_invalid_tokens(
    app: &Router,
    guard: &PostgresGuard,
    user_token: &str,
    admin_token: &str,
) -> TestResult {
    let (foreign_app, _) = app_over(guard, &config(OTHER_SECRET))?;
    let foreign = token(&foreign_app, "USER0001", DUSRSECJ_PASSWORD).await?;
    let short_lived = Config {
        jwt_ttl_seconds: 1,
        ..config(SECRET)
    };
    let (short_app, _) = app_over(guard, &short_lived)?;
    let expiring = token(&short_app, "USER0001", DUSRSECJ_PASSWORD).await?;
    // The token is refused from the second after its `exp`, with no leeway.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let refused = [
        None,
        Some("Bearer".to_owned()),
        Some("Bearer not-a-token".to_owned()),
        Some(format!("Basic {user_token}")),
        Some(bearer(&spliced(admin_token, user_token)?)),
        Some(bearer(&foreign)),
        Some(bearer(&expiring)),
    ];
    for (case, authorization) in refused.iter().enumerate() {
        let (status, body) =
            call(app, Method::GET, MAIN_MENU, authorization.as_deref(), None).await?;
        // audit: COMEN01C-002
        // The API's token message: no COBOL literal applies, and no option table is returned.
        assert_eq!(
            (status, body),
            (401, screen_error(SIGNON_REQUIRED, None)),
            "refused token case {case}"
        );
    }
    Ok(())
}

#[test]
fn comen01c_main_menu_rows() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let (app, _state) = app_over(&guard, &config(SECRET))?;
        let user_token = token(&app, "USER0001", DUSRSECJ_PASSWORD).await?;
        let admin_token = token(&app, "ADMIN001", DUSRSECJ_PASSWORD).await?;

        let (status, user_menu) = get(&app, MAIN_MENU, Some(&user_token)).await?;
        // audit: COMEN01C-004
        assert_eq!(status, 200);
        assert_main_menu(&user_menu, "U")?;
        let (status, admin_menu) = get(&app, MAIN_MENU, Some(&admin_token)).await?;
        // audit: COMEN01C-004
        assert_eq!(status, 200);
        assert_main_menu(&admin_menu, "A")?;
        // audit: COMEN01C-004
        assert_eq!(
            without_user_type(admin_menu),
            without_user_type(user_menu.clone())
        );
        // A single resource, not a collection: no `page_size` is declared or read.
        let paged = format!("{MAIN_MENU}?page_size=101");
        // audit: COMEN01C-004
        assert_eq!(
            get(&app, &paged, Some(&user_token)).await?,
            (200, user_menu)
        );
        let (status, document) = get(&app, DOCUMENT_ROUTE, None).await?;
        assert_eq!(status, 200);
        // audit: COMEN01C-004
        assert!(!declares_page_size(
            &document,
            "/paths/~1api~1v1~1menus~1main/get"
        )?);
        main_menu_refuses_invalid_tokens(&app, &guard, &user_token, &admin_token).await
    })
}

/// Admin option `index` of endpoint 3's body against row `index` of [`ADMIN_OPTIONS`].
fn assert_admin_option(options: &[Value], index: usize) -> TestResult {
    let &(number, name, program) = ADMIN_OPTIONS.get(index).ok_or("no such admin row")?;
    let option = options
        .get(index)
        .ok_or_else(|| format!("no admin option {number}"))?;
    // No `user_type` member: `COADM02Y` has no user-type column (COADM01C-012).
    let expected = json!({
        "number": number,
        "name": name,
        "text": option_text(number, name),
        "program": program,
        "admin_only": true,
        "installed": true,
        "unavailable_message": NOT_INSTALLED,
    });
    assert_eq!(option, &expected, "admin option {number}");
    assert_eq!(
        option.get("name").and_then(Value::as_str).map(str::len),
        Some(35),
        "admin option {number}"
    );
    assert_eq!(
        option.get("text").and_then(Value::as_str).map(str::len),
        Some(40),
        "admin option {number}"
    );
    Ok(())
}

/// Endpoint 3's body (COADM01C-005 to COADM01C-014).
fn assert_admin_menu(body: &Value) -> TestResult {
    // audit: COADM01C-012
    // audit: COADM01C-014
    assert_eq!(
        keys(body),
        BTreeSet::from(["invalid_option_message", "options"])
    );
    let options = body
        .get("options")
        .and_then(Value::as_array)
        .ok_or("no option table")?;
    // audit: COADM01C-005
    assert_eq!(options.len(), 6);
    // audit: COADM01C-006
    assert_admin_option(options, 0)?;
    // audit: COADM01C-007
    assert_admin_option(options, 1)?;
    // audit: COADM01C-008
    assert_admin_option(options, 2)?;
    // audit: COADM01C-009
    assert_admin_option(options, 3)?;
    // audit: COADM01C-010
    assert_admin_option(options, 4)?;
    // audit: COADM01C-011
    assert_admin_option(options, 5)?;
    // audit: COADM01C-012
    assert!(options.iter().all(|option| {
        option.get("admin_only") == Some(&json!(true)) && option.get("user_type").is_none()
    }));
    // audit: COADM01C-013
    assert!(
        options
            .iter()
            .all(|option| option.get("installed") == Some(&json!(true)))
    );
    // audit: COADM01C-014
    assert!(
        options
            .iter()
            .all(|option| option.get("unavailable_message") == Some(&json!(NOT_INSTALLED)))
    );
    // audit: COADM01C-014
    assert_eq!(
        body.get("invalid_option_message"),
        Some(&json!(INVALID_OPTION))
    );
    Ok(())
}

/// The `AdminUser` gate of endpoint 3 (COADM01C-001): 403 for a valid token whose `usr_type` is
/// not `A`, and 401 without a valid token, an `A` payload under a `U` signature included.
async fn admin_menu_gate(app: &Router, admin_token: &str) -> TestResult {
    let user_token = token(app, "USER0001", DUSRSECJ_PASSWORD).await?;
    add_user(app, admin_token, "TYPEX001", "TYPEXPW1", "X").await?;
    let other_type = token(app, "TYPEX001", "TYPEXPW1").await?;
    for (case, refused) in [&user_token, &other_type].into_iter().enumerate() {
        // audit: COADM01C-001
        // The API's gate message; no COBOL literal applies.
        assert_eq!(
            get(app, ADMIN_MENU, Some(refused)).await?,
            (403, screen_error(ADMIN_REQUIRED, None)),
            "non-admin token case {case}"
        );
    }
    let forged = bearer(&spliced(admin_token, &user_token)?);
    let invalid = [None, Some("Bearer not-a-token".to_owned()), Some(forged)];
    for (case, authorization) in invalid.iter().enumerate() {
        // audit: COADM01C-001
        assert_eq!(
            call(app, Method::GET, ADMIN_MENU, authorization.as_deref(), None).await?,
            (401, screen_error(SIGNON_REQUIRED, None)),
            "invalid token case {case}"
        );
    }
    Ok(())
}

#[test]
fn coadm01c_admin_menu_rows() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let (app, _state) = app_over(&guard, &config(SECRET))?;
        let first_admin = token(&app, "ADMIN001", DUSRSECJ_PASSWORD).await?;
        let second_admin = token(&app, "ADMIN002", DUSRSECJ_PASSWORD).await?;

        let (status, menu) = get(&app, ADMIN_MENU, Some(&first_admin)).await?;
        // audit: COADM01C-001
        assert_eq!(status, 200);
        assert_admin_menu(&menu)?;
        // audit: COADM01C-005
        assert_eq!(
            get(&app, ADMIN_MENU, Some(&second_admin)).await?,
            (200, menu.clone())
        );
        // A single resource, not a collection: no `page_size` is declared or read.
        let paged = format!("{ADMIN_MENU}?page_size=101");
        // audit: COADM01C-005
        assert_eq!(get(&app, &paged, Some(&first_admin)).await?, (200, menu));
        let (status, document) = get(&app, DOCUMENT_ROUTE, None).await?;
        assert_eq!(status, 200);
        // audit: COADM01C-005
        assert!(!declares_page_size(
            &document,
            "/paths/~1api~1v1~1menus~1admin/get"
        )?);
        admin_menu_gate(&app, &first_admin).await
    })
}

/// A `GET` of `uri` under `token` that also sends the header `name: value`, as a caller naming
/// its own user type would send it.
async fn get_with_header(
    app: &Router,
    uri: &str,
    token: &str,
    (name, value): (&str, &str),
) -> TestResult<(u16, Value)> {
    let request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::AUTHORIZATION, bearer(token))
        .header(name, value)
        .body(Body::empty())?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, value))
}

/// Every parameter name and every schema property name of the `OpenAPI` document `value`, at
/// any depth, added to `names`.
fn document_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            let parameters = object.get("parameters").and_then(Value::as_array);
            names.extend(
                parameters
                    .into_iter()
                    .flatten()
                    .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                    .map(str::to_owned),
            );
            if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                names.extend(properties.keys().cloned());
            }
            for nested in object.values() {
                document_names(nested, names);
            }
        }
        Value::Array(items) => {
            for nested in items {
                document_names(nested, names);
            }
        }
        _ => {}
    }
}

/// Endpoint 1 for `case`'s typed user ID with the DUSRSECJ password, held to what COSGN00C
/// moves into the COMMAREA before its `XCTL` (`COSGN00C.cbl:224-238`): the user ID and type
/// become the claims `sub` and `usr_type`, the transfer target becomes `next_route`, and no other
/// field travels. `case` is the typed ID, the stored ID, the user type and the target route.
/// Returns the token.
async fn commarea_signon(
    app: &Router,
    state: &AppState,
    case: (&str, &str, &str, &str),
) -> TestResult<String> {
    let (typed_id, user_id, user_type, next_route) = case;
    let (status, body) = sign_on(app, typed_id, DUSRSECJ_PASSWORD).await?;
    assert_eq!(status, 200, "{typed_id}");
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .ok_or("no token")?
        .to_owned();
    let claims = state.jwt.verify(&token)?;
    // audit: COCOM01Y-005
    assert_eq!(claims.sub, user_id, "{typed_id}: sub");
    // audit: COCOM01Y-006
    assert_eq!(claims.usr_type, user_type, "{typed_id}: usr_type");
    // The token holds the user ID, the user type and its own times, and nothing else.
    // audit: COCOM01Y-001
    // audit: COCOM01Y-002
    // audit: COCOM01Y-007
    assert_eq!(
        claim_names(&token, SECRET)?,
        BTreeSet::from(["exp", "iat", "sub", "usr_type"].map(str::to_owned)),
        "{typed_id}: claims"
    );
    // audit: COCOM01Y-001
    // audit: COCOM01Y-002
    // audit: COCOM01Y-003
    // audit: COCOM01Y-004
    // audit: COCOM01Y-007
    assert_eq!(
        keys(&body),
        BTreeSet::from(["next_route", "token", "user_id", "user_type"]),
        "{typed_id}: members"
    );
    // The target screen is named by its route, never by its program, transaction, map or mapset.
    // audit: COCOM01Y-003
    // audit: COCOM01Y-004
    // audit: COCOM01Y-015
    // audit: COCOM01Y-016
    assert_eq!(
        body.get("next_route"),
        Some(&json!(next_route)),
        "{typed_id}: next_route"
    );
    let mut shown = body;
    if let Some(object) = shown.as_object_mut() {
        object.remove("token");
    }
    let shown = shown.to_string();
    // What COSGN00C moves or names (`WS-TRANID`, `WS-PGMNAME`, the `XCTL` targets and their
    // transactions, the maps and mapsets of the three screens): no member of the answer holds it.
    let moved: [(&str, &[&str]); 6] = [
        // audit: COCOM01Y-001
        ("CDEMO-FROM-TRANID", &["CC00"]),
        // audit: COCOM01Y-002
        ("CDEMO-FROM-PROGRAM", &["COSGN00C"]),
        // audit: COCOM01Y-003
        ("CDEMO-TO-TRANID", &["CA00", "CM00"]),
        // audit: COCOM01Y-004
        ("CDEMO-TO-PROGRAM", &["COADM01C", "COMEN01C"]),
        // audit: COCOM01Y-015
        ("CDEMO-LAST-MAP", &["COSGN0A", "COADM1A", "COMEN1A"]),
        // audit: COCOM01Y-016
        ("CDEMO-LAST-MAPSET", &["COSGN00", "COADM01", "COMEN01"]),
    ];
    for (field, values) in moved {
        assert!(
            values.iter().all(|value| !shown.contains(value)),
            "{typed_id}: the answer carries {field}"
        );
    }
    Ok(token)
}

/// The user ID and type of every request come from its verified token alone (COCOM01Y-005,
/// COCOM01Y-006), whatever the query string or the headers name, and no request depends on the
/// one before it (COCOM01Y-007).
async fn commarea_from_token_only(
    app: &Router,
    state: &AppState,
    admin_token: &str,
    user_token: &str,
) -> TestResult {
    // ADMIN001's claims under USER0001's signature: a payload alone names nobody.
    let forged = bearer(&spliced(admin_token, user_token)?);
    let (status, refused) = call(app, Method::GET, MAIN_MENU, Some(&forged), None).await?;
    // audit: COCOM01Y-005
    assert!(
        status == 401 && refused == screen_error(SIGNON_REQUIRED, None),
        "a spliced token is served"
    );
    let named_admin = format!("{MAIN_MENU}?usr_type=A");
    for token in [admin_token, user_token] {
        let claimed = state.jwt.verify(token)?.usr_type;
        let (status, menu) =
            get_with_header(app, &named_admin, token, ("x-user-type", "A")).await?;
        // audit: COCOM01Y-006
        assert_eq!(
            (status, menu.get("user_type")),
            (200, Some(&json!(claimed))),
            "main menu for the {claimed} token"
        );
    }
    let named_user = format!("{ADMIN_MENU}?usr_type=U");
    let (status, _) = get_with_header(app, &named_user, admin_token, ("x-user-type", "U")).await?;
    // audit: COCOM01Y-006
    assert_eq!(status, 200, "an A token whose request names type U");
    let named_admin = format!("{ADMIN_MENU}?usr_type=A");
    let (status, refused) =
        get_with_header(app, &named_admin, user_token, ("x-user-type", "A")).await?;
    // audit: COCOM01Y-006
    assert!(
        status == 403 && refused == screen_error(ADMIN_REQUIRED, None),
        "a U token whose request names type A is served the admin menu"
    );
    // No first entry is told from a re-entry: the same request twice answers the same body.
    for (uri, token) in [(MAIN_MENU, user_token), (ADMIN_MENU, admin_token)] {
        let first = get(app, uri, Some(token)).await?;
        let second = get(app, uri, Some(token)).await?;
        // audit: COCOM01Y-007
        assert!(
            first.0 == 200 && first == second,
            "{uri}: a repeated request answers otherwise"
        );
    }
    Ok(())
}

/// Whether an `OpenAPI` name would carry a COMMAREA field.
type Carries = fn(&str) -> bool;

/// No parameter and no schema property of the `OpenAPI` document, at any depth, carries a
/// COMMAREA field the replacement drops: no from- or to- name, no program context and no map
/// (COCOM01Y-001 to COCOM01Y-004, COCOM01Y-007, COCOM01Y-015, COCOM01Y-016).
async fn commarea_document_absence(app: &Router) -> TestResult {
    let (status, document) = get(app, DOCUMENT_ROUTE, None).await?;
    assert_eq!(status, 200);
    let mut names = BTreeSet::new();
    document_names(&document, &mut names);
    // The scan reaches query parameters and nested schema properties, so no result is vacuous.
    for reached in [
        "acct_id",
        "card_ref",
        "first_name",
        "next_route",
        "page_size",
    ] {
        assert!(names.contains(reached), "the scan misses {reached}");
    }
    let absent: [(&str, Carries); 7] = [
        // audit: COCOM01Y-001
        ("CDEMO-FROM-TRANID", |name| {
            name.starts_with("from_") || name.contains("tranid")
        }),
        // audit: COCOM01Y-002
        ("CDEMO-FROM-PROGRAM", |name| {
            name.starts_with("from_") || name.contains("pgm")
        }),
        // audit: COCOM01Y-003
        ("CDEMO-TO-TRANID", |name| {
            name.starts_with("to_") || name.contains("tranid")
        }),
        // audit: COCOM01Y-004
        ("CDEMO-TO-PROGRAM", |name| {
            name.starts_with("to_") || name.contains("pgm")
        }),
        // audit: COCOM01Y-007
        ("CDEMO-PGM-CONTEXT", |name| {
            name.contains("context") || name.contains("reenter")
        }),
        // audit: COCOM01Y-015
        ("CDEMO-LAST-MAP", |name| name.contains("map")),
        // audit: COCOM01Y-016
        ("CDEMO-LAST-MAPSET", |name| name.contains("map")),
    ];
    for (field, carries) in absent {
        let carried: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| carries(name))
            .collect();
        assert!(carried.is_empty(), "{field} is carried as {carried:?}");
    }
    Ok(())
}

/// COCOM01Y's general and more-info blocks as the replacement carries them (COCOM01Y-001 to
/// COCOM01Y-007, COCOM01Y-015, COCOM01Y-016): COSGN00C's moves before its `XCTL` become the
/// claims `sub` and `usr_type` and the route `next_route`, every request takes its user from its
/// verified token, and nothing else travels.
#[test]
fn cocom01y_commarea_rows() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let (app, state) = app_over(&guard, &config(SECRET))?;
        let admin = ("admin001", "ADMIN001", "A", "/admin");
        let admin_token = commarea_signon(&app, &state, admin).await?;
        let user = ("USER0001", "USER0001", "U", "/menu");
        let user_token = commarea_signon(&app, &state, user).await?;
        commarea_from_token_only(&app, &state, &admin_token, &user_token).await?;
        commarea_document_absence(&app).await
    })
}
