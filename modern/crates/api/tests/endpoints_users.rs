//! Binding g1b for the user-security programs (AAP 0.9.1): every `server` row, and the server
//! half of every `both` row, of `modern/audit/online/COUSR00C.md`, `COUSR01C.md`, `COUSR02C.md`
//! and `COUSR03C.md`, and the COUSR00 to COUSR03 `both` rows of `modern/audit/screens.md`, held
//! through endpoints 16 to 20 of the router `api::app::app` serves:
//!
//! | # | Operation | Program |
//! |---|---|---|
//! | 16 | `GET /api/v1/users` | COUSR00C |
//! | 17 | `POST /api/v1/users` | COUSR01C |
//! | 18 | `GET /api/v1/users/{user_id}` | COUSR02C and COUSR03C (the shared read) |
//! | 19 | `PUT /api/v1/users/{user_id}` | COUSR02C |
//! | 20 | `DELETE /api/v1/users/{user_id}` | COUSR03C |
//!
//! Each test function owns one `postgres:16` container from `test_support`, loaded with the
//! fixtures (`ADMIN001` to `ADMIN005` of type `A`, `USER0001` to `USER0005` of type `U`, every
//! password `PASSWORD`), or left empty and closed for the store failures. One loaded container
//! is served through a probed store (see "Rewrite and delete outcomes after the locked read").
//! Requests go through `tower::ServiceExt::oneshot`, so nothing listens on a port. The `A` token
//! is the one endpoint 1 issues to `ADMIN001`; every sign-on check also goes through endpoint 1.
//! Store effects are read back through the real `persistence::UserRepository`.
//!
//! # Rows without a parity value
//!
//! The rows that a halt candidate of the audits blocks still carry their tags, as
//! `modern/audit/README.md` requires: COUSR00C-025 (`X-COUSR00C-H1`), COUSR00C-027
//! (`X-COUSR00C-H2`), the filler of COUSR01C-017 (`X-COUSR01C-H1`), COUSR02C-025
//! (`X-COUSR02C-H1`) and COUSR03C-018 (`X-COUSR03C-H1`). Their tests exercise the path, assert
//! only the store effect the row leaves assertable, and print the case as `BLOCKED`. No halt of
//! `modern/audit/HALTS.md` names any of these rows, so no test here is ignored.
//!
//! # Rewrite and delete outcomes after the locked read
//!
//! COUSR02C-031, COUSR02C-032, COUSR03C-021 and COUSR03C-022 are a rewrite or a delete that
//! affects no row or fails after the locked read has found the row. The lock rules the first
//! out and a real store gives no ordinary trigger for the second, so
//! `cousr02c_cousr03c_write_outcome_rows` proves them through endpoints 19 and 20 over the real
//! store of a loaded container with a `ScriptedProbe` installed (`PgStore::with_probe`), which
//! the transactions the store opens answer to as well:
//!
//! - Every read runs for real: the sign-on's, endpoint 19's unlocked read, and the locked
//!   `FOR UPDATE` read that finds the row in the endpoint's transaction. No read is a probe
//!   point.
//! - The probe answers only the statement that follows the lock, `UserRepository::update` or
//!   `UserRepository::delete`: `NoRow` skips it as a statement that matched no row, and `Fail`
//!   as one that failed with SQLSTATE `57014`. The SQLSTATE stands for the CICS codes, which
//!   are never asserted (halt H4).
//! - Each case asserts the endpoint's status, message and `field`, that the probed statement was
//!   reached exactly once with the entry or key the service built, and that the `users` table is
//!   unchanged afterwards. A last request that the probe lets proceed then succeeds, so each
//!   refusal came from the probed statement alone and its transaction ended.
//!
//! # Redaction
//!
//! Every response of endpoints 16 to 20 is checked for a JSON key naming a password or a hash,
//! for an argon2 PHC prefix and for each password this file submits (Cardholder Data
//! Redaction). No assertion message quotes a password.

use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use api::app::{AppState, app};
use api::dto::FIELD_LENGTH_MESSAGE;
use api::dto::ScreenResult;
use api::dto::cosgn00c::SignonResponse;
use api::dto::cousr00c::UserListResponse;
use api::dto::cousr02c::UserView;
use api::error::{ScreenError, UNABLE_TO_PROCESS};
use axum::body::{self, Body};
use axum::http::{Method, Request, StatusCode, header};
use chrono::{NaiveDate, NaiveDateTime};
use domain::FixedClock;
use domain::copybook::PicX;
use domain::online::cosgn00c::UserType;
use domain::password;
use domain::records::{UserEntry, UserListEntry};
use persistence::{
    PersistenceError, PgStore, ProbeCall, ProbePoint, ProbeVerdict, RepositoryProbe, UserRepository,
};
use runtime::config::Config;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use test_support::fixtures::start_loaded_postgres;
use test_support::postgres::{PostgresGuard, start_postgres};
use tower::ServiceExt as _;

type TestResult = Result<(), Box<dyn Error>>;

/// Runs `test` to completion on a current-thread runtime, the one Tokio's test attribute
/// builds, without the lint attributes that attribute's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

/// A local-only signing secret for the tokens of these tests.
const JWT_SECRET: &str = "endpoints-users-local-test-secret";

/// The password of every fixture user (`DUSRSECJ.jcl:35-44`, `README.md:203-204`).
const FIXTURE_PASSWORD: &str = "PASSWORD";
/// Passwords this file submits through endpoints 1, 17 and 19.
const UPPER_PASSWORD: &str = "UPPERPWD";
const LOWER_PASSWORD: &str = "lowercas";
const OTHER_PASSWORD: &str = "OTHERPW1";
const NEW_PASSWORD: &str = "NEWPASS1";
const NEW_LOWER_PASSWORD: &str = "newpass2";
/// [`UPPER_PASSWORD`] as typed in lower case at sign-on.
const UPPER_PASSWORD_LOWERED: &str = "upperpwd";

/// Every password above: no response of endpoints 16 to 20 may contain one.
const SUBMITTED_PASSWORDS: [&str; 7] = [
    FIXTURE_PASSWORD,
    UPPER_PASSWORD,
    LOWER_PASSWORD,
    OTHER_PASSWORD,
    NEW_PASSWORD,
    NEW_LOWER_PASSWORD,
    UPPER_PASSWORD_LOWERED,
];

// The audited literals, copied from the audit rows rather than from the domain constants.
const AT_TOP: &str = "You are already at the top of the page...";
const AT_BOTTOM: &str = "You are already at the bottom of the page...";
const REACHED_BOTTOM: &str = "You have reached the bottom of the page...";
const REACHED_TOP: &str = "You have reached the top of the page...";
const LOOKUP_FAILED: &str = "Unable to lookup User...";
const FIRST_NAME_EMPTY: &str = "First Name can NOT be empty...";
const LAST_NAME_EMPTY: &str = "Last Name can NOT be empty...";
const USER_ID_EMPTY: &str = "User ID can NOT be empty...";
const PASSWORD_EMPTY: &str = "Password can NOT be empty...";
const USER_TYPE_EMPTY: &str = "User Type can NOT be empty...";
const ALREADY_EXISTS: &str = "User ID already exist...";
const ADD_FAILED: &str = "Unable to Add User...";
const UPDATE_FAILED: &str = "Unable to Update User...";
const NOT_FOUND: &str = "User ID NOT found...";
const NOT_MODIFIED: &str = "Please modify to update ...";
const UPDATE_PROMPT: &str = "Press PF5 key to save your updates ...";
const DELETE_PROMPT: &str = "Press PF5 key to delete this user ...";
const WRONG_PASSWORD: &str = "Wrong Password. Try again ...";
const SIGNON_NOT_FOUND: &str = "User not found. Try again ...";

/// The ten fixture users in key order: user ID, first name, last name, type.
const FIXTURE_USERS: [(&str, &str, &str, &str); 10] = [
    ("ADMIN001", "MARGARET", "GOLD", "A"),
    ("ADMIN002", "RUSSELL", "RUSSELL", "A"),
    ("ADMIN003", "RAYMOND", "WHITMORE", "A"),
    ("ADMIN004", "EMMANUEL", "CASGRAIN", "A"),
    ("ADMIN005", "GRANVILLE", "LACHAPELLE", "A"),
    ("USER0001", "LAWRENCE", "THOMAS", "U"),
    ("USER0002", "AJITH", "KUMAR", "U"),
    ("USER0003", "LAURITZ", "ALME", "U"),
    ("USER0004", "AVERARDO", "MAZZI", "U"),
    ("USER0005", "LEE", "TING", "U"),
];

/// One answer of the API: its status and its JSON body.
struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    /// The body as `T`.
    fn parse<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.body.clone())
    }
}

/// Asserts that `reply` is the `ScreenError` `message` on `field` with `status`.
fn expect_error(
    reply: &Reply,
    status: StatusCode,
    message: &str,
    field: Option<&str>,
) -> TestResult {
    let error: ScreenError = reply.parse()?;
    assert_eq!(
        (reply.status, error.message(), error.field()),
        (status, message, field)
    );
    Ok(())
}

/// Asserts that `reply` is a 200 `ScreenResult` whose body is `message` and nothing else.
fn expect_message(reply: &Reply, message: &str) -> TestResult {
    let result: ScreenResult = reply.parse()?;
    assert_eq!(
        (reply.status, result.message.as_str()),
        (StatusCode::OK, message)
    );
    assert_eq!(reply.body, json!({ "message": message }));
    Ok(())
}

