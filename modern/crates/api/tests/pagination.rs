//! Bounded Pagination for the API (AAP 0.3.4.1): every collection endpoint takes `page_size`
//! from 1 to 100, reads its program's screen size when the member is absent, answers any other
//! value with 422 and a `ScreenError` naming `page_size`, and 422 answers nothing else. The index
//! half of the rule, one index scan per paginated query, is `crates/persistence/tests/explain.rs`.
//!
//! | # | Request | Program | Screen size | Rows the test database holds |
//! |---|---|---|---|---|
//! | 7 | `GET /api/v1/cards` | COCRDLIC | 7 | the 50 fixture cards |
//! | 11 | `GET /api/v1/transactions` | COTRN00C | 10 | 12 seeded daily records |
//! | 16 | `GET /api/v1/users` `[A]` | COUSR00C | 10 | the 10 fixture users and one added |
//! | 21 | `GET /api/v1/accounts/00000000050/authorizations` | COPAUS0C | 5 | 6 seeded details |
//! | 24 | `GET /api/v1/transaction-types` `[A]` | COTRTLIC | 7 | the 7 Db2 types of migration 0006 |
//! | 25 | `POST /api/v1/transaction-types/list-actions` `[A]` | COTRTLIC | 7 | the same 7 types |
//!
//! A page holds the smaller of the size asked for and the rows stored; COTRTLIC's page is also
//! bounded by the seven positions of its map. The screens never send `page_size`, so the request
//! without it is the paging-parity case: the program's own screen size.
//!
//! Each test starts one fixture-loaded `postgres:16` container of `test_support` and seeds
//! through the repository traits and the API alone, never by SQL text: twelve `dailytran.txt`
//! records through codec A and `TransactionRepository::insert`, one pending-authorization
//! summary with six details for the first cross-reference of `cardxref.txt` (card
//! `0500024453765740`, customer 50, account `00000000050`), and one user through endpoint 17.
//! Requests run in process through `tower::ServiceExt::oneshot` with an `A` token for
//! `ADMIN001`, which every endpoint admits.
//!
//! These are pattern-lane checks of a user rule, so they carry no audit tag. Assertion messages
//! name the endpoint, the page size and the status, never a response body: the card rows are
//! redacted, but a failure text stays clean of every response value (Cardholder Data Redaction).

