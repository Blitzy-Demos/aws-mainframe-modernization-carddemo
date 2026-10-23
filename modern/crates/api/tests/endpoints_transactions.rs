//! Endpoint-level g1b bindings for COTRN00C/01C/02C and their CSUTLDTC call sites.
//!
//! Each test owns a fixture-loaded `PostgreSQL` container. Transactions are decoded from
//! `dailytran.txt` and inserted through the real repository; synthetic cross-references exercise
//! missing accounts and duplicate alternate keys. Every response is scanned for full card
//! numbers, and time-dependent fields are compared by format.
//!
//! Halted audit rows remain BLOCKED: ignored tests fail explicitly when requested. Their tags
//! preserve the audit coverage index without presenting unresolved COBOL outcomes as equivalent.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app};
use api::dto::FIELD_LENGTH_MESSAGE;
use axum::body::{self, Body};
use axum::http::{Request, StatusCode, header};
use chrono::{NaiveDate, NaiveDateTime};
use domain::copybook::framing::decode_all;
use domain::copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};
use domain::money::Decimal;
use domain::online::cosgn00c::UserType;
use domain::records::{AccountRecord, CardXrefRecord, DailyTransactionRecord, TransactionRecord};
use domain::sensitive::CardNumber;
use domain::{Clock, FixedClock};
use persistence::{AccountRepository, CardXrefRepository, PgStore, TransactionRepository};
use runtime::config::Config;
use serde_json::{Value, json};
use test_support::fixtures::{fixture_card_numbers, fixture_path, start_loaded_postgres};
use test_support::postgres::{PostgresGuard, start_postgres};
use tower::ServiceExt as _;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// A local-only signing secret for the tokens of these tests.
const SECRET: &str = "endpoints-transactions-local-secret";

/// The password of every `DUSRSECJ` user (`README.md:203-204`).
const PASSWORD: &str = "PASSWORD";

/// Endpoints 11 and 13 share this path; endpoint 12 appends the ID.
const TRANSACTIONS: &str = "/api/v1/transactions";

/// The first fixture cross-reference (`cardxref.txt:1`): its card and its account.
const XREF_CARD: &str = "0500024453765740";
const XREF_ACCT: &str = "00000000050";
const XREF_CARD_REDACTED: &str = "************5740";

/// Synthetic card numbers in no fixture, each below 2^53 (X-ARITH-5); no response may carry one.
const SYNTHETIC_CARDS: [&str; 3] = ["4000001234567899", "4000001234567881", "4000001234567873"];

/// An account no fixture cross-reference names (COTRN02C-013).
const UNKNOWN_ACCT: &str = "99999999999";

/// A synthetic account whose one cross-reference `accounts` lacks (COTRN02C-011).
const ORPHAN_ACCT: &str = "99999999990";

/// A synthetic account two cross-references share, the `DUPKEY` of `CXACAIX` (COTRN02C-014).
const SHARED_ACCT: &str = "99999999980";

// PIC X(50) pads the 49-byte VALUE with one more space: ten trailing spaces in all.
const INVALID_KEY: &str = "Invalid key pressed. Please see below...          ";

/// The description of the first add, shorter than its 60-byte map field.
const DESC: &str = "Purchase at Abshire-Lowe";

/// The rows of a `COTRN0A` page: the screen's ten, as no request here sends `page_size`.
const PAGE_ROWS: usize = 10;

/// How many `dailytran.txt` records the list test seeds.
const LIST_RECORDS: usize = 25;

/// The API over one database: the state, the store the test seeds and inspects through, the
/// card numbers no response may carry, and the container, which dropping removes.
struct Harness {
    state: AppState,
    store: PgStore,
    needles: Vec<String>,
    _guard: PostgresGuard,
}