/// Every object key anywhere in `value`.
fn keys_of(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, inner) in object {
                keys.push(key.clone());
                keys_of(inner, keys);
            }
        }
        Value::Array(items) => {
            for item in items {
                keys_of(item, keys);
            }
        }
        _ => {}
    }
}

/// Fails when a response of endpoints 16 to 20 carries a password key, a hash or a password
/// this file submitted. The message names the route only, never the value found.
fn check_redacted(route: &str, body: &Value) -> TestResult {
    let mut keys = Vec::new();
    keys_of(body, &mut keys);
    if keys
        .iter()
        .any(|key| key.contains("password") || key.contains("hash") || key.contains("pwd"))
    {
        return Err(format!("a response of {route} has a password or hash key").into());
    }
    let text = body.to_string();
    if text.contains("$argon2") {
        return Err(format!("a response of {route} carries a password hash").into());
    }
    if SUBMITTED_PASSWORDS
        .iter()
        .any(|password| text.contains(password))
    {
        return Err(format!("a response of {route} carries a submitted password").into());
    }
    Ok(())
}

/// A user ID as a path or query segment: spaces percent-encoded, every other byte as typed.
fn segment(user_id: &str) -> String {
    user_id.replace(' ', "%20")
}

/// The key a user ID is stored under: the text left-justified in `PIC X(08)`.
fn key(user_id: &str) -> Result<PicX<8>, Box<dyn Error>> {
    Ok(PicX::from_text(user_id)?)
}

/// The fixed instant of the injected clock, 2022-07-18 10:30:00.000.
fn instant() -> Result<NaiveDateTime, Box<dyn Error>> {
    Ok(NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or("an invalid test instant")?)
}

/// The API over one container: the state the router serves, and the guard that removes the
/// container once the state, declared first, has dropped its pool handle.
struct Api {
    state: AppState,
    _db: PostgresGuard,
}

impl Api {
    /// The API over a fixture-loaded database.
    async fn loaded() -> Result<Self, Box<dyn Error>> {
        Self::over(start_loaded_postgres().await?)
    }

    /// The API over an empty database whose pool is closed, so every store call fails.
    async fn closed() -> Result<Self, Box<dyn Error>> {
        let db = start_postgres().await?;
        db.pool().close().await;
        Self::over(db)
    }

    /// The API over a fixture-loaded database whose store, and every transaction it opens,
    /// asks `probe` at each probe point (see "Rewrite and delete outcomes after the locked
    /// read").
    async fn probed(probe: Arc<dyn RepositoryProbe>) -> Result<Self, Box<dyn Error>> {
        let db = start_loaded_postgres().await?;
        let store = PgStore::new(db.pool().clone()).with_probe(probe);
        Self::serving(store, db)
    }

    /// The state `carddemo-api` builds from its configuration, over `db`.
    fn over(db: PostgresGuard) -> Result<Self, Box<dyn Error>> {
        Self::serving(PgStore::new(db.pool().clone()), db)
    }

    /// The state `carddemo-api` builds from its configuration, over `store`, a store on the
    /// pool of `db`, with the fixed clock and a job-deck directory of the test target's own.
    fn serving(store: PgStore, db: PostgresGuard) -> Result<Self, Box<dyn Error>> {
        let config = Config {
            jwt_secret: JWT_SECRET.to_owned(),
            intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints_users_jobs"),
            ..Config::default()
        };
        let state = AppState::from_config(store, &config, Arc::new(FixedClock::new(instant()?)))?;
        Ok(Self { state, _db: db })
    }

    /// `method uri` with `token` as bearer token and `body` as JSON, through the router. A
    /// response of endpoints 16 to 20 is checked for redaction before it is returned.
    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<Reply, Box<dyn Error>> {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))?,
            None => request.body(Body::empty())?,
        };
        let response = app(self.state.clone()).oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        let body: Value = serde_json::from_slice(&bytes)?;
        if uri.starts_with("/api/v1/users") {
            check_redacted(uri, &body)?;
        }
        Ok(Reply { status, body })
    }

    /// Endpoint 1 for `user_id` and `password`.
    async fn sign_on(&self, user_id: &str, password: &str) -> Result<Reply, Box<dyn Error>> {
        let body = json!({ "user_id": user_id, "password": password });
        self.send(Method::POST, "/api/v1/signon", None, Some(&body))
            .await
    }

    /// The token endpoint 1 issues to `user_id`, which must sign on.
    async fn token(&self, user_id: &str, password: &str) -> Result<String, Box<dyn Error>> {
        let reply = self.sign_on(user_id, password).await?;
        assert_eq!(reply.status, StatusCode::OK, "sign-on of {user_id}");
        Ok(reply.parse::<SignonResponse>()?.token)
    }

    /// Endpoint 16 with the query string `query`, empty or starting with `?`.
    async fn list(&self, token: Option<&str>, query: &str) -> Result<Reply, Box<dyn Error>> {
        self.send(Method::GET, &format!("/api/v1/users{query}"), token, None)
            .await
    }

    /// Endpoint 16 answered with a page or a message: the `UserListResponse`.
    async fn page(&self, token: &str, query: &str) -> Result<UserListResponse, Box<dyn Error>> {
        let reply = self.list(Some(token), query).await?;
        assert_eq!(reply.status, StatusCode::OK, "GET /api/v1/users{query}");
        Ok(reply.parse()?)
    }

    /// Endpoint 17 with `body`.
    async fn add(&self, token: Option<&str>, body: &Value) -> Result<Reply, Box<dyn Error>> {
        self.send(Method::POST, "/api/v1/users", token, Some(body))
            .await
    }

    /// Endpoint 17 for `user_id` with names `ADDED USER`, `password` and `user_type`, which must
    /// be answered 200.
    async fn add_user(
        &self,
        token: &str,
        user_id: &str,
        password: &str,
        user_type: &str,
    ) -> TestResult {
        let body = json!({
            "first_name": "ADDED",
            "last_name": "USER",
            "user_id": user_id,
            "password": password,
            "user_type": user_type,
        });
        let reply = self.add(Some(token), &body).await?;
        assert_eq!(reply.status, StatusCode::OK, "add of {user_id:?}");
        Ok(())
    }

    /// Endpoint 18 for `user_id`.
    async fn fetch(&self, token: Option<&str>, user_id: &str) -> Result<Reply, Box<dyn Error>> {
        let uri = format!("/api/v1/users/{}", segment(user_id));
        self.send(Method::GET, &uri, token, None).await
    }

    /// Endpoint 19 for `user_id` with `body`.
    async fn update(
        &self,
        token: Option<&str>,
        user_id: &str,
        body: &Value,
    ) -> Result<Reply, Box<dyn Error>> {
        let uri = format!("/api/v1/users/{}", segment(user_id));
        self.send(Method::PUT, &uri, token, Some(body)).await
    }

    /// Endpoint 20 for `user_id`.
    async fn delete(&self, token: Option<&str>, user_id: &str) -> Result<Reply, Box<dyn Error>> {
        let uri = format!("/api/v1/users/{}", segment(user_id));
        self.send(Method::DELETE, &uri, token, None).await
    }

    /// The stored row of `user_id`, read back through `UserRepository`.
    async fn user(&self, user_id: &str) -> Result<Option<UserEntry>, Box<dyn Error>> {
        let mut store = self.state.store.clone();
        Ok(UserRepository::find_by_id(&mut store, &key(user_id)?).await?)
    }

    /// Every stored user in key order, without hashes: the table as COUSR00C lists it.
    async fn users(&self) -> Result<Vec<UserListEntry>, Box<dyn Error>> {
        let mut store = self.state.store.clone();
        Ok(UserRepository::page_after(&mut store, &PicX::spaces(), 100).await?)
    }
}

/// The key an endpoint 16 request stands for.
#[derive(Clone, Copy)]
enum Aid<'a> {
    /// ENTER, with the search field when it is not blank.
    Enter(Option<&'a str>),
    /// F7, the page before.
    F7,
    /// F8, the page after.
    F8,
}

/// The query string the screen sends for `aid` after the response `after`: the paging state
/// that response returned, echoed unchanged (COUSR00C-006), with the first user ID as the F7
/// key and the last as the F8 key, and `page_size` when the test sets one.
fn list_query(aid: Aid<'_>, after: Option<&UserListResponse>, page_size: Option<u8>) -> String {
    let mut pairs = Vec::new();
    if let Aid::Enter(Some(start)) = aid {
        pairs.push(format!("start_user_id={}", segment(start)));
    }
    if let Some(page) = after {
        if let Some(first) = &page.first_key {
            pairs.push(format!("first_key={}", segment(first)));
        }
        if let Some(last) = &page.last_key {
            pairs.push(format!("last_key={}", segment(last)));
        }
        pairs.push(format!("page_no={}", page.page_no));
        pairs.push(format!("has_next_page={}", page.has_next_page));
        let page_key = match aid {
            Aid::Enter(_) => None,
            Aid::F7 => page.first_key.as_deref(),
            Aid::F8 => page.last_key.as_deref(),
        };
        if let Some(page_key) = page_key {
            pairs.push(format!("page_key={}", segment(page_key)));
        }
    }
    match aid {
        Aid::Enter(_) => {}
        Aid::F7 => pairs.push("direction=backward".to_owned()),
        Aid::F8 => pairs.push("direction=forward".to_owned()),
    }
    if let Some(size) = page_size {
        pairs.push(format!("page_size={size}"));
    }
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

/// The user ID of each row position of `page`, `None` for a blank row; `None` as a whole when
/// the response keeps the rows the screen shows.
fn row_ids(page: &UserListResponse) -> Option<Vec<Option<&str>>> {
    page.rows.as_ref().map(|rows| {
        rows.iter()
            .map(|row| row.as_ref().map(|row| row.user_id.as_str()))
            .collect()
    })
}

/// Row positions holding `leading` blanks, then `ids`, then `trailing` blanks, as [`row_ids`]
/// gives them for a page that replaces the rows.
fn positions<'a>(leading: usize, ids: &[&'a str], trailing: usize) -> Vec<Option<&'a str>> {
    let mut rows = vec![None; leading];
    rows.extend(ids.iter().map(|&id| Some(id)));
    rows.extend(std::iter::repeat_n(None, trailing));
    rows
}

