//! Bindings R8 and n5 (AAP 0.9.1) against the fixture-loaded database.
//!
//! - **R8:** after `dusrsecj`, `ADMIN001` and `USER0001` sign on with the DUSRSECJ password
//!   (`app/jcl/DUSRSECJ.jcl:35,40`) and receive user type `A` and `U` and next route `/admin`
//!   and `/menu`; the audited sign-on failures answer with their COSGN00C messages, statuses and
//!   fields (`modern/audit/online/COSGN00C.md`).
//! - **n5:** each of the 26 bearer operations (every endpoint except 1) refuses a request without
//!   a valid token with 401; each `[A]` operation refuses a `U` token with 403 and serves an `A`
//!   token; each other operation serves both.
//! - **Path rejections after the gate:** on the eight operations whose path segments reach their
//!   program's edit as raw text (4, 5, 6, 10, 12, 21, 22 and 23), a segment that is not UTF-8
//!   once percent-decoded is a 401 without a valid token and, with one, the JSON `ScreenError`
//!   400 `api::dto::INVALID_PATH` that echoes nothing (AAP 0.3.4.1).
//!
//! The acting identity comes only from the verified token (AAP 0.5.1, 0.3.4.6): valid tokens come
//! only from endpoint 1, and a spoofed header or query value does not change the gate's answer.
//! The 401 and 403 texts are this crate's own, so they are compared with the
//! `api::auth::extract` constants; the sign-on literals are copied from the audit rows. Every
//! response read here is checked for the bearer challenge: a 401 carries exactly one
//! `WWW-Authenticate: Bearer` (RFC 9110 §11.6.1, RFC 6750 §3), and any other status none.
//!
//! Every request goes to the router in process. Each test runs against its own `postgres:16`
//! container, which `test_support` prepares and loads with the fixtures and removes when its
//! guard drops. No request is destructive: each update is an ENTER that only validates, the user
//! delete names a user that does not exist, and no report is selected.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app};
use api::auth::extract::{ADMIN_REQUIRED, SIGNON_REQUIRED};
use api::auth::jwt::{JwtError, JwtKeys};
use api::dto::INVALID_PATH;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, header};
use chrono::NaiveDate;
use domain::clock::FixedClock;
use domain::copybook::PicX;
use domain::online::cosgn00c::UserType;
use persistence::PgStore;
use runtime::config::Config;
use serde_json::{Value, json};
use test_support::fixtures::start_loaded_postgres;
use test_support::postgres::PostgresGuard;
use tower::ServiceExt as _;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The local-only signing secret of these tests.
const SECRET: &str = "auth-gate-local-only-test-signing-secret";

/// The token lifetime the API is configured with: the default `CARDDEMO_JWT_TTL_SECONDS`.
const TTL: u64 = 3_600;

/// The password of every in-stream `USRSEC` user (`app/jcl/DUSRSECJ.jcl:35-44`), the demo
/// credential the repository's README publishes. Failure messages never print it.
const DUSRSECJ_PASSWORD: &str = "PASSWORD";

/// Endpoint 1, the one operation that takes no token.
const SIGNON_URI: &str = "/api/v1/signon";

/// Endpoints 5 and 6: account `00000000001`, whose card and customer the fixtures cross-reference
/// (`app/data/ASCII/cardxref.txt:49`).
const ACCOUNT_URI: &str = "/api/v1/accounts/00000000001";

/// Endpoint 8: card `0500024453765740` of account `00000000050`
/// (`app/data/ASCII/cardxref.txt:1`).
const CARD_VIEW_URI: &str = "/api/v1/cards/view?acct_id=00000000050&card_num=0500024453765740";

/// Endpoint 9: the same card, fetched for update.
const CARD_EDIT_URI: &str = "/api/v1/cards/edit?acct_id=00000000050&card_num=0500024453765740";

/// Endpoint 24 with no filter: the first page of the Db2 transaction types.
const TYPE_LIST_URI: &str = "/api/v1/transaction-types";

/// An issue time in 2001, so that a token issued then expired long before the test runs.
/// `jsonwebtoken` checks `exp` against the system clock, not the API's `FixedClock`.
const LONG_AGO: u64 = 1_000_000_000;

/// The endpoints of the 26 bearer operations: every endpoint except sign-on.
const BEARER_ENDPOINTS: [u8; 26] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
];

/// The `[A]` operations of AAP 0.3.4.1.
const ADMIN_ENDPOINTS: [u8; 10] = [3, 16, 17, 18, 19, 20, 24, 25, 26, 27];

