//! Shared HTTP test harness of the `carddemo-api` integration suites.
//!
//! Each Docker-backed test file in `crates/api/tests/` includes this module
//! with `pub mod common;`. Cargo builds no target of its own from
//! `tests/common/mod.rs`, and the `pub` keeps the helpers a given file does
//! not call free of dead-code warnings.
//!
//! A [`TestApp`] is the real router of [`carddemo_api::app`] served over a
//! local TCP port, on its own seeded `postgres:16.15` container from
//! [`carddemo_testkit::TestStack`]. Requests go through `reqwest`, so every
//! test crosses the same HTTP layer the SPA does: extractors, the bearer
//! token, the `{code, message, fields}` error body and the status mapping of
//! `carddemo_api::error`. Docker must be running.
//!
//! Tests are plain `#[test]` functions, never `#[tokio::test]`:
//!
//! - create the app first, outside any runtime, with [`TestApp::start`] or
//!   [`TestApp::start_with_clock`];
//! - run the requests inside `carddemo_testkit::block_on(async { … })`, which
//!   drives the one process-wide runtime the container, the pool, the server
//!   task and the HTTP client all live on;
//! - let the app drop at the end of the test function, after `block_on` has
//!   returned.
//!
//! Expected message texts come from `carddemo_domain::messages` in the
//! calling test. The harness names only the seed credentials and route
//! paths, and reaches SQL only through [`TestApp::stack`], whose
//! `exec_sql` and `fetch_text_rows` run text-typed statements.
//!
//! Every request is bounded, so a handler stalled on SQL or a lock fails its
//! test instead of hanging the suite: connecting may take at most
//! [`CONNECT_TIMEOUT`], and the whole exchange, from the start of the
//! connection until the response body has been read, at most
//! [`REQUEST_TIMEOUT`]. A test that deliberately needs another bound shapes
//! its request through [`TestApp::client`], sets it with
//! `RequestBuilder::timeout`, which replaces [`REQUEST_TIMEOUT`] for that
//! request only, and passes the builder to [`send`]. An expired bound panics
//! naming the stage, the URL and the bound.

use std::collections::BTreeSet;
use std::time::Duration;

use carddemo_audit::{Document, IdKind, PARITY_AUDIT_PATH, Repo, VALIDATION_AUDIT_PATH};
use carddemo_runtime::config::Settings;
use carddemo_testkit::{TestStack, block_on, fixtures};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The admin user of the seed: `ADMIN001`, type `A`, first `SYSUT1` row of
/// `DUSRSECJ` [app/jcl/DUSRSECJ.jcl:L35].
const ADMIN_USER: &str = "ADMIN001";

/// The regular user of the seed: `USER0001`, type `U`, the first type `U` row
/// of `DUSRSECJ` [app/jcl/DUSRSECJ.jcl:L40].
const REGULAR_USER: &str = "USER0001";

/// The password every `DUSRSECJ` user row stores, upper case as sign-on
/// compares it [app/jcl/DUSRSECJ.jcl:L35-44].
const SEED_PASSWORD: &str = "PASSWORD";

/// The sign-on route, the only issuer of bearer tokens.
const LOGIN_PATH: &str = "/auth/login";

/// The bound on opening the TCP connection to the test server.
///
/// The server listens on `127.0.0.1`, so a connection that is not open within
/// this bound means the server task is not accepting.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The default bound on one whole request, from the start of the connection
/// until the response body has been read.
///
/// It is generous enough for unoptimised builds of the report and posting
/// handlers on a loaded host, and finite, so a handler stalled on SQL or a
/// lock fails its test. `RequestBuilder::timeout` replaces it for a single
/// request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// One HTTP response: the status, the body parsed as JSON (`Value::Null` when
/// the body is empty or not JSON), and the raw body text.
///
/// The raw text is kept because some checks need the exact bytes, for example
/// a points balance above 2^53 that must arrive as the JSON string it was
/// stored as.
pub type Resp = (StatusCode, Value, String);

