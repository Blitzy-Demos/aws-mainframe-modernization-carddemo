//! Binding g1b for CPVS and CPVD: the `server` rows of `modern/audit/online/COPAUS0C.md`,
//! `COPAUS1C.md` and `COPAUS2C.md`, and the server half of the one `both` row of the `COPAU00`
//! and `COPAU01` maps in `modern/audit/screens.md`, proved through endpoints 21, 22 and 23.
//!
//! # Harness
//!
//! Every test starts its own fixture-loaded `postgres:16` container through
//! `test_support::fixtures::start_loaded_postgres`, the only schema source the tests reach
//! (Single Schema Executor, Local-Only Runtime). The API is `api::app::app` over a `PgStore`,
//! called in process with `tower::ServiceExt::oneshot`, under a `FixedClock` stopped at
//! 2022-07-18 10:30:00.000, as `USER0001`, a `U` user, signed on through endpoint 1. The
//! pending authorizations and the cross-references a case needs are seeded through the real
//! repositories (Repository Method Naming); `pending_auth_*` and `auth_fraud` start empty.
//!
//! A store failure is produced against the real store, never through a test double, because
//! `AppState` holds the concrete `PgStore`:
//!
//! - a second pool on the same database, closed before the request, fails the first read;
//! - a second pool whose sessions carry `lock_timeout`, beside a transaction of the test that
//!   holds a row lock, fails exactly the statement that needs that row, the insert into
//!   `auth_fraud` or the replace of the detail, and leaves every earlier statement untouched.
//!
//! # Halted rows
//!
//! A row a halt blocks keeps its tag, on a `BLOCKED` report printed by the test that reaches
//! the halted case, and no value is asserted for it (`modern/audit/README.md`, "Layers and
//! required tags"): halt H4 for the CICS and IMS codes of COPAUS0C-056 and COPAUS1C-038, halt
//! H3 for COPAUS2C-011, and the halt candidates X-COPAUS1C-H1, X-COPAUS1C-H2 and X-COPAUS2C-H3
//! for COPAUS1C-027, COPAUS1C-029, COPAUS1C-032 and COPAUS2C-001. Where a halted row also names
//! what stays assertable, the fixed pieces of COPAUS2C-005 and COPAUS2C-010 included, those
//! pieces are asserted and the halted positions skipped. The committed fraud toggle,
//! COPAUS1C-032's success branch, is still driven once to check the replacement's intended
//! behaviour; its checks carry no tag, so it is a parity result for no row. The two ignored
//! tests at the end name the halts H3 and H4 and carry no tag.
//!
//! # Rows this file cannot prove
//!
//! Each needs a store failure after the first read of its request, or an observation inside
//! endpoint 23's transaction, which only a test double over the repository traits gives, and
//! `AppState` admits none. They carry no tag here, so the audit coverage check reports them:
//!
//! - COPAUS0C-017, COPAUS0C-019, COPAUS0C-030, COPAUS0C-037 and COPAUS0C-038, the account,
//!   customer, summary, detail and repositioning reads failing after the cross-reference read
//!   succeeded; COPAUS1C-011 and COPAUS1C-026, the detail and next-detail reads failing after
//!   the summary read succeeded. Plain reads take no lock, so no held lock can fail them.
//! - COPAUS1C-028, COPAUS1C-030 and COPAUS1C-034, and COPAUS2C-006: the flipped flag, the
//!   customer passed to COPAUS2C, the key handed back and the column moves show only in the
//!   writes endpoint 23 commits, COPAUS1C-032's halted branch, or in a recording double.
//!
//! # Rules
//!
//! - Cardholder Data Redaction: no test writes a fixture card number; the card of account 50
//!   is read from its cross-reference and compared as a `CardNumber`. Every response body is
//!   kept and scanned for the 50 fixture card numbers, and no assertion message prints a body.
//! - Typed Dates and Timestamps: `auth_fraud.auth_ts` is compared as a `NaiveDateTime`. The
//!   dates and times endpoints 21 and 22 show come from `PIC X(06)` text the rule does not
//!   cover and travel as the display strings the audits give (COPAUS0C-034, COPAUS1C-013).
//! - Checked Integer Narrowing: every PIC value is built through its checked constructor.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use chrono::{NaiveDate, NaiveDateTime};
use domain::copybook::{Count, PicS, PicU, PicX};
use domain::money::{Decimal, Money};
use domain::online::copaus2c::AuthFraudRow;
use domain::records::{
    AccountRecord, CardXrefRecord, PendingAuthDetailRecord, PendingAuthSummaryRecord,
};
use domain::{CardNumber, Clock, FixedClock};
use persistence::{
    AccountRepository, AuthFraudRepository, CardXrefRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PgStore,
};
use runtime::config::Config;
use serde_json::{Value, json};
use test_support::fixtures::{fixture_card_numbers, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use tower::ServiceExt;

/// The result of every test and helper.
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The HS256 secret of the API under test, a local-only value.
const JWT_SECRET: &str = "endpoints-authorizations-local-secret";

/// The account, customer and card of `cardxref.txt` line 1, under which the authorizations are
/// seeded, and the account as its 11 digits.
const ACCT_50: u64 = 50;
const ACCT_50_TEXT: &str = "00000000050";

/// The redacted form of the card of account 50: twelve `*` and its last four digits (E32).
const CARD_50_REDACTED: &str = "************5740";

/// The fixture accounts of `cardxref.txt` lines 2 and 3: one gets a summary with no detail, the
/// other has no summary.
const ACCT_27_TEXT: &str = "00000000027";
const ACCT_2_TEXT: &str = "00000000002";

/// A rendered key between the first two seeded keys, which no detail holds.
const KEY_ABSENT: &str = "0778000898469750";

/// The text endpoints 21 and 22 never return, because they schedule no PSB (COPAUS0C-051,
/// COPAUS1C-007).
const PSB_TEXT: &str = "System error while scheduling PSB";

/// Endpoint 23's failure text when the replace of the detail fails, before the IMS status
/// (COPAUS1C-033).
const TAGGING_ERROR: &str = " System error while FRAUD Tagging, ROLLBACK||";

/// COPAUS2C's insert-failure text: bytes 1-24, then bytes 32-40 between the two Db2 codes
/// (COPAUS2C-010).
const SYSTEM_ERROR: &str = " SYSTEM ERROR DB2: CODE:";
const STATE_LABEL: &str = ", STATE: ";

/// One seeded detail: its key parts and the fields in which the details differ. The amounts are
/// in cents.
struct Spec {
    date_9c: i64,
    time_9c: i64,
    orig_date: &'static str,
    orig_time: &'static str,
    auth_type: &'static str,
    resp: &'static str,
    reason: &'static str,
    transaction_cents: i64,
    approved_cents: i64,
    transaction_id: &'static str,
    match_status: &'static str,
}

/// The six details of account 50, in ascending key order. The keys are CP00's inverted ones,
/// `99999 − YYDDD` for 2022-07-18, 07-17 and 07-16 and `999999999 − HHMMSSmmm` for the
/// processing time, so the newest authorization comes first (COPAUS0C-031).
const SPECS: [Spec; 6] = [
    // 10:15:30.250 on 2022-07-18, approved below the amount asked for.
    Spec {
        date_9c: 77_800,
        time_9c: 898_469_749,
        orig_date: "220718",
        orig_time: "101530",
        auth_type: "SALE",
        resp: "00",
        reason: "0000",
        transaction_cents: 200_000,
        approved_cents: 123_456,
        transaction_id: "TXN000000000001",
        match_status: "P",
    },
    // 09:05:00.000, declined for insufficient funds.
    Spec {
        date_9c: 77_800,
        time_9c: 909_499_999,
        orig_date: "220718",
        orig_time: "090500",
        auth_type: "PURC",
        resp: "05",
        reason: "4100",
        transaction_cents: 75_000,
        approved_cents: 0,
        transaction_id: "PTX220718A00002",
        match_status: "D",
    },
    // 23:59:59.999 on 2022-07-17, declined with a reason outside the decline table.
    Spec {
        date_9c: 77_801,
        time_9c: 764_040_000,
        orig_date: "220717",
        orig_time: "235959",
        auth_type: "SALE",
        resp: "05",
        reason: "4500",
        transaction_cents: 1_999,
        approved_cents: 0,
        transaction_id: "PTX220717A00003",
        match_status: "E",
    },
    // 12:00:00.000, request text that names no calendar date and an amount wider than the
    // display picture.
    Spec {
        date_9c: 77_801,
        time_9c: 879_999_999,
        orig_date: "241399",
        orig_time: "120000",
        auth_type: "SALE",
        resp: "00",
        reason: "0000",
        transaction_cents: 123_456_789_012,
        approved_cents: 123_456_789_012,
        transaction_id: "PTX241399A00004",
        match_status: "M",
    },
    // 08:00:00.500 on 2022-07-16, approved.
    Spec {
        date_9c: 77_802,
        time_9c: 919_999_499,
        orig_date: "220716",
        orig_time: "080000",
        auth_type: "SALE",
        resp: "00",
        reason: "0000",
        transaction_cents: 4_567,
        approved_cents: 4_567,
        transaction_id: "PTX220716A00005",
        match_status: "P",
    },
    // 07:00:00.000, declined for an invalid card.
    Spec {
        date_9c: 77_802,
        time_9c: 929_999_999,
        orig_date: "220716",
        orig_time: "070000",
        auth_type: "PURC",
        resp: "05",
        reason: "3100",
        transaction_cents: 2_500,
        approved_cents: 0,
        transaction_id: "PTX220716A00006",
        match_status: "D",
    },
];

/// The detail of the lock tests: processed at 13:45:01.789 on 2022-07-18 and requested on
/// 2022-07-15 at 13:45:00, so its `AUTH_TS` joins the request date with the processing time.
const LOCKED_SPEC: Spec = Spec {
    date_9c: 77_800,
    time_9c: 865_498_210,
    orig_date: "220715",
    orig_time: "134500",
    auth_type: "SALE",
    resp: "00",
    reason: "0000",
    transaction_cents: 9_900,
    approved_cents: 9_900,
    transaction_id: "PTX220715A00007",
    match_status: "P",
};

/// The rendered key of [`LOCKED_SPEC`].
const LOCKED_KEY: &str = "0778000865498210";

/// The rendered keys of [`SPECS`], in order: a sign digit and 5 date digits, then a sign digit
/// and 9 time digits.
const KEYS: [&str; 6] = [
    "0778000898469749",
    "0778000909499999",
    "0778010764040000",
    "0778010879999999",
    "0778020919999499",
    "0778020929999999",
];

/// The members of an endpoint 21 row, in the order of [`ROWS`].
const ROW_MEMBERS: [&str; 8] = [
    "auth_key",
    "transaction_id",
    "date",
    "time",
    "auth_type",
    "approval",
    "match_status",
    "amount",
];

/// Endpoint 21's row for each detail of [`SPECS`]: key, Transaction ID (the 15 stored bytes and
/// a space), Date `MM/DD/YY`, Time `HH:MM:SS`, Type, A/D, STS and the approved amount through
/// `-zzzzzzz9.99` (COPAUS0C-032 to COPAUS0C-034).
const ROWS: [[&str; 8]; 6] = [
    [
        KEYS[0],
        "TXN000000000001 ",
        "07/18/22",
        "10:15:30",
        "SALE",
        "A",
        "P",
        "     1234.56",
    ],
    [
        KEYS[1],
        "PTX220718A00002 ",
        "07/18/22",
        "09:05:00",
        "PURC",
        "D",
        "D",
        "        0.00",
    ],
    [
        KEYS[2],
        "PTX220717A00003 ",
        "07/17/22",
        "23:59:59",
        "SALE",
        "D",
        "E",
        "        0.00",
    ],
    [
        KEYS[3],
        "PTX241399A00004 ",
        "13/99/24",
        "12:00:00",
        "SALE",
        "A",
        "M",
        " 34567890.12",
    ],
    [
        KEYS[4],
        "PTX220716A00005 ",
        "07/16/22",
        "08:00:00",
        "SALE",
        "A",
        "P",
        "       45.67",
    ],
    [
        KEYS[5],
        "PTX220716A00006 ",
        "07/16/22",
        "07:00:00",
        "PURC",
        "D",
        "D",
        "        0.00",
    ],
];

/// One response: its status and its JSON body.
struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    /// The message member, or a marker when the body has none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }

    /// The `ScreenError` of an error response: its status, message and field.
    fn error(&self) -> (StatusCode, &str, Option<&str>) {
        (
            self.status,
            self.message(),
            self.body.get("field").and_then(Value::as_str),
        )
    }

    /// The string at `pointer`.
    fn text(&self, pointer: &str) -> TestResult<&str> {
        text(&self.body, pointer)
    }

    /// Fails unless the response is a 200, naming the message of an error response.
    fn expect_ok(&self) -> TestResult {
        if self.status == StatusCode::OK {
            Ok(())
        } else {
            Err(format!("status {}: {}", self.status, self.message()).into())
        }
    }
}