/// The API over the fixture-loaded database of `pg`: tokens signed with [`SECRET`] and valid for
/// [`TTL`] seconds, the clock fixed at 2022-07-18 10:30:00.000, job decks under this test
/// target's temporary directory.
fn api(pg: &PostgresGuard) -> TestResult<Router> {
    let config = Config {
        jwt_secret: SECRET.to_owned(),
        jwt_ttl_seconds: TTL,
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("auth_gate_jobs"),
        ..Config::default()
    };
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or("an invalid test instant")?;
    let store = PgStore::new(pg.pool().clone());
    let state = AppState::from_config(store, &config, Arc::new(FixedClock::new(at)))?;
    Ok(app(state))
}

/// The bearer challenge of a response of `status` with `headers`: exactly one `WWW-Authenticate`
/// value, `Bearer`, on a 401, and none on any other status.
fn check_challenge(status: u16, headers: &HeaderMap) -> TestResult {
    let challenges: Vec<String> = headers
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect();
    let expected: &[&str] = if status == 401 { &["Bearer"] } else { &[] };
    if challenges == expected {
        Ok(())
    } else {
        Err(format!(
            "a {status} response carries the WWW-Authenticate values {challenges:?}, not {expected:?}"
        )
        .into())
    }
}

/// Sends `request` to `api` and returns the status and the JSON body, `Value::Null` when the
/// body is empty, once [`check_challenge`] has passed the response's headers.
async fn send(api: &Router, request: Request<Body>) -> TestResult<(u16, Value)> {
    let response = api.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    check_challenge(status, response.headers())?;
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, json))
}

/// Sends `method` on `uri`, with `body` as JSON when one is given and `authorization` as the raw
/// `Authorization` header value when one is given, so that a test can send `Bearer …`,
/// `Basic …` or no header at all.
async fn call(
    api: &Router,
    method: Method,
    uri: &str,
    body: Option<&Value>,
    authorization: Option<&str>,
) -> TestResult<(u16, Value)> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(json)?))?,
        None => builder.body(Body::empty())?,
    };
    send(api, request).await
}

/// The string member `key` of a JSON object, if it has one.
fn member<'v>(body: &'v Value, key: &str) -> Option<&'v str> {
    body.get(key).and_then(Value::as_str)
}

/// `token` as the value of an `Authorization: Bearer` header.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Endpoint 1 with `body` and no `Authorization` header.
async fn signon(api: &Router, body: &Value) -> TestResult<(u16, Value)> {
    call(api, Method::POST, SIGNON_URI, Some(body), None).await
}

/// The bearer token endpoint 1 issues to `user_id` for the DUSRSECJ password.
async fn signon_token(api: &Router, user_id: &str) -> TestResult<String> {
    let body = json!({ "user_id": user_id, "password": DUSRSECJ_PASSWORD });
    let (status, response) = signon(api, &body).await?;
    if status != 200 {
        return Err(format!("the sign-on of {user_id} answered {status}").into());
    }
    member(&response, "token")
        .map(str::to_owned)
        .ok_or_else(|| format!("the sign-on of {user_id} returned no token").into())
}

/// Whether `value` holds the JSON key `key` at any depth.
fn has_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(name, member)| name == key || has_key(member, key)),
        Value::Array(items) => items.iter().any(|item| has_key(item, key)),
        _ => false,
    }
}

/// Asserts that a sign-on response carries no password: no `password` key at any depth and not
/// the non-blank `submitted` text. The failure message names neither (Cardholder Data
/// Redaction).
fn assert_no_password(response: &Value, submitted: &str, case: &str) {
    assert!(
        !has_key(response, "password"),
        "{case}: the sign-on response has a password key"
    );
    if !submitted.trim().is_empty() {
        assert!(
            !response.to_string().contains(submitted),
            "{case}: the sign-on response carries the submitted password"
        );
    }
}

/// R8: the two DUSRSECJ users sign on to their menus, the audited failures answer with their
/// messages, and an unreadable user store answers with the `WHEN OTHER` message.
#[test]
fn signon_parity_r8() -> TestResult {
    block_on(async {
        let pg = start_loaded_postgres().await?;
        let api = api(&pg)?;
        signed_on_users(&api).await?;
        signon_failures(&api).await?;
        unreadable_user_store(&api, &pg).await
    })
}