/// The paging state of `page`: page number, first and last user IDs, next-page indicator.
fn state_of(page: &UserListResponse) -> (u64, Option<&str>, Option<&str>, bool) {
    (
        page.page_no,
        page.first_key.as_deref(),
        page.last_key.as_deref(),
        page.has_next_page,
    )
}

/// The ten fixture user IDs in key order.
fn fixture_ids() -> Vec<&'static str> {
    FIXTURE_USERS.iter().map(|&(id, ..)| id).collect()
}

/// The fixture user IDs `USER0001` to `USER0005`.
const REGULAR_IDS: [&str; 5] = ["USER0001", "USER0002", "USER0003", "USER0004", "USER0005"];

/// Endpoint 16, COUSR00C: the gate, the first entry, ENTER with a search value, F7 and F8 over
/// one page and over two, short pages, and the browses a halt candidate blocks.
#[test]
fn cousr00c_list_rows() -> TestResult {
    block_on(async {
        let api = Api::loaded().await?;
        let admin = api.token("ADMIN001", FIXTURE_PASSWORD).await?;
        list_gate(&api).await?;
        let first = list_first_entry(&api, &admin).await?;
        list_on_the_only_page(&api, &admin, &first).await?;
        list_from_a_search_value(&api, &admin, &first).await?;
        // An eleventh user, so that the list has a second page.
        api.add_user(&admin, "USER0006", UPPER_PASSWORD, "U")
            .await?;
        list_two_pages(&api, &admin).await?;
        list_short_pages(&api, &admin).await?;
        list_blocked(&api, &admin).await
    })
}

/// COUSR00C-001: no token and an invalid one are 401, a `U` token 403.
async fn list_gate(api: &Api) -> TestResult {
    // audit: COUSR00C-001
    assert_eq!(api.list(None, "").await?.status, StatusCode::UNAUTHORIZED);
    let invalid = api.list(Some("not-a-token"), "").await?;
    // audit: COUSR00C-001
    assert_eq!(invalid.status, StatusCode::UNAUTHORIZED);
    let user = api.token("USER0001", FIXTURE_PASSWORD).await?;
    // audit: COUSR00C-001
    assert_eq!(
        api.list(Some(&user), "").await?.status,
        StatusCode::FORBIDDEN
    );
    Ok(())
}

/// The first entry, ENTER with a blank search field: page 1 of the ten fixture users, the
/// bottom message and the paging state (COUSR00C-012 to COUSR00C-016, COUSR00C-028).
async fn list_first_entry(api: &Api, admin: &str) -> Result<UserListResponse, Box<dyn Error>> {
    let reply = api.list(Some(admin), "").await?;
    // audit: COUSR00C-001
    assert_eq!(reply.status, StatusCode::OK);
    let first: UserListResponse = reply.parse()?;
    // audit: COUSR00C-012
    // audit: COUSR00C-013
    assert_eq!(row_ids(&first), Some(positions(0, &fixture_ids(), 0)));
    // audit: COUSR00C-014
    assert_eq!(first.message.as_deref(), Some(REACHED_BOTTOM));
    // audit: COUSR00C-014
    // audit: COUSR00C-016
    assert_eq!(
        state_of(&first),
        (1, Some("ADMIN001"), Some("USER0005"), false)
    );
    let rows = reply.body["rows"]
        .as_array()
        .ok_or("the page has no rows")?;
    for (row, &(user_id, first_name, last_name, user_type)) in rows.iter().zip(&FIXTURE_USERS) {
        // audit: COUSR00C-028
        assert_eq!(
            row,
            &json!({
                "user_id": user_id,
                "first_name": first_name,
                "last_name": last_name,
                "user_type": user_type,
            })
        );
    }
    Ok(first)
}

/// F7 and F8 over the only page of ten users: the top message with the next-page indicator
/// set, then ten blank rows, then the already-at-bottom message (COUSR00C-006,
/// COUSR00C-018 to COUSR00C-021).
async fn list_on_the_only_page(api: &Api, admin: &str, first: &UserListResponse) -> TestResult {
    let top = api
        .page(admin, &list_query(Aid::F7, Some(first), None))
        .await?;
    // audit: COUSR00C-021
    assert_eq!(
        (row_ids(&top), top.message.as_deref()),
        (None, Some(AT_TOP))
    );
    // audit: COUSR00C-006
    // audit: COUSR00C-021
    assert_eq!(
        state_of(&top),
        (1, Some("ADMIN001"), Some("USER0005"), true)
    );

    let bottom = api
        .page(admin, &list_query(Aid::F8, Some(&top), None))
        .await?;
    // audit: COUSR00C-018
    // audit: COUSR00C-019
    assert_eq!(row_ids(&bottom), Some(positions(10, &[], 0)));
    // audit: COUSR00C-018
    // audit: COUSR00C-019
    assert_eq!(bottom.message.as_deref(), Some(REACHED_BOTTOM));
    // audit: COUSR00C-018
    assert_eq!(
        state_of(&bottom),
        (1, Some("ADMIN001"), Some("USER0005"), false)
    );

    let again = api
        .page(admin, &list_query(Aid::F8, Some(&bottom), None))
        .await?;
    // audit: COUSR00C-020
    assert_eq!(
        (row_ids(&again), again.message.as_deref()),
        (None, Some(AT_BOTTOM))
    );
    // audit: COUSR00C-020
    assert_eq!(state_of(&again), state_of(&bottom));
    Ok(())
}

/// ENTER with a search value: it positions the list without an edit or an existence check,
/// and a value longer than the map's 8 characters is refused (COUSR00C-012, COUSR00C-015,
/// COUSR00C-016, COUSR00-015).
async fn list_from_a_search_value(api: &Api, admin: &str, first: &UserListResponse) -> TestResult {
    let query = list_query(Aid::Enter(Some("USER0001")), Some(first), None);
    let search = api.page(admin, &query).await?;
    // audit: COUSR00C-012
    // audit: COUSR00C-015
    assert_eq!(row_ids(&search), Some(positions(0, &REGULAR_IDS, 5)));
    // audit: COUSR00C-015
    assert_eq!(search.message.as_deref(), Some(REACHED_BOTTOM));
    // The last user ID is the one the request carried, since row 10 stayed blank.
    // audit: COUSR00C-016
    assert_eq!(
        state_of(&search),
        (1, Some("USER0001"), Some("USER0005"), false)
    );

    let positioned = api
        .page(admin, &list_query(Aid::Enter(Some("B")), None, None))
        .await?;
    // audit: COUSR00C-012
    assert_eq!(row_ids(&positioned), Some(positions(0, &REGULAR_IDS, 5)));
    // audit: COUSR00C-012
    assert_eq!(positioned.page_no, 1);

    let too_long = api.list(Some(admin), "?start_user_id=USER00010").await?;
    // audit: COUSR00-015
    expect_error(
        &too_long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("start_user_id"),
    )
}

/// F8 to a second page and F7 back, with eleven users and the screen's ten rows
/// (COUSR00C-014 to COUSR00C-017, COUSR00C-022, COUSR00C-023).
async fn list_two_pages(api: &Api, admin: &str) -> TestResult {
    let first = api.page(admin, "").await?;
    // A full page with a user after it: next-page indicator set and no message.
    // audit: COUSR00C-014
    assert_eq!(
        (row_ids(&first), first.message.as_deref()),
        (Some(positions(0, &fixture_ids(), 0)), None)
    );
    // audit: COUSR00C-014
    assert_eq!(
        state_of(&first),
        (1, Some("ADMIN001"), Some("USER0005"), true)
    );

    let second = api
        .page(admin, &list_query(Aid::F8, Some(&first), None))
        .await?;
    // audit: COUSR00C-017
    assert_eq!(row_ids(&second), Some(positions(0, &["USER0006"], 9)));
    // audit: COUSR00C-015
    assert_eq!(second.message.as_deref(), Some(REACHED_BOTTOM));
    // audit: COUSR00C-016
    // audit: COUSR00C-017
    assert_eq!(
        state_of(&second),
        (2, Some("USER0006"), Some("USER0005"), false)
    );

    let back = api
        .page(admin, &list_query(Aid::F7, Some(&second), None))
        .await?;
    // audit: COUSR00C-022
    assert_eq!(row_ids(&back), Some(positions(0, &fixture_ids(), 0)));
    // audit: COUSR00C-023
    assert_eq!(back.message.as_deref(), Some(REACHED_TOP));
    // audit: COUSR00C-022
    // audit: COUSR00C-023
    assert_eq!(
        state_of(&back),
        (1, Some("ADMIN001"), Some("USER0005"), true)
    );
    Ok(())
}