/// The API as one signed-on user calls it, and every response body it has answered.
struct Api {
    router: Router,
    token: String,
    bodies: Vec<String>,
}

impl Api {
    /// `USER0001`, a `U` user, signed on with `PASSWORD` through endpoint 1 of `router`.
    async fn signed_on(router: Router) -> TestResult<Self> {
        let mut api = Self::with_token(router, "");
        let credentials = json!({"user_id": "USER0001", "password": "PASSWORD"});
        let reply = api
            .send(Method::POST, "/api/v1/signon", Some(credentials))
            .await?;
        reply.expect_ok()?;
        reply.text("/token")?.clone_into(&mut api.token);
        Ok(api)
    }

    /// `router` called with the bearer `token` another API signed on for: the token is
    /// verified by its signature alone, so every router with the same secret accepts it.
    fn with_token(router: Router, token: &str) -> Self {
        Self {
            router,
            token: token.to_owned(),
            bodies: Vec::new(),
        }
    }

    async fn get(&mut self, uri: &str) -> TestResult<Reply> {
        self.send(Method::GET, uri, None).await
    }

    async fn post(&mut self, uri: &str) -> TestResult<Reply> {
        self.send(Method::POST, uri, None).await
    }

    /// One request, in process, its body kept for [`Api::assert_redacted`].
    async fn send(&mut self, method: Method, uri: &str, body: Option<Value>) -> TestResult<Reply> {
        let mut request = Request::builder().method(method).uri(uri);
        if !self.token.is_empty() {
            request = request.header(header::AUTHORIZATION, format!("Bearer {}", self.token));
        }
        let request = match body {
            Some(json) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json.to_string()))?,
            None => request.body(Body::empty())?,
        };
        let response = self.router.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let text = String::from_utf8(bytes.to_vec())?;
        let body = serde_json::from_str(&text)?;
        self.bodies.push(text);
        Ok(Reply { status, body })
    }

    /// Cardholder Data Redaction: no body answered so far holds a fixture card number, and none
    /// holds `PSB_TEXT`, which no endpoint produces. No message prints a body or a needle.
    fn assert_redacted(&self) -> TestResult {
        let needles = fixture_card_numbers()?;
        let leaks = self
            .bodies
            .iter()
            .filter(|body| needles.iter().any(|needle| body.contains(needle.as_str())))
            .count();
        assert_eq!(leaks, 0, "responses holding a fixture card number");
        let scheduling = self
            .bodies
            .iter()
            .filter(|body| body.contains(PSB_TEXT))
            .count();
        assert_eq!(scheduling, 0, "responses holding the PSB scheduling text");
        Ok(())
    }
}

