//! Binding g1b for endpoints 14 and 15 (AAP 0.9.1): the `server` rows of
//! `modern/audit/online/CORPT00C.md` and `modern/audit/online/COBIL00C.md`, the rows of
//! `modern/audit/online/CSUTLDTC.md` that endpoint 14 reaches, and the server half of the
//! `CORPT00` and `COBIL00` `both` rows of `modern/audit/screens.md`, each proved through
//! `POST /api/v1/reports/transactions` or `POST /api/v1/bill-payments`.
//!
//! # Harness
//!
//! - Every test function starts its own fixture-loaded `postgres:16` container through
//!   `test_support::fixtures::start_loaded_postgres`, the only way this file obtains a schema
//!   (Single Schema Executor), and reads results back through the real repository traits.
//! - Requests run in process through `tower::ServiceExt::oneshot` against `api::app::app`, over an
//!   `AppState` built by `AppState::from_config` with a local secret, a `FixedClock` at
//!   2022-07-18 10:30:00.000 and a job-deck directory of the test's own under
//!   `CARGO_TARGET_TMPDIR`, emptied first so the generations start at `G0001`. The bearer token is
//!   the one endpoint 1 issues to `USER0001`, a user of type `U`.
//! - Every response body is kept and searched for the 50 fixture card numbers, which no body of
//!   either endpoint may hold (Cardholder Data Redaction). The deck files are read from disk and
//!   never served.
//! - An account read that fails at once, whatever the confirmation (COBIL00C-015, beside the
//!   lock waits below), and the outcomes a running database cannot raise once that read holds
//!   its lock, a failed payment insert or balance rewrite (COBIL00C-030, COBIL00C-033) and a
//!   rewrite that matches no row (COBIL00C-032), come from a [`Script`] installed with
//!   `PgStore::with_probe` on the store of the request's state. At each probe point it names, the
//!   store skips the statement, sending no SQL, and fails with the scripted error or answers as
//!   when no row has the key; every other statement runs against the database. The script counts
//!   the calls at the lock, the insert and the rewrite, and each test asserts them, so the
//!   scripted statement is shown reached exactly once. Effects are read back through an unprobed
//!   store over the same pool.
//! - The lock waits of the pool `carddemo-api` serves requests from,
//!   `runtime::db::connect_for_requests` on the container's URL, are proved over that pool: an
//!   account read that meets the row lock a transaction of the test holds waits until that
//!   transaction commits and then reads the balance it left (COBIL00C-013), and only a
//!   `lock_timeout` or `statement_timeout` the URL itself sets ends the wait, as COBIL00C-015's
//!   failed read. Once the container is removed from under a pool holding several idle
//!   connections, the first payment answers within the pool's acquire timeout, and the payments
//!   that follow it fail at once, though broken connections are still idle in the pool, because
//!   the store, built as `carddemo-api` builds its own, fails fast while the pool's refusal watch
//!   finds the server refusing connections.
//!
//! # Halts
//!
//! - Which custom report dates CSUTLDTC accepts is halt H5, and the endpoint cannot substitute
//!   the verdict. The custom-date assertions therefore hold for every verdict: the outcome is one
//!   of the three the audit allows, and a written deck holds the dates CORPT00C built. Rows the
//!   halt covers whole carry their tags on the `blocked_*` tests, which assert no parity value and
//!   fail when run with `--ignored`, as every audit row of a `BLOCKED` halt keeps its tags.
//! - A payment against an empty `transactions` table is halt candidate X-COBIL00C-H3, so each
//!   payment test first inserts the first record of `dailytran.txt` and expects the next ID above
//!   it, never the end-of-file ID `0000000000000001`.
//! - What the COBOL runs after the screen of a failed account read (X-COBIL00C-H1) and the card
//!   number a payment writes with no cross-reference or after `DUPKEY` (X-COBIL00C-H2) are not
//!   asserted: COBIL00C-015, COBIL00C-021 and COBIL00C-022 assert every other effect, and the
//!   `blocked_*` tests report those parts.

use std::cell::RefCell;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use api::app::{AppState, app, check_intrdr_dir};
use api::dto::{FIELD_LENGTH_MESSAGE, FIELD_NUMERIC_MESSAGE, INVALID_BODY};
use api::error::UNABLE_TO_PROCESS;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use chrono::{NaiveDate, NaiveDateTime};
use domain::clock::{Clock, FixedClock};
use domain::copybook::{FixedRecord, PicU, PicX, TsForm};
use domain::money::{Decimal, Money};
use domain::records::account::AccountRecord;
use domain::records::card_xref::CardXrefRecord;
use domain::records::transaction::TransactionRecord;
use domain::sensitive::CardNumber;
use persistence::{
    AccountRepository, CardXrefRepository, PersistenceError, PgStore, ProbeCall, ProbePoint,
    ProbeVerdict, RepositoryProbe, TransactionRepository,
};
use runtime::config::Config;
use runtime::db::{ACQUIRE_TIMEOUT, ErrorSummary, connect, connect_for_requests};
use serde_json::{Value, json};
use test_support::fixtures::{fixture_card_numbers, fixture_path, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use tower::ServiceExt as _;

type TestResult = Result<(), Box<dyn Error>>;
type Outcome<T> = Result<T, Box<dyn Error>>;

/// The HS256 secret of the test states: a local value, never a credential.
const SECRET: &str = "endpoints-reports-billpay-local-secret";

const SIGNON: &str = "/api/v1/signon";
const REPORTS: &str = "/api/v1/reports/transactions";
const BILL_PAYMENTS: &str = "/api/v1/bill-payments";

/// The deck: 17 records of `JOB-DATA-1`, each 80 bytes followed by a line feed (CORPT00C-034,
/// CORPT00C-036).
const DECK_RECORDS: usize = 17;
const DECK_RECORD_LEN: usize = 80;

/// The confirmed report requests in flight at once in the concurrent submission row
/// (CORPT00C-036).
const CONCURRENT_SUBMISSIONS: usize = 16;

// The CORPT00C literals, copied from `modern/audit/online/CORPT00C.md`. The audit names a date
// part's field as a dotted path such as `start_date.month`; the request carries the parts as the
// flat members `start_month` to `end_year`, and `ScreenError.field` carries the member's wire
// name, so the expected fields below are those member names.
const SELECT_REPORT: &str = "Select a report type to print report...";
const MONTHLY_PROMPT: &str = "Please confirm to print the Monthly report...";
const YEARLY_PROMPT: &str = "Please confirm to print the Yearly report...";
const CUSTOM_PROMPT: &str = "Please confirm to print the Custom report...";
const CONFIRM_X_INVALID: &str = "\"X\" is not a valid value to confirm...";
const MONTHLY_SUBMITTED: &str = "Monthly report submitted for printing ...";
const YEARLY_SUBMITTED: &str = "Yearly report submitted for printing ...";
const CUSTOM_SUBMITTED: &str = "Custom report submitted for printing ...";
const WRITE_FAILURE: &str = "Unable to Write TDQ (JOBS)...";
const START_MONTH_EMPTY: &str = "Start Date - Month can NOT be empty...";
const START_DAY_EMPTY: &str = "Start Date - Day can NOT be empty...";
const START_YEAR_EMPTY: &str = "Start Date - Year can NOT be empty...";
const END_MONTH_EMPTY: &str = "End Date - Month can NOT be empty...";
const END_DAY_EMPTY: &str = "End Date - Day can NOT be empty...";
const END_YEAR_EMPTY: &str = "End Date - Year can NOT be empty...";
const START_MONTH_INVALID: &str = "Start Date - Not a valid Month...";
const START_DAY_INVALID: &str = "Start Date - Not a valid Day...";
const END_MONTH_INVALID: &str = "End Date - Not a valid Month...";
const END_DAY_INVALID: &str = "End Date - Not a valid Day...";
const START_DATE_INVALID: &str = "Start Date - Not a valid date...";
const END_DATE_INVALID: &str = "End Date - Not a valid date...";

// The COBIL00C literals, copied from `modern/audit/online/COBIL00C.md`.
const ACCT_ID_EMPTY: &str = "Acct ID can NOT be empty...";
const CONFIRM_INVALID: &str = "Invalid value. Valid values are (Y/N)...";
const ACCOUNT_NOT_FOUND: &str = "Account ID NOT found...";
const ACCOUNT_LOOKUP_FAILED: &str = "Unable to lookup Account...";
const NOTHING_TO_PAY: &str = "You have nothing to pay...";
const PAY_PROMPT: &str = "Confirm to make a bill payment...";
const TRAN_ID_EXISTS: &str = "Tran ID already exist...";
const TRAN_WRITE_FAILED: &str = "Unable to Add Bill pay Transaction...";
const ACCOUNT_UPDATE_FAILED: &str = "Unable to Update Account...";

/// The probe points of a payment, in the order the service reaches them: the account read for
/// update, the payment insert and the balance rewrite. [`Script::calls`] counts in this order.
const PAYMENT_POINTS: [ProbePoint; 3] = [
    ProbePoint::AccountLock,
    ProbePoint::TransactionInsert,
    ProbePoint::AccountUpdate,
];

/// The text CSUTLDTC writes into its 80-byte result, labels, picture and `WS-RESULT` values
/// alike: none of it reaches any consumer's response (CSUTLDTC-003, CSUTLDTC-007, CSUTLDTC-009
/// to CSUTLDTC-018).
const CSUTLDTC_RESULT_TEXT: [&str; 15] = [
    "Mesg Code:",
    "TstDate:",
    "Mask used:",
    "YYYY-MM-DD",
    "Date is valid",
    "Insufficient",
    "Datevalue error",
    "Invalid Era",
    "Unsupp. Range",
    "Invalid month",
    "Bad Pic String",
    "Nonnumeric data",
    "YearInEra is 0",
    "Date is invalid",
    "validation error Sev code",
];

/// The instant of a test clock: `year`-`month`-`day` 10:30:00.000.
fn instant(year: i32, month: u32, day: u32) -> Outcome<NaiveDateTime> {
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or_else(|| "an invalid test instant".into())
}

/// A job-deck directory of `name`'s own under Cargo's temporary directory for integration
/// tests, removed and created again so that it starts empty.
fn fresh_dir(name: &str) -> Outcome<PathBuf> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("endpoints_reports_billpay")
        .join(name);
    match fs::remove_dir_all(&dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The API over `store`, with job decks under `intrdr_dir` and time from a `FixedClock` at `at`.
fn router(store: PgStore, intrdr_dir: PathBuf, at: NaiveDateTime) -> Outcome<Router> {
    let config = Config {
        jwt_secret: SECRET.to_owned(),
        intrdr_dir,
        ..Config::default()
    };
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(at));
    Ok(app(AppState::from_config(store, &config, clock)?))
}

/// One response: its status and its JSON body.
struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    /// The text member `key` of the body.
    fn text(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(Value::as_str)
    }

    /// The status with the `message` and `field` of a `ScreenError` body.
    fn screen(&self) -> (StatusCode, Option<&str>, Option<&str>) {
        (self.status, self.text("message"), self.text("field"))
    }

    /// The member names of the body, sorted.
    fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .body
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        keys
    }
}

/// Endpoint calls against one `AppState`, with the bearer token endpoint 1 issued to `USER0001`;
/// every response body is kept, in one ledger shared by the APIs [`Api::sharing`] derives, for
/// the redaction scan.
struct Api {
    store: PgStore,
    router: Router,
    token: String,
    bodies: Rc<RefCell<Vec<String>>>,
}

impl Api {
    /// Signs `USER0001` on through endpoint 1 against the state over `db`, with job decks under
    /// `intrdr_dir` and time from a `FixedClock` at `at`.
    async fn signed_on(
        db: &PostgresGuard,
        intrdr_dir: PathBuf,
        at: NaiveDateTime,
    ) -> Outcome<Self> {
        Self::signed_on_over(PgStore::new(db.pool().clone()), intrdr_dir, at).await
    }

