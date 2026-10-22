//! Binding g1b for CTLI / COTRTLIC and CTTU / COTRTUPC, endpoints 24 to 27 (AAP 0.3.4.1).
//!
//! Every `server` row of `modern/audit/online/COTRTLIC.md` and `COTRTUPC.md`, and the server
//! half of every `both` row there and of the COTRTLI and COTRTUP rows of `modern/audit/screens.md`,
//! is proved here through the HTTP endpoints. Each test function gets its own test-support
//! Postgres container, with the schema built and the fixtures loaded. Each proving assertion
//! carries its `// audit:` tag, and the literals are copied from the audits.
//!
//! - Data version (E23): only the Db2 version, `db2.transaction_type` and its categories, is read
//!   or written; the VSAM version that `tranrept` reads is only compared before and after.
//! - The Db2 seed is migration 0006's and is never re-seeded: types `01` to `07`, every one with
//!   categories, so a successful delete always targets a type this file adds without categories.
//! - Halt H3 (E19): the text a Db2 message carries after its fixed prefix is Db2's, so the prefix
//!   is asserted and nothing after it; `blocked_h3_db2_messages` reports the halted rows `BLOCKED`.
//! - E28: the selection-clearing branch of `1210-EDIT-ARRAY` is never taken, so a selection keeps
//!   its row position when a changed filter brings a new page (COTRTLIC-019, COTRTLIC-041).
//!
//! # Store failures
//!
//! The rows a failing statement decides are driven against the real repositories, never a test
//! double. A closed pool stands for a database the service cannot reach. A second pool, opened
//! through `runtime::db::connect`, serves the API with one session setting:
//!
//! - `lock_timeout` or `statement_timeout`, while another transaction holds the row lock or an
//!   uncommitted insert of the same key: the API's statement then fails with SQLSTATE `55P03`,
//!   the -911 class the programs test, or `57014`, a failure of no class they name;
//! - `role` `pg_monitor`, which holds no privilege on schema `db2`: every read fails with
//!   `42501` while the database is reachable.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app};
use api::dto::FIELD_LENGTH_MESSAGE;
use api::dto::cotrtlic::{
    Aid as ListAid, TranTypeListActionRequest, TranTypeListActionResult, TranTypeListCommitted,
    TranTypeListFilterStatus, TranTypeListPendingAction, TranTypeListResponse,
    TranTypeListRowInput, TranTypeListSelectors, TranTypeListShownRow, TranTypeListState,
};
use api::dto::cotrtupc::{TranTypeActionResult, TranTypeMaintState, TranTypeView};
use api::error::ScreenError;
use axum::Router;
use axum::body::{self, Body};
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use domain::clock::{Clock, FixedClock};
use domain::copybook::PicX;
use domain::records::Db2TransactionType;
use persistence::{
    Db2TransactionTypeCategoryRepository, Db2TransactionTypeRepository, PgStore, PgTx,
    TransactionTypeRepository,
};
use runtime::config::Config;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use test_support::fixtures::start_loaded_postgres;
use test_support::postgres::PostgresGuard;
use tower::ServiceExt as _;

type TestResult = Result<(), Box<dyn Error>>;
type Outcome<T> = Result<T, Box<dyn Error>>;

/// The signing secret of every state this file builds: a local test value, never a credential.
const SECRET: &str = "endpoints-transaction-types-secret";

/// The password of `ADMIN001` and `USER0001` in the `DUSRSECJ` deck (`README.md:203-204`).
const PASSWORD: &str = "PASSWORD";

const LIST: &str = "/api/v1/transaction-types";
const LIST_ACTIONS: &str = "/api/v1/transaction-types/list-actions";

// COTRTLIC literals (`modern/audit/online/COTRTLIC.md`).
const RECORD_ACTIONS: &str = "Type U to update, D to delete any record";
const DELETE_PROMPT: &str = "Delete HIGHLIGHTED row ? Press F10 to confirm";
const UPDATE_PROMPT: &str = "Update HIGHLIGHTED row. Press F10 to save";
const ROW_DELETED: &str = "HIGHLIGHTED row deleted.Hit Enter to continue";
const ROW_UPDATED: &str = "HIGHLIGHTED row was updated";
const NO_RECORDS_FOUND: &str = "No records found for this search condition.";
const NO_MORE_PAGES_FOR_SEARCH: &str = "No more pages for these search conditions";
const NO_MORE_PAGES: &str = "No more pages to display";
const NO_PREVIOUS_PAGES: &str = "No previous pages to display";
const ONLY_ONE_ACTION: &str = "Please select only 1 action";
const INVALID_ACTION: &str = "Action code selected is invalid";
const NO_CHANGE_DB: &str = "No change detected with respect to database values.";
const DESC_REQUIRED: &str = "Transaction Desc must be supplied.";
const DESC_ALPHANUM: &str = "Transaction Desc can have numbers or alphabets only.";
const TYPE_FILTER_NOT_NUMERIC: &str = "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER";
const NO_RECORDS_FOR_FILTERS: &str = "No Records found for these filter conditions";
// COTRTLIC's Db2 messages up to and including ` SQLCODE:`; the rest is halt H3 (COTRTLIC-054).
const LIST_DELETE_RESTRICTED: &str = "Please delete associated child records first: SQLCODE:";
const LIST_UPDATE_NOT_FOUND: &str = "Record not found. Deleted by others ? SQLCODE:";
const LIST_DEADLOCK: &str = "Deadlock. Someone else updating ? SQLCODE:";
const LIST_UPDATE_FAILED: &str = "Update failed with SQLCODE:";
const LIST_DELETE_FAILED: &str = "Delete failed with message: SQLCODE:";
const FORWARD_OPEN: &str = "C-TR-TYPE-FORWARD Open SQLCODE:";
const BACKWARD_OPEN: &str = "C-TR-TYPE-BACKWARD Open SQLCODE:";
const BACKWARD_FETCH: &str = "Error on fetch Cursor C-TR-TYPE-BACKWARD SQLCODE:";
const ACCESS_FAILURE: &str = "Db2 access failure. SQLCODE:";

// COTRTUPC literals (`modern/audit/online/COTRTUPC.md`).
const SEARCH_PROMPT: &str = "Enter transaction type to be maintained";
const ADD_PROMPT: &str = "Press F05 to add. F12 to cancel";
const NOT_FOUND_KEY: &str = "No record found for this key in database";
const VALIDATED: &str = "Changes validated.Press F5 to save";
const COMMITTED: &str = "Changes committed to database";
const DELETE_DONE: &str = "Delete successful.";
const NO_CHANGE_FETCHED: &str = "No change detected with respect to values fetched.";
const INVALID_KEY: &str = "Invalid key pressed";
const CODE_SUPPLIED: &str = "Tran Type code must be supplied.";
const CODE_NUMERIC: &str = "Tran Type code must be numeric.";
const CODE_NOT_ZERO: &str = "Tran Type code must not be zero.";
const COULD_NOT_LOCK: &str = "Could not lock record for update";
const UPDATE_ERROR: &str = "Error updating: TRANSACTION_TYPE Table. SQLCODE:";
const INSERT_ERROR: &str = "Error inserting record into: TRANSACTION_TYPE Table. SQLCODE:";
const FETCH_ERROR: &str = "Error accessing: TRANSACTION_TYPE table. SQLCODE:";
const MAINT_DELETE_RESTRICTED: &str = "Please delete associated child records first:SQLCODE :";
const MAINT_DELETE_FAILED: &str = "Delete failed with message:SQLCODE :";

/// The seeded page: migration 0006's seven types in code order (`DB2LTTYP.ctl:15-23`).
const SEED_CODES: [&str; 7] = ["01", "02", "03", "04", "05", "06", "07"];

/// The codes of the seven positions of a page, as [`codes`] reads them.
fn listed(shown: &[&str]) -> Vec<String> {
    shown.iter().map(|&code| code.to_owned()).collect()
}

/// The seeded page.
fn seed_page() -> Vec<String> {
    listed(&SEED_CODES)
}

/// The codes of a page that shows one row at the top.
fn first_only(code: &str) -> Vec<String> {
    listed(&[code, "", "", "", "", "", ""])
}

/// `text` space-padded to the 50 bytes a CTTU or CTLI save writes (COTRTLIC-044, COTRTUPC-023).
fn padded(text: &str) -> String {
    format!("{text:<50}")
}

/// The fixed instant of every request: 2022-07-18 10:30:00.000.
fn clock() -> Outcome<Arc<dyn Clock>> {
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or("an invalid test instant")?;
    Ok(Arc::new(FixedClock::new(at)))
}

/// The API over `store`, signing with [`SECRET`], so a token of one router serves every other.
fn router_over(store: PgStore) -> Outcome<Router> {
    let config = Config {
        jwt_secret: SECRET.to_owned(),
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints_transaction_types"),
        ..Config::default()
    };
    Ok(app(AppState::from_config(store, &config, clock()?)?))
}

/// Every byte of `raw` that is not an ASCII letter or digit, percent-encoded: a path segment or
/// a query value that axum decodes back to `raw`.
fn encoded(raw: &str) -> String {
    raw.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// A request with `token` as bearer token when one is given, and `body` as JSON when one is
/// given.
fn request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Outcome<Request<Body>> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(json)?))?,
        None => builder.body(Body::empty())?,
    };
    Ok(request)
}

/// The status and body of `request` sent to `router`.
async fn send(router: &Router, request: Request<Body>) -> Outcome<(StatusCode, Vec<u8>)> {
    let response = router.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, bytes.to_vec()))
}

/// The bearer token endpoint 1 issues to `user_id` signing on with [`PASSWORD`].
async fn sign_on(router: &Router, user_id: &str) -> Outcome<String> {
    let body = json!({ "user_id": user_id, "password": PASSWORD });
    let (status, bytes) = send(
        router,
        request("POST", "/api/v1/signon", None, Some(&body))?,
    )
    .await?;
    if status != StatusCode::OK {
        return Err(format!("sign-on of {user_id} answered {status}").into());
    }
    let answer: Value = serde_json::from_slice(&bytes)?;
    Ok(answer
        .get("token")
        .and_then(Value::as_str)
        .ok_or("the sign-on answer holds no token")?
        .to_owned())
}

/// The codes of the seven positions shown, empty for an empty position.
fn codes(rows: &[Option<TranTypeListShownRow>]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.as_ref()
                .map_or_else(String::new, |row| row.type_code.clone())
        })
        .collect()
}

/// The descriptions shown at the seven positions, empty for an empty position.
fn shown_descriptions(rows: &[Option<TranTypeListShownRow>]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.as_ref()
                .map_or_else(String::new, |row| row.description.clone())
        })
        .collect()
}

/// The selections shown at the seven positions, empty for an empty position or a blank one.
fn selections(rows: &[Option<TranTypeListShownRow>]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.as_ref()
                .map_or_else(String::new, |row| row.selection.clone())
        })
        .collect()
}

/// The row errors of an endpoint 25 result as `(row, field)`.
fn flagged(result: &TranTypeListActionResult) -> Vec<(i64, &str)> {
    result
        .row_errors
        .iter()
        .map(|error| (error.row, error.field.as_str()))
        .collect()
}