use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app};
use api::dto::{INVALID_BODY, INVALID_QUERY, PAGE_SIZE_FIELD, PAGE_SIZE_MESSAGE};
use axum::Router;
use axum::body::{self, Body};
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use domain::CardNumber;
use domain::clock::{Clock, FixedClock};
use domain::copybook::framing::decode_all;
use domain::copybook::{Count, PicS, PicU, PicX};
use domain::money::{Decimal, Money};
use domain::records::{
    DailyTransactionRecord, PendingAuthDetailRecord, PendingAuthSummaryRecord, TransactionRecord,
};
use persistence::{
    PendingAuthDetailRepository, PendingAuthSummaryRepository, PgStore, TransactionRepository,
};
use runtime::config::Config;
use serde_json::{Value, json};
use test_support::fixtures::{fixture_path, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use tower::ServiceExt as _;

type TestResult = Result<(), Box<dyn Error>>;

/// An endpoint number and a request that endpoint must refuse.
type Refused = (u8, Request<Body>);

/// An endpoint number, a request whose query or body that endpoint cannot deserialise, and the
/// message of its 400.
type Unreadable = (u8, Request<Body>, &'static str);

/// The HS256 secret of the test state: a local-only value.
const SECRET: &str = "pagination-test-secret";

/// The `dailytran.txt` records seeded into `transactions`: more than COTRN00C's ten rows.
const SEEDED_TRANSACTIONS: usize = 12;

/// The pending-authorization details seeded under account 50: more than COPAUS0C's five rows.
const SEEDED_DETAILS: usize = 6;

/// The account and customer of the first `cardxref.txt` record, the keys of the seeded summary.
const AUTH_ACCOUNT: i64 = 50;
const AUTH_CUSTOMER: u64 = 50;

/// The card of the first `cardxref.txt` record, which the seeded details carry.
const AUTH_CARD: &str = "0500024453765740";

/// `YYDDD` of 2022-07-18, the day of the seeded details, and `HHMMSSmmm` of the first one.
const AUTH_DAY: u64 = 22_199;
const AUTH_TIME: u64 = 103_000_000;

/// The largest page any collection admits, so a cap of this size bounds nothing further.
const MAX_ROWS: usize = 100;

/// The input positions of COTRTLIC's map, which also bound its page.
const MAP_ROWS: usize = 7;

/// Endpoint 25, whose `page_size` travels in the body.
const LIST_ACTIONS: &str = "/api/v1/transaction-types/list-actions";

/// How a collection endpoint is called.
enum Call {
    /// A `GET` of this path, `page_size` in the query string.
    Get(&'static str),
    /// Endpoint 25: an ENTER body over the state endpoint 24 returned, `page_size` in the body.
    ListActions,
}

/// One collection endpoint of Bounded Pagination.
struct Collection {
    /// The endpoint number of AAP 0.3.4.1.
    number: u8,
    call: Call,
    /// The program's screen size, read when `page_size` is absent.
    screen_rows: usize,
    /// The rows the test database holds for the request.
    stored: usize,
    /// The most rows one page can hold whatever the size asked for.
    page_cap: usize,
}

impl Collection {
    /// The rows a page of `requested` rows holds over the test database.
    fn expected_rows(&self, requested: usize) -> usize {
        requested.min(self.stored).min(self.page_cap)
    }
}

/// The six collection endpoints in endpoint order.
const COLLECTIONS: [Collection; 6] = [
    Collection {
        number: 7,
        call: Call::Get("/api/v1/cards"),
        screen_rows: 7,
        stored: 50,
        page_cap: MAX_ROWS,
    },
    Collection {
        number: 11,
        call: Call::Get("/api/v1/transactions"),
        screen_rows: 10,
        stored: SEEDED_TRANSACTIONS,
        page_cap: MAX_ROWS,
    },
    Collection {
        number: 16,
        call: Call::Get("/api/v1/users"),
        screen_rows: 10,
        stored: 11,
        page_cap: MAX_ROWS,
    },
    Collection {
        number: 21,
        call: Call::Get("/api/v1/accounts/00000000050/authorizations"),
        screen_rows: 5,
        stored: SEEDED_DETAILS,
        page_cap: MAX_ROWS,
    },
    Collection {
        number: 24,
        call: Call::Get("/api/v1/transaction-types"),
        screen_rows: 7,
        stored: 7,
        page_cap: MAP_ROWS,
    },
    Collection {
        number: 25,
        call: Call::ListActions,
        screen_rows: 7,
        stored: 7,
        page_cap: MAP_ROWS,
    },
];

/// The API over one fixture-loaded and seeded database, with the token every request carries.
///
/// Fields drop in declaration order, so the router and its pool handle go before the guard
/// removes the container.
struct Harness {
    router: Router,
    token: String,
    /// The `state` of endpoint 24's screen load, which every endpoint 25 body echoes.
    list_state: Value,
    /// The ID of the first seeded transaction, a key endpoint 12 finds.
    first_tran_id: String,
    _database: PostgresGuard,
}

impl Harness {
    /// Starts the database, seeds it, builds the state from the configuration and signs on.
    async fn start() -> Result<Self, Box<dyn Error>> {
        let database = start_loaded_postgres().await?;
        let mut store = PgStore::new(database.pool().clone());
        let first_tran_id = seed_transactions(&mut store).await?;
        seed_authorizations(&mut store).await?;

        let config = Config {
            jwt_secret: SECRET.to_owned(),
            intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pagination-jobs"),
            ..Config::default()
        };
        let state = AppState::from_config(store, &config, clock()?)?;
        let mut harness = Self {
            router: app(state),
            token: String::new(),
            list_state: Value::Null,
            first_tran_id,
            _database: database,
        };
        harness.token = harness.sign_on().await?;
        harness.add_user().await?;
        let (status, body) = harness
            .send(harness.get("/api/v1/transaction-types")?)
            .await?;
        if status != StatusCode::OK {
            return Err(format!("the screen load of endpoint 24 answered {status}").into());
        }
        harness.list_state = body
            .get("state")
            .cloned()
            .ok_or("endpoint 24 sent no state")?;
        Ok(harness)
    }

    /// Endpoint 1 for `ADMIN001`, a user of type `A`: the bearer token of every other request.
    async fn sign_on(&self) -> Result<String, Box<dyn Error>> {
        let credentials = json!({"user_id": "ADMIN001", "password": "PASSWORD"});
        let request = post("/api/v1/signon", None, &credentials)?;
        let (status, body) = self.send(request).await?;
        if status != StatusCode::OK {
            return Err(format!("sign-on answered {status}").into());
        }
        let token = body.get("token").and_then(Value::as_str);
        Ok(token.ok_or("sign-on returned no token")?.to_owned())
    }

    /// Endpoint 17 adds an eleventh user, so that COUSR00C's ten-row page leaves one out.
    async fn add_user(&self) -> TestResult {
        let user = json!({
            "first_name": "PAGE",
            "last_name": "TESTER",
            "user_id": "PAGEUSR1",
            "password": "PAGEPASS",
            "user_type": "U",
        });
        let (status, _) = self
            .send(post("/api/v1/users", Some(self.token.as_str()), &user)?)
            .await?;
        if status == StatusCode::OK {
            Ok(())
        } else {
            Err(format!("adding the eleventh user answered {status}").into())
        }
    }

    /// The status of `request` and its body as JSON, `null` for an empty or non-JSON body.
    async fn send(&self, request: Request<Body>) -> Result<(StatusCode, Value), Box<dyn Error>> {
        let response = self.router.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }

    /// A `GET` of `uri` with the bearer token.
    fn get(&self, uri: &str) -> Result<Request<Body>, axum::http::Error> {
        Request::get(uri)
            .header(header::AUTHORIZATION, bearer(&self.token))
            .body(Body::empty())
    }

    /// Endpoint 25's ENTER over the screen load's state, nothing typed, with `page_size` as
    /// given: a JSON number when the text is one, `1.5` included, otherwise a JSON string.
    fn list_action_body(&self, page_size: Option<&str>) -> Value {
        let mut body = json!({
            "aid": "ENTER",
            "type_filter": "",
            "desc_filter": "",
            "rows": vec![json!({"selection": "", "description": ""}); MAP_ROWS],
            "state": self.list_state,
        });
        if let (Some(size), Some(members)) = (page_size, body.as_object_mut()) {
            let size = serde_json::from_str::<Value>(size)
                .ok()
                .filter(Value::is_number)
                .unwrap_or_else(|| Value::from(size));
            members.insert(PAGE_SIZE_FIELD.to_owned(), size);
        }
        body
    }

    /// Endpoint 25's ENTER of [`Harness::list_action_body`] with a last `page_size` member whose
    /// value is the raw JSON text `page_size`, for a number that no `Value` holds.
    fn raw_list_action(&self, page_size: &str) -> Result<Request<Body>, Box<dyn Error>> {
        let body = self.list_action_body(None).to_string();
        let members = body
            .strip_suffix('}')
            .ok_or("the endpoint 25 body is not an object")?;
        Ok(Request::post(LIST_ACTIONS)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, bearer(&self.token))
            .body(Body::from(format!(
                r#"{members},"{PAGE_SIZE_FIELD}":{page_size}}}"#
            )))?)
    }

    /// One request to `endpoint`, with `page_size` as given or without the member.
    async fn collection(
        &self,
        endpoint: &Collection,
        page_size: Option<&str>,
    ) -> Result<(StatusCode, Value), Box<dyn Error>> {
        let request = match endpoint.call {
            Call::Get(path) => self.get(&page_uri(path, page_size))?,
            Call::ListActions => post(
                LIST_ACTIONS,
                Some(self.token.as_str()),
                &self.list_action_body(page_size),
            )?,
        };
        self.send(request).await
    }
}

/// The `Authorization` value of `token`.
fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// A JSON `POST` of `body` to `uri`, with the bearer token when one is given.
fn post(uri: &str, token: Option<&str>, body: &Value) -> Result<Request<Body>, axum::http::Error> {
    let builder = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    let builder = match token {
        Some(token) => builder.header(header::AUTHORIZATION, bearer(token)),
        None => builder,
    };
    builder.body(Body::from(body.to_string()))
}

/// `path` with `page_size` in its query string, or `path` alone.
fn page_uri(path: &str, page_size: Option<&str>) -> String {
    page_size.map_or_else(
        || path.to_owned(),
        |size| format!("{path}?page_size={size}"),
    )
}

/// The rows of a page, the non-null elements of its `rows` array; `None` without one.
fn filled_rows(body: &Value) -> Option<usize> {
    let rows = body.get("rows")?.as_array()?;
    Some(rows.iter().filter(|row| !row.is_null()).count())
}

/// The clock of every request: 2022-07-18 10:30:00.000.
fn clock() -> Result<Arc<dyn Clock>, Box<dyn Error>> {
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|day| day.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or("an invalid test instant")?;
    Ok(Arc::new(FixedClock::new(at)))
}

/// Inserts the first [`SEEDED_TRANSACTIONS`] records of `dailytran.txt`, decoded by codec A and
/// moved to the `transactions` row shape unchanged, and returns the first one's ID.
async fn seed_transactions(store: &mut PgStore) -> Result<String, Box<dyn Error>> {
    let input = fs::read(fixture_path("dailytran.txt"))?;
    let lines = decode_all::<DailyTransactionRecord>(&input)?;
    let mut first_id = None;
    for line in lines.into_iter().take(SEEDED_TRANSACTIONS) {
        let record = TransactionRecord::from(line.record);
        first_id.get_or_insert_with(|| record.id.to_text());
        TransactionRepository::insert(store, &record).await?;
    }
    Ok(first_id.ok_or("dailytran.txt holds no record")?)
}

/// Inserts the summary of account 50 for customer 50 and [`SEEDED_DETAILS`] approved details of
/// 10.00 each under it, keyed one millisecond apart on 2022-07-18.
async fn seed_authorizations(store: &mut PgStore) -> TestResult {
    let account = PicS::new(AUTH_ACCOUNT)?;
    let amount = Decimal::new(1_000, 2);
    let mut summary = PendingAuthSummaryRecord::initialized(account, PicU::new(AUTH_CUSTOMER)?);
    summary.approved_auth_cnt = Count::new(i16::try_from(SEEDED_DETAILS)?);
    summary.approved_auth_amt = Money::new(amount * Decimal::from(SEEDED_DETAILS))?;
    PendingAuthSummaryRepository::insert(store, &summary).await?;
    for index in 0..u64::try_from(SEEDED_DETAILS)? {
        let detail = approved_detail(index, Money::new(amount)?)?;
        PendingAuthDetailRepository::insert(store, account, &detail).await?;
    }
    Ok(())
}

/// The approval `index` of the seeded day, for [`AUTH_CARD`], keyed as CP00 keys it: the day and
/// the time inverted.
fn approved_detail(
    index: u64,
    amount: Money<10>,
) -> Result<PendingAuthDetailRecord, Box<dyn Error>> {
    Ok(PendingAuthDetailRecord {
        auth_date_9c: PicS::complement_from(AUTH_DAY)?,
        auth_time_9c: PicS::complement_from(AUTH_TIME + index)?,
        orig_date: PicX::from_text("220718")?,
        orig_time: PicX::from_text(&format!("1030{index:02}"))?,
        card_num: CardNumber::parse(AUTH_CARD)?,
        auth_type: PicX::from_text("AUTH")?,
        card_expiry_date: PicX::from_text("1227")?,
        message_type: PicX::from_text("0100")?,
        message_source: PicX::from_text("POS")?,
        auth_id_code: PicX::from_text(&format!("{index:06}"))?,
        auth_resp_code: PicX::from_array(*PendingAuthDetailRecord::AUTH_APPROVED),
        auth_resp_reason: PicX::from_text("0000")?,
        processing_code: PicU::new(0)?,
        transaction_amt: amount,
        approved_amt: amount,
        merchant_catagory_code: PicX::from_text("5411")?,
        acqr_country_code: PicX::from_text("840")?,
        pos_entry_mode: PicU::new(5)?,
        merchant_id: PicX::from_text("MERCH0000000001")?,
        merchant_name: PicX::from_text("CORNER GROCERY")?,
        merchant_city: PicX::from_text("SEATTLE")?,
        merchant_state: PicX::from_text("WA")?,
        merchant_zip: PicX::from_text("98101")?,
        transaction_id: PicX::from_text(&format!("TXN{index:012}"))?,
        match_status: PicX::from_array(*PendingAuthDetailRecord::MATCH_PENDING),
        auth_fraud: PicX::spaces(),
        fraud_rpt_date: PicX::spaces(),
        filler: PicX::spaces(),
    })
}

/// 101, 0 and -1 on each collection endpoint, the largest `i64`, which must be refused rather
/// than wrapped, integers beyond `i64` either way, and values that are no integer at all, the
/// empty value included: 422 with the `page_size` message and field, before any rule of the
/// program. Endpoint 25 receives each as a JSON number where the text is one (`1.5` and the
/// integers beyond `i64` included) and as a JSON string otherwise, and also, as raw body text,
/// an unquoted integer of 310 digits either way, beyond the range the JSON parser represents.
#[tokio::test]
async fn out_of_range_page_size_is_422() -> TestResult {
    let harness = Harness::start().await?;
    for endpoint in &COLLECTIONS {
        for size in [
            "101",
            "0",
            "-1",
            "9223372036854775807",
            "9223372036854775808",
            "99999999999999999999",
            "-9223372036854775809",
            "abc",
            "",
            "1.5",
        ] {
            let (status, body) = harness.collection(endpoint, Some(size)).await?;
            let number = endpoint.number;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "endpoint {number}, page_size {size}"
            );
            let message = body.get("message").and_then(Value::as_str);
            assert!(
                message.is_some_and(|text| !text.is_empty()),
                "endpoint {number}, page_size {size}: no message"
            );
            assert_eq!(
                message,
                Some(PAGE_SIZE_MESSAGE),
                "endpoint {number}, page_size {size}"
            );
            assert_eq!(
                body.get("field").and_then(Value::as_str),
                Some(PAGE_SIZE_FIELD),
                "endpoint {number}, page_size {size}"
            );
        }
    }
    let nines = "9".repeat(310);
    for (sign, size) in [("+", nines.clone()), ("-", format!("-{nines}"))] {
        let (status, body) = harness.send(harness.raw_list_action(&size)?).await?;
        assert_eq!(
            (
                status,
                body.get("message").and_then(Value::as_str),
                body.get("field").and_then(Value::as_str)
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Some(PAGE_SIZE_MESSAGE),
                Some(PAGE_SIZE_FIELD)
            ),
            "endpoint 25, page_size {sign} 310 nines unquoted"
        );
    }
    Ok(())
}

/// 100 and 1, the bounds, and the request without `page_size`, the only one a screen sends: 200,
/// and a page of the smaller of the size and the rows stored, within the map's positions. The
/// request without the member reads the screen size, so cards, transactions, users and
/// authorizations, which store more rows than one screen shows, return exactly one screen.
#[tokio::test]
async fn in_range_and_default_page_sizes_succeed() -> TestResult {
    let harness = Harness::start().await?;
    for endpoint in &COLLECTIONS {
        let number = endpoint.number;
        assert!(
            endpoint.stored >= endpoint.screen_rows,
            "endpoint {number}: the test database must fill one screen"
        );
        for (page_size, requested) in [
            (Some("100"), MAX_ROWS),
            (Some("1"), 1),
            (None, endpoint.screen_rows),
        ] {
            let (status, body) = harness.collection(endpoint, page_size).await?;
            assert_eq!(
                status,
                StatusCode::OK,
                "endpoint {number}, page_size {page_size:?}"
            );
            assert_eq!(
                filled_rows(&body),
                Some(endpoint.expected_rows(requested)),
                "endpoint {number}, page_size {page_size:?}: rows on the page"
            );
        }
    }
    Ok(())
}

/// No other refusal is a 422. On a collection endpoint, with a valid `page_size`, another member
/// that does not deserialise is the request layer's 400 with no field, and a program edit or a
/// member rule is the program's 400; and the endpoints that are no collection, 4, 12 and 18,
/// ignore a `page_size` and refuse bad keys with 400 or 404.
#[tokio::test]
async fn status_422_is_used_for_nothing_else() -> TestResult {
    let harness = Harness::start().await?;
    for (number, request, refusal) in unreadable_requests(&harness)? {
        let (status, body) = harness.send(request).await?;
        assert_eq!(
            (
                status,
                body.get("message").and_then(Value::as_str),
                body.get("field")
            ),
            (StatusCode::BAD_REQUEST, Some(refusal), None),
            "endpoint {number}: unreadable member"
        );
    }
    for (number, request) in edit_failures(&harness)? {
        let (status, _) = harness.send(request).await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "endpoint {number}: edit failure"
        );
    }
    for (number, uri, expected) in non_collection_requests(&harness) {
        let (status, _) = harness.send(harness.get(&uri)?).await?;
        assert_eq!(status, expected, "endpoint {number}: {uri}");
    }
    Ok(())
}