    /// Signs `USER0001` on through endpoint 1 against the state over `store`, with job decks
    /// under `intrdr_dir` and time from a `FixedClock` at `at`.
    async fn signed_on_over(
        store: PgStore,
        intrdr_dir: PathBuf,
        at: NaiveDateTime,
    ) -> Outcome<Self> {
        let unsigned = Self {
            router: router(store.clone(), intrdr_dir, at)?,
            store,
            token: String::new(),
            bodies: Rc::default(),
        };
        let credentials = json!({"user_id": "USER0001", "password": "PASSWORD"});
        let reply = unsigned.send(SIGNON, &credentials, None).await?;
        assert_eq!(reply.status, StatusCode::OK, "USER0001 did not sign on");
        let token = reply
            .text("token")
            .ok_or("sign-on answered no token")?
            .to_owned();
        Ok(Self { token, ..unsigned })
    }

    /// The same store, token and ledger over another state: job decks under `intrdr_dir` and time
    /// from a `FixedClock` at `at`.
    fn sharing(&self, intrdr_dir: PathBuf, at: NaiveDateTime) -> Outcome<Self> {
        Ok(Self {
            store: self.store.clone(),
            router: router(self.store.clone(), intrdr_dir, at)?,
            token: self.token.clone(),
            bodies: Rc::clone(&self.bodies),
        })
    }

    /// The same token and ledger over a state whose store answers to `script`, with job decks
    /// under `intrdr_dir` and time from a `FixedClock` at `at`. The returned API keeps this one's
    /// unprobed store, so what a test reads back is what the database holds.
    fn probed(
        &self,
        script: &Arc<Script>,
        intrdr_dir: PathBuf,
        at: NaiveDateTime,
    ) -> Outcome<Self> {
        let probe: Arc<dyn RepositoryProbe> = Arc::<Script>::clone(script);
        Ok(Self {
            store: self.store.clone(),
            router: router(self.store.clone().with_probe(probe), intrdr_dir, at)?,
            token: self.token.clone(),
            bodies: Rc::clone(&self.bodies),
        })
    }

    /// `POST uri` with `body` and the bearer token.
    async fn post(&self, uri: &str, body: &Value) -> Outcome<Reply> {
        self.send(uri, body, Some(&self.token)).await
    }

    /// `POST uri` with the JSON `body` and, when given, `token` as bearer token.
    async fn send(&self, uri: &str, body: &Value, token: Option<&str>) -> Outcome<Reply> {
        let request = Self::request(uri, body, token)?;
        let response = self.router.clone().oneshot(request).await?;
        self.reply(response).await
    }

    /// `count` requests `POST uri` with `body` and the bearer token, each served by a task of
    /// its own and all in flight at once, and their replies in the order they were sent.
    async fn post_concurrently(
        &self,
        uri: &str,
        body: &Value,
        count: usize,
    ) -> Outcome<Vec<Reply>> {
        let mut tasks = Vec::with_capacity(count);
        for _ in 0..count {
            let request = Self::request(uri, body, Some(&self.token))?;
            tasks.push(tokio::spawn(self.router.clone().oneshot(request)));
        }
        let mut replies = Vec::with_capacity(count);
        for task in tasks {
            replies.push(self.reply(task.await??).await?);
        }
        Ok(replies)
    }

    /// The request `POST uri` with the JSON `body` and, when given, `token` as bearer token.
    fn request(uri: &str, body: &Value, token: Option<&str>) -> Outcome<Request<Body>> {
        let mut request = Request::post(uri).header(CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        Ok(request.body(Body::from(serde_json::to_vec(body)?))?)
    }

    /// The status and the JSON body of `response`, whose body joins the ledger.
    async fn reply(&self, response: Response) -> Outcome<Reply> {
        let status = response.status();
        let text = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await?.to_vec())?;
        let body = serde_json::from_str(&text)?;
        self.bodies.borrow_mut().push(text);
        Ok(Reply { status, body })
    }
}

/// The bearer token of `api` with the first character of its signature changed, so that the
/// signature no longer verifies. The last base64url character of an HS256 signature carries two
/// unused bits, so changing it might leave the signature intact; the first carries six of its
/// bits.
fn tampered(api: &Api) -> String {
    let (head, signature) = api
        .token
        .rsplit_once('.')
        .unwrap_or((api.token.as_str(), ""));
    let mut rest = signature.chars();
    let first = rest
        .next()
        .map_or('A', |c| if c == 'A' { 'B' } else { 'A' });
    format!("{head}.{first}{}", rest.as_str())
}

/// Asserts that no response body of `api`'s ledger holds one of the 50 fixture card numbers.
/// The needles are never printed.
fn assert_no_card_number(api: &Api) -> TestResult {
    let needles = fixture_card_numbers()?;
    let bodies = api.bodies.borrow();
    assert!(!bodies.is_empty(), "no response was recorded");
    for body in &*bodies {
        assert!(
            !needles.iter().any(|needle| body.contains(needle.as_str())),
            "a response body holds a fixture card number"
        );
    }
    Ok(())
}

/// An endpoint 14 body with the three selectors and the confirmation; the six date parts are
/// absent, the empty text.
fn report(monthly: &str, yearly: &str, custom: &str, confirm: &str) -> Value {
    json!({"monthly": monthly, "yearly": yearly, "custom": custom, "confirm": confirm})
}

/// An endpoint 14 body for the custom report, with the six date parts in map order: start
/// month, day and year, then end month, day and year.
fn custom(parts: [&str; 6], confirm: &str) -> Value {
    let [
        start_month,
        start_day,
        start_year,
        end_month,
        end_day,
        end_year,
    ] = parts;
    json!({
        "custom": "X",
        "start_month": start_month,
        "start_day": start_day,
        "start_year": start_year,
        "end_month": end_month,
        "end_day": end_day,
        "end_year": end_year,
        "confirm": confirm,
    })
}

/// The names of the entries of `dir`, sorted.
fn deck_names(dir: &Path) -> Outcome<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        names.push(
            entry?
                .file_name()
                .into_string()
                .map_err(|_| "a non-UTF-8 name")?,
        );
    }
    names.sort_unstable();
    Ok(names)
}

/// The records of the deck `name` in `dir`, each checked to be 80 bytes followed by a line feed,
/// 17 of them, 1,377 bytes in all (CORPT00C-035, CORPT00C-036).
fn read_deck(dir: &Path, name: &str) -> Outcome<Vec<String>> {
    let bytes = fs::read(dir.join(name))?;
    assert_eq!(bytes.len(), DECK_RECORDS * (DECK_RECORD_LEN + 1));
    bytes
        .split_inclusive(|&byte| byte == b'\n')
        .map(|line| {
            let record = line
                .strip_suffix(b"\n")
                .ok_or("a deck record lacks its LF")?;
            assert_eq!(record.len(), DECK_RECORD_LEN);
            Ok(String::from_utf8(record.to_vec())?)
        })
        .collect()
}

/// The 17 records of `JOB-DATA-1` for the start date `start` and the end date `end`, each its
/// text padded with spaces to 80 bytes, written out from `CORPT00C.cbl:81-125` as
/// CORPT00C-034 lists them.
fn expected_deck(start: &str, end: &str) -> Vec<String> {
    [
        "//TRNRPT00 JOB 'TRAN REPORT',CLASS=A,MSGCLASS=0,".to_owned(),
        "// NOTIFY=&SYSUID".to_owned(),
        "//*".to_owned(),
        "//JOBLIB JCLLIB ORDER=('AWS.M2.CARDDEMO.PROC')".to_owned(),
        "//*".to_owned(),
        "//STEP10 EXEC PROC=TRANREPT".to_owned(),
        "//*".to_owned(),
        "//STEP05R.SYMNAMES DD *".to_owned(),
        "TRAN-CARD-NUM,263,16,ZD".to_owned(),
        "TRAN-PROC-DT,305,10,CH".to_owned(),
        format!("PARM-START-DATE,C'{start}'"),
        format!("PARM-END-DATE,C'{end}'"),
        "/*".to_owned(),
        "//STEP10R.DATEPARM DD *".to_owned(),
        format!("{start} {end}"),
        "/*".to_owned(),
        "/*EOF".to_owned(),
    ]
    .into_iter()
    .map(|text| format!("{text:<DECK_RECORD_LEN$}"))
    .collect()
}

// Worker threads of its own, so that the requests of `report_concurrent_row` are served in
// parallel and reach the job-deck directory at once.
#[test]
fn corpt00c_report_rows() -> TestResult {
    multi_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let dir = fresh_dir("corpt00c_report_rows")?;
        let api = Api::signed_on(&db, dir.clone(), instant(2022, 7, 18)?).await?;
        report_token_rows(&api, &dir).await?;
        report_selection_rows(&api, &dir).await?;
        report_confirmation_rows(&api, &dir).await?;
        report_map_rows(&api, &dir).await?;
        report_non_object_rows(&api, &dir).await?;
        report_submission_rows(&api, &dir).await?;
        report_concurrent_row(&api).await?;
        report_month_end_rows(&api).await?;
        report_write_failure_row(&api).await?;
        assert_no_card_number(&api)
    })
}

/// CORPT00C-043: without a valid token endpoint 14 answers the extractor's 401 before any edit,
/// whatever the body holds, and writes no deck.
async fn report_token_rows(api: &Api, dir: &Path) -> TestResult {
    let confirmed = report("X", "", "", "Y");
    let tampered = tampered(api);
    for token in [None, Some("not-a-token"), Some(tampered.as_str())] {
        let reply = api.send(REPORTS, &confirmed, token).await?;
        // audit: CORPT00C-043
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert!(
            reply
                .text("message")
                .is_some_and(|message| !message.is_empty())
        );
        assert_eq!(reply.text("job_deck"), None);
    }
    // audit: CORPT00C-043
    assert!(deck_names(dir)?.is_empty());
    Ok(())
}

/// CORPT00C-003, CORPT00C-007 and CORPT00C-029: the report selection, for a `U` token.
async fn report_selection_rows(api: &Api, dir: &Path) -> TestResult {
    // No selector: neither the date parts nor the confirmation is examined.
    let unselected = [
        json!({}),
        json!({"monthly": " ", "custom": " ", "start_month": "99", "confirm": "Y"}),
    ];
    for body in unselected {
        // audit: CORPT00C-029
        assert_eq!(
            api.post(REPORTS, &body).await?.screen(),
            (
                StatusCode::BAD_REQUEST,
                Some(SELECT_REPORT),
                Some("monthly")
            )
        );
    }
    // The first selector that is not blank wins, in the order monthly, yearly, custom, whatever
    // character marks it; the custom date parts, all empty here, are then never edited.
    let selected = [
        (report("N", "X", "X", ""), MONTHLY_PROMPT),
        (report("", "X", "X", ""), YEARLY_PROMPT),
        (report(" ", "Y", "", ""), YEARLY_PROMPT),
    ];
    for (body, prompt) in selected {
        let reply = api.post(REPORTS, &body).await?;
        // audit: CORPT00C-003
        assert_ne!(reply.status, StatusCode::FORBIDDEN);
        // audit: CORPT00C-007
        assert_eq!(
            reply.screen(),
            (StatusCode::BAD_REQUEST, Some(prompt), Some("confirm"))
        );
    }
    // audit: CORPT00C-006
    assert!(deck_names(dir)?.is_empty());
    Ok(())
}

/// CORPT00C-030, CORPT00C-032 and CORPT00C-033: the confirmation of a report whose dates are
/// set. None of these answers writes a deck.
async fn report_confirmation_rows(api: &Api, dir: &Path) -> TestResult {
    let prompts = [
        (report("X", "", "", ""), MONTHLY_PROMPT),
        (report("", "X", "", " "), YEARLY_PROMPT),
    ];
    for (body, prompt) in prompts {
        // audit: CORPT00C-030
        assert_eq!(
            api.post(REPORTS, &body).await?.screen(),
            (StatusCode::BAD_REQUEST, Some(prompt), Some("confirm"))
        );
    }
    let invalid = [
        ("X", CONFIRM_X_INVALID),
        ("1", "\"1\" is not a valid value to confirm..."),
        ("z", "\"z\" is not a valid value to confirm..."),
    ];
    for (value, message) in invalid {
        // audit: CORPT00C-033
        assert_eq!(
            api.post(REPORTS, &report("X", "", "", value))
                .await?
                .screen(),
            (StatusCode::BAD_REQUEST, Some(message), Some("confirm"))
        );
    }
    for value in ["N", "n"] {
        let reply = api.post(REPORTS, &report("", "X", "", value)).await?;
        // audit: CORPT00C-032
        assert_eq!(
            (reply.status, reply.body),
            (StatusCode::OK, json!({"message": ""}))
        );
    }
    // audit: CORPT00C-006
    assert!(deck_names(dir)?.is_empty());
    Ok(())
}