/// An endpoint 25 body over `state`: the key, the filters, then `(row number, selection,
/// description)` for each row that is not blank.
fn action(
    aid: ListAid,
    state: &TranTypeListState,
    filters: (&str, &str),
    entries: &[(usize, &str, &str)],
) -> TranTypeListActionRequest {
    let rows = (1..=7)
        .map(|number| {
            entries
                .iter()
                .find(|(wanted, ..)| *wanted == number)
                .map_or_else(TranTypeListRowInput::default, |&(_, selection, text)| {
                    TranTypeListRowInput {
                        selection: selection.to_owned(),
                        description: text.to_owned(),
                    }
                })
        })
        .collect();
    TranTypeListActionRequest {
        aid,
        type_filter: filters.0.to_owned(),
        desc_filter: filters.1.to_owned(),
        rows,
        state: state.clone(),
        page_size: None,
    }
}

/// One API router and the bearer token every call of it carries.
struct Client {
    router: Router,
    token: String,
}

impl Client {
    /// The status and raw body of a call.
    async fn raw(
        &self,
        method: &str,
        uri: &str,
        body: Option<&Value>,
    ) -> Outcome<(StatusCode, Vec<u8>)> {
        send(&self.router, request(method, uri, Some(&self.token), body)?).await
    }

    /// The status and decoded body of a call.
    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        uri: &str,
        body: Option<&Value>,
    ) -> Outcome<(StatusCode, T)> {
        let (status, bytes) = self.raw(method, uri, body).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    /// Endpoint 24 without `direction`: the screen load (COTRTLIC-002).
    async fn load(&self) -> Outcome<(StatusCode, TranTypeListResponse)> {
        self.call("GET", LIST, None).await
    }

    /// Endpoint 24: F8 (`forward`) or F7 (`backward`) from `page_key`, with the filters on the
    /// screen.
    async fn page(
        &self,
        page_key: &str,
        direction: &str,
        filters: (&str, &str),
    ) -> Outcome<(StatusCode, TranTypeListResponse)> {
        let uri = format!(
            "{LIST}?page_key={page_key}&direction={direction}&type_filter={}&desc_filter={}",
            encoded(filters.0),
            encoded(filters.1)
        );
        self.call("GET", &uri, None).await
    }

    /// Endpoint 25 with `body`.
    async fn act(
        &self,
        body: &TranTypeListActionRequest,
    ) -> Outcome<(StatusCode, TranTypeListActionResult)> {
        self.call("POST", LIST_ACTIONS, Some(&serde_json::to_value(body)?))
            .await
    }

    /// Endpoint 26 for the code typed as `raw`.
    async fn fetch(&self, raw: &str) -> Outcome<(StatusCode, Vec<u8>)> {
        self.raw("GET", &format!("{LIST}/{}", encoded(raw)), None)
            .await
    }

    /// Endpoint 27 for the code shown as `raw`, with the key `aid` and the description typed.
    async fn maintain(
        &self,
        raw: &str,
        aid: &str,
        description: Option<&str>,
    ) -> Outcome<(StatusCode, Vec<u8>)> {
        let body = json!({ "aid": aid, "description": description });
        self.raw(
            "POST",
            &format!("{LIST}/{}/actions", encoded(raw)),
            Some(&body),
        )
        .await
    }
}

/// A fixture-loaded `postgres:16` container, the store on it, and the API called with the
/// `ADMIN001` token that endpoint 1 issued. Dropping it removes the container.
struct Harness {
    guard: PostgresGuard,
    store: PgStore,
    admin: Client,
}

impl Harness {
    async fn start() -> Outcome<Self> {
        let guard = start_loaded_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let router = router_over(store.clone())?;
        let token = sign_on(&router, "ADMIN001").await?;
        Ok(Self {
            guard,
            store,
            admin: Client { router, token },
        })
    }

    /// The same API called with the token endpoint 1 issues to `USER0001`, user type `U`.
    async fn user(&self) -> Outcome<Client> {
        let router = self.admin.router.clone();
        let token = sign_on(&router, "USER0001").await?;
        Ok(Client { router, token })
    }

    /// The API over a second pool on the same database whose sessions run with the server
    /// setting `setting` at `value`, called with the admin token.
    async fn client_with(&self, setting: &str, value: &str) -> Outcome<Client> {
        let config = Config {
            database_url: format!("{}?options[{setting}]={value}", self.guard.database_url()),
            ..Config::default()
        };
        let pool = runtime::db::connect(&config).await?;
        Ok(Client {
            router: router_over(PgStore::new(pool))?,
            token: self.admin.token.clone(),
        })
    }

    /// Adds a type without categories, as another program would.
    async fn insert(&self, code: &str, description: &str) -> TestResult {
        let row = Db2TransactionType::new(PicX::from_text(code)?, description.to_owned())?;
        let mut store = self.store.clone();
        Db2TransactionTypeRepository::insert(&mut store, &row).await?;
        Ok(())
    }

    /// Adds the two-digit codes `first..=last`, each described as `description`.
    async fn insert_codes(&self, first: u32, last: u32, description: &str) -> TestResult {
        for number in first..=last {
            self.insert(&format!("{number:02}"), description).await?;
        }
        Ok(())
    }

    /// Changes the description of `code`, as another user would.
    async fn rename(&self, code: &str, description: &str) -> TestResult {
        let row = Db2TransactionType::new(PicX::from_text(code)?, description.to_owned())?;
        let mut store = self.store.clone();
        if Db2TransactionTypeRepository::update(&mut store, &row).await? {
            Ok(())
        } else {
            Err(format!("no row {code} to rename").into())
        }
    }

    /// A store on a pool of its own, for reading back once the harness pool is closed.
    async fn reopened(&self) -> Outcome<PgStore> {
        let config = Config {
            database_url: self.guard.database_url().to_owned(),
            ..Config::default()
        };
        Ok(PgStore::new(runtime::db::connect(&config).await?))
    }

    /// Removes the type `code`, as another user would; whether a row was removed.
    async fn remove(&self, code: &str) -> Outcome<bool> {
        let mut store = self.store.clone();
        Ok(Db2TransactionTypeRepository::delete(&mut store, &PicX::from_text(code)?).await?)
    }

    /// The stored description of `code` in `db2.transaction_type`, `None` when no row has it.
    async fn stored(&self, code: &str) -> Outcome<Option<String>> {
        stored_in(&self.store, code).await
    }

    /// How many categories `db2.transaction_type_category` holds for `code`.
    async fn categories(&self, code: &str) -> Outcome<usize> {
        let mut store = self.store.clone();
        let rows = Db2TransactionTypeCategoryRepository::find_all_by_type(
            &mut store,
            &PicX::from_text(code)?,
        )
        .await?;
        Ok(rows.len())
    }

    /// A transaction of another user that has updated `code` and holds its row lock until it
    /// ends.
    async fn hold_update(&self, code: &str) -> Outcome<PgTx<'static>> {
        let mut tx = self.store.begin().await?;
        let row = Db2TransactionType::new(PicX::from_text(code)?, "HELD BY ANOTHER".to_owned())?;
        if !Db2TransactionTypeRepository::update(&mut tx, &row).await? {
            return Err(format!("no row {code} to hold").into());
        }
        Ok(tx)
    }

    /// A transaction of another user that has inserted `code` and not yet committed it.
    async fn hold_insert(&self, code: &str) -> Outcome<PgTx<'static>> {
        let mut tx = self.store.begin().await?;
        let row = Db2TransactionType::new(PicX::from_text(code)?, "INSERTED ELSEWHERE".to_owned())?;
        Db2TransactionTypeRepository::insert(&mut tx, &row).await?;
        Ok(tx)
    }
}