/// The string at `pointer` of `value`.
fn text<'a>(value: &'a Value, pointer: &str) -> TestResult<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no string at {pointer}").into())
}

/// Asserts the parts of `message` the audit leaves assertable: its length and the fixed text at
/// each 0-based offset. The positions between them hold codes CICS, IMS or Db2 define (halts H3
/// and H4), which are not asserted.
fn assert_fixed_pieces(message: &str, len: usize, pieces: &[(usize, &str)]) {
    assert_eq!(message.len(), len, "message length: {message:?}");
    for &(at, piece) in pieces {
        let end = at.saturating_add(piece.len());
        assert_eq!(message.get(at..end), Some(piece), "{message:?} at {at}");
    }
}

/// The configuration of the API under test: the local secret and a job-deck directory of the
/// test target's own, which these endpoints never write.
fn config() -> Config {
    Config {
        jwt_secret: JWT_SECRET.to_owned(),
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints_authorizations"),
        ..Config::default()
    }
}

/// 2022-07-18 10:30:00.000, the instant of the `FixedClock`.
fn clock_now() -> TestResult<NaiveDateTime> {
    NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or_else(|| "2022-07-18 10:30:00.000 is a date and time".into())
}

/// The API over `store`, reading time from the `FixedClock`.
fn router_over(store: PgStore) -> TestResult<Router> {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(clock_now()?));
    Ok(app(AppState::from_config(store, &config(), clock)?))
}

/// A store over a second pool on `guard`'s database whose sessions take the server `options`,
/// URL query parameters of the form `options[<setting>]=<value>`.
async fn store_with_options(guard: &PostgresGuard, options: &str) -> TestResult<PgStore> {
    let database_url = format!("{}?{options}", guard.database_url());
    let config = Config {
        database_url,
        ..Config::default()
    };
    Ok(PgStore::new(runtime::db::connect(&config).await?))
}

/// A store over a second pool on `guard`'s database, closed before it is returned, so that the
/// first statement of any request fails with the store unavailable.
async fn closed_store(guard: &PostgresGuard) -> TestResult<PgStore> {
    let config = Config {
        database_url: guard.database_url().to_owned(),
        ..Config::default()
    };
    let pool = runtime::db::connect(&config).await?;
    pool.close().await;
    Ok(PgStore::new(pool))
}

/// Endpoint 21 for the `acct` path segment, with `query` (empty or starting with `?`).
fn summary_uri(acct: &str, query: &str) -> String {
    format!("/api/v1/accounts/{acct}/authorizations{query}")
}

/// Endpoint 22 for `key` of account 50, with `query`.
fn detail_uri(acct: &str, key: &str, query: &str) -> String {
    format!("/api/v1/authorizations/{acct}/{key}{query}")
}

/// Endpoint 23 for `key` of `acct`.
fn fraud_uri(acct: &str, key: &str) -> String {
    format!("/api/v1/authorizations/{acct}/{key}/fraud")
}

/// The fields every seeded detail shares, as CP00 stores them.
const CARD_EXPIRY: &str = "1225";
const MESSAGE_TYPE: &str = "AUTHRQ";
const MESSAGE_SOURCE: &str = "POSTRM";
const AUTH_ID_CODE: &str = "AB12CD";
const PROCESSING_CODE: u64 = 1_234;
const MCC: &str = "5411";
const COUNTRY: &str = "840";
const POS_ENTRY_MODE: u64 = 5;
const MERCHANT_ID: &str = "MERCH-0042-ABC";
const MERCHANT_NAME: &str = "CORNER GROCERY";
const MERCHANT_CITY: &str = "SPRINGFIELD";
const MERCHANT_STATE: &str = "IL";
const MERCHANT_ZIP: &str = "62701";

/// Endpoint 22's members that every seeded detail shows alike: the redacted card, the
/// processing code under `Auth Code:`, POS Entry Mode, Source, MCC Code and Card Exp. Date, the
/// blank fraud flag, and the merchant fields padded to the map (COPAUS1C-012, COPAUS1C-019,
/// COPAUS1C-020, COPAUS1C-022, COPAUS1C-023).
const COMMON_DETAIL: [(&str, &str); 12] = [
    ("card_num", CARD_50_REDACTED),
    ("auth_code", "001234"),
    ("pos_entry_mode", "05  "),
    ("source", "POSTRM    "),
    ("merchant_category_code", "5411"),
    ("card_expiry", "12/25"),
    ("fraud_status", "-         "),
    ("merchant_name", "CORNER GROCERY           "),
    ("merchant_id", "MERCH-0042-ABC "),
    ("merchant_city", "SPRINGFIELD              "),
    ("merchant_state", "IL"),
    ("merchant_zip", "62701     "),
];

/// Endpoint 22's Resp Reason for each detail of [`SPECS`]: the table entry, or `9999-ERROR`
/// for the reason the table lacks, padded to 20 (COPAUS1C-017, COPAUS1C-018).
const REASONS: [&str; 6] = [
    "0000-APPROVED       ",
    "4100-INSUFFICNT FUND",
    "9999-ERROR          ",
    "0000-APPROVED       ",
    "0000-APPROVED       ",
    "3100-INVALID CARD   ",
];

/// Endpoint 22's Tran Id for each detail of [`SPECS`]: a run of nine digits or more shows `*`
/// for each digit but its last four, a shorter run as stored (COPAUS1C-021, E32).
const DETAIL_TRAN_IDS: [&str; 6] = [
    "TXN********0001",
    "PTX220718A00002",
    "PTX220717A00003",
    "PTX241399A00004",
    "PTX220716A00005",
    "PTX220716A00006",
];

/// `cents` hundredths as a money value of `INT` integer digits.
fn money<const INT: u32>(cents: i64) -> TestResult<Money<INT>> {
    Ok(Money::new(Decimal::new(cents, 2))?)
}

/// An account key as the signed key of the pending-authorization tables.
fn auth_acct(acct: u64) -> TestResult<PicS<11>> {
    Ok(PicS::from(PicU::<11>::new(acct)?))
}

/// The card of account `acct`, read from its one cross-reference, so that no test writes a
/// fixture card number.
async fn card_of(store: &mut PgStore, acct: u64) -> TestResult<CardNumber> {
    let xrefs = CardXrefRepository::find_first_two_by_acct_id(store, PicU::new(acct)?).await?;
    match xrefs.as_slice() {
        [xref] => Ok(xref.card_num.clone()),
        _ => Err(format!("account {acct} has {} cross-references", xrefs.len()).into()),
    }
}

/// The detail `spec` describes, for `card`, with the fields every detail shares and a blank
/// fraud flag, as CP00 writes it.
fn detail_of(spec: &Spec, card: &CardNumber) -> TestResult<PendingAuthDetailRecord> {
    Ok(PendingAuthDetailRecord {
        auth_date_9c: PicS::new(spec.date_9c)?,
        auth_time_9c: PicS::new(spec.time_9c)?,
        orig_date: PicX::from_text(spec.orig_date)?,
        orig_time: PicX::from_text(spec.orig_time)?,
        card_num: card.clone(),
        auth_type: PicX::from_text(spec.auth_type)?,
        card_expiry_date: PicX::from_text(CARD_EXPIRY)?,
        message_type: PicX::from_text(MESSAGE_TYPE)?,
        message_source: PicX::from_text(MESSAGE_SOURCE)?,
        auth_id_code: PicX::from_text(AUTH_ID_CODE)?,
        auth_resp_code: PicX::from_text(spec.resp)?,
        auth_resp_reason: PicX::from_text(spec.reason)?,
        processing_code: PicU::new(PROCESSING_CODE)?,
        transaction_amt: money(spec.transaction_cents)?,
        approved_amt: money(spec.approved_cents)?,
        merchant_catagory_code: PicX::from_text(MCC)?,
        acqr_country_code: PicX::from_text(COUNTRY)?,
        pos_entry_mode: PicU::new(POS_ENTRY_MODE)?,
        merchant_id: PicX::from_text(MERCHANT_ID)?,
        merchant_name: PicX::from_text(MERCHANT_NAME)?,
        merchant_city: PicX::from_text(MERCHANT_CITY)?,
        merchant_state: PicX::from_text(MERCHANT_STATE)?,
        merchant_zip: PicX::from_text(MERCHANT_ZIP)?,
        transaction_id: PicX::from_text(spec.transaction_id)?,
        match_status: PicX::from_text(spec.match_status)?,
        auth_fraud: PicX::spaces(),
        fraud_rpt_date: PicX::spaces(),
        filler: PicX::spaces(),
    })
}