/// The server half of the `CORPT00` `both` rows: each member longer than its map `LENGTH` or
/// holding text its `PIC X(n)` field cannot hold (a character outside ASCII such as `é`, or a
/// control byte such as TAB), and each date part holding a character
/// other than a digit apart from spaces around the digits (`NUM`), is a 400 naming the member,
/// before any program edit, and writes no deck.
async fn report_map_rows(api: &Api, dir: &Path) -> TestResult {
    let before = deck_names(dir)?;
    // Member, a value one character longer than the map field, and a value that fits it but is
    // not digits.
    let members = [
        // audit: CORPT00-012
        ("monthly", "XX", None),
        // audit: CORPT00-015
        ("yearly", "XX", None),
        // audit: CORPT00-018
        ("custom", "XX", None),
        // audit: CORPT00-022
        ("start_month", "123", Some("1a")),
        // audit: CORPT00-024
        ("start_day", "123", Some("+1")),
        // audit: CORPT00-026
        ("start_year", "20222", Some("20 2")),
        // audit: CORPT00-030
        ("end_month", "123", Some("-1")),
        // audit: CORPT00-032
        ("end_day", "123", Some("1.")),
        // audit: CORPT00-034
        ("end_year", "20222", Some("2O22")),
        // audit: CORPT00-038
        ("confirm", "YY", None),
    ];
    for (member, too_long, not_digits) in members {
        // A Monthly request with a blank confirmation otherwise, which only the map rule rejects
        // as anything but the confirmation prompt. `{"monthly":"é"}` is not the Monthly report,
        // and `{"monthly":"X","confirm":"\t"}` is no confirmation the program echoes.
        for value in [too_long, "é", "\t"] {
            let mut body = report("X", "", "", "");
            body[member] = json!(value);
            assert_eq!(
                api.post(REPORTS, &body).await?.screen(),
                (
                    StatusCode::BAD_REQUEST,
                    Some(FIELD_LENGTH_MESSAGE),
                    Some(member)
                ),
                "{member} = {value:?}"
            );
        }
        if let Some(value) = not_digits {
            let mut body = report("X", "", "", "");
            body[member] = json!(value);
            assert_eq!(
                api.post(REPORTS, &body).await?.screen(),
                (
                    StatusCode::BAD_REQUEST,
                    Some(FIELD_NUMERIC_MESSAGE),
                    Some(member)
                )
            );
        }
    }
    assert_eq!(deck_names(dir)?, before);
    Ok(())
}

/// A body whose top-level value is not a JSON object, an array whose elements would fill the
/// members in order included, is the request layer's 400 with no field, before any edit, and
/// writes no deck.
async fn report_non_object_rows(api: &Api, dir: &Path) -> TestResult {
    let before = deck_names(dir)?;
    let bodies = [
        json!(["X", "", "", "", "", "", "", "", "", "Y"]),
        json!(["", "X", "", "", "", "", "", "", "", "Y"]),
        json!([]),
        Value::Null,
        json!("X"),
    ];
    for body in bodies {
        let reply = api.post(REPORTS, &body).await?;
        assert_eq!(
            (reply.status, reply.body),
            (StatusCode::BAD_REQUEST, json!({"message": INVALID_BODY}))
        );
    }
    assert_eq!(deck_names(dir)?, before);
    Ok(())
}

/// CORPT00C-008 to CORPT00C-010, CORPT00C-031 and CORPT00C-034 to CORPT00C-039: a confirmed
/// Monthly report, then a confirmed Yearly one, each its own generation file.
async fn report_submission_rows(api: &Api, dir: &Path) -> TestResult {
    let submissions = [
        (
            report("X", "", "", "Y"),
            MONTHLY_SUBMITTED,
            "TRNRPT00.G0001",
            ("2022-07-01", "2022-07-31"),
        ),
        (
            report("", "X", "", "y"),
            YEARLY_SUBMITTED,
            "TRNRPT00.G0002",
            ("2022-01-01", "2022-12-31"),
        ),
    ];
    for (body, message, deck, (start, end)) in submissions {
        let reply = api.post(REPORTS, &body).await?;
        // audit: CORPT00C-003
        // audit: CORPT00C-031
        // audit: CORPT00C-038
        assert_eq!(
            (reply.status, reply.text("message"), reply.text("job_deck")),
            (StatusCode::OK, Some(message), Some(deck))
        );
        // audit: CORPT00C-038
        assert!(
            reply
                .text("job_deck")
                .is_some_and(|name| !name.contains(['/', '\\']))
        );
        // audit: CORPT00C-039
        assert_eq!(reply.keys(), ["job_deck", "message"]);
        // audit: CORPT00C-008
        // audit: CORPT00C-009
        // audit: CORPT00C-010
        // audit: CORPT00C-034
        // audit: CORPT00C-035
        // audit: CORPT00C-036
        assert_eq!(read_deck(dir, deck)?, expected_deck(start, end));
    }
    // audit: CORPT00C-036
    assert_eq!(deck_names(dir)?, ["TRNRPT00.G0001", "TRNRPT00.G0002"]);
    Ok(())
}

/// CORPT00C-036 and CORPT00C-038: [`CONCURRENT_SUBMISSIONS`] confirmed Monthly requests in
/// flight at once, as `WRITEQ TD` accepts every concurrent `CR00` task's deck. Each is
/// submitted with a generation of its own, the generations are exactly `G0001` onward, and
/// every file holds one whole deck, so no deck was overwritten.
async fn report_concurrent_row(api: &Api) -> TestResult {
    let dir = fresh_dir("corpt00c_concurrent")?;
    let concurrent = api.sharing(dir.clone(), instant(2022, 7, 18)?)?;
    let replies = concurrent
        .post_concurrently(REPORTS, &report("X", "", "", "Y"), CONCURRENT_SUBMISSIONS)
        .await?;
    let mut decks = Vec::with_capacity(CONCURRENT_SUBMISSIONS);
    for reply in &replies {
        // audit: CORPT00C-036
        // audit: CORPT00C-038
        assert_eq!(
            (reply.status, reply.text("message")),
            (StatusCode::OK, Some(MONTHLY_SUBMITTED))
        );
        decks.push(reply.text("job_deck").ok_or("no job_deck")?.to_owned());
    }
    decks.sort_unstable();
    let expected: Vec<String> = (1..=CONCURRENT_SUBMISSIONS)
        .map(|generation| format!("TRNRPT00.G{generation:04}"))
        .collect();
    // audit: CORPT00C-036
    assert_eq!(decks, expected);
    assert_eq!(deck_names(&dir)?, expected);
    for deck in &expected {
        // audit: CORPT00C-036
        assert_eq!(
            read_deck(&dir, deck)?,
            expected_deck("2022-07-01", "2022-07-31")
        );
    }
    Ok(())
}

/// CORPT00C-009: the Monthly end date is the last day of the clock's month, across a year end,
/// a leap February and a common February.
async fn report_month_end_rows(api: &Api) -> TestResult {
    let months = [
        ((2022, 12, 5), "2022-12-01", "2022-12-31"),
        ((2024, 2, 10), "2024-02-01", "2024-02-29"),
        ((2023, 2, 10), "2023-02-01", "2023-02-28"),
    ];
    for ((year, month, day), start, end) in months {
        let dir = fresh_dir(&format!("corpt00c_month_end_{start}"))?;
        let dated = api.sharing(dir.clone(), instant(year, month, day)?)?;
        let reply = dated.post(REPORTS, &report("X", "", "", "Y")).await?;
        assert_eq!(
            (reply.status, reply.text("job_deck")),
            (StatusCode::OK, Some("TRNRPT00.G0001"))
        );
        // audit: CORPT00C-008
        // audit: CORPT00C-009
        assert_eq!(
            read_deck(&dir, "TRNRPT00.G0001")?,
            expected_deck(start, end)
        );
    }
    Ok(())
}

/// CORPT00C-037: `CARDDEMO_INTRDR_DIR` names an existing regular file, so the generation cannot
/// be named or created: 500 with the TDQ message on `monthly`, no `job_deck`, and the file left
/// as it was, as when the directory becomes a file after `carddemo-api` started; at startup,
/// `check_intrdr_dir` refuses it. What a COBOL write failure leaves behind is halt candidate
/// X-CORPT00C-H2 and is not asserted.
async fn report_write_failure_row(api: &Api) -> TestResult {
    let parent = fresh_dir("corpt00c_write_failure")?;
    let file = parent.join("not-a-directory");
    fs::write(&file, b"")?;
    let failing = api.sharing(file.clone(), instant(2022, 7, 18)?)?;
    let reply = failing.post(REPORTS, &report("X", "", "", "Y")).await?;
    // audit: CORPT00C-037
    assert_eq!(
        reply.screen(),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(WRITE_FAILURE),
            Some("monthly")
        )
    );
    // audit: CORPT00C-037
    assert_eq!(reply.text("job_deck"), None);
    assert!(fs::metadata(&file)?.is_file());
    assert!(fs::read(&file)?.is_empty());
    assert_eq!(deck_names(&parent)?, ["not-a-directory"]);

    // The same directory refused at startup, naming the variable and the cause, never the path.
    let config = Config {
        intrdr_dir: file,
        ..Config::default()
    };
    let refused = check_intrdr_dir(&config)
        .err()
        .ok_or("the startup check accepted a regular file")?;
    assert_eq!(refused.kind(), io::ErrorKind::NotADirectory);
    let text = refused.to_string();
    assert_eq!(
        text,
        "CARDDEMO_INTRDR_DIR, or jobs under CARDDEMO_VAR_DIR when CARDDEMO_INTRDR_DIR is unset, \
         cannot hold job decks: not a directory"
    );
    assert!(!text.contains(&*parent.to_string_lossy()));
    Ok(())
}

#[test]
fn corpt00c_custom_date_rows() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let dir = fresh_dir("corpt00c_custom_date_rows")?;
        let api = Api::signed_on(&db, dir.clone(), instant(2022, 7, 18)?).await?;
        custom_blank_part_rows(&api, &dir).await?;
        custom_part_edit_rows(&api, &dir).await?;
        custom_date_edit_rows(&api, &dir).await?;
        custom_submission_row(&api, &dir).await?;
        custom_padded_part_rows(&api).await?;
        assert_no_result_text(&api);
        assert_no_card_number(&api)
    })
}

/// CORPT00C-006 and CORPT00C-011 to CORPT00C-016: the six emptiness tests of the custom path in
/// map order. The first blank part decides, before any range test and before the confirmation,
/// and no deck is written.
async fn custom_blank_part_rows(api: &Api, dir: &Path) -> TestResult {
    let full = ["01", "05", "2022", "07", "06", "2022"];
    let blank_from = [
        // audit: CORPT00C-011
        (START_MONTH_EMPTY, "start_month"),
        // audit: CORPT00C-012
        (START_DAY_EMPTY, "start_day"),
        // audit: CORPT00C-013
        (START_YEAR_EMPTY, "start_year"),
        // audit: CORPT00C-014
        (END_MONTH_EMPTY, "end_month"),
        // audit: CORPT00C-015
        (END_DAY_EMPTY, "end_day"),
        // audit: CORPT00C-016
        (END_YEAR_EMPTY, "end_year"),
    ];
    for (first_blank, (message, field)) in blank_from.into_iter().enumerate() {
        // That part and every part after it empty, the confirmation invalid.
        let mut parts = full;
        for part in parts.iter_mut().skip(first_blank) {
            *part = "";
        }
        // audit: CORPT00C-006
        assert_eq!(
            api.post(REPORTS, &custom(parts, "X")).await?.screen(),
            (StatusCode::BAD_REQUEST, Some(message), Some(field))
        );
    }
    // A part of spaces is blank too, and the emptiness tests precede every range test: the
    // start month 13 and the start day 99 are not reached.
    let spaced = custom(["13", "99", "2022", "  ", "06", "2022"], "Y");
    // audit: CORPT00C-006
    // audit: CORPT00C-014
    assert_eq!(
        api.post(REPORTS, &spaced).await?.screen(),
        (
            StatusCode::BAD_REQUEST,
            Some(END_MONTH_EMPTY),
            Some("end_month")
        )
    );
    // audit: CORPT00C-006
    assert!(deck_names(dir)?.is_empty());
    Ok(())
}