/// The API router served on `127.0.0.1` over a seeded test database.
///
/// Fields drop in declaration order after [`Drop::drop`] has aborted the
/// server task: the client, the base URL, the task handle, and last the stack,
/// whose own drop removes the container.
///
/// A `TestApp` must be dropped outside `carddemo_testkit::block_on`: create it
/// before the first `block_on` of a test and let it drop when the test
/// function returns. Its stack's drop then enters the shared runtime itself.
pub struct TestApp {
    client: Client,
    base_url: String,
    server: JoinHandle<()>,
    stack: TestStack,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl TestApp {
    /// Starts a seeded database and serves the router over it, on the system
    /// clock.
    ///
    /// The database is migrated and holds the seeded state of `batch seed` on
    /// an empty database: 50 accounts, cards, cross-references and customers,
    /// the 10 `DUSRSECJ` users, the transaction types and categories (masters
    /// and processing copies), the disclosure groups, the category balances,
    /// the 2 reward seed rules, and an empty transaction master (E-07).
    ///
    /// # Panics
    ///
    /// Panics if the bounded HTTP client cannot be built, if Docker is
    /// unavailable or the container does not start, if the seed fails, if the
    /// settings are rejected by `AppState::new`, or if no local port can be
    /// bound.
    #[must_use]
    pub fn start() -> Self {
        Self::launch(None)
    }

    /// As [`TestApp::start`], with the processing clock pinned to `rfc3339`,
    /// for example `"2022-07-01T12:00:00Z"`.
    ///
    /// This is `CARDDEMO_CLOCK_OVERRIDE`: every time-dependent output of the
    /// API (payment timestamps, the fraud-report date, the report's current
    /// date, reward accrual and soft-delete instants) uses that instant.
    ///
    /// # Panics
    ///
    /// Panics as [`TestApp::start`] does, and also if `rfc3339` is not an RFC
    /// 3339 instant.
    #[must_use]
    pub fn start_with_clock(rfc3339: &str) -> Self {
        Self::launch(Some(rfc3339.to_owned()))
    }

    /// Builds the client bounded by [`CONNECT_TIMEOUT`] and
    /// [`REQUEST_TIMEOUT`], then the stack, the state and the server task, all
    /// inside one `block_on`, so each is bound to the shared test runtime.
    ///
    /// The client comes first, so a client that cannot be built fails the test
    /// before a container or a server task exists.
    fn launch(clock_override: Option<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|error| {
                panic!("building the bounded test HTTP client failed: {error}")
            });
        let (stack, port, server) = block_on(async move {
            let stack = TestStack::postgres().await;
            stack
                .seed()
                .await
                .unwrap_or_else(|error| panic!("seeding the test database failed: {error}"));
            let settings = Settings {
                database_url: stack.database_url().to_owned(),
                clock_override,
                ..Settings::default()
            };
            let db = carddemo_persistence::Db::from_pool(stack.pool().clone());
            let state = carddemo_api::AppState::new(db, settings)
                .unwrap_or_else(|error| panic!("the test settings were rejected: {error}"));
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|error| panic!("binding a local test port failed: {error}"));
            let port = listener
                .local_addr()
                .unwrap_or_else(|error| panic!("reading the bound test address failed: {error}"))
                .port();
            let server = tokio::spawn(async move {
                if let Err(error) = axum::serve(listener, carddemo_api::app(state)).await {
                    eprintln!("the carddemo-api test server stopped: {error}");
                }
            });
            (stack, port, server)
        });
        Self {
            client,
            base_url: format!("http://127.0.0.1:{port}"),
            server,
            stack,
        }
    }

    /// The test stack, for direct SQL through its `exec_sql` and
    /// `fetch_text_rows`.
    #[must_use]
    pub fn stack(&self) -> &TestStack {
        &self.stack
    }

    /// The absolute URL of `path`, a router path such as `/auth/login` or
    /// `/accounts/00000000001?x=y`. Routes are served at the root, with no
    /// `/api` prefix; any query string is part of `path`.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// The HTTP client, for requests the helpers below do not shape, such as
    /// a malformed `Authorization` header.
    ///
    /// It is bounded by [`CONNECT_TIMEOUT`] and [`REQUEST_TIMEOUT`]. A request
    /// that deliberately needs another total bound sets it with
    /// `RequestBuilder::timeout`, for example `.timeout(Duration::from_secs(300))`,
    /// which replaces [`REQUEST_TIMEOUT`] for that request only; passing the
    /// builder to [`send`] keeps the harness's diagnostics.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Sends `GET path`, with `token` as the bearer token when given.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be sent or its body cannot be read.
    pub async fn get(&self, path: &str, token: Option<&str>) -> Resp {
        send(self.client.get(self.url(path)), token).await
    }

    /// Sends `POST path` with the JSON `body`, with `token` as the bearer
    /// token when given.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be sent or its body cannot be read.
    pub async fn post(&self, path: &str, token: Option<&str>, body: &Value) -> Resp {
        send(self.client.post(self.url(path)).json(body), token).await
    }

    /// Sends `PUT path` with the JSON `body`, with `token` as the bearer token
    /// when given.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be sent or its body cannot be read.
    pub async fn put(&self, path: &str, token: Option<&str>, body: &Value) -> Resp {
        send(self.client.put(self.url(path)).json(body), token).await
    }

    /// Sends `DELETE path`, with `token` as the bearer token when given.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be sent or its body cannot be read.
    pub async fn delete(&self, path: &str, token: Option<&str>) -> Resp {
        send(self.client.delete(self.url(path)), token).await
    }

    /// Signs on through `POST /auth/login` and returns the bearer token.
    ///
    /// # Panics
    ///
    /// Panics, showing the raw body, unless the sign-on answers 200 with a
    /// string `token`.
    pub async fn login(&self, user: &str, password: &str) -> String {
        let body = json!({ "user_id": user, "password": password });
        let (status, json, raw) = self.post(LOGIN_PATH, None, &body).await;
        assert_eq!(status, StatusCode::OK, "sign-on of {user} failed: {raw}");
        json["token"]
            .as_str()
            .unwrap_or_else(|| panic!("the sign-on of {user} returned no string token: {raw}"))
            .to_owned()
    }

    /// The bearer token of the seeded admin user `ADMIN001` (type `A`).
    ///
    /// # Panics
    ///
    /// Panics as [`TestApp::login`] does.
    pub async fn admin_token(&self) -> String {
        self.login(ADMIN_USER, SEED_PASSWORD).await
    }

    /// The bearer token of the seeded regular user `USER0001` (type `U`).
    ///
    /// # Panics
    ///
    /// Panics as [`TestApp::login`] does.
    pub async fn user_token(&self) -> String {
        self.login(REGULAR_USER, SEED_PASSWORD).await
    }
}