/// One request per collection endpoint whose query or body does not deserialise, each with an
/// in-range `page_size`, and the message of its 400: an unknown `direction` in a query string,
/// an unknown `aid` in endpoint 25's body.
fn unreadable_requests(harness: &Harness) -> Result<Vec<Unreadable>, Box<dyn Error>> {
    let mut requests = Vec::with_capacity(COLLECTIONS.len());
    for endpoint in &COLLECTIONS {
        let (request, refusal) = match endpoint.call {
            Call::Get(path) => (
                harness.get(&format!("{path}?direction=sideways&page_size=5"))?,
                INVALID_QUERY,
            ),
            Call::ListActions => {
                let mut body = harness.list_action_body(Some("5"));
                body.as_object_mut()
                    .ok_or("the endpoint 25 body is not an object")?
                    .insert("aid".to_owned(), Value::from("PF7"));
                (
                    post(LIST_ACTIONS, Some(harness.token.as_str()), &body)?,
                    INVALID_BODY,
                )
            }
        };
        requests.push((endpoint.number, request, refusal));
    }
    Ok(requests)
}

/// One request per collection endpoint that the program or a member rule refuses, each with an
/// in-range `page_size` so that only the refusal decides the status.
fn edit_failures(harness: &Harness) -> Result<Vec<Refused>, Box<dyn Error>> {
    let mut type_filter_not_numeric = harness.list_action_body(Some("5"));
    let members = type_filter_not_numeric
        .as_object_mut()
        .ok_or("the endpoint 25 body is not an object")?;
    members.insert("type_filter".to_owned(), Value::from("AB"));
    Ok(vec![
        // COCRDLIC-006: an account filter that is not 11 digits.
        (7, harness.get("/api/v1/cards?acct_id=ABC&page_size=5")?),
        // `Tran ID must be Numeric ...` (COTRN00C-012).
        (
            11,
            harness.get("/api/v1/transactions?start_tran_id=ABC&page_size=5")?,
        ),
        // A search value longer than the map's eight characters.
        (
            16,
            harness.get("/api/v1/users?start_user_id=TOOLONGID&page_size=5")?,
        ),
        // An account ID that is not 11 digits (COPAUS0C-006).
        (
            21,
            harness.get("/api/v1/accounts/ABC/authorizations?page_size=5")?,
        ),
        // A paging key this API never issued.
        (
            24,
            harness.get("/api/v1/transaction-types?page_key=XYZ&direction=forward&page_size=5")?,
        ),
        // `TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER`.
        (
            25,
            post(
                LIST_ACTIONS,
                Some(harness.token.as_str()),
                &type_filter_not_numeric,
            )?,
        ),
    ])
}