/// CORPT00C-017 to CORPT00C-023: `NUMVAL-C` normalises each part to its zero-padded value, and
/// the range tests run in map order, a month above 12 or a day above 31 failing on its own
/// member. Month and day `00` pass, and the year parts are never rejected here.
async fn custom_part_edit_rows(api: &Api, dir: &Path) -> TestResult {
    let cases = [
        // audit: CORPT00C-018
        (
            ["13", "32", "2022", "13", "32", "2022"],
            START_MONTH_INVALID,
            "start_month",
        ),
        // audit: CORPT00C-018
        // audit: CORPT00C-019
        (
            ["00", "32", "2022", "07", "06", "2022"],
            START_DAY_INVALID,
            "start_day",
        ),
        // The one-digit month `5` is `05` after NUMVAL-C and passes the `> '12'` test, which its
        // space-padded text `5 ` would fail.
        // audit: CORPT00C-017
        (
            ["5", "99", "2022", "07", "06", "2022"],
            START_DAY_INVALID,
            "start_day",
        ),
        // audit: CORPT00C-021
        (
            ["01", "05", "2022", "13", "32", "2022"],
            END_MONTH_INVALID,
            "end_month",
        ),
        // audit: CORPT00C-017
        // audit: CORPT00C-022
        (
            ["01", "7", "2022", "00", "32", "2022"],
            END_DAY_INVALID,
            "end_day",
        ),
    ];
    for (parts, message, field) in cases {
        assert_eq!(
            api.post(REPORTS, &custom(parts, "")).await?.screen(),
            (StatusCode::BAD_REQUEST, Some(message), Some(field))
        );
    }
    // audit: CORPT00C-006
    assert!(deck_names(dir)?.is_empty());
    Ok(())
}

/// Asserts that `reply`, to a custom request whose six parts passed their edits and whose
/// confirmation is blank, is one of the three outcomes the date check allows whatever its
/// verdict (halt H5): the start date rejected on `start_month`, the end date rejected on
/// `end_month`, or both accepted and the Custom confirmation prompt on `confirm`. No year
/// message and no comparison of the two dates can answer it.
fn assert_date_outcome(reply: &Reply) {
    let outcomes = [
        (Some(START_DATE_INVALID), Some("start_month")),
        (Some(END_DATE_INVALID), Some("end_month")),
        (Some(CUSTOM_PROMPT), Some("confirm")),
    ];
    let (status, message, field) = reply.screen();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        outcomes.contains(&(message, field)),
        "{message:?} on {field:?} is no outcome of the date check"
    );
}

/// CORPT00C-020 and CORPT00C-023 to CORPT00C-026, CSUTLDTC-002 and CSUTLDTC-030: requests whose
/// parts pass their edits reach the date check, start date first. Every assertion holds for
/// every verdict, and the same request answers the same after another request in between.
async fn custom_date_edit_rows(api: &Api, dir: &Path) -> TestResult {
    let requests = [
        // An impossible start date with a possible end date, and the reverse.
        ["02", "30", "2022", "07", "06", "2022"],
        ["01", "05", "2022", "02", "30", "2022"],
        // Year parts of one digit and of zeros: `0001` and `0000` after NUMVAL-C.
        ["01", "05", "1", "07", "06", "0000"],
        // Month and day `00`, which the part edits pass.
        ["00", "00", "2022", "07", "06", "2022"],
        // An end date before the start date, which no edit compares.
        ["07", "06", "2022", "01", "05", "2022"],
    ];
    let other = custom(["12", "31", "2021", "01", "01", "2022"], "");
    for parts in requests {
        let first = api.post(REPORTS, &custom(parts, "")).await?;
        // audit: CORPT00C-020
        // audit: CORPT00C-023
        // audit: CORPT00C-024
        // audit: CORPT00C-025
        // audit: CORPT00C-026
        // audit: CSUTLDTC-030
        assert_date_outcome(&first);
        assert_date_outcome(&api.post(REPORTS, &other).await?);
        let again = api.post(REPORTS, &custom(parts, "")).await?;
        // audit: CSUTLDTC-002
        assert_eq!(again.screen(), first.screen());
    }
    // A blank confirmation never submits, whatever the verdict.
    // audit: CORPT00C-030
    assert!(deck_names(dir)?.is_empty());
    Ok(())
}

/// CORPT00C-028, CSUTLDTC-001 and CSUTLDTC-005: a confirmed custom request with the start parts
/// `7`, `6`, `2022` and the end parts `1`, `5`, `2022`, built as `2022-07-06` and `2022-01-05`.
/// Whether CSUTLDTC accepts them is halt H5, so no verdict is asserted: an accepted pair writes
/// the Custom deck with exactly the 10-character dates CORPT00C built, and a rejected one writes
/// no deck.
async fn custom_submission_row(api: &Api, dir: &Path) -> TestResult {
    let reply = api
        .post(REPORTS, &custom(["7", "6", "2022", "1", "5", "2022"], "Y"))
        .await?;
    if reply.status == StatusCode::OK {
        // audit: CORPT00C-028
        assert_eq!(
            (reply.text("message"), reply.text("job_deck")),
            (Some(CUSTOM_SUBMITTED), Some("TRNRPT00.G0001"))
        );
        // audit: CORPT00C-028
        // audit: CSUTLDTC-001
        // audit: CSUTLDTC-005
        assert_eq!(
            read_deck(dir, "TRNRPT00.G0001")?,
            expected_deck("2022-07-06", "2022-01-05")
        );
    } else {
        let (status, message, _) = reply.screen();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!([Some(START_DATE_INVALID), Some(END_DATE_INVALID)].contains(&message));
        assert!(deck_names(dir)?.is_empty());
    }
    Ok(())
}

/// CORPT00C-017: a date part of digits with spaces before or after them keeps its value, against
/// a job-deck directory of its own so that the generations start at `G0001`.
///
/// - The part edits read the value, not the text: the months `" 5"` and `"5 "` and the day `"4 "`
///   pass as `05` and `04`, though the texts `"5 "` and `"4 "` are above `12` and `31`, so the
///   next part decides.
/// - Parts that normalise to the parts of another request get its answer, whatever CSUTLDTC's
///   verdict on the dates they build (halt H5), and a blank confirmation gets one of the outcomes
///   of the date check.
/// - Confirmed, the space-padded request either writes the Custom deck with the normalised dates
///   or is refused with a date message and writes none, the verdict deciding which.
/// - A space between two digits is not NUMVAL-C's format: the DTO's 400 on that part, no deck.
async fn custom_padded_part_rows(api: &Api) -> TestResult {
    let dir = fresh_dir("corpt00c_padded_parts")?;
    let padded = api.sharing(dir.clone(), instant(2022, 7, 18)?)?;
    let part_edits = [
        (
            [" 5", "99", "2022", "07", "06", "2022"],
            START_DAY_INVALID,
            "start_day",
        ),
        (
            ["5 ", "99", "2022", "07", "06", "2022"],
            START_DAY_INVALID,
            "start_day",
        ),
        (
            ["01", "4 ", "2022", "13", "06", "2022"],
            END_MONTH_INVALID,
            "end_month",
        ),
    ];
    for (parts, message, field) in part_edits {
        // audit: CORPT00C-017
        assert_eq!(
            padded.post(REPORTS, &custom(parts, "")).await?.screen(),
            (StatusCode::BAD_REQUEST, Some(message), Some(field)),
            "{parts:?}"
        );
    }

    let same_dates = [
        (
            [" 1", "5 ", "2022", "7 ", " 6", "2022"],
            ["01", "05", "2022", "07", "06", "2022"],
        ),
        (
            ["01", "05", "  22", "07", "06", "22  "],
            ["01", "05", "0022", "07", "06", "0022"],
        ),
    ];
    for (spaced, zero_filled) in same_dates {
        let reply = padded.post(REPORTS, &custom(spaced, "")).await?;
        assert_date_outcome(&reply);
        // audit: CORPT00C-017
        assert_eq!(
            reply.screen(),
            padded
                .post(REPORTS, &custom(zero_filled, ""))
                .await?
                .screen(),
            "{spaced:?}"
        );
    }

    let confirmed = padded
        .post(
            REPORTS,
            &custom([" 1", "5 ", "2022", "7 ", " 6", "2022"], "Y"),
        )
        .await?;
    if confirmed.status == StatusCode::OK {
        // audit: CORPT00C-017
        assert_eq!(
            (confirmed.text("message"), confirmed.text("job_deck")),
            (Some(CUSTOM_SUBMITTED), Some("TRNRPT00.G0001"))
        );
        // audit: CORPT00C-017
        assert_eq!(
            read_deck(&dir, "TRNRPT00.G0001")?,
            expected_deck("2022-01-05", "2022-07-06")
        );
    } else {
        let (status, message, _) = confirmed.screen();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!([Some(START_DATE_INVALID), Some(END_DATE_INVALID)].contains(&message));
        assert!(deck_names(&dir)?.is_empty());
    }
    let written = deck_names(&dir)?;

    // audit: CORPT00-026
    // audit: CORPT00-034
    for (parts, field) in [
        (["01", "05", "20 2", "07", "06", "2022"], "start_year"),
        (["01", "05", "2022", "07", "06", " 2 2"], "end_year"),
    ] {
        assert_eq!(
            padded.post(REPORTS, &custom(parts, "Y")).await?.screen(),
            (
                StatusCode::BAD_REQUEST,
                Some(FIELD_NUMERIC_MESSAGE),
                Some(field)
            )
        );
    }
    assert_eq!(deck_names(&dir)?, written);
    Ok(())
}

/// Asserts that no response of `api`'s ledger carries any text of CSUTLDTC's 80-byte result:
/// its consumers read only the severity and the message number, so neither the labels, the
/// picture, the tested date nor a `WS-RESULT` value reaches an endpoint.
fn assert_no_result_text(api: &Api) {
    // audit: CSUTLDTC-003
    // audit: CSUTLDTC-007
    // audit: CSUTLDTC-009
    // audit: CSUTLDTC-010
    // audit: CSUTLDTC-011
    // audit: CSUTLDTC-012
    // audit: CSUTLDTC-013
    // audit: CSUTLDTC-014
    // audit: CSUTLDTC-015
    // audit: CSUTLDTC-016
    // audit: CSUTLDTC-017
    // audit: CSUTLDTC-018
    for body in &*api.bodies.borrow() {
        for text in CSUTLDTC_RESULT_TEXT {
            assert!(
                !body.contains(text),
                "a response carries CSUTLDTC's {text:?}"
            );
        }
    }
}

/// A [`RepositoryProbe`] that answers each point it scripts with its verdict and every other
/// point with [`ProbeVerdict::Proceed`], and keeps every point it is asked at.
struct Script {
    verdicts: Vec<(ProbePoint, ProbeVerdict)>,
    seen: Mutex<Vec<ProbePoint>>,
}

impl Script {
    /// A script that answers the point of each pair of `verdicts` with the pair's verdict.
    fn new(verdicts: Vec<(ProbePoint, ProbeVerdict)>) -> Arc<Self> {
        Arc::new(Self {
            verdicts,
            seen: Mutex::default(),
        })
    }

    /// The number of calls asked at each of [`PAYMENT_POINTS`], in its order.
    fn calls(&self) -> [usize; 3] {
        let seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        PAYMENT_POINTS.map(|point| seen.iter().filter(|&&asked| asked == point).count())
    }
}

impl RepositoryProbe for Script {
    fn answer(&self, call: &ProbeCall<'_>) -> ProbeVerdict {
        let point = call.point();
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(point);
        self.verdicts
            .iter()
            .find(|(scripted, _)| *scripted == point)
            .map_or(ProbeVerdict::Proceed, |(_, verdict)| verdict.clone())
    }
}