/// `ADMIN001` and `USER0001` sign on, also when both fields are typed in lower case, and the
/// token carries the identity and lifetime of COSGN00C-014.
async fn signed_on_users(api: &Router) -> TestResult {
    let lower_case_password = DUSRSECJ_PASSWORD.to_ascii_lowercase();
    let cases = [
        // audit: COSGN00C-015
        ("ADMIN001", DUSRSECJ_PASSWORD, "ADMIN001", "A", "/admin"),
        // audit: COSGN00C-016
        ("USER0001", DUSRSECJ_PASSWORD, "USER0001", "U", "/menu"),
        // Sign-on upper-cases both fields before the read and the comparison (E26).
        // audit: COSGN00C-009
        (
            "admin001",
            lower_case_password.as_str(),
            "ADMIN001",
            "A",
            "/admin",
        ),
    ];
    let keys = JwtKeys::new(SECRET, TTL);
    for (typed_id, typed_password, user_id, user_type, next_route) in cases {
        let body = json!({ "user_id": typed_id, "password": typed_password });
        let (status, response) = signon(api, &body).await?;
        // audit: COSGN00C-013
        assert_eq!(status, 200, "{typed_id}: status");
        assert_eq!(member(&response, "user_id"), Some(user_id), "{typed_id}");
        assert_eq!(
            member(&response, "user_type"),
            Some(user_type),
            "{typed_id}"
        );
        assert_eq!(
            member(&response, "next_route"),
            Some(next_route),
            "{typed_id}"
        );
        assert_eq!(
            response.get("message"),
            None,
            "{typed_id}: a success has no message"
        );
        assert_no_password(&response, typed_password, typed_id);

        let token = member(&response, "token").unwrap_or_default();
        assert!(!token.is_empty(), "{typed_id}: no token");
        let claims = keys.verify(token)?;
        // audit: COSGN00C-014
        assert_eq!(claims.sub, user_id, "{typed_id}: sub");
        assert_eq!(claims.usr_type, user_type, "{typed_id}: usr_type");
        assert_eq!(
            claims.exp.checked_sub(claims.iat),
            Some(TTL),
            "{typed_id}: lifetime"
        );
    }
    Ok(())
}

/// The blank-field edits, the wrong password and the unknown user: status, message and field
/// as the COSGN00C audit rows give them, and no token.
async fn signon_failures(api: &Router) -> TestResult {
    let cases = [
        // A blank user ID is reported whatever the password holds.
        // audit: COSGN00C-007
        (
            json!({ "user_id": "", "password": DUSRSECJ_PASSWORD }),
            400,
            "Please enter User ID ...",
            "user_id",
        ),
        (
            json!({ "user_id": "        ", "password": "" }),
            400,
            "Please enter User ID ...",
            "user_id",
        ),
        (
            json!({ "password": DUSRSECJ_PASSWORD }),
            400,
            "Please enter User ID ...",
            "user_id",
        ),
        // audit: COSGN00C-008
        (
            json!({ "user_id": "ADMIN001", "password": "" }),
            400,
            "Please enter Password ...",
            "password",
        ),
        (
            json!({ "user_id": "ADMIN001", "password": "        " }),
            400,
            "Please enter Password ...",
            "password",
        ),
        (
            json!({ "user_id": "ADMIN001" }),
            400,
            "Please enter Password ...",
            "password",
        ),
        // audit: COSGN00C-018
        (
            json!({ "user_id": "ADMIN001", "password": "WRONGPWD" }),
            401,
            "Wrong Password. Try again ...",
            "password",
        ),
        // audit: COSGN00C-019
        (
            json!({ "user_id": "NOUSER01", "password": DUSRSECJ_PASSWORD }),
            401,
            "User not found. Try again ...",
            "user_id",
        ),
    ];
    for (body, status, message, field) in cases {
        let case = format!(
            "{} / {message}",
            member(&body, "user_id").unwrap_or("<omitted>")
        );
        let (answered, response) = signon(api, &body).await?;
        assert_eq!(answered, status, "{case}: status");
        assert_eq!(member(&response, "message"), Some(message), "{case}");
        assert_eq!(member(&response, "field"), Some(field), "{case}: field");
        assert_eq!(
            response.get("token"),
            None,
            "{case}: a failure issues no token"
        );
        assert_no_password(
            &response,
            member(&body, "password").unwrap_or_default(),
            &case,
        );
    }
    Ok(())
}

/// A user read that fails for a reason other than not-found, here a closed pool, answers with
/// the `WHEN OTHER` message and no token. It runs last: the pool stays closed.
async fn unreadable_user_store(api: &Router, pg: &PostgresGuard) -> TestResult {
    pg.pool().close().await;
    let body = json!({ "user_id": "ADMIN001", "password": DUSRSECJ_PASSWORD });
    let (status, response) = signon(api, &body).await?;
    // audit: COSGN00C-020
    assert_eq!(status, 500, "closed store: status");
    assert_eq!(
        member(&response, "message"),
        Some("Unable to verify the User ...")
    );
    assert_eq!(
        member(&response, "field"),
        Some("user_id"),
        "closed store: field"
    );
    assert_eq!(response.get("token"), None, "closed store: no token");
    assert_no_password(&response, DUSRSECJ_PASSWORD, "closed store");
    Ok(())
}