impl Harness {
    /// The API over the database `guard` holds, with a `FixedClock` at 2022-07-18 10:30:00.000.
    fn over(guard: PostgresGuard) -> TestResult<Self> {
        let store = PgStore::new(guard.pool().clone());
        let config = Config {
            jwt_secret: SECRET.to_owned(),
            intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints-transactions"),
            ..Config::default()
        };
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_milli_opt(10, 30, 0, 0))
            .ok_or("an invalid test instant")?;
        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(at));
        let state = AppState::from_config(store.clone(), &config, clock)?;
        let mut needles = fixture_card_numbers()?;
        needles.extend(SYNTHETIC_CARDS.map(str::to_owned));
        Ok(Self {
            state,
            store,
            needles,
            _guard: guard,
        })
    }

    /// The API over a fresh, migrated and fixture-loaded database.
    async fn loaded() -> TestResult<Self> {
        Self::over(start_loaded_postgres().await?)
    }

    /// The status and JSON body `request` gets. Every body is first searched for the card
    /// numbers no response may carry (Cardholder Data Redaction).
    async fn send(&self, request: Request<Body>) -> TestResult<(StatusCode, Value)> {
        let response = Box::pin(app(self.state.clone()).oneshot(request)).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        let text = String::from_utf8(bytes.to_vec())?;
        for needle in &self.needles {
            // The needle itself is never printed.
            assert!(
                !text.contains(needle.as_str()),
                "a response body carried a full card number"
            );
        }
        Ok((status, serde_json::from_str(&text)?))
    }

    /// The bearer token endpoint 1 issues to `user_id` for the password `PASSWORD`.
    async fn sign_on(&self, user_id: &str) -> TestResult<String> {
        let body = json!({ "user_id": user_id, "password": PASSWORD });
        let (status, signed) = self.send(post("/api/v1/signon", None, &body)?).await?;
        assert_eq!(status, StatusCode::OK, "sign-on of {user_id}");
        Ok(signed["token"].as_str().ok_or("no token")?.to_owned())
    }

    /// Endpoint 13 with `body`, for the bearer `token`.
    async fn add(&self, token: &str, body: &Value) -> TestResult<(StatusCode, Value)> {
        self.send(post(TRANSACTIONS, Some(token), body)?).await
    }

    /// Endpoint 12 for the path segment `tran_id`, already percent-encoded.
    async fn view(&self, token: &str, tran_id: &str) -> TestResult<(StatusCode, Value)> {
        self.send(get(&format!("{TRANSACTIONS}/{tran_id}"), token)?)
            .await
    }

    /// Endpoint 11 with the query pairs `query`.
    async fn list(&self, token: &str, query: &[(&str, String)]) -> TestResult<(StatusCode, Value)> {
        let pairs: Vec<String> = query
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let response = self
            .send(get(&format!("{TRANSACTIONS}?{}", pairs.join("&")), token)?)
            .await?;
        if response.1.get("rows").is_some() {
            assert_eq!(response.0, StatusCode::OK);
        }
        Ok(response)
    }

    /// The transaction with the highest ID, or `None` on an empty table.
    async fn last(&self) -> TestResult<Option<TransactionRecord>> {
        let mut store = self.store.clone();
        Ok(TransactionRepository::find_last(&mut store).await?)
    }

    /// The transaction `id` names.
    async fn stored(&self, id: &str) -> TestResult<TransactionRecord> {
        let mut store = self.store.clone();
        let key = PicX::from_text(id)?;
        Ok(TransactionRepository::find_by_id(&mut store, &key)
            .await?
            .ok_or("the transaction is not stored")?)
    }

    /// Inserts `records` in one transaction, as a load or `posttran` writes them.
    async fn insert(&self, records: &[TransactionRecord]) -> TestResult {
        let mut tx = self.store.begin().await?;
        for record in records {
            TransactionRepository::insert(&mut tx, record).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Inserts a cross-reference of `card` to `acct` with customer 999.
    async fn insert_xref(&self, card: &str, acct: &str) -> TestResult {
        let mut store = self.store.clone();
        let record = CardXrefRecord {
            card_num: CardNumber::new(PicX::from_text(card)?),
            cust_id: PicU::new(999)?,
            acct_id: PicU::new(acct.parse()?)?,
            filler: PicX::spaces(),
        };
        Ok(CardXrefRepository::insert(&mut store, &record).await?)
    }

    /// The account `acct` names, or `None`.
    async fn account(&self, acct: &str) -> TestResult<Option<AccountRecord>> {
        let mut store = self.store.clone();
        Ok(AccountRepository::find_by_id(&mut store, PicU::new(acct.parse()?)?).await?)
    }
}

fn post(uri: &str, token: Option<&str>, body: &Value) -> TestResult<Request<Body>> {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    Ok(request.body(Body::from(serde_json::to_vec(body)?))?)
}

fn get(uri: &str, token: &str) -> TestResult<Request<Body>> {
    Ok(Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())?)
}

fn entered() -> Value {
    json!({
        "aid": "ENTER", "acct_id": XREF_ACCT, "confirm": "",
        "fields": {
            "type_cd": "01", "cat_cd": "0005", "source": "POS TERM",
            "description": DESC, "amount": "+00000012.34",
            "orig_date": "2022-06-10", "proc_date": "2022-06-11",
            "merchant_id": "800000000", "merchant_name": "Abshire-Lowe",
            "merchant_city": "North Enoshaven", "merchant_zip": "72112"
        }
    })
}

fn fixture_transactions() -> TestResult<Vec<TransactionRecord>> {
    let bytes = std::fs::read(fixture_path("dailytran.txt"))?;
    Ok(decode_all::<DailyTransactionRecord>(&bytes)?
        .into_iter()
        .map(|framed| TransactionRecord::from(framed.record))
        .collect())
}

fn error_response(actual: &(StatusCode, Value), status: StatusCode, message: &str, field: &str) {
    assert_eq!(actual.0, status);
    assert_eq!(actual.1, json!({"message": message, "field": field}));
}

fn prefix<const N: usize>(value: &PicX<N>, width: usize) -> TestResult<String> {
    Ok(std::str::from_utf8(value.as_bytes().get(..width).ok_or("field width")?)?.to_owned())
}

/// Independent expansion of the eight-integer-position signed screen picture.
fn amount(value: Decimal) -> String {
    let sign = if value.is_sign_negative() { '-' } else { '+' };
    let magnitude = value.abs() % Decimal::from(100_000_000);
    format!("{sign}{magnitude:011.2}")
}

fn timestamp_format(value: &Value) -> TestResult {
    let text = value.as_str().ok_or("timestamp is not text")?;
    let parsed = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")?;
    assert_eq!(parsed.format("%Y-%m-%dT%H:%M:%S").to_string().len(), 19);
    Ok(())
}

fn copied(record: &TransactionRecord) -> TestResult<Value> {
    Ok(json!({
        "type_cd": prefix(&record.type_cd, 2)?,
        "cat_cd": format!("{:04}", record.cat_cd.get()),
        "source": prefix(&record.source, 10)?, "description": prefix(&record.desc, 60)?,
        "amount": amount(record.amt.as_decimal()),
        "orig_date": record.orig_ts.value().map(|ts| ts.value().date()),
        "proc_date": record.proc_ts.value().map(|ts| ts.value().date()),
        "merchant_id": format!("{:09}", record.merchant_id.get()),
        "merchant_name": prefix(&record.merchant_name, 30)?,
        "merchant_city": prefix(&record.merchant_city, 25)?,
        "merchant_zip": prefix(&record.merchant_zip, 10)?
    }))
}