/// A statement the server cancelled, SQLSTATE `57014`: a failure that is neither a missing row
/// nor a duplicate key.
fn cancelled() -> PersistenceError {
    PersistenceError::Database {
        sqlstate: Some("57014".to_owned()),
        constraint: None,
    }
}

/// The text of a card number that no fixture holds, assembled from its parts at run time and
/// checked against the 50 fixture numbers. Never printed.
fn synthetic_card_text() -> Outcome<String> {
    let text = format!("{}{}", "9".repeat(4), "0".repeat(12));
    if fixture_card_numbers()?.contains(&text) {
        return Err("the synthetic card number is a fixture card number".into());
    }
    Ok(text)
}

/// An endpoint 15 body.
fn pay(acct_id: &str, confirm: &str) -> Value {
    json!({"acct_id": acct_id, "confirm": confirm})
}

/// The amount `text`, such as `194.00`, as money of picture `S9(N)V99`.
fn money<const N: u32>(text: &str) -> Outcome<Money<N>> {
    Ok(Money::new(text.parse::<Decimal>()?)?)
}

/// The stored account `id`.
async fn account(store: &PgStore, id: u64) -> Outcome<AccountRecord> {
    AccountRepository::find_by_id(&mut store.clone(), PicU::new(id)?)
        .await?
        .ok_or_else(|| "a fixture account is missing".into())
}

/// The stored transaction `id`, if any.
async fn transaction(store: &PgStore, id: &str) -> Outcome<Option<TransactionRecord>> {
    Ok(TransactionRepository::find_by_id(&mut store.clone(), &PicX::from_text(id)?).await?)
}

/// The ID one above `id`, as 16 digits with leading zeros.
fn following_id(id: &PicX<16>) -> Outcome<String> {
    let next = id
        .to_text()
        .parse::<u64>()?
        .checked_add(1)
        .ok_or("no ID follows")?;
    Ok(format!("{next:016}"))
}

/// The message of a written payment with transaction ID `id` (COBIL00C-028).
fn payment_message(id: &str) -> String {
    format!("Payment successful.  Your Transaction ID is {id}.")
}

/// Inserts the first record of `dailytran.txt`, decoded by codec A as a `TRAN-RECORD` (the two
/// layouts are identical), as `posttran` would post it, and returns it.
///
/// The nine loads leave `transactions` empty, and a payment's browse of an empty file is halt
/// candidate X-COBIL00C-H3, so every payment here follows a stored transaction.
async fn seed_transaction(store: &PgStore) -> Outcome<TransactionRecord> {
    let file = fs::read(fixture_path("dailytran.txt"))?;
    let line = file
        .split(|&byte| byte == b'\n')
        .next()
        .ok_or("dailytran.txt is empty")?;
    let record = TransactionRecord::decode(line)?;
    TransactionRepository::insert(&mut store.clone(), &record).await?;
    Ok(record)
}

/// Whether `text` is a timestamp of layout `I` at whole seconds, `YYYY-MM-DD HH:MM:SS.000000`.
fn is_iso_layout(text: &str) -> bool {
    const LAYOUT: &[u8; 26] = b"9999-99-99 99:99:99.000000";
    text.len() == LAYOUT.len()
        && text.bytes().zip(LAYOUT).all(|(byte, &mask)| {
            if mask == b'9' {
                byte.is_ascii_digit()
            } else {
                byte == mask
            }
        })
}

/// Asserts that `record` is the payment record COBIL00C-025 to COBIL00C-027 describe: the fixed
/// type, category, source, description and merchant fields, the amount `amount`, the FILLER of
/// `prior`, the transaction with the highest ID before it, and both timestamps the same value,
/// checked by format only: layout `I` at whole seconds. The card number is asserted by the
/// caller.
fn assert_payment_record(
    record: &TransactionRecord,
    amount: &str,
    prior: &TransactionRecord,
) -> TestResult {
    // audit: COBIL00C-025
    assert_eq!(record.type_cd, PicX::from_text("02")?);
    assert_eq!(record.cat_cd, PicU::new(2)?);
    assert_eq!(record.source, PicX::from_text("POS TERM")?);
    assert_eq!(record.desc, PicX::from_text("BILL PAYMENT - ONLINE")?);
    assert_eq!(record.merchant_id, PicU::new(999_999_999)?);
    assert_eq!(record.merchant_name, PicX::from_text("BILL PAYMENT")?);
    assert_eq!(record.merchant_city, PicX::from_text("N/A")?);
    assert_eq!(record.merchant_zip, PicX::from_text("N/A")?);
    assert_eq!(record.filler, prior.filler);
    // audit: COBIL00C-026
    assert_eq!(record.amt, money::<9>(amount)?);
    // audit: COBIL00C-027
    assert_eq!(record.orig_ts, record.proc_ts);
    let stamp = record.orig_ts.value().ok_or("a blank payment timestamp")?;
    assert_eq!(stamp.form(), TsForm::Iso);
    let mut text = Vec::new();
    record.orig_ts.encode_into(&mut text);
    assert!(is_iso_layout(&String::from_utf8(text)?));
    Ok(())
}

/// Asserts that account 1 is still `before` and that no transaction followed `seed`.
async fn assert_unwritten(
    api: &Api,
    before: &AccountRecord,
    seed: &TransactionRecord,
) -> TestResult {
    assert_eq!(account(&api.store, 1).await?, *before);
    assert_eq!(
        transaction(&api.store, &following_id(&seed.id)?).await?,
        None
    );
    Ok(())
}

#[test]
fn cobil00c_bill_payment_rows() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let api = Api::signed_on(&db, fresh_dir("cobil00c_bill_payment_rows")?, at).await?;
        let seed = seed_transaction(&api.store).await?;
        let before = account(&api.store, 1).await?;
        pay_token_rows(&api, &before, &seed).await?;
        pay_edit_rows(&api, &before, &seed).await?;
        pay_map_rows(&api, &before, &seed).await?;
        pay_non_object_rows(&api, &before, &seed).await?;
        pay_balance_row(&api, &before, &seed).await?;
        let paid = pay_rows(&api, &before, &seed).await?;
        pay_nothing_rows(&api, &paid).await?;
        pay_truncation_rows(&api).await?;
        assert_no_card_number(&api)
    })
}

/// COBIL00C-038: without a valid token endpoint 15 answers the extractor's 401 before any edit,
/// reading no account and writing nothing.
async fn pay_token_rows(api: &Api, before: &AccountRecord, seed: &TransactionRecord) -> TestResult {
    let tampered = tampered(api);
    for token in [None, Some("not-a-token"), Some(tampered.as_str())] {
        let reply = api
            .send(BILL_PAYMENTS, &pay("00000000001", "Y"), token)
            .await?;
        // audit: COBIL00C-038
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert!(
            reply
                .text("message")
                .is_some_and(|message| !message.is_empty())
        );
    }
    // audit: COBIL00C-038
    assert_unwritten(api, before, seed).await
}

/// COBIL00C-007, COBIL00C-008, COBIL00C-010, COBIL00C-012 and COBIL00C-014: the edits before any
/// read, in the program's order (blank account, confirmation, key), and the read that finds no
/// account. None of them writes anything.
async fn pay_edit_rows(api: &Api, before: &AccountRecord, seed: &TransactionRecord) -> TestResult {
    let empty = (
        StatusCode::BAD_REQUEST,
        Some(ACCT_ID_EMPTY),
        Some("acct_id"),
    );
    let invalid = (
        StatusCode::BAD_REQUEST,
        Some(CONFIRM_INVALID),
        Some("confirm"),
    );
    let not_found = (
        StatusCode::NOT_FOUND,
        Some(ACCOUNT_NOT_FOUND),
        Some("acct_id"),
    );
    let cases = [
        // audit: COBIL00C-007
        (pay("", ""), empty),
        // audit: COBIL00C-007
        (pay("", "N"), empty),
        // audit: COBIL00C-007
        (pay("           ", "Y"), empty),
        // audit: COBIL00C-012
        (pay("00000000001", "X"), invalid),
        // The confirmation is judged before the key, so an unknown account gets its message.
        // audit: COBIL00C-012
        (pay("ABC", "x"), invalid),
        // audit: COBIL00C-008
        // audit: COBIL00C-014
        (pay("99999999999", ""), not_found),
        // audit: COBIL00C-014
        (pay("99999999999", "Y"), not_found),
        // A non-digit among the first ten characters, a short entry included, names no key.
        // audit: COBIL00C-008
        (pay("A0000000001", "y"), not_found),
        // audit: COBIL00C-008
        (pay("000000001", ""), not_found),
    ];
    for (body, expected) in cases {
        assert_eq!(api.post(BILL_PAYMENTS, &body).await?.screen(), expected);
    }
    // `N` or `n` clears the screen without reading, or even editing, the account.
    for body in [
        pay("00000000001", "N"),
        pay("00000000001", "n"),
        pay("ABC", "N"),
    ] {
        let reply = api.post(BILL_PAYMENTS, &body).await?;
        // audit: COBIL00C-010
        assert_eq!(
            (reply.status, reply.body),
            (StatusCode::OK, json!({"acct_id": "", "cleared": true}))
        );
    }
    // audit: COBIL00C-010
    // audit: COBIL00C-014
    assert_unwritten(api, before, seed).await
}

/// The server half of the `COBIL00` `both` rows: an account or a confirmation longer than its
/// map `LENGTH`, or holding text its `PIC X(n)` field cannot hold (eleven `é`, 22 bytes in the
/// 11-byte `ACTIDIN`; a control byte such as TAB), is a 400 naming the member, before any read,
/// and writes nothing.
async fn pay_map_rows(api: &Api, before: &AccountRecord, seed: &TransactionRecord) -> TestResult {
    let wide_acct_id = "é".repeat(11);
    let cases = [
        // audit: COBIL00-013
        (pay("000000000011", "Y"), "acct_id"),
        // audit: COBIL00-013
        (pay(&wide_acct_id, ""), "acct_id"),
        // audit: COBIL00-013
        (pay("0000000001\t", "Y"), "acct_id"),
        // audit: COBIL00-020
        (pay("00000000001", "YY"), "confirm"),
        // audit: COBIL00-020
        (pay("00000000001", "é"), "confirm"),
        // audit: COBIL00-020
        (pay("00000000001", "\t"), "confirm"),
    ];
    for (body, member) in cases {
        assert_eq!(
            api.post(BILL_PAYMENTS, &body).await?.screen(),
            (
                StatusCode::BAD_REQUEST,
                Some(FIELD_LENGTH_MESSAGE),
                Some(member)
            ),
            "{body}"
        );
    }
    assert_unwritten(api, before, seed).await
}

/// A body whose top-level value is not a JSON object, an array whose elements would fill the
/// members in order included, is the request layer's 400 with no field, before any edit or read,
/// and writes nothing.
async fn pay_non_object_rows(
    api: &Api,
    before: &AccountRecord,
    seed: &TransactionRecord,
) -> TestResult {
    let bodies = [
        json!(["00000000001", "Y"]),
        json!(["00000000001", ""]),
        json!([]),
        Value::Null,
        json!("00000000001"),
    ];
    for body in bodies {
        let reply = api.post(BILL_PAYMENTS, &body).await?;
        assert_eq!(
            (reply.status, reply.body),
            (StatusCode::BAD_REQUEST, json!({"message": INVALID_BODY}))
        );
    }
    assert_unwritten(api, before, seed).await
}

/// COBIL00C-011, COBIL00C-013, COBIL00C-017 and COBIL00C-019: a blank confirmation reads
/// account 1 and shows its balance of 194.00 as `+9999999999.99` edits it, with the prompt;
/// nothing is written, and the row lock ends with the request.
async fn pay_balance_row(
    api: &Api,
    before: &AccountRecord,
    seed: &TransactionRecord,
) -> TestResult {
    let reply = api.post(BILL_PAYMENTS, &pay("00000000001", "")).await?;
    // audit: COBIL00C-003
    // audit: COBIL00C-011
    // audit: COBIL00C-017
    // audit: COBIL00C-019
    // audit: ARITH-019
    assert_eq!(
        (reply.status, reply.body),
        (
            StatusCode::OK,
            json!({
                "acct_id": "00000000001",
                "current_balance": "+0000000194.00",
                "message": PAY_PROMPT,
                "cleared": false,
            })
        )
    );
    // audit: COBIL00C-013
    // audit: COBIL00C-019
    assert_unwritten(api, before, seed).await
}