/// The gate an operation's handler declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// `AdminUser`: the `[A]` operations.
    Admin,
    /// `SignedOnUser`: every other bearer operation.
    SignedOn,
}

/// One bearer operation with a concrete, non-destructive request.
struct Operation {
    gate: Gate,
    /// The endpoint number of AAP 0.3.4.1.
    endpoint: u8,
    method: Method,
    /// The path template under `/api/v1`, the only part of the request a failure message
    /// prints: query strings carry card numbers.
    template: &'static str,
    uri: String,
    body: Option<Value>,
}

impl Operation {
    fn new(
        gate: Gate,
        endpoint: u8,
        method: Method,
        template: &'static str,
        uri: impl Into<String>,
    ) -> Self {
        Self {
            gate,
            endpoint,
            method,
            template,
            uri: uri.into(),
            body: None,
        }
    }

    fn with_body(self, body: Value) -> Self {
        Self {
            body: Some(body),
            ..self
        }
    }

    /// The request, with `authorization` as the raw `Authorization` header value.
    async fn send(&self, api: &Router, authorization: Option<&str>) -> TestResult<(u16, Value)> {
        let method = self.method.clone();
        call(api, method, &self.uri, self.body.as_ref(), authorization).await
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "endpoint {} ({} {})",
            self.endpoint, self.method, self.template
        )
    }
}

/// What the requests of endpoints 6, 10 and 25 echo, read first with an `A` token: the
/// snapshots the update screens send back, the card handle of endpoint 10's path and the list
/// state endpoint 25 receives.
struct Prepared {
    account_snapshot: Value,
    card_ref: i64,
    card_snapshot: Value,
    type_list_state: Value,
}

/// The member `key` of a preparatory read's response.
fn echoed(response: &Value, key: &str, endpoint: u8) -> TestResult<Value> {
    response
        .get(key)
        .cloned()
        .ok_or_else(|| format!("endpoint {endpoint} returned no {key}").into())
}

/// The 200 response of a `GET` of `uri` with `authorization`.
async fn read_served(
    api: &Router,
    endpoint: u8,
    uri: &str,
    authorization: &str,
) -> TestResult<Value> {
    let (status, response) = call(api, Method::GET, uri, None, Some(authorization)).await?;
    if status == 200 {
        Ok(response)
    } else {
        Err(format!("the preparatory read of endpoint {endpoint} answered {status}").into())
    }
}

/// Reads what endpoints 6, 10 and 25 echo through endpoints 5, 9 and 24.
async fn prepare(api: &Router, admin: &str) -> TestResult<Prepared> {
    let account = read_served(api, 5, ACCOUNT_URI, admin).await?;
    let card = read_served(api, 9, CARD_EDIT_URI, admin).await?;
    let types = read_served(api, 24, TYPE_LIST_URI, admin).await?;
    Ok(Prepared {
        account_snapshot: echoed(&account, "snapshot", 5)?,
        card_ref: card
            .get("card_ref")
            .and_then(Value::as_i64)
            .ok_or("endpoint 9 returned no card_ref")?,
        card_snapshot: echoed(&card, "snapshot", 9)?,
        type_list_state: echoed(&types, "state", 24)?,
    })
}

/// The `SignedOnUser` operations 2 and 4 to 10: the main menu, the account view and update,
/// and the card list, view and update.
fn menu_account_card_operations(prepared: &Prepared) -> Vec<Operation> {
    let gate = Gate::SignedOn;
    let card_uri = format!("/api/v1/cards/{}", prepared.card_ref);
    vec![
        // audit: COMEN01C-002
        Operation::new(gate, 2, Method::GET, "/menus/main", "/api/v1/menus/main"),
        Operation::new(
            gate,
            4,
            Method::GET,
            "/accounts/{acct_id}/view",
            format!("{ACCOUNT_URI}/view"),
        ),
        Operation::new(gate, 5, Method::GET, "/accounts/{acct_id}", ACCOUNT_URI),
        Operation::new(gate, 6, Method::PUT, "/accounts/{acct_id}", ACCOUNT_URI).with_body(json!({
            "aid": "ENTER", "fields": {}, "snapshot": prepared.account_snapshot,
        })),
        Operation::new(
            gate,
            7,
            Method::GET,
            "/cards",
            "/api/v1/cards?acct_id=00000000050",
        ),
        Operation::new(gate, 8, Method::GET, "/cards/view", CARD_VIEW_URI),
        Operation::new(gate, 9, Method::GET, "/cards/edit", CARD_EDIT_URI),
        Operation::new(gate, 10, Method::PUT, "/cards/{card_ref}", card_uri).with_body(json!({
            "aid": "ENTER", "fields": {}, "snapshot": prepared.card_snapshot,
        })),
    ]
}