fn paging(page: &Value, direction: &str) -> TestResult<Vec<(&'static str, String)>> {
    let mut query = vec![("direction", direction.to_owned())];
    for key in ["first_key", "last_key", "page_no", "has_next_page"] {
        let value = page.get(key).ok_or("missing paging state")?;
        if !value.is_null() {
            query.push((
                key,
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned),
            ));
        }
    }
    Ok(query)
}

fn blocked(rule: &str, halt: &str) {
    eprintln!("BLOCKED({halt}): {rule}; no parity result asserted");
}

/// Runs `test` to completion on a current-thread runtime, the one Tokio's test attribute
/// builds, without the lint attributes that attribute's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

#[test]
fn cotrn02c_add_then_view_rows() -> TestResult {
    block_on(add_then_view_rows())
}

async fn add_then_view_rows() -> TestResult {
    let harness = Harness::loaded().await?;
    let token = harness.sign_on("USER0001").await?;
    key_edits(&harness, &token).await?;
    data_edits(&harness, &token).await?;
    map_lengths(&harness, &token).await?;
    confirmations(&harness, &token).await?;
    let id = add_and_copy(&harness, &token).await?;
    view_rows(&harness, &token, &id).await?;
    cross_reference_paths(&harness, &token).await?;
    // audit: COTRN02C-061
    assert_eq!(
        harness.send(post(TRANSACTIONS, None, &entered())?).await?.0,
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

async fn key_edits(h: &Harness, token: &str) -> TestResult {
    let cases = [
        (
            None,
            None,
            StatusCode::BAD_REQUEST,
            "Account or Card Number must be entered...",
            "acct_id",
        ),
        (
            Some("1"),
            Some(XREF_CARD),
            StatusCode::BAD_REQUEST,
            "Account ID must be Numeric...",
            "acct_id",
        ),
        (
            None,
            Some("ABC"),
            StatusCode::BAD_REQUEST,
            "Card Number must be Numeric...",
            "card_num",
        ),
        (
            None,
            Some(XREF_CARD_REDACTED),
            StatusCode::BAD_REQUEST,
            "Card Number must be Numeric...",
            "card_num",
        ),
        (
            Some(UNKNOWN_ACCT),
            None,
            StatusCode::NOT_FOUND,
            "Account ID NOT found...",
            "acct_id",
        ),
        (
            None,
            Some(SYNTHETIC_CARDS[0]),
            StatusCode::NOT_FOUND,
            "Card Number NOT found...",
            "card_num",
        ),
    ];
    for aid in ["ENTER", "PF5"] {
        for (acct, card, status, message, field) in cases {
            let mut request = entered();
            request["aid"] = json!(aid);
            request["acct_id"] = json!(acct);
            request["card_num"] = json!(card);
            // audit: COTRN02C-009
            // audit: COTRN02C-010
            // audit: COTRN02C-012
            // audit: COTRN02C-013
            // audit: COTRN02C-015
            // audit: COTRN02C-018
            // audit: COTRN02C-052
            error_response(&h.add(token, &request).await?, status, message, field);
        }
    }
    let mut request = entered();
    request["aid"] = json!("PF7");
    let actual = h.add(token, &request).await?;
    // audit: COTRN02C-007
    assert_eq!(
        actual,
        (StatusCode::BAD_REQUEST, json!({"message": INVALID_KEY}))
    );
    // An empty-table copy has no source-backed result.
    request["aid"] = json!("PF5");
    h.add(token, &request).await?;
    blocked("COTRN02C-053", "X-COTRN02C-H1");
    Ok(())
}

async fn data_edits(h: &Harness, token: &str) -> TestResult {
    let fields = [
        ("type_cd", "Type CD can NOT be empty..."),
        ("cat_cd", "Category CD can NOT be empty..."),
        ("source", "Source can NOT be empty..."),
        ("description", "Description can NOT be empty..."),
        ("amount", "Amount can NOT be empty..."),
        ("orig_date", "Orig Date can NOT be empty..."),
        ("proc_date", "Proc Date can NOT be empty..."),
        ("merchant_id", "Merchant ID can NOT be empty..."),
        ("merchant_name", "Merchant Name can NOT be empty..."),
        ("merchant_city", "Merchant City can NOT be empty..."),
        ("merchant_zip", "Merchant Zip can NOT be empty..."),
    ];
    for start in 0..fields.len() {
        let mut request = entered();
        request["confirm"] = json!("Y");
        for (field, _) in fields.iter().skip(start) {
            request["fields"][field] = json!("");
        }
        let (field, message) = fields[start];
        // Multiple invalid fields prove the first edit wins.
        // audit: COTRN02C-008
        // audit: COTRN02C-020
        // audit: COTRN02C-021
        // audit: COTRN02C-022
        // audit: COTRN02C-023
        // audit: COTRN02C-024
        // audit: COTRN02C-025
        // audit: COTRN02C-026
        // audit: COTRN02C-027
        // audit: COTRN02C-028
        // audit: COTRN02C-029
        // audit: COTRN02C-030
        // audit: COTRN02C-040
        error_response(
            &h.add(token, &request).await?,
            StatusCode::BAD_REQUEST,
            message,
            field,
        );
    }
    for (field, value, message) in [
        ("type_cd", "1", "Type CD must be Numeric..."),
        ("cat_cd", "1", "Category CD must be Numeric..."),
        ("amount", "12.34", "Amount should be in format -99999999.99"),
        (
            "amount",
            "*00000012.34",
            "Amount should be in format -99999999.99",
        ),
        (
            "amount",
            "+0000001X.34",
            "Amount should be in format -99999999.99",
        ),
        (
            "amount",
            "+00000012,34",
            "Amount should be in format -99999999.99",
        ),
        (
            "amount",
            "+00000012.3X",
            "Amount should be in format -99999999.99",
        ),
        (
            "orig_date",
            "2022/06/10",
            "Orig Date should be in format YYYY-MM-DD",
        ),
        (
            "proc_date",
            "202X-06-11",
            "Proc Date should be in format YYYY-MM-DD",
        ),
        ("merchant_id", "1", "Merchant ID must be Numeric..."),
    ] {
        let mut request = entered();
        request["fields"][field] = json!(value);
        // audit: COTRN02C-031
        // audit: COTRN02C-032
        // audit: COTRN02C-033
        // audit: COTRN02C-034
        // audit: COTRN02C-035
        // audit: COTRN02C-039
        error_response(
            &h.add(token, &request).await?,
            StatusCode::BAD_REQUEST,
            message,
            field,
        );
    }
    assert!(h.last().await?.is_none());
    Ok(())
}

async fn map_lengths(h: &Harness, token: &str) -> TestResult {
    for (field, length) in [("acct_id", 11), ("card_num", 16), ("confirm", 1)] {
        let mut request = entered();
        request[field] = json!("1".repeat(length + 1));
        // audit: COTRN02-013
        // audit: COTRN02-017
        // audit: COTRN02-057
        error_response(
            &h.add(token, &request).await?,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            field,
        );
    }
    for (field, length) in [
        ("type_cd", 2),
        ("cat_cd", 4),
        ("source", 10),
        ("description", 60),
        ("amount", 12),
        ("orig_date", 10),
        ("proc_date", 10),
        ("merchant_id", 9),
        ("merchant_name", 30),
        ("merchant_city", 25),
        ("merchant_zip", 10),
    ] {
        let mut request = entered();
        request["fields"][field] = json!("1".repeat(length + 1));
        // audit: COTRN02-021
        // audit: COTRN02-024
        // audit: COTRN02-027
        // audit: COTRN02-030
        // audit: COTRN02-033
        // audit: COTRN02-036
        // audit: COTRN02-039
        // audit: COTRN02-045
        // audit: COTRN02-048
        // audit: COTRN02-051
        // audit: COTRN02-054
        error_response(
            &h.add(token, &request).await?,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            &format!("fields.{field}"),
        );
    }
    Ok(())
}

async fn confirmations(h: &Harness, token: &str) -> TestResult {
    for confirm in ["", " ", "N", "n", "X"] {
        let mut request = entered();
        request["confirm"] = json!(confirm);
        let response = h.add(token, &request).await?;
        if confirm == "X" {
            // audit: COTRN02C-042
            error_response(
                &response,
                StatusCode::BAD_REQUEST,
                "Invalid value. Valid values are (Y/N)...",
                "confirm",
            );
        } else {
            // audit: COTRN02C-041
            assert_eq!(response.0, StatusCode::OK);
            assert_eq!(response.1["message"], "Confirm to add this transaction...");
            assert_eq!(response.1["field"], "confirm");
            assert!(response.1["new_tran_id"].is_null());
            // audit: COTRN02C-012
            assert_eq!(response.1["card_num"], XREF_CARD_REDACTED);
            // audit: COTRN02C-036
            assert_eq!(response.1["copied"]["amount"], "+00000012.34");
            // These assert parameter preservation and fresh results, not CEEDAYS's verdict.
            // audit: CSUTLDTC-001
            // audit: CSUTLDTC-002
            // audit: CSUTLDTC-003
            // audit: CSUTLDTC-005
            // audit: CSUTLDTC-028
            assert_eq!(response.1["copied"]["orig_date"], "2022-06-10");
            assert_eq!(response.1["copied"]["proc_date"], "2022-06-11");
        }
        // audit: COTRN02C-008
        assert!(h.last().await?.is_none());
    }
    let mut request = entered();
    request["acct_id"] = Value::Null;
    request["card_num"] = json!(XREF_CARD);
    let (status, response) = h.add(token, &request).await?;
    // audit: COTRN02C-016
    // audit: COTRN02C-017
    assert_eq!(
        (status, response["acct_id"].clone()),
        (StatusCode::OK, json!(XREF_ACCT))
    );
    assert_eq!(response["card_num"], XREF_CARD_REDACTED);
    Ok(())
}

async fn add_and_copy(h: &Harness, token: &str) -> TestResult<String> {
    let account_before = h.account(XREF_ACCT).await?;
    let mut store = h.store.clone();
    let card_key = CardNumber::new(PicX::from_text(XREF_CARD)?);
    let xref_before = CardXrefRepository::find_by_id(&mut store, &card_key).await?;
    let mut request = entered();
    request["confirm"] = json!("Y");
    let (status, first) = h.add(token, &request).await?;
    assert_eq!(status, StatusCode::OK);
    blocked("COTRN02C-044 empty-table ID", "X-COTRN02C-H1");
    let first_id = first["new_tran_id"].as_str().ok_or("no new ID")?.to_owned();
    let record = h.stored(&first_id).await?;
    check_inserted(&record)?;
    // audit: COTRN02C-049
    assert_eq!(
        first,
        json!({"acct_id":null, "card_num":null, "copied":null,
        "field":"acct_id", "new_tran_id":first_id,
        "message":format!("Transaction added successfully.  Your Tran ID is {first_id}.")})
    );
    let (status, second) = h.add(token, &request).await?;
    // audit: COTRN02C-043
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        second["new_tran_id"],
        format!("{:016}", first_id.parse::<u64>()? + 1)
    );
    // audit: COTRN02C-048
    assert_eq!(h.account(XREF_ACCT).await?, account_before);
    assert_eq!(
        CardXrefRepository::find_by_id(&mut store, &card_key).await?,
        xref_before
    );
    request["aid"] = json!("PF5");
    request["confirm"] = json!("");
    request["fields"] = json!({});
    let (status, response) = h.add(token, &request).await?;
    // audit: COTRN02C-054
    // audit: COTRN02C-055
    // audit: COTRN02C-056
    // audit: COTRN02C-057
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["copied"], copied(&record)?);
    assert_eq!(response["message"], "Confirm to add this transaction...");
    assert_eq!(response["acct_id"], XREF_ACCT);
    assert_eq!(response["card_num"], XREF_CARD_REDACTED);
    request["confirm"] = json!("y");
    // audit: COTRN02C-040
    assert_eq!(h.add(token, &request).await?.0, StatusCode::OK);
    assert_eq!(
        h.last().await?.ok_or("no last")?.id.to_text(),
        format!("{:016}", first_id.parse::<u64>()? + 2)
    );
    Ok(first_id)
}