/// The `auth_fraud` row of account 50's detail `spec` under `auth_ts` with flag `flag`, built
/// column by column from COPAUS2C-006 rather than through the code under test.
fn fraud_row_of(
    spec: &Spec,
    card: &CardNumber,
    auth_ts: NaiveDateTime,
    flag: &str,
) -> TestResult<AuthFraudRow> {
    Ok(AuthFraudRow {
        card_num: card.clone(),
        auth_ts,
        auth_type: PicX::from_text(spec.auth_type)?,
        card_expiry_date: PicX::from_text(CARD_EXPIRY)?,
        message_type: PicX::from_text(MESSAGE_TYPE)?,
        message_source: PicX::from_text(MESSAGE_SOURCE)?,
        auth_id_code: PicX::from_text(AUTH_ID_CODE)?,
        auth_resp_code: PicX::from_text(spec.resp)?,
        auth_resp_reason: PicX::from_text(spec.reason)?,
        processing_code: PicX::from_text("001234")?,
        transaction_amt: money(spec.transaction_cents)?,
        approved_amt: money(spec.approved_cents)?,
        merchant_catagory_code: PicX::from_text(MCC)?,
        acqr_country_code: PicX::from_text(COUNTRY)?,
        pos_entry_mode: PicU::new(POS_ENTRY_MODE)?,
        merchant_id: PicX::from_text(MERCHANT_ID)?,
        merchant_name: PicX::from_text(MERCHANT_NAME)?,
        merchant_city: PicX::from_text(MERCHANT_CITY)?,
        merchant_state: PicX::from_text(MERCHANT_STATE)?,
        merchant_zip: PicX::from_text(MERCHANT_ZIP)?,
        transaction_id: PicX::from_text(spec.transaction_id)?,
        match_status: PicX::from_text(spec.match_status)?,
        auth_fraud: PicX::from_text(flag)?,
        acct_id: PicU::new(ACCT_50)?,
        cust_id: PicU::new(ACCT_50)?,
    })
}

/// The summary of account `acct` and customer `cust`, its figures chosen so that each edited
/// picture drops or pads digits: credit balance 123456789.01, cash balance 123456.78, counts 7
/// and 1234, amounts 1234.56 and −5.25. Its own limits, which the screen does not show, differ
/// from the account's.
async fn seed_summary(store: &mut PgStore, acct: u64, cust: u64) -> TestResult {
    let mut summary = PendingAuthSummaryRecord::initialized(auth_acct(acct)?, PicU::new(cust)?);
    summary.credit_limit = money(999_900)?;
    summary.cash_limit = money(888_800)?;
    summary.credit_balance = money(12_345_678_901)?;
    summary.cash_balance = money(12_345_678)?;
    summary.approved_auth_cnt = Count::new(7);
    summary.declined_auth_cnt = Count::new(1_234);
    summary.approved_auth_amt = money(123_456)?;
    summary.declined_auth_amt = money(-525)?;
    PendingAuthSummaryRepository::insert(store, &summary).await?;
    Ok(())
}

/// Account 50's summary and the details `specs` describe, all for `card`.
async fn seed_account_50(store: &mut PgStore, card: &CardNumber, specs: &[Spec]) -> TestResult {
    seed_summary(store, ACCT_50, ACCT_50).await?;
    for spec in specs {
        PendingAuthDetailRepository::insert(store, auth_acct(ACCT_50)?, &detail_of(spec, card)?)
            .await?;
    }
    Ok(())
}

/// Account 50's stored detail `spec`, read back through its key.
async fn stored_detail(
    store: &mut PgStore,
    spec: &Spec,
) -> TestResult<Option<PendingAuthDetailRecord>> {
    Ok(PendingAuthDetailRepository::find_by_id(
        store,
        auth_acct(ACCT_50)?,
        PicS::new(spec.date_9c)?,
        PicS::new(spec.time_9c)?,
    )
    .await?)
}

/// Prints the `BLOCKED` report of a halted case, for which nothing is asserted.
fn report_blocked(case: &str) {
    println!("BLOCKED {case}");
}

/// Account 50's customer block, from the cross-reference's customer and the account's limits
/// (COPAUS0C-020 to COPAUS0C-025).
const CUSTOMER_50: [(&str, &str); 7] = [
    ("customer_id", "000000050"),
    ("customer_name", "Aniya A Von              "),
    ("address_line_1", "1588 Nienow Cape,Suite 18"),
    ("address_line_2", "New Aricchester,OR,04257 "),
    ("phone", "(325)301-0827"),
    ("credit_limit", "     6169.00"),
    ("cash_limit", "  4587.00"),
];

/// The summary block of [`seed_summary`]'s figures (COPAUS0C-028).
const SUMMARY_FIGURES: [(&str, &str); 6] = [
    ("approved_count", "007"),
    ("declined_count", "234"),
    ("credit_balance", " 23456789.01"),
    ("cash_balance", " 23456.78"),
    ("approved_amount", "  1234.56 "),
    ("declined_amount", "-    5.25 "),
];

/// The summary block of an account with no summary: `ZERO` in every alphanumeric field
/// (COPAUS0C-029).
const NO_SUMMARY_FIGURES: [(&str, &str); 6] = [
    ("approved_count", "000"),
    ("declined_count", "000"),
    ("credit_balance", "000000000000"),
    ("cash_balance", "000000000"),
    ("approved_amount", "0000000000"),
    ("declined_amount", "0000000000"),
];

/// Asserts each `(member, value)` of `expected` under `prefix` of `reply`.
fn assert_members(reply: &Reply, prefix: &str, expected: &[(&str, &str)]) -> TestResult {
    for (member, value) in expected {
        assert_eq!(
            reply.text(&format!("{prefix}/{member}"))?,
            *value,
            "{member}"
        );
    }
    Ok(())
}

/// Asserts that `reply` lists exactly the rows `expected`, in order.
fn assert_rows(reply: &Reply, expected: &[[&str; 8]]) -> TestResult {
    let rows = reply
        .body
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("no rows array")?;
    assert_eq!(rows.len(), expected.len(), "row count");
    for (row, want) in rows.iter().zip(expected) {
        for (member, value) in ROW_MEMBERS.iter().zip(want) {
            assert_eq!(
                text(row, &format!("/{member}"))?,
                *value,
                "row member {member}"
            );
        }
    }
    Ok(())
}

/// The next-page flag endpoint 21 reported: `Some` after a full page, `None` when it reported
/// none.
fn next_page_flag(reply: &Reply) -> Option<bool> {
    reply.body.get("has_next_page").and_then(Value::as_bool)
}

/// Endpoints 21 to 23 for USER0001 over account 50's six details, account 27's summary with no
/// detail, and account 2 with no summary.
#[tokio::test]
async fn copaus0c_summary_rows() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let card = card_of(&mut store, ACCT_50).await?;
    seed_account_50(&mut store, &card, &SPECS).await?;
    seed_summary(&mut store, 27, 27).await?;
    let mut api = Api::signed_on(router_over(store)?).await?;
    summary_edits(&mut api).await?;
    summary_first_page(&mut api).await?;
    summary_paging(&mut api).await?;
    summary_other_accounts(&mut api).await?;
    // audit: COPAUS0C-051
    // audit: COPAUS0C-055
    api.assert_redacted()
}