/// Pages of five rows over eleven users, where F7 finds a user before the page it fills
/// (COUSR00C-023), and a backward page that reaches the first user before filling its rows
/// (COUSR00C-024). The page size is a Bounded Pagination member the screen never sends.
async fn list_short_pages(api: &Api, admin: &str) -> TestResult {
    let size = Some(5);
    let one = api
        .page(admin, &list_query(Aid::Enter(None), None, size))
        .await?;
    let two = api
        .page(admin, &list_query(Aid::F8, Some(&one), size))
        .await?;
    // audit: COUSR00C-017
    assert_eq!(
        (row_ids(&two), state_of(&two)),
        (
            Some(positions(0, &REGULAR_IDS, 0)),
            (2, Some("USER0001"), Some("USER0005"), true)
        )
    );
    let three = api
        .page(admin, &list_query(Aid::F8, Some(&two), size))
        .await?;
    // audit: COUSR00C-015
    assert_eq!(
        (row_ids(&three), three.message.as_deref()),
        (Some(positions(0, &["USER0006"], 4)), Some(REACHED_BOTTOM))
    );

    let back = api
        .page(admin, &list_query(Aid::F7, Some(&three), size))
        .await?;
    // audit: COUSR00C-022
    assert_eq!(row_ids(&back), Some(positions(0, &REGULAR_IDS, 0)));
    // A user precedes the page, so the page number goes down by one and no message is set.
    // audit: COUSR00C-023
    assert_eq!((back.page_no, back.message.as_deref()), (2, None));

    // The state of a third page whose row 1 is ADMIN004, echoed as the screen sends it: only
    // three users precede it, so rows 8 to 10 are filled and the rest stay blank.
    let short = api
        .page(
            admin,
            "?direction=backward&page_key=ADMIN004&first_key=ADMIN004&last_key=USER0003\
             &page_no=3&has_next_page=true",
        )
        .await?;
    // audit: COUSR00C-024
    assert_eq!(
        row_ids(&short),
        Some(positions(7, &["ADMIN001", "ADMIN002", "ADMIN003"], 0))
    );
    // audit: COUSR00C-024
    assert_eq!(short.message.as_deref(), Some(REACHED_TOP));
    // The page number and the first user ID keep their values, as row 1 was not filled.
    // audit: COUSR00C-024
    assert_eq!(
        state_of(&short),
        (3, Some("ADMIN004"), Some("ADMIN003"), true)
    );
    Ok(())
}