fn check_inserted(record: &TransactionRecord) -> TestResult {
    // audit: COTRN02C-045
    assert_eq!(record.type_cd, PicX::from_text("01")?);
    assert_eq!(record.cat_cd.get(), 5);
    assert_eq!(record.source, PicX::from_text("POS TERM")?);
    assert_eq!(record.desc, PicX::from_text(DESC)?);
    assert_eq!(record.merchant_id.get(), 800_000_000);
    assert_eq!(record.merchant_name, PicX::from_text("Abshire-Lowe")?);
    assert_eq!(record.merchant_city, PicX::from_text("North Enoshaven")?);
    assert_eq!(record.merchant_zip, PicX::from_text("72112")?);
    assert!(record.card_num.expose() == &PicX::from_text(XREF_CARD)?);
    // audit: COTRN02C-047
    assert_eq!(record.amt.as_decimal(), Decimal::new(1234, 2));
    // audit: COTRN02C-046
    // Each field holds its own entered date of `entered()` at midnight, never a clock value.
    for (field, stored, (year, month, day)) in [
        ("orig_ts", &record.orig_ts, (2022, 6, 10)),
        ("proc_ts", &record.proc_ts, (2022, 6, 11)),
    ] {
        let ts = stored.value().ok_or("no stored timestamp")?;
        let midnight = NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .ok_or("an invalid entered date")?;
        assert_eq!(ts.form(), TsForm::ScreenDate, "{field} layout");
        assert_eq!(ts.value(), midnight, "{field} at 00:00:00.000000");
    }
    Ok(())
}