/// COBIL00C-009, COBIL00C-020, COBIL00C-023, COBIL00C-025 to COBIL00C-028, COBIL00C-031 and
/// COBIL00C-034: `Y` pays the whole balance of account 1. Returns the account as the payment
/// left it.
async fn pay_rows(
    api: &Api,
    before: &AccountRecord,
    seed: &TransactionRecord,
) -> Outcome<AccountRecord> {
    let new_id = following_id(&seed.id)?;
    let reply = api.post(BILL_PAYMENTS, &pay("00000000001", "Y")).await?;
    // audit: COBIL00C-009
    // audit: COBIL00C-023
    // audit: COBIL00C-028
    // audit: COBIL00C-034
    assert_eq!(
        (reply.status, reply.body),
        (
            StatusCode::OK,
            json!({
                "acct_id": "",
                "message": payment_message(&new_id),
                "new_tran_id": new_id,
                "cleared": true,
            })
        )
    );
    // audit: COBIL00C-028
    assert_eq!(payment_message(&new_id).len(), 61);
    let written = transaction(&api.store, &new_id)
        .await?
        .ok_or("the payment was not written")?;
    // audit: ARITH-020
    assert_payment_record(&written, "194.00", seed)?;
    let xref = CardXrefRepository::find_by_acct_id(&mut api.store.clone(), before.id)
        .await?
        .ok_or("account 1 has no cross-reference")?;
    // The card of account 1's one cross-reference, compared whole and never printed.
    // audit: COBIL00C-020
    // audit: COBIL00C-025
    assert!(
        written.card_num == xref.card_num,
        "the payment's card number is not that of account 1's cross-reference"
    );
    // Its redacted form shows the last four digits only.
    assert_eq!(written.card_num.last4(), *b"3697");
    assert_eq!(written.card_num.redacted(), "************3697");
    let after = account(&api.store, 1).await?;
    // audit: COBIL00C-031
    // audit: ARITH-022
    assert_eq!(after.curr_bal, money::<10>("0.00")?);
    // audit: COBIL00C-031
    assert_eq!(
        AccountRecord {
            curr_bal: before.curr_bal,
            ..after.clone()
        },
        *before
    );
    Ok(after)
}

/// COBIL00C-018: once paid, account 1 has nothing to pay, whether the confirmation is `Y` or
/// blank, and a negative balance has nothing to pay either. No transaction is written and no
/// balance changes.
async fn pay_nothing_rows(api: &Api, paid: &AccountRecord) -> TestResult {
    let mut store = api.store.clone();
    let next = TransactionRepository::next_id(&mut store).await?;
    let owing = account(&api.store, 4).await?;
    let negative = AccountRecord {
        curr_bal: money("-25.00")?,
        ..owing
    };
    assert!(AccountRepository::update(&mut store, &negative).await?);
    let nothing = (
        StatusCode::BAD_REQUEST,
        Some(NOTHING_TO_PAY),
        Some("acct_id"),
    );
    for body in [
        pay("00000000001", "Y"),
        pay("00000000001", ""),
        pay("00000000004", ""),
        pay("00000000004", "y"),
    ] {
        // audit: COBIL00C-018
        // audit: ARITH-018
        assert_eq!(api.post(BILL_PAYMENTS, &body).await?.screen(), nothing);
    }
    // audit: COBIL00C-018
    assert_eq!(TransactionRepository::next_id(&mut store).await?, next);
    assert_eq!(account(&api.store, 1).await?, *paid);
    assert_eq!(account(&api.store, 4).await?, negative);
    Ok(())
}

/// COBIL00C-026 and E11: account 2's balance raised to 1234567890.12 pays its low-order nine
/// integer digits, 234567890.12, and keeps 1000000000.00, which then pays 0.00 and stays.
async fn pay_truncation_rows(api: &Api) -> TestResult {
    let mut store = api.store.clone();
    let rich = AccountRecord {
        curr_bal: money("1234567890.12")?,
        ..account(&api.store, 2).await?
    };
    assert!(AccountRepository::update(&mut store, &rich).await?);
    let payments = [
        ("+1234567890.12", "234567890.12", "1000000000.00"),
        ("+1000000000.00", "0.00", "1000000000.00"),
    ];
    for (shown, amount, left) in payments {
        let reply = api.post(BILL_PAYMENTS, &pay("00000000002", "")).await?;
        // audit: ARITH-019
        assert_eq!(reply.text("current_balance"), Some(shown));
        let prior = TransactionRepository::find_last(&mut store)
            .await?
            .ok_or("no transaction precedes the payment")?;
        let new_id = following_id(&prior.id)?;
        let reply = api.post(BILL_PAYMENTS, &pay("00000000002", "Y")).await?;
        assert_eq!(
            (reply.status, reply.text("new_tran_id")),
            (StatusCode::OK, Some(new_id.as_str()))
        );
        let written = transaction(&api.store, &new_id)
            .await?
            .ok_or("the payment was not written")?;
        // audit: COBIL00C-026
        // audit: ARITH-021
        assert_payment_record(&written, amount, &prior)?;
        // audit: ARITH-022
        assert_eq!(account(&api.store, 2).await?.curr_bal, money::<10>(left)?);
    }
    Ok(())
}

#[test]
fn cobil00c_store_rows() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let api = Api::signed_on(&db, fresh_dir("cobil00c_store_rows")?, at).await?;
        let seed = seed_transaction(&api.store).await?;
        pay_without_xref_row(&api, &seed).await?;
        pay_concurrent_row(&api).await?;
        pay_wrapped_id_rows(&api, &seed).await?;
        assert_no_card_number(&api)
    })
}

/// COBIL00C-021 and COBIL00C-034: an account that no cross-reference names. The payment block
/// tests no error between its steps, so the payment is written and the balance debited, and
/// the write's message replaces the cross-reference's. The card number written is
/// `HALTED(X-COBIL00C-H2)` and is not asserted.
async fn pay_without_xref_row(api: &Api, seed: &TransactionRecord) -> TestResult {
    let orphan = AccountRecord {
        id: PicU::new(99)?,
        curr_bal: money("50.00")?,
        ..account(&api.store, 1).await?
    };
    AccountRepository::insert(&mut api.store.clone(), &orphan).await?;
    let new_id = following_id(&seed.id)?;
    let reply = api.post(BILL_PAYMENTS, &pay("00000000099", "Y")).await?;
    // audit: COBIL00C-021
    // audit: COBIL00C-034
    assert_eq!(
        (
            reply.status,
            reply.text("message"),
            reply.text("new_tran_id")
        ),
        (
            StatusCode::OK,
            Some(payment_message(&new_id).as_str()),
            Some(new_id.as_str())
        )
    );
    let written = transaction(&api.store, &new_id)
        .await?
        .ok_or("the payment was not written")?;
    // audit: COBIL00C-021
    assert_payment_record(&written, "50.00", seed)?;
    // audit: COBIL00C-021
    assert_eq!(
        account(&api.store, 99).await?.curr_bal,
        money::<10>("0.00")?
    );
    Ok(())
}

/// COBIL00C-013: two payments of account 3 at once. The read for update locks the row until the
/// first request's transaction ends, so the second waits, reads the 0.00 the first left and has
/// nothing to pay: exactly one transaction is written.
async fn pay_concurrent_row(api: &Api) -> TestResult {
    let mut store = api.store.clone();
    let next = TransactionRepository::next_id(&mut store).await?;
    let body = pay("00000000003", "Y");
    let replies = api.post_concurrently(BILL_PAYMENTS, &body, 2).await?;
    let next_text = next.to_text();
    let paid = replies
        .iter()
        .filter(|reply| reply.status == StatusCode::OK)
        .count();
    // audit: COBIL00C-013
    assert_eq!(paid, 1);
    for reply in &replies {
        if reply.status == StatusCode::OK {
            assert_eq!(reply.text("new_tran_id"), Some(next_text.as_str()));
        } else {
            // audit: COBIL00C-013
            assert_eq!(
                reply.screen(),
                (
                    StatusCode::BAD_REQUEST,
                    Some(NOTHING_TO_PAY),
                    Some("acct_id")
                )
            );
        }
    }
    // audit: COBIL00C-013
    assert_eq!(
        TransactionRepository::next_id(&mut store).await?.to_text(),
        following_id(&next)?
    );
    assert_eq!(account(&api.store, 3).await?.curr_bal, money::<10>("0.00")?);
    Ok(())
}

/// COBIL00C-023, COBIL00C-029, COBIL00C-031 and COBIL00C-034: with `9999999999999999` the
/// highest ID, `ADD 1` wraps to `0000000000000000`, which a transaction already holds. The
/// write fails as `DUPREC` and keeps that transaction, yet the debit of account 1 runs and
/// stands, with no transaction recording it (E11).
async fn pay_wrapped_id_rows(api: &Api, seed: &TransactionRecord) -> TestResult {
    let mut store = api.store.clone();
    let highest = TransactionRecord {
        id: PicX::from_text("9999999999999999")?,
        ..seed.clone()
    };
    let lowest = TransactionRecord {
        id: PicX::from_text("0000000000000000")?,
        ..seed.clone()
    };
    TransactionRepository::insert(&mut store, &highest).await?;
    TransactionRepository::insert(&mut store, &lowest).await?;
    // audit: COBIL00C-023
    assert_eq!(TransactionRepository::next_id(&mut store).await?, lowest.id);
    let before = account(&api.store, 1).await?;
    let reply = api.post(BILL_PAYMENTS, &pay("00000000001", "Y")).await?;
    // audit: COBIL00C-029
    // audit: COBIL00C-034
    assert_eq!(
        reply.screen(),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(TRAN_ID_EXISTS),
            Some("acct_id")
        )
    );
    // audit: COBIL00C-029
    assert_eq!(
        transaction(&api.store, "0000000000000000").await?,
        Some(lowest)
    );
    // audit: COBIL00C-031
    // audit: COBIL00C-034
    assert_eq!(
        account(&api.store, 1).await?,
        AccountRecord {
            curr_bal: money("0.00")?,
            ..before
        }
    );
    Ok(())
}

/// COBIL00C-015: the read for update of account 1 fails for a reason other than not found,
/// whether the confirmation is `Y`, `y` or blank: 500 `Unable to lookup Account...` on
/// `acct_id`, and nothing is written. What the COBOL runs after that screen is
/// `HALTED(X-COBIL00C-H1)` (COBIL00C-016) and is not asserted.
#[test]
fn cobil00c_account_read_failure_row() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let dir = fresh_dir("cobil00c_account_read_failure_row")?;
        let api = Api::signed_on(&db, dir.clone(), at).await?;
        let seed = seed_transaction(&api.store).await?;
        let before = account(&api.store, 1).await?;
        let cases = [
            ("Y", cancelled()),
            ("y", PersistenceError::Unavailable),
            ("", PersistenceError::Check { constraint: None }),
        ];
        for (confirm, error) in cases {
            let script = Script::new(vec![(ProbePoint::AccountLock, ProbeVerdict::Fail(error))]);
            let reply = api
                .probed(&script, dir.clone(), at)?
                .post(BILL_PAYMENTS, &pay("00000000001", confirm))
                .await?;
            // audit: COBIL00C-015
            assert_eq!(
                reply.screen(),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some(ACCOUNT_LOOKUP_FAILED),
                    Some("acct_id")
                )
            );
            // audit: COBIL00C-015
            assert_eq!(script.calls(), [1, 0, 0]);
            // audit: COBIL00C-015
            assert_unwritten(&api, &before, &seed).await?;
        }
        assert_no_card_number(&api)
    })
}