/// The `SignedOnUser` operations 11 to 15 and 21 to 23: the transaction list, view and add,
/// the report request, bill payment, and the pending authorizations.
fn transaction_authorization_operations() -> Vec<Operation> {
    let gate = Gate::SignedOn;
    let detail_uri = "/api/v1/authorizations/00000000001/0778000000000001";
    vec![
        Operation::new(
            gate,
            11,
            Method::GET,
            "/transactions",
            "/api/v1/transactions",
        ),
        Operation::new(
            gate,
            12,
            Method::GET,
            "/transactions/{tran_id}",
            "/api/v1/transactions/0000000000000001",
        ),
        // audit: COTRN02C-003
        // audit: COTRN02C-061
        Operation::new(
            gate,
            13,
            Method::POST,
            "/transactions",
            "/api/v1/transactions",
        )
        .with_body(json!({ "aid": "ENTER", "fields": {}, "confirm": "" })),
        // audit: CORPT00C-003
        // audit: CORPT00C-043
        Operation::new(
            gate,
            14,
            Method::POST,
            "/reports/transactions",
            "/api/v1/reports/transactions",
        )
        .with_body(json!({})),
        // audit: COBIL00C-003
        // audit: COBIL00C-038
        Operation::new(
            gate,
            15,
            Method::POST,
            "/bill-payments",
            "/api/v1/bill-payments",
        )
        .with_body(json!({ "acct_id": "00000000001", "confirm": "" })),
        Operation::new(
            gate,
            21,
            Method::GET,
            "/accounts/{acct_id}/authorizations",
            format!("{ACCOUNT_URI}/authorizations"),
        ),
        Operation::new(
            gate,
            22,
            Method::GET,
            "/authorizations/{acct_id}/{auth_key}",
            detail_uri,
        ),
        Operation::new(
            gate,
            23,
            Method::POST,
            "/authorizations/{acct_id}/{auth_key}/fraud",
            format!("{detail_uri}/fraud"),
        ),
    ]
}

/// The 10 `[A]` operations behind `AdminUser`: 3, 16 to 20 and 24 to 27. Each gate row is
/// tagged above the operations it covers, for its 401 and 403 cases alike.
fn admin_operations(prepared: &Prepared) -> Vec<Operation> {
    let gate = Gate::Admin;
    let type_uri = "/api/v1/transaction-types/01";
    vec![
        // audit: COADM01C-001
        Operation::new(gate, 3, Method::GET, "/menus/admin", "/api/v1/menus/admin"),
        // audit: COUSR00C-001
        Operation::new(gate, 16, Method::GET, "/users", "/api/v1/users"),
        // audit: COUSR01C-001
        // audit: COUSR01C-002
        Operation::new(gate, 17, Method::POST, "/users", "/api/v1/users").with_body(json!({})),
        // audit: COUSR02C-001
        // audit: COUSR02C-002
        Operation::new(
            gate,
            18,
            Method::GET,
            "/users/{user_id}",
            "/api/v1/users/USER0002",
        ),
        // USER0002's stored names and type (`DUSRSECJ.jcl:41`), no password: nothing changes.
        Operation::new(
            gate,
            19,
            Method::PUT,
            "/users/{user_id}",
            "/api/v1/users/USER0002",
        )
        .with_body(json!({
            "first_name": "AJITH", "last_name": "KUMAR", "user_type": "U",
        })),
        // audit: COUSR03C-001
        // audit: COUSR03C-002
        Operation::new(
            gate,
            20,
            Method::DELETE,
            "/users/{user_id}",
            "/api/v1/users/NOSUCHU1",
        ),
        // audit: COTRTLIC-001
        Operation::new(gate, 24, Method::GET, "/transaction-types", TYPE_LIST_URI),
        Operation::new(
            gate,
            25,
            Method::POST,
            "/transaction-types/list-actions",
            "/api/v1/transaction-types/list-actions",
        )
        .with_body(json!({
            "aid": "ENTER", "type_filter": "", "desc_filter": "", "rows": [],
            "state": prepared.type_list_state,
        })),
        // audit: COTRTUPC-001
        Operation::new(
            gate,
            26,
            Method::GET,
            "/transaction-types/{type_code}",
            type_uri,
        ),
        Operation::new(
            gate,
            27,
            Method::POST,
            "/transaction-types/{type_code}/actions",
            format!("{type_uri}/actions"),
        )
        .with_body(json!({ "aid": "ENTER" })),
    ]
}