/// The account edits of `PROCESS-ENTER-KEY`, and the unknown account.
async fn summary_edits(api: &mut Api) -> TestResult {
    let blank = api.get(&summary_uri("%20%20%20%20%20", "")).await?;
    let entered = "Please enter Acct Id...";
    // audit: COPAUS0C-005
    assert_eq!(
        blank.error(),
        (StatusCode::BAD_REQUEST, entered, Some("acct_id"))
    );
    let numeric = "Acct Id must be Numeric ...";
    for acct in [
        "0000000005",
        "0000000005X",
        "00000%2000050",
        "%2000000000050",
    ] {
        let reply = api.get(&summary_uri(acct, "")).await?;
        // audit: COPAUS0C-006
        assert_eq!(
            reply.error(),
            (StatusCode::BAD_REQUEST, numeric, Some("acct_id")),
            "{acct}"
        );
    }
    // The map field holds 11 characters; the server refuses a longer value with the same edit.
    let long = api.get(&summary_uri("000000000050", "")).await?;
    // audit: COPAU00-013
    assert_eq!(
        long.error(),
        (StatusCode::BAD_REQUEST, numeric, Some("acct_id"))
    );
    let unknown = api.get(&summary_uri("99999999999", "")).await?;
    // audit: COPAUS0C-014
    assert_eq!(
        (unknown.status, unknown.error().2),
        (StatusCode::NOT_FOUND, Some("acct_id"))
    );
    let pieces = [
        (0, "Account:"),
        (8, "99999999999"),
        (19, " not found in XREF file. Resp:"),
        (58, " Reas:"),
    ];
    // audit: COPAUS0C-014
    assert_fixed_pieces(unknown.message(), 73, &pieces);
    // audit: COPAUS0C-056
    report_blocked("H4: COPAUS0C-056, the RESP and RESP2 positions of the XREF not-found text");
    Ok(())
}

/// ENTER for account 50: the customer and summary block and the first five details.
async fn summary_first_page(api: &mut Api) -> TestResult {
    let page = api.get(&summary_uri(ACCT_50_TEXT, "")).await?;
    // audit: COPAUS0C-008
    page.expect_ok()?;
    // audit: COPAUS0C-052
    assert_eq!(page.body.get("message"), None);
    assert_eq!(page.text("/acct_id")?, ACCT_50_TEXT);
    // audit: COPAUS0C-013
    assert_eq!(page.text("/summary/customer_id")?, "000000050");
    // audit: COPAUS0C-020
    // audit: COPAUS0C-021
    // audit: COPAUS0C-022
    // audit: COPAUS0C-023
    // audit: COPAUS0C-024
    // audit: COPAUS0C-025
    assert_members(&page, "/summary", &CUSTOMER_50)?;
    // audit: COPAUS0C-027
    assert_eq!(
        page.body.pointer("/summary/summary_found"),
        Some(&json!(true))
    );
    // audit: COPAUS0C-028
    assert_members(&page, "/summary", &SUMMARY_FIGURES)?;
    // audit: COPAUS0C-031
    // audit: COPAUS0C-032
    // audit: COPAUS0C-033
    // audit: COPAUS0C-034
    assert_rows(&page, &ROWS[..5])?;
    assert_eq!(page.body.get("first_key"), Some(&json!(KEYS[0])));
    assert_eq!(page.body.get("last_key"), Some(&json!(KEYS[4])));
    // audit: COPAUS0C-036
    assert_eq!(next_page_flag(&page), Some(true));
    let shown = page.body.to_string();
    // audit: COPAUS0C-055
    assert!(
        !shown.contains("****") && !shown.contains("card"),
        "a card number on CPVS"
    );
    Ok(())
}

/// F8 and F7 from the keys a page returned, a key no detail holds, and a page of six rows.
async fn summary_paging(api: &mut Api) -> TestResult {
    let first = api.get(&summary_uri(ACCT_50_TEXT, "")).await?;
    let last_key = first.text("/last_key")?.to_owned();
    let next = format!("?page_key={last_key}&direction=forward");
    let next = api.get(&summary_uri(ACCT_50_TEXT, &next)).await?;
    // audit: COPAUS0C-041
    next.expect_ok()?;
    // audit: COPAUS0C-052
    assert_eq!(next.body.get("message"), None);
    // audit: COPAUS0C-041
    assert_rows(&next, &ROWS[5..])?;
    // audit: COPAUS0C-036
    assert_eq!(next_page_flag(&next), None);
    let past = format!("?page_key={KEY_ABSENT}&direction=forward");
    let past = api.get(&summary_uri(ACCT_50_TEXT, &past)).await?;
    // audit: COPAUS0C-041
    assert_rows(&past, &[])?;
    for (key, rows, flag) in [(KEYS[0], &ROWS[..5], true), (KEYS[1], &ROWS[1..], false)] {
        let back = format!("?page_key={key}&direction=backward");
        let back = api.get(&summary_uri(ACCT_50_TEXT, &back)).await?;
        // audit: COPAUS0C-047
        back.expect_ok()?;
        // audit: COPAUS0C-047
        assert_rows(&back, rows)?;
        // audit: COPAUS0C-036
        assert_eq!(next_page_flag(&back), Some(flag), "{key}");
    }
    let gone = format!("?page_key={KEY_ABSENT}&direction=backward");
    let gone = api.get(&summary_uri(ACCT_50_TEXT, &gone)).await?;
    // audit: COPAUS0C-047
    assert_rows(&gone, &[])?;
    let six = api.get(&summary_uri(ACCT_50_TEXT, "?page_size=6")).await?;
    // audit: COPAUS0C-031
    assert_rows(&six, &ROWS)?;
    // audit: COPAUS0C-036
    assert_eq!(next_page_flag(&six), Some(false));
    Ok(())
}

/// Account 27, whose summary has no detail, and account 2, which has no summary.
async fn summary_other_accounts(api: &mut Api) -> TestResult {
    let empty = api.get(&summary_uri(ACCT_27_TEXT, "")).await?;
    empty.expect_ok()?;
    assert_eq!(
        empty.body.pointer("/summary/summary_found"),
        Some(&json!(true))
    );
    // audit: COPAUS0C-031
    assert_rows(&empty, &[])?;
    assert_eq!(empty.body.get("message"), None);
    let missing = api.get(&summary_uri(ACCT_2_TEXT, "")).await?;
    // audit: COPAUS0C-029
    missing.expect_ok()?;
    // audit: COPAUS0C-029
    assert_eq!(missing.body.get("message"), None);
    // audit: COPAUS0C-027
    // audit: COPAUS0C-029
    assert_eq!(
        missing.body.pointer("/summary/summary_found"),
        Some(&json!(false))
    );
    // audit: COPAUS0C-029
    assert_members(&missing, "/summary", &NO_SUMMARY_FIGURES)?;
    // audit: COPAUS0C-025
    assert_members(
        &missing,
        "/summary",
        &[
            ("credit_limit", "     6130.00"),
            ("cash_limit", "  5448.00"),
        ],
    )?;
    // audit: COPAUS0C-029
    assert_rows(&missing, &[])?;
    Ok(())
}

/// The account a cross-reference names that has no `accounts` row, and the copy of account 50
/// whose cross-reference names a customer with no `customers` row.
const ACCT_NO_ACCOUNT: u64 = 12_345_678_901;
const ACCT_NO_CUSTOMER: u64 = 12_345_678_902;
const CUST_MISSING: u64 = 999_999_999;

