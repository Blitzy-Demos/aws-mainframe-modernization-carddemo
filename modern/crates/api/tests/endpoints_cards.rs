//! Binding g1b for the card programs: every `server` row of `modern/audit/online/COCRDLIC.md`,
//! `COCRDSLC.md` and `COCRDUPC.md`, and the server half of every `both` row there and of maps
//! `COCRDLI`, `COCRDSL` and `COCRDUP` in `modern/audit/screens.md`, proved through endpoints 7
//! to 10 over a fixture-loaded `PostgreSQL` (AAP 0.9.1).
//!
//! They also bind the COMMAREA row COCOM01Y-014 of `modern/audit/copybooks.md`:
//! `cocom01y_card_rows` proves that the selected card travels from the list to the view and
//! update fetches as its `card_ref` handle, never as its number, which each of those answers
//! shows only as twelve `*` and the last four digits.
//!
//! # Harness
//!
//! Each test function starts its own container through
//! `test_support::fixtures::start_loaded_postgres`, builds the API over it with a `FixedClock`
//! and sends every request in process with `tower::ServiceExt::oneshot`. Both tokens come from
//! endpoint 1: `USER0001` (type `U`) and `ADMIN001` (type `A`). Expected pages are computed from
//! `carddata.txt`, sorted by card number, with the handle each card received at the load.
//!
//! Every response body is scanned as it arrives: no fixture or inserted card number may appear
//! in it, except the one card an `A` token asks endpoint 8 for (Cardholder Data Redaction), and
//! no JSON key may name a CVV. Assertion messages print no body.
//!
//! Store failures are real, not simulated: a second pool on the same database, closed before
//! use, makes every read fail, so the file-error rows run through the real repositories.
//!
//! # Halted rows
//!
//! A row whose case a halt or halt candidate blocks asserts no value. Its tag sits on an ignored
//! test that reports the case `BLOCKED`, because the a8 coverage check requires blocked rows to
//! carry their layer's tags like every other row.
//!
//! # Audited results that look like mistakes
//!
//! - A valid card under another account is found and shown under the account entered
//!   (COCRDSLC-014, COCRDUPC-016), never a not-found answer.
//! - `Account number must be a non zero 11 digit number` and `Card number if supplied must be a
//!   16 digit number` are declared and never moved, so the tests assert that no response carries
//!   them (COCRDSLC-028, COCRDSLC-029).
//! - The CVV a save writes is not asserted (COCRDUPC-038 under `X-COCRDUPC-H2`).

use std::cell::RefCell;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app};
use api::dto::FIELD_LENGTH_MESSAGE;
use axum::body::{self, Body};
use axum::http::{Method, Request, StatusCode, header};
use chrono::NaiveDate;
use domain::clock::FixedClock;
use domain::copybook::{PicDate, PicU, PicX};
use domain::online::cocrdlic::CardRef;
use domain::records::card::CardRecord;
use domain::sensitive::{CardNumber, Cvv};
use persistence::{CardRepository, PgStore};
use runtime::config::Config;
use serde_json::{Value, json};
use test_support::fixtures::{fixture_card_numbers, fixture_path, start_loaded_postgres};
use test_support::postgres::PostgresGuard;
use tower::ServiceExt as _;

type TestResult = Result<(), Box<dyn Error>>;

type Outcome<T> = Result<T, Box<dyn Error>>;

/// A query string as name and value pairs, each value percent-encoded when sent.
type Query = Vec<(&'static str, String)>;

/// One listed row as the tests compare it: handle, card number, account and status.
type Row = (i64, String, String, String);

const SIGNON: &str = "/api/v1/signon";
const LIST: &str = "/api/v1/cards";
const VIEW: &str = "/api/v1/cards/view";
const EDIT: &str = "/api/v1/cards/edit";

/// The HS256 secret of the test API: a local-only value.
const SECRET: &str = "endpoints-cards-local-secret";

/// The password of every `DUSRSECJ` user (`README.md:203-204`).
const PASSWORD: &str = "PASSWORD";

/// The rows of one COCRDLIC page (`WS-MAX-SCREEN-LINES`, COCRDLIC-017).
const PAGE_ROWS: usize = 7;

// COCRDLIC message lines, as `modern/audit/online/COCRDLIC.md` quotes them.
const RECORD_ACTIONS: &str = "TYPE S FOR DETAIL, U TO UPDATE ANY RECORD";
const NO_RECORDS_FOUND: &str = "NO RECORDS FOUND FOR THIS SEARCH CONDITION.";
const NO_MORE_RECORDS: &str = "NO MORE RECORDS TO SHOW";
const NO_PREVIOUS_PAGES: &str = "NO PREVIOUS PAGES TO DISPLAY";
const NO_MORE_PAGES: &str = "NO MORE PAGES TO DISPLAY";

/// Bytes 1-48 of the file-error text shared by the three programs (COCRDLIC-031,
/// COCRDSLC-016, COCRDUPC-019): `'File Error: '`, `'READ'` padded to 8, `' on '`, `'CARDDAT '`
/// padded to 9 and `' returned RESP '`.
const FILE_ERROR_HEAD: &str = concat!(
    "File Error: ",
    "READ    ",
    " on ",
    "CARDDAT  ",
    " returned RESP "
);

/// Bytes 59-65 of the file-error text; bytes 49-58 and 66-75 are `HALTED(H4)`.
const FILE_ERROR_RESP2: &str = ",RESP2 ";

// COCRDSLC and COCRDUPC messages, as their audits quote them.
const DISPLAYING_DETAILS: &str = "   Displaying requested details";
const NO_INPUT: &str = "No input received";
const PROMPT_FOR_ACCT: &str = "Account number not provided";
const PROMPT_FOR_CARD: &str = "Card number not provided";
const ACCOUNT_FILTER: &str = "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";
const CARD_FILTER: &str = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";
const NOT_FOUND: &str = "Did not find cards for this search condition";
const DETAILS_SHOWN: &str = "Details of selected card shown above";
const NO_CHANGE: &str = "No change detected with respect to values fetched.";
const PROMPT_FOR_NAME: &str = "Card name not provided";
const NAME_ALPHA: &str = "Card name can only contain alphabets and spaces";
const STATUS_YES_NO: &str = "Card Active Status must be Y or N";
const MONTH_INVALID: &str = "Card expiry month must be between 1 and 12";
const YEAR_INVALID: &str = "Invalid card expiry year";
const CONFIRM_PROMPT: &str = "Changes validated.Press F5 to save";
const COMMITTED: &str = "Changes committed to database";
const LOCK_FAILED: &str = "Could not lock record for update";
const CHANGED_BY_OTHER: &str = "Record changed by some one else. Please review";
const UPDATE_FAILED: &str = "Update of record failed";

/// Texts COCRDSLC declares and never moves (COCRDSLC-028 to COCRDSLC-031), with the prompt the
/// screen alone shows (COCRDSLC-025).
const NEVER_SENT_BY_VIEW: [&str; 7] = [
    "Account number must be a non zero 11 digit number",
    "Card number if supplied must be a 16 digit number",
    "Did not find this account in cards database",
    "Error reading Card Data File",
    "Looks Good.... so far",
    "PF03 pressed.Exiting",
    "Please enter Account and Card Number",
];

/// One card as the tests expect to see it: from `carddata.txt` (its 1-based `line`) or inserted
/// by a test (`line` 0), with the handle the database assigned.
///
/// `number` is a protected value: it is sent only as request input and compared as bytes, never
/// printed; the type has no `Debug` for that reason.
#[derive(Clone)]
struct Card {
    line: usize,
    number: String,
    redacted: String,
    acct: String,
    name: String,
    expiry: String,
    status: String,
    handle: i64,
}

impl Card {
    /// The card of one 150-byte `carddata.txt` record (`CVACT02Y`): number at 0-15, account at
    /// 16-26, embossed name at 30-79, expiry date at 80-89 and status at 90.
    fn parse(line: usize, record: &[u8]) -> Outcome<Self> {
        let field = |from: usize, to: usize| -> Outcome<String> {
            let bytes = record
                .get(from..to)
                .ok_or("a carddata.txt record is shorter than CVACT02Y")?;
            Ok(String::from_utf8(bytes.to_vec())?)
        };
        let number = field(0, 16)?;
        Ok(Self {
            line,
            redacted: redacted(&number)?,
            acct: field(16, 27)?,
            name: field(30, 80)?,
            expiry: field(80, 90)?,
            status: field(90, 91)?,
            number,
            handle: 0,
        })
    }

    /// The card as one listed row of endpoint 7 (COCRDLIC-032).
    fn row(&self) -> Row {
        (
            self.handle,
            self.redacted.clone(),
            self.acct.clone(),
            self.status.clone(),
        )
    }

    /// The handle as a query or path value.
    fn handle_text(&self) -> String {
        self.handle.to_string()
    }
}

/// Twelve `*` and the last four digits of `number` (PIC-024, Cardholder Data Redaction).
fn redacted(number: &str) -> Outcome<String> {
    let last4 = number
        .get(12..16)
        .ok_or("a card number is shorter than 16 characters")?;
    Ok(format!("************{last4}"))
}

/// The cards of `carddata.txt` in card-number order, each with the handle its load assigned.
async fn fixture_cards(store: &PgStore) -> Outcome<Vec<Card>> {
    let text = fs::read(fixture_path("carddata.txt"))?;
    let mut cards = Vec::new();
    for (index, record) in text.split(|&byte| byte == b'\n').enumerate() {
        let record = record.strip_suffix(b"\r").unwrap_or(record);
        if !record.is_empty() {
            cards.push(Card::parse(index + 1, record)?);
        }
    }
    cards.sort_by(|left, right| left.number.cmp(&right.number));
    let mut store = store.clone();
    let entries = CardRepository::find_all(&mut store).await?;
    for card in &mut cards {
        card.handle = entries
            .iter()
            .find(|(_, record)| record.num.expose().as_bytes().as_slice() == card.number.as_bytes())
            .map(|(handle, _)| handle.get())
            .ok_or("a fixture card is missing from the loaded table")?;
    }
    Ok(cards)
}

/// The cards of page `page` (1-based) of an unfiltered list over `cards`.
fn page_of(cards: &[Card], page: usize) -> Vec<&Card> {
    cards
        .iter()
        .skip(page.saturating_sub(1).saturating_mul(PAGE_ROWS))
        .take(PAGE_ROWS)
        .collect()
}

/// The expected rows of `cards`, in order.
fn rows(cards: &[&Card]) -> Vec<Row> {
    cards.iter().map(|card| card.row()).collect()
}

/// The fixed instant of the test clock, 2022-07-18 10:30:00.000.
fn instant() -> Outcome<chrono::NaiveDateTime> {
    NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or_else(|| "2022-07-18 10:30:00.000 is an instant".into())
}

/// The API state over `store`, with the test secret, the fixed clock and a job-deck directory
/// under Cargo's per-target temporary directory, which no request of this file writes.
fn state_over(store: PgStore) -> Outcome<AppState> {
    let config = Config {
        jwt_secret: SECRET.to_owned(),
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints_cards_jobs"),
        ..Config::default()
    };
    Ok(AppState::from_config(
        store,
        &config,
        Arc::new(FixedClock::new(instant()?)),
    )?)
}

/// `pairs` as a [`Query`].
fn query(pairs: &[(&'static str, &str)]) -> Query {
    pairs
        .iter()
        .map(|&(name, value)| (name, value.to_owned()))
        .collect()
}

/// The query of endpoints 8 and 9 naming the card by its typed number.
fn pair(acct: &str, card: &str) -> Query {
    query(&[("acct_id", acct), ("card_num", card)])
}

/// The query of endpoints 8 and 9 naming the card by a list row's handle.
fn by_ref(acct: &str, handle: &str) -> Query {
    query(&[("acct_id", acct), ("card_ref", handle)])
}

/// `value` percent-encoded for a query string: every byte but ASCII letters, digits and
/// `-_.~*` as `%XX`, so spaces and other screen bytes arrive as typed.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'*') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `path` with `pairs` as its query string.
fn uri(path: &str, pairs: &[(&'static str, String)]) -> String {
    if pairs.is_empty() {
        return path.to_owned();
    }
    let parts: Vec<String> = pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect();
    format!("{path}?{}", parts.join("&"))
}

/// Whether any key of `value`, at any depth, names a CVV.
fn has_cvv_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, item)| key.to_ascii_lowercase().contains("cvv") || has_cvv_key(item)),
        Value::Array(items) => items.iter().any(has_cvv_key),
        _ => false,
    }
}

/// The keys of the JSON object `value`, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut found: Vec<String> = value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// `names` sorted, as [`keys`] returns them.
fn sorted(names: &[&str]) -> Vec<String> {
    let mut list: Vec<String> = names.iter().map(|&name| name.to_owned()).collect();
    list.sort();
    list
}

/// A response: its status and its JSON body (`null` for an empty body).
struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    fn text(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(Value::as_str)
    }

    fn int(&self, key: &str) -> Option<i64> {
        self.body.get(key).and_then(Value::as_i64)
    }

    fn flag(&self, key: &str) -> Option<bool> {
        self.body.get(key).and_then(Value::as_bool)
    }

    /// The listed rows of an endpoint 7 page.
    fn rows(&self) -> Outcome<Vec<Row>> {
        let listed = self
            .body
            .get("rows")
            .and_then(Value::as_array)
            .ok_or("the page has no rows array")?;
        listed
            .iter()
            .map(|row| {
                let text = |key: &str| -> Outcome<String> {
                    Ok(row
                        .get(key)
                        .and_then(Value::as_str)
                        .ok_or("a row member is missing")?
                        .to_owned())
                };
                Ok((
                    row.get("card_ref")
                        .and_then(Value::as_i64)
                        .ok_or("a row has no card_ref")?,
                    text("card_num")?,
                    text("acct_id")?,
                    text("status")?,
                ))
            })
            .collect()
    }

    /// The body's `snapshot`, with its `state` set to `state`.
    fn snapshot_in(&self, state: &str) -> Outcome<Value> {
        let mut snapshot = self
            .body
            .get("snapshot")
            .cloned()
            .ok_or("the response has no snapshot")?;
        snapshot
            .as_object_mut()
            .ok_or("the snapshot is no object")?
            .insert("state".to_owned(), Value::from(state));
        Ok(snapshot)
    }
}