/// A 200 answer decoded as `T`.
fn ok<T: DeserializeOwned>((status, bytes): (StatusCode, Vec<u8>)) -> Outcome<T> {
    if status != StatusCode::OK {
        return Err(format!(
            "expected 200, got {status}: {}",
            String::from_utf8_lossy(&bytes)
        )
        .into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// A `ScreenError` answer as the screen reads it: status, message and field.
fn refused(
    (status, bytes): (StatusCode, Vec<u8>),
) -> Outcome<(StatusCode, String, Option<String>)> {
    let error: ScreenError = serde_json::from_slice(&bytes)?;
    Ok((
        status,
        error.message().to_owned(),
        error.field().map(str::to_owned),
    ))
}

/// The expected `(status, message, field)` of a refused answer.
fn refusal(
    status: StatusCode,
    message: &str,
    field: Option<&str>,
) -> (StatusCode, String, Option<String>) {
    (status, message.to_owned(), field.map(str::to_owned))
}

/// The stored description of `code` in `db2.transaction_type` read through `store`, `None` when
/// no row has it.
async fn stored_in(store: &PgStore, code: &str) -> Outcome<Option<String>> {
    let mut store = store.clone();
    let row = Db2TransactionTypeRepository::find_by_id(&mut store, &PicX::from_text(code)?).await?;
    Ok(row.map(|row| row.tr_description().to_owned()))
}

/// The two-digit codes `first..=last`.
fn code_range(first: u32, last: u32) -> Vec<String> {
    (first..=last)
        .map(|number| format!("{number:02}"))
        .collect()
}

/// The JSON a serialisable body travels as.
fn json_of(value: &impl Serialize) -> Outcome<Value> {
    Ok(serde_json::to_value(value)?)
}

/// The screen load of the seeded table: page 1 of the seven types, the record-actions prompt and
/// an empty state but for the rows read and their keys.
fn assert_first_entry(status: StatusCode, loaded: &TranTypeListResponse) {
    // audit: COTRTLIC-002
    assert_eq!(status, StatusCode::OK);
    assert_eq!(codes(&loaded.rows), SEED_CODES);
    let seeded = [
        "PURCHASE",
        "PAYMENT",
        "CREDIT",
        "AUTHORIZATION",
        "REFUND",
        "REVERAL",
        "ADJUSTMENT",
    ];
    assert_eq!(shown_descriptions(&loaded.rows), seeded);
    assert_eq!(loaded.page_no, 1);
    assert_eq!(loaded.message, None);
    assert_eq!(loaded.info_message.as_deref(), Some(RECORD_ACTIONS));
    let state = &loaded.state;
    assert_eq!(
        (state.type_filter.as_str(), state.desc_filter.as_str()),
        ("", "")
    );
    assert_eq!(
        (state.selected_row, state.pending_action),
        (None, TranTypeListPendingAction::None)
    );
    // audit: COTRTLIC-004
    let state_rows: Vec<&str> = state
        .rows
        .iter()
        .flatten()
        .map(|row| row.type_code.as_str())
        .collect();
    assert_eq!(state_rows, SEED_CODES);
    assert_eq!(
        (state.first_key.as_deref(), state.last_key.as_deref()),
        (Some("01"), Some("07"))
    );
    assert!(!state.next_page_exists && !loaded.has_next_page);
}

// Endpoints 24 to 27 serve an `A` token only: 403 for `U`, 401 without a valid token, and the
// refused requests read and write nothing.
#[tokio::test]
async fn admin_gate_guards_endpoints_24_to_27() -> TestResult {
    let harness = Harness::start().await?;
    let user = harness.user().await?;
    let (status, loaded) = harness.admin.load().await?;
    assert_eq!(status, StatusCode::OK);
    let list_body = json_of(&action(ListAid::Enter, &loaded.state, ("", ""), &[]))?;
    let maint_body = json!({ "aid": "PF5", "description": "PURCHASES" });
    let calls = [
        ("GET", LIST.to_owned(), None),
        ("POST", LIST_ACTIONS.to_owned(), Some(&list_body)),
        ("GET", format!("{LIST}/01"), None),
        ("POST", format!("{LIST}/01/actions"), Some(&maint_body)),
    ];
    for (method, uri, body) in &calls {
        // audit: COTRTLIC-001
        // audit: COTRTUPC-001
        let (status, _) = user.raw(method, uri, *body).await?;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        for token in [None, Some("not-a-token")] {
            let unsigned = request(method, uri, token, *body)?;
            let (status, _) = send(&harness.admin.router, unsigned).await?;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
        assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    }
    // The admin token is served; the PF5 above is the one write, made only now.
    for (method, uri, body) in &calls {
        let (status, _) = harness.admin.raw(method, uri, *body).await?;
        assert_eq!(status, StatusCode::OK, "{method} {uri}");
    }
    assert_eq!(harness.stored("01").await?, Some(padded("PURCHASES")));
    Ok(())
}

// Endpoint 24: the screen load, the last-page messages, F7 on page 1, the refresh, and the move
// to a second page and back once another user adds a type.
#[tokio::test]
async fn cotrtlic_list_and_paging_rows() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (status, loaded) = api.load().await?;
    assert_first_entry(status, &loaded);

    let (status, again) = api.page(&loaded.page_key, "forward", ("", "")).await?;
    // audit: COTRTLIC-008
    assert_eq!(
        (status, codes(&again.rows), again.page_no),
        (StatusCode::OK, seed_page(), 1)
    );
    assert_eq!(again.message.as_deref(), Some(NO_MORE_PAGES_FOR_SEARCH));
    assert_eq!(again.info_message.as_deref(), Some(RECORD_ACTIONS));
    assert!(again.state.last_page_shown);
    let (status, last) = api.page(&again.page_key, "forward", ("", "")).await?;
    // audit: COTRTLIC-009
    assert_eq!((status, codes(&last.rows)), (StatusCode::OK, seed_page()));
    assert_eq!(
        (last.message.as_deref(), last.info_message),
        (Some(NO_MORE_PAGES), None)
    );
    let (status, first) = api.page(&last.page_key, "backward", ("", "")).await?;
    // audit: COTRTLIC-010
    assert_eq!(
        (status, codes(&first.rows), first.page_no),
        (StatusCode::OK, seed_page(), 1)
    );
    assert_eq!(
        (first.message.as_deref(), first.info_message),
        (Some(NO_PREVIOUS_PAGES), None)
    );

    // Another user renames 03 and adds 08: ENTER re-reads the displayed page.
    harness.rename("03", "CREDIT LINE").await?;
    harness.insert("08", "OBSOLETE").await?;
    let (status, refreshed) = api
        .act(&action(ListAid::Enter, &first.state, ("", ""), &[]))
        .await?;
    // audit: COTRTLIC-006
    assert_eq!(
        (status, codes(&refreshed.rows), refreshed.page_no),
        (StatusCode::OK, seed_page(), 1)
    );
    assert_eq!(shown_descriptions(&refreshed.rows)[2], "CREDIT LINE");
    assert_eq!(
        (refreshed.message, refreshed.info_message.as_deref()),
        (None, Some(RECORD_ACTIONS))
    );
    assert_eq!(refreshed.committed, TranTypeListCommitted::Nothing);
    // audit: COTRTLIC-004
    assert!(refreshed.state.next_page_exists && refreshed.has_next_page);
    assert_eq!(refreshed.state.last_key.as_deref(), Some("08"));

    let (status, second) = api.page(&refreshed.page_key, "forward", ("", "")).await?;
    // audit: COTRTLIC-007
    assert_eq!(
        (status, codes(&second.rows), second.page_no),
        (StatusCode::OK, first_only("08"), 2)
    );
    assert_eq!(second.message.as_deref(), Some(NO_MORE_PAGES_FOR_SEARCH));
    assert_eq!(second.info_message.as_deref(), Some(RECORD_ACTIONS));
    assert!(second.state.last_page_shown && !second.has_next_page);
    // audit: COTRTLIC-004
    assert_eq!(
        (second.state.first_key.as_deref(), second.state.last_key),
        (Some("08"), None)
    );
    let (status, back) = api.page(&second.page_key, "backward", ("", "")).await?;
    // audit: COTRTLIC-011
    assert_eq!(
        (status, codes(&back.rows), back.page_no),
        (StatusCode::OK, seed_page(), 1)
    );
    assert_eq!(
        (back.message.as_deref(), back.info_message),
        (Some(NO_PREVIOUS_PAGES), None)
    );
    let keys = (
        back.state.first_key.as_deref(),
        back.state.last_key.as_deref(),
    );
    assert_eq!(keys, (Some("01"), Some("08")));
    assert!(back.state.next_page_exists);
    Ok(())
}

// Endpoint 24 under a description filter: F8 through three pages, F7 back, and F7 once another
// user's deletes leave fewer than seven rows before the page.
#[tokio::test]
async fn cotrtlic_backward_paging_and_short_reads() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert_codes(10, 30, "EXTRA TYPE").await?;
    let filters = ("", "EXTRA");
    let (_, loaded) = api.load().await?;
    let (status, first) = api
        .act(&action(ListAid::Enter, &loaded.state, filters, &[]))
        .await?;
    // audit: COTRTLIC-005
    // audit: COTRTLIC-035
    assert_eq!(
        (status, codes(&first.rows), first.page_no),
        (StatusCode::OK, code_range(10, 16), 1)
    );
    assert!(first.has_next_page);
    let (status, second) = api.page(&first.page_key, "forward", filters).await?;
    // audit: COTRTLIC-007
    assert_eq!(
        (status, codes(&second.rows), second.page_no),
        (StatusCode::OK, code_range(17, 23), 2)
    );
    assert_eq!(
        (second.message, second.info_message.as_deref()),
        (None, Some(RECORD_ACTIONS))
    );
    let (_, third) = api.page(&second.page_key, "forward", filters).await?;
    // audit: COTRTLIC-007
    assert_eq!((codes(&third.rows), third.page_no), (code_range(24, 30), 3));
    assert_eq!(third.message.as_deref(), Some(NO_MORE_PAGES_FOR_SEARCH));
    let (_, again) = api.page(&third.page_key, "forward", filters).await?;
    // audit: COTRTLIC-009
    assert_eq!((codes(&again.rows), again.page_no), (code_range(24, 30), 3));
    assert_eq!(
        (again.message.as_deref(), again.info_message),
        (Some(NO_MORE_PAGES), None)
    );

    let (status, back) = api.page(&again.page_key, "backward", filters).await?;
    // audit: COTRTLIC-005
    // audit: COTRTLIC-011
    assert_eq!(
        (status, codes(&back.rows), back.page_no),
        (StatusCode::OK, code_range(17, 23), 2)
    );
    assert_eq!(
        (back.message, back.info_message.as_deref()),
        (None, Some(RECORD_ACTIONS))
    );
    let keys = (
        back.state.first_key.as_deref(),
        back.state.last_key.as_deref(),
    );
    assert_eq!(keys, (Some("17"), Some("24")));
    assert!(back.has_next_page);
    let (_, top) = api.page(&back.page_key, "backward", filters).await?;
    // audit: COTRTLIC-011
    assert_eq!((codes(&top.rows), top.page_no), (code_range(10, 16), 1));
    assert_eq!(
        (top.message.as_deref(), top.info_message),
        (Some(NO_PREVIOUS_PAGES), None)
    );

    // Another user deletes 10 to 17: only six rows lie before page 3 now.
    for number in 10..=17 {
        assert!(harness.remove(&format!("{number:02}")).await?);
    }
    let mut bottom_up = vec![String::new()];
    bottom_up.extend(code_range(18, 23));
    let (status, short) = api.page(&again.page_key, "backward", filters).await?;
    // audit: COTRTLIC-012
    // audit: COTRTLIC-054
    assert_eq!(
        (status, codes(&short.rows), short.page_no),
        (StatusCode::INTERNAL_SERVER_ERROR, bottom_up.clone(), 2)
    );
    assert_eq!(short.message.as_deref(), Some(BACKWARD_FETCH));
    assert_eq!(short.info_message.as_deref(), Some(RECORD_ACTIONS));
    let keys = (
        short.state.first_key.as_deref(),
        short.state.last_key.as_deref(),
    );
    assert_eq!(keys, (Some("24"), Some("24")));
    let (status, short_top) = api.page(&short.page_key, "backward", filters).await?;
    // audit: COTRTLIC-012
    assert_eq!(
        (status, codes(&short_top.rows), short_top.page_no),
        (StatusCode::OK, bottom_up, 1)
    );
    assert_eq!(
        (short_top.message.as_deref(), short_top.info_message),
        (Some(NO_PREVIOUS_PAGES), None)
    );
    Ok(())
}

// Endpoint 24 past page 9 (the page number keeps one digit), then ENTER on page 1 after another
// user deleted every row from its first key on.
#[tokio::test]
async fn cotrtlic_page_number_wraps_and_an_emptied_first_page() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert_codes(8, 70, "WRAP TYPE").await?;
    let (_, mut page) = api.load().await?;
    for number in 2..=9 {
        let (status, next) = api.page(&page.page_key, "forward", ("", "")).await?;
        assert_eq!((status, next.page_no), (StatusCode::OK, number));
        page = next;
    }
    assert_eq!(codes(&page.rows), code_range(57, 63));
    let (status, tenth) = api.page(&page.page_key, "forward", ("", "")).await?;
    // audit: COTRTLIC-013
    assert_eq!(
        (status, codes(&tenth.rows), tenth.page_no),
        (StatusCode::OK, code_range(64, 70), 1)
    );
    assert_eq!(tenth.message.as_deref(), Some(NO_MORE_PAGES_FOR_SEARCH));
    let (status, refused_back) = api.page(&tenth.page_key, "backward", ("", "")).await?;
    // audit: COTRTLIC-010
    // audit: COTRTLIC-013
    assert_eq!(
        (status, codes(&refused_back.rows), refused_back.page_no),
        (StatusCode::OK, code_range(64, 70), 1)
    );
    assert_eq!(
        (refused_back.message.as_deref(), refused_back.info_message),
        (Some(NO_PREVIOUS_PAGES), None)
    );

    for number in 64..=70 {
        assert!(harness.remove(&format!("{number:02}")).await?);
    }
    let (status, empty) = api
        .act(&action(ListAid::Enter, &refused_back.state, ("", ""), &[]))
        .await?;
    // audit: COTRTLIC-003
    assert_eq!(
        (status, codes(&empty.rows)),
        (StatusCode::OK, listed(&[""; 7]))
    );
    assert_eq!(
        (empty.message.as_deref(), empty.info_message),
        (Some(NO_RECORDS_FOUND), None)
    );
    assert!(!empty.has_next_page);
    Ok(())
}

// Endpoint 25: a valid filter is counted, then read; the type filter is an equality, the
// description filter a case-sensitive `LIKE '%text%'` of the trimmed text, and both are ANDed.
#[tokio::test]
async fn cotrtlic_valid_filters_select_by_type_and_like_pattern() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert("08", "NEW  TYPE").await?;
    let (_, loaded) = api.load().await?;
    let cases = [
        (("03", ""), first_only("03")),
        (("", "PAY"), first_only("02")),
        (("", "P_Y"), first_only("02")),
        (("", "  PAYMENT  "), first_only("02")),
        (("", "NEW  TYPE"), first_only("08")),
        (("06", "R"), first_only("06")),
        (("", "R"), listed(&["01", "03", "04", "05", "06", "", ""])),
        (("", "%"), seed_page()),
    ];
    for (filters, expected) in cases {
        let (status, page) = api
            .act(&action(ListAid::Enter, &loaded.state, filters, &[]))
            .await?;
        // audit: COTRTLIC-005
        // audit: COTRTLIC-029
        // audit: COTRTLIC-030
        // audit: COTRTLIC-035
        assert_eq!(
            (status, codes(&page.rows), page.page_no),
            (StatusCode::OK, expected, 1),
            "{filters:?}"
        );
        assert_eq!(page.state.type_filter, filters.0, "{filters:?}");
        assert_eq!(page.state.desc_filter, filters.1.trim_end(), "{filters:?}");
        assert_eq!(
            page.info_message.as_deref(),
            Some(RECORD_ACTIONS),
            "{filters:?}"
        );
    }
    // An unchanged filter keeps its page: F8 on its only page re-reads it under the same filter.
    let (_, by_type) = api
        .act(&action(ListAid::Enter, &loaded.state, ("03", ""), &[]))
        .await?;
    let (status, kept) = api.page(&by_type.page_key, "forward", ("03", "")).await?;
    // audit: COTRTLIC-029
    assert_eq!(
        (status, codes(&kept.rows), kept.page_no),
        (StatusCode::OK, first_only("03"), 1)
    );
    assert_eq!(kept.message.as_deref(), Some(NO_MORE_PAGES_FOR_SEARCH));
    // `00` and a blank type filter add no predicate; with blank selections the filter shows empty.
    for raw in ["00", "  ", ""] {
        let (status, page) = api
            .act(&action(ListAid::Enter, &loaded.state, (raw, ""), &[]))
            .await?;
        // audit: COTRTLIC-027
        assert_eq!(
            (status, codes(&page.rows)),
            (StatusCode::OK, seed_page()),
            "{raw:?}"
        );
        assert_eq!(page.type_filter.text, "", "{raw:?}");
        assert_eq!(
            page.type_filter.status,
            TranTypeListFilterStatus::Blank,
            "{raw:?}"
        );
    }
    // A changed filter on F8 moves to page 1 of it: endpoint 24 runs the filter edits too.
    let (status, moved) = api
        .page(&loaded.page_key, "forward", ("", "REFUND"))
        .await?;
    // audit: COTRTLIC-030
    // audit: COTRTLIC-035
    assert_eq!(
        (status, codes(&moved.rows), moved.page_no),
        (StatusCode::OK, first_only("05"), 1)
    );
    Ok(())
}