async fn cross_reference_paths(h: &Harness, token: &str) -> TestResult {
    h.insert_xref(SYNTHETIC_CARDS[0], ORPHAN_ACCT).await?;
    // audit: COTRN02C-011
    assert!(h.account(ORPHAN_ACCT).await?.is_none());
    let mut request = entered();
    request["acct_id"] = json!(ORPHAN_ACCT);
    request["card_num"] = json!("not-a-number");
    request["fields"]["type_cd"] = json!("99");
    request["fields"]["cat_cd"] = json!("9999");
    // No account or reference-data lookup is part of the edit.
    // audit: COTRN02C-031
    // audit: COTRN02C-032
    assert_eq!(h.add(token, &request).await?.0, StatusCode::OK);
    h.insert_xref(SYNTHETIC_CARDS[1], SHARED_ACCT).await?;
    h.insert_xref(SYNTHETIC_CARDS[2], SHARED_ACCT).await?;
    request["acct_id"] = json!(SHARED_ACCT);
    // audit: COTRN02C-014
    error_response(
        &h.add(token, &request).await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unable to lookup Acct in XREF AIX file...",
        "acct_id",
    );
    let records = fixture_transactions()?;
    h.insert(&records[..1]).await?;
    request["acct_id"] = json!(XREF_ACCT);
    request["aid"] = json!("PF5");
    // The whole-file highest is unrelated to the entered account and has blank processing time.
    // audit: COTRN02C-053
    // audit: COTRN02C-056
    error_response(
        &h.add(token, &request).await?,
        StatusCode::BAD_REQUEST,
        "Proc Date can NOT be empty...",
        "proc_date",
    );
    Ok(())
}

async fn view_rows(h: &Harness, token: &str, id: &str) -> TestResult {
    let record = h.stored(id).await?;
    let before = h.last().await?;
    let admin = h.sign_on("ADMIN001").await?;
    for bearer in [token, &admin] {
        let (status, response) = h.view(bearer, id).await?;
        // audit: COTRN01C-015
        assert_eq!(status, StatusCode::OK);
        check_view(&response, &record)?;
        // audit: COTRN01C-011
        assert_eq!(h.stored(id).await?, record);
        assert_eq!(h.last().await?, before);
        // audit: COTRN02C-003
        assert_eq!(h.add(bearer, &entered()).await?.0, StatusCode::OK);
    }
    for (id, status, message) in [
        (
            "%20",
            StatusCode::BAD_REQUEST,
            "Tran ID can NOT be empty...",
        ),
        (
            "9999999999999998",
            StatusCode::NOT_FOUND,
            "Transaction ID NOT found...",
        ),
        ("1", StatusCode::NOT_FOUND, "Transaction ID NOT found..."),
        (
            "%20000000000000001",
            StatusCode::NOT_FOUND,
            "Transaction ID NOT found...",
        ),
    ] {
        // audit: COTRN01C-009
        // audit: COTRN01C-010
        // audit: COTRN01C-012
        error_response(&h.view(token, id).await?, status, message, "tran_id");
    }
    // One character beyond the map `LENGTH` of `TRNIDIN` is refused by the request DTO, while
    // the sixteen-character `9999999999999998` above reaches the read.
    // audit: COTRN01-013
    error_response(
        &h.view(token, "11111111111111111").await?,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        "tran_id",
    );
    Ok(())
}