/// Sends `builder`, with `token` as the bearer token when given, and reads the
/// whole response.
///
/// `builder` comes from [`TestApp::client`], so the exchange runs under
/// [`CONNECT_TIMEOUT`] and, from the start of the connection until the body
/// has been read, under [`REQUEST_TIMEOUT`], or under the request's own bound
/// when `builder` sets one with `RequestBuilder::timeout`. The helpers of
/// [`TestApp`] call it; a test calls it directly for a request it shapes
/// itself.
///
/// # Panics
///
/// Panics if the request cannot be built or sent or its body cannot be read.
/// When a bound expires, the panic names the stage (connecting, waiting for
/// the response, or reading the body), the URL and the bound that expired.
pub async fn send(builder: RequestBuilder, token: Option<&str>) -> Resp {
    let builder = match token {
        Some(token) => builder.bearer_auth(token),
        None => builder,
    };
    let (client, request) = builder.build_split();
    let request = request.unwrap_or_else(|error| panic!("the test request failed: {error}"));
    let url = request.url().clone();
    let (bound, origin) = match request.timeout() {
        Some(own) => (*own, "the request's own timeout"),
        None => (REQUEST_TIMEOUT, "the harness REQUEST_TIMEOUT"),
    };
    let response = client.execute(request).await.unwrap_or_else(|error| {
        let url = error.url().unwrap_or(&url);
        if error.is_timeout() && error.is_connect() {
            panic!(
                "the test request to {url} timed out connecting: no connection within \
                 {CONNECT_TIMEOUT:?}, the harness CONNECT_TIMEOUT ({error})"
            )
        } else if error.is_timeout() {
            panic!(
                "the test request to {url} timed out waiting for the response: no response \
                 within {bound:?}, {origin} ({error})"
            )
        } else {
            panic!("the test request failed: {error}")
        }
    });
    let status = response.status();
    let url = response.url().clone();
    let text = response.text().await.unwrap_or_else(|error| {
        if error.is_timeout() {
            panic!(
                "the test request to {url} timed out reading the response body: the {status} \
                 response was not complete within {bound:?} of the request start, {origin} \
                 ({error})"
            )
        } else {
            panic!("reading the test response body failed: {error}")
        }
    });
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

/// Asserts that `resp` is the error answer `status` with the `{code, message,
/// fields}` body: a non-empty `code`, exactly `message`, and the `field`
/// values of `fields`, in order.
///
/// `message` is compared untrimmed, because the COBOL texts keep their
/// trailing dots and spaces. An absent, `null` or empty `fields` equals an
/// empty `fields` slice.
///
/// # Panics
///
/// Panics, showing the raw body, on the first part that differs, and if
/// `fields` in the body is neither absent, `null` nor an array of objects with
/// a string `field`.
pub fn assert_error(resp: &Resp, status: StatusCode, message: &str, fields: &[&str]) {
    let (actual_status, body, raw) = resp;
    assert_eq!(*actual_status, status, "status of the error answer: {raw}");
    assert!(
        body["code"].as_str().is_some_and(|code| !code.is_empty()),
        "the error body needs a non-empty string `code`: {raw}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(message),
        "`message` of the error body: {raw}"
    );
    let named: Vec<&str> = match &body["fields"] {
        Value::Null => Vec::new(),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                entry["field"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a `fields` entry has no string `field`: {raw}"))
            })
            .collect(),
        other => panic!("`fields` of the error body is not an array but {other}: {raw}"),
    };
    assert_eq!(named, fields, "`fields` of the error body: {raw}");
}