/// COBIL00C-022: account 1 with a second cross-reference, so its read through the path ends
/// `DUPKEY`. The payment block tests no error between its steps, so the payment proceeds as in
/// COBIL00C-021: written with the next ID, the balance debited and the write's message the
/// answer. The card number written is `HALTED(X-COBIL00C-H2)`, reported by
/// `blocked_cobil00c_candidate_parts`, and is not asserted.
#[test]
fn cobil00c_duplicate_xref_row() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let api = Api::signed_on(&db, fresh_dir("cobil00c_duplicate_xref_row")?, at).await?;
        let seed = seed_transaction(&api.store).await?;
        let before = account(&api.store, 1).await?;
        let mut store = api.store.clone();
        let only = CardXrefRepository::find_by_acct_id(&mut store, before.id)
            .await?
            .ok_or("account 1 has no cross-reference")?;
        let synthetic = synthetic_card_text()?;
        let second = CardXrefRecord {
            card_num: CardNumber::parse(&synthetic)?,
            ..only
        };
        CardXrefRepository::insert(&mut store, &second).await?;
        // audit: COBIL00C-022
        assert_eq!(
            CardXrefRepository::find_first_two_by_acct_id(&mut store, before.id)
                .await?
                .len(),
            2
        );
        let new_id = following_id(&seed.id)?;
        let reply = api.post(BILL_PAYMENTS, &pay("00000000001", "Y")).await?;
        // audit: COBIL00C-022
        assert_eq!(
            (reply.status, reply.body),
            (
                StatusCode::OK,
                json!({
                    "acct_id": "",
                    "message": payment_message(&new_id),
                    "new_tran_id": new_id,
                    "cleared": true,
                })
            )
        );
        let written = transaction(&api.store, &new_id)
            .await?
            .ok_or("the payment was not written")?;
        // audit: COBIL00C-022
        assert_payment_record(&written, "194.00", &seed)?;
        // audit: COBIL00C-022
        assert_eq!(
            account(&api.store, 1).await?,
            AccountRecord {
                curr_bal: money("0.00")?,
                ..before
            }
        );
        assert!(
            !api.bodies
                .borrow()
                .iter()
                .any(|body| body.contains(synthetic.as_str())),
            "a response body holds the synthetic card number"
        );
        assert_no_card_number(&api)
    })
}

/// COBIL00C-030 and E11: the payment insert fails for a reason other than a duplicate ID:
/// 500 `Unable to Add Bill pay Transaction...` on `acct_id`. No transaction is written, yet the
/// rewrite after it runs and the debit of account 1 stands.
#[test]
fn cobil00c_insert_failure_row() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let dir = fresh_dir("cobil00c_insert_failure_row")?;
        let api = Api::signed_on(&db, dir.clone(), at).await?;
        let seed = seed_transaction(&api.store).await?;
        let before = account(&api.store, 1).await?;
        let script = Script::new(vec![(
            ProbePoint::TransactionInsert,
            ProbeVerdict::Fail(cancelled()),
        )]);
        let reply = api
            .probed(&script, dir, at)?
            .post(BILL_PAYMENTS, &pay("00000000001", "Y"))
            .await?;
        // audit: COBIL00C-030
        assert_eq!(
            reply.screen(),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(TRAN_WRITE_FAILED),
                Some("acct_id")
            )
        );
        // audit: COBIL00C-030
        assert_eq!(script.calls(), [1, 1, 1]);
        // audit: COBIL00C-030
        assert_eq!(
            TransactionRepository::find_last(&mut api.store.clone()).await?,
            Some(seed)
        );
        // audit: COBIL00C-030
        assert_eq!(
            account(&api.store, 1).await?,
            AccountRecord {
                curr_bal: money("0.00")?,
                ..before
            }
        );
        assert_no_card_number(&api)
    })
}

/// COBIL00C-032 (X-COBIL00C-2): the balance rewrite matches no row, which the read's row lock
/// otherwise prevents: 404 `Account ID NOT found...` on `acct_id`, and the balance of account 1
/// is unchanged. The payment written before the rewrite stands, because the transaction commits
/// whatever the rewrite gave.
#[test]
fn cobil00c_rewrite_not_found_row() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let dir = fresh_dir("cobil00c_rewrite_not_found_row")?;
        let api = Api::signed_on(&db, dir.clone(), at).await?;
        let seed = seed_transaction(&api.store).await?;
        let before = account(&api.store, 1).await?;
        let new_id = following_id(&seed.id)?;
        let script = Script::new(vec![(ProbePoint::AccountUpdate, ProbeVerdict::NoRow)]);
        let reply = api
            .probed(&script, dir, at)?
            .post(BILL_PAYMENTS, &pay("00000000001", "Y"))
            .await?;
        // audit: COBIL00C-032
        assert_eq!(
            reply.screen(),
            (
                StatusCode::NOT_FOUND,
                Some(ACCOUNT_NOT_FOUND),
                Some("acct_id")
            )
        );
        // audit: COBIL00C-032
        assert_eq!(script.calls(), [1, 1, 1]);
        // audit: COBIL00C-032
        assert_eq!(account(&api.store, 1).await?, before);
        let written = transaction(&api.store, &new_id)
            .await?
            .ok_or("the payment was not written")?;
        // audit: COBIL00C-032
        assert_payment_record(&written, "194.00", &seed)?;
        assert_no_card_number(&api)
    })
}

/// COBIL00C-033: the balance rewrite fails: 500 `Unable to Update Account...` on `acct_id`
/// replaces the write's message, and the balance of account 1 is unchanged. After a written
/// payment (COBIL00C-028) that transaction stays written; after a failed insert (COBIL00C-030)
/// nothing is written at all.
#[test]
fn cobil00c_rewrite_failure_rows() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let dir = fresh_dir("cobil00c_rewrite_failure_rows")?;
        let api = Api::signed_on(&db, dir.clone(), at).await?;
        seed_transaction(&api.store).await?;
        let before = account(&api.store, 1).await?;
        let update_failed = (
            ProbePoint::AccountUpdate,
            ProbeVerdict::Fail(PersistenceError::Unavailable),
        );
        let cases = [
            (vec![update_failed.clone()], true),
            (
                vec![
                    (
                        ProbePoint::TransactionInsert,
                        ProbeVerdict::Fail(cancelled()),
                    ),
                    update_failed,
                ],
                false,
            ),
        ];
        let mut store = api.store.clone();
        for (verdicts, kept) in cases {
            let prior = TransactionRepository::find_last(&mut store)
                .await?
                .ok_or("no transaction precedes the payment")?;
            let new_id = following_id(&prior.id)?;
            let script = Script::new(verdicts);
            let reply = api
                .probed(&script, dir.clone(), at)?
                .post(BILL_PAYMENTS, &pay("00000000001", "Y"))
                .await?;
            // audit: COBIL00C-033
            assert_eq!(
                reply.screen(),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some(ACCOUNT_UPDATE_FAILED),
                    Some("acct_id")
                )
            );
            // audit: COBIL00C-033
            assert_eq!(script.calls(), [1, 1, 1]);
            // audit: COBIL00C-033
            assert_eq!(account(&api.store, 1).await?, before);
            let found = transaction(&api.store, &new_id).await?;
            if kept {
                // audit: COBIL00C-033
                assert_payment_record(
                    &found.ok_or("the payment was not written")?,
                    "194.00",
                    &prior,
                )?;
            } else {
                // audit: COBIL00C-033
                assert_eq!(found, None);
                assert_eq!(
                    TransactionRepository::find_last(&mut store).await?,
                    Some(prior)
                );
            }
        }
        assert_no_card_number(&api)
    })
}

/// A store over the pool `carddemo-api` serves requests from, built as `carddemo-api` builds
/// its own: `runtime::db::connect_for_requests` on `database_url`, failing at once while the
/// pool's refusal watch finds the server refusing connections.
async fn request_store(database_url: String) -> Outcome<PgStore> {
    let config = Config {
        database_url,
        jwt_secret: SECRET.to_owned(),
        ..Config::default()
    };
    let (pool, refusals) = connect_for_requests(&config).await?;
    Ok(PgStore::new(pool)
        .failing_fast_while(move || refusals.server_refusing())
        .await)
}

/// A current-thread runtime with every driver, the one Tokio's test attribute would build, made
/// here so that no test macro expands into a lint suppression or an unchecked pin.
fn current_thread() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// A runtime of four worker threads with every driver, the one Tokio's test attribute would build
/// for `flavor = "multi_thread", worker_threads = 4`, made here so that no test macro expands
/// into a lint suppression or an unchecked pin.
fn multi_thread() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
}

/// How long a transaction of the test holds account 1's row lock while a request over the
/// request pool waits for it: longer than the 5 seconds after which a request once gave up, so
/// a wait that ended before the holder did fails the test.
const LOCK_HOLD: Duration = Duration::from_secs(6);

/// COBIL00C-013 and COBIL00C-015 over the pool `carddemo-api` serves requests from, while a
/// transaction of the test holds the row lock of account 1.
///
/// - On the container's own URL, whose sessions keep the server's lock and statement settings,
///   a balance request (blank confirmation) and then a payment (`Y`) each wait in the account
///   read for update for as long as the holder keeps the lock, [`LOCK_HOLD`]. The holder then
///   commits a new balance, and the request reads it: the balance request shows it and the
///   payment pays it (COBIL00C-013).
/// - With a `DATABASE_URL` that sets an operator's own bound, which reaches the server
///   unchanged, the server ends the wait, and both requests answer `Unable to lookup
///   Account...` with 500 no sooner than that bound and before [`LOCK_HOLD`]; once the lock is
///   released, nothing is written (COBIL00C-015):
///   1. a `lock_timeout` of 1 second (`55P03`);
///   2. no lock timeout and a `statement_timeout` of 2 seconds, which cancels the read
///      (`57014`).
#[test]
fn cobil00c_lock_wait_rows() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let dir = fresh_dir("cobil00c_lock_wait_rows")?;
        let holder = PgStore::new(db.pool().clone());
        let seed = seed_transaction(&holder).await?;
        let before = account(&holder, 1).await?;
        let bounded = [
            (
                "options[lock_timeout]=1s",
                "-c lock_timeout=1s",
                Duration::from_secs(1),
            ),
            (
                "options[lock_timeout]=0&options[statement_timeout]=2s",
                "-c lock_timeout=0 -c statement_timeout=2s",
                Duration::from_secs(2),
            ),
        ];
        let plain = request_store(db.database_url().to_owned()).await?;
        // The request pool's sessions carry exactly the server options the URL names.
        assert_eq!(plain.pool().connect_options().get_options(), None);
        let plain = Api::signed_on_over(plain, dir.clone(), at).await?;
        let mut apis = Vec::new();
        for (query, options, _) in bounded {
            let store = request_store(format!("{}?{query}", db.database_url())).await?;
            assert_eq!(store.pool().connect_options().get_options(), Some(options));
            apis.push(Api::signed_on_over(store, dir.clone(), at).await?);
        }

        let mut lock = holder.begin().await?;
        let held = AccountRepository::lock_by_id(&mut lock, PicU::new(1)?).await?;
        assert_eq!(held.as_ref(), Some(&before));
        for (api, (query, _, from)) in apis.iter().zip(bounded) {
            for confirm in ["", "Y"] {
                let body = pay("00000000001", confirm);
                let started = Instant::now();
                let reply = tokio::time::timeout(LOCK_HOLD, api.post(BILL_PAYMENTS, &body))
                    .await
                    .map_err(|_| {
                        format!("{query:?} {confirm:?}: the bound did not end the wait")
                    })??;
                let waited = started.elapsed();
                // audit: COBIL00C-015
                assert_eq!(
                    reply.screen(),
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Some(ACCOUNT_LOOKUP_FAILED),
                        Some("acct_id")
                    ),
                    "{query:?} {confirm:?}"
                );
                assert!(
                    from <= waited,
                    "{query:?} {confirm:?}: the read waited {waited:?}, less than {from:?}"
                );
            }
        }
        lock.rollback().await?;
        for api in &apis {
            // audit: COBIL00C-015
            assert_unwritten(api, &before, &seed).await?;
            assert_no_card_number(api)?;
        }
        lock_wait_rows(&plain, &holder, before, &seed).await
    })
}