fn check_view(response: &Value, record: &TransactionRecord) -> TestResult {
    let mut expected = copied(record)?;
    let object = expected.as_object_mut().ok_or("expected fields")?;
    object.remove("orig_date");
    object.remove("proc_date");
    object.insert("tran_id".into(), json!(record.id.to_text()));
    object.insert("card_num".into(), json!(record.card_num.redacted()));
    object.insert("message".into(), Value::Null);
    for (key, stored) in [("orig_ts", &record.orig_ts), ("proc_ts", &record.proc_ts)] {
        // Only format, not the clock instant, is a timestamp parity assertion.
        if stored.value().is_none() {
            assert!(response[key].is_null());
        } else {
            timestamp_format(&response[key])?;
        }
        object.insert(key.into(), response[key].clone());
    }
    // audit: COTRN01C-016
    // audit: COTRN01C-017
    // audit: COTRN01C-018
    // audit: COTRN01C-019
    // audit: COTRN01C-020
    // audit: COTRN01C-021
    // audit: COTRN01C-022
    // audit: COTRN01C-023
    // audit: COTRN01C-024
    // audit: COTRN01C-025
    // audit: COTRN01C-026
    // audit: COTRN01C-027
    // audit: COTRN01C-028
    assert_eq!(response, &expected);
    Ok(())
}

#[test]
fn cotrn00c_list_rows() -> TestResult {
    block_on(list_rows())
}

async fn list_rows() -> TestResult {
    let h = Harness::loaded().await?;
    let token = h.sign_on("USER0001").await?;
    h.list(&token, &[]).await?;
    blocked("COTRN00C-020 empty browse", "X-COTRN00C-H1");
    let records: Vec<_> = fixture_transactions()?
        .into_iter()
        .take(LIST_RECORDS)
        .collect();
    h.insert(&records[..1]).await?;
    let (_, single) = h.list(&token, &[("entry_key", "pf5".to_owned())]).await?;
    // audit: COTRN00C-004
    assert_eq!(
        single["rows"],
        json!([null, null, null, null, null, null, null, null, null, null])
    );
    assert_eq!(single["page_no"], 0);
    assert_eq!(single["has_next_page"], false);
    assert_eq!(
        single["message"],
        "You have reached the bottom of the page..."
    );
    h.insert(&records[1..]).await?;
    let (status, first) = h.list(&token, &[]).await?;
    // audit: COTRN00C-003
    // audit: COTRN00C-011
    // audit: COTRN00C-014
    // audit: COTRN00C-016
    assert_eq!(status, StatusCode::OK);
    check_page(&first, &records[..10], 1, true, None)?;
    let (status, second) = h.list(&token, &paging(&first, "forward")?).await?;
    // audit: COTRN00C-015
    assert_eq!(status, StatusCode::OK);
    check_page(&second, &records[10..20], 2, true, None)?;
    let (_, third) = h.list(&token, &paging(&second, "forward")?).await?;
    // audit: COTRN00C-018
    check_page(
        &third,
        &records[20..],
        3,
        false,
        Some("You have reached the bottom of the page..."),
    )?;
    assert_eq!(third["last_key"], second["last_key"]);
    let (_, bottom) = h.list(&token, &paging(&third, "forward")?).await?;
    // audit: COTRN00C-021
    assert_kept(
        &bottom,
        &third,
        false,
        "You are already at the bottom of the page...",
    );
    let (_, back) = h.list(&token, &paging(&third, "backward")?).await?;
    // audit: COTRN00C-023
    // audit: COTRN00C-024
    check_page(&back, &records[10..20], 2, true, None)?;
    let (_, top) = h.list(&token, &paging(&back, "backward")?).await?;
    // audit: COTRN00C-025
    check_page(
        &top,
        &records[..10],
        1,
        true,
        Some("You have reached the top of the page..."),
    )?;
    let (_, kept) = h.list(&token, &paging(&top, "backward")?).await?;
    // audit: COTRN00C-022
    assert_kept(
        &kept,
        &top,
        true,
        "You are already at the top of the page...",
    );
    search_and_boundaries(&h, &token, &records).await?;
    for record in &records {
        let (status, response) = h.view(&token, &record.id.to_text()).await?;
        assert_eq!(status, StatusCode::OK);
        check_view(&response, record)?;
    }
    Ok(())
}

fn check_page(
    page: &Value,
    records: &[TransactionRecord],
    number: u64,
    next: bool,
    message: Option<&str>,
) -> TestResult {
    let rows = page["rows"].as_array().ok_or("no rows")?;
    assert_eq!(rows.len(), PAGE_ROWS);
    assert_eq!(page["rows_kept"], false);
    assert_eq!(page["page_no"], number);
    assert_eq!(page["has_next_page"], next);
    // audit: COTRN00C-035
    assert_eq!(page["message"], json!(message));
    for (index, row) in rows.iter().enumerate() {
        if let Some(record) = records.get(index) {
            let ts = record.orig_ts.value().ok_or("fixture date blank")?;
            timestamp_format(&row["orig_ts"])?;
            // audit: COTRN00C-028
            // audit: COTRN00C-029
            // audit: COTRN00C-031
            // audit: COTRN00C-032
            // audit: COTRN00C-034
            assert_eq!(
                row,
                &json!({"tran_id":record.id.to_text(),
                "orig_ts":row["orig_ts"], "display_date":ts.value().format("%m/%d/%y").to_string(),
                "description":prefix(&record.desc, 26)?, "amount":amount(record.amt.as_decimal())})
            );
        } else {
            assert!(row.is_null());
        }
    }
    assert_eq!(
        page["first_key"],
        records.first().ok_or("no first record")?.id.to_text()
    );
    if records.len() == PAGE_ROWS {
        assert_eq!(
            page["last_key"],
            records.last().ok_or("no last record")?.id.to_text()
        );
    }
    Ok(())
}