// Endpoint 25 and 24: a type filter that is not two digits, a filter that matches nothing, and
// which message wins when several are raised.
#[tokio::test]
async fn cotrtlic_filter_edits_and_filters_matching_nothing() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    for raw in ["A1", "1", "1 ", "+1"] {
        let body = action(ListAid::Enter, &loaded.state, (raw, ""), &[(1, "X", "")]);
        let (status, page) = api.act(&body).await?;
        // audit: COTRTLIC-028
        // audit: COTRTLIC-033
        // audit: COTRTLIC-034
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw:?}");
        assert_eq!(
            page.message.as_deref(),
            Some(TYPE_FILTER_NOT_NUMERIC),
            "{raw:?}"
        );
        assert_eq!(
            (page.info_message, page.type_filter.status),
            (None, TranTypeListFilterStatus::InError)
        );
        assert_eq!(
            (codes(&page.rows), selections(&page.rows)[0].as_str()),
            (seed_page(), "X")
        );
        assert_eq!(
            page.state.selectors,
            TranTypeListSelectors::Protected,
            "{raw:?}"
        );
        assert_eq!(
            (page.state.page_no, page.state.type_filter.as_str()),
            (0, raw.trim_end())
        );
        assert_eq!(page.committed, TranTypeListCommitted::Nothing);
    }
    let (status, paged) = api.page(&loaded.page_key, "forward", ("A1", "")).await?;
    // audit: COTRTLIC-028
    assert_eq!(
        (status, paged.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(TYPE_FILTER_NOT_NUMERIC))
    );

    // A valid filter that matches nothing: the description match is case-sensitive and interior
    // spaces count.
    for filters in [("09", ""), ("", "pay"), ("", "PUR CHASE"), ("03", "PAY")] {
        let body = action(ListAid::Enter, &loaded.state, filters, &[(2, "D", "")]);
        let (status, page) = api.act(&body).await?;
        // audit: COTRTLIC-005
        // audit: COTRTLIC-031
        assert_eq!(status, StatusCode::BAD_REQUEST, "{filters:?}");
        assert_eq!(
            (page.message.as_deref(), page.info_message),
            (Some(NO_RECORDS_FOR_FILTERS), None)
        );
        assert_eq!(codes(&page.rows), seed_page(), "{filters:?}");
        assert_eq!((page.page_no, page.state.page_no), (0, 0), "{filters:?}");
        assert_eq!(
            page.state.selectors,
            TranTypeListSelectors::Protected,
            "{filters:?}"
        );
        let flagged_filter = if filters.0.is_empty() {
            &page.desc_filter
        } else {
            &page.type_filter
        };
        assert_eq!(
            flagged_filter.status,
            TranTypeListFilterStatus::InError,
            "{filters:?}"
        );
        // The single valid selection still becomes pending at its position in those rows.
        assert_eq!(
            page.state.pending_action,
            TranTypeListPendingAction::Delete,
            "{filters:?}"
        );
        assert_eq!(page.state.selected_row, Some(2), "{filters:?}");
    }
    // The no-match message replaces the selection message.
    let body = action(ListAid::Enter, &loaded.state, ("09", ""), &[(3, "X", "")]);
    let (status, page) = api.act(&body).await?;
    // audit: COTRTLIC-033
    assert_eq!(
        (status, page.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(NO_RECORDS_FOR_FILTERS))
    );
    Ok(())
}