/// The 26 bearer operations in endpoint order, with the echoed values read through `admin`.
async fn bearer_operations(api: &Router, admin: &str) -> TestResult<Vec<Operation>> {
    let prepared = prepare(api, admin).await?;
    let mut operations = menu_account_card_operations(&prepared);
    operations.extend(transaction_authorization_operations());
    operations.extend(admin_operations(&prepared));
    operations.sort_by_key(|operation| operation.endpoint);
    let endpoints: Vec<u8> = operations
        .iter()
        .map(|operation| operation.endpoint)
        .collect();
    assert_eq!(
        endpoints, BEARER_ENDPOINTS,
        "one operation per bearer endpoint"
    );
    let admin_endpoints: Vec<u8> = operations
        .iter()
        .filter(|operation| operation.gate == Gate::Admin)
        .map(|operation| operation.endpoint)
        .collect();
    assert_eq!(admin_endpoints, ADMIN_ENDPOINTS, "the [A] operations");
    Ok(operations)
}

/// Whether a status shows the gate let the request through to its service.
const fn served(status: u16) -> bool {
    status != 401 && status != 403
}

/// n5, the admin gate: a `U` token is refused on every `[A]` operation with 403 and served on
/// every other one; an `A` token is served on all 26.
#[test]
fn admin_gate_n5() -> TestResult {
    block_on(async {
        let pg = start_loaded_postgres().await?;
        let api = api(&pg)?;
        let admin = bearer(&signon_token(&api, "ADMIN001").await?);
        let user = bearer(&signon_token(&api, "USER0001").await?);
        let operations = bearer_operations(&api, &admin).await?;
        for operation in &operations {
            let (status, response) = operation.send(&api, Some(&user)).await?;
            match operation.gate {
                Gate::Admin => {
                    assert_eq!(status, 403, "{operation} with a U token");
                    assert_eq!(
                        member(&response, "message"),
                        Some(ADMIN_REQUIRED),
                        "{operation}"
                    );
                    assert_eq!(
                        response.get("field"),
                        None,
                        "{operation}: the gate names no field"
                    );
                }
                Gate::SignedOn => {
                    assert!(served(status), "{operation} refused a U token: {status}");
                }
            }
            let (status, _) = operation.send(&api, Some(&admin)).await?;
            assert!(served(status), "{operation} refused an A token: {status}");
        }
        spoofed_identity_is_ignored(&api, &user).await
    })
}

/// A `U` token with an `X-User-Type: A` header and `usr_type=A` in the query string is still
/// refused by endpoint 3: the gate reads the verified claims only (AAP 0.5.1).
async fn spoofed_identity_is_ignored(api: &Router, user: &str) -> TestResult {
    let request = Request::get("/api/v1/menus/admin?usr_type=A")
        .header(header::AUTHORIZATION, user)
        .header("X-User-Type", "A")
        .body(Body::empty())?;
    let (status, response) = send(api, request).await?;
    // audit: COADM01C-001
    assert_eq!(status, 403, "a spoofed user type opened the admin menu");
    assert_eq!(member(&response, "message"), Some(ADMIN_REQUIRED));
    Ok(())
}

/// `user`'s header and signature around `admin`'s payload: a token that claims `ADMIN001` and
/// type `A` under a signature computed over `USER0001`'s claims.
fn splice(user: &str, admin: &str) -> TestResult<String> {
    let (header, user_payload, signature) = segments(user)?;
    let (_, admin_payload, _) = segments(admin)?;
    assert_ne!(user_payload, admin_payload, "the two payloads must differ");
    Ok(format!("{header}.{admin_payload}.{signature}"))
}

/// The header, payload and signature segments of a compact JWT.
fn segments(token: &str) -> TestResult<(&str, &str, &str)> {
    let mut parts = token.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(header), Some(payload), Some(signature), None) => Ok((header, payload, signature)),
        _ => Err("a sign-on token that is not three dot-separated segments".into()),
    }
}