fn assert_kept(page: &Value, previous: &Value, next: bool, message: &str) {
    assert_eq!(page["rows"], json!([]));
    assert_eq!(page["rows_kept"], true);
    assert_eq!(page["page_no"], previous["page_no"]);
    assert_eq!(page["first_key"], previous["first_key"]);
    assert_eq!(page["last_key"], previous["last_key"]);
    assert_eq!(page["has_next_page"], next);
    assert_eq!(page["message"], message);
}

async fn search_and_boundaries(
    h: &Harness,
    token: &str,
    records: &[TransactionRecord],
) -> TestResult {
    for entry in ["enter", "pf3", "pf5"] {
        let (_, response) = h.list(token, &[("entry_key", entry.to_owned())]).await?;
        let skip = usize::from(entry == "pf5");
        // audit: COTRN00C-004
        check_page(&response, &records[skip..skip + 10], 1, true, None)?;
    }
    let between = format!("{:016}", records[4].id.to_text().parse::<u64>()? + 1);
    let (_, searched) = h.list(token, &[("start_tran_id", between)]).await?;
    // audit: COTRN00C-012
    check_page(&searched, &records[5..15], 1, true, None)?;
    let (_, exact) = h
        .list(token, &[("start_tran_id", records[15].id.to_text())])
        .await?;
    // audit: COTRN00C-017
    check_page(
        &exact,
        &records[15..],
        1,
        false,
        Some("You have reached the bottom of the page..."),
    )?;
    let (_, top) = h.list(token, &paging(&exact, "backward")?).await?;
    // audit: COTRN00C-022
    assert_kept(
        &top,
        &exact,
        true,
        "You are already at the top of the page...",
    );
    let (_, end) = h.list(token, &paging(&top, "forward")?).await?;
    // audit: COTRN00C-019
    assert_eq!(
        end["rows"],
        json!([null, null, null, null, null, null, null, null, null, null])
    );
    assert_eq!(end["page_no"], 1);
    assert_eq!(end["has_next_page"], false);
    assert_eq!(end["message"], "You have reached the bottom of the page...");
    for preceding in [0, 5] {
        let query = [
            ("direction", "backward".to_owned()),
            ("page_no", "2".to_owned()),
            ("first_key", records[preceding].id.to_text()),
            ("last_key", records[14].id.to_text()),
        ];
        let (_, short) = h.list(token, &query).await?;
        let rows = short["rows"].as_array().ok_or("rows")?;
        // audit: COTRN00C-026
        assert_eq!(rows.len(), PAGE_ROWS);
        assert!(rows.iter().take(PAGE_ROWS - preceding).all(Value::is_null));
        assert_eq!(short["page_no"], 2);
        assert_eq!(short["has_next_page"], true);
        assert_eq!(short["first_key"], records[preceding].id.to_text());
        let last = if preceding == 0 { 14 } else { preceding - 1 };
        assert_eq!(short["last_key"], records[last].id.to_text());
        assert_eq!(short["message"], "You have reached the top of the page...");
        for (row, record) in rows.iter().skip(PAGE_ROWS - preceding).zip(records) {
            assert_eq!(row["tran_id"], record.id.to_text());
        }
    }
    // audit: COTRN00-015
    error_response(
        &h.list(token, &[("start_tran_id", "1".repeat(17))]).await?,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        "start_tran_id",
    );
    Ok(())
}

#[test]
fn view_and_copy_numeric_pictures() -> TestResult {
    block_on(numeric_pictures())
}

async fn numeric_pictures() -> TestResult {
    let h = Harness::loaded().await?;
    let token = h.sign_on("USER0001").await?;
    let mut record = fixture_transactions()?
        .into_iter()
        .next()
        .ok_or("fixture empty")?;
    record.desc = PicX::from_text(&"D".repeat(100))?;
    record.merchant_name = PicX::from_text(&"N".repeat(50))?;
    record.merchant_city = PicX::from_text(&"C".repeat(50))?;
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_opt(10, 30, 0))
        .ok_or("test instant")?;
    record.orig_ts = PicTimestamp::new(Some(CobolTimestamp::new(at, TsForm::Db2)?));
    record.proc_ts = PicTimestamp::new(Some(CobolTimestamp::new(at, TsForm::Iso)?));
    let mut request = entered();
    request["aid"] = json!("PF5");
    for (index, (cents, picture)) in [
        (0, "+00000000.00"),
        (-1200, "-00000012.00"),
        (12_345_678_901, "+23456789.01"),
        (-12_345_678_901, "-23456789.01"),
    ]
    .into_iter()
    .enumerate()
    {
        record.id = PicX::from_text(&format!("{:016}", index + 1))?;
        record.amt = domain::money::Money::new(Decimal::new(cents, 2))?;
        h.insert(&[record.clone()]).await?;
        let (status, response) = h.view(&token, &record.id.to_text()).await?;
        assert_eq!(status, StatusCode::OK);
        check_view(&response, &record)?;
        // audit: COTRN01C-022
        assert_eq!(response["amount"], picture);
        let (status, response) = h.add(&token, &request).await?;
        // audit: COTRN02C-054
        // audit: COTRN02C-055
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["copied"], copied(&record)?);
        assert_eq!(response["copied"]["amount"], picture);
        assert_eq!(h.stored(&record.id.to_text()).await?, record);
    }
    Ok(())
}

#[test]
fn store_failures_answer_the_lookup_messages() -> TestResult {
    block_on(store_failure_messages())
}