/// The COBIL00C-013 half of [`cobil00c_lock_wait_rows`], over `plain`, the request pool on the
/// container's own URL: a balance request and then a payment for account 1, each waiting for the
/// row lock of `holder` and then reading the balance it committed. `before` is account 1 and
/// `seed` the last transaction before either.
async fn lock_wait_rows(
    plain: &Api,
    holder: &PgStore,
    before: AccountRecord,
    seed: &TransactionRecord,
) -> TestResult {
    let reply = after_lock_wait(plain, holder, "", "250.00").await?;
    // audit: COBIL00C-013
    assert_eq!(
        (reply.status, reply.body),
        (
            StatusCode::OK,
            json!({
                "acct_id": "00000000001",
                "current_balance": "+0000000250.00",
                "message": PAY_PROMPT,
                "cleared": false,
            })
        )
    );
    let left = AccountRecord {
        curr_bal: money("250.00")?,
        ..before.clone()
    };
    assert_unwritten(plain, &left, seed).await?;

    let new_id = following_id(&seed.id)?;
    let reply = after_lock_wait(plain, holder, "Y", "300.00").await?;
    // audit: COBIL00C-013
    assert_eq!(
        (reply.status, reply.body),
        (
            StatusCode::OK,
            json!({
                "acct_id": "",
                "message": payment_message(&new_id),
                "new_tran_id": new_id,
                "cleared": true,
            })
        )
    );
    let written = transaction(&plain.store, &new_id)
        .await?
        .ok_or("the payment was not written")?;
    // audit: COBIL00C-013
    assert_payment_record(&written, "300.00", seed)?;
    let paid = AccountRecord {
        curr_bal: money("0.00")?,
        ..before
    };
    assert_eq!(account(&plain.store, 1).await?, paid);
    assert_no_card_number(plain)
}

/// The reply to endpoint 15 for account 1 with `confirm`, sent over `api` while a transaction of
/// `holder` holds the account's row lock and has set its balance to `balance`: the request
/// must still be waiting after [`LOCK_HOLD`], and it must answer within another [`LOCK_HOLD`]
/// once the holder commits.
async fn after_lock_wait(
    api: &Api,
    holder: &PgStore,
    confirm: &str,
    balance: &str,
) -> Outcome<Reply> {
    let mut lock = holder.begin().await?;
    let held = AccountRepository::lock_by_id(&mut lock, PicU::new(1)?)
        .await?
        .ok_or("account 1 is not stored")?;
    let left = AccountRecord {
        curr_bal: money(balance)?,
        ..held
    };
    assert!(AccountRepository::update(&mut lock, &left).await?);
    let body = pay("00000000001", confirm);
    let mut request = Box::pin(api.post(BILL_PAYMENTS, &body));
    // audit: COBIL00C-013
    assert!(
        tokio::time::timeout(LOCK_HOLD, request.as_mut())
            .await
            .is_err(),
        "{confirm:?}: the request answered before the row lock was released"
    );
    lock.commit().await?;
    tokio::time::timeout(LOCK_HOLD, request)
        .await
        .map_err(|_| format!("{confirm:?}: no answer once the row lock was released"))?
}

/// Waits, for at most a minute, until a connection to `database_url` is refused, as once its
/// container is removed. While the removal is under way a connection can also fail otherwise,
/// for example on a reset, which a pool does not retry.
///
/// `database_url` names an IPv4 address: a connection to `localhost` that also fails on `::1`,
/// where the container maps no port, ends as `address not available`, which a pool does not
/// retry either.
async fn wait_until_refused(database_url: &str) -> TestResult {
    let config = Config {
        database_url: database_url.to_owned(),
        ..Config::default()
    };
    let started = Instant::now();
    loop {
        let refused = connect(&config).await.is_err_and(|error| {
            ErrorSummary::from(&error).to_string() == "io (connection refused)"
        });
        if refused {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(60) {
            return Err("the server's port does not refuse connections".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// How long a payment may take to fail once the request pool's refusal watch has found the
/// server refusing connections: well under the pool's acquire timeout.
const FAIL_FAST_BOUND: Duration = Duration::from_secs(1);

/// How many connections the request pool holds idle when the outage of [`cobil00c_outage_row`]
/// begins, each of which the removed server leaves broken.
const IDLE_CONNECTIONS: usize = 4;

/// Whether `condition` holds within `within`, tested every 10 milliseconds.
async fn eventually(within: Duration, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Leaves `count` connections idle in the request pool of `api`, as a busy period does: a
/// transaction of `holder` takes account 1's row lock, and `count` balance requests for account
/// 1, in flight at once, each wait for it on a connection of their own until the pool holds
/// `count` connections and none idle. The transaction then ends, and each request shows the
/// balance and hands its connection back.
async fn hold_idle_connections(api: &Api, holder: &PgStore, count: usize) -> TestResult {
    let pool = api.store.pool();
    // TRUNCATION: checked, never lossy. The only caller passes `IDLE_CONNECTIONS`, 4
    // connections, which `u32` holds; a refusal is a test failure, never a wrapped count.
    let opened = u32::try_from(count)?;
    let mut lock = holder.begin().await?;
    AccountRepository::lock_by_id(&mut lock, PicU::new(1)?)
        .await?
        .ok_or("account 1 is not stored")?;
    let body = pay("00000000001", "");
    let mut requests = Box::pin(api.post_concurrently(BILL_PAYMENTS, &body, count));
    // Polled once, the requests run as tasks of their own, and none can answer while the lock
    // is held.
    assert!(
        tokio::time::timeout(Duration::from_millis(10), requests.as_mut())
            .await
            .is_err(),
        "a balance request answered while the row lock was held"
    );
    let all_in_use = eventually(LOCK_HOLD, || pool.size() == opened && pool.num_idle() == 0).await;
    lock.rollback().await?;
    assert!(
        all_in_use,
        "the request pool did not open a connection for each waiting request"
    );
    for reply in requests.await? {
        assert_eq!(
            (reply.status, reply.text("message")),
            (StatusCode::OK, Some(PAY_PROMPT))
        );
    }
    let all_idle = eventually(LOCK_HOLD, || {
        pool.size() == opened && pool.num_idle() == count
    });
    assert!(
        all_idle.await,
        "the request pool does not hold {count} idle connections"
    );
    Ok(())
}

/// A `PostgreSQL` outage, over a store built as `carddemo-api` builds its own, whose pool holds
/// [`IDLE_CONNECTIONS`] idle connections when the server goes away. Once the server's port
/// refuses connections, the first payment's transaction cannot begin: the idle connection it
/// is handed fails its check, and the pool retries the refused connection it opens instead
/// until its acquire timeout, `runtime::db::ACQUIRE_TIMEOUT`. The pool's refusal watch
/// meanwhile finds a caller waiting without the server answering, probes the port and finds it
/// refusing. Every payment that follows fails at once, without waiting for a connection,
/// although broken connections are still idle in the pool. Endpoint 15 answers each with the
/// API's fixed 500 for a store failure without an audited message: the first within twice the
/// acquire timeout, rather than after the 30 seconds a `sqlx` pool waits by default, and each
/// of the others, one more than the idle connections, within [`FAIL_FAST_BOUND`].
///
/// The pool connects to `127.0.0.1`, as the local stack's `DATABASE_URL` does, rather than to
/// the container's `localhost` (see [`wait_until_refused`]).
#[test]
fn cobil00c_outage_row() -> TestResult {
    current_thread()?.block_on(async {
        let db = start_loaded_postgres().await?;
        let at = instant(2022, 7, 18)?;
        let database_url = db.database_url().replacen("@localhost:", "@127.0.0.1:", 1);
        let store = request_store(database_url.clone()).await?;
        let api = Api::signed_on_over(store, fresh_dir("cobil00c_outage_row")?, at).await?;
        let holder = PgStore::new(db.pool().clone());
        hold_idle_connections(&api, &holder, IDLE_CONNECTIONS).await?;
        drop(holder);
        drop(db);
        wait_until_refused(&database_url).await?;
        let body = pay("00000000001", "Y");
        let unavailable = (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(UNABLE_TO_PROCESS),
            None,
        );
        for attempt in 1..=IDLE_CONNECTIONS + 1 {
            let bound = if attempt == 1 {
                ACQUIRE_TIMEOUT * 2
            } else {
                FAIL_FAST_BOUND
            };
            let started = Instant::now();
            let reply = api.post(BILL_PAYMENTS, &body).await?;
            let waited = started.elapsed();
            assert_eq!(reply.screen(), unavailable, "payment {attempt}");
            assert!(
                waited < bound,
                "payment {attempt} waited {waited:?} for a connection, not less than {bound:?}"
            );
        }
        assert_no_card_number(&api)
    })
}

// audit: CORPT00C-027
// audit: CSUTLDTC-004
// audit: CSUTLDTC-006
// audit: CSUTLDTC-008
// audit: CSUTLDTC-031
#[test]
#[ignore = "BLOCKED: HALTED(H5) which custom report dates CSUTLDTC accepts is the verdict of \
            CEEDAYS; no parity assertion (AAP 0.8.5)"]
fn blocked_h5_date_validity() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: CORPT00C-027, CSUTLDTC-004, CSUTLDTC-006, CSUTLDTC-008 and CSUTLDTC-031 (H5), \
         and the date verdicts of CSUTLDTC-007 and CSUTLDTC-009 to CSUTLDTC-018: which start \
         and end dates are accepted, which feedback token, severity, message number and \
         RETURN-CODE CEEDAYS yields, and so which of CORPT00C-025, CORPT00C-026 and \
         CORPT00C-028 a date reaches, live in Language Environment, outside the repository"
            .into(),
    )
}

// audit: COBIL00C-016
#[test]
#[ignore = "BLOCKED: HALTED(X-COBIL00C-H1) the balance move after the send on an account record \
            no read filled; no parity assertion (AAP 0.8.5)"]
fn blocked_x_cobil00c_h1_unread_account_record() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COBIL00C-016 (X-COBIL00C-H1): after the screens of COBIL00C-010, COBIL00C-012, \
         COBIL00C-014 and COBIL00C-015, the COBOL moves ACCT-CURR-BAL of an unfilled record into \
         the edited balance, and whether the task then ends normally depends on runtime storage"
            .into(),
    )
}

// audit: COBIL00C-024
#[test]
#[ignore = "BLOCKED: HALTED(X-COBIL00C-H3) the transaction browse of an empty or failing file; \
            no parity assertion (AAP 0.8.5)"]
fn blocked_x_cobil00c_h3_transaction_browse() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COBIL00C-024 (X-COBIL00C-H3): STARTBR at HIGH-VALUES on an empty file, and \
         READPREV and ENDBR after a failed browse, are defined by CICS, so neither the ID \
         0000000000000001 nor the answer to a failed browse is a parity result"
            .into(),
    )
}

#[test]
#[ignore = "BLOCKED: HALTED(X-COBIL00C-H2) and HALTED(X-COBIL00C-H4) parts of rows proved \
            above; no parity assertion (AAP 0.8.5)"]
fn blocked_cobil00c_candidate_parts() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: the card number a payment writes with no cross-reference or after DUPKEY \
         (COBIL00C-021, COBIL00C-022; X-COBIL00C-H2), and the result of an account entry whose \
         first ten characters are digits and whose eleventh is not (COBIL00C-008, COBIL00C-014; \
         X-COBIL00C-H4), depend on storage or compiler options the repository does not fix"
            .into(),
    )
}

#[test]
#[ignore = "BLOCKED: HALTED(X-CORPT00C-H1), HALTED(X-CORPT00C-H2) and HALTED(X-CORPT00C-H3) \
            parts of rows proved above; no parity assertion (AAP 0.8.5)"]
fn blocked_corpt00c_candidate_parts() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: NUMVAL-C of a date part that is not digits (CORPT00C-017; X-CORPT00C-H1), the \
         records a failed JOBS write leaves and what JES makes of concurrent tasks' records \
         interleaved in one reader (CORPT00C-036, CORPT00C-037; X-CORPT00C-H2), and the Monthly \
         end date of a clock outside January 1601 to November 9999 (CORPT00C-009; \
         X-CORPT00C-H3) have no single COBOL result"
            .into(),
    )
}