/// Asserts a `ScreenError`: `status`, `message` and `field`, and no other member.
fn assert_error(reply: &Reply, status: StatusCode, message: &str, field: Option<&str>) {
    assert_eq!(reply.status, status, "the status answering {message:?}");
    assert_eq!(reply.text("message"), Some(message));
    assert_eq!(reply.text("field"), field, "the field of {message:?}");
    let expected = if field.is_some() {
        sorted(&["field", "message"])
    } else {
        sorted(&["message"])
    };
    assert_eq!(keys(&reply.body), expected, "the members of {message:?}");
}

/// Asserts the 500 of a failed card read: bytes 1-48 and 59-65 of the file-error text, whose
/// code fields are not asserted (`HALTED(H4)`), and `field`.
fn assert_file_error(reply: &Reply, field: Option<&str>) {
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    let message = reply.text("message").unwrap_or_default();
    assert_eq!(message.get(..48), Some(FILE_ERROR_HEAD));
    assert_eq!(message.get(58..65), Some(FILE_ERROR_RESP2));
    assert_eq!(reply.text("field"), field);
}

/// Asserts a 200 page of endpoint 7: its rows, page number, error line and information line.
fn assert_page(
    reply: &Reply,
    cards: &[&Card],
    page_no: i64,
    message: Option<&str>,
    info: Option<&str>,
) -> Outcome<()> {
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.rows()?, rows(cards));
    assert_eq!(reply.int("page_no"), Some(page_no));
    assert_eq!(reply.text("message"), message);
    assert_eq!(reply.text("info_message"), info);
    Ok(())
}

/// The next request of the card list: the previous page's paging state echoed unchanged, with
/// the key pressed (`forward` F8, `backward` F7, none for ENTER), as the screen sends it.
fn echo(previous: &Reply, direction: Option<&str>) -> Query {
    let mut sent = Query::new();
    if let Some(direction) = direction {
        sent.push(("direction", direction.to_owned()));
    }
    for name in ["page_no", "first_key", "last_key"] {
        if let Some(value) = previous.int(name) {
            sent.push((name, value.to_string()));
        }
    }
    for name in ["has_next_page", "last_page_shown"] {
        if let Some(value) = previous.flag(name) {
            sent.push((name, value.to_string()));
        }
    }
    sent
}

/// `sent` with `name` set to `value`, replacing an echoed value.
fn with(mut sent: Query, name: &'static str, value: &str) -> Query {
    sent.retain(|(present, _)| *present != name);
    sent.push((name, value.to_owned()));
    sent
}

/// One API over a fixture-loaded database, signed on as `USER0001` and `ADMIN001`.
struct Harness {
    state: AppState,
    store: PgStore,
    user: String,
    admin: String,
    /// The card numbers no response may carry: the 50 fixture cards and every inserted one.
    needles: Vec<String>,
    /// Every response body this harness received, for the never-sent message rows.
    seen: RefCell<Vec<String>>,
    cards: Vec<Card>,
    /// Dropped last: it removes the container.
    guard: PostgresGuard,
}

impl Harness {
    async fn start() -> Outcome<Self> {
        let guard = start_loaded_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let mut harness = Self {
            state: state_over(store.clone())?,
            cards: fixture_cards(&store).await?,
            store,
            user: String::new(),
            admin: String::new(),
            needles: fixture_card_numbers()?,
            seen: RefCell::new(Vec::new()),
            guard,
        };
        harness.user = harness.sign_on("USER0001").await?;
        harness.admin = harness.sign_on("ADMIN001").await?;
        Ok(harness)
    }

    /// The bearer token endpoint 1 issues to `user_id`.
    async fn sign_on(&self, user_id: &str) -> Outcome<String> {
        let body = json!({"user_id": user_id, "password": PASSWORD});
        let reply = self
            .exchange(&self.state, Method::POST, SIGNON, None, Some(&body), None)
            .await?;
        assert_eq!(reply.status, StatusCode::OK, "sign-on of {user_id}");
        Ok(reply
            .text("token")
            .ok_or("sign-on returned no token")?
            .to_owned())
    }

    /// The fixture card of `carddata.txt` line `line`.
    fn line(&self, line: usize) -> Outcome<&Card> {
        self.cards
            .iter()
            .find(|card| card.line == line)
            .ok_or_else(|| format!("carddata.txt has no line {line}").into())
    }