/// A cross-reference of invented card `card`, which no fixture holds, for `acct` and `cust`.
async fn seed_xref(store: &mut PgStore, card: &str, acct: u64, cust: u64) -> TestResult {
    let xref = CardXrefRecord {
        card_num: CardNumber::parse(card)?,
        cust_id: PicU::new(cust)?,
        acct_id: PicU::new(acct)?,
        filler: PicX::spaces(),
    };
    CardXrefRepository::insert(store, &xref).await?;
    Ok(())
}

/// The masters the failed reads need: a cross-reference to a missing account, an account whose
/// cross-reference names a missing customer, and a second cross-reference of account 50, which
/// no fixture account has (CICS `DUPKEY`).
async fn seed_failed_reads(store: &mut PgStore) -> TestResult {
    seed_xref(store, "9200000000000016", ACCT_NO_ACCOUNT, ACCT_50).await?;
    let mut account: AccountRecord = AccountRepository::find_by_id(store, PicU::new(ACCT_50)?)
        .await?
        .ok_or("account 50 is loaded")?;
    account.id = PicU::new(ACCT_NO_CUSTOMER)?;
    AccountRepository::insert(store, &account).await?;
    seed_xref(store, "9200000000000018", ACCT_NO_CUSTOMER, CUST_MISSING).await?;
    seed_xref(store, "9200000000000015", ACCT_50, ACCT_50).await
}

/// Endpoint 21 when the cross-reference, account or customer read finds nothing or fails.
#[tokio::test]
async fn copaus0c_failed_reads() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    seed_failed_reads(&mut store).await?;
    let mut api = Api::signed_on(router_over(store)?).await?;
    missing_masters(&mut api).await?;
    xref_system_errors(&mut api, &guard).await?;
    // audit: COPAUS0C-051
    api.assert_redacted()
}

/// The account and the customer the cross-reference names are not stored.
async fn missing_masters(api: &mut Api) -> TestResult {
    let no_account = api.get(&summary_uri("12345678901", "")).await?;
    // audit: COPAUS0C-016
    assert_eq!(no_account.status, StatusCode::NOT_FOUND);
    let pieces = [
        (0, "Account:"),
        (8, "12345678901"),
        (19, " not found in ACCT file. Resp:"),
        (58, " Reas:"),
    ];
    // audit: COPAUS0C-016
    assert_fixed_pieces(no_account.message(), 73, &pieces);
    // audit: COPAUS0C-016
    assert_eq!(no_account.body.get("summary"), None);
    let no_customer = api.get(&summary_uri("12345678902", "")).await?;
    // audit: COPAUS0C-018
    assert_eq!(no_customer.status, StatusCode::NOT_FOUND);
    let pieces = [
        (0, "Customer:"),
        (9, "999999999"),
        (18, " not found in CUST file. Resp:"),
        (57, " Reas:"),
    ];
    // audit: COPAUS0C-018
    assert_fixed_pieces(no_customer.message(), 72, &pieces);
    // audit: COPAUS0C-018
    assert_eq!(no_customer.body.get("summary"), None);
    // audit: COPAUS0C-056
    report_blocked("H4: COPAUS0C-056, the RESP and RESP2 positions of the ACCT and CUST texts");
    Ok(())
}

/// The cross-reference read ends neither found nor not found: two rows for the account, and a
/// store that cannot be reached.
async fn xref_system_errors(api: &mut Api, guard: &PostgresGuard) -> TestResult {
    let mut closed = Api::with_token(router_over(closed_store(guard).await?)?, &api.token);
    let unreachable = closed.get(&summary_uri(ACCT_50_TEXT, "")).await?;
    let twice = api.get(&summary_uri(ACCT_50_TEXT, "")).await?;
    let pieces = [
        (0, "Account:"),
        (8, ACCT_50_TEXT),
        (19, " System error while reading XREF file. Resp:"),
        (72, " Reas:"),
    ];
    for reply in [&unreachable, &twice] {
        // audit: COPAUS0C-015
        assert_eq!(reply.error().0, StatusCode::INTERNAL_SERVER_ERROR);
        // audit: COPAUS0C-015
        assert_fixed_pieces(reply.message(), 78, &pieces);
        // audit: COPAUS0C-015
        assert_eq!(reply.body.get("rows"), None);
    }
    // audit: COPAUS0C-056
    report_blocked("H4: COPAUS0C-056, the RESP position of the XREF system-error text");
    // audit: COPAUS0C-051
    closed.assert_redacted()
}

/// Whether `reply` carries no message: the member is absent or null.
fn no_message(reply: &Reply) -> bool {
    reply.body.get("message").is_none_or(Value::is_null)
}

/// Asserts that `reply` shows account 50's detail `SPECS[index]` as `POPULATE-AUTH-DETAILS`
/// fills `COPAU1A`.
fn assert_detail(reply: &Reply, prefix: &str, index: usize) -> TestResult {
    let row = ROWS.get(index).ok_or("no such detail")?;
    assert_eq!(reply.text(&format!("{prefix}/acct_id"))?, ACCT_50_TEXT);
    assert_eq!(reply.text(&format!("{prefix}/auth_key"))?, row[0]);
    // audit: COPAUS1C-012
    // audit: COPAUS1C-019
    // audit: COPAUS1C-020
    // audit: COPAUS1C-022
    // audit: COPAUS1C-023
    assert_members(reply, prefix, &COMMON_DETAIL)?;
    let auth_date = format!("{}  ", row[2]);
    let auth_time = format!("{}  ", row[3]);
    let auth_type = format!("{}          ", row[4]);
    let reason = REASONS.get(index).ok_or("no reason")?;
    let tran_id = DETAIL_TRAN_IDS.get(index).ok_or("no transaction ID")?;
    // audit: COPAUS1C-013
    assert_members(reply, prefix, &[("auth_date", auth_date.as_str())])?;
    // audit: COPAUS1C-014
    assert_members(reply, prefix, &[("auth_time", auth_time.as_str())])?;
    // audit: COPAUS1C-015
    assert_members(reply, prefix, &[("amount", row[7])])?;
    // audit: COPAUS1C-016
    assert_members(reply, prefix, &[("response", row[5])])?;
    // audit: COPAUS1C-017
    // audit: COPAUS1C-018
    assert_members(reply, prefix, &[("reason", *reason)])?;
    // audit: COPAUS1C-021
    assert_members(
        reply,
        prefix,
        &[
            ("auth_type", auth_type.as_str()),
            ("transaction_id", *tran_id),
            ("match_status", row[6]),
        ],
    )
}

/// Endpoints 22 and 23 for USER0001 over account 50's six details.
#[tokio::test]
async fn copaus1c_detail_and_fraud_rows() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let card = card_of(&mut store, ACCT_50).await?;
    seed_account_50(&mut store, &card, &SPECS).await?;
    let mut api = Api::signed_on(router_over(PgStore::new(guard.pool().clone()))?).await?;
    detail_enter(&mut api).await?;
    detail_next(&mut api).await?;
    detail_refusals(&mut api).await?;
    invalid_date_toggle(&mut api, &mut store).await?;
    detail_store_unavailable(&mut api, &guard).await?;
    committed_toggles(&mut api, &mut store, &card).await?;
    // audit: COPAUS1C-007
    // audit: COPAUS2C-013
    api.assert_redacted()
}

/// ENTER on each detail: every `COPAU01` field, and no message.
async fn detail_enter(api: &mut Api) -> TestResult {
    for (index, key) in KEYS.iter().enumerate() {
        let reply = api.get(&detail_uri(ACCT_50_TEXT, key, "")).await?;
        // audit: COPAUS1C-008
        reply.expect_ok()?;
        // audit: COPAUS1C-035
        assert!(no_message(&reply), "{key}");
        assert_detail(&reply, "", index)?;
    }
    Ok(())
}