/// Every `Authorization` value the extractors must refuse, each with its label: none, a bearer
/// value that is no JWT, another scheme, a token of another secret, an expired token and a
/// tampered one. Each forged token is first checked to fail verification for its own reason.
fn refused_authorizations(
    admin: &str,
    user: &str,
) -> TestResult<Vec<(&'static str, Option<String>)>> {
    let admin_id = PicX::from_text("ADMIN001")?;
    let keys = JwtKeys::new(SECRET, TTL);

    let foreign =
        JwtKeys::new("some-other-local-secret", TTL).issue(&admin_id, &UserType::ADMIN)?;
    assert_eq!(
        keys.verify(&foreign),
        Err(JwtError::Invalid),
        "a token of another secret"
    );
    let expired = keys.issue_at(&admin_id, &UserType::ADMIN, LONG_AGO)?;
    assert_eq!(
        keys.verify(&expired),
        Err(JwtError::Expired),
        "an expired token"
    );
    let tampered = splice(user, admin)?;
    assert_eq!(
        keys.verify(&tampered),
        Err(JwtError::Invalid),
        "a tampered token"
    );

    Ok(vec![
        ("no Authorization header", None),
        (
            "a bearer value that is not a JWT",
            Some("Bearer not-a-jwt".to_owned()),
        ),
        ("the Basic scheme", Some("Basic dXNlcjpwYXNz".to_owned())),
        ("a token of another secret", Some(bearer(&foreign))),
        ("an expired token", Some(bearer(&expired))),
        ("a tampered token", Some(bearer(&tampered))),
    ])
}

/// n5, the token gate: every bearer operation refuses every [`refused_authorizations`] value
/// with 401 and the sign-on text, while endpoint 1 needs no token and reads none it is sent.
#[test]
fn missing_or_bad_token_is_401() -> TestResult {
    block_on(async {
        let pg = start_loaded_postgres().await?;
        let api = api(&pg)?;
        // Endpoint 1 signs on without an `Authorization` header.
        // audit: COSGN00C-006
        let admin = signon_token(&api, "ADMIN001").await?;
        let user = signon_token(&api, "USER0001").await?;
        signon_ignores_a_bearer_header(&api).await?;

        let operations = bearer_operations(&api, &bearer(&admin)).await?;
        let refused = refused_authorizations(&admin, &user)?;
        for operation in &operations {
            for (label, authorization) in &refused {
                let (status, response) = operation.send(&api, authorization.as_deref()).await?;
                assert_eq!(status, 401, "{operation} with {label}");
                assert_eq!(
                    member(&response, "message"),
                    Some(SIGNON_REQUIRED),
                    "{operation} with {label}"
                );
                assert_eq!(
                    response.get("field"),
                    None,
                    "{operation} with {label}: no field"
                );
            }
        }
        Ok(())
    })
}

/// Sign-on decides from its own fields alone: an invalid bearer header does not stop it.
async fn signon_ignores_a_bearer_header(api: &Router) -> TestResult {
    let body = json!({ "user_id": "USER0001", "password": DUSRSECJ_PASSWORD });
    let (status, response) = call(
        api,
        Method::POST,
        SIGNON_URI,
        Some(&body),
        Some("Bearer not-a-jwt"),
    )
    .await?;
    // audit: COSGN00C-006
    assert_eq!(status, 200, "a bearer header stopped the sign-on");
    assert_eq!(member(&response, "user_type"), Some("U"));
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

/// The eight operations whose path segments reach their program's edit as raw text, each with
/// a segment that is not UTF-8 once percent-decoded (`%FF`); endpoints 22 and 23 once in each of
/// their two segments. The bodies of 6 and 10 would fail their own extractor, so only a path
/// rejection that comes first answers [`INVALID_PATH`].
fn undecodable_path_operations() -> Vec<Operation> {
    let gate = Gate::SignedOn;
    let body = json!({ "aid": "ENTER" });
    let detail = "/authorizations/{acct_id}/{auth_key}";
    let fraud = "/authorizations/{acct_id}/{auth_key}/fraud";
    vec![
        Operation::new(
            gate,
            4,
            Method::GET,
            "/accounts/{acct_id}/view",
            "/api/v1/accounts/%FF/view",
        ),
        Operation::new(
            gate,
            5,
            Method::GET,
            "/accounts/{acct_id}",
            "/api/v1/accounts/%FF",
        ),
        Operation::new(
            gate,
            6,
            Method::PUT,
            "/accounts/{acct_id}",
            "/api/v1/accounts/%FF",
        )
        .with_body(body.clone()),
        Operation::new(
            gate,
            10,
            Method::PUT,
            "/cards/{card_ref}",
            "/api/v1/cards/%FF",
        )
        .with_body(body),
        Operation::new(
            gate,
            12,
            Method::GET,
            "/transactions/{tran_id}",
            "/api/v1/transactions/%FF",
        ),
        Operation::new(
            gate,
            21,
            Method::GET,
            "/accounts/{acct_id}/authorizations",
            "/api/v1/accounts/%FF/authorizations",
        ),
        Operation::new(
            gate,
            22,
            Method::GET,
            detail,
            "/api/v1/authorizations/%FF/0778000000000001",
        ),
        Operation::new(
            gate,
            22,
            Method::GET,
            detail,
            "/api/v1/authorizations/00000000001/%FF",
        ),
        Operation::new(
            gate,
            23,
            Method::POST,
            fraud,
            "/api/v1/authorizations/%FF/0778000000000001/fraud",
        ),
        Operation::new(
            gate,
            23,
            Method::POST,
            fraud,
            "/api/v1/authorizations/00000000001/%FF/fraud",
        ),
    ]
}

/// Sends `operation` with `authorization` and returns the status, the `Content-Type` and the
/// body as text, so that a body that is not JSON is reported rather than failing the parse, once
/// [`check_challenge`] has passed the response's headers.
async fn send_untyped(
    api: &Router,
    operation: &Operation,
    authorization: &str,
) -> TestResult<(u16, String, String)> {
    let builder = Request::builder()
        .method(operation.method.clone())
        .uri(&operation.uri)
        .header(header::AUTHORIZATION, authorization);
    let request = match &operation.body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(json)?))?,
        None => builder.body(Body::empty())?,
    };
    let response = api.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    check_challenge(status, response.headers())?;
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .unwrap_or_default();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    Ok((
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    ))
}