    /// Sends one request to the API over `state` and scans its body: no card number of
    /// [`Self::needles`] but `disclosed`, and no CVV key.
    async fn exchange(
        &self,
        state: &AppState,
        method: Method,
        target: &str,
        token: Option<&str>,
        json: Option<&Value>,
        disclosed: Option<&str>,
    ) -> Outcome<Reply> {
        let mut builder = Request::builder().method(method).uri(target);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match json {
            Some(json) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(json)?))?,
            None => builder.body(Body::empty())?,
        };
        let response = app(state.clone()).oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        let text = String::from_utf8(bytes.to_vec())?;
        let leaked = self
            .needles
            .iter()
            .filter(|needle| Some(needle.as_str()) != disclosed && text.contains(needle.as_str()))
            .count();
        assert_eq!(leaked, 0, "a response body carried a full card number");
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        };
        assert!(!has_cvv_key(&body), "a response body names a CVV");
        self.seen.borrow_mut().push(text);
        Ok(Reply { status, body })
    }

    /// A `GET` of `path` with `sent` as its query string, under `token`.
    async fn get(
        &self,
        path: &str,
        sent: &[(&'static str, String)],
        token: &str,
    ) -> Outcome<Reply> {
        self.exchange(
            &self.state,
            Method::GET,
            &uri(path, sent),
            Some(token),
            None,
            None,
        )
        .await
    }

    /// As [`Self::get`], allowed to carry the full number `card`: the one disclosure of
    /// endpoint 8 to an `A` token (COCRDSLC-022).
    async fn get_disclosed(
        &self,
        path: &str,
        sent: &[(&'static str, String)],
        card: &str,
    ) -> Outcome<Reply> {
        let target = uri(path, sent);
        self.exchange(
            &self.state,
            Method::GET,
            &target,
            Some(&self.admin),
            None,
            Some(card),
        )
        .await
    }

    /// An endpoint 7 request under the `U` token.
    async fn list(&self, sent: &[(&'static str, String)]) -> Outcome<Reply> {
        self.get(LIST, sent, &self.user).await
    }

    /// An endpoint 10 request for the card `handle` names, under the `U` token.
    async fn put(&self, handle: &str, json: &Value) -> Outcome<Reply> {
        let target = format!("{LIST}/{handle}");
        self.exchange(
            &self.state,
            Method::PUT,
            &target,
            Some(&self.user),
            Some(json),
            None,
        )
        .await
    }

    /// A `GET` over a store whose pool is closed, so that every card read fails.
    async fn get_failing(&self, path: &str, sent: &[(&'static str, String)]) -> Outcome<Reply> {
        let config = Config {
            database_url: self.guard.database_url().to_owned(),
            ..Config::default()
        };
        let pool = runtime::db::connect(&config).await?;
        pool.close().await;
        let failing = state_over(PgStore::new(pool))?;
        self.exchange(
            &failing,
            Method::GET,
            &uri(path, sent),
            Some(&self.user),
            None,
            None,
        )
        .await
    }

    /// The stored card that `handle` names, read back through the card repository.
    async fn stored(&self, handle: i64) -> Outcome<CardRecord> {
        let mut store = self.store.clone();
        Ok(
            CardRepository::find_by_ref(&mut store, CardRef::new(handle))
                .await?
                .ok_or("no card has the handle")?
                .1,
        )
    }
}

/// Endpoint 7 over the 50 fixture cards: the fresh start, blank filters, paging both ways, the
/// last page, the filters and the message precedence, the map lengths and a failed read.
#[tokio::test]
async fn cocrdlic_list_rows() -> TestResult {
    let h = Harness::start().await?;
    let first = list_first_page(&h).await?;
    list_blank_filters(&h, &first).await?;
    let third = list_paging(&h, &first).await?;
    list_last_page(&h, &third).await?;
    list_filters(&h, &third).await?;
    list_lengths_and_store(&h).await?;
    Ok(())
}

/// The fresh start, with both tokens; returns page 1.
async fn list_first_page(h: &Harness) -> Outcome<Reply> {
    let page1 = page_of(&h.cards, 1);
    let page2 = page_of(&h.cards, 2);
    let fresh = h.list(&[]).await?;
    // audit: COCRDLIC-017
    assert_page(&fresh, &page1, 1, None, Some(RECORD_ACTIONS))?;
    // audit: COCRDLIC-022
    assert_eq!(
        fresh.int("first_key"),
        page1.first().map(|card| card.handle)
    );
    // audit: COCRDLIC-019
    assert_eq!(fresh.flag("has_next_page"), Some(true));
    // audit: COCRDLIC-019
    assert_eq!(fresh.int("last_key"), page2.first().map(|card| card.handle));
    // audit: COCRDLIC-033
    assert_eq!(fresh.text("info_message"), Some(RECORD_ACTIONS));
    assert_eq!(fresh.flag("last_page_shown"), Some(false));
    assert_eq!(fresh.int("blank_rows_above"), Some(0));
    let row_members = sorted(&["acct_id", "card_num", "card_ref", "status"]);
    for row in fresh
        .body
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // audit: COCRDLIC-032
        assert_eq!(
            keys(row),
            row_members,
            "a listed row carries no other member"
        );
    }
    // audit: COCRDLIC-032
    assert!(fresh.rows()?.iter().all(|(_, number, _, status)| {
        number.len() == 16 && number.starts_with("************") && status == "Y"
    }));
    let admin = h.get(LIST, &[], &h.admin).await?;
    // audit: COCRDLIC-001
    assert!(
        admin.status == fresh.status && admin.body == fresh.body,
        "the A and U tokens list different cards"
    );
    Ok(fresh)
}

/// Blank, all-space and all-zero filters are no filter (COCRDLIC-005, COCRDLIC-007).
async fn list_blank_filters(h: &Harness, first: &Reply) -> Outcome<()> {
    for blank in ["", "           ", "00000000000"] {
        let reply = h.list(&query(&[("acct_id", blank)])).await?;
        // audit: COCRDLIC-005
        assert!(
            reply.body == first.body,
            "a blank account filter filtered the list"
        );
    }
    for blank in ["", "                ", "0000000000000000"] {
        let reply = h.list(&query(&[("card_num", blank)])).await?;
        // audit: COCRDLIC-007
        assert!(
            reply.body == first.body,
            "a blank card filter filtered the list"
        );
    }
    Ok(())
}

/// ENTER, F8 and F7 over pages 1 to 3, the filter change between pages and the page number
/// leaving 0; returns page 3.
async fn list_paging(h: &Harness, first: &Reply) -> Outcome<Reply> {
    let [page1, page2, page3] = [1, 2, 3].map(|page| page_of(&h.cards, page));
    let enter = h.list(&echo(first, None)).await?;
    // audit: COCRDLIC-018
    assert_page(&enter, &page1, 1, None, Some(RECORD_ACTIONS))?;
    let second = h.list(&echo(first, Some("forward"))).await?;
    // audit: COCRDLIC-023
    assert_page(&second, &page2, 2, None, Some(RECORD_ACTIONS))?;
    // audit: COCRDLIC-022
    assert_eq!(
        second.int("first_key"),
        page2.first().map(|card| card.handle)
    );
    // audit: COCRDLIC-019
    assert_eq!(
        second.int("last_key"),
        page3.first().map(|card| card.handle)
    );
    let again = h.list(&echo(&second, None)).await?;
    // audit: COCRDLIC-018
    assert_page(&again, &page2, 2, None, Some(RECORD_ACTIONS))?;
    // ENTER after a filter change reads from the displayed page's first key: a matching card
    // on that page is listed, one before it is not, and the page number stays.
    let later = h.line(10)?;
    let reply = h
        .list(&with(echo(&second, None), "acct_id", &later.acct))
        .await?;
    // audit: COCRDLIC-018
    assert_page(
        &reply,
        &[later],
        2,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    // audit: COCRDLIC-010
    assert_eq!(reply.int("first_key"), Some(later.handle));
    let earlier = h.line(2)?;
    let reply = h
        .list(&with(echo(&second, None), "acct_id", &earlier.acct))
        .await?;
    // audit: COCRDLIC-021
    assert_page(&reply, &[], 2, Some(NO_MORE_RECORDS), Some(RECORD_ACTIONS))?;
    // audit: COCRDLIC-022
    assert_eq!(reply.int("first_key"), second.int("first_key"));
    let third = h.list(&echo(&second, Some("forward"))).await?;
    assert_page(&third, &page3, 3, None, Some(RECORD_ACTIONS))?;
    let back = h.list(&echo(&third, Some("backward"))).await?;
    // audit: COCRDLIC-027
    assert_page(&back, &page2, 2, None, Some(RECORD_ACTIONS))?;
    // audit: COCRDLIC-027
    assert_eq!(back.int("first_key"), page2.first().map(|card| card.handle));
    // audit: COCRDLIC-027
    assert_eq!(back.int("last_key"), page3.first().map(|card| card.handle));
    // audit: COCRDLIC-027
    assert_eq!(back.flag("has_next_page"), Some(true));
    let top = h.list(&echo(&back, Some("backward"))).await?;
    // audit: COCRDLIC-028
    assert_page(&top, &page1, 1, Some(NO_PREVIOUS_PAGES), None)?;
    // audit: COCRDLIC-028
    assert_eq!(top.int("last_key"), page2.first().map(|card| card.handle));
    let stay = h.list(&echo(&top, Some("backward"))).await?;
    // audit: COCRDLIC-026
    assert_page(&stay, &page1, 1, Some(NO_PREVIOUS_PAGES), None)?;
    // F7 from a page number of 0 stores 1, so the read moves back and the page is the first.
    let zero = h
        .list(&with(echo(&third, Some("backward")), "page_no", "0"))
        .await?;
    // audit: COCRDLIC-030
    assert_page(&zero, &page2, 1, Some(NO_PREVIOUS_PAGES), None)?;
    Ok(third)
}

/// F8 from page 3 to the last page, then F8 and ENTER on it.
async fn list_last_page(h: &Harness, third: &Reply) -> Outcome<()> {
    let mut reply = h.list(&echo(third, Some("forward"))).await?;
    for page in 4..=7 {
        assert_page(
            &reply,
            &page_of(&h.cards, page),
            page_number(page)?,
            None,
            Some(RECORD_ACTIONS),
        )?;
        if page < 7 {
            reply = h.list(&echo(&reply, Some("forward"))).await?;
        }
    }
    let page8 = page_of(&h.cards, 8);
    // audit: COCRDLIC-019
    assert_eq!(reply.flag("has_next_page"), Some(true));
    // audit: COCRDLIC-019
    assert_eq!(reply.int("last_key"), page8.first().map(|card| card.handle));
    let last = h.list(&echo(&reply, Some("forward"))).await?;
    // audit: COCRDLIC-020
    assert_page(
        &last,
        &page8,
        8,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    // audit: COCRDLIC-020
    assert_eq!(last.flag("has_next_page"), Some(false));
    // audit: COCRDLIC-025
    assert_eq!(last.flag("last_page_shown"), Some(true));
    let again = h.list(&echo(&last, Some("forward"))).await?;
    // audit: COCRDLIC-024
    assert_page(&again, &page8, 8, Some(NO_MORE_PAGES), None)?;
    // audit: COCRDLIC-024
    assert_eq!(again.flag("last_page_shown"), Some(true));
    let enter = h.list(&echo(&again, None)).await?;
    // audit: COCRDLIC-025
    assert_eq!(enter.flag("last_page_shown"), Some(false));
    // audit: COCRDLIC-033
    assert_page(
        &enter,
        &page8,
        8,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    Ok(())
}

/// `page` as the page number a response carries.
fn page_number(page: usize) -> Outcome<i64> {
    Ok(i64::try_from(page)?)
}

/// The account and card filters, the precedence of the error line, and a backward read that
/// reaches the start of the card file.
async fn list_filters(h: &Harness, third: &Reply) -> Outcome<()> {
    let own = h.line(1)?;
    let by_acct = h.list(&query(&[("acct_id", &own.acct)])).await?;
    // audit: COCRDLIC-010
    assert_page(
        &by_acct,
        &[own],
        1,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    // audit: COCRDLIC-020
    assert_eq!(by_acct.flag("has_next_page"), Some(false));
    let by_card = h.list(&query(&[("card_num", &own.number)])).await?;
    // audit: COCRDLIC-010
    assert!(
        by_card.body == by_acct.body,
        "the card filter lists another page"
    );
    let both = h
        .list(&query(&[("acct_id", &own.acct), ("card_num", &own.number)]))
        .await?;
    // audit: COCRDLIC-010
    assert!(both.body == by_acct.body, "both filters list another page");
    let other = h.line(49)?;
    let crossed = h
        .list(&query(&[
            ("acct_id", &other.acct),
            ("card_num", &own.number),
        ]))
        .await?;
    // audit: COCRDLIC-010
    assert_page(&crossed, &[], 1, Some(NO_RECORDS_FOUND), None)?;
    let none = h.list(&query(&[("acct_id", "00000000099")])).await?;
    // audit: COCRDLIC-021
    assert_page(&none, &[], 1, Some(NO_RECORDS_FOUND), None)?;
    // audit: COCRDLIC-033
    assert_eq!(none.text("info_message"), None);
    // F7 on page 1 replaces the read's message, the no-records message included.
    let back = h
        .list(&with(
            echo(&by_acct, Some("backward")),
            "acct_id",
            &own.acct,
        ))
        .await?;
    // audit: COCRDLIC-034
    assert_page(&back, &[own], 1, Some(NO_PREVIOUS_PAGES), None)?;
    let back = h
        .list(&with(
            echo(&none, Some("backward")),
            "acct_id",
            "00000000099",
        ))
        .await?;
    // audit: COCRDLIC-034
    assert_page(&back, &[], 1, Some(NO_PREVIOUS_PAGES), None)?;
    // F7 from page 3 under a filter only an earlier card matches: the backward read reaches the
    // start of the file with one card, which fills the last row.
    let start = page_of(&h.cards, 3).first().map(|card| card.handle);
    let early = h.line(2)?;
    let reached = h
        .list(&with(echo(third, Some("backward")), "acct_id", &early.acct))
        .await?;
    // audit: COCRDLIC-029
    assert_eq!(reached.rows()?, rows(&[early]));
    // audit: COCRDLIC-029
    assert_eq!(reached.int("blank_rows_above"), Some(6));
    // audit: COCRDLIC-029
    assert_eq!(
        (reached.int("first_key"), reached.int("last_key")),
        (start, start)
    );
    // audit: COCRDLIC-029
    assert_eq!(
        (reached.flag("has_next_page"), reached.int("page_no")),
        (Some(true), Some(2))
    );
    // audit: COCRDLIC-031
    assert_eq!(
        reached.text("message").and_then(|text| text.get(..48)),
        Some(FILE_ERROR_HEAD)
    );
    // audit: COCRDLIC-031
    assert_eq!(
        reached.text("message").and_then(|text| text.get(58..65)),
        Some(FILE_ERROR_RESP2)
    );
    Ok(())
}

/// The map lengths of the two filters, and a card read that fails in the store.
async fn list_lengths_and_store(h: &Harness) -> Outcome<()> {
    let long = h.list(&query(&[("acct_id", "000000000050")])).await?;
    // audit: COCRDLI-015
    assert_error(
        &long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("acct_id"),
    );
    let long = h.list(&query(&[("card_num", "12345678901234567")])).await?;
    // audit: COCRDLI-018
    assert_error(
        &long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("card_num"),
    );
    let failed = h.get_failing(LIST, &[]).await?;
    // audit: COCRDLIC-031
    assert_file_error(&failed, None);
    Ok(())
}

/// Endpoint 7 over cards the fixture cannot provide, inserted through the card repository:
/// several cards per account, a filtered page whose look-ahead finds a card the filter
/// excludes, a full last page and more than 63 cards for the page-number wrap.
#[tokio::test]
async fn cocrdlic_inserted_cards_rows() -> TestResult {
    let mut h = Harness::start().await?;
    let own = h.line(1)?.clone();
    let following = h.line(2)?.clone();
    // Six more cards of line 1's account, numbered right after line 1 and before line 2.
    let base = own.number.parse::<u64>()?;
    let numbers: Vec<String> = (1..=6).map(|n| format!("{:016}", base + n)).collect();
    assert!(
        numbers.iter().all(|number| *number < following.number),
        "an inserted card does not sort between lines 1 and 2"
    );
    let group = insert_cards(&mut h, &numbers, &own.acct).await?;
    let mut matching: Vec<&Card> = vec![&own];
    matching.extend(group.iter());
    let filtered = h.list(&query(&[("acct_id", &own.acct)])).await?;
    // audit: COCRDLIC-010
    assert_page(&filtered, &matching, 1, None, Some(RECORD_ACTIONS))?;
    // audit: COCRDLIC-019
    assert_eq!(filtered.int("last_key"), Some(following.handle));
    // audit: COCRDLIC-019
    assert_eq!(filtered.flag("has_next_page"), Some(true));
    let empty = h
        .list(&with(
            echo(&filtered, Some("forward")),
            "acct_id",
            &own.acct,
        ))
        .await?;
    // audit: COCRDLIC-021
    assert_page(&empty, &[], 2, Some(NO_MORE_RECORDS), Some(RECORD_ACTIONS))?;
    // audit: COCRDLIC-022
    assert_eq!(empty.int("first_key"), Some(own.handle));
    // audit: COCRDLIC-025
    assert_eq!(empty.flag("last_page_shown"), Some(true));
    let mut all = h.cards.clone();
    all.extend(group);
    all.sort_by(|left, right| left.number.cmp(&right.number));
    let last = walk_forward(&h, &all, 8).await?;
    // A full last page: the look-ahead finds no card.
    // audit: COCRDLIC-019
    assert_eq!(last.flag("has_next_page"), Some(false));
    // audit: COCRDLIC-019
    assert_eq!(
        last.int("last_key"),
        page_of(&all, 8).last().map(|card| card.handle)
    );
    // audit: COCRDLIC-019
    assert_page(
        &last,
        &page_of(&all, 8),
        8,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    inserted_wrap_rows(&mut h, all).await
}

/// Eight more cards of line 50's account, above every fixture number: paging under a filter
/// both ways, then the page number wrapping from 9 to 0 over 64 cards.
async fn inserted_wrap_rows(h: &mut Harness, mut all: Vec<Card>) -> Outcome<()> {
    let top = h.line(50)?.clone();
    let numbers: Vec<String> = (1..=8).map(|n| format!("99000000000000{n:02}")).collect();
    assert!(
        numbers.iter().all(|number| *number > top.number),
        "an inserted card does not sort after line 50"
    );
    let group = insert_cards(h, &numbers, &top.acct).await?;
    let mut matching: Vec<&Card> = vec![&top];
    matching.extend(group.iter());
    let (head, tail) = matching.split_at(PAGE_ROWS);
    let filtered = h.list(&query(&[("acct_id", &top.acct)])).await?;
    // audit: COCRDLIC-010
    assert_page(&filtered, head, 1, None, Some(RECORD_ACTIONS))?;
    let forward = h
        .list(&with(
            echo(&filtered, Some("forward")),
            "acct_id",
            &top.acct,
        ))
        .await?;
    // audit: COCRDLIC-010
    assert_page(
        &forward,
        tail,
        2,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    let backward = h
        .list(&with(
            echo(&forward, Some("backward")),
            "acct_id",
            &top.acct,
        ))
        .await?;
    // audit: COCRDLIC-010
    assert_page(&backward, head, 1, Some(NO_PREVIOUS_PAGES), None)?;
    // audit: COCRDLIC-027
    assert_eq!(
        backward.int("last_key"),
        tail.first().map(|card| card.handle)
    );
    all.extend(group);
    all.sort_by(|left, right| left.number.cmp(&right.number));
    let ninth = walk_forward(h, &all, 9).await?;
    assert_eq!(ninth.flag("has_next_page"), Some(true));
    let wrapped = h.list(&echo(&ninth, Some("forward"))).await?;
    // audit: COCRDLIC-030
    assert_page(
        &wrapped,
        &page_of(&all, 10),
        1,
        Some(NO_MORE_RECORDS),
        Some(RECORD_ACTIONS),
    )?;
    // audit: COCRDLIC-022
    assert_eq!(
        wrapped.int("first_key"),
        page_of(&all, 10).first().map(|card| card.handle)
    );
    Ok(())
}

/// The unfiltered list from the fresh start to page `pages` by F8, each page checked against
/// `all`; returns the last page.
async fn walk_forward(h: &Harness, all: &[Card], pages: usize) -> Outcome<Reply> {
    let mut reply = h.list(&[]).await?;
    for page in 1..=pages {
        if page > 1 {
            reply = h.list(&echo(&reply, Some("forward"))).await?;
        }
        assert_eq!(reply.rows()?, rows(&page_of(all, page)), "page {page}");
        assert_eq!(reply.int("page_no"), Some(page_number(page)?));
    }
    Ok(reply)
}

/// Inserts one card of account `acct` per number of `numbers` through the card repository and
/// returns them with their handles; each number joins the redaction needles.
async fn insert_cards(h: &mut Harness, numbers: &[String], acct: &str) -> Outcome<Vec<Card>> {
    let mut store = h.store.clone();
    let mut inserted = Vec::new();
    for number in numbers {
        let record = CardRecord {
            num: CardNumber::parse(number)?,
            acct_id: PicU::new(acct.parse::<u64>()?)?,
            cvv_cd: Cvv::new(PicU::new(123)?),
            embossed_name: PicX::from_text("Test Holder")?,
            expiraion_date: PicDate::new(NaiveDate::from_ymd_opt(2030, 1, 31))?,
            active_status: PicX::from_text("Y")?,
            filler: PicX::from_text("")?,
        };
        CardRepository::insert(&mut store, &record).await?;
        let (handle, _) = CardRepository::find_by_id(&mut store, &record.num)
            .await?
            .ok_or("an inserted card cannot be read back")?;
        h.needles.push(number.clone());
        inserted.push(Card {
            line: 0,
            number: number.clone(),
            redacted: redacted(number)?,
            acct: acct.to_owned(),
            name: format!("{:<50}", "Test Holder"),
            expiry: "2030-01-31".to_owned(),
            status: "Y".to_owned(),
            handle: handle.get(),
        });
    }
    Ok(inserted)
}

/// Endpoint 8: the found card for both tokens, by number and by handle, the edits, the
/// not-found and store-failure answers, the map lengths and the texts no response carries.
#[tokio::test]
async fn cocrdslc_view_rows() -> TestResult {
    let h = Harness::start().await?;
    view_found(&h).await?;
    view_by_handle_and_account(&h).await?;
    view_edits(&h).await?;
    view_not_found_and_store(&h).await?;
    for text in NEVER_SENT_BY_VIEW {
        let carried = h
            .seen
            .borrow()
            .iter()
            .filter(|body| body.contains(text))
            .count();
        // audit: COCRDSLC-028
        // audit: COCRDSLC-029
        // audit: COCRDSLC-030
        // audit: COCRDSLC-031
        assert_eq!(carried, 0, "a response carried {text:?}");
    }
    Ok(())
}

/// A found card: its fields for a `U` token and the full number for an `A` token.
async fn view_found(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let user = h
        .get(VIEW, &pair(&card.acct, &card.number), &h.user)
        .await?;
    assert_eq!(user.status, StatusCode::OK);
    // audit: COCRDSLC-018
    assert_eq!(user.text("acct_id"), Some(card.acct.as_str()));
    // audit: COCRDSLC-018
    assert_eq!(user.int("card_ref"), Some(card.handle));
    // audit: COCRDSLC-018
    assert_eq!(user.text("message"), None);
    // audit: COCRDSLC-023
    assert_eq!(user.text("card_num"), Some(card.redacted.as_str()));
    // audit: COCRDSLC-019
    assert_eq!(user.text("embossed_name"), Some(card.name.as_str()));
    // audit: COCRDSLC-019
    assert_eq!(user.text("expiration_date"), Some(card.expiry.as_str()));
    // audit: COCRDSLC-019
    assert_eq!(user.text("status"), Some(card.status.as_str()));
    let members = sorted(&[
        "acct_id",
        "card_ref",
        "card_num",
        "embossed_name",
        "expiration_date",
        "info_message",
        "message",
        "status",
    ]);
    // audit: COCRDSLC-019
    assert_eq!(keys(&user.body), members);
    // audit: COCRDSLC-020
    assert_eq!(user.text("info_message"), Some(DISPLAYING_DETAILS));
    // audit: COCRDSLC-021
    assert!(!has_cvv_key(&user.body), "the card view names a CVV");
    let admin = h
        .get_disclosed(VIEW, &pair(&card.acct, &card.number), &card.number)
        .await?;
    // audit: COCRDSLC-022
    assert!(
        admin.text("card_num") == Some(card.number.as_str()),
        "an A token is not shown the full card number"
    );
    // audit: COCRDSLC-022
    assert!(
        without_card_num(&admin.body) == without_card_num(&user.body),
        "the A token sees other fields than the U token"
    );
    Ok(())
}

/// The same card by the handle of a list row, and a card shown under another account.
async fn view_by_handle_and_account(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let user = h
        .get(VIEW, &pair(&card.acct, &card.number), &h.user)
        .await?;
    let admin = h
        .get_disclosed(VIEW, &pair(&card.acct, &card.number), &card.number)
        .await?;
    let listed = h.list(&query(&[("card_num", &card.number)])).await?;
    let handle = listed
        .rows()?
        .first()
        .map(|row| row.0.to_string())
        .ok_or("the card list lists no row")?;
    let by_handle = h.get(VIEW, &by_ref(&card.acct, &handle), &h.user).await?;
    // audit: COCRDSLC-006
    assert!(by_handle.body == user.body, "the handle reads another card");
    let disclosed = h
        .get_disclosed(VIEW, &by_ref(&card.acct, &handle), &card.number)
        .await?;
    // audit: COCRDSLC-006
    assert!(
        disclosed.body == admin.body,
        "the handle reads another card for an A token"
    );
    // The read takes the card number alone: another account's card is found and shown under
    // the account entered.
    let other = h.line(49)?;
    let crossed = h
        .get(VIEW, &pair(&other.acct, &card.number), &h.user)
        .await?;
    // audit: COCRDSLC-014
    assert_eq!(crossed.status, StatusCode::OK);
    // audit: COCRDSLC-014
    assert_eq!(crossed.text("acct_id"), Some(other.acct.as_str()));
    // audit: COCRDSLC-014
    assert_eq!(crossed.int("card_ref"), Some(card.handle));
    Ok(())
}

/// `body` without its `card_num` member.
fn without_card_num(body: &Value) -> Value {
    let mut rest = body.clone();
    if let Some(map) = rest.as_object_mut() {
        map.remove("card_num");
    }
    rest
}

/// COCRDSLC's edits and the two map lengths: each answer's status, message and field.
async fn view_edits(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let (acct, number) = (card.acct.as_str(), card.number.as_str());
    let handle = card.handle_text();
    let with_both = with(pair(acct, number), "card_ref", &handle);
    let cases: [(Query, &str, &str); 15] = [
        // audit: COCRDSLC-008
        (Query::new(), NO_INPUT, "acct_id"),
        // audit: COCRDSLC-008
        (pair("*", "*"), NO_INPUT, "acct_id"),
        // audit: COCRDSLC-008
        (pair("00000000000", "0000000000000000"), NO_INPUT, "acct_id"),
        // audit: COCRDSLC-009
        (query(&[("card_num", number)]), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDSLC-009
        (by_ref("           ", &handle), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDSLC-028
        (pair("00000000000", number), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDSLC-010
        (query(&[("acct_id", acct)]), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDSLC-010
        (pair(acct, "*"), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDSLC-010
        (pair(acct, "                "), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDSLC-010
        (pair(acct, "0000000000000000"), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDSLC-007
        (with_both.clone(), CARD_FILTER, "card_num"),
        // audit: COCRDSLC-007
        (with(with_both, "acct_id", ""), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDSLC-013
        (pair(acct, ""), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDSL-013
        (
            pair("000000000050", number),
            FIELD_LENGTH_MESSAGE,
            "acct_id",
        ),
        // audit: COCRDSL-016
        (
            pair(acct, "12345678901234567"),
            FIELD_LENGTH_MESSAGE,
            "card_num",
        ),
    ];
    for (sent, message, field) in &cases {
        let reply = h.get(VIEW, sent, &h.user).await?;
        assert_error(&reply, StatusCode::BAD_REQUEST, message, Some(*field));
    }
    Ok(())
}

/// No card with the number or the handle, and a card read that fails in the store.
async fn view_not_found_and_store(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let missing = h
        .get(VIEW, &pair(&card.acct, "1234567890123456"), &h.user)
        .await?;
    // audit: COCRDSLC-015
    assert_error(&missing, StatusCode::NOT_FOUND, NOT_FOUND, Some("acct_id"));
    let unknown = h.get(VIEW, &by_ref(&card.acct, "999999"), &h.user).await?;
    // audit: COCRDSLC-006
    assert_error(&unknown, StatusCode::NOT_FOUND, NOT_FOUND, Some("acct_id"));
    // audit: COCRDSLC-013
    assert_eq!(unknown.text("field"), Some("acct_id"));
    let failed = h.get_failing(VIEW, &pair(&card.acct, &card.number)).await?;
    // audit: COCRDSLC-016
    assert_file_error(&failed, Some("acct_id"));
    Ok(())
}

/// The four detail fields of an endpoint 10 request, as typed.
#[derive(Clone, Copy)]
struct Fields<'a> {
    name: &'a str,
    status: &'a str,
    month: &'a str,
    year: &'a str,
}

impl<'a> Fields<'a> {
    /// The endpoint 10 body with key `aid` and the echoed `snapshot`.
    fn body(self, aid: &str, snapshot: &Value) -> Value {
        json!({
            "aid": aid,
            "fields": {
                "embossed_name": self.name,
                "status": self.status,
                "expiry_month": self.month,
                "expiry_year": self.year,
            },
            "snapshot": snapshot,
        })
    }

    fn with_name(self, name: &'a str) -> Self {
        Self { name, ..self }
    }

    fn with_status(self, status: &'a str) -> Self {
        Self { status, ..self }
    }

    fn with_month(self, month: &'a str) -> Self {
        Self { month, ..self }
    }

    fn with_year(self, year: &'a str) -> Self {
        Self { year, ..self }
    }
}

/// Endpoints 9 and 10, in the order the screen uses them: the search edits, the fetch, the
/// change check and the detail edits, the confirmation and the save, the conflicts and failures
/// of a save, the account a save writes, the map lengths and a failed read.
#[tokio::test]
async fn cocrdupc_update_rows() -> TestResult {
    let h = Harness::start().await?;
    update_search_edits(&h).await?;
    let view = update_fetch(&h).await?;
    update_no_change(&h, &view).await?;
    update_detail_edits(&h, &view).await?;
    let validated = update_confirm(&h, &view).await?;
    update_save(&h, &validated).await?;
    update_conflicts(&h, &validated).await?;
    update_failures(&h).await?;
    update_account_move(&h).await?;
    update_lengths_and_store(&h).await?;
    Ok(())
}

/// The search-key edits of endpoint 9: each a 400 with no read (COCRDUPC-015).
async fn update_search_edits(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let (acct, number) = (card.acct.as_str(), card.number.as_str());
    let cases: [(Query, &str, &str); 13] = [
        // audit: COCRDUPC-014
        (Query::new(), NO_INPUT, "acct_id"),
        // audit: COCRDUPC-009
        (pair("*", "*         "), NO_INPUT, "acct_id"),
        // audit: COCRDUPC-010
        (query(&[("card_num", number)]), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDUPC-010
        (pair("00000000000", number), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDUPC-009
        (pair("           ", number), PROMPT_FOR_ACCT, "acct_id"),
        // audit: COCRDUPC-011
        (pair("1234567890A", number), ACCOUNT_FILTER, "acct_id"),
        // audit: COCRDUPC-011
        (pair("12345", number), ACCOUNT_FILTER, "acct_id"),
        // audit: COCRDUPC-012
        (query(&[("acct_id", acct)]), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDUPC-012
        (pair(acct, "0000000000000000"), PROMPT_FOR_CARD, "card_num"),
        // audit: COCRDUPC-012
        (
            query(&[("acct_id", "1234567890A")]),
            ACCOUNT_FILTER,
            "acct_id",
        ),
        // audit: COCRDUPC-013
        (pair(acct, "12345678901234AB"), CARD_FILTER, "card_num"),
        // audit: COCRDUPC-013
        (pair(acct, "123"), CARD_FILTER, "card_num"),
        // audit: COCRDUPC-015
        (pair("", ""), NO_INPUT, "acct_id"),
    ];
    for (sent, message, field) in &cases {
        let reply = h.get(EDIT, sent, &h.user).await?;
        assert_error(&reply, StatusCode::BAD_REQUEST, message, Some(*field));
    }
    Ok(())
}

/// The list entry by handle (line 1), the fetched view and its snapshot, and no card for a
/// handle or a number; returns the view.
async fn update_fetch(h: &Harness) -> Outcome<Reply> {
    let card = h.line(1)?;
    let view = h
        .get(EDIT, &by_ref(&card.acct, &card.handle_text()), &h.user)
        .await?;
    // audit: COCRDUPC-002
    assert_eq!(view.status, StatusCode::OK);
    // audit: COCRDUPC-002
    assert_eq!(view.text("info_message"), Some(DETAILS_SHOWN));
    // audit: COCRDUPC-002
    assert_eq!(view.text("message"), None);
    // audit: COCRDUPC-002
    assert_eq!(view.int("card_ref"), Some(card.handle));
    let upper = card.name.to_ascii_uppercase();
    // audit: COCRDUPC-017
    assert_eq!(view.text("embossed_name"), Some(upper.as_str()));
    // audit: COCRDUPC-017
    assert_eq!(view.text("expiration_date"), Some(card.expiry.as_str()));
    // audit: COCRDUPC-017
    assert_eq!(view.text("status"), Some(card.status.as_str()));
    // audit: COCRDUPC-042
    assert_eq!(view.text("card_num"), Some(card.redacted.as_str()));
    // audit: COCRDUPC-042
    assert!(!has_cvv_key(&view.body), "the card update view names a CVV");
    let snapshot = view.body.get("snapshot").cloned().unwrap_or_default();
    let members = sorted(&[
        "acct_id",
        "embossed_name",
        "expiration_date",
        "state",
        "status",
    ]);
    // audit: COCRDUPC-044
    assert_eq!(keys(&snapshot), members);
    let expected = json!({
        "state": "show_details",
        "acct_id": card.acct,
        "embossed_name": upper,
        "expiration_date": card.expiry,
        "status": card.status,
    });
    // audit: COCRDUPC-044
    assert_eq!(snapshot, expected);
    let unknown = h.get(EDIT, &by_ref(&card.acct, "999999"), &h.user).await?;
    // audit: COCRDUPC-003
    assert_error(&unknown, StatusCode::NOT_FOUND, NOT_FOUND, Some("acct_id"));
    let missing = h
        .get(EDIT, &pair(&card.acct, "1234567890123456"), &h.user)
        .await?;
    // audit: COCRDUPC-018
    assert_error(&missing, StatusCode::NOT_FOUND, NOT_FOUND, Some("acct_id"));
    Ok(view)
}

/// The fields of line 1's card as the screen shows them after the fetch: the upper-cased name,
/// the status, and the month and year of `2023-03-09`.
const SHOWN: Fields<'static> = Fields {
    name: "ANIYA VON",
    status: "Y",
    month: "03",
    year: "2023",
};

/// Fifty `0` digits, a name the edit treats as not supplied (COCRDUPC-024).
const ZERO_NAME: &str = "00000000000000000000000000000000000000000000000000";

/// ENTER with the fetched values, in states `S` and `E`, by letter case alone included.
async fn update_no_change(h: &Harness, view: &Reply) -> Outcome<()> {
    let handle = h.line(1)?.handle_text();
    let shown = view.snapshot_in("show_details")?;
    let not_ok = view.snapshot_in("changes_not_ok")?;
    let lower = SHOWN.with_name("aniya von").with_status("y");
    let cases = [
        // audit: COCRDUPC-020
        (SHOWN, &shown),
        // audit: COCRDUPC-021
        (SHOWN, &not_ok),
        // audit: COCRDUPC-022
        (lower, &shown),
        // audit: COCRDUPC-022
        (lower, &not_ok),
    ];
    for (fields, snapshot) in cases {
        let reply = h.put(&handle, &fields.body("ENTER", snapshot)).await?;
        assert_error(
            &reply,
            StatusCode::BAD_REQUEST,
            NO_CHANGE,
            Some("embossed_name"),
        );
    }
    Ok(())
}

/// The four detail edits under ENTER in states `S` and `E`; nothing is written.
async fn update_detail_edits(h: &Harness, view: &Reply) -> Outcome<()> {
    let card = h.line(1)?;
    let handle = card.handle_text();
    let before = h.stored(card.handle).await?;
    let failing = SHOWN.with_status("Q").with_month("13").with_year("1900");
    let cases: [(Fields<'_>, &str, &str); 17] = [
        // audit: COCRDUPC-024
        (SHOWN.with_name(""), PROMPT_FOR_NAME, "embossed_name"),
        // audit: COCRDUPC-024
        (SHOWN.with_name(ZERO_NAME), PROMPT_FOR_NAME, "embossed_name"),
        // audit: COCRDUPC-009
        (SHOWN.with_name("*"), PROMPT_FOR_NAME, "embossed_name"),
        // audit: COCRDUPC-025
        (SHOWN.with_name("Aniya Von2"), NAME_ALPHA, "embossed_name"),
        // audit: COCRDUPC-025
        (SHOWN.with_name("Aniya-Von"), NAME_ALPHA, "embossed_name"),
        // audit: COCRDUPC-023
        (failing.with_name("X1"), NAME_ALPHA, "embossed_name"),
        // audit: COCRDUPC-023
        (failing, STATUS_YES_NO, "status"),
        // audit: COCRDUPC-027
        (SHOWN.with_status(""), STATUS_YES_NO, "status"),
        // audit: COCRDUPC-027
        (SHOWN.with_status("0"), STATUS_YES_NO, "status"),
        // audit: COCRDUPC-028
        (
            SHOWN.with_name("ANIYA WARD").with_status("y"),
            STATUS_YES_NO,
            "status",
        ),
        // audit: COCRDUPC-029
        (SHOWN.with_month(""), MONTH_INVALID, "expiry_month"),
        // audit: COCRDUPC-029
        (SHOWN.with_month("00"), MONTH_INVALID, "expiry_month"),
        // audit: COCRDUPC-030
        (SHOWN.with_month("13"), MONTH_INVALID, "expiry_month"),
        // audit: COCRDUPC-031
        (SHOWN.with_year(""), YEAR_INVALID, "expiry_year"),
        // audit: COCRDUPC-031
        (SHOWN.with_year("0000"), YEAR_INVALID, "expiry_year"),
        // audit: COCRDUPC-032
        (SHOWN.with_year("1949"), YEAR_INVALID, "expiry_year"),
        // audit: COCRDUPC-032
        (SHOWN.with_year("2100"), YEAR_INVALID, "expiry_year"),
    ];
    for state in ["show_details", "changes_not_ok"] {
        let snapshot = view.snapshot_in(state)?;
        for (fields, message, field) in cases {
            let reply = h.put(&handle, &fields.body("ENTER", &snapshot)).await?;
            assert_error(&reply, StatusCode::BAD_REQUEST, message, Some(field));
        }
    }
    // audit: COCRDUPC-023
    assert!(
        h.stored(card.handle).await? == before,
        "an edit answer wrote the card"
    );
    // The stored name of line 34 holds an apostrophe, so a change to its status alone fails
    // the name edit.
    let quoted = h.line(34)?;
    let quoted_ref = quoted.handle_text();
    let quoted_view = h
        .get(EDIT, &by_ref(&quoted.acct, &quoted_ref), &h.user)
        .await?;
    let name = quoted.name.trim_end().to_ascii_uppercase();
    let status_only = SHOWN
        .with_name(&name)
        .with_status("N")
        .with_month("06")
        .with_year("2025");
    let snapshot = quoted_view.snapshot_in("show_details")?;
    let reply = h
        .put(&quoted_ref, &status_only.body("ENTER", &snapshot))
        .await?;
    // audit: COCRDUPC-026
    assert_error(
        &reply,
        StatusCode::BAD_REQUEST,
        NAME_ALPHA,
        Some("embossed_name"),
    );
    Ok(())
}

/// The change saved to line 1's card: a mixed-case name, a new status and the upper bounds of
/// the month and year edits.
const CHANGE: Fields<'static> = Fields {
    name: "Aniya Ward",
    status: "N",
    month: "12",
    year: "2099",
};

/// Asserts the 200 of a validated change (COCRDUPC-034, COCRDUPC-035).
fn assert_validated(reply: &Reply) {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "a valid change was not validated"
    );
    assert_eq!(reply.text("info_message"), Some(CONFIRM_PROMPT));
    assert_eq!(reply.text("state"), Some("changes_ok_not_confirmed"));
    assert_eq!(reply.flag("save_enabled"), Some(true));
    assert_eq!(reply.flag("committed"), Some(false));
    let state = reply
        .body
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("state"));
    assert_eq!(
        state.and_then(Value::as_str),
        Some("changes_ok_not_confirmed")
    );
}

/// ENTER with a valid change, ENTER again in state `N`, and F5 outside state `N` or with
/// fields that fail; nothing is written. Returns the validated response.
async fn update_confirm(h: &Harness, view: &Reply) -> Outcome<Reply> {
    let card = h.line(1)?;
    let handle = card.handle_text();
    let before = h.stored(card.handle).await?;
    let shown = view.snapshot_in("show_details")?;
    let validated = h.put(&handle, &CHANGE.body("ENTER", &shown)).await?;
    // audit: COCRDUPC-034
    assert_validated(&validated);
    let lower_bounds = SHOWN.with_month("01").with_year("1950");
    let reply = h.put(&handle, &lower_bounds.body("ENTER", &shown)).await?;
    // audit: COCRDUPC-030
    // audit: COCRDUPC-032
    assert_validated(&reply);
    let confirmed = validated.snapshot_in("changes_ok_not_confirmed")?;
    let reply = h.put(&handle, &CHANGE.body("ENTER", &confirmed)).await?;
    // audit: COCRDUPC-035
    assert_validated(&reply);
    let reply = h.put(&handle, &CHANGE.body("PF5", &shown)).await?;
    // audit: COCRDUPC-007
    assert_validated(&reply);
    let unchanged = h.put(&handle, &SHOWN.body("PF5", &confirmed)).await?;
    // audit: COCRDUPC-041
    assert_error(
        &unchanged,
        StatusCode::BAD_REQUEST,
        NO_CHANGE,
        Some("embossed_name"),
    );
    let failing = CHANGE.with_status("Q");
    let reply = h.put(&handle, &failing.body("PF5", &confirmed)).await?;
    // audit: COCRDUPC-041
    assert_error(
        &reply,
        StatusCode::BAD_REQUEST,
        STATUS_YES_NO,
        Some("status"),
    );
    // audit: COCRDUPC-034
    assert!(
        h.stored(card.handle).await? == before,
        "a validation wrote the card"
    );
    Ok(validated)
}

/// F5 in state `N` with the validated change: the card rewritten as received, its number kept,
/// its day kept from the snapshot, and no other card written.
async fn update_save(h: &Harness, validated: &Reply) -> Outcome<()> {
    let card = h.line(1)?;
    let before = h.stored(card.handle).await?;
    let neighbour = h.line(2)?;
    let neighbour_before = h.stored(neighbour.handle).await?;
    let confirmed = validated.snapshot_in("changes_ok_not_confirmed")?;
    let saved = h
        .put(&card.handle_text(), &CHANGE.body("PF5", &confirmed))
        .await?;
    // audit: COCRDUPC-039
    assert_eq!(saved.status, StatusCode::OK);
    // audit: COCRDUPC-039
    assert_eq!(saved.text("info_message"), Some(COMMITTED));
    // audit: COCRDUPC-039
    assert_eq!(saved.text("state"), Some("changes_okayed_and_done"));
    // audit: COCRDUPC-039
    assert_eq!(saved.flag("committed"), Some(true));
    // audit: COCRDUPC-039
    assert_eq!(saved.flag("save_enabled"), Some(false));
    let after = h.stored(card.handle).await?;
    // audit: COCRDUPC-038
    assert!(after.num == before.num, "the save changed the card number");
    // audit: COCRDUPC-038
    assert_eq!(after.acct_id, before.acct_id);
    // audit: COCRDUPC-038
    assert_eq!(after.embossed_name, PicX::from_text(CHANGE.name)?);
    // audit: COCRDUPC-038
    assert_eq!(after.active_status, PicX::from_text(CHANGE.status)?);
    // The day is the snapshot's: 2023-03-09 saved with month 12 and year 2099.
    // audit: COCRDUPC-009
    assert_eq!(
        after.expiraion_date.value(),
        NaiveDate::from_ymd_opt(2099, 12, 9)
    );
    // audit: COCRDUPC-043
    assert!(
        h.stored(neighbour.handle).await? == neighbour_before,
        "the save wrote another card"
    );
    Ok(())
}

/// A save against a card changed since its fetch, and a change of letter case alone, which the
/// comparison does not detect.
async fn update_conflicts(h: &Harness, validated: &Reply) -> Outcome<()> {
    let card = h.line(1)?;
    let first_save = h.stored(card.handle).await?;
    let stale = validated.snapshot_in("changes_ok_not_confirmed")?;
    let again = CHANGE.with_name("Aniya Vonn");
    let conflict = h
        .put(&card.handle_text(), &again.body("PF5", &stale))
        .await?;
    // audit: COCRDUPC-037
    assert_error(&conflict, StatusCode::CONFLICT, CHANGED_BY_OTHER, None);
    // audit: COCRDUPC-037
    assert!(
        h.stored(card.handle).await? == first_save,
        "a conflicting save wrote the card"
    );
    // Another user lowers the case of line 2's name after the fetch: not detected.
    let other = h.line(2)?;
    let other_ref = other.handle_text();
    let view = h
        .get(EDIT, &by_ref(&other.acct, &other_ref), &h.user)
        .await?;
    let mut record = h.stored(other.handle).await?;
    record.embossed_name = PicX::from_text(&other.name.trim_end().to_ascii_lowercase())?;
    let mut store = h.store.clone();
    let rewritten = CardRepository::update(&mut store, &record).await?;
    assert!(rewritten, "the other user's change was not stored");
    let upper = other.name.trim_end().to_ascii_uppercase();
    let fields = SHOWN
        .with_name(&upper)
        .with_status("N")
        .with_month("07")
        .with_year("2025");
    let shown = view.snapshot_in("show_details")?;
    let checked = h.put(&other_ref, &fields.body("ENTER", &shown)).await?;
    assert_validated(&checked);
    let confirmed = checked.snapshot_in("changes_ok_not_confirmed")?;
    let saved = h.put(&other_ref, &fields.body("PF5", &confirmed)).await?;
    // audit: COCRDUPC-037
    assert_eq!(saved.text("state"), Some("changes_okayed_and_done"));
    // The name left as fetched is written upper-cased.
    // audit: COCRDUPC-038
    assert_eq!(
        h.stored(other.handle).await?.embossed_name,
        PicX::from_text(&upper)?
    );
    Ok(())
}

/// A save whose card cannot be read for update, and a rewrite that fails.
async fn update_failures(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let view = h
        .get(EDIT, &by_ref(&card.acct, &card.handle_text()), &h.user)
        .await?;
    let current = h.stored(card.handle).await?;
    let change = CHANGE.with_status("Y").with_month("11").with_year("2030");
    let confirmed = view.snapshot_in("changes_ok_not_confirmed")?;
    for target in ["999999", "not-a-handle"] {
        let reply = h.put(target, &change.body("PF5", &confirmed)).await?;
        // audit: COCRDUPC-036
        assert_error(&reply, StatusCode::INTERNAL_SERVER_ERROR, LOCK_FAILED, None);
    }
    // audit: COCRDUPC-036
    assert!(
        h.stored(card.handle).await? == current,
        "a failed lock wrote the card"
    );
    // Line 35 expires on day 29, so the month 02 of 2025 that the edits accept makes no calendar
    // date, which the typed column refuses: the rewrite fails and nothing is written.
    let leap = h.line(35)?;
    let leap_ref = leap.handle_text();
    let leap_view = h.get(EDIT, &by_ref(&leap.acct, &leap_ref), &h.user).await?;
    let leap_before = h.stored(leap.handle).await?;
    let upper = leap.name.trim_end().to_ascii_uppercase();
    let february = SHOWN.with_name(&upper).with_month("02").with_year("2025");
    let shown = leap_view.snapshot_in("show_details")?;
    let checked = h.put(&leap_ref, &february.body("ENTER", &shown)).await?;
    assert_validated(&checked);
    let confirmed = checked.snapshot_in("changes_ok_not_confirmed")?;
    let failed = h.put(&leap_ref, &february.body("PF5", &confirmed)).await?;
    // audit: COCRDUPC-040
    assert_error(
        &failed,
        StatusCode::INTERNAL_SERVER_ERROR,
        UPDATE_FAILED,
        None,
    );
    // audit: COCRDUPC-040
    assert!(
        h.stored(leap.handle).await? == leap_before,
        "a failed rewrite wrote the card"
    );
    Ok(())
}

/// A card searched by number under another account: shown under the account entered, and the
/// save moves it to that account.
async fn update_account_move(h: &Harness) -> Outcome<()> {
    let card = h.line(3)?;
    let handle = card.handle_text();
    let entered = h.line(49)?.acct.as_str();
    let view = h.get(EDIT, &pair(entered, &card.number), &h.user).await?;
    // audit: COCRDUPC-016
    assert_eq!(view.status, StatusCode::OK);
    // audit: COCRDUPC-016
    assert_eq!(view.text("acct_id"), Some(entered));
    // audit: COCRDUPC-016
    assert_eq!(view.int("card_ref"), Some(card.handle));
    // audit: COCRDUPC-016
    assert_eq!(view.text("info_message"), Some(DETAILS_SHOWN));
    let shown = view.snapshot_in("show_details")?;
    // audit: COCRDUPC-044
    assert_eq!(shown.get("acct_id").and_then(Value::as_str), Some(entered));
    let before = h.stored(card.handle).await?;
    let upper = card.name.trim_end().to_ascii_uppercase();
    let fields = SHOWN
        .with_name(&upper)
        .with_status("N")
        .with_month("08")
        .with_year("2024");
    let checked = h.put(&handle, &fields.body("ENTER", &shown)).await?;
    assert_validated(&checked);
    let confirmed = checked.snapshot_in("changes_ok_not_confirmed")?;
    let saved = h.put(&handle, &fields.body("PF5", &confirmed)).await?;
    assert_eq!(saved.text("state"), Some("changes_okayed_and_done"));
    let after = h.stored(card.handle).await?;
    // audit: COCRDUPC-038
    assert_eq!(after.acct_id, PicU::new(entered.parse::<u64>()?)?);
    // audit: COCRDUPC-038
    assert!(after.num == before.num, "the save changed the card number");
    Ok(())
}

/// The map lengths of endpoints 9 and 10, and a card read that fails in the store on the
/// search and on the list entry.
async fn update_lengths_and_store(h: &Harness) -> Outcome<()> {
    let card = h.line(1)?;
    let handle = card.handle_text();
    let long = h
        .get(EDIT, &pair("000000000050", &card.number), &h.user)
        .await?;
    // audit: COCRDUP-013
    assert_error(
        &long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("acct_id"),
    );
    let long = h
        .get(EDIT, &pair(&card.acct, "12345678901234567"), &h.user)
        .await?;
    // audit: COCRDUP-016
    assert_error(
        &long,
        StatusCode::BAD_REQUEST,
        FIELD_LENGTH_MESSAGE,
        Some("card_num"),
    );
    let view = h.get(EDIT, &by_ref(&card.acct, &handle), &h.user).await?;
    let shown = view.snapshot_in("show_details")?;
    let long_name = "A".repeat(51);
    let cases = [
        // audit: COCRDUP-019
        (SHOWN.with_name(&long_name), "fields.embossed_name"),
        // audit: COCRDUP-022
        (SHOWN.with_status("YN"), "fields.status"),
        // audit: COCRDUP-025
        (SHOWN.with_month("012"), "fields.expiry_month"),
        // audit: COCRDUP-027
        (SHOWN.with_year("02030"), "fields.expiry_year"),
    ];
    for (fields, field) in cases {
        let reply = h.put(&handle, &fields.body("ENTER", &shown)).await?;
        assert_error(
            &reply,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            Some(field),
        );
    }
    let failed = h.get_failing(EDIT, &pair(&card.acct, &card.number)).await?;
    // audit: COCRDUPC-019
    assert_file_error(&failed, Some("acct_id"));
    let failed = h.get_failing(EDIT, &by_ref(&card.acct, &handle)).await?;
    // audit: COCRDUPC-019
    assert_file_error(&failed, Some("acct_id"));
    Ok(())
}

/// The selected card as route state (COCOM01Y-014): an endpoint 7 row filtered by the card's
/// account carries its handle and its redacted number, and endpoints 8 and 9, sent that account
/// and handle and no card number, read the same card and show it redacted the same way.
#[tokio::test]
async fn cocom01y_card_rows() -> TestResult {
    let h = Harness::start().await?;
    let card = h.line(1)?;
    let listed = h.list(&query(&[("acct_id", &card.acct)])).await?;
    assert_eq!(listed.status, StatusCode::OK);
    let rows = listed.rows()?;
    let (handle, number, _, _) = rows.first().ok_or("the card list lists no row")?;
    // audit: COCOM01Y-014
    assert!(
        *handle == card.handle && *number == card.redacted,
        "the listed row is not the card's handle and redacted number"
    );
    // audit: COCOM01Y-014
    assert!(
        number.len() == 16
            && number.get(..12) == Some("************")
            && number.get(12..) == card.number.get(12..)
            && *number != card.number,
        "the listed card number is not twelve * and the last four digits"
    );
    let sent = by_ref(&card.acct, &handle.to_string());
    for path in [VIEW, EDIT] {
        let reply = h.get(path, &sent, &h.user).await?;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        // audit: COCOM01Y-014
        assert!(
            reply.int("card_ref") == Some(*handle)
                && reply.text("acct_id") == Some(card.acct.as_str())
                && reply.text("card_num") == Some(number.as_str()),
            "{path} reads another card by the listed handle"
        );
    }
    Ok(())
}

// The cases below have no single COBOL result. Each test asserts nothing and is reported
// ignored with its halt; the tags keep the rows visible to the a8 coverage check, which counts
// them as blocked, never as satisfied.

// audit: COCRDLIC-037
#[test]
#[ignore = "BLOCKED(H4): COCRDLIC-037 — the CICS RESP and RESP2 codes in bytes 49-58 and 66-75 of the CARDDAT file-error text have no PostgreSQL counterpart"]
fn blocked_cocrdlic_file_error_codes() {}

// audit: COCRDLIC-038
#[test]
#[ignore = "BLOCKED(X-COCRDLIC-H1): COCRDLIC-038 — a browse whose STARTBR finds no card ends the task by CICS's abnormal end, which no endpoint response reproduces"]
fn blocked_cocrdlic_unpositioned_browse() {}

// audit: COCRDLIC-006
// audit: COCRDLIC-008
// audit: COCRDLIC-009
#[test]
#[ignore = "BLOCKED(X-COCRDLIC-H2): COCRDLIC-006, COCRDLIC-008, COCRDLIC-009 — the zero test of a non-numeric filter reads invalid zoned data, whose result the compiler options decide"]
fn blocked_cocrdlic_filter_zero_test() {}

// audit: COCRDSLC-005
#[test]
#[ignore = "BLOCKED(X-COCRDSLC-H2): COCRDSLC-005 — the first-entry test reads uninitialised working storage before the COMMAREA is moved in"]
fn blocked_cocrdslc_first_entry_storage() {}

// audit: COCRDSLC-011
// audit: COCRDSLC-012
#[test]
#[ignore = "BLOCKED(X-COCRDSLC-H1): COCRDSLC-011, COCRDSLC-012 — the zero test of a non-numeric input reads invalid zoned data, whose result the compiler options decide"]
fn blocked_cocrdslc_input_zero_test() {}

// audit: COCRDSLC-017
#[test]
#[ignore = "BLOCKED(H4): COCRDSLC-017 — the CICS RESP and RESP2 codes in bytes 49-58 and 66-75 of the CARDDAT file-error text have no PostgreSQL counterpart"]
fn blocked_cocrdslc_file_error_codes() {}

// audit: COCRDUPC-047
#[test]
#[ignore = "BLOCKED(H4): COCRDUPC-047 — the CICS RESP and RESP2 codes in bytes 49-58 and 66-75 of the CARDDAT file-error text have no PostgreSQL counterpart"]
fn blocked_cocrdupc_file_error_codes() {}

// audit: COCRDUPC-002
// audit: COCRDUPC-038
#[test]
#[ignore = "BLOCKED(X-COCRDUPC-H1): COCRDUPC-002 (the C, L and F first-entry case), COCRDUPC-038 (the FILLER bytes written) — both depend on storage past the 160-byte COMMAREA"]
fn blocked_cocrdupc_commarea_overrun() {}

// audit: COCRDUPC-011
// audit: COCRDUPC-013
// audit: COCRDUPC-030
// audit: COCRDUPC-032
// audit: COCRDUPC-038
#[test]
#[ignore = "BLOCKED(X-COCRDUPC-H2): COCRDUPC-011, COCRDUPC-013 (zero-nibble keys), COCRDUPC-030, COCRDUPC-032 (non-digit month or year), COCRDUPC-038 (the CVV written) — numeric redefinitions read unchecked screen text"]
fn blocked_cocrdupc_unchecked_numeric() {}