/// F8 from a detail to the next, and on the last detail.
async fn detail_next(api: &mut Api) -> TestResult {
    for (from, to) in [(0, 1), (2, 3), (4, 5)] {
        let key = KEYS.get(from).ok_or("no key")?;
        let reply = api
            .get(&detail_uri(ACCT_50_TEXT, key, "?direction=forward"))
            .await?;
        // audit: COPAUS1C-024
        reply.expect_ok()?;
        // audit: COPAUS1C-035
        assert!(no_message(&reply), "{key}");
        // audit: COPAUS1C-024
        assert_detail(&reply, "", to)?;
    }
    let last = api
        .get(&detail_uri(ACCT_50_TEXT, KEYS[5], "?direction=forward"))
        .await?;
    // audit: COPAUS1C-025
    last.expect_ok()?;
    // audit: COPAUS1C-025
    assert_eq!(last.message(), "Already at the last Authorization...");
    // audit: COPAUS1C-025
    assert_eq!(last.text("/auth_key")?, KEYS[5]);
    Ok(())
}

/// The edits of endpoints 22 and 23, and keys no detail holds.
async fn detail_refusals(api: &mut Api) -> TestResult {
    let edited = [
        detail_uri("0000000005", KEYS[0], ""),
        detail_uri("0000000005X", KEYS[0], ""),
        detail_uri(ACCT_50_TEXT, "%20%20%20", ""),
    ];
    for uri in &edited {
        let reply = api.get(uri).await?;
        // audit: COPAUS1C-006
        assert_eq!(reply.error().0, StatusCode::BAD_REQUEST, "{uri}");
        // audit: COPAUS1C-006
        assert_eq!(
            (reply.message(), reply.body.get("card_num")),
            ("", None),
            "{uri}"
        );
    }
    for uri in [
        fraud_uri("0000000005X", KEYS[0]),
        fraud_uri(ACCT_50_TEXT, "%20"),
    ] {
        let reply = api.post(&uri).await?;
        // audit: COPAUS1C-006
        assert_eq!(
            (reply.status, reply.message()),
            (StatusCode::BAD_REQUEST, ""),
            "{uri}"
        );
    }
    let unknown = [
        detail_uri(ACCT_50_TEXT, KEY_ABSENT, ""),
        detail_uri(ACCT_50_TEXT, "ABC", ""),
        detail_uri(ACCT_2_TEXT, KEYS[0], ""),
    ];
    for uri in &unknown {
        let reply = api.get(uri).await?;
        // audit: COPAUS1C-009
        assert_eq!(
            (reply.status, reply.message()),
            (StatusCode::NOT_FOUND, ""),
            "{uri}"
        );
        // audit: COPAUS1C-009
        assert_eq!(reply.body.get("card_num"), None, "{uri}");
    }
    let lost = api
        .get(&detail_uri(ACCT_50_TEXT, KEY_ABSENT, "?direction=forward"))
        .await?;
    // audit: COPAUS1C-027
    report_blocked("X-COPAUS1C-H1, X-COPAUS1C-H2: COPAUS1C-027, F8 from a key no detail holds");
    let missed = api.post(&fraud_uri(ACCT_50_TEXT, KEY_ABSENT)).await?;
    // audit: COPAUS1C-029
    report_blocked("X-COPAUS1C-H1, X-COPAUS1C-H2: COPAUS1C-029, F5 on a key no detail holds");
    println!(
        "F8 and F5 on an absent key answered {} and {}",
        lost.status, missed.status
    );
    Ok(())
}

/// F5 on the detail whose request date names no calendar date: COPAUS2C's insert-failure text,
/// a rollback, and nothing written.
async fn invalid_date_toggle(api: &mut Api, store: &mut PgStore) -> TestResult {
    let before = stored_detail(store, &SPECS[3]).await?;
    let reply = api.post(&fraud_uri(ACCT_50_TEXT, KEYS[3])).await?;
    // audit: COPAUS2C-005
    // audit: COPAUS2C-010
    // audit: COPAUS1C-031
    assert_eq!(reply.error().0, StatusCode::INTERNAL_SERVER_ERROR);
    // audit: COPAUS2C-005
    // audit: COPAUS2C-010
    // audit: COPAUS1C-031
    assert_fixed_pieces(reply.message(), 50, &[(0, SYSTEM_ERROR), (31, STATE_LABEL)]);
    report_blocked("H3: COPAUS2C-005 and COPAUS2C-010, the SQLCODE and SQLSTATE positions");
    // audit: COPAUS1C-031
    // audit: COPAUS2C-012
    assert_eq!(stored_detail(store, &SPECS[3]).await?, before);
    assert!(before.is_some(), "the detail is seeded");
    Ok(())
}

/// Endpoint 22 over a store that cannot be reached: the summary read fails.
async fn detail_store_unavailable(api: &mut Api, guard: &PostgresGuard) -> TestResult {
    let mut closed = Api::with_token(router_over(closed_store(guard).await?)?, &api.token);
    let reply = closed.get(&detail_uri(ACCT_50_TEXT, KEYS[0], "")).await?;
    // audit: COPAUS1C-010
    assert_eq!(reply.error().0, StatusCode::INTERNAL_SERVER_ERROR);
    let summary_error = " System error while reading Auth Summary: Code:";
    // audit: COPAUS1C-010
    assert_fixed_pieces(reply.message(), 49, &[(0, summary_error)]);
    // audit: COPAUS1C-010
    assert_eq!(reply.body.get("card_num"), None);
    // audit: COPAUS1C-038
    report_blocked("H4: COPAUS1C-038, the IMS status position of the summary-read text");
    // audit: COPAUS1C-007
    closed.assert_redacted()
}

/// `hour:minute:second.milli` on `year-month-day`.
fn at(year: i32, month: u32, day: u32, time: (u32, u32, u32, u32)) -> TestResult<NaiveDateTime> {
    let (hour, minute, second, milli) = time;
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_milli_opt(hour, minute, second, milli))
        .ok_or_else(|| "a valid date and time".into())
}

/// F5 twice on the first detail, the branch whose committed writes COPAUS1C-032 halts. It runs
/// to check the replacement's intended behaviour, a mark then a removal written to the detail
/// and to one `auth_fraud` row, and its checks carry no tag: they are no parity result.
async fn committed_toggles(api: &mut Api, store: &mut PgStore, card: &CardNumber) -> TestResult {
    // The request date of the detail and CP00's processing time, 10:15:30.250.
    let auth_ts = at(2022, 7, 18, (10, 15, 30, 250))?;
    let report_date = clock_now()?.date();
    for (flag, message) in [
        ("F", "AUTH MARKED FRAUD..."),
        ("R", "AUTH FRAUD REMOVED..."),
    ] {
        let reply = api.post(&fraud_uri(ACCT_50_TEXT, KEYS[0])).await?;
        reply.expect_ok()?;
        assert_eq!(reply.message(), message);
        assert_eq!(reply.text("/detail/card_num")?, CARD_50_REDACTED);
        assert_eq!(
            reply.text("/detail/fraud_status")?,
            format!("{flag}-07/18/22")
        );
        let detail = stored_detail(store, &SPECS[0])
            .await?
            .ok_or("the detail is stored")?;
        let shown = (detail.auth_fraud.to_text(), detail.fraud_rpt_date.to_text());
        assert_eq!(shown, (flag.to_owned(), "07/18/22".to_owned()), "{message}");
        let stored = AuthFraudRepository::find_by_id(store, card, auth_ts).await?;
        let expected = fraud_row_of(&SPECS[0], card, auth_ts, flag)?;
        assert_eq!(stored, Some((expected, Some(report_date))), "{message}");
    }
    // audit: COPAUS1C-032
    report_blocked("X-COPAUS1C-H2: COPAUS1C-032, the detail and fraud row committed by F5");
    // audit: COPAUS2C-001
    report_blocked("X-COPAUS1C-H2, X-COPAUS2C-H3: COPAUS2C-001, the report date written by F5");
    Ok(())
}