/// The repository checkout the audits are read from.
fn repo() -> Repo {
    Repo::new(fixtures::repo_root())
}

/// The parsed `docs/parity/validation-audit.md` (`V-` rows).
///
/// # Panics
///
/// Panics, naming the path, if the document cannot be read.
#[must_use]
pub fn validation_audit() -> Document {
    repo()
        .validation_audit()
        .unwrap_or_else(|error| panic!("{VALIDATION_AUDIT_PATH} cannot be read: {error}"))
}

/// The parsed `docs/parity/parity-oracle-audit.md` (`P-` rows).
///
/// # Panics
///
/// Panics, naming the path, if the document cannot be read.
#[must_use]
pub fn parity_audit() -> Document {
    repo()
        .parity_audit()
        .unwrap_or_else(|error| panic!("{PARITY_AUDIT_PATH} cannot be read: {error}"))
}

/// The `V-<program>-nnn` IDs the validation audit defines, in document order
/// and without duplicates, for a test to assert that it exercised every row
/// of its program. Review logs, whose first column is `Row ID`, define none.
///
/// # Panics
///
/// Panics if the document cannot be read or defines no row for `program`.
#[must_use]
pub fn validation_ids(program: &str) -> Vec<String> {
    let doc = validation_audit();
    let prefix = format!("V-{program}-");
    let mut seen = BTreeSet::new();
    let ids: Vec<String> = doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| &table.rows)
        .filter_map(|row| row.cells.first())
        .map(|cell| cell.trim())
        .filter(|id| {
            id.starts_with(&prefix) && carddemo_audit::classify_id(id) == Some(IdKind::Validation)
        })
        .filter(|id| seen.insert(*id))
        .map(str::to_owned)
        .collect();
    assert!(
        !ids.is_empty(),
        "{VALIDATION_AUDIT_PATH} defines no {prefix}nnn row"
    );
    ids
}