/// Endpoints 4, 12 and 18, which page nothing: a stored key with `page_size=101` is served, a
/// blank, malformed or unknown key is refused with the program's 400 or 404, and a key longer
/// than its map field with the length 400.
fn non_collection_requests(harness: &Harness) -> Vec<(u8, String, StatusCode)> {
    let tran = &harness.first_tran_id;
    vec![
        (
            4,
            "/api/v1/accounts/00000000050/view?page_size=101".to_owned(),
            StatusCode::OK,
        ),
        (
            4,
            "/api/v1/accounts/ABC/view".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            4,
            "/api/v1/accounts/000000000500/view".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            4,
            "/api/v1/accounts/99999999999/view".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            12,
            format!("/api/v1/transactions/{tran}?page_size=101"),
            StatusCode::OK,
        ),
        (
            12,
            "/api/v1/transactions/%20".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            12,
            "/api/v1/transactions/ABC".to_owned(),
            StatusCode::NOT_FOUND,
        ),
        (
            12,
            "/api/v1/transactions/00000000006835800".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            18,
            "/api/v1/users/ADMIN001?page_size=101".to_owned(),
            StatusCode::OK,
        ),
        (18, "/api/v1/users/%20".to_owned(), StatusCode::BAD_REQUEST),
        (
            18,
            "/api/v1/users/NOSUCHID".to_owned(),
            StatusCode::NOT_FOUND,
        ),
    ]
}