/// The processing time of [`LOCKED_SPEC`] on its request date: its `AUTH_TS` as COPAUS2C
/// rebuilds it, the request's `YYMMDD` with `999999999 − PA-AUTH-TIME-9C`.
fn locked_auth_ts() -> TestResult<NaiveDateTime> {
    at(2022, 7, 15, (13, 45, 1, 789))
}

/// The report date of the `auth_fraud` row seeded before the lock tests.
fn seeded_report_date() -> TestResult<NaiveDate> {
    NaiveDate::from_ymd_opt(2022, 7, 1).ok_or_else(|| "2022-07-01 is a date".into())
}

/// Endpoint 23 failing after its first statements, against a held row lock: the second pool's
/// sessions give up a lock wait after 500 milliseconds.
#[tokio::test]
async fn copaus2c_rollback_paths() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let card = card_of(&mut store, ACCT_50).await?;
    seed_account_50(&mut store, &card, std::slice::from_ref(&LOCKED_SPEC)).await?;
    let waiting = store_with_options(&guard, "options[lock_timeout]=500").await?;
    let mut api = Api::signed_on(router_over(waiting)?).await?;
    replace_fails_after_insert(&mut api, &mut store, &card).await?;
    let seeded = fraud_row_of(&LOCKED_SPEC, &card, locked_auth_ts()?, "R")?;
    AuthFraudRepository::insert(&mut store, &seeded, seeded_report_date()?).await?;
    insert_fails_on_held_key(&mut api, &mut store, &card).await?;
    replace_fails_after_update(&mut api, &mut store, &card).await?;
    // audit: COPAUS2C-011
    report_blocked("H3: COPAUS2C-011, the update failure: no real-store path reaches it");
    // audit: COPAUS2C-013
    api.assert_redacted()
}

/// Runs F5 on [`LOCKED_KEY`] while a transaction of the test holds the detail's row lock, and
/// releases it after the response.
async fn toggle_beside_detail_lock(
    api: &mut Api,
    store: &PgStore,
    card: &CardNumber,
) -> TestResult<Reply> {
    let mut holder = store.begin().await?;
    let held = detail_of(&LOCKED_SPEC, card)?;
    let locked =
        PendingAuthDetailRepository::update(&mut holder, auth_acct(ACCT_50)?, &held).await?;
    assert!(locked, "the test holds the detail's row lock");
    let reply = api.post(&fraud_uri(ACCT_50_TEXT, LOCKED_KEY)).await;
    holder.rollback().await?;
    reply
}

/// Asserts COPAUS1C's replace-failure text, the fixed part of its 47 characters.
fn assert_tagging_error(reply: &Reply) {
    // audit: COPAUS1C-033
    assert_eq!(reply.error().0, StatusCode::INTERNAL_SERVER_ERROR);
    // audit: COPAUS1C-033
    assert_fixed_pieces(reply.message(), 47, &[(0, TAGGING_ERROR)]);
    // audit: COPAUS1C-038
    report_blocked("H4: COPAUS1C-038, the IMS status position of the FRAUD Tagging text");
}

/// The insert succeeds and the replace of the detail cannot take its lock: both roll back.
async fn replace_fails_after_insert(
    api: &mut Api,
    store: &mut PgStore,
    card: &CardNumber,
) -> TestResult {
    let reply = toggle_beside_detail_lock(api, store, card).await?;
    // The replace runs only after COPAUS2C's insert returned status `S`.
    // audit: COPAUS2C-007
    assert_tagging_error(&reply);
    let fraud = AuthFraudRepository::find_by_id(store, card, locked_auth_ts()?).await?;
    // audit: COPAUS2C-012
    // audit: COPAUS1C-033
    assert_eq!(fraud, None);
    // audit: COPAUS2C-012
    // audit: COPAUS1C-033
    assert_eq!(
        stored_detail(store, &LOCKED_SPEC).await?,
        Some(detail_of(&LOCKED_SPEC, card)?)
    );
    Ok(())
}

/// The insert meets the key of an `auth_fraud` row another transaction is updating, waits on
/// it and fails: COPAUS2C's insert-failure text proves that the rebuilt `AUTH_TS` and the card
/// are that row's key, as an insert of any other key would not wait.
async fn insert_fails_on_held_key(
    api: &mut Api,
    store: &mut PgStore,
    card: &CardNumber,
) -> TestResult {
    let auth_ts = locked_auth_ts()?;
    let mut holder = store.begin().await?;
    let flag = PicX::from_text("R")?;
    let held =
        AuthFraudRepository::update_flag(&mut holder, card, auth_ts, &flag, seeded_report_date()?)
            .await?;
    assert!(held, "the test holds the fraud row's lock");
    let reply = api.post(&fraud_uri(ACCT_50_TEXT, LOCKED_KEY)).await;
    holder.rollback().await?;
    let reply = reply?;
    // audit: COPAUS2C-002
    // audit: COPAUS2C-003
    // audit: COPAUS2C-004
    // audit: COPAUS2C-010
    // audit: COPAUS1C-031
    assert_eq!(reply.error().0, StatusCode::INTERNAL_SERVER_ERROR);
    // audit: COPAUS2C-010
    // audit: COPAUS1C-031
    assert_fixed_pieces(reply.message(), 50, &[(0, SYSTEM_ERROR), (31, STATE_LABEL)]);
    report_blocked("H3: COPAUS2C-010, the SQLCODE and SQLSTATE positions");
    let seeded = fraud_row_of(&LOCKED_SPEC, card, auth_ts, "R")?;
    let fraud = AuthFraudRepository::find_by_id(store, card, auth_ts).await?;
    // audit: COPAUS2C-012
    // audit: COPAUS1C-031
    assert_eq!(fraud, Some((seeded, Some(seeded_report_date()?))));
    // audit: COPAUS2C-012
    // audit: COPAUS1C-031
    assert_eq!(
        stored_detail(store, &LOCKED_SPEC).await?,
        Some(detail_of(&LOCKED_SPEC, card)?)
    );
    Ok(())
}

/// The insert meets the committed row's key, the update that follows succeeds, and the replace
/// of the detail cannot take its lock: the update rolls back with it.
async fn replace_fails_after_update(
    api: &mut Api,
    store: &mut PgStore,
    card: &CardNumber,
) -> TestResult {
    let reply = toggle_beside_detail_lock(api, store, card).await?;
    // The replace runs only after the duplicate key led to `FRAUD-UPDATE` and its status `S`.
    // audit: COPAUS2C-008
    // audit: COPAUS2C-009
    assert_tagging_error(&reply);
    let seeded = fraud_row_of(&LOCKED_SPEC, card, locked_auth_ts()?, "R")?;
    let fraud = AuthFraudRepository::find_by_id(store, card, locked_auth_ts()?).await?;
    // audit: COPAUS2C-012
    // audit: COPAUS1C-033
    assert_eq!(fraud, Some((seeded, Some(seeded_report_date()?))));
    // audit: COPAUS2C-012
    // audit: COPAUS1C-033
    assert_eq!(
        stored_detail(store, &LOCKED_SPEC).await?,
        Some(detail_of(&LOCKED_SPEC, card)?)
    );
    Ok(())
}

#[test]
#[ignore = "BLOCKED(H3): COPAUS2C-005, COPAUS2C-010, COPAUS2C-011 — SQLCODE/SQLERRM text defined by Db2"]
fn blocked_h3_db2_texts() {}

#[test]
#[ignore = "BLOCKED(H4): COPAUS0C-014 to COPAUS0C-019, COPAUS0C-030, COPAUS0C-037, COPAUS0C-038, COPAUS0C-051, COPAUS1C-007, COPAUS1C-010, COPAUS1C-011, COPAUS1C-026, COPAUS1C-033 — CICS RESP and IMS status codes inside texts"]
fn blocked_h4_codes() {}