// Endpoint 25, AAP cases 7 and 8: invalid action codes, two actions, and a selection where no
// row is shown.
#[tokio::test]
async fn cotrtlic_list_actions_case_invalid_and_double_selections() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let start = &loaded.state;
    for code in ["X", "d", "u"] {
        let (status, result) = api
            .act(&action(ListAid::Enter, start, ("", ""), &[(2, code, "")]))
            .await?;
        // audit: COTRTLIC-020
        // audit: COTRTLIC-021
        // audit: COTRTLIC-034
        assert_eq!(
            (status, result.message.as_deref()),
            (StatusCode::BAD_REQUEST, Some(INVALID_ACTION))
        );
        assert_eq!(
            result.info_message.as_deref(),
            Some(RECORD_ACTIONS),
            "{code}"
        );
        assert_eq!(flagged(&result), [(2, "selection")], "{code}");
        assert_eq!(
            (codes(&result.rows), selections(&result.rows)[1].as_str()),
            (seed_page(), code)
        );
        assert_eq!(
            result.state.pending_action,
            TranTypeListPendingAction::None,
            "{code}"
        );
    }
    let body = action(
        ListAid::Enter,
        start,
        ("", ""),
        &[(1, "D", ""), (3, "U", "")],
    );
    let (status, two) = api.act(&body).await?;
    // audit: COTRTLIC-022
    assert_eq!(
        (status, two.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(ONLY_ONE_ACTION))
    );
    assert_eq!(two.info_message.as_deref(), Some(RECORD_ACTIONS));
    assert_eq!(flagged(&two), [(1, "selection"), (3, "selection")]);
    assert_eq!(two.state.pending_action, TranTypeListPendingAction::None);
    // Filter 03 shows one row, so a selection on row 2 is never received.
    let (_, by_type) = api
        .act(&action(ListAid::Enter, start, ("03", ""), &[]))
        .await?;
    let (status, unseen) = api
        .act(&action(
            ListAid::Enter,
            &by_type.state,
            ("03", ""),
            &[(2, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-018
    assert_eq!(
        (status, unseen.message, unseen.info_message.as_deref()),
        (StatusCode::OK, None, Some(RECORD_ACTIONS))
    );
    assert_eq!(
        (unseen.state.pending_action, unseen.state.selected_row),
        (TranTypeListPendingAction::None, None)
    );

    Ok(())
}

// Endpoint 25, AAP case 9 and the `U` description edits: the first `U` finds no change, an
// unchanged description stays no change, and a changed one must be supplied and alphanumeric.
#[tokio::test]
async fn cotrtlic_list_actions_case_update_description_edits() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let start = &loaded.state;
    // The first `U` finds no change: its description is protected until this response.
    let (status, opened) = api
        .act(&action(
            ListAid::Enter,
            start,
            ("", ""),
            &[(1, "U", "Ignored")],
        ))
        .await?;
    // audit: COTRTLIC-018
    // audit: COTRTLIC-024
    // audit: COTRTLIC-040
    assert_eq!(
        (status, opened.message.as_deref()),
        (StatusCode::OK, Some(NO_CHANGE_DB))
    );
    assert_eq!(opened.info_message.as_deref(), Some(UPDATE_PROMPT));
    assert_eq!(
        (opened.state.pending_action, opened.state.selected_row),
        (TranTypeListPendingAction::Update, Some(1))
    );
    assert_eq!(
        (
            opened.state.description_entry,
            shown_descriptions(&opened.rows)[0].as_str()
        ),
        (Some(1), "PURCHASE")
    );
    let row = opened
        .rows
        .first()
        .and_then(Option::as_ref)
        .ok_or("row 1 is empty")?;
    assert!(row.highlighted && row.description_enterable);
    let body = action(
        ListAid::Enter,
        &opened.state,
        ("", ""),
        &[(1, "U", "  purchase ")],
    );
    let (status, same) = api.act(&body).await?;
    // audit: COTRTLIC-017
    // audit: COTRTLIC-024
    assert_eq!(
        (status, same.message.as_deref()),
        (StatusCode::OK, Some(NO_CHANGE_DB))
    );
    // A changed description must be supplied and alphanumeric; the update stays pending.
    for (typed, message, shown) in [
        ("*", DESC_REQUIRED, "*"),
        ("   ", DESC_REQUIRED, "*"),
        ("Buy-now", DESC_ALPHANUM, "Buy-now"),
    ] {
        let body = action(ListAid::Enter, &opened.state, ("", ""), &[(1, "U", typed)]);
        let (status, refused_edit) = api.act(&body).await?;
        // audit: COTRTLIC-017
        // audit: COTRTLIC-025
        // audit: COTRTLIC-026
        // audit: COTRTLIC-040
        assert_eq!(
            (status, refused_edit.message.as_deref()),
            (StatusCode::BAD_REQUEST, Some(message))
        );
        assert_eq!(
            refused_edit.info_message.as_deref(),
            Some(UPDATE_PROMPT),
            "{typed:?}"
        );
        assert_eq!(
            (
                flagged(&refused_edit),
                shown_descriptions(&refused_edit.rows)[0].as_str()
            ),
            (vec![(1, "description")], shown)
        );
        assert_eq!(
            refused_edit.state.pending_action,
            TranTypeListPendingAction::Update
        );
    }
    // F10 while the description stays blank is an input error and commits nothing.
    let (_, blank) = api
        .act(&action(
            ListAid::Enter,
            &opened.state,
            ("", ""),
            &[(1, "U", "*")],
        ))
        .await?;
    let (status, kept) = api
        .act(&action(
            ListAid::Pf10,
            &blank.state,
            ("", ""),
            &[(1, "U", "*")],
        ))
        .await?;
    // audit: COTRTLIC-025
    // audit: COTRTLIC-034
    assert_eq!(
        (status, kept.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(DESC_REQUIRED))
    );
    assert_eq!(kept.committed, TranTypeListCommitted::Nothing);
    assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    Ok(())
}

// Endpoint 25, AAP cases 1 and 3: ENTER and F10 after a changed type filter with blank
// selections, then a `D` on the old page's row 1 together with a changed type filter.
#[tokio::test]
async fn cotrtlic_list_actions_case_changed_type_filter() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert("08", "OBSOLETE").await?;
    let (_, loaded) = api.load().await?;
    let (status, entered) = api
        .act(&action(ListAid::Enter, &loaded.state, ("03", ""), &[]))
        .await?;
    // audit: COTRTLIC-029
    // audit: COTRTLIC-035
    assert_eq!(
        (status, codes(&entered.rows), entered.page_no),
        (StatusCode::OK, first_only("03"), 1)
    );
    assert_eq!(
        (entered.message, entered.info_message.as_deref()),
        (None, Some(RECORD_ACTIONS))
    );
    assert_eq!(
        (entered.state.type_filter.as_str(), entered.committed),
        ("03", TranTypeListCommitted::Nothing)
    );
    let (status, confirmed) = api
        .act(&action(ListAid::Pf10, &entered.state, ("05", ""), &[]))
        .await?;
    // audit: COTRTLIC-035
    // audit: COTRTLIC-036
    // audit: COTRTLIC-037
    assert_eq!(
        (status, codes(&confirmed.rows), confirmed.page_no),
        (StatusCode::OK, first_only("05"), 1)
    );
    assert_eq!(
        (confirmed.state.type_filter.as_str(), confirmed.committed),
        ("05", TranTypeListCommitted::Nothing)
    );
    assert_eq!(confirmed.info_message.as_deref(), Some(RECORD_ACTIONS));

    // The `D` is edited against the old page, where row 1 is 01; the new page holds 08 there.
    let (status, kept) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("08", ""),
            &[(1, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-019
    // audit: COTRTLIC-029
    // audit: COTRTLIC-041
    assert_eq!(
        (status, codes(&kept.rows), kept.page_no),
        (StatusCode::OK, first_only("08"), 1)
    );
    assert_eq!(selections(&kept.rows)[0], "D");
    assert_eq!(
        (kept.state.pending_action, kept.state.selected_row),
        (TranTypeListPendingAction::Delete, Some(1))
    );
    assert_eq!((kept.message, kept.info_message), (None, None));
    assert_eq!(harness.stored("08").await?.as_deref(), Some("OBSOLETE"));
    let (status, deleted) = api
        .act(&action(
            ListAid::Pf10,
            &kept.state,
            ("08", ""),
            &[(1, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-036
    // audit: COTRTLIC-042
    // audit: COTRTLIC-049
    assert_eq!(
        (status, deleted.committed),
        (StatusCode::OK, TranTypeListCommitted::Deleted)
    );
    assert_eq!(deleted.info_message.as_deref(), Some(ROW_DELETED));
    // No re-read: the deleted row is still listed, its selection blank.
    assert_eq!(
        (codes(&deleted.rows), selections(&deleted.rows)[0].as_str()),
        (first_only("08"), "")
    );
    assert_eq!(
        (
            harness.stored("08").await?,
            harness.stored("01").await?.as_deref()
        ),
        (None, Some("PURCHASE"))
    );
    // The state is reset as for a first entry.
    let reset = &deleted.state;
    assert_eq!(
        (
            reset.type_filter.as_str(),
            reset.selected_row,
            reset.page_no
        ),
        ("", None, 1)
    );
    assert_eq!(
        (reset.pending_action, reset.first_key.as_deref()),
        (TranTypeListPendingAction::None, None)
    );
    assert!(reset.rows.iter().all(|row| {
        row.as_ref()
            .is_none_or(|row| row.type_code.is_empty() && row.description.is_empty())
    }));
    let (status, after) = api
        .act(&action(ListAid::Enter, reset, ("", ""), &[]))
        .await?;
    // audit: COTRTLIC-049
    assert_eq!(
        (status, codes(&after.rows), after.page_no),
        (StatusCode::OK, seed_page(), 1)
    );
    Ok(())
}

// Endpoint 25, AAP cases 2 and 4: ENTER and F10 after a changed description filter with blank
// selections, then a `U` with an edited description together with a changed description filter.
#[tokio::test]
async fn cotrtlic_list_actions_case_changed_description_filter() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let (status, entered) = api
        .act(&action(ListAid::Enter, &loaded.state, ("", "CREDIT"), &[]))
        .await?;
    // audit: COTRTLIC-030
    // audit: COTRTLIC-035
    assert_eq!(
        (status, codes(&entered.rows), entered.page_no),
        (StatusCode::OK, first_only("03"), 1)
    );
    assert_eq!(
        (entered.state.desc_filter.as_str(), entered.committed),
        ("CREDIT", TranTypeListCommitted::Nothing)
    );
    assert_eq!(
        (entered.message, entered.info_message.as_deref()),
        (None, Some(RECORD_ACTIONS))
    );
    let (status, confirmed) = api
        .act(&action(ListAid::Pf10, &entered.state, ("", "REFUND"), &[]))
        .await?;
    // audit: COTRTLIC-030
    // audit: COTRTLIC-035
    // audit: COTRTLIC-036
    // audit: COTRTLIC-037
    assert_eq!(
        (status, codes(&confirmed.rows)),
        (StatusCode::OK, first_only("05"))
    );
    assert_eq!(
        (confirmed.state.desc_filter.as_str(), confirmed.committed),
        ("REFUND", TranTypeListCommitted::Nothing)
    );

    // The description of a `U` is enterable only after the ENTER that selects it.
    let (_, opened) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("", ""),
            &[(1, "U", "")],
        ))
        .await?;
    assert_eq!(opened.state.description_entry, Some(1));
    let body = action(
        ListAid::Enter,
        &opened.state,
        ("", "PAYMENT"),
        &[(1, "U", "PAYMENT due")],
    );
    let (status, moved) = api.act(&body).await?;
    // audit: COTRTLIC-018
    // audit: COTRTLIC-019
    // audit: COTRTLIC-030
    // audit: COTRTLIC-041
    assert_eq!(
        (status, codes(&moved.rows), moved.page_no),
        (StatusCode::OK, first_only("02"), 1)
    );
    assert_eq!(
        (
            shown_descriptions(&moved.rows)[0].as_str(),
            selections(&moved.rows)[0].as_str()
        ),
        ("PAYMENT due", "U")
    );
    assert_eq!(
        (moved.state.pending_action, moved.state.description_entry),
        (TranTypeListPendingAction::Update, Some(1))
    );
    assert_eq!((moved.message, moved.info_message), (None, None));
    assert_eq!(harness.stored("02").await?.as_deref(), Some("PAYMENT"));
    let body = action(
        ListAid::Pf10,
        &moved.state,
        ("", "PAYMENT"),
        &[(1, "U", "  PAYMENT due  ")],
    );
    let (status, saved) = api.act(&body).await?;
    // audit: COTRTLIC-042
    // audit: COTRTLIC-044
    // audit: COTRTLIC-045
    assert_eq!(
        (status, saved.committed),
        (StatusCode::OK, TranTypeListCommitted::Updated)
    );
    assert_eq!(
        (saved.message, saved.info_message.as_deref()),
        (None, Some(ROW_UPDATED))
    );
    assert_eq!(
        (codes(&saved.rows), selections(&saved.rows)[0].as_str()),
        (first_only("02"), "")
    );
    assert_eq!(shown_descriptions(&saved.rows)[0], "PAYMENT due");
    assert_eq!(saved.state.pending_action, TranTypeListPendingAction::None);
    // Trimmed, in the case typed, padded to 50; the row first selected is untouched.
    assert_eq!(harness.stored("02").await?, Some(padded("PAYMENT due")));
    assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    Ok(())
}

// Endpoint 25, AAP cases 11 and 5: F10 with nothing pending, and F10 with a pending action and
// a changed filter, each processed as ENTER.
#[tokio::test]
async fn cotrtlic_list_actions_case_f10_processed_as_enter() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert("08", "OBSOLETE").await?;
    let (_, loaded) = api.load().await?;
    let (status, refreshed) = api
        .act(&action(ListAid::Pf10, &loaded.state, ("", ""), &[]))
        .await?;
    // audit: COTRTLIC-006
    // audit: COTRTLIC-037
    assert_eq!(
        (status, codes(&refreshed.rows), refreshed.committed),
        (StatusCode::OK, seed_page(), TranTypeListCommitted::Nothing)
    );
    assert_eq!(
        (refreshed.message, refreshed.info_message.as_deref()),
        (None, Some(RECORD_ACTIONS))
    );
    let (status, prompted) = api
        .act(&action(
            ListAid::Pf10,
            &loaded.state,
            ("", ""),
            &[(4, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-037
    // audit: COTRTLIC-039
    assert_eq!(
        (status, prompted.info_message.as_deref()),
        (StatusCode::OK, Some(DELETE_PROMPT))
    );
    assert_eq!(
        (prompted.state.pending_action, prompted.state.selected_row),
        (TranTypeListPendingAction::Delete, Some(4))
    );
    assert_eq!(
        (prompted.committed, selections(&prompted.rows)[3].as_str()),
        (TranTypeListCommitted::Nothing, "D")
    );
    assert!(
        prompted
            .rows
            .get(3)
            .and_then(Option::as_ref)
            .is_some_and(|row| row.highlighted)
    );
    assert_eq!(
        harness.stored("04").await?.as_deref(),
        Some("AUTHORIZATION")
    );

    // A pending delete of 08, then F10 with the type filter changed back to blank.
    let (_, by_type) = api
        .act(&action(ListAid::Enter, &loaded.state, ("08", ""), &[]))
        .await?;
    let (_, pending) = api
        .act(&action(
            ListAid::Enter,
            &by_type.state,
            ("08", ""),
            &[(1, "D", "")],
        ))
        .await?;
    assert_eq!(
        (
            pending.info_message.as_deref(),
            pending.state.pending_action
        ),
        (Some(DELETE_PROMPT), TranTypeListPendingAction::Delete)
    );
    let (status, downgraded) = api
        .act(&action(
            ListAid::Pf10,
            &pending.state,
            ("", ""),
            &[(1, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-038
    // audit: COTRTLIC-041
    assert_eq!(
        (status, codes(&downgraded.rows), downgraded.committed),
        (StatusCode::OK, seed_page(), TranTypeListCommitted::Nothing)
    );
    assert_eq!(
        (
            selections(&downgraded.rows)[0].as_str(),
            downgraded.info_message
        ),
        ("D", None)
    );
    assert_eq!(
        downgraded.state.pending_action,
        TranTypeListPendingAction::Delete
    );
    assert_eq!(harness.stored("08").await?.as_deref(), Some("OBSOLETE"));

    Ok(())
}

// Endpoint 25, AAP case 6: a changed filter with an invalid action code, and a valid changed
// filter that matches no rows, whose protected selectors make the next F10 an ENTER.
#[tokio::test]
async fn cotrtlic_list_actions_case_changed_filter_input_errors() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    // A changed filter with an invalid action code re-reads page 1 of the new filter.
    let (status, invalid) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("03", ""),
            &[(1, "X", "")],
        ))
        .await?;
    // audit: COTRTLIC-021
    // audit: COTRTLIC-034
    assert_eq!(
        (status, invalid.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(INVALID_ACTION))
    );
    assert_eq!(
        (codes(&invalid.rows), selections(&invalid.rows)[0].as_str()),
        (first_only("03"), "X")
    );
    assert_eq!(
        (invalid.state.type_filter.as_str(), invalid.page_no),
        ("03", 1)
    );
    // A valid changed filter matching no rows protects the selectors, so the next F10 is ENTER.
    let (status, nothing) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("09", ""),
            &[(2, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-031
    assert_eq!(
        (status, nothing.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(NO_RECORDS_FOR_FILTERS))
    );
    let (status, after) = api
        .act(&action(
            ListAid::Pf10,
            &nothing.state,
            ("09", ""),
            &[(2, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-018
    // audit: COTRTLIC-031
    assert_eq!(
        (status, after.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(NO_RECORDS_FOR_FILTERS))
    );
    assert_eq!(after.committed, TranTypeListCommitted::Nothing);
    Ok(())
}

// Endpoint 25, AAP case 10, the update: a confirmed `U` with the filters unchanged, and an
// unchanged description rewritten.
#[tokio::test]
async fn cotrtlic_list_actions_case_confirmed_update() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let (_, opened) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("", ""),
            &[(2, "U", "")],
        ))
        .await?;
    let (status, edited) = api
        .act(&action(
            ListAid::Enter,
            &opened.state,
            ("", ""),
            &[(2, "U", "Paid out")],
        ))
        .await?;
    // audit: COTRTLIC-040
    assert_eq!(
        (status, edited.message, edited.info_message.as_deref()),
        (StatusCode::OK, None, Some(UPDATE_PROMPT))
    );
    assert_eq!(shown_descriptions(&edited.rows)[1], "Paid out");
    assert_eq!(harness.stored("02").await?.as_deref(), Some("PAYMENT"));
    let (status, saved) = api
        .act(&action(
            ListAid::Pf10,
            &edited.state,
            ("", ""),
            &[(2, "U", "Paid out")],
        ))
        .await?;
    // audit: COTRTLIC-036
    // audit: COTRTLIC-044
    // audit: COTRTLIC-045
    assert_eq!(
        (status, saved.committed, saved.info_message.as_deref()),
        (
            StatusCode::OK,
            TranTypeListCommitted::Updated,
            Some(ROW_UPDATED)
        )
    );
    assert_eq!(
        (
            shown_descriptions(&saved.rows)[1].as_str(),
            selections(&saved.rows)[1].as_str()
        ),
        ("Paid out", "")
    );
    assert_eq!(
        (saved.state.pending_action, saved.page_no),
        (TranTypeListPendingAction::None, 1)
    );
    assert_eq!(harness.stored("02").await?, Some(padded("Paid out")));

    // An unchanged description is still written, at 50 bytes, beside the no-change message.
    let (_, credit) = api
        .act(&action(
            ListAid::Enter,
            &saved.state,
            ("", ""),
            &[(3, "U", "")],
        ))
        .await?;
    let (status, rewritten) = api
        .act(&action(
            ListAid::Pf10,
            &credit.state,
            ("", ""),
            &[(3, "U", "CREDIT")],
        ))
        .await?;
    // audit: COTRTLIC-024
    // audit: COTRTLIC-045
    assert_eq!(
        (status, rewritten.committed),
        (StatusCode::OK, TranTypeListCommitted::Updated)
    );
    assert_eq!(
        (
            rewritten.message.as_deref(),
            rewritten.info_message.as_deref()
        ),
        (Some(NO_CHANGE_DB), Some(ROW_UPDATED))
    );
    assert_eq!(harness.stored("03").await?, Some(padded("CREDIT")));

    Ok(())
}

// Endpoint 25, AAP case 10, the delete: a confirmed `D` with the filters unchanged, and a pending
// update overtaken by a `D` at the same row.
#[tokio::test]
async fn cotrtlic_list_actions_case_confirmed_delete() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert("08", "OBSOLETE").await?;
    harness.insert("09", "RETIRED").await?;
    let (_, loaded) = api.load().await?;
    let (_, by_type) = api
        .act(&action(ListAid::Enter, &loaded.state, ("09", ""), &[]))
        .await?;
    let (status, prompted) = api
        .act(&action(
            ListAid::Enter,
            &by_type.state,
            ("09", ""),
            &[(1, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-039
    assert_eq!(
        (status, prompted.info_message.as_deref()),
        (StatusCode::OK, Some(DELETE_PROMPT))
    );
    assert_eq!(
        (prompted.state.pending_action, prompted.committed),
        (
            TranTypeListPendingAction::Delete,
            TranTypeListCommitted::Nothing
        )
    );
    assert_eq!(harness.stored("09").await?.as_deref(), Some("RETIRED"));
    let (status, deleted) = api
        .act(&action(
            ListAid::Pf10,
            &prompted.state,
            ("09", ""),
            &[(1, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-036
    // audit: COTRTLIC-049
    assert_eq!(
        (status, deleted.committed, deleted.info_message.as_deref()),
        (
            StatusCode::OK,
            TranTypeListCommitted::Deleted,
            Some(ROW_DELETED)
        )
    );
    assert_eq!(harness.stored("09").await?, None);

    // The F10 gate compares the selected position only: a `D` typed over a pending `U` deletes.
    let (_, by_type) = api
        .act(&action(ListAid::Enter, &loaded.state, ("08", ""), &[]))
        .await?;
    let (_, update) = api
        .act(&action(
            ListAid::Enter,
            &by_type.state,
            ("08", ""),
            &[(1, "U", "")],
        ))
        .await?;
    assert_eq!(
        update.state.pending_action,
        TranTypeListPendingAction::Update
    );
    let (status, swapped) = api
        .act(&action(
            ListAid::Pf10,
            &update.state,
            ("08", ""),
            &[(1, "D", "")],
        ))
        .await?;
    // audit: COTRTLIC-023
    // audit: COTRTLIC-043
    assert_eq!(
        (status, swapped.committed, swapped.info_message.as_deref()),
        (
            StatusCode::OK,
            TranTypeListCommitted::Deleted,
            Some(ROW_DELETED)
        )
    );
    assert_eq!(harness.stored("08").await?, None);
    Ok(())
}

// Endpoint 25, AAP case 12: a delete the categories' `ON DELETE RESTRICT` refuses, tried again
// by a repeated F10.
#[tokio::test]
async fn cotrtlic_list_actions_case_restricted_delete() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let (_, prompted) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("", ""),
            &[(5, "D", "")],
        ))
        .await?;
    let mut state = prompted.state;
    for attempt in 1..=2 {
        let (status, refused_delete) = api
            .act(&action(ListAid::Pf10, &state, ("", ""), &[(5, "D", "")]))
            .await?;
        // audit: COTRTLIC-050
        // audit: COTRTLIC-054
        assert_eq!(
            (status, refused_delete.message.as_deref()),
            (StatusCode::BAD_REQUEST, Some(LIST_DELETE_RESTRICTED)),
            "F10 {attempt}"
        );
        assert_eq!(
            (
                refused_delete.info_message.as_deref(),
                refused_delete.committed
            ),
            (Some(RECORD_ACTIONS), TranTypeListCommitted::Nothing)
        );
        assert_eq!(
            (
                codes(&refused_delete.rows),
                selections(&refused_delete.rows)[4].as_str()
            ),
            (seed_page(), "D")
        );
        assert_eq!(
            refused_delete.state.pending_action,
            TranTypeListPendingAction::Delete
        );
        assert_eq!(harness.stored("05").await?.as_deref(), Some("REFUND"));
        assert_eq!(harness.categories("05").await?, 1);
        state = refused_delete.state;
    }

    Ok(())
}

// Endpoint 25 commits on rows another user removed between ENTER and F10: the update and the
// delete each find no row (+100).
#[tokio::test]
async fn cotrtlic_commits_on_rows_another_user_removed() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    // 00 sorts first: the page re-read after the failed update still has rows.
    harness.insert("00", "TEMPORARY").await?;
    let (_, reloaded) = api.load().await?;
    let (_, opened) = api
        .act(&action(
            ListAid::Enter,
            &reloaded.state,
            ("", ""),
            &[(1, "U", "")],
        ))
        .await?;
    let (_, edited) = api
        .act(&action(
            ListAid::Enter,
            &opened.state,
            ("", ""),
            &[(1, "U", "Kept")],
        ))
        .await?;
    assert!(harness.remove("00").await?);
    let (status, gone) = api
        .act(&action(
            ListAid::Pf10,
            &edited.state,
            ("", ""),
            &[(1, "U", "Kept")],
        ))
        .await?;
    // audit: COTRTLIC-046
    // audit: COTRTLIC-054
    assert_eq!(
        (status, gone.message.as_deref()),
        (StatusCode::NOT_FOUND, Some(LIST_UPDATE_NOT_FOUND))
    );
    assert_eq!(
        (gone.committed, gone.state.pending_action),
        (
            TranTypeListCommitted::Nothing,
            TranTypeListPendingAction::Update
        )
    );
    assert_eq!(
        (
            codes(&gone.rows),
            shown_descriptions(&gone.rows)[0].as_str()
        ),
        (seed_page(), "Kept")
    );
    assert_eq!(harness.stored("00").await?, None);

    // Under a type filter the cross edit runs first: the filter matches nothing any more, an
    // input error, so no commit is tried.
    harness.insert("09", "RETIRED").await?;
    let (_, by_type) = api
        .act(&action(ListAid::Enter, &loaded.state, ("09", ""), &[]))
        .await?;
    let delete = [(1, "D", "")];
    let (_, prompted) = api
        .act(&action(ListAid::Enter, &by_type.state, ("09", ""), &delete))
        .await?;
    assert!(harness.remove("09").await?);
    let (status, unmatched) = api
        .act(&action(ListAid::Pf10, &prompted.state, ("09", ""), &delete))
        .await?;
    // audit: COTRTLIC-031
    // audit: COTRTLIC-034
    assert_eq!(
        (status, unmatched.message.as_deref()),
        (StatusCode::BAD_REQUEST, Some(NO_RECORDS_FOR_FILTERS))
    );
    assert_eq!(unmatched.committed, TranTypeListCommitted::Nothing);

    // Unfiltered, the delete itself finds no row; nothing is re-read.
    harness.insert("00", "RETIRED").await?;
    let (_, reloaded) = api.load().await?;
    let (_, prompted) = api
        .act(&action(ListAid::Enter, &reloaded.state, ("", ""), &delete))
        .await?;
    assert_eq!(
        prompted.state.pending_action,
        TranTypeListPendingAction::Delete
    );
    assert!(harness.remove("00").await?);
    let (status, vanished) = api
        .act(&action(ListAid::Pf10, &prompted.state, ("", ""), &delete))
        .await?;
    // audit: COTRTLIC-051
    // audit: COTRTLIC-054
    assert_eq!(
        (status, vanished.message.as_deref()),
        (StatusCode::NOT_FOUND, Some(LIST_DELETE_FAILED))
    );
    assert_eq!(
        (vanished.committed, vanished.state.pending_action),
        (
            TranTypeListCommitted::Nothing,
            TranTypeListPendingAction::Delete
        )
    );
    assert_eq!(
        codes(&vanished.rows),
        listed(&["00", "01", "02", "03", "04", "05", "06"])
    );
    Ok(())
}

// Endpoint 25 commits whose statement fails while another transaction holds the row: a lock
// timeout is the -911 class, a cancelled statement any other failure.
#[tokio::test]
async fn cotrtlic_commits_that_fail_on_a_held_row() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert("08", "OBSOLETE").await?;
    let locking = harness.client_with("lock_timeout", "500ms").await?;
    let timing = harness.client_with("statement_timeout", "500ms").await?;
    let (_, loaded) = api.load().await?;
    let (_, by_type) = api
        .act(&action(ListAid::Enter, &loaded.state, ("08", ""), &[]))
        .await?;
    let (_, opened) = api
        .act(&action(
            ListAid::Enter,
            &by_type.state,
            ("08", ""),
            &[(1, "U", "")],
        ))
        .await?;
    let (_, edited) = api
        .act(&action(
            ListAid::Enter,
            &opened.state,
            ("08", ""),
            &[(1, "U", "Locked out")],
        ))
        .await?;
    let save = action(
        ListAid::Pf10,
        &edited.state,
        ("08", ""),
        &[(1, "U", "Locked out")],
    );
    for (client, message) in [(&locking, LIST_DEADLOCK), (&timing, LIST_UPDATE_FAILED)] {
        let holder = harness.hold_update("08").await?;
        let (status, failed) = client.act(&save).await?;
        holder.rollback().await?;
        // audit: COTRTLIC-047
        // audit: COTRTLIC-048
        // audit: COTRTLIC-054
        assert_eq!(
            (status, failed.message.as_deref()),
            (StatusCode::INTERNAL_SERVER_ERROR, Some(message))
        );
        assert_eq!(
            (failed.committed, failed.state.pending_action),
            (
                TranTypeListCommitted::Nothing,
                TranTypeListPendingAction::Update
            )
        );
        assert_eq!(codes(&failed.rows), first_only("08"));
        assert_eq!(harness.stored("08").await?.as_deref(), Some("OBSOLETE"));
    }
    let (_, prompted) = api
        .act(&action(
            ListAid::Enter,
            &by_type.state,
            ("08", ""),
            &[(1, "D", "")],
        ))
        .await?;
    let holder = harness.hold_update("08").await?;
    let (status, failed) = timing
        .act(&action(
            ListAid::Pf10,
            &prompted.state,
            ("08", ""),
            &[(1, "D", "")],
        ))
        .await?;
    holder.rollback().await?;
    // audit: COTRTLIC-051
    assert_eq!(
        (status, failed.message.as_deref()),
        (StatusCode::INTERNAL_SERVER_ERROR, Some(LIST_DELETE_FAILED))
    );
    assert_eq!(
        (failed.committed, failed.state.pending_action),
        (
            TranTypeListCommitted::Nothing,
            TranTypeListPendingAction::Delete
        )
    );
    assert_eq!(harness.stored("08").await?.as_deref(), Some("OBSOLETE"));
    Ok(())
}

// Endpoints 24 and 25 over a database the API reaches but cannot read: the session runs as
// `pg_monitor`, which holds no privilege on schema `db2`, so the count and the page reads fail
// with SQLSTATE 42501 while the priming query would succeed.
#[tokio::test]
async fn cotrtlic_reads_that_fail_on_a_reachable_database() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert_codes(8, 15, "EXTRA TYPE").await?;
    let (_, loaded) = api.load().await?;
    let (_, second) = api.page(&loaded.page_key, "forward", ("", "")).await?;
    let (_, third) = api.page(&second.page_key, "forward", ("", "")).await?;
    assert_eq!(third.page_no, 3);
    let unprivileged = harness.client_with("role", "pg_monitor").await?;

    let (status, unread) = unprivileged.load().await?;
    // audit: COTRTLIC-052
    // audit: COTRTLIC-054
    assert_eq!(
        (status, unread.message.as_deref()),
        (StatusCode::INTERNAL_SERVER_ERROR, Some(FORWARD_OPEN))
    );
    assert_eq!(codes(&unread.rows), listed(&[""; 7]));
    let (status, back) = unprivileged
        .page(&third.page_key, "backward", ("", ""))
        .await?;
    // audit: COTRTLIC-052
    assert_eq!(
        (status, back.message.as_deref()),
        (StatusCode::INTERNAL_SERVER_ERROR, Some(BACKWARD_OPEN))
    );
    // Onto page 1 the paging message overwrites the Db2 one; the status stays 500.
    let (status, top) = unprivileged
        .page(&second.page_key, "backward", ("", ""))
        .await?;
    // audit: COTRTLIC-033
    // audit: COTRTLIC-052
    assert_eq!(
        (status, top.message.as_deref()),
        (StatusCode::INTERNAL_SERVER_ERROR, Some(NO_PREVIOUS_PAGES))
    );
    // The failed count: its message is halt candidate X-COTRTLIC-H2, so only the status and the
    // absence of a commit are asserted.
    let (status, counted) = unprivileged
        .act(&action(ListAid::Enter, &loaded.state, ("03", ""), &[]))
        .await?;
    // audit: COTRTLIC-032
    assert_eq!(
        (status, counted.committed),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            TranTypeListCommitted::Nothing
        )
    );
    Ok(())
}

// A database the service cannot reach, a closed pool: COTRTLIC's failed `9998-PRIMING-QUERY`
// sends no screen, and COTRTUPC's reads fail with their own messages.
#[tokio::test]
async fn store_unreachable_answers_500_and_commits_nothing() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let (_, prompted) = api
        .act(&action(
            ListAid::Enter,
            &loaded.state,
            ("", ""),
            &[(3, "D", "")],
        ))
        .await?;
    assert_eq!(
        prompted.state.pending_action,
        TranTypeListPendingAction::Delete
    );
    harness.guard.pool().close().await;

    let refresh = json_of(&action(ListAid::Enter, &prompted.state, ("", ""), &[]))?;
    let confirm = json_of(&action(
        ListAid::Pf10,
        &prompted.state,
        ("", ""),
        &[(3, "D", "")],
    ))?;
    for (method, uri, body) in [
        ("GET", LIST, None),
        ("POST", LIST_ACTIONS, Some(&refresh)),
        ("POST", LIST_ACTIONS, Some(&confirm)),
    ] {
        let (status, bytes) = api.raw(method, uri, body).await?;
        let shown: Value = serde_json::from_slice(&bytes)?;
        // audit: COTRTLIC-053
        // audit: COTRTLIC-054
        assert_eq!(
            refused((status, bytes))?,
            refusal(StatusCode::INTERNAL_SERVER_ERROR, ACCESS_FAILURE, None)
        );
        // audit: COTRTLIC-053
        assert!(shown.get("rows").is_none() && shown.get("state").is_none());
    }

    let fetched = refused(api.fetch("01").await?)?;
    // audit: COTRTUPC-011
    assert_eq!(
        fetched,
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            FETCH_ERROR,
            Some("type_code")
        )
    );
    // The re-read endpoint 27 makes in place of the COMMAREA fails: a 500 naming no field. The
    // COBOL's abend data carries no text of its own, so none is asserted.
    let (status, _, field) = refused(api.maintain("01", "PF5", Some("PURCHASES")).await?)?;
    // audit: COTRTUPC-038
    assert_eq!((status, field), (StatusCode::INTERNAL_SERVER_ERROR, None));

    // audit: COTRTLIC-053
    // audit: COTRTUPC-038
    let store = harness.reopened().await?;
    assert_eq!(stored_in(&store, "03").await?.as_deref(), Some("CREDIT"));
    assert_eq!(stored_in(&store, "01").await?.as_deref(), Some("PURCHASE"));
    Ok(())
}

// The map `LENGTH` of every field the two screens send, revalidated by the request DTOs: 400
// naming the field.
#[tokio::test]
async fn map_lengths_are_revalidated_by_the_request_dtos() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let (_, loaded) = api.load().await?;
    let long = "X".repeat(51);
    let too_long =
        |field: &str| refusal(StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(field));
    for (field, value) in [("type_filter", "123"), ("desc_filter", long.as_str())] {
        let answer = api
            .raw("GET", &format!("{LIST}?{field}={}", encoded(value)), None)
            .await?;
        // audit: COTRTLI-015
        // audit: COTRTLI-018
        assert_eq!(refused(answer)?, too_long(field));
    }
    let mut bodies = vec![
        (
            action(ListAid::Enter, &loaded.state, ("123", ""), &[]),
            "type_filter".to_owned(),
        ),
        (
            action(ListAid::Enter, &loaded.state, ("", long.as_str()), &[]),
            "desc_filter".to_owned(),
        ),
    ];
    for row in 1..=7 {
        let index = row - 1;
        let selection = action(ListAid::Enter, &loaded.state, ("", ""), &[(row, "UU", "")]);
        bodies.push((selection, format!("rows[{index}].selection")));
        let description = action(
            ListAid::Enter,
            &loaded.state,
            ("", ""),
            &[(row, "", long.as_str())],
        );
        bodies.push((description, format!("rows[{index}].description")));
    }
    for (body, field) in bodies {
        let answer = api
            .raw("POST", LIST_ACTIONS, Some(&json_of(&body)?))
            .await?;
        // audit: COTRTLI-015
        // audit: COTRTLI-018
        // audit: COTRTLI-026
        // audit: COTRTLI-030
        // audit: COTRTLI-032
        // audit: COTRTLI-036
        // audit: COTRTLI-038
        // audit: COTRTLI-042
        // audit: COTRTLI-044
        // audit: COTRTLI-048
        // audit: COTRTLI-050
        // audit: COTRTLI-054
        // audit: COTRTLI-056
        // audit: COTRTLI-060
        // audit: COTRTLI-062
        // audit: COTRTLI-066
        assert_eq!(refused(answer)?, too_long(&field));
    }
    for answer in [
        api.fetch("123").await?,
        api.maintain("123", "ENTER", Some("NEW")).await?,
    ] {
        // audit: COTRTUP-013
        assert_eq!(refused(answer)?, too_long("type_code"));
    }
    let answer = api.maintain("01", "PF5", Some(long.as_str())).await?;
    // audit: COTRTUP-016
    assert_eq!(refused(answer)?, too_long("description"));
    // Fifty characters fit: the program's own edits answer them.
    let fits: TranTypeActionResult =
        ok(api.maintain("01", "ENTER", Some(&"A".repeat(50))).await?)?;
    // audit: COTRTUP-016
    assert_eq!(fits.state, TranTypeMaintState::ChangesOkNotConfirmed);
    assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    Ok(())
}

// Endpoint 26: the key edits, the normalised key, a type shown and a type no row holds.
#[tokio::test]
async fn cotrtupc_fetch_rows() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let found: TranTypeView = ok(api.fetch("01").await?)?;
    // audit: COTRTUPC-009
    let purchase = TranTypeView {
        type_code: "01".to_owned(),
        description: Some("PURCHASE".to_owned()),
        message: None,
        info_message: SEARCH_PROMPT.to_owned(),
        state: TranTypeMaintState::ShowDetails,
    };
    assert_eq!(found, purchase);
    // Stored trimmed and left-justified, then the two digits of the absolute integer part.
    for raw in [" 1", "1", "1 ", "+1", "-1", "1-", "1."] {
        let view: TranTypeView = ok(api.fetch(raw).await?)?;
        // audit: COTRTUPC-004
        // audit: COTRTUPC-008
        assert_eq!(view, purchase, "{raw:?}");
    }
    // A valid code no row holds offers the add path: a 200, never a 404.
    for (raw, code) in [("09", "09"), (".5", "00")] {
        let view: TranTypeView = ok(api.fetch(raw).await?)?;
        // audit: COTRTUPC-008
        // audit: COTRTUPC-010
        let add = TranTypeView {
            type_code: code.to_owned(),
            description: None,
            message: Some(NOT_FOUND_KEY.to_owned()),
            info_message: ADD_PROMPT.to_owned(),
            state: TranTypeMaintState::DetailsNotFound,
        };
        assert_eq!(view, add, "{raw:?}");
    }
    // The key edits refuse a blank, non-numeric or zero code before any read. COTRTUPC-012's
    // skip of these edits in state X is not asserted: the request carries no state-X marker yet
    // (X-COTRTUPC-1), so its blank-code case alone is proved here.
    let refused_codes: [(&[&str], &str); 3] = [
        (&["*", "  ", "* "], CODE_SUPPLIED),
        (&["AB", "1A", "-", ".", "CR"], CODE_NUMERIC),
        (&["00", "0", "-0", "0.", ".0"], CODE_NOT_ZERO),
    ];
    for (raws, message) in refused_codes {
        for raw in raws {
            let answer = refused(api.fetch(raw).await?)?;
            // audit: COTRTUPC-004
            // audit: COTRTUPC-005
            // audit: COTRTUPC-006
            // audit: COTRTUPC-007
            // audit: COTRTUPC-012
            assert_eq!(
                answer,
                refusal(StatusCode::BAD_REQUEST, message, Some("type_code")),
                "{raw:?}"
            );
        }
    }
    Ok(())
}

// Endpoint 27 ENTER and PF5 on a type a row holds: the no-change and description edits, the
// validated change, the save and the keys the endpoint does not serve.
#[tokio::test]
async fn cotrtupc_maintenance_rows() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    for typed in ["purchase", " PURCHASE "] {
        let answer = refused(api.maintain("01", "ENTER", Some(typed)).await?)?;
        // audit: COTRTUPC-017
        assert_eq!(
            answer,
            refusal(
                StatusCode::BAD_REQUEST,
                NO_CHANGE_FETCHED,
                Some("description")
            )
        );
    }
    let edits = [
        (None, DESC_REQUIRED),
        (Some(""), DESC_REQUIRED),
        (Some("*"), DESC_REQUIRED),
        (Some("Buy-now"), DESC_ALPHANUM),
    ];
    for (typed, message) in edits {
        let answer = refused(api.maintain("01", "ENTER", typed).await?)?;
        // audit: COTRTUPC-004
        // audit: COTRTUPC-018
        // audit: COTRTUPC-019
        assert_eq!(
            answer,
            refusal(StatusCode::BAD_REQUEST, message, Some("description")),
            "{typed:?}"
        );
    }
    let validated: TranTypeActionResult =
        ok(api.maintain("01", "ENTER", Some(" Purchases 2 ")).await?)?;
    // audit: COTRTUPC-016
    // audit: COTRTUPC-020
    let pending = TranTypeActionResult {
        type_code: "01".to_owned(),
        description: Some(padded("Purchases 2")),
        message: None,
        info_message: VALIDATED.to_owned(),
        state: TranTypeMaintState::ChangesOkNotConfirmed,
        committed: false,
        deleted: false,
    };
    assert_eq!(validated, pending);
    assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    // PF5 edits the received code and description again before it writes.
    let resent = [
        ("*", Some("Other"), CODE_SUPPLIED, "type_code"),
        ("01", Some(""), DESC_REQUIRED, "description"),
        ("01", Some("Buy-now"), DESC_ALPHANUM, "description"),
    ];
    for (code, typed, message, field) in resent {
        let answer = refused(api.maintain(code, "PF5", typed).await?)?;
        // audit: COTRTUPC-024
        assert_eq!(
            answer,
            refusal(StatusCode::BAD_REQUEST, message, Some(field)),
            "{code} {typed:?}"
        );
    }
    assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    let saved: TranTypeActionResult = ok(api.maintain("01", "PF5", Some(" Purchases 2 ")).await?)?;
    // audit: COTRTUPC-023
    // audit: COTRTUPC-024
    let done = TranTypeActionResult {
        info_message: COMMITTED.to_owned(),
        state: TranTypeMaintState::ChangesOkayedAndDone,
        committed: true,
        ..pending
    };
    assert_eq!(saved, done);
    assert_eq!(harness.stored("01").await?, Some(padded("Purchases 2")));
    let shown: TranTypeView = ok(api.fetch("01").await?)?;
    // audit: COTRTUPC-009
    assert_eq!(shown.description, Some(padded("Purchases 2")));
    for aid in ["PF12", "PF3", "PF10", "enter"] {
        let answer = refused(api.maintain("01", aid, Some("Other")).await?)?;
        // audit: COTRTUPC-043
        assert_eq!(
            answer,
            refusal(StatusCode::BAD_REQUEST, INVALID_KEY, None),
            "{aid}"
        );
    }
    // A stored code the key edits refuse, as an add in state X can store, is refused while its
    // row exists.
    harness.insert("AB", "ODD CODE").await?;
    let odd = refused(api.maintain("AB", "ENTER", Some("Other")).await?)?;
    // audit: COTRTUPC-016
    assert_eq!(
        odd,
        refusal(StatusCode::BAD_REQUEST, CODE_NUMERIC, Some("type_code"))
    );
    assert_eq!(harness.stored("01").await?, Some(padded("Purchases 2")));
    Ok(())
}

// Endpoint 27 on a type no row holds, and saves that compare nothing with the values fetched;
// the VSAM version of the types never changes.
#[tokio::test]
async fn cotrtupc_add_path_and_unguarded_saves() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    let mut vsam = harness.store.clone();
    let before = TransactionTypeRepository::find_all(&mut vsam).await?;
    let validated: TranTypeActionResult =
        ok(api.maintain("09", "ENTER", Some("New type")).await?)?;
    // audit: COTRTUPC-021
    assert_eq!(
        (validated.type_code.as_str(), validated.state),
        ("09", TranTypeMaintState::ChangesOkNotConfirmed)
    );
    assert_eq!(
        (validated.info_message.as_str(), validated.committed),
        (VALIDATED, false)
    );
    let blank = refused(api.maintain("09", "ENTER", None).await?)?;
    // audit: COTRTUPC-021
    assert_eq!(
        blank,
        refusal(StatusCode::BAD_REQUEST, DESC_REQUIRED, Some("description"))
    );
    assert_eq!(harness.stored("09").await?, None);
    let added: TranTypeActionResult = ok(api.maintain("09", "PF5", Some("New type")).await?)?;
    // audit: COTRTUPC-026
    assert_eq!(
        (added.state, added.info_message.as_str()),
        (TranTypeMaintState::ChangesOkayedAndDone, COMMITTED)
    );
    assert_eq!((added.committed, added.deleted), (true, false));
    assert_eq!(harness.stored("09").await?, Some(padded("New type")));

    // Another user changes the row after the fetch: PF5 overwrites it, and never answers 409.
    let fetched: TranTypeView = ok(api.fetch("09").await?)?;
    assert_eq!(fetched.state, TranTypeMaintState::ShowDetails);
    harness.rename("09", "CHANGED ELSEWHERE").await?;
    let overwritten: TranTypeActionResult = ok(api.maintain("09", "PF5", Some("Mine")).await?)?;
    // audit: COTRTUPC-025
    assert!(overwritten.committed);
    assert_eq!(harness.stored("09").await?, Some(padded("Mine")));
    // Another user deletes it: PF5 inserts it again.
    assert!(harness.remove("09").await?);
    let reinserted: TranTypeActionResult = ok(api.maintain("09", "PF5", Some("Again")).await?)?;
    // audit: COTRTUPC-025
    // audit: COTRTUPC-026
    assert_eq!(
        (reinserted.state, reinserted.committed),
        (TranTypeMaintState::ChangesOkayedAndDone, true)
    );
    assert_eq!(harness.stored("09").await?, Some(padded("Again")));

    let saved: TranTypeActionResult = ok(api.maintain("01", "PF5", Some("Purchases")).await?)?;
    assert!(saved.committed);
    // audit: COTRTUPC-002
    assert_eq!(
        TransactionTypeRepository::find_all(&mut vsam).await?,
        before
    );
    assert_eq!(harness.stored("01").await?, Some(padded("Purchases")));
    Ok(())
}

// Endpoint 27 PF4, the second F4: a type without categories is deleted, a gone one fails, and
// the categories' `ON DELETE RESTRICT` keeps a seeded one.
#[tokio::test]
async fn cotrtupc_delete_rows() -> TestResult {
    let harness = Harness::start().await?;
    let api = &harness.admin;
    harness.insert("09", "RETIRED").await?;
    let deleted: TranTypeActionResult = ok(api.maintain("09", "PF4", None).await?)?;
    // audit: COTRTUPC-032
    let done = TranTypeActionResult {
        type_code: "09".to_owned(),
        description: None,
        message: None,
        info_message: DELETE_DONE.to_owned(),
        state: TranTypeMaintState::DeleteDone,
        committed: true,
        deleted: true,
    };
    assert_eq!(deleted, done);
    assert_eq!(harness.stored("09").await?, None);
    let gone = refused(api.maintain("09", "PF4", None).await?)?;
    // audit: COTRTUPC-034
    assert_eq!(
        gone,
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            MAINT_DELETE_FAILED,
            Some("type_code")
        )
    );
    let restricted = refused(api.maintain("01", "PF4", None).await?)?;
    // audit: COTRTUPC-033
    assert_eq!(
        restricted,
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            MAINT_DELETE_RESTRICTED,
            None
        )
    );
    assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    assert_eq!(harness.categories("01").await?, 5);
    Ok(())
}

// Endpoint 27 writes whose statement fails while another transaction holds the row or an
// uncommitted insert of the same code.
#[tokio::test]
async fn cotrtupc_writes_that_fail_on_a_held_row() -> TestResult {
    let harness = Harness::start().await?;
    let locking = harness.client_with("lock_timeout", "500ms").await?;
    let timing = harness.client_with("statement_timeout", "500ms").await?;
    for (client, message) in [(&locking, COULD_NOT_LOCK), (&timing, UPDATE_ERROR)] {
        let holder = harness.hold_update("01").await?;
        let answer = refused(client.maintain("01", "PF5", Some("Locked out")).await?)?;
        holder.rollback().await?;
        // audit: COTRTUPC-028
        // audit: COTRTUPC-029
        assert_eq!(
            answer,
            refusal(StatusCode::INTERNAL_SERVER_ERROR, message, None)
        );
        assert_eq!(harness.stored("01").await?.as_deref(), Some("PURCHASE"));
    }
    // The update finds no committed 09, and the insert then meets another user's insert of it.
    let holder = harness.hold_insert("09").await?;
    let answer = refused(locking.maintain("09", "PF5", Some("Racing")).await?)?;
    holder.rollback().await?;
    // audit: COTRTUPC-027
    assert_eq!(
        answer,
        refusal(StatusCode::INTERNAL_SERVER_ERROR, INSERT_ERROR, None)
    );
    assert_eq!(harness.stored("09").await?, None);
    harness.insert("08", "OBSOLETE").await?;
    let holder = harness.hold_update("08").await?;
    let answer = refused(timing.maintain("08", "PF4", None).await?)?;
    holder.rollback().await?;
    // audit: COTRTUPC-034
    assert_eq!(
        answer,
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            MAINT_DELETE_FAILED,
            Some("type_code")
        )
    );
    assert_eq!(harness.stored("08").await?.as_deref(), Some("OBSOLETE"));
    Ok(())
}

// audit: COTRTLIC-055
// audit: COTRTUPC-049
#[test]
#[ignore = "BLOCKED(H3): COTRTLIC-055, COTRTUPC-049 — the SQLCODE and the SQLERRM or DSNTIAC text after each Db2 message prefix are defined by Db2 (E19)"]
fn blocked_h3_db2_messages() {}