/// A path segment that is not UTF-8 once percent-decoded is judged after the token: without a
/// valid token it is the gate's 401, and with one it is the JSON `ScreenError` of every other
/// input rejection, 400 [`INVALID_PATH`] with no field, before any program edit or read.
#[test]
fn undecodable_path_segments_are_a_json_400_after_the_gate() -> TestResult {
    block_on(async {
        let pg = start_loaded_postgres().await?;
        let api = api(&pg)?;
        let user = bearer(&signon_token(&api, "USER0001").await?);
        for operation in undecodable_path_operations() {
            let case = format!("{operation} at {}", operation.uri);
            let (status, response) = operation.send(&api, None).await?;
            assert_eq!(status, 401, "{case} without a token");
            assert_eq!(
                member(&response, "message"),
                Some(SIGNON_REQUIRED),
                "{case}"
            );

            let (status, content_type, body) = send_untyped(&api, &operation, &user).await?;
            assert_eq!(
                (status, content_type.as_str()),
                (400, "application/json"),
                "{case}: {body}"
            );
            assert_eq!(
                serde_json::from_str::<Value>(&body)?,
                json!({ "message": INVALID_PATH }),
                "{case}"
            );
        }
        Ok(())
    })
}

/// A token signed with [`SECRET`] for `ADMIN001`, type `A`, issued now and valid for [`TTL`]
/// seconds, whose `nbf` claim is `not_before`: a claim set sign-on never issues.
fn not_before_token(not_before: u64) -> TestResult<String> {
    let iat = jsonwebtoken::get_current_timestamp();
    let exp = iat.checked_add(TTL).ok_or("a token lifetime beyond u64")?;
    let claims = json!({
        "sub": "ADMIN001",
        "usr_type": "A",
        "iat": iat,
        "exp": exp,
        "nbf": not_before,
    });
    Ok(jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )?)
}

/// n5, the not-before time: a correctly signed `A` token whose `nbf` is an hour ahead is refused
/// by a `SignedOnUser` and an `AdminUser` operation with 401 and the sign-on text, and the same
/// claims with an `nbf` a minute ago are served.
#[test]
fn a_token_before_its_not_before_time_is_401() -> TestResult {
    block_on(async {
        let pg = start_loaded_postgres().await?;
        let api = api(&pg)?;
        let now = jsonwebtoken::get_current_timestamp();
        let ahead = now.checked_add(3_600).ok_or("an nbf beyond u64")?;
        let ago = now.checked_sub(60).ok_or("an nbf before the epoch")?;
        let not_yet_valid = bearer(&not_before_token(ahead)?);
        let valid = bearer(&not_before_token(ago)?);
        let uris = [
            // audit: COMEN01C-002
            "/api/v1/menus/main",
            // audit: COADM01C-001
            "/api/v1/menus/admin",
        ];
        for uri in uris {
            let (status, response) =
                call(&api, Method::GET, uri, None, Some(&not_yet_valid)).await?;
            assert_eq!(status, 401, "GET {uri} with an nbf an hour ahead");
            assert_eq!(
                member(&response, "message"),
                Some(SIGNON_REQUIRED),
                "GET {uri} with an nbf an hour ahead"
            );
            assert_eq!(
                response.get("field"),
                None,
                "GET {uri} with an nbf an hour ahead: no field"
            );
            let (status, _) = call(&api, Method::GET, uri, None, Some(&valid)).await?;
            assert_eq!(status, 200, "GET {uri} with an nbf a minute ago");
        }
        Ok(())
    })
}