/// COUSR00C-025 and COUSR00C-027: browses whose result halt candidates `X-COUSR00C-H1` and
/// `X-COUSR00C-H2` block. Each is requested, its response passes the redaction check, and no
/// parity value is asserted for it; the list writes nothing.
async fn list_blocked(api: &Api, admin: &str) -> TestResult {
    let before = api.users().await?;
    // ENTER with a search value above every user ID.
    // audit: COUSR00C-025
    api.list(Some(admin), "?start_user_id=ZZZZZZZZ").await?;
    println!("BLOCKED COUSR00C-025 (X-COUSR00C-H1): no parity value asserted");
    // F8 with the next-page indicator that F7 set and a blank last user ID.
    // audit: COUSR00C-027
    api.list(
        Some(admin),
        "?direction=forward&first_key=USER0001&page_no=1&has_next_page=true",
    )
    .await?;
    println!("BLOCKED COUSR00C-027 (X-COUSR00C-H2): no parity value asserted");
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// A body of endpoint 17 that every edit admits, for user `user_id`.
fn add_body(user_id: &str) -> Value {
    json!({
        "first_name": "ALEX",
        "last_name": "MORGAN",
        "user_id": user_id,
        "password": UPPER_PASSWORD,
        "user_type": "U",
    })
}

/// [`add_body`] for `NEWUSR01` with `member` set to `value`.
fn add_body_with(member: &str, value: &str) -> Value {
    let mut body = add_body("NEWUSR01");
    body[member] = json!(value);
    body
}

/// Endpoint 17, COUSR01C: the gate, the empty-field edits in order, the map lengths, the row
/// stored as typed with the hash of its password, the sign-on consequences of storing as
/// typed (E26), the success message and the duplicate.
#[test]
fn cousr01c_add_rows() -> TestResult {
    block_on(async {
        let api = Api::loaded().await?;
        let admin = api.token("ADMIN001", FIXTURE_PASSWORD).await?;
        add_gate(&api).await?;
        add_empty_fields(&api, &admin).await?;
        add_map_lengths(&api, &admin).await?;
        add_stores_the_row_as_typed(&api, &admin).await?;
        add_any_user_type(&api, &admin).await?;
        add_then_sign_on(&api, &admin).await?;
        add_messages_and_duplicates(&api, &admin).await
    })
}

/// COUSR01C-001 and COUSR01C-002: 401 without a valid token and 403 for a `U` token, before
/// any edit, with no row written.
async fn add_gate(api: &Api) -> TestResult {
    let before = api.users().await?;
    let body = add_body("NEWUSR01");
    // audit: COUSR01C-002
    assert_eq!(api.add(None, &body).await?.status, StatusCode::UNAUTHORIZED);
    let invalid = api.add(Some("not-a-token"), &body).await?;
    // audit: COUSR01C-002
    assert_eq!(invalid.status, StatusCode::UNAUTHORIZED);
    let user = api.token("USER0001", FIXTURE_PASSWORD).await?;
    // audit: COUSR01C-001
    assert_eq!(
        api.add(Some(&user), &body).await?.status,
        StatusCode::FORBIDDEN
    );
    // The gate answers before the edits: an empty body is refused with 403 too.
    // audit: COUSR01C-001
    assert_eq!(
        api.add(Some(&user), &json!({})).await?.status,
        StatusCode::FORBIDDEN
    );
    // audit: COUSR01C-001
    // audit: COUSR01C-002
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// COUSR01C-010 to COUSR01C-014: each empty field in `EVALUATE` order, the first empty one
/// answering whatever the later fields hold; absent, empty and all-space values alike.
async fn add_empty_fields(api: &Api, admin: &str) -> TestResult {
    let before = api.users().await?;
    let all_empty = api.add(Some(admin), &json!({})).await?;
    // audit: COUSR01C-010
    expect_error(
        &all_empty,
        StatusCode::BAD_REQUEST,
        FIRST_NAME_EMPTY,
        Some("first_name"),
    )?;
    let spaces = api
        .add(Some(admin), &add_body_with("first_name", "    "))
        .await?;
    // audit: COUSR01C-010
    expect_error(
        &spaces,
        StatusCode::BAD_REQUEST,
        FIRST_NAME_EMPTY,
        Some("first_name"),
    )?;

    let last = api
        .add(Some(admin), &json!({ "first_name": "ALEX" }))
        .await?;
    // audit: COUSR01C-011
    expect_error(
        &last,
        StatusCode::BAD_REQUEST,
        LAST_NAME_EMPTY,
        Some("last_name"),
    )?;

    let body = json!({ "first_name": "ALEX", "last_name": "MORGAN", "user_id": "  " });
    let user_id = api.add(Some(admin), &body).await?;
    // audit: COUSR01C-012
    expect_error(
        &user_id,
        StatusCode::BAD_REQUEST,
        USER_ID_EMPTY,
        Some("user_id"),
    )?;

    let mut body = add_body("NEWUSR01");
    body["password"] = json!("");
    body["user_type"] = json!("");
    let password = api.add(Some(admin), &body).await?;
    // audit: COUSR01C-013
    expect_error(
        &password,
        StatusCode::BAD_REQUEST,
        PASSWORD_EMPTY,
        Some("password"),
    )?;

    let user_type = api
        .add(Some(admin), &add_body_with("user_type", " "))
        .await?;
    // audit: COUSR01C-014
    expect_error(
        &user_type,
        StatusCode::BAD_REQUEST,
        USER_TYPE_EMPTY,
        Some("user_type"),
    )?;

    // audit: COUSR01C-010
    // audit: COUSR01C-014
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// The server half of the `LENGTH` `both` rows of COUSR01: a value one byte beyond its map
/// field is a 400 naming the field, and nothing is written.
async fn add_map_lengths(api: &Api, admin: &str) -> TestResult {
    let before = api.users().await?;
    let password = format!("{UPPER_PASSWORD}9");
    let cases = [
        ("first_name", "F".repeat(21)),
        ("last_name", "L".repeat(21)),
        ("user_id", "NEWUSR019".to_owned()),
        ("password", password),
        ("user_type", "UA".to_owned()),
    ];
    for (member, value) in &cases {
        let reply = api.add(Some(admin), &add_body_with(member, value)).await?;
        // audit: COUSR01-013
        // audit: COUSR01-016
        // audit: COUSR01-019
        // audit: COUSR01-022
        // audit: COUSR01-025
        expect_error(
            &reply,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            Some(member),
        )?;
    }
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// A successful add: the message, the row as typed with no trimming or case change, the hash
/// of the 8 password bytes as typed, nothing else changed, and the caller's identity untouched
/// (COUSR01C-016 to COUSR01C-018, COUSR01C-021, COUSR01C-026).
async fn add_stores_the_row_as_typed(api: &Api, admin: &str) -> TestResult {
    let before = api.users().await?;
    let body = json!({
        "first_name": " alex",
        "last_name": "Morgan",
        "user_id": "NEWUSR01",
        "password": UPPER_PASSWORD,
        "user_type": "A",
    });
    let reply = api.add(Some(admin), &body).await?;
    // audit: COUSR01C-021
    expect_message(&reply, "User NEWUSR01 has been added ...")?;
    // The new user is an admin, yet the response carries no token and the caller's token still
    // passes the gate as the caller.
    // audit: COUSR01C-026
    assert_eq!(api.list(Some(admin), "").await?.status, StatusCode::OK);

    let entry = api
        .user("NEWUSR01")
        .await?
        .ok_or("the added user is not stored")?;
    // audit: COUSR01C-016
    // audit: COUSR01C-017
    assert_eq!(
        (&entry.id, &entry.fname, &entry.lname, &entry.usr_type),
        (
            &key("NEWUSR01")?,
            &PicX::<20>::from_text(" alex")?,
            &PicX::<20>::from_text("Morgan")?,
            &PicX::<1>::from_text("A")?
        )
    );
    println!("BLOCKED COUSR01C-017 (X-COUSR01C-H1): the filler value is not asserted");
    // audit: COUSR01C-018
    assert!(entry.password_hash.starts_with("$argon2id$"));
    // audit: COUSR01C-018
    assert!(
        password::verify(b"UPPERPWD", &entry.password_hash)?,
        "the stored hash does not verify the password as typed"
    );

    let added = key("NEWUSR01")?;
    let others: Vec<UserListEntry> = api
        .users()
        .await?
        .into_iter()
        .filter(|user| user.id != added)
        .collect();
    // audit: COUSR01C-016
    assert_eq!(others, before);
    Ok(())
}

/// COUSR01C-015: any non-blank user type is stored as typed; only `A` is an admin.
async fn add_any_user_type(api: &Api, admin: &str) -> TestResult {
    api.add_user(admin, "TYPEA001", UPPER_PASSWORD, "a").await?;
    api.add_user(admin, "TYPEX001", UPPER_PASSWORD, "X").await?;
    for (user_id, user_type) in [("TYPEA001", "a"), ("TYPEX001", "X")] {
        let entry = api
            .user(user_id)
            .await?
            .ok_or("the added user is not stored")?;
        // audit: COUSR01C-015
        assert_eq!(
            entry.usr_type,
            PicX::<1>::from_text(user_type)?,
            "{user_id}"
        );
    }
    let signed_on: SignonResponse = api.sign_on("TYPEA001", UPPER_PASSWORD).await?.parse()?;
    // A type `a` user signs on to the main menu, not the admin menu.
    // audit: COUSR01C-015
    assert_eq!(
        (signed_on.user_type.as_str(), signed_on.next_route.as_str()),
        ("a", "/menu")
    );
    // audit: COUSR01C-015
    assert_eq!(
        api.list(Some(&signed_on.token), "").await?.status,
        StatusCode::FORBIDDEN
    );
    Ok(())
}

/// What sign-on (endpoint 1) makes of what COUSR01C stores as typed: a password with a
/// lower-case letter never verifies, one without verifies in either case, and a user ID with a
/// lower-case letter is never read (COUSR01C-019, COUSR01C-020; E26).
async fn add_then_sign_on(api: &Api, admin: &str) -> TestResult {
    api.add_user(admin, "LOWPWD01", LOWER_PASSWORD, "U").await?;
    let entry = api
        .user("LOWPWD01")
        .await?
        .ok_or("the added user is not stored")?;
    // audit: COUSR01C-018
    assert!(
        password::verify(b"lowercas", &entry.password_hash)?,
        "the stored hash does not verify the password as typed"
    );
    let lower = api.sign_on("lowpwd01", LOWER_PASSWORD).await?;
    // audit: COUSR01C-019
    expect_error(
        &lower,
        StatusCode::UNAUTHORIZED,
        WRONG_PASSWORD,
        Some("password"),
    )?;
    let upper = api.sign_on("newusr01", UPPER_PASSWORD_LOWERED).await?;
    // audit: COUSR01C-019
    assert_eq!(upper.status, StatusCode::OK);

    api.add_user(admin, "newusr02", UPPER_PASSWORD, "U").await?;
    let unread = api.sign_on("newusr02", UPPER_PASSWORD).await?;
    // audit: COUSR01C-020
    expect_error(
        &unread,
        StatusCode::UNAUTHORIZED,
        SIGNON_NOT_FOUND,
        Some("user_id"),
    )?;

    // `user0001` is stored beside `USER0001`, whose row sign-on reads in its place.
    api.add_user(admin, "user0001", OTHER_PASSWORD, "A").await?;
    let fixture: SignonResponse = api.sign_on("user0001", FIXTURE_PASSWORD).await?.parse()?;
    // audit: COUSR01C-020
    assert_eq!(
        (fixture.user_id.as_str(), fixture.user_type.as_str()),
        ("USER0001", "U")
    );
    let added = api.sign_on("user0001", OTHER_PASSWORD).await?;
    // audit: COUSR01C-020
    expect_error(
        &added,
        StatusCode::UNAUTHORIZED,
        WRONG_PASSWORD,
        Some("password"),
    )
}

/// The success message takes the user ID up to its first space (COUSR01C-021), and a user ID
/// already stored is refused with the existing row unchanged (COUSR01C-023).
async fn add_messages_and_duplicates(api: &Api, admin: &str) -> TestResult {
    for (user_id, message) in [
        ("AB CD", "User AB has been added ..."),
        (" AB", "User  has been added ..."),
    ] {
        let reply = api.add(Some(admin), &add_body(user_id)).await?;
        // audit: COUSR01C-021
        expect_message(&reply, message)?;
    }

    let stored = api.user("USER0001").await?;
    let duplicate = api.add(Some(admin), &add_body("USER0001")).await?;
    // 400, not 409: the audit keeps 409 for a record changed by another user, and a duplicate
    // is input the operator must change.
    // audit: COUSR01C-023
    expect_error(
        &duplicate,
        StatusCode::BAD_REQUEST,
        ALREADY_EXISTS,
        Some("user_id"),
    )?;
    // audit: COUSR01C-023
    assert_eq!(api.user("USER0001").await?, stored);
    Ok(())
}

/// A body of endpoint 19: names, type and, when given, the password.
fn update_body(
    first_name: &str,
    last_name: &str,
    user_type: &str,
    password: Option<&str>,
) -> Value {
    let mut body = json!({
        "first_name": first_name,
        "last_name": last_name,
        "user_type": user_type,
    });
    if let Some(password) = password {
        body["password"] = json!(password);
    }
    body
}

/// Endpoints 18 to 20, COUSR02C and COUSR03C, in one fixed order on one database: the gate, the
/// shared read, the update's edits, change test, password and type rules, its messages, then
/// the delete and an admin's delete of their own user.
#[test]
fn cousr02c_cousr03c_maintenance_rows() -> TestResult {
    block_on(async {
        let api = Api::loaded().await?;
        let admin = api.token("ADMIN001", FIXTURE_PASSWORD).await?;
        maintenance_gate(&api).await?;
        read_rows(&api, &admin).await?;
        update_edit_rows(&api, &admin).await?;
        update_change_rows(&api, &admin).await?;
        update_password_rows(&api, &admin).await?;
        update_type_and_token_rows(&api, &admin).await?;
        update_message_and_blocked_rows(&api, &admin).await?;
        delete_rows(&api, &admin).await?;
        delete_own_user_rows(&api, &admin).await
    })
}

/// 401 without a valid token and 403 for a `U` token on endpoints 18, 19 and 20, with nothing
/// read or written (COUSR02C-001, COUSR02C-002, COUSR03C-001, COUSR03C-002).
async fn maintenance_gate(api: &Api) -> TestResult {
    let before = api.users().await?;
    let user = api.token("USER0002", FIXTURE_PASSWORD).await?;
    let body = update_body("CHANGED", "THOMAS", "U", None);
    for token in [None, Some("not-a-token")] {
        // audit: COUSR02C-001
        assert_eq!(
            api.fetch(token, "USER0001").await?.status,
            StatusCode::UNAUTHORIZED
        );
        let update = api.update(token, "USER0001", &body).await?;
        // audit: COUSR02C-001
        assert_eq!(update.status, StatusCode::UNAUTHORIZED);
        // audit: COUSR03C-001
        assert_eq!(
            api.delete(token, "USER0001").await?.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let fetch = api.fetch(Some(&user), "USER0001").await?;
    // audit: COUSR02C-002
    assert_eq!(fetch.status, StatusCode::FORBIDDEN);
    let update = api.update(Some(&user), "USER0001", &body).await?;
    // audit: COUSR02C-002
    assert_eq!(update.status, StatusCode::FORBIDDEN);
    let delete = api.delete(Some(&user), "USER0001").await?;
    // audit: COUSR03C-002
    assert_eq!(delete.status, StatusCode::FORBIDDEN);
    // audit: COUSR02C-001
    // audit: COUSR03C-001
    // audit: COUSR03C-002
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// Endpoint 18, the read both programs share: the blank and over-long IDs, a found user with
/// both programs' prompts and no password, and a key read exactly as typed (COUSR02C-010 to
/// COUSR02C-012, COUSR02C-015; COUSR03C-010, COUSR03C-013; COUSR02-013, COUSR03-013).
async fn read_rows(api: &Api, admin: &str) -> TestResult {
    let before = api.users().await?;
    let blank = api.fetch(Some(admin), "   ").await?;
    // audit: COUSR02C-010
    // audit: COUSR03C-010
    expect_error(
        &blank,
        StatusCode::BAD_REQUEST,
        USER_ID_EMPTY,
        Some("user_id"),
    )?;
    let too_long = api.fetch(Some(admin), "USER00019").await?;
    // audit: COUSR02-013
    // audit: COUSR03-013
    expect_error(
        &too_long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("user_id"),
    )?;

    let found = api.fetch(Some(admin), "USER0001").await?;
    let expected = UserView {
        user_id: "USER0001".to_owned(),
        first_name: "LAWRENCE".to_owned(),
        last_name: "THOMAS".to_owned(),
        user_type: "U".to_owned(),
        update_prompt: UPDATE_PROMPT.to_owned(),
        delete_prompt: DELETE_PROMPT.to_owned(),
    };
    // audit: COUSR02C-012
    assert_eq!(
        (found.status, found.parse::<UserView>()?),
        (StatusCode::OK, expected.clone())
    );
    // Exactly the view's members: no password, no hash and no message.
    // audit: COUSR02C-012
    assert_eq!(found.body, serde_json::to_value(&expected)?);

    // The key is the path as typed, never upper-cased: `admin001` does not find `ADMIN001`.
    let lower = api.fetch(Some(admin), "admin001").await?;
    // audit: COUSR02C-011
    // audit: COUSR02C-015
    // audit: COUSR03C-013
    expect_error(&lower, StatusCode::NOT_FOUND, NOT_FOUND, Some("user_id"))?;
    let unknown = api.fetch(Some(admin), "NOUSER01").await?;
    // audit: COUSR02C-015
    // audit: COUSR03C-013
    expect_error(&unknown, StatusCode::NOT_FOUND, NOT_FOUND, Some("user_id"))?;
    // The read writes nothing.
    // audit: COUSR02C-011
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// The edits of endpoint 19 in the program's order, before any read, and the map lengths of
/// the path and the body (COUSR02C-018 to COUSR02C-022; COUSR02-013, COUSR02-017, COUSR02-020,
/// COUSR02-023, COUSR02-026).
async fn update_edit_rows(api: &Api, admin: &str) -> TestResult {
    let stored = api.user("USER0001").await?;
    let blank = api.update(Some(admin), "   ", &json!({})).await?;
    // The blank user ID is tested first, before the blank fields.
    // audit: COUSR02C-018
    expect_error(
        &blank,
        StatusCode::BAD_REQUEST,
        USER_ID_EMPTY,
        Some("user_id"),
    )?;
    let valid = update_body("LAWRENCE", "THOMSON", "U", None);
    let too_long = api.update(Some(admin), "USER00019", &valid).await?;
    // audit: COUSR02-013
    expect_error(
        &too_long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("user_id"),
    )?;

    let first = api.update(Some(admin), "USER0001", &json!({})).await?;
    // audit: COUSR02C-019
    expect_error(
        &first,
        StatusCode::BAD_REQUEST,
        FIRST_NAME_EMPTY,
        Some("first_name"),
    )?;
    let body = json!({ "first_name": "LAWRENCE", "last_name": "  " });
    let last = api.update(Some(admin), "USER0001", &body).await?;
    // audit: COUSR02C-020
    expect_error(
        &last,
        StatusCode::BAD_REQUEST,
        LAST_NAME_EMPTY,
        Some("last_name"),
    )?;
    let body = update_body("LAWRENCE", "THOMAS", "", Some("   "));
    let password = api.update(Some(admin), "USER0001", &body).await?;
    // audit: COUSR02C-021
    expect_error(
        &password,
        StatusCode::BAD_REQUEST,
        PASSWORD_EMPTY,
        Some("password"),
    )?;
    // An omitted password passes its edit, so the blank type answers.
    let omitted = api
        .update(
            Some(admin),
            "USER0001",
            &update_body("LAWRENCE", "THOMAS", "", None),
        )
        .await?;
    // audit: COUSR02C-021
    // audit: COUSR02C-022
    expect_error(
        &omitted,
        StatusCode::BAD_REQUEST,
        USER_TYPE_EMPTY,
        Some("user_type"),
    )?;
    let body = update_body("LAWRENCE", "THOMAS", " ", Some(FIXTURE_PASSWORD));
    let user_type = api.update(Some(admin), "USER0001", &body).await?;
    // audit: COUSR02C-022
    expect_error(
        &user_type,
        StatusCode::BAD_REQUEST,
        USER_TYPE_EMPTY,
        Some("user_type"),
    )?;

    let password = format!("{NEW_PASSWORD}9");
    for (member, value) in [
        ("first_name", "F".repeat(21)),
        ("last_name", "L".repeat(21)),
        ("password", password),
        ("user_type", "UA".to_owned()),
    ] {
        let mut body = valid.clone();
        body[member] = json!(value);
        let reply = api.update(Some(admin), "USER0001", &body).await?;
        // audit: COUSR02-017
        // audit: COUSR02-020
        // audit: COUSR02-023
        // audit: COUSR02-026
        expect_error(
            &reply,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            Some(member),
        )?;
    }
    // audit: COUSR02C-018
    // audit: COUSR02C-019
    assert_eq!(api.user("USER0001").await?, stored);
    Ok(())
}

/// The change test against the row as read now, byte for byte, and the rewrite of the whole
/// row (COUSR02C-024, COUSR02C-026 to COUSR02C-029).
async fn update_change_rows(api: &Api, admin: &str) -> TestResult {
    let stored = api
        .user("USER0001")
        .await?
        .ok_or("USER0001 is not stored")?;
    let unchanged = update_body("LAWRENCE", "THOMAS", "U", None);
    let same = api.update(Some(admin), "USER0001", &unchanged).await?;
    // audit: COUSR02C-026
    // audit: COUSR02C-028
    expect_error(
        &same,
        StatusCode::BAD_REQUEST,
        NOT_MODIFIED,
        Some("first_name"),
    )?;
    // The current password verifies against the stored hash, so it is no modification (E27).
    let current = update_body("LAWRENCE", "THOMAS", "U", Some(FIXTURE_PASSWORD));
    let same_password = api.update(Some(admin), "USER0001", &current).await?;
    // audit: COUSR02C-027
    // audit: COUSR02C-028
    expect_error(
        &same_password,
        StatusCode::BAD_REQUEST,
        NOT_MODIFIED,
        Some("first_name"),
    )?;
    // audit: COUSR02C-028
    assert_eq!(api.user("USER0001").await?.as_ref(), Some(&stored));

    let body = update_body("LAWRENCE", "THOMSON", "U", None);
    let changed = api.update(Some(admin), "USER0001", &body).await?;
    // audit: COUSR02C-029
    expect_message(&changed, "User USER0001 has been updated ...")?;
    // The whole row is rewritten: the ID, the filler and the hash as read, the new last name.
    let rewritten = UserEntry {
        lname: PicX::from_text("THOMSON")?,
        ..stored.clone()
    };
    // audit: COUSR02C-027
    // audit: COUSR02C-029
    assert_eq!(api.user("USER0001").await?, Some(rewritten));
    // An omitted password stays as it was: the fixture password still signs on.
    let signon = api.sign_on("USER0001", FIXTURE_PASSWORD).await?;
    // audit: COUSR02C-027
    assert_eq!(signon.status, StatusCode::OK);

    // Case and leading spaces are significant, and the value is stored as received.
    for first_name in ["lawrence", " lawrence"] {
        let body = update_body(first_name, "THOMSON", "U", None);
        let reply = api.update(Some(admin), "USER0001", &body).await?;
        // audit: COUSR02C-026
        expect_message(&reply, "User USER0001 has been updated ...")?;
        let entry = api
            .user("USER0001")
            .await?
            .ok_or("USER0001 is not stored")?;
        // audit: COUSR02C-026
        assert_eq!(entry.fname, PicX::<20>::from_text(first_name)?);
    }

    // No changed-by-another-user check: the values a screen fetched before another update are
    // compared with the row as read now, and stored.
    let fetched: UserView = api.fetch(Some(admin), "USER0003").await?.parse()?;
    let other = update_body("LAURITZ", "ALMEIDA", "U", None);
    let first = api.update(Some(admin), "USER0003", &other).await?;
    expect_message(&first, "User USER0003 has been updated ...")?;
    let stale = update_body(
        &fetched.first_name,
        &fetched.last_name,
        &fetched.user_type,
        None,
    );
    let second = api.update(Some(admin), "USER0003", &stale).await?;
    // audit: COUSR02C-024
    expect_message(&second, "User USER0003 has been updated ...")?;
    let entry = api
        .user("USER0003")
        .await?
        .ok_or("USER0003 is not stored")?;
    // audit: COUSR02C-024
    assert_eq!(entry.lname, PicX::<20>::from_text("ALME")?);
    Ok(())
}

/// A supplied password that differs is hashed and replaces the stored hash. It is stored as
/// typed, so one holding a lower-case letter never signs on (COUSR02C-027; E26, E27).
async fn update_password_rows(api: &Api, admin: &str) -> TestResult {
    let before = api
        .user("USER0001")
        .await?
        .ok_or("USER0001 is not stored")?;
    let body = update_body(" lawrence", "THOMSON", "U", Some(NEW_PASSWORD));
    let reply = api.update(Some(admin), "USER0001", &body).await?;
    // audit: COUSR02C-027
    // audit: COUSR02C-029
    expect_message(&reply, "User USER0001 has been updated ...")?;
    let entry = api
        .user("USER0001")
        .await?
        .ok_or("USER0001 is not stored")?;
    // audit: COUSR02C-027
    assert!(
        entry.password_hash != before.password_hash,
        "the stored hash was not replaced"
    );
    // audit: COUSR02C-027
    assert!(
        password::verify(b"NEWPASS1", &entry.password_hash)?,
        "the stored hash does not verify the password as typed"
    );
    let new = api.sign_on("USER0001", NEW_PASSWORD).await?;
    // audit: COUSR02C-027
    assert_eq!(new.status, StatusCode::OK);
    let old = api.sign_on("USER0001", FIXTURE_PASSWORD).await?;
    // audit: COUSR02C-027
    expect_error(
        &old,
        StatusCode::UNAUTHORIZED,
        WRONG_PASSWORD,
        Some("password"),
    )?;

    let body = update_body(" lawrence", "THOMSON", "U", Some(NEW_LOWER_PASSWORD));
    let lower = api.update(Some(admin), "USER0001", &body).await?;
    // audit: COUSR02C-027
    expect_message(&lower, "User USER0001 has been updated ...")?;
    let entry = api
        .user("USER0001")
        .await?
        .ok_or("USER0001 is not stored")?;
    // audit: COUSR02C-027
    assert!(
        password::verify(b"newpass2", &entry.password_hash)?,
        "the stored hash does not verify the password as typed"
    );
    let refused = api.sign_on("USER0001", NEW_LOWER_PASSWORD).await?;
    // audit: COUSR02C-027
    expect_error(
        &refused,
        StatusCode::UNAUTHORIZED,
        WRONG_PASSWORD,
        Some("password"),
    )
}

/// Any non-blank user type is stored as received, and a token already issued keeps its claims
/// after its user is rewritten (COUSR02C-023, COUSR02C-029).
async fn update_type_and_token_rows(api: &Api, admin: &str) -> TestResult {
    let body = update_body("AJITH", "KUMAR", "x", None);
    let reply = api.update(Some(admin), "USER0002", &body).await?;
    // audit: COUSR02C-023
    expect_message(&reply, "User USER0002 has been updated ...")?;
    let entry = api
        .user("USER0002")
        .await?
        .ok_or("USER0002 is not stored")?;
    // audit: COUSR02C-023
    assert_eq!(entry.usr_type, PicX::<1>::from_text("x")?);
    let signed_on: SignonResponse = api.sign_on("USER0002", FIXTURE_PASSWORD).await?.parse()?;
    // A type other than `A` signs on to the main menu.
    // audit: COUSR02C-023
    assert_eq!(
        (signed_on.user_type.as_str(), signed_on.next_route.as_str()),
        ("x", "/menu")
    );

    let issued = api.token("ADMIN002", FIXTURE_PASSWORD).await?;
    let body = update_body("RUSSELL", "RUSSELL", "U", None);
    let demoted = api.update(Some(admin), "ADMIN002", &body).await?;
    // audit: COUSR02C-029
    expect_message(&demoted, "User ADMIN002 has been updated ...")?;
    // audit: COUSR02C-029
    assert_eq!(api.list(Some(&issued), "").await?.status, StatusCode::OK);
    let next: SignonResponse = api.sign_on("ADMIN002", FIXTURE_PASSWORD).await?.parse()?;
    // audit: COUSR02C-029
    assert_eq!(
        (next.user_type.as_str(), next.next_route.as_str()),
        ("U", "/menu")
    );
    Ok(())
}

/// The success message takes the user ID up to its first space (COUSR02C-030), and an update
/// of a user ID with no row, which halt candidate `X-COUSR02C-H1` blocks, changes and creates
/// nothing (COUSR02C-025).
async fn update_message_and_blocked_rows(api: &Api, admin: &str) -> TestResult {
    for (user_id, message) in [
        ("AB CD", "User AB has been updated ..."),
        (" AB", "User  has been updated ..."),
    ] {
        api.add_user(admin, user_id, UPPER_PASSWORD, "U").await?;
        let body = update_body("ADDED", "CHANGED", "U", None);
        let reply = api.update(Some(admin), user_id, &body).await?;
        // audit: COUSR02C-030
        expect_message(&reply, message)?;
    }

    let before = api.users().await?;
    let body = update_body("NOBODY", "NOWHERE", "U", None);
    // audit: COUSR02C-025
    api.update(Some(admin), "NOUSER01", &body).await?;
    println!("BLOCKED COUSR02C-025 (X-COUSR02C-H1): only the store effect is asserted");
    // audit: COUSR02C-025
    assert_eq!(api.users().await?, before);
    Ok(())
}

/// Endpoint 20: the blank and over-long IDs, IDs with no row, which halt candidate
/// `X-COUSR03C-H1` blocks, and successful deletes with their messages (COUSR03C-016 to
/// COUSR03C-020; COUSR03-013).
async fn delete_rows(api: &Api, admin: &str) -> TestResult {
    let before = api.users().await?;
    let blank = api.delete(Some(admin), "   ").await?;
    // audit: COUSR03C-016
    expect_error(
        &blank,
        StatusCode::BAD_REQUEST,
        USER_ID_EMPTY,
        Some("user_id"),
    )?;
    let too_long = api.delete(Some(admin), "USER00019").await?;
    // audit: COUSR03-013
    expect_error(
        &too_long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("user_id"),
    )?;

    // The key is the path as typed: `user0001` does not find `USER0001`, and with no row the
    // delete's answer has no single COBOL result, so only the store effect is asserted. The 500
    // the replacement answers for a missing user is not claimed as parity, and no 404 is
    // expected here: a delete that affects no row cannot follow the locked read.
    // audit: COUSR03C-017
    // audit: COUSR03C-018
    api.delete(Some(admin), "user0001").await?;
    // audit: COUSR03C-018
    api.delete(Some(admin), "NOUSER01").await?;
    println!("BLOCKED COUSR03C-018 (X-COUSR03C-H1): only the store effect is asserted");
    // audit: COUSR03C-016
    // audit: COUSR03C-017
    // audit: COUSR03C-018
    assert_eq!(api.users().await?, before);

    let deleted = api.delete(Some(admin), "USER0004").await?;
    // audit: COUSR03C-019
    expect_message(&deleted, "User USER0004 has been deleted ...")?;
    // audit: COUSR03C-019
    assert_eq!(api.user("USER0004").await?, None);
    let removed = key("USER0004")?;
    let rest: Vec<UserListEntry> = before
        .into_iter()
        .filter(|user| user.id != removed)
        .collect();
    // No other row changes.
    // audit: COUSR03C-019
    assert_eq!(api.users().await?, rest);
    // A second delete of the same ID finds no row: blocked as above, and nothing changes.
    // audit: COUSR03C-018
    api.delete(Some(admin), "USER0004").await?;
    // audit: COUSR03C-018
    assert_eq!(api.users().await?, rest);

    for (user_id, message) in [
        ("AB CD", "User AB has been deleted ..."),
        (" AB", "User  has been deleted ..."),
    ] {
        let reply = api.delete(Some(admin), user_id).await?;
        // audit: COUSR03C-020
        expect_message(&reply, message)?;
        // audit: COUSR03C-020
        assert_eq!(api.user(user_id).await?, None);
    }
    Ok(())
}

/// An admin deletes their own user: nothing stops it, the token already issued keeps its
/// claims, and the next sign-on finds no user (COUSR03C-024).
async fn delete_own_user_rows(api: &Api, admin: &str) -> TestResult {
    let reply = api.delete(Some(admin), "ADMIN001").await?;
    // audit: COUSR03C-024
    expect_message(&reply, "User ADMIN001 has been deleted ...")?;
    // audit: COUSR03C-024
    assert_eq!(api.list(Some(admin), "").await?.status, StatusCode::OK);
    let next = api.sign_on("ADMIN001", FIXTURE_PASSWORD).await?;
    // audit: COUSR03C-024
    expect_error(
        &next,
        StatusCode::UNAUTHORIZED,
        SIGNON_NOT_FOUND,
        Some("user_id"),
    )
}

/// Store failures, over an empty database whose pool is closed: the list, the read and the
/// add answer with their programs' messages (COUSR00C-026, COUSR01C-024, COUSR02C-016,
/// COUSR03C-014).
#[test]
fn store_failure_rows() -> TestResult {
    block_on(async {
        let api = Api::closed().await?;
        // Sign-on reads the store, so the token comes from the state's own keys.
        let admin = api.state.jwt.issue(&key("ADMIN001")?, &UserType::ADMIN)?;

        let list = api.list(Some(&admin), "").await?;
        // audit: COUSR00C-026
        expect_error(
            &list,
            StatusCode::INTERNAL_SERVER_ERROR,
            LOOKUP_FAILED,
            None,
        )?;
        let fetch = api.fetch(Some(&admin), "USER0001").await?;
        // audit: COUSR02C-016
        // audit: COUSR03C-014
        expect_error(
            &fetch,
            StatusCode::INTERNAL_SERVER_ERROR,
            LOOKUP_FAILED,
            Some("first_name"),
        )?;
        let add = api.add(Some(&admin), &add_body("NEWUSR01")).await?;
        // audit: COUSR01C-024
        expect_error(
            &add,
            StatusCode::INTERNAL_SERVER_ERROR,
            ADD_FAILED,
            Some("first_name"),
        )?;

        // The delete opens its transaction before it reads, and the update, whose unlocked read
        // fails, then reads inside its transaction. A transaction that cannot begin has no audited
        // text, so the API's fixed 500 answers, with no store detail. The rewrite and delete
        // failures of COUSR02C-032 and COUSR03C-022 follow a successful locked read, which a closed
        // pool never gives; `cousr02c_cousr03c_write_outcome_rows` proves them over a probed store.
        let body = update_body("LAWRENCE", "THOMSON", "U", None);
        let update = api.update(Some(&admin), "USER0001", &body).await?;
        expect_error(
            &update,
            StatusCode::INTERNAL_SERVER_ERROR,
            UNABLE_TO_PROCESS,
            None,
        )?;
        let delete = api.delete(Some(&admin), "USER0001").await?;
        expect_error(
            &delete,
            StatusCode::INTERNAL_SERVER_ERROR,
            UNABLE_TO_PROCESS,
            None,
        )
    })
}

/// One call a [`ScriptedProbe`] was asked about, with the argument it carried.
#[derive(Debug, PartialEq, Eq)]
enum Probed {
    /// `UserRepository::update`, with the entry it rewrites.
    Update(UserEntry),
    /// `UserRepository::delete`, with the key it deletes.
    Delete(PicX<8>),
    /// A call at any other point, or one whose argument is not the type its point lists.
    Other(ProbePoint),
}

/// The probe of [`Api::probed`]: it answers its scripted verdict at its scripted point, lets
/// every other call proceed, and logs every call it is asked about.
struct ScriptedProbe {
    /// The point to answer and the verdict to answer there; `None` lets every call proceed.
    script: Mutex<Option<(ProbePoint, ProbeVerdict)>>,
    /// The calls asked about since the last look.
    seen: Mutex<Vec<Probed>>,
}

impl ScriptedProbe {
    /// A probe that lets every call proceed until it is scripted.
    fn new() -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(None),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// Answers `verdict` at `point` from now on.
    fn script(&self, point: ProbePoint, verdict: ProbeVerdict) -> TestResult {
        *self.script.lock().map_err(|_| "script lock poisoned")? = Some((point, verdict));
        Ok(())
    }

    /// The calls asked about since the last look, in the order they were asked.
    fn seen(&self) -> Result<Vec<Probed>, Box<dyn Error>> {
        Ok(std::mem::take(
            &mut *self.seen.lock().map_err(|_| "seen lock poisoned")?,
        ))
    }
}

impl RepositoryProbe for ScriptedProbe {
    fn answer(&self, call: &ProbeCall<'_>) -> ProbeVerdict {
        let probed = match call.point() {
            ProbePoint::UserUpdate => call.arg::<UserEntry>(0).cloned().map(Probed::Update),
            ProbePoint::UserDelete => call.arg::<PicX<8>>(0).cloned().map(Probed::Delete),
            _ => None,
        };
        // A poisoned log loses this call, and the test's next look at the log fails.
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(probed.unwrap_or(Probed::Other(call.point())));
        }
        self.script
            .lock()
            .ok()
            .and_then(|script| script.clone())
            .filter(|(point, _)| *point == call.point())
            .map_or(ProbeVerdict::Proceed, |(_, verdict)| verdict)
    }
}

/// The failure the probe injects in place of a rewrite or a delete: SQLSTATE `57014`
/// (`query_canceled`), which no statement of these tests raises. It stands for the CICS codes
/// of the `WHEN OTHER` branch, whose values are never asserted (halt H4).
fn canceled() -> PersistenceError {
    PersistenceError::Database {
        sqlstate: Some("57014".to_owned()),
        constraint: None,
    }
}

/// Endpoints 19 and 20 whose rewrite or delete, after the real locked read has found the row,
/// affects no row or fails, over a loaded database whose store a [`ScriptedProbe`] watches
/// (COUSR02C-031, COUSR02C-032, COUSR03C-021, COUSR03C-022).
#[test]
fn cousr02c_cousr03c_write_outcome_rows() -> TestResult {
    block_on(async {
        let probe = ScriptedProbe::new();
        let api = Api::probed(probe.clone()).await?;
        let admin = api.token("ADMIN001", FIXTURE_PASSWORD).await?;
        assert!(
            probe.seen()?.is_empty(),
            "sign-on's read is not a probe point"
        );
        rewrite_outcome_rows(&api, &probe, &admin).await?;
        delete_outcome_rows(&api, &probe, &admin).await
    })
}

/// Endpoint 19 with new names and type for `USER0001` and no password: the unlocked read and
/// the locked read run on the real store, with the change test between them, and the probe
/// answers the rewrite in the transaction that holds the row. `NoRow` is a rewrite that finds
/// no row, 404 `User ID NOT found...` on `user_id` (COUSR02C-031); `Fail` is any other failure,
/// 500 `Unable to Update User...` on `first_name` (COUSR02C-032). Either way nothing is
/// written.
async fn rewrite_outcome_rows(api: &Api, probe: &ScriptedProbe, admin: &str) -> TestResult {
    let stored = api
        .user("USER0001")
        .await?
        .ok_or("USER0001 is not stored")?;
    let before = api.users().await?;
    let body = update_body("LAURENT", "THOMSON", "A", None);
    // The request's names and type over the row as read: its ID, its hash, since no password
    // was supplied, and its filler.
    let intended = UserEntry {
        fname: PicX::from_text("LAURENT")?,
        lname: PicX::from_text("THOMSON")?,
        usr_type: PicX::from_text("A")?,
        ..stored.clone()
    };
    for (verdict, status, message, field) in [
        (
            ProbeVerdict::NoRow,
            StatusCode::NOT_FOUND,
            NOT_FOUND,
            "user_id",
        ),
        (
            ProbeVerdict::Fail(canceled()),
            StatusCode::INTERNAL_SERVER_ERROR,
            UPDATE_FAILED,
            "first_name",
        ),
    ] {
        probe.script(ProbePoint::UserUpdate, verdict)?;
        let reply = api.update(Some(admin), "USER0001", &body).await?;
        // audit: COUSR02C-031
        // audit: COUSR02C-032
        expect_error(&reply, status, message, Some(field))?;
        // The rewrite is reached once, after the locked read, with the entry the service built.
        // audit: COUSR02C-031
        // audit: COUSR02C-032
        assert_eq!(probe.seen()?, [Probed::Update(intended.clone())]);
        // The transaction rolled back: the row and the table are as they were.
        // audit: COUSR02C-031
        // audit: COUSR02C-032
        assert_eq!(api.user("USER0001").await?.as_ref(), Some(&stored));
        // audit: COUSR02C-031
        // audit: COUSR02C-032
        assert_eq!(api.users().await?, before);
    }

    // With the rewrite let through, the same request locks the row again and rewrites it.
    probe.script(ProbePoint::UserUpdate, ProbeVerdict::Proceed)?;
    let reply = api.update(Some(admin), "USER0001", &body).await?;
    expect_message(&reply, "User USER0001 has been updated ...")?;
    assert_eq!(probe.seen()?, [Probed::Update(intended.clone())]);
    assert_eq!(api.user("USER0001").await?, Some(intended));
    Ok(())
}

/// Endpoint 20 for `USER0002`: the locked read runs on the real store and finds the row, and
/// the probe answers the delete in the transaction that holds it. `NoRow` is a delete that
/// finds no row, 404 `User ID NOT found...` on `user_id` (COUSR03C-021); `Fail` is any other
/// failure, 500 `Unable to Update User...` on `first_name`, `Update` kept (COUSR03C-022).
/// Either way the user stays.
async fn delete_outcome_rows(api: &Api, probe: &ScriptedProbe, admin: &str) -> TestResult {
    let stored = api
        .user("USER0002")
        .await?
        .ok_or("USER0002 is not stored")?;
    let before = api.users().await?;
    let target = key("USER0002")?;
    for (verdict, status, message, field) in [
        (
            ProbeVerdict::NoRow,
            StatusCode::NOT_FOUND,
            NOT_FOUND,
            "user_id",
        ),
        (
            ProbeVerdict::Fail(canceled()),
            StatusCode::INTERNAL_SERVER_ERROR,
            UPDATE_FAILED,
            "first_name",
        ),
    ] {
        probe.script(ProbePoint::UserDelete, verdict)?;
        let reply = api.delete(Some(admin), "USER0002").await?;
        // audit: COUSR03C-021
        // audit: COUSR03C-022
        expect_error(&reply, status, message, Some(field))?;
        // The delete is reached once, after the locked read, with the path's key.
        // audit: COUSR03C-021
        // audit: COUSR03C-022
        assert_eq!(probe.seen()?, [Probed::Delete(target.clone())]);
        // The transaction rolled back: the user and the table are as they were.
        // audit: COUSR03C-021
        // audit: COUSR03C-022
        assert_eq!(api.user("USER0002").await?.as_ref(), Some(&stored));
        // audit: COUSR03C-021
        // audit: COUSR03C-022
        assert_eq!(api.users().await?, before);
    }

    // With the delete let through, the same request locks the row again and deletes it.
    probe.script(ProbePoint::UserDelete, ProbeVerdict::Proceed)?;
    let reply = api.delete(Some(admin), "USER0002").await?;
    expect_message(&reply, "User USER0002 has been deleted ...")?;
    assert_eq!(probe.seen()?, [Probed::Delete(target)]);
    assert_eq!(api.user("USER0002").await?, None);
    Ok(())
}