async fn store_failure_messages() -> TestResult {
    let guard = start_postgres().await?;
    guard.pool().close().await;
    let h = Harness::over(guard)?;
    // A closed database cannot sign on; the same signing keys supply the verified identity.
    let token = h
        .state
        .jwt
        .issue(&PicX::from_text("USER0001")?, &UserType::USER)?;
    let (status, response) = h.list(&token, &[]).await?;
    // audit: COTRN00C-027
    assert_eq!(
        (status, response),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"message":"Unable to lookup transaction..."})
        )
    );
    // audit: COTRN01C-013
    error_response(
        &h.view(&token, "0000000000000001").await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unable to lookup Transaction...",
        "tran_id",
    );
    let request = entered();
    // audit: COTRN02C-014
    error_response(
        &h.add(&token, &request).await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unable to lookup Acct in XREF AIX file...",
        "acct_id",
    );
    let mut request = entered();
    request["acct_id"] = Value::Null;
    request["card_num"] = json!(XREF_CARD);
    // audit: COTRN02C-019
    error_response(
        &h.add(&token, &request).await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unable to lookup Card # in XREF file...",
        "card_num",
    );
    // The rejected search does not issue a page query; its final text is halted.
    let (status, response) = h
        .list(&token, &[("start_tran_id", "ABC".to_owned())])
        .await?;
    // audit: COTRN00C-013
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.get("rows").is_none());
    blocked("COTRN00C-040 final message/status", "X-COTRN00C-H2");
    // audit: COTRN01C-009
    error_response(
        &h.view(&token, "%20").await?,
        StatusCode::BAD_REQUEST,
        "Tran ID can NOT be empty...",
        "tran_id",
    );
    request["card_num"] = json!("ABC");
    // audit: COTRN02C-015
    error_response(
        &h.add(&token, &request).await?,
        StatusCode::BAD_REQUEST,
        "Card Number must be Numeric...",
        "card_num",
    );
    Ok(())
}

#[test]
fn allocation_and_write_failures_roll_back() -> TestResult {
    block_on(write_failures_roll_back())
}

async fn write_failures_roll_back() -> TestResult {
    let mut h = Harness::loaded().await?;
    let token = h.sign_on("USER0001").await?;
    let mut request = entered();
    request["confirm"] = json!("Y");
    // Startup parameters make only writes fail: no SQL is issued by the test and the actual
    // repository still performs its reads, transaction and insert against PostgreSQL.
    let pool = h.store.pool();
    let options = (*pool.connect_options())
        .clone()
        .options([("default_transaction_read_only", "on")]);
    let read_only = pool.options().clone().connect_with(options).await?;
    let writable = h.state.store.clone();
    h.state.store = PgStore::new(read_only.clone());
    // audit: COTRN02C-051
    error_response(
        &h.add(&token, &request).await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unable to Add Transaction...",
        "acct_id",
    );
    assert!(h.last().await?.is_none());
    h.state.store = writable;
    read_only.close().await;
    let mut high = fixture_transactions()?
        .into_iter()
        .next()
        .ok_or("fixture empty")?;
    high.id = PicX::from_text("9999999999999999")?;
    high.filler = PicX::from_text("retained filler")?;
    h.insert(&[high.clone()]).await?;
    let (status, response) = h.add(&token, &request).await?;
    // audit: COTRN02C-043
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["new_tran_id"], "0000000000000000");
    // audit: COTRN02C-045
    assert_eq!(h.stored("0000000000000000").await?.filler, high.filler);
    // Wrapping again is a deterministic primary-key violation, not a timing-sensitive race.
    // audit: COTRN02C-050
    error_response(
        &h.add(&token, &request).await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Tran ID already exist...",
        "acct_id",
    );
    assert_eq!(h.last().await?, Some(high.clone()));
    // The typed repository refuses numeric allocation from a nonnumeric highest key.
    high.id = PicX::from_text("ZZZZZZZZZZZZZZZZ")?;
    h.insert(&[high.clone()]).await?;
    // audit: COTRN02C-044
    error_response(
        &h.add(&token, &request).await?,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Unable to lookup Transaction...",
        "acct_id",
    );
    assert_eq!(h.last().await?, Some(high));
    Ok(())
}

// Derive-lane coverage requires tags even on blocked bindings. An explicit run fails instead
// of turning the unresolved oracle into an empty passing test.
// audit: COTRN02C-037
// audit: COTRN02C-038
// audit: CSUTLDTC-004
// audit: CSUTLDTC-006
// audit: CSUTLDTC-007
// audit: CSUTLDTC-008
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
// audit: CSUTLDTC-029
#[test]
#[ignore = "BLOCKED(H5): COTRN02C-037/038, CSUTLDTC-004/006..018/029; validity delegated to CEEDAYS"]
fn blocked_h5_date_validity() -> TestResult {
    Err("BLOCKED(H5): no source-backed date-validity verdict".into())
}

// audit: COTRN00C-020
// audit: COTRN00C-040
// audit: COTRN02C-044
// audit: COTRN02C-053
#[test]
#[ignore = "BLOCKED: X-COTRN00C-H1/H2, X-COTRN02C-H1; CICS browse and initial storage outcomes"]
fn blocked_browse_and_unassigned_storage() -> TestResult {
    Err("BLOCKED: browse and unassigned storage have no single COBOL outcome".into())
}

// audit: COTRN00C-030
// audit: COTRN00C-033
// audit: COTRN02C-036
// audit: COTRN02C-016
#[test]
#[ignore = "BLOCKED: X-COTRN00C-H3, X-COTRN02C-H2, X-ARITH-5; compiled MOVE/NUMVAL behaviour"]
fn blocked_compiler_dependent_values() -> TestResult {
    Err(
        "BLOCKED: blank date, negative zero and wide NUMVAL need deployed compiler semantics"
            .into(),
    )
}
