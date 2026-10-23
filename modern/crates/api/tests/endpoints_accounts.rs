//! g1b for COACTVWC and COACTUPC (endpoints 4 to 6): every `server` row of
//! `modern/audit/online/COACTVWC.md` and `modern/audit/online/COACTUPC.md`, the rows of
//! `modern/audit/online/CSUTLDTC.md` that COACTUPC reaches (CSUTLDTC-001 to CSUTLDTC-027) and
//! the server half of the `both` rows of maps `COACTVW` and `COACTUP` in
//! `modern/audit/screens.md`, proved through `GET /api/v1/accounts/{acct_id}/view`,
//! `GET /api/v1/accounts/{acct_id}` and `PUT /api/v1/accounts/{acct_id}` over the
//! fixture-loaded database.
//!
//! They also bind the COMMAREA rows COCOM01Y-008 to COCOM01Y-013 of
//! `modern/audit/copybooks.md`: `cocom01y_account_rows` proves that endpoint 4 takes the
//! selected account from each request's path and that the customer ID, the three customer names
//! and the active status it shows come from the stored records that account leads to, never from
//! state a request carries.
//!
//! # Harness
//!
//! - Each test owns one `postgres:16` container from
//!   `test_support::fixtures::start_loaded_postgres`, held for the whole test and removed when it
//!   ends. The schema comes from test-support alone (Single Schema Executor, Local-Only Runtime).
//! - The API is `api::app::app` over that database, signing tokens with a local secret and
//!   reading time from a `FixedClock` at 2022-07-18 10:30:00.000. Requests go in process through
//!   `tower::ServiceExt::oneshot`, with the `U` token of `USER0001` and the `A` token of
//!   `ADMIN001` that endpoint 1 issues.
//! - Outcomes the fixtures do not hold are built through the real repositories: test accounts,
//!   customers and cross-references numbered from 60, a cross-reference whose account or
//!   customer is missing, and a second cross-reference for fixture account 2. A store that
//!   fails every statement is a second pool on the same database, closed before the call. An
//!   account or customer read that fails after the cross-reference read found its row is the
//!   loaded pool behind `FailingReads`, a `persistence::RepositoryProbe` installed with
//!   `PgStore::with_probe`, which skips the statement at its probe points, fails it with a
//!   store error and records each call it is asked about. A lock that cannot be had is a row
//!   lock another transaction holds while the API's pool runs with a short `lock_timeout` its
//!   URL sets. Over the pool `carddemo-api` serves requests from,
//!   `runtime::db::connect_for_requests`, such a lock makes the save wait until its holder
//!   commits instead. The loaded pool itself is never closed.
//! - Every response body is searched for the 50 fixture SSN values in both forms, the fixture
//!   and test card numbers, the test SSN values and a masked card number: none may appear
//!   (Cardholder Data Redaction; COACTVWC-017, COACTVWC-034, COACTUPC-076, COACTUPC-090). No
//!   assertion message prints a stored SSN.
//!
//! # Reported, never passed
//!
//! The `#[ignore]` tests at the end carry the tags of what no endpoint test can assert: the CICS
//! codes of halt H4, the `CEEDAYS` verdict of halt H5 and the cases the audits' halt candidates
//! name. Each returns an error when it is run, so a run with `--ignored` fails on them.

use std::error::Error;
use std::fmt::{self, Write as _};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use api::app::{AppState, app};
use api::dto::FIELD_LENGTH_MESSAGE;
use api::dto::coactupc::{AccountUpdateFields, AccountUpdateSnapshot};
use axum::body::{self, Body};
use axum::http::{Method, Request, StatusCode, header};
use chrono::{NaiveDate, NaiveDateTime};
use domain::copybook::framing::decode_all;
use domain::copybook::{FixedRecord, PicDate, PicU, PicX};
use domain::money::{Decimal, Money};
use domain::records::account::AccountRecord;
use domain::records::card_xref::CardXrefRecord;
use domain::records::customer::CustomerRecord;
use domain::{CardNumber, FixedClock, Ssn};
use persistence::{
    AccountRepository, CardXrefRepository, CustomerRepository, PersistenceError, PgStore,
    ProbeCall, ProbePoint, ProbeVerdict, RepositoryProbe,
};
use runtime::config::Config;
use serde_json::{Map, Value, json};
use test_support::fixtures::{
    fixture_card_numbers, fixture_path, fixture_ssn_forms, start_loaded_postgres,
};
use test_support::postgres::PostgresGuard;
use tower::ServiceExt as _;

type TestResult = Result<(), Box<dyn Error>>;
type Fallible<T> = Result<T, Box<dyn Error>>;

/// The local-only secret the tests' API signs and verifies tokens with.
const SECRET: &str = "endpoints-accounts-local-secret";
/// The password of every fixture user (`app/jcl/DUSRSECJ.jcl`, `README.md`).
const PASSWORD: &str = "PASSWORD";
/// The server setting of the impatient pool: a row lock another transaction holds fails the
/// waiting statement after half a second instead of making it wait (SQLSTATE `55P03`).
const LOCK_TIMEOUT_QUERY: &str = "options[lock_timeout]=500ms";

// Messages, as the audits quote them.
const NO_INPUT: &str = "No input received";
const VIEW_KEY_INVALID: &str = "Account Filter must  be a non-zero 11 digit number";
const UPDATE_KEY_INVALID: &str = "Account Number if supplied must be a 11 digit Non-Zero Number";
const NO_CHANGE: &str = "No change detected with respect to values fetched.";
const SEARCH_KEYS: &str = "Enter or update id of account to update";
const PRESENTED: &str = "Update account details presented above.";
const VALIDATED: &str = "Changes validated.Press F5 to save";
const COMMITTED: &str = "Changes committed to database";
const LOCK_ACCOUNT: &str = "Could not lock account record for update";
const LOCK_CUSTOMER: &str = "Could not lock customer record for update";
const CHANGED: &str = "Record changed by some one else. Please review";
const UPDATE_FAILED: &str = "Update of record failed";

/// Fixture account 1: its card is `cardxref.txt:49` and its customer `000000001`.
const ACCOUNT_1: &str = "00000000001";
/// No `card_xref` row has this account.
const ACCOUNT_UNKNOWN: &str = "99999999999";
/// Fixture account 12, whose customer's stored SSN starts with a forbidden first part.
const ACCOUNT_12: &str = "00000000012";

/// The test account and customer every field edit of which passes; its group ID is not blank.
const CLEAN: u64 = 60;
/// As [`CLEAN`], with a blank group ID (E07, COACTUPC-042).
const BLANK_GROUP: u64 = 61;
/// As [`CLEAN`], with a ZIP+4 stored for its customer (X-COACTUPC-9).
const ZIP_PLUS_4: u64 = 62;
/// A cross-reference to customer [`CLEAN`] whose account is missing.
const NO_ACCOUNT: u64 = 63;
/// A cross-reference to fixture customer 1 whose account is missing.
const NO_ACCOUNT_FIXTURE_CUSTOMER: u64 = 70;
/// A cross-reference whose account and customer are both missing.
const NOTHING_BEHIND: u64 = 71;
/// An account whose cross-reference names a missing customer.
const NO_CUSTOMER: u64 = 72;
/// The missing customer of [`NOTHING_BEHIND`].
const MISSING_CUSTOMER_71: u64 = 999_999_971;
/// The missing customer of [`NO_CUSTOMER`].
const MISSING_CUSTOMER_72: u64 = 999_999_972;

/// The stored SSN of the test customers, and the one a save re-enters: protected values that
/// no response may hold, in either form.
const TEST_SSN: &str = "345678912";
const TEST_SSN_PARTS: [&str; 3] = ["345", "67", "8912"];
const REENTERED_SSN: &str = "345118912";
const REENTERED_SSN_PARTS: [&str; 3] = ["345", "11", "8912"];
/// The SSN another user stores for customer [`CLEAN`] between a fetch and its save: the same
/// last four digits as [`TEST_SSN`] (COACTUPC-115).
const OTHER_USER_SSN: &str = "456788912";
/// The SSN another user stores for customer [`CLEAN`] in `coactupc_change_check_terms`: the
/// first five digits of [`TEST_SSN`] and other last four (COACTUPC-115).
const OTHER_LAST4_SSN: &str = "345674321";
const TEST_SSN_NEEDLES: [&str; 8] = [
    "345678912",
    "345-67-8912",
    "345118912",
    "345-11-8912",
    "456788912",
    "456-78-8912",
    "345674321",
    "345-67-4321",
];

/// The account number of a test entity, eleven digits.
fn acct(id: u64) -> String {
    format!("{id:011}")
}

/// The card number of the test cross-reference numbered `id`.
fn test_card(id: u64) -> String {
    format!("{:016}", 4_000_000_000_000_000_u64 + id)
}

/// The instant of the fixed clock: 2022-07-18 10:30:00.000.
fn test_instant() -> Fallible<NaiveDateTime> {
    NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|day| day.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or_else(|| "the test instant is no date".into())
}

/// `raw` as one path segment, every byte but an ASCII letter or digit percent-encoded, so that
/// spaces, `*`, `%00` and signs reach the handler as typed.
fn segment(raw: &str) -> Result<String, fmt::Error> {
    let mut encoded = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}")?;
        }
    }
    Ok(encoded)
}

/// The records of the ASCII fixture `name`, read in place through codec A.
fn fixture<R: FixedRecord>(name: &str) -> Fallible<Vec<R>> {
    let bytes = fs::read(fixture_path(name))?;
    Ok(decode_all::<R>(&bytes)?
        .into_iter()
        .map(|framed| framed.record)
        .collect())
}

/// `***-**-` and the last four digits of the SSN of `custdata.txt`'s customer `customer_id`,
/// read from bytes 285 to 288 of its raw line rather than through the codec under test.
fn redacted_fixture_ssn(customer_id: &str) -> Fallible<String> {
    let text = fs::read_to_string(fixture_path("custdata.txt"))?;
    let line = text
        .lines()
        .find(|line| line.starts_with(customer_id))
        .ok_or("no such fixture customer")?;
    let last4 = line.get(284..288).ok_or("a short customer line")?;
    Ok(format!("***-**-{last4}"))
}

/// The last four digits of that SSN, as the snapshot's `ssn_last4` holds them.
fn fixture_ssn_last4(customer_id: &str) -> Fallible<String> {
    let redacted = redacted_fixture_ssn(customer_id)?;
    Ok(redacted.trim_start_matches("***-**-").to_owned())
}

/// `text` left-justified in a field of `width` characters.
fn pad(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

/// A test account, `CVACT01Y` (300 bytes): active, balance 1,234.56, credit limit 5,000.00, cash
/// limit 1,500.00, opened 2014-11-03, expiring and reissued 2025-05-20, cycle credit zero, cycle
/// debit -25.50, the group code `A000000000` in `ACCT-ADDR-ZIP` as the fixtures hold it, and
/// `group` as its group ID.
fn test_account(id: u64, group: &str) -> Fallible<AccountRecord> {
    let line = [
        acct(id),
        "Y".to_owned(),
        "00000012345F".to_owned(),
        "00000050000{".to_owned(),
        "00000015000{".to_owned(),
        "2014-11-03".to_owned(),
        "2025-05-20".to_owned(),
        "2025-05-20".to_owned(),
        "00000000000{".to_owned(),
        "00000000255}".to_owned(),
        "A000000000".to_owned(),
        pad(group, 10),
        pad("", 178),
    ]
    .concat();
    Ok(AccountRecord::decode(line.as_bytes())?)
}

/// A test customer, `CVCUS01Y` (500 bytes), every field of which passes COACTUPC's edits: a
/// state and ZIP the combination table holds, general-purpose area codes, a FICO score in range,
/// a date of birth in the past and [`TEST_SSN`]; no text field is blank.
fn test_customer(id: u64, zip: &str) -> Fallible<CustomerRecord> {
    let line = [
        format!("{id:09}"),
        pad("John", 25),
        pad("Quincy", 25),
        pad("Public", 25),
        pad("1 Main Street", 50),
        pad("Suite 200", 50),
        pad("Springfield", 50),
        "IL".to_owned(),
        "USA".to_owned(),
        pad(zip, 10),
        pad("(312)555-0100", 15),
        pad("(415)555-0199", 15),
        TEST_SSN.to_owned(),
        pad("GOVT-ID-0001", 20),
        "1980-01-15".to_owned(),
        "0000000042".to_owned(),
        "Y".to_owned(),
        "750".to_owned(),
        pad("", 168),
    ]
    .concat();
    Ok(CustomerRecord::decode(line.as_bytes())?)
}

/// The test cross-reference of card [`test_card`]`(card)`, customer `cust` and account `account`.
fn test_xref(card: u64, cust: u64, account: u64) -> Fallible<CardXrefRecord> {
    Ok(CardXrefRecord {
        card_num: CardNumber::parse(&test_card(card))?,
        cust_id: PicU::new(cust)?,
        acct_id: PicU::new(account)?,
        filler: PicX::spaces(),
    })
}

/// Stores the test entities: accounts 60, 61, 62 and 72, customers 60 to 62, one card for
/// each of accounts 60 to 63 and 70 to 72, and a second card for fixture account 2.
async fn add_test_entities(store: &PgStore) -> TestResult {
    let mut store = store.clone();
    for (id, group) in [
        (CLEAN, "GOLD"),
        (BLANK_GROUP, ""),
        (ZIP_PLUS_4, "GOLD"),
        (NO_CUSTOMER, "GOLD"),
    ] {
        AccountRepository::insert(&mut store, &test_account(id, group)?).await?;
    }
    for (id, zip) in [
        (CLEAN, "62701"),
        (BLANK_GROUP, "62701"),
        (ZIP_PLUS_4, "62701-1234"),
    ] {
        CustomerRepository::insert(&mut store, &test_customer(id, zip)?).await?;
    }
    for (id, cust) in [
        (CLEAN, CLEAN),
        (BLANK_GROUP, BLANK_GROUP),
        (ZIP_PLUS_4, ZIP_PLUS_4),
        (NO_ACCOUNT, CLEAN),
        (NO_ACCOUNT_FIXTURE_CUSTOMER, 1),
        (NOTHING_BEHIND, MISSING_CUSTOMER_71),
        (NO_CUSTOMER, MISSING_CUSTOMER_72),
    ] {
        CardXrefRepository::insert(&mut store, &test_xref(id, cust, id)?).await?;
    }
    let first = fixture::<CardXrefRecord>("cardxref.txt")?
        .into_iter()
        .find(|xref| xref.acct_id.get() == 2)
        .ok_or("fixture account 2 has no card")?;
    let second = CardXrefRecord {
        card_num: CardNumber::parse(&test_card(2))?,
        ..first
    };
    CardXrefRepository::insert(&mut store, &second).await?;
    Ok(())
}

/// A response: its status and its JSON body.
struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    /// The body's `message`, empty when it has none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The body's member `key` as text, empty when it is not text.
    fn text(&self, key: &str) -> &str {
        self.body
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

/// The API state over `store`: the local secret, job decks under this test binary's temporary
/// directory, and the fixed clock.
fn state_over(store: PgStore) -> Fallible<AppState> {
    let config = Config {
        jwt_secret: SECRET.to_owned(),
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("endpoints-accounts-jobs"),
        ..Config::default()
    };
    let clock = Arc::new(FixedClock::new(test_instant()?));
    Ok(AppState::from_config(store, &config, clock)?)
}

/// One test's database, its API and its tokens.
struct Harness {
    /// The fixture-loaded container, removed when the test ends.
    guard: PostgresGuard,
    /// The loaded pool, through which the test reads back and builds what it needs.
    store: PgStore,
    /// The API over the loaded pool.
    state: AppState,
    /// The token of `USER0001`, user type `U`.
    user: String,
    /// The token of `ADMIN001`, user type `A`.
    admin: String,
    /// The protected values no response may hold.
    needles: Vec<String>,
}

impl Harness {
    /// A fixture-loaded database with the test entities, its API, and both users signed on
    /// through endpoint 1.
    async fn start() -> Fallible<Self> {
        let guard = start_loaded_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        add_test_entities(&store).await?;
        let state = state_over(store.clone())?;
        let mut needles = fixture_ssn_forms()?;
        needles.extend(fixture_card_numbers()?);
        needles.extend(
            [CLEAN, BLANK_GROUP, ZIP_PLUS_4, NO_ACCOUNT, 2]
                .into_iter()
                .chain([NO_ACCOUNT_FIXTURE_CUSTOMER, NOTHING_BEHIND, NO_CUSTOMER])
                .map(test_card),
        );
        needles.extend(TEST_SSN_NEEDLES.map(str::to_owned));
        let mut harness = Self {
            guard,
            store,
            state,
            user: String::new(),
            admin: String::new(),
            needles,
        };
        harness.user = harness.sign_on("USER0001").await?;
        harness.admin = harness.sign_on("ADMIN001").await?;
        Ok(harness)
    }

    /// The token endpoint 1 issues to `user_id`.
    async fn sign_on(&self, user_id: &str) -> Fallible<String> {
        let credentials = json!({ "user_id": user_id, "password": PASSWORD });
        let reply = self
            .call(
                &self.state,
                Method::POST,
                "/api/v1/signon",
                None,
                Some(&credentials),
            )
            .await?;
        assert_eq!(reply.status, StatusCode::OK, "sign-on of {user_id}");
        Ok(reply.text("token").to_owned())
    }

    /// `method` on `uri` against `state`, with `token` as bearer token and `payload` as JSON
    /// body when given. The body is read to its end and searched for every protected value.
    async fn call(
        &self,
        state: &AppState,
        method: Method,
        uri: &str,
        token: Option<&str>,
        payload: Option<&Value>,
    ) -> Fallible<Reply> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match payload {
            Some(json) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(json)?))?,
            None => builder.body(Body::empty())?,
        };
        let response = app(state.clone()).oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        let text = String::from_utf8(bytes.to_vec())?;
        self.assert_redacted(&text);
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        };
        Ok(Reply { status, body })
    }

    /// Cardholder Data Redaction: `text` holds no fixture or test SSN in either form, no card
    /// number and no masked card number. The message names no value.
    fn assert_redacted(&self, text: &str) {
        let leaked = self
            .needles
            .iter()
            .any(|needle| text.contains(needle.as_str()));
        assert!(!leaked, "a response body holds a protected value");
        assert!(
            !text.contains("************"),
            "a response body holds a masked card number"
        );
    }

    /// Endpoint 4 for the raw account-number text `raw`, with the `U` token.
    async fn view(&self, raw: &str) -> Fallible<Reply> {
        self.view_as(&self.state, &self.user, raw).await
    }

    /// Endpoint 4 for `raw` against `state` with `token`.
    async fn view_as(&self, state: &AppState, token: &str, raw: &str) -> Fallible<Reply> {
        let uri = format!("/api/v1/accounts/{}/view", segment(raw)?);
        self.call(state, Method::GET, &uri, Some(token), None).await
    }

    /// Endpoint 5 for `raw`, with the `U` token.
    async fn fetch(&self, raw: &str) -> Fallible<Reply> {
        self.fetch_as(&self.state, &self.user, raw).await
    }

    /// Endpoint 5 for `raw` against `state` with `token`.
    async fn fetch_as(&self, state: &AppState, token: &str, raw: &str) -> Fallible<Reply> {
        let uri = format!("/api/v1/accounts/{}", segment(raw)?);
        self.call(state, Method::GET, &uri, Some(token), None).await
    }

    /// Endpoint 6 for `raw` with `payload`, with the `U` token.
    async fn save(&self, raw: &str, payload: &Value) -> Fallible<Reply> {
        self.save_as(&self.state, &self.user, raw, payload).await
    }

    /// Endpoint 6 for `raw` with `payload` against `state` with `token`.
    async fn save_as(
        &self,
        state: &AppState,
        token: &str,
        raw: &str,
        payload: &Value,
    ) -> Fallible<Reply> {
        let uri = format!("/api/v1/accounts/{}", segment(raw)?);
        self.call(state, Method::PUT, &uri, Some(token), Some(payload))
            .await
    }

    /// Endpoint 5 for test or fixture account `id`, which must answer 200: the view, the
    /// fields a screen sends back unchanged, the SSN parts omitted, and the snapshot.
    async fn fetched(
        &self,
        id: u64,
    ) -> Fallible<(Value, AccountUpdateFields, AccountUpdateSnapshot)> {
        let reply = self.fetch(&acct(id)).await?;
        assert_eq!(reply.status, StatusCode::OK, "fetch of account {id}");
        let fields = echoed(&reply.body)?;
        let snapshot = snapshot_of(&reply.body)?;
        Ok((reply.body, fields, snapshot))
    }

    /// The stored account `id`, read back through the repository.
    async fn account(&self, id: u64) -> Fallible<AccountRecord> {
        let mut store = self.store.clone();
        AccountRepository::find_by_id(&mut store, PicU::new(id)?)
            .await?
            .ok_or_else(|| format!("account {id} is not stored").into())
    }

    /// The stored customer `id`, read back through the repository.
    async fn customer(&self, id: u64) -> Fallible<CustomerRecord> {
        let mut store = self.store.clone();
        CustomerRepository::find_by_id(&mut store, PicU::new(id)?)
            .await?
            .ok_or_else(|| format!("customer {id} is not stored").into())
    }

    /// A second pool on this test's database, built by `runtime::db::connect`, with the
    /// connection-URL query `query` when one is given.
    async fn second_store(&self, query: Option<&str>) -> Fallible<PgStore> {
        let url = self.guard.database_url();
        let database_url = query.map_or_else(|| url.to_owned(), |query| format!("{url}?{query}"));
        let config = Config {
            database_url,
            ..Config::default()
        };
        Ok(PgStore::new(runtime::db::connect(&config).await?))
    }

    /// An API whose store fails every statement: a second pool, closed before any call.
    async fn failing_state(&self) -> Fallible<AppState> {
        let store = self.second_store(None).await?;
        store.pool().close().await;
        state_over(store)
    }

    /// An API whose row locks give up after [`LOCK_TIMEOUT_QUERY`]'s half second.
    async fn impatient_state(&self) -> Fallible<AppState> {
        state_over(self.second_store(Some(LOCK_TIMEOUT_QUERY)).await?)
    }

    /// An API over the pool `carddemo-api` serves requests from, with the store built as
    /// `carddemo-api` builds its own: `runtime::db::connect_for_requests` on this test's
    /// database, failing at once while the pool's refusal watch finds the server refusing.
    async fn request_state(&self) -> Fallible<AppState> {
        let config = Config {
            database_url: self.guard.database_url().to_owned(),
            ..Config::default()
        };
        let (pool, refusals) = runtime::db::connect_for_requests(&config).await?;
        let store = PgStore::new(pool)
            .failing_fast_while(move || refusals.server_refusing())
            .await;
        state_over(store)
    }

    /// An API over the loaded pool whose store skips and fails every call at one of `fails`,
    /// and the probe that records what that store asked it ([`FailingReads`]). The tokens of
    /// [`Harness::start`] hold against it, as it signs with the same secret.
    fn probed_state(&self, fails: &[ProbePoint]) -> Fallible<(AppState, Arc<FailingReads>)> {
        let probe = FailingReads::at(fails);
        let shared: Arc<dyn RepositoryProbe> = probe.clone();
        let store = PgStore::new(self.guard.pool().clone()).with_probe(shared);
        Ok((state_over(store)?, probe))
    }
}

/// One call a probe was asked about: its point, and the account or customer ID its first
/// argument names, `None` when the call is keyed otherwise.
type Seen = (ProbePoint, Option<u64>);

/// The probed reads of the chain after a found cross-reference: the account's, the customer's,
/// and both.
const ACCOUNT_READ: &[ProbePoint] = &[ProbePoint::AccountRead];
const CUSTOMER_READ: &[ProbePoint] = &[ProbePoint::CustomerRead];
const BOTH_READS: &[ProbePoint] = &[ProbePoint::AccountRead, ProbePoint::CustomerRead];

/// The probe of [`Harness::probed_state`]: every call at one of `fails` skips its statement and
/// fails with [`probed_store_error`], every other call runs it, and each call is recorded, in
/// order. It reaches the account and customer reads that `find_with_customer` issues after the
/// cross-reference read found exactly one row, which no input fails over a healthy pool.
struct FailingReads {
    /// The points whose calls fail.
    fails: Vec<ProbePoint>,
    /// The calls asked about so far.
    seen: Mutex<Vec<Seen>>,
}

impl FailingReads {
    /// A probe that fails every call at one of `points`.
    fn at(points: &[ProbePoint]) -> Arc<Self> {
        Arc::new(Self {
            fails: points.to_vec(),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// The calls recorded so far, in order. A test that panicked while recording leaves its
    /// records readable.
    fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl RepositoryProbe for FailingReads {
    fn answer(&self, call: &ProbeCall<'_>) -> ProbeVerdict {
        let key = call
            .arg::<PicU<11>>(0)
            .copied()
            .map(PicU::get)
            .or_else(|| call.arg::<PicU<9>>(0).copied().map(PicU::get));
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((call.point(), key));
        if self.fails.contains(&call.point()) {
            ProbeVerdict::Fail(probed_store_error())
        } else {
            ProbeVerdict::Proceed
        }
    }
}

/// The error a probed call fails with: an SQLSTATE `XX000` internal error with no constraint,
/// as `persistence::error` maps an unexpected server error.
fn probed_store_error() -> PersistenceError {
    PersistenceError::Database {
        sqlstate: Some("XX000".to_owned()),
        constraint: None,
    }
}

/// What a probe sees of one read chain whose cross-reference was found for account `account`
/// and names customer `customer`: the account read, then the customer read, whatever the
/// account read returned (COACTVWC-033, COACTUPC-084).
fn chain_reads(account: u64, customer: u64) -> [Seen; 2] {
    [
        (ProbePoint::AccountRead, Some(account)),
        (ProbePoint::CustomerRead, Some(customer)),
    ]
}

/// An error response: its status, message and field, and nothing else in the body, because a
/// `ScreenError` is `{ message, field? }` (AAP 0.3.4.1).
#[track_caller]
fn assert_screen_error(reply: &Reply, status: StatusCode, message: &str, field: Option<&str>) {
    assert_eq!(reply.status, status, "status of {message:?}");
    assert_eq!(reply.message(), message);
    assert_eq!(
        reply.body.get("field").and_then(Value::as_str),
        field,
        "field of {message:?}"
    );
    let members = reply.body.as_object().map_or(0, Map::len);
    assert_eq!(
        members,
        1 + usize::from(field.is_some()),
        "members of {message:?}"
    );
}

/// The members of endpoint 4's account block (COACTVWC-010 to COACTVWC-015).
const VIEW_ACCOUNT_MEMBERS: [&str; 10] = [
    "active_status",
    "open_date",
    "expiration_date",
    "reissue_date",
    "credit_limit",
    "cash_credit_limit",
    "current_balance",
    "current_cycle_credit",
    "current_cycle_debit",
    "group_id",
];

/// The members of endpoint 4's customer block (COACTVWC-016 to COACTVWC-026).
const VIEW_CUSTOMER_MEMBERS: [&str; 18] = [
    "customer_id",
    "ssn",
    "date_of_birth",
    "fico_score",
    "first_name",
    "middle_name",
    "last_name",
    "address_line_1",
    "address_line_2",
    "city",
    "state",
    "zip",
    "country",
    "phone_1",
    "phone_2",
    "government_id",
    "eft_account_id",
    "primary_card_holder",
];

/// The customer members of endpoint 5's view (COACTUPC-075 to COACTUPC-077).
const UPDATE_CUSTOMER_MEMBERS: [&str; 22] = [
    "customer_id",
    "ssn",
    "dob",
    "fico_score",
    "first_name",
    "middle_name",
    "last_name",
    "address_line_1",
    "address_line_2",
    "city",
    "state",
    "zip",
    "country",
    "phone_1_area",
    "phone_1_prefix",
    "phone_1_line",
    "phone_2_area",
    "phone_2_prefix",
    "phone_2_line",
    "government_id",
    "eft_account_id",
    "primary_card_holder",
];

/// The customer members of endpoint 5's snapshot (COACTUPC-100).
const SNAPSHOT_CUSTOMER_MEMBERS: [&str; 18] = [
    "customer_id",
    "first_name",
    "middle_name",
    "last_name",
    "address_line_1",
    "address_line_2",
    "city",
    "state_code",
    "country",
    "zip",
    "phone_1",
    "phone_2",
    "ssn_last4",
    "government_id",
    "dob",
    "eft_account_id",
    "primary_card_holder",
    "fico_score",
];

/// The member names of the object `value`, sorted; none when it is not an object.
fn sorted_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    keys
}

/// The members `names` of the object `value`, as an object, a missing member as `null`.
fn members_of(value: &Value, names: &[&str]) -> Value {
    Value::Object(
        names
            .iter()
            .map(|&name| {
                (
                    name.to_owned(),
                    value.get(name).cloned().unwrap_or(Value::Null),
                )
            })
            .collect(),
    )
}

/// An error response whose body is its `ScreenError`, status, message and field, with exactly
/// the members `beside` next to them (COACTVWC-008, COACTUPC-099).
#[track_caller]
fn assert_error_beside(reply: &Reply, status: StatusCode, field: Option<&str>, beside: &[&str]) {
    assert_eq!(reply.status, status, "status of {:?}", reply.message());
    assert!(
        !reply.message().is_empty(),
        "an error body without a message"
    );
    assert_eq!(
        reply.body.get("field").and_then(Value::as_str),
        field,
        "field of {:?}",
        reply.message()
    );
    let mut expected: Vec<&str> = ["message"]
        .into_iter()
        .chain(field.map(|_| "field"))
        .chain(beside.iter().copied())
        .collect();
    expected.sort_unstable();
    assert_eq!(
        sorted_keys(&reply.body),
        expected,
        "members of {:?}",
        reply.message()
    );
}

/// The fixed pieces of a message built in the 75-byte `WS-RETURN-MSG`, each at its 1-based
/// position. The message is padded back to 75 characters, the field's width, because the
/// response drops the trailing spaces.
#[track_caller]
fn assert_pieces(reply: &Reply, pieces: &[(usize, &str)]) {
    let message = format!("{:<75}", reply.message());
    for &(position, piece) in pieces {
        let start = position - 1;
        assert_eq!(
            message.get(start..start + piece.len()),
            Some(piece),
            "position {position} of {message:?}"
        );
    }
}

/// The fixed pieces of the `Account:` not-found text for account `key` with the file label
/// `label` (COACTVWC-035, COACTVWC-040, COACTUPC-085, COACTUPC-091).
#[track_caller]
fn assert_account_not_found(reply: &Reply, key: &str, label: &str) {
    assert_pieces(
        reply,
        &[
            (1, "Account:"),
            (9, key),
            (20, " not found in"),
            (33, label),
            (66, " Reas:"),
        ],
    );
}

/// The fixed pieces of the `CustId:` not-found text for customer `key` (COACTVWC-044,
/// COACTUPC-095).
#[track_caller]
fn assert_customer_not_found(reply: &Reply, key: &str) {
    assert_pieces(
        reply,
        &[
            (1, "CustId:"),
            (8, key),
            (17, " not found"),
            (27, " in customer master.Resp: "),
            (63, " REAS:"),
        ],
    );
}

/// The fixed pieces of the `File Error:` text of a failed `READ` of `file` (COACTVWC-037,
/// COACTUPC-087).
#[track_caller]
fn assert_file_error(reply: &Reply, file: &str) {
    assert_pieces(
        reply,
        &[
            (1, "File Error: "),
            (13, "READ    "),
            (21, " on "),
            (25, file),
            (34, " returned RESP "),
            (59, ",RESP2 "),
        ],
    );
}

/// A 200 of endpoint 6 that leaves state `N`: the info line, both keys shown, nothing saved
/// (COACTUPC-014, COACTUPC-079, COACTUPC-082).
#[track_caller]
fn assert_validated(reply: &Reply) {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["message"], VALIDATED);
    assert_eq!(reply.body["state"], "N");
    assert_eq!(reply.body["save_enabled"], true);
    assert_eq!(reply.body["cancel_enabled"], true);
    assert_eq!(reply.body["committed"], false);
}

/// A 200 of endpoint 6 that saved both records: state `C`, no key shown (COACTUPC-079,
/// COACTUPC-082, COACTUPC-108).
#[track_caller]
fn assert_committed(reply: &Reply) {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["message"], COMMITTED);
    assert_eq!(reply.body["state"], "C");
    assert_eq!(reply.body["save_enabled"], false);
    assert_eq!(reply.body["cancel_enabled"], false);
    assert_eq!(reply.body["committed"], true);
}

/// The text of the view member `key`.
fn member(view: &Value, key: &str) -> Fallible<String> {
    view.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("the view has no text member {key}").into())
}

/// A typed date of the view, `null` for a blank one, as the screen's year, month and day parts.
fn date_parts(view: &Value, key: &str) -> Fallible<[String; 3]> {
    match view.get(key).and_then(Value::as_str) {
        None => Ok(<[String; 3]>::default()),
        Some(text) => {
            let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")?;
            Ok(["%Y", "%m", "%d"].map(|part| date.format(part).to_string()))
        }
    }
}

/// The fields a screen sends back unchanged after showing the endpoint 5 view `view`: every
/// member as shown, the dates as their parts, and the SSN parts omitted (COACTUPC-064).
fn echoed(view: &Value) -> Fallible<AccountUpdateFields> {
    let [open_year, open_month, open_day] = date_parts(view, "open_date")?;
    let [expiry_year, expiry_month, expiry_day] = date_parts(view, "expiry_date")?;
    let [reissue_year, reissue_month, reissue_day] = date_parts(view, "reissue_date")?;
    let [dob_year, dob_month, dob_day] = date_parts(view, "dob")?;
    let text = |key: &str| member(view, key);
    Ok(AccountUpdateFields {
        active_status: text("active_status")?,
        open_year,
        open_month,
        open_day,
        credit_limit: text("credit_limit")?,
        expiry_year,
        expiry_month,
        expiry_day,
        cash_credit_limit: text("cash_credit_limit")?,
        reissue_year,
        reissue_month,
        reissue_day,
        current_balance: text("current_balance")?,
        current_cycle_credit: text("current_cycle_credit")?,
        group_id: text("group_id")?,
        current_cycle_debit: text("current_cycle_debit")?,
        customer_id: text("customer_id")?,
        ssn_part1: None,
        ssn_part2: None,
        ssn_part3: None,
        dob_year,
        dob_month,
        dob_day,
        fico_score: text("fico_score")?,
        first_name: text("first_name")?,
        middle_name: text("middle_name")?,
        last_name: text("last_name")?,
        address_line_1: text("address_line_1")?,
        state: text("state")?,
        address_line_2: text("address_line_2")?,
        zip: text("zip")?,
        city: text("city")?,
        country: text("country")?,
        phone_1_area: text("phone_1_area")?,
        phone_1_prefix: text("phone_1_prefix")?,
        phone_1_line: text("phone_1_line")?,
        government_id: text("government_id")?,
        phone_2_area: text("phone_2_area")?,
        phone_2_prefix: text("phone_2_prefix")?,
        phone_2_line: text("phone_2_line")?,
        eft_account_id: text("eft_account_id")?,
        primary_card_holder: text("primary_card_holder")?,
    })
}

/// The snapshot of the endpoint 5 view `view`.
fn snapshot_of(view: &Value) -> Fallible<AccountUpdateSnapshot> {
    let snapshot = view
        .get("snapshot")
        .cloned()
        .ok_or("the view has no snapshot")?;
    Ok(serde_json::from_value(snapshot)?)
}

/// `snapshot` with the state code `state`, as the client echoes it after a response that left
/// that state.
fn in_state(snapshot: &AccountUpdateSnapshot, state: &str) -> AccountUpdateSnapshot {
    AccountUpdateSnapshot {
        state: state.to_owned(),
        ..snapshot.clone()
    }
}

/// The SSN parts of `fields` set to `parts`, as the operator re-enters them.
fn with_ssn(fields: &mut AccountUpdateFields, parts: [&str; 3]) {
    let [area, group, serial] = parts.map(|part| Some(part.to_owned()));
    fields.ssn_part1 = area;
    fields.ssn_part2 = group;
    fields.ssn_part3 = serial;
}

/// The body of endpoint 6: `aid`, `fields` and `snapshot`. The SSN parts are written into the
/// JSON by hand, because `AccountUpdateFields` never serialises them.
fn update_body(
    aid: &str,
    fields: &AccountUpdateFields,
    snapshot: &AccountUpdateSnapshot,
) -> Fallible<Value> {
    let mut members = serde_json::to_value(fields)?;
    let parts = [
        ("ssn_part1", &fields.ssn_part1),
        ("ssn_part2", &fields.ssn_part2),
        ("ssn_part3", &fields.ssn_part3),
    ];
    if let Some(object) = members.as_object_mut() {
        for (name, part) in parts {
            if let Some(part) = part {
                object.insert(name.to_owned(), Value::from(part.as_str()));
            }
        }
    }
    Ok(json!({ "aid": aid, "fields": members, "snapshot": snapshot }))
}

/// An amount as `Money<10>`, from its value in cents.
fn money(cents: i64) -> Fallible<Money<10>> {
    Ok(Money::new(Decimal::new(cents, 2))?)
}

/// A calendar date of the tests.
fn day(year: i32, month: u32, date: u32) -> Fallible<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, date).ok_or_else(|| "no such date".into())
}

// ---------------------------------------------------------------------------------------------
// Endpoint 4: COACTVWC
// ---------------------------------------------------------------------------------------------

#[test]
fn coactvwc_view_rows() -> TestResult {
    current_thread()?.block_on(async {
        let harness = Harness::start().await?;
        view_gate_and_account_edit(&harness).await?;
        view_fixture_account(&harness).await?;
        view_test_accounts(&harness).await?;
        view_read_chain_failures(&harness).await?;
        view_reads_after_the_cross_reference(&harness).await?;
        Ok(())
    })
}

/// The token check, the user types and the account-number edit of endpoint 4 (COACTVWC-002,
/// COACTVWC-029 to COACTVWC-031, `screens.md` COACTVW-013).
async fn view_gate_and_account_edit(h: &Harness) -> TestResult {
    let uri = format!("/api/v1/accounts/{ACCOUNT_1}/view");
    let unsigned = h.call(&h.state, Method::GET, &uri, None, None).await?;
    let as_user = h.view(ACCOUNT_1).await?;
    let as_admin = h.view_as(&h.state, &h.admin, ACCOUNT_1).await?;
    // audit: COACTVWC-002
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    assert_eq!(as_user.status, StatusCode::OK);
    assert_eq!(
        as_user.body, as_admin.body,
        "endpoint 4 answers every user type alike"
    );

    for blank in [
        " ",
        "           ",
        "*",
        "*          ",
        "\0\0\0\0\0\0\0\0\0\0\0",
    ] {
        let reply = h.view(blank).await?;
        // audit: COACTVWC-029
        // audit: COACTVWC-030
        assert_screen_error(&reply, StatusCode::BAD_REQUEST, NO_INPUT, Some("acct_id"));
    }
    for invalid in [
        "*1",
        " 0000000001",
        "0000000001 ",
        "123",
        "0000000000A",
        "00000000000",
        "+0000000001",
        "1.000000000",
    ] {
        let reply = h.view(invalid).await?;
        // audit: COACTVWC-029
        // audit: COACTVWC-031
        assert_screen_error(
            &reply,
            StatusCode::BAD_REQUEST,
            VIEW_KEY_INVALID,
            Some("acct_id"),
        );
    }
    // A text longer than the 11-character `ACCTSID` field is refused, never cut to account 1.
    let reply = h.view("000000000012").await?;
    // audit: COACTVW-013
    // audit: COACTVWC-031
    assert_screen_error(
        &reply,
        StatusCode::BAD_REQUEST,
        VIEW_KEY_INVALID,
        Some("acct_id"),
    );
    Ok(())
}

/// Endpoint 4 for fixture account 1: every member of both blocks, no message, and no write
/// (COACTVWC-005, COACTVWC-008, COACTVWC-010 to COACTVWC-026, COACTVWC-032 to COACTVWC-034).
async fn view_fixture_account(h: &Harness) -> TestResult {
    let account_before = h.account(1).await?;
    let customer_before = h.customer(1).await?;
    let reply = h.view(ACCOUNT_1).await?;
    let view = &reply.body;
    // audit: COACTVWC-005
    // audit: COACTVWC-008
    // audit: COACTVWC-032
    // audit: COACTVWC-033
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(view["acct_id"], ACCOUNT_1);
    assert!(view.get("message").is_none(), "a view carries no message");
    // audit: COACTVWC-010
    assert_eq!(view["active_status"], "Y");
    // audit: COACTVWC-011
    assert_eq!(view["current_balance"], "+        194.00");
    // audit: COACTVWC-012
    assert_eq!(view["credit_limit"], "+      2,020.00");
    assert_eq!(view["cash_credit_limit"], "+      1,020.00");
    // audit: COACTVWC-013
    assert_eq!(view["current_cycle_credit"], "+           .00");
    assert_eq!(view["current_cycle_debit"], "+           .00");
    // audit: COACTVWC-014
    assert_eq!(view["open_date"], "2014-11-20");
    assert_eq!(view["expiration_date"], "2025-05-20");
    assert_eq!(view["reissue_date"], "2025-05-20");
    // audit: COACTVWC-015
    assert_eq!(view["group_id"], pad("", 10));
    // audit: COACTVWC-016
    // audit: COACTVWC-034
    assert_eq!(view["customer_id"], "000000001");
    // audit: COACTVWC-017
    assert_eq!(view["ssn"], redacted_fixture_ssn("000000001")?);
    // audit: COACTVWC-018
    assert_eq!(view["fico_score"], "274");
    // audit: COACTVWC-019
    assert_eq!(view["date_of_birth"], "1961-06-08");
    // audit: COACTVWC-020
    assert_eq!(view["first_name"], pad("Immanuel", 25));
    assert_eq!(view["middle_name"], pad("Madeline", 25));
    assert_eq!(view["last_name"], pad("Kessler", 25));
    // audit: COACTVWC-021
    assert_eq!(view["address_line_1"], pad("618 Deshaun Route", 50));
    assert_eq!(view["address_line_2"], pad("Apt. 802", 50));
    assert_eq!(view["city"], pad("Altenwerthshire", 50));
    // audit: COACTVWC-022
    assert_eq!(view["state"], "NC");
    assert_eq!(view["country"], "USA");
    // audit: COACTVWC-023
    assert_eq!(view["zip"], "12546");
    // audit: COACTVWC-024
    assert_eq!(view["phone_1"], "(908)119-8310");
    assert_eq!(view["phone_2"], "(373)693-8684");
    // audit: COACTVWC-025
    assert_eq!(view["government_id"], "00000000000049368437");
    assert_eq!(view["eft_account_id"], "0053581756");
    // audit: COACTVWC-026
    assert_eq!(view["primary_card_holder"], "Y");
    // The card number the cross-reference read returns is in no member, full or masked.
    // audit: COACTVWC-034
    h.assert_redacted(&view.to_string());
    // audit: COACTVWC-005
    assert!(
        h.account(1).await? == account_before,
        "endpoint 4 rewrote the account"
    );
    assert!(
        h.customer(1).await? == customer_before,
        "endpoint 4 rewrote the customer"
    );
    Ok(())
}

/// Endpoint 4 for the test accounts: a negative amount, a group ID, ZIP+4 and phone numbers
/// that the narrower map fields cut (COACTVWC-011 to COACTVWC-025).
async fn view_test_accounts(h: &Harness) -> TestResult {
    let reply = h.view(&acct(CLEAN)).await?;
    let view = &reply.body;
    assert_eq!(reply.status, StatusCode::OK);
    // audit: COACTVWC-011
    assert_eq!(view["current_balance"], "+      1,234.56");
    // audit: COACTVWC-012
    assert_eq!(view["credit_limit"], "+      5,000.00");
    assert_eq!(view["cash_credit_limit"], "+      1,500.00");
    // audit: COACTVWC-013
    assert_eq!(view["current_cycle_debit"], "-         25.50");
    // audit: COACTVWC-014
    assert_eq!(view["open_date"], "2014-11-03");
    // audit: COACTVWC-015
    assert_eq!(view["group_id"], pad("GOLD", 10));
    // audit: COACTVWC-016
    assert_eq!(view["customer_id"], "000000060");
    // audit: COACTVWC-017
    assert_eq!(view["ssn"], "***-**-8912");
    // audit: COACTVWC-019
    assert_eq!(view["date_of_birth"], "1980-01-15");
    // audit: COACTVWC-023
    assert_eq!(view["zip"], "62701");
    // Thirteen characters of `(AAA)BBB-CCCC` and two spaces: the two dropped are spaces.
    // audit: COACTVWC-024
    assert_eq!(view["phone_1"], "(312)555-0100");
    assert_eq!(view["phone_2"], "(415)555-0199");
    // audit: COACTVWC-025
    assert_eq!(view["government_id"], pad("GOVT-ID-0001", 20));
    assert_eq!(view["eft_account_id"], "0000000042");

    let zip_plus_4 = h.view(&acct(ZIP_PLUS_4)).await?;
    // `62701-1234` keeps its first five characters in the 5-byte field.
    // audit: COACTVWC-023
    assert_eq!(zip_plus_4.status, StatusCode::OK);
    assert_eq!(zip_plus_4.body["zip"], "62701");
    Ok(())
}

/// The read chain's outcomes on endpoint 4: each not-found text, the `DUPKEY` of two cards, the
/// message precedence, the blocks beside a message after a partial read and a failing store
/// (COACTVWC-008, COACTVWC-009, COACTVWC-033 to COACTVWC-040, COACTVWC-044).
async fn view_read_chain_failures(h: &Harness) -> TestResult {
    let unknown = h.view(ACCOUNT_UNKNOWN).await?;
    // audit: COACTVWC-033
    // audit: COACTVWC-035
    assert_screen_error(
        &unknown,
        StatusCode::NOT_FOUND,
        unknown.message(),
        Some("acct_id"),
    );
    assert_account_not_found(&unknown, ACCOUNT_UNKNOWN, " Cross ref file.  Resp:");

    let two_cards = h.view(&acct(2)).await?;
    // audit: COACTVWC-037
    // audit: COACTVWC-039
    let status = StatusCode::INTERNAL_SERVER_ERROR;
    assert_screen_error(&two_cards, status, two_cards.message(), Some("acct_id"));
    assert_file_error(&two_cards, "CXACAIX  ");

    // The account is missing and the customer is found: the account's message, status and
    // field, the customer block beside them, and an account block whose values are halt
    // candidate X-COACTVWC-H1's and are not asserted (`blocked_uninitialised_storage`).
    let no_account = h.view(&acct(NO_ACCOUNT_FIXTURE_CUSTOMER)).await?;
    let fixture_customer = h.view(ACCOUNT_1).await?;
    // audit: COACTVWC-008
    // audit: COACTVWC-009
    // audit: COACTVWC-040
    assert_error_beside(
        &no_account,
        StatusCode::NOT_FOUND,
        Some("acct_id"),
        &["account", "customer"],
    );
    let key = acct(NO_ACCOUNT_FIXTURE_CUSTOMER);
    assert_account_not_found(&no_account, &key, " Acct Master file.Resp:");
    assert_eq!(
        no_account.body["customer"],
        members_of(&fixture_customer.body, &VIEW_CUSTOMER_MEMBERS),
        "the customer block is fixture customer 1's"
    );
    let mut account_members = VIEW_ACCOUNT_MEMBERS.to_vec();
    account_members.sort_unstable();
    assert_eq!(
        sorted_keys(&no_account.body["account"]),
        account_members,
        "the account block is present with the account members"
    );

    // Account and customer both missing: the first message set, the account's, stands.
    let nothing = h.view(&acct(NOTHING_BEHIND)).await?;
    // audit: COACTVWC-033
    // audit: COACTVWC-040
    // audit: COACTVWC-044
    assert_screen_error(
        &nothing,
        StatusCode::NOT_FOUND,
        nothing.message(),
        Some("acct_id"),
    );
    assert_account_not_found(&nothing, &acct(NOTHING_BEHIND), " Acct Master file.Resp:");

    // The account is found and its customer is missing: the customer's text, no field, and the
    // account block beside them. Account 72 is stored as account 60 is, apart from its key.
    let no_customer = h.view(&acct(NO_CUSTOMER)).await?;
    let same_account = h.view(&acct(CLEAN)).await?;
    // audit: COACTVWC-008
    // audit: COACTVWC-044
    assert_error_beside(&no_customer, StatusCode::NOT_FOUND, None, &["account"]);
    assert_customer_not_found(&no_customer, &format!("{MISSING_CUSTOMER_72:09}"));
    assert_eq!(
        no_customer.body["account"],
        members_of(&same_account.body, &VIEW_ACCOUNT_MEMBERS),
        "the account block is account 72's"
    );

    // A store that fails every statement fails the first read, the cross-reference's.
    let failing = h.failing_state().await?;
    let failed = h.view_as(&failing, &h.user, ACCOUNT_1).await?;
    // audit: COACTVWC-033
    // audit: COACTVWC-037
    assert_screen_error(&failed, status, failed.message(), Some("acct_id"));
    assert_file_error(&failed, "CXACAIX  ");
    Ok(())
}

/// Endpoint 4 when the account or the customer read fails after the cross-reference read found
/// its row, each failure made by [`FailingReads`] on the loaded pool; no outcome writes
/// (COACTVWC-005).
async fn view_reads_after_the_cross_reference(h: &Harness) -> TestResult {
    let account_before = h.account(1).await?;
    let customer_before = h.customer(1).await?;
    let healthy = h.view(ACCOUNT_1).await?;
    assert_eq!(
        healthy.status,
        StatusCode::OK,
        "the unprobed view of account 1"
    );
    view_account_read_failing(h, &healthy).await?;
    view_customer_read_failing(h, &healthy).await?;
    // audit: COACTVWC-005
    assert!(
        h.account(1).await? == account_before,
        "a failing read rewrote the account"
    );
    assert!(
        h.customer(1).await? == customer_before,
        "a failing read rewrote the customer"
    );
    Ok(())
}

/// The account read fails: its `ACCTDAT` text, status 500 and field `acct_id`, while the
/// customer read still runs; a customer it finds is shown beside the message, a missing one
/// leaves the account's message alone. A chain that ends at the cross-reference asks for
/// neither read. `healthy` is the unprobed view of account 1 (COACTVWC-008, COACTVWC-033,
/// COACTVWC-042, COACTVWC-044).
async fn view_account_read_failing(h: &Harness, healthy: &Reply) -> TestResult {
    let status = StatusCode::INTERNAL_SERVER_ERROR;
    // Customer 1 is found: its block beside the account's message, status and field, and an
    // account block whose values are halt candidate X-COACTVWC-H1's and are not asserted
    // (`blocked_uninitialised_storage`).
    let (state, probe) = h.probed_state(ACCOUNT_READ)?;
    let failed = h.view_as(&state, &h.user, ACCOUNT_1).await?;
    // audit: COACTVWC-008
    // audit: COACTVWC-042
    assert_error_beside(&failed, status, Some("acct_id"), &["account", "customer"]);
    assert_file_error(&failed, "ACCTDAT  ");
    assert_eq!(
        failed.body["customer"],
        members_of(&healthy.body, &VIEW_CUSTOMER_MEMBERS),
        "the customer block is fixture customer 1's"
    );
    let mut account_members = VIEW_ACCOUNT_MEMBERS.to_vec();
    account_members.sort_unstable();
    assert_eq!(
        sorted_keys(&failed.body["account"]),
        account_members,
        "the account block is present with the account members"
    );
    // Both reads ran after the cross-reference was found: the account's by the path key, then
    // the customer's by the cross-reference's customer ID, though the account read failed.
    // audit: COACTVWC-033
    // audit: COACTVWC-042
    assert_eq!(probe.seen(), chain_reads(1, 1));

    // The customer is missing: the account's message stands, with no block.
    let (state, probe) = h.probed_state(ACCOUNT_READ)?;
    let no_customer = h.view_as(&state, &h.user, &acct(NO_CUSTOMER)).await?;
    // audit: COACTVWC-042
    // audit: COACTVWC-044
    let message = no_customer.message();
    assert_screen_error(&no_customer, status, message, Some("acct_id"));
    assert_file_error(&no_customer, "ACCTDAT  ");
    assert_eq!(probe.seen(), chain_reads(NO_CUSTOMER, MISSING_CUSTOMER_72));

    // No cross-reference, or two: the chain ends there, and neither later read is asked for.
    let (state, probe) = h.probed_state(BOTH_READS)?;
    let unknown = h.view_as(&state, &h.user, ACCOUNT_UNKNOWN).await?;
    let two_cards = h.view_as(&state, &h.user, &acct(2)).await?;
    // audit: COACTVWC-033
    let not_found = StatusCode::NOT_FOUND;
    assert_screen_error(&unknown, not_found, unknown.message(), Some("acct_id"));
    assert_account_not_found(&unknown, ACCOUNT_UNKNOWN, " Cross ref file.  Resp:");
    assert_screen_error(&two_cards, status, two_cards.message(), Some("acct_id"));
    assert_file_error(&two_cards, "CXACAIX  ");
    assert!(
        probe.seen().is_empty(),
        "a read was asked for after the cross-reference"
    );
    Ok(())
}

/// The customer read fails: its `CUSTDAT` text and status 500 replace any earlier message, with
/// field `acct_id` only when the account read did not succeed and the account block only when
/// it found the account. `healthy` is the unprobed view of account 1 (COACTVWC-008,
/// COACTVWC-033, COACTVWC-040, COACTVWC-046).
async fn view_customer_read_failing(h: &Harness, healthy: &Reply) -> TestResult {
    let status = StatusCode::INTERNAL_SERVER_ERROR;
    let (state, probe) = h.probed_state(CUSTOMER_READ)?;
    let failed = h.view_as(&state, &h.user, ACCOUNT_1).await?;
    // audit: COACTVWC-008
    // audit: COACTVWC-046
    assert_error_beside(&failed, status, None, &["account"]);
    assert_file_error(&failed, "CUSTDAT  ");
    assert_eq!(
        failed.body["account"],
        members_of(&healthy.body, &VIEW_ACCOUNT_MEMBERS),
        "the account block is fixture account 1's"
    );
    assert_eq!(probe.seen(), chain_reads(1, 1));

    // The account read fails too: the customer's text with the account's field, and no block.
    let (state, _) = h.probed_state(BOTH_READS)?;
    let both = h.view_as(&state, &h.user, ACCOUNT_1).await?;
    // audit: COACTVWC-033
    // audit: COACTVWC-046
    assert_screen_error(&both, status, both.message(), Some("acct_id"));
    assert_file_error(&both, "CUSTDAT  ");

    // The account is missing: the customer's 500 replaces the account's 404, with no block.
    let (state, _) = h.probed_state(CUSTOMER_READ)?;
    let key = acct(NO_ACCOUNT_FIXTURE_CUSTOMER);
    let no_account = h.view_as(&state, &h.user, &key).await?;
    // audit: COACTVWC-040
    // audit: COACTVWC-046
    let message = no_account.message();
    assert_screen_error(&no_account, status, message, Some("acct_id"));
    assert_file_error(&no_account, "CUSTDAT  ");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Endpoint 5: COACTUPC fetch
// ---------------------------------------------------------------------------------------------

#[test]
fn coactupc_fetch_rows() -> TestResult {
    current_thread()?.block_on(async {
        let harness = Harness::start().await?;
        fetch_gate_and_account_edit(&harness).await?;
        fetch_fixture_account(&harness).await?;
        fetch_read_chain_failures(&harness).await?;
        fetch_reads_after_the_cross_reference(&harness).await?;
        Ok(())
    })
}

/// The token check, the user types and the account edit of endpoints 5 and 6 (COACTUPC-002,
/// COACTUPC-011, COACTUPC-045 to COACTUPC-047, `screens.md` COACTUP-013).
async fn fetch_gate_and_account_edit(h: &Harness) -> TestResult {
    let unsigned = h
        .call(
            &h.state,
            Method::GET,
            &format!("/api/v1/accounts/{ACCOUNT_1}"),
            None,
            None,
        )
        .await?;
    let as_user = h.fetch(ACCOUNT_1).await?;
    let as_admin = h.fetch_as(&h.state, &h.admin, ACCOUNT_1).await?;
    // audit: COACTUPC-002
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    assert_eq!(as_user.status, StatusCode::OK);
    assert_eq!(
        as_user.body, as_admin.body,
        "endpoint 5 answers every user type alike"
    );

    for blank in [
        " ",
        "           ",
        "*",
        "*          ",
        "\0\0\0\0\0\0\0\0\0\0\0",
    ] {
        let reply = h.fetch(blank).await?;
        // audit: COACTUPC-011
        // audit: COACTUPC-045
        assert_screen_error(&reply, StatusCode::BAD_REQUEST, NO_INPUT, Some("acct_id"));
    }
    for invalid in [
        "123",
        "0000000000A",
        "00000000000",
        "-0000000001",
        "1.000000000",
        " 0000000001",
    ] {
        let reply = h.fetch(invalid).await?;
        // audit: COACTUPC-011
        // audit: COACTUPC-046
        assert_screen_error(
            &reply,
            StatusCode::BAD_REQUEST,
            UPDATE_KEY_INVALID,
            Some("acct_id"),
        );
    }
    // A text longer than the 11-character `ACCTSID` field is refused on both endpoints, never
    // cut to the account its first eleven characters name.
    let too_long = "000000000012";
    let fetched = h.fetch(too_long).await?;
    let fields = echoed(&as_user.body)?;
    let payload = update_body("ENTER", &fields, &snapshot_of(&as_user.body)?)?;
    let saved = h.save(too_long, &payload).await?;
    // audit: COACTUP-013
    // audit: COACTUPC-046
    assert_screen_error(
        &fetched,
        StatusCode::BAD_REQUEST,
        UPDATE_KEY_INVALID,
        Some("acct_id"),
    );
    assert_screen_error(
        &saved,
        StatusCode::BAD_REQUEST,
        UPDATE_KEY_INVALID,
        Some("acct_id"),
    );
    Ok(())
}

/// Endpoint 5 for fixture account 1: the account and customer blocks as shown, the redacted
/// SSN, the snapshot, and no write (COACTUPC-010, COACTUPC-047, COACTUPC-068, COACTUPC-074 to
/// COACTUPC-077, COACTUPC-079, COACTUPC-084, COACTUPC-090, COACTUPC-100).
async fn fetch_fixture_account(h: &Harness) -> TestResult {
    let account_before = h.account(1).await?;
    let customer_before = h.customer(1).await?;
    let reply = h.fetch(ACCOUNT_1).await?;
    let view = &reply.body;
    // audit: COACTUPC-010
    // audit: COACTUPC-047
    // audit: COACTUPC-068
    // audit: COACTUPC-079
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(view["info_message"], PRESENTED);
    // audit: COACTUPC-074
    assert_eq!(view["account_id"], ACCOUNT_1);
    assert_eq!(view["active_status"], "Y");
    assert_eq!(view["current_balance"], "+        194.00");
    assert_eq!(view["credit_limit"], "+      2,020.00");
    assert_eq!(view["cash_credit_limit"], "+      1,020.00");
    assert_eq!(view["current_cycle_credit"], "+           .00");
    assert_eq!(view["current_cycle_debit"], "+           .00");
    assert_eq!(view["open_date"], "2014-11-20");
    assert_eq!(view["expiry_date"], "2025-05-20");
    assert_eq!(view["reissue_date"], "2025-05-20");
    assert_eq!(view["group_id"], pad("", 10));
    // audit: COACTUPC-075
    assert_eq!(view["customer_id"], "000000001");
    assert_eq!(view["fico_score"], "274");
    assert_eq!(view["dob"], "1961-06-08");
    assert_eq!(view["first_name"], pad("Immanuel", 25));
    assert_eq!(view["middle_name"], pad("Madeline", 25));
    assert_eq!(view["last_name"], pad("Kessler", 25));
    assert_eq!(view["address_line_1"], pad("618 Deshaun Route", 50));
    assert_eq!(view["address_line_2"], pad("Apt. 802", 50));
    assert_eq!(view["city"], pad("Altenwerthshire", 50));
    assert_eq!(view["state"], "NC");
    assert_eq!(view["country"], "USA");
    assert_eq!(view["government_id"], "00000000000049368437");
    assert_eq!(view["eft_account_id"], "0053581756");
    assert_eq!(view["primary_card_holder"], "Y");
    // audit: COACTUPC-076
    assert_eq!(view["ssn"], redacted_fixture_ssn("000000001")?);
    // audit: COACTUPC-077
    assert_eq!(view["zip"], "12546");
    let phones = ["phone_1_area", "phone_1_prefix", "phone_1_line"]
        .into_iter()
        .chain(["phone_2_area", "phone_2_prefix", "phone_2_line"])
        .map(|key| reply.text(key))
        .collect::<Vec<_>>();
    assert_eq!(phones, ["908", "119", "8310", "373", "693", "8684"]);
    fetch_fixture_snapshot(&reply)?;
    // The cross-reference's card number is in no member, full or masked.
    // audit: COACTUPC-090
    h.assert_redacted(&view.to_string());
    // audit: COACTUPC-084
    assert!(
        h.account(1).await? == account_before,
        "endpoint 5 rewrote the account"
    );
    assert!(
        h.customer(1).await? == customer_before,
        "endpoint 5 rewrote the customer"
    );
    Ok(())
}

/// The snapshot endpoint 5 returns for fixture account 1: state `S`, plain decimal amounts,
/// typed dates, the stored ZIP and phone numbers in full and only the SSN's last four digits
/// (COACTUPC-100).
fn fetch_fixture_snapshot(reply: &Reply) -> TestResult {
    let snapshot = &reply.body["snapshot"];
    // audit: COACTUPC-100
    assert_eq!(snapshot["state"], "S");
    assert_eq!(snapshot["account_id"], ACCOUNT_1);
    assert_eq!(snapshot["active_status"], "Y");
    assert_eq!(snapshot["current_balance"], "194.00");
    assert_eq!(snapshot["credit_limit"], "2020.00");
    assert_eq!(snapshot["cash_credit_limit"], "1020.00");
    assert_eq!(snapshot["current_cycle_credit"], "0.00");
    assert_eq!(snapshot["current_cycle_debit"], "0.00");
    assert_eq!(snapshot["open_date"], "2014-11-20");
    assert_eq!(snapshot["expiry_date"], "2025-05-20");
    assert_eq!(snapshot["reissue_date"], "2025-05-20");
    assert_eq!(snapshot["group_id"], pad("", 10));
    assert_eq!(snapshot["customer_id"], "000000001");
    assert_eq!(snapshot["first_name"], pad("Immanuel", 25));
    assert_eq!(snapshot["city"], pad("Altenwerthshire", 50));
    assert_eq!(snapshot["state_code"], "NC");
    assert_eq!(snapshot["zip"], pad("12546", 10));
    assert_eq!(snapshot["phone_1"], pad("(908)119-8310", 15));
    assert_eq!(snapshot["phone_2"], pad("(373)693-8684", 15));
    assert_eq!(snapshot["ssn_last4"], fixture_ssn_last4("000000001")?);
    assert_eq!(snapshot["dob"], "1961-06-08");
    assert_eq!(snapshot["eft_account_id"], "0053581756");
    assert_eq!(snapshot["primary_card_holder"], "Y");
    assert_eq!(snapshot["fico_score"], "274");
    assert!(
        snapshot.get("ssn").is_none(),
        "the snapshot holds no SSN member"
    );
    Ok(())
}

/// The read chain's outcomes on endpoints 5 and 6: each not-found text, the `DUPKEY` of two
/// cards, the message precedence, the state and details beside a message after a partial read
/// and a failing store (COACTUPC-084 to COACTUPC-099).
async fn fetch_read_chain_failures(h: &Harness) -> TestResult {
    let unknown = h.fetch(ACCOUNT_UNKNOWN).await?;
    // audit: COACTUPC-084
    // audit: COACTUPC-085
    assert_screen_error(
        &unknown,
        StatusCode::NOT_FOUND,
        unknown.message(),
        Some("acct_id"),
    );
    assert_account_not_found(&unknown, ACCOUNT_UNKNOWN, " Cross ref file.  Resp:");

    let status = StatusCode::INTERNAL_SERVER_ERROR;
    let two_cards = h.fetch(&acct(2)).await?;
    // audit: COACTUPC-087
    // audit: COACTUPC-089
    assert_screen_error(&two_cards, status, two_cards.message(), Some("acct_id"));
    assert_file_error(&two_cards, "CXACAIX  ");

    // The account is missing and the customer is found: the account's message, status and
    // field, state `S` and the details beside them, whose customer values are customer 60's.
    // Their account values and the account part of their snapshot are halt candidate
    // X-COACTUPC-H1's and are not asserted (`blocked_uninitialised_storage`).
    let no_account = h.fetch(&acct(NO_ACCOUNT)).await?;
    let (same_customer, _, _) = h.fetched(CLEAN).await?;
    // audit: COACTUPC-091
    // audit: COACTUPC-099
    assert_error_beside(
        &no_account,
        StatusCode::NOT_FOUND,
        Some("acct_id"),
        &["details", "state"],
    );
    assert_account_not_found(&no_account, &acct(NO_ACCOUNT), " Acct Master file.Resp:");
    let details = &no_account.body["details"];
    assert_eq!(no_account.body["state"], "S");
    assert_eq!(details["info_message"], PRESENTED);
    assert_eq!(details["snapshot"]["state"], "S");
    assert_eq!(
        members_of(details, &UPDATE_CUSTOMER_MEMBERS),
        members_of(&same_customer, &UPDATE_CUSTOMER_MEMBERS),
        "the details show customer 60"
    );
    assert_eq!(
        members_of(&details["snapshot"], &SNAPSHOT_CUSTOMER_MEMBERS),
        members_of(&same_customer["snapshot"], &SNAPSHOT_CUSTOMER_MEMBERS),
        "the snapshot keeps customer 60"
    );
    // The details are what a screen echoes: they read back as the fields and snapshot of an
    // endpoint 6 request.
    echoed(details)?;
    snapshot_of(details)?;

    // Both missing: the account's message, the first set, stands.
    let nothing = h.fetch(&acct(NOTHING_BEHIND)).await?;
    // audit: COACTUPC-084
    // audit: COACTUPC-091
    // audit: COACTUPC-095
    assert_screen_error(
        &nothing,
        StatusCode::NOT_FOUND,
        nothing.message(),
        Some("acct_id"),
    );
    assert_account_not_found(&nothing, &acct(NOTHING_BEHIND), " Acct Master file.Resp:");

    let no_customer = h.fetch(&acct(NO_CUSTOMER)).await?;
    // audit: COACTUPC-095
    assert_screen_error(
        &no_customer,
        StatusCode::NOT_FOUND,
        no_customer.message(),
        None,
    );
    assert_customer_not_found(&no_customer, &format!("{MISSING_CUSTOMER_72:09}"));

    // Endpoint 6 re-reads the chain first and answers its failures as endpoint 5 does.
    let (_, fields, snapshot) = h.fetched(CLEAN).await?;
    let payload = update_body("ENTER", &fields, &snapshot)?;
    let saved = h.save(&acct(NO_CUSTOMER), &payload).await?;
    // audit: COACTUPC-095
    assert_screen_error(&saved, StatusCode::NOT_FOUND, saved.message(), None);
    assert_customer_not_found(&saved, &format!("{MISSING_CUSTOMER_72:09}"));

    // A store that fails every statement fails the first read, the cross-reference's.
    let failing = h.failing_state().await?;
    let fetch_failed = h.fetch_as(&failing, &h.user, ACCOUNT_1).await?;
    let save_failed = h.save_as(&failing, &h.user, &acct(CLEAN), &payload).await?;
    // audit: COACTUPC-084
    // audit: COACTUPC-087
    for failed in [&fetch_failed, &save_failed] {
        assert_screen_error(failed, status, failed.message(), Some("acct_id"));
        assert_file_error(failed, "CXACAIX  ");
    }
    Ok(())
}

/// Endpoints 5 and 6 when the account or the customer read fails after the cross-reference
/// read found its row, each failure made by [`FailingReads`] on the loaded pool; nothing is
/// written (COACTUPC-084, COACTUPC-093, COACTUPC-097).
async fn fetch_reads_after_the_cross_reference(h: &Harness) -> TestResult {
    let account_before = h.account(CLEAN).await?;
    let customer_before = h.customer(CLEAN).await?;
    let account_72_before = h.account(NO_CUSTOMER).await?;
    fetch_account_read_failing(h).await?;
    fetch_customer_read_failing(h).await?;
    save_reads_failing(h).await?;
    // audit: COACTUPC-084
    // audit: COACTUPC-093
    // audit: COACTUPC-097
    assert_stored(h, CLEAN, &account_before, &customer_before).await?;
    assert_eq!(
        h.account(NO_CUSTOMER).await?,
        account_72_before,
        "stored account {NO_CUSTOMER}"
    );
    Ok(())
}

/// Endpoint 5 when the account read fails: its `ACCTDAT` text, status 500 and field `acct_id`,
/// while the customer read still runs; a customer it finds is shown in state `S` beside the
/// message, a missing one leaves the account's message and no details. A chain that ends at the
/// cross-reference asks for neither read (COACTUPC-084, COACTUPC-093, COACTUPC-095,
/// COACTUPC-099).
async fn fetch_account_read_failing(h: &Harness) -> TestResult {
    let status = StatusCode::INTERNAL_SERVER_ERROR;
    let (healthy, _, _) = h.fetched(CLEAN).await?;
    // Customer 60 is found: state `S` and the details beside the account's message, status and
    // field, whose customer values are customer 60's. Their account values and the account part
    // of their snapshot are halt candidate X-COACTUPC-H1's and are not asserted
    // (`blocked_uninitialised_storage`).
    let (state, probe) = h.probed_state(ACCOUNT_READ)?;
    let failed = h.fetch_as(&state, &h.user, &acct(CLEAN)).await?;
    // audit: COACTUPC-093
    // audit: COACTUPC-099
    assert_error_beside(&failed, status, Some("acct_id"), &["details", "state"]);
    assert_file_error(&failed, "ACCTDAT  ");
    let details = &failed.body["details"];
    assert_eq!(failed.body["state"], "S");
    assert_eq!(details["info_message"], PRESENTED);
    assert_eq!(details["snapshot"]["state"], "S");
    assert_eq!(
        members_of(details, &UPDATE_CUSTOMER_MEMBERS),
        members_of(&healthy, &UPDATE_CUSTOMER_MEMBERS),
        "the details show customer 60"
    );
    assert_eq!(
        members_of(&details["snapshot"], &SNAPSHOT_CUSTOMER_MEMBERS),
        members_of(&healthy["snapshot"], &SNAPSHOT_CUSTOMER_MEMBERS),
        "the snapshot keeps customer 60"
    );
    // The details are what a screen echoes: they read back as the fields and snapshot of an
    // endpoint 6 request.
    echoed(details)?;
    snapshot_of(details)?;
    // Both reads ran after the cross-reference was found, though the account read failed.
    // audit: COACTUPC-084
    // audit: COACTUPC-093
    assert_eq!(probe.seen(), chain_reads(CLEAN, CLEAN));

    // The customer is missing: the account's message stands, the state stays not fetched.
    let (state, probe) = h.probed_state(ACCOUNT_READ)?;
    let no_customer = h.fetch_as(&state, &h.user, &acct(NO_CUSTOMER)).await?;
    // audit: COACTUPC-093
    // audit: COACTUPC-095
    let message = no_customer.message();
    assert_screen_error(&no_customer, status, message, Some("acct_id"));
    assert_file_error(&no_customer, "ACCTDAT  ");
    assert_eq!(probe.seen(), chain_reads(NO_CUSTOMER, MISSING_CUSTOMER_72));

    // No cross-reference, or two: the chain ends there, and neither later read is asked for.
    let (state, probe) = h.probed_state(BOTH_READS)?;
    let unknown = h.fetch_as(&state, &h.user, ACCOUNT_UNKNOWN).await?;
    let two_cards = h.fetch_as(&state, &h.user, &acct(2)).await?;
    // audit: COACTUPC-084
    let not_found = StatusCode::NOT_FOUND;
    assert_screen_error(&unknown, not_found, unknown.message(), Some("acct_id"));
    assert_account_not_found(&unknown, ACCOUNT_UNKNOWN, " Cross ref file.  Resp:");
    assert_screen_error(&two_cards, status, two_cards.message(), Some("acct_id"));
    assert_file_error(&two_cards, "CXACAIX  ");
    assert!(
        probe.seen().is_empty(),
        "a read was asked for after the cross-reference"
    );
    Ok(())
}

/// Endpoint 5 when the customer read fails: its `CUSTDAT` text and status 500 replace any
/// earlier message, the state stays not fetched and no value is shown, with field `acct_id`
/// only when the account read did not succeed (COACTUPC-084, COACTUPC-091, COACTUPC-097).
async fn fetch_customer_read_failing(h: &Harness) -> TestResult {
    let status = StatusCode::INTERNAL_SERVER_ERROR;
    let (state, probe) = h.probed_state(CUSTOMER_READ)?;
    let failed = h.fetch_as(&state, &h.user, &acct(CLEAN)).await?;
    // audit: COACTUPC-097
    assert_screen_error(&failed, status, failed.message(), None);
    assert_file_error(&failed, "CUSTDAT  ");
    assert_eq!(probe.seen(), chain_reads(CLEAN, CLEAN));

    // The account read fails too: the customer's text with the account's field.
    let (state, _) = h.probed_state(BOTH_READS)?;
    let both = h.fetch_as(&state, &h.user, &acct(CLEAN)).await?;
    // audit: COACTUPC-084
    // audit: COACTUPC-097
    assert_screen_error(&both, status, both.message(), Some("acct_id"));
    assert_file_error(&both, "CUSTDAT  ");

    // The account is missing: the customer's 500 replaces the account's 404.
    let (state, _) = h.probed_state(CUSTOMER_READ)?;
    let no_account = h.fetch_as(&state, &h.user, &acct(NO_ACCOUNT)).await?;
    // audit: COACTUPC-091
    // audit: COACTUPC-097
    let message = no_account.message();
    assert_screen_error(&no_account, status, message, Some("acct_id"));
    assert_file_error(&no_account, "CUSTDAT  ");
    Ok(())
}

/// Endpoint 6 re-reads the chain before its decision, for ENTER and for F5 in state `N` alike,
/// and answers a read failing after the cross-reference as endpoint 5 does when the re-read
/// yields no customer; no lock is asked for, so nothing is written (COACTUPC-093,
/// COACTUPC-097). A re-read whose account read alone fails yields the customer, so its save
/// goes on to the account lock (COACTUPC-099, COACTUPC-101), which this step does not reach.
async fn save_reads_failing(h: &Harness) -> TestResult {
    let status = StatusCode::INTERNAL_SERVER_ERROR;
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    set(&mut fields.fico_score, "751");
    let payloads = [
        update_body("ENTER", &fields, &snapshot)?,
        update_body("PF5", &fields, &in_state(&snapshot, "N"))?,
    ];
    for (fails, id, customer, field, file) in [
        (CUSTOMER_READ, CLEAN, CLEAN, None, "CUSTDAT  "),
        (BOTH_READS, CLEAN, CLEAN, Some("acct_id"), "CUSTDAT  "),
        (
            ACCOUNT_READ,
            NO_CUSTOMER,
            MISSING_CUSTOMER_72,
            Some("acct_id"),
            "ACCTDAT  ",
        ),
    ] {
        for payload in &payloads {
            let (state, probe) = h.probed_state(fails)?;
            let saved = h.save_as(&state, &h.user, &acct(id), payload).await?;
            // audit: COACTUPC-093
            // audit: COACTUPC-097
            assert_screen_error(&saved, status, saved.message(), field);
            assert_file_error(&saved, file);
            assert_eq!(
                probe.seen(),
                chain_reads(id, customer),
                "the calls of {} on account {id}",
                payload["aid"]
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Endpoint 6: COACTUPC change check, keys and field edits (nothing written)
// ---------------------------------------------------------------------------------------------

/// One change to the echo of account [`CLEAN`], every field of which passes as fetched.
type Touch = fn(&mut AccountUpdateFields);

/// `value` into the request field `field`.
fn set(field: &mut String, value: &str) {
    value.clone_into(field);
}

/// The three parts of a date's map fields.
fn set_date(parts: [&mut String; 3], date: [&str; 3]) {
    let [year, month, day] = parts;
    set(year, date[0]);
    set(month, date[1]);
    set(day, date[2]);
}

/// The open date's three map fields.
fn open_date(fields: &mut AccountUpdateFields) -> [&mut String; 3] {
    [
        &mut fields.open_year,
        &mut fields.open_month,
        &mut fields.open_day,
    ]
}

/// The date of birth's three map fields.
fn dob(fields: &mut AccountUpdateFields) -> [&mut String; 3] {
    [
        &mut fields.dob_year,
        &mut fields.dob_month,
        &mut fields.dob_day,
    ]
}

/// Only the SSN part `index` re-entered, as `value`; the others omitted.
fn only_ssn_part(fields: &mut AccountUpdateFields, index: usize, value: &str) {
    let part = Some(value.to_owned());
    fields.ssn_part1 = None;
    fields.ssn_part2 = None;
    fields.ssn_part3 = None;
    match index {
        1 => fields.ssn_part1 = part,
        2 => fields.ssn_part2 = part,
        _ => fields.ssn_part3 = part,
    }
}

/// A change that fails an edit, with the message and field of the first failing edit.
struct EditCase {
    touch: Touch,
    message: &'static str,
    field: &'static str,
}

const fn fails(touch: Touch, message: &'static str, field: &'static str) -> EditCase {
    EditCase {
        touch,
        message,
        field,
    }
}

/// The failing edits of `1200-EDIT-MAP-INPUTS`, each on the echo of account [`CLEAN`]; a case
/// that changes two fields shows which edit runs first.
const EDIT_CASES: &[EditCase] = &[
    // audit: COACTUPC-013
    // audit: COACTUPC-015
    // audit: COACTUPC-049
    fails(
        |f| set(&mut f.active_status, ""),
        "Account Status must be supplied.",
        "active_status",
    ),
    fails(
        |f| set(&mut f.active_status, "0"),
        "Account Status must be supplied.",
        "active_status",
    ),
    // audit: COACTUPC-007
    fails(
        |f| set(&mut f.active_status, "*"),
        "Account Status must be supplied.",
        "active_status",
    ),
    // audit: COACTUPC-015
    // audit: COACTUPC-049
    fails(
        |f| set(&mut f.active_status, "X"),
        "Account Status must be Y or N.",
        "active_status",
    ),
    fails(
        |f| {
            set(&mut f.active_status, "y");
            set(&mut f.first_name, "Jim");
        },
        "Account Status must be Y or N.",
        "active_status",
    ),
    // audit: COACTUPC-016
    // audit: COACTUPC-117
    // audit: COACTUPC-118
    // audit: CSUTLDTC-027
    fails(
        |f| set(&mut f.open_year, ""),
        "Open Date : Year must be supplied.",
        "open_year",
    ),
    // audit: COACTUPC-119
    fails(
        |f| set(&mut f.open_year, "20A4"),
        "Open Date must be 4 digit number.",
        "open_year",
    ),
    // audit: COACTUPC-120
    fails(
        |f| set(&mut f.open_year, "1899"),
        "Open Date : Century is not valid.",
        "open_year",
    ),
    fails(
        |f| set(&mut f.open_year, "2100"),
        "Open Date : Century is not valid.",
        "open_year",
    ),
    // audit: COACTUPC-121
    fails(
        |f| set(&mut f.open_month, ""),
        "Open Date : Month must be supplied.",
        "open_month",
    ),
    // audit: COACTUPC-122
    fails(
        |f| set(&mut f.open_month, "13"),
        "Open Date: Month must be a number between 1 and 12.",
        "open_month",
    ),
    fails(
        |f| set(&mut f.open_month, "00"),
        "Open Date: Month must be a number between 1 and 12.",
        "open_month",
    ),
    // audit: COACTUPC-124
    fails(
        |f| set(&mut f.open_day, ""),
        "Open Date : Day must be supplied.",
        "open_day",
    ),
    // audit: COACTUPC-125
    fails(
        |f| set(&mut f.open_day, "32"),
        "Open Date:day must be a number between 1 and 31.",
        "open_day",
    ),
    fails(
        |f| set(&mut f.open_day, "00"),
        "Open Date:day must be a number between 1 and 31.",
        "open_day",
    ),
    fails(
        |f| set(&mut f.open_day, "AB"),
        "Open Date:day must be a number between 1 and 31.",
        "open_day",
    ),
    // audit: COACTUPC-126
    fails(
        |f| set_date(open_date(f), ["2014", "04", "31"]),
        "Open Date:Cannot have 31 days in this month.",
        "open_month",
    ),
    // audit: COACTUPC-127
    fails(
        |f| set_date(open_date(f), ["2016", "02", "30"]),
        "Open Date:Cannot have 30 days in this month.",
        "open_month",
    ),
    // audit: COACTUPC-128
    fails(
        |f| set_date(open_date(f), ["1900", "02", "29"]),
        "Open Date:Not a leap year.Cannot have 29 days in this month.",
        "open_year",
    ),
    fails(
        |f| set_date(open_date(f), ["2023", "02", "29"]),
        "Open Date:Not a leap year.Cannot have 29 days in this month.",
        "open_year",
    ),
    // The year part fails first; its message and field are returned.
    // audit: COACTUPC-117
    fails(
        |f| set_date(open_date(f), ["", "13", "03"]),
        "Open Date : Year must be supplied.",
        "open_year",
    ),
    // audit: COACTUPC-017
    // audit: COACTUPC-054
    fails(
        |f| set(&mut f.credit_limit, ""),
        "Credit Limit must be supplied.",
        "credit_limit",
    ),
    fails(
        |f| set(&mut f.credit_limit, "12AB"),
        "Credit Limit is not valid",
        "credit_limit",
    ),
    // audit: COACTUPC-018
    fails(
        |f| set(&mut f.expiry_month, ""),
        "Expiry Date : Month must be supplied.",
        "expiry_month",
    ),
    fails(
        |f| set(&mut f.expiry_year, "1800"),
        "Expiry Date : Century is not valid.",
        "expiry_year",
    ),
    // audit: COACTUPC-019
    // audit: COACTUPC-054
    fails(
        |f| set(&mut f.cash_credit_limit, ""),
        "Cash Credit Limit must be supplied.",
        "cash_credit_limit",
    ),
    fails(
        |f| set(&mut f.cash_credit_limit, "1.2.3"),
        "Cash Credit Limit is not valid",
        "cash_credit_limit",
    ),
    // audit: COACTUPC-020
    fails(
        |f| set(&mut f.reissue_day, "32"),
        "Reissue Date:day must be a number between 1 and 31.",
        "reissue_day",
    ),
    // audit: COACTUPC-021
    // audit: COACTUPC-054
    fails(
        |f| set(&mut f.current_balance, ""),
        "Current Balance must be supplied.",
        "current_balance",
    ),
    fails(
        |f| set(&mut f.current_balance, "12 34"),
        "Current Balance is not valid",
        "current_balance",
    ),
    // The 26-character name loses its final `t` in the 25-character field.
    // audit: COACTUPC-022
    fails(
        |f| set(&mut f.current_cycle_credit, ""),
        "Current Cycle Credit Limi must be supplied.",
        "current_cycle_credit",
    ),
    fails(
        |f| set(&mut f.current_cycle_credit, "1,,0"),
        "Current Cycle Credit Limi is not valid",
        "current_cycle_credit",
    ),
    // audit: COACTUPC-023
    fails(
        |f| set(&mut f.current_cycle_debit, ""),
        "Current Cycle Debit Limit must be supplied.",
        "current_cycle_debit",
    ),
    fails(
        |f| set(&mut f.current_cycle_debit, "ten"),
        "Current Cycle Debit Limit is not valid",
        "current_cycle_debit",
    ),
    // audit: COACTUPC-024
    // audit: COACTUPC-060
    // audit: COACTUPC-064
    fails(
        |f| with_ssn(f, ["", "", ""]),
        "SSN: First 3 chars must be supplied.",
        "ssn_part1",
    ),
    // audit: COACTUPC-053
    // audit: COACTUPC-060
    fails(
        |f| with_ssn(f, ["12A", "45", "6789"]),
        "SSN: First 3 chars must be all numeric.",
        "ssn_part1",
    ),
    fails(
        |f| with_ssn(f, ["000", "45", "6789"]),
        "SSN: First 3 chars must not be zero.",
        "ssn_part1",
    ),
    // audit: COACTUPC-061
    fails(
        |f| with_ssn(f, ["666", "45", "6789"]),
        "SSN: First 3 chars: should not be 000, 666, or between 900 and 999",
        "ssn_part1",
    ),
    fails(
        |f| with_ssn(f, ["900", "45", "6789"]),
        "SSN: First 3 chars: should not be 000, 666, or between 900 and 999",
        "ssn_part1",
    ),
    fails(
        |f| with_ssn(f, ["999", "45", "6789"]),
        "SSN: First 3 chars: should not be 000, 666, or between 900 and 999",
        "ssn_part1",
    ),
    // A part 1 that fails leaves parts 2 and 3 unreported, whatever they hold.
    // audit: COACTUPC-024
    // audit: COACTUPC-062
    // audit: COACTUPC-063
    fails(
        |f| with_ssn(f, ["000", "", ""]),
        "SSN: First 3 chars must not be zero.",
        "ssn_part1",
    ),
    fails(
        |f| with_ssn(f, ["666", "", "0000"]),
        "SSN: First 3 chars: should not be 000, 666, or between 900 and 999",
        "ssn_part1",
    ),
    // audit: COACTUPC-062
    fails(
        |f| with_ssn(f, ["345", "", "6789"]),
        "SSN 4th & 5th chars must be supplied.",
        "ssn_part2",
    ),
    fails(
        |f| with_ssn(f, ["345", "4A", "6789"]),
        "SSN 4th & 5th chars must be all numeric.",
        "ssn_part2",
    ),
    fails(
        |f| with_ssn(f, ["345", "00", "6789"]),
        "SSN 4th & 5th chars must not be zero.",
        "ssn_part2",
    ),
    // audit: COACTUPC-063
    fails(
        |f| with_ssn(f, ["345", "67", ""]),
        "SSN Last 4 chars must be supplied.",
        "ssn_part3",
    ),
    fails(
        |f| with_ssn(f, ["345", "67", "89A2"]),
        "SSN Last 4 chars must be all numeric.",
        "ssn_part3",
    ),
    fails(
        |f| with_ssn(f, ["345", "67", "0000"]),
        "SSN Last 4 chars must not be zero.",
        "ssn_part3",
    ),
    // A request that carries only some parts edits the omitted ones as blank.
    // audit: COACTUPC-064
    fails(
        |f| only_ssn_part(f, 2, "67"),
        "SSN: First 3 chars must be supplied.",
        "ssn_part1",
    ),
    fails(
        |f| {
            with_ssn(f, ["345", "67", ""]);
            f.ssn_part3 = None;
        },
        "SSN Last 4 chars must be supplied.",
        "ssn_part3",
    ),
    // audit: COACTUPC-025
    fails(
        |f| set(&mut f.dob_day, ""),
        "Date of Birth : Day must be supplied.",
        "dob_day",
    ),
    // Today, the fixed clock's date, counts as the future.
    // audit: COACTUPC-025
    // audit: COACTUPC-130
    // audit: CSUTLDTC-026
    fails(
        |f| set_date(dob(f), ["2022", "07", "18"]),
        "Date of Birth:cannot be in the future",
        "dob_year",
    ),
    fails(
        |f| set_date(dob(f), ["2030", "01", "01"]),
        "Date of Birth:cannot be in the future",
        "dob_year",
    ),
    // audit: COACTUPC-026
    // audit: COACTUPC-053
    fails(
        |f| set(&mut f.fico_score, ""),
        "FICO Score must be supplied.",
        "fico_score",
    ),
    fails(
        |f| set(&mut f.fico_score, "7A0"),
        "FICO Score must be all numeric.",
        "fico_score",
    ),
    fails(
        |f| set(&mut f.fico_score, "000"),
        "FICO Score must not be zero.",
        "fico_score",
    ),
    // audit: COACTUPC-026
    // audit: COACTUPC-066
    fails(
        |f| set(&mut f.fico_score, "299"),
        "FICO Score: should be between 300 and 850",
        "fico_score",
    ),
    fails(
        |f| set(&mut f.fico_score, "851"),
        "FICO Score: should be between 300 and 850",
        "fico_score",
    ),
    // audit: COACTUPC-027
    // audit: COACTUPC-050
    fails(
        |f| set(&mut f.first_name, ""),
        "First Name must be supplied.",
        "first_name",
    ),
    // A digit gets the letters text: the alphanumeric edits are never performed.
    // audit: COACTUPC-027
    // audit: COACTUPC-050
    // audit: COACTUPC-051
    fails(
        |f| set(&mut f.first_name, "J0hn"),
        "First Name can have alphabets only.",
        "first_name",
    ),
    // audit: COACTUPC-028
    // audit: COACTUPC-052
    fails(
        |f| set(&mut f.middle_name, "Qu1ncy"),
        "Middle Name can have alphabets only.",
        "middle_name",
    ),
    // audit: COACTUPC-029
    fails(
        |f| set(&mut f.last_name, ""),
        "Last Name must be supplied.",
        "last_name",
    ),
    fails(
        |f| set(&mut f.last_name, "O'Neil"),
        "Last Name can have alphabets only.",
        "last_name",
    ),
    // audit: COACTUPC-030
    // audit: COACTUPC-048
    fails(
        |f| set(&mut f.address_line_1, ""),
        "Address Line 1 must be supplied.",
        "address_line_1",
    ),
    // audit: COACTUPC-031
    fails(
        |f| set(&mut f.state, ""),
        "State must be supplied.",
        "state",
    ),
    fails(
        |f| set(&mut f.state, "I1"),
        "State can have alphabets only.",
        "state",
    ),
    // audit: COACTUPC-031
    // audit: COACTUPC-065
    fails(
        |f| set(&mut f.state, "ZZ"),
        "State: is not a valid state code",
        "state",
    ),
    fails(
        |f| {
            set(&mut f.state, "il");
            set(&mut f.first_name, "Jim");
        },
        "State: is not a valid state code",
        "state",
    ),
    // audit: COACTUPC-032
    // audit: COACTUPC-053
    fails(|f| set(&mut f.zip, ""), "Zip must be supplied.", "zip"),
    fails(
        |f| set(&mut f.zip, "6270A"),
        "Zip must be all numeric.",
        "zip",
    ),
    fails(|f| set(&mut f.zip, "00000"), "Zip must not be zero.", "zip"),
    // audit: COACTUPC-033
    fails(|f| set(&mut f.city, ""), "City must be supplied.", "city"),
    fails(
        |f| set(&mut f.city, "St. Louis"),
        "City can have alphabets only.",
        "city",
    ),
    // audit: COACTUPC-034
    fails(
        |f| set(&mut f.country, ""),
        "Country must be supplied.",
        "country",
    ),
    fails(
        |f| set(&mut f.country, "U5A"),
        "Country can have alphabets only.",
        "country",
    ),
    // audit: COACTUPC-035
    // audit: COACTUPC-055
    // audit: COACTUPC-056
    fails(
        |f| set(&mut f.phone_1_area, ""),
        "Phone Number 1: Area code must be supplied.",
        "phone_1_area",
    ),
    fails(
        |f| set(&mut f.phone_1_area, "3A2"),
        "Phone Number 1: Area code must be A 3 digit number.",
        "phone_1_area",
    ),
    fails(
        |f| set(&mut f.phone_1_area, "000"),
        "Phone Number 1: Area code cannot be zero",
        "phone_1_area",
    ),
    // `555` and `200` are in `VALID-PHONE-AREA-CODE` only, which no edit tests.
    // audit: COACTUPC-057
    fails(
        |f| set(&mut f.phone_1_area, "555"),
        "Phone Number 1: Not valid North America general purpose area code",
        "phone_1_area",
    ),
    fails(
        |f| set(&mut f.phone_1_area, "200"),
        "Phone Number 1: Not valid North America general purpose area code",
        "phone_1_area",
    ),
    // audit: COACTUPC-058
    fails(
        |f| set(&mut f.phone_1_prefix, ""),
        "Phone Number 1: Prefix code must be supplied.",
        "phone_1_prefix",
    ),
    fails(
        |f| set(&mut f.phone_1_prefix, "5B5"),
        "Phone Number 1: Prefix code must be A 3 digit number.",
        "phone_1_prefix",
    ),
    fails(
        |f| set(&mut f.phone_1_prefix, "000"),
        "Phone Number 1: Prefix code cannot be zero",
        "phone_1_prefix",
    ),
    // audit: COACTUPC-059
    fails(
        |f| set(&mut f.phone_1_line, ""),
        "Phone Number 1: Line number code must be supplied.",
        "phone_1_line",
    ),
    fails(
        |f| set(&mut f.phone_1_line, "01C0"),
        "Phone Number 1: Line number code must be A 4 digit number.",
        "phone_1_line",
    ),
    fails(
        |f| set(&mut f.phone_1_line, "0000"),
        "Phone Number 1: Line number code cannot be zero",
        "phone_1_line",
    ),
    // audit: COACTUPC-036
    fails(
        |f| set(&mut f.phone_2_area, "000"),
        "Phone Number 2: Area code cannot be zero",
        "phone_2_area",
    ),
    fails(
        |f| set(&mut f.phone_2_line, ""),
        "Phone Number 2: Line number code must be supplied.",
        "phone_2_line",
    ),
    // audit: COACTUPC-037
    // audit: COACTUPC-053
    fails(
        |f| set(&mut f.eft_account_id, ""),
        "EFT Account Id must be supplied.",
        "eft_account_id",
    ),
    fails(
        |f| set(&mut f.eft_account_id, "00000000X2"),
        "EFT Account Id must be all numeric.",
        "eft_account_id",
    ),
    fails(
        |f| set(&mut f.eft_account_id, "0000000000"),
        "EFT Account Id must not be zero.",
        "eft_account_id",
    ),
    // audit: COACTUPC-038
    // audit: COACTUPC-049
    fails(
        |f| set(&mut f.primary_card_holder, ""),
        "Primary Card Holder must be supplied.",
        "primary_card_holder",
    ),
    fails(
        |f| set(&mut f.primary_card_holder, "0"),
        "Primary Card Holder must be supplied.",
        "primary_card_holder",
    ),
    fails(
        |f| set(&mut f.primary_card_holder, "X"),
        "Primary Card Holder must be Y or N.",
        "primary_card_holder",
    ),
    // audit: COACTUPC-067
    fails(
        |f| set(&mut f.state, "CA"),
        "Invalid zip code for state",
        "state",
    ),
    fails(
        |f| set(&mut f.zip, "10001"),
        "Invalid zip code for state",
        "state",
    ),
    // Two failures: the first in `1200-EDIT-MAP-INPUTS` order is returned.
    // audit: COACTUPC-013
    fails(
        |f| {
            set(&mut f.active_status, "");
            set(&mut f.fico_score, "000");
        },
        "Account Status must be supplied.",
        "active_status",
    ),
    fails(
        |f| {
            set(&mut f.fico_score, "000");
            set(&mut f.first_name, "");
        },
        "FICO Score must not be zero.",
        "fico_score",
    ),
    // The four dates are edited in the order open, expiry, reissue, date of birth.
    // audit: CSUTLDTC-027
    fails(
        |f| {
            set(&mut f.open_year, "");
            set(&mut f.expiry_month, "");
        },
        "Open Date : Year must be supplied.",
        "open_year",
    ),
    fails(
        |f| {
            set(&mut f.expiry_month, "");
            set(&mut f.reissue_day, "32");
        },
        "Expiry Date : Month must be supplied.",
        "expiry_month",
    ),
    fails(
        |f| {
            set(&mut f.reissue_day, "32");
            set(&mut f.dob_day, "");
        },
        "Reissue Date:day must be a number between 1 and 31.",
        "reissue_day",
    ),
    // The state and ZIP cross edit runs last: a later field's failure is returned instead.
    // audit: COACTUPC-067
    fails(
        |f| {
            set(&mut f.state, "CA");
            set(&mut f.eft_account_id, "");
        },
        "EFT Account Id must be supplied.",
        "eft_account_id",
    ),
];

/// Changes to the echo of account [`CLEAN`] that pass every edit: endpoint 6 validates them
/// and writes nothing.
const PASSING_CHANGES: &[Touch] = &[
    // audit: COACTUPC-015
    // audit: COACTUPC-038
    // audit: COACTUPC-049
    |f| {
        set(&mut f.active_status, "N");
        set(&mut f.primary_card_holder, "N");
    },
    // No edit runs on these four fields, whatever they hold.
    // audit: COACTUPC-039
    // audit: COACTUPC-051
    |f| {
        set(&mut f.group_id, "X1");
        set(&mut f.government_id, "@@@");
        set(&mut f.address_line_2, "#7 & 8");
    },
    // audit: COACTUPC-028
    // audit: COACTUPC-052
    |f| set(&mut f.middle_name, ""),
    // audit: COACTUPC-030
    // audit: COACTUPC-048
    |f| set(&mut f.address_line_1, "12 #$% Rd"),
    // A phone number is optional: three blank parts are valid and no part is edited.
    // audit: COACTUPC-055
    |f| {
        set(&mut f.phone_2_area, "");
        set(&mut f.phone_2_prefix, "");
        set(&mut f.phone_2_line, "");
    },
    // audit: COACTUPC-057
    |f| set(&mut f.phone_1_area, "212"),
    // A day typed with another fill is a change, and its `NUMVAL` passes.
    // audit: COACTUPC-040
    // audit: COACTUPC-125
    |f| set(&mut f.open_day, " 3"),
    |f| set(&mut f.open_day, "3."),
    // audit: COACTUPC-123
    |f| set(&mut f.open_month, "01"),
    // audit: COACTUPC-128
    |f| set_date(open_date(f), ["2000", "02", "29"]),
    |f| set_date(open_date(f), ["2024", "02", "29"]),
    // audit: COACTUPC-130
    |f| set_date(dob(f), ["2022", "07", "17"]),
    // audit: COACTUPC-066
    |f| set(&mut f.fico_score, "300"),
    |f| set(&mut f.fico_score, "850"),
    // audit: COACTUPC-065
    // audit: COACTUPC-067
    |f| {
        set(&mut f.state, "CA");
        set(&mut f.zip, "94105");
    },
    // Re-entered SSN parts other than the stored SSN are a change that passes.
    // audit: COACTUPC-044
    // audit: COACTUPC-064
    |f| with_ssn(f, REENTERED_SSN_PARTS),
];

/// Echoes of account [`CLEAN`] that equal its snapshot term by term: 400 with
/// `No change detected with respect to values fetched.`, and no edit runs.
const UNCHANGED_ECHOES: &[Touch] = &[
    // audit: COACTUPC-040
    // audit: COACTUPC-041
    // audit: COACTUPC-043
    |_| {},
    // The status and group ID compare without letter case, the group ID also trimmed.
    // audit: COACTUPC-040
    |f| set(&mut f.active_status, "y"),
    |f| set(&mut f.group_id, "gold"),
    |f| set(&mut f.group_id, "  GOLD"),
    // audit: COACTUPC-041
    |f| set(&mut f.first_name, "JOHN"),
    |f| set(&mut f.city, " springfield"),
    // Re-entered SSN parts equal to the stored SSN are no change.
    // audit: COACTUPC-044
    |f| with_ssn(f, TEST_SSN_PARTS),
];

/// The enterable members of `AccountUpdateFields` and their map `LENGTH`: one more character
/// is refused by the request DTO with 400 and the member's path (`screens.md`, the server half
/// of each `both` row).
const MAP_LENGTHS: &[(&str, usize)] = &[
    // audit: COACTUP-016
    ("active_status", 1),
    // audit: COACTUP-019
    ("open_year", 4),
    // audit: COACTUP-021
    ("open_month", 2),
    // audit: COACTUP-023
    ("open_day", 2),
    // audit: COACTUP-026
    ("credit_limit", 15),
    // audit: COACTUP-029
    ("expiry_year", 4),
    // audit: COACTUP-031
    ("expiry_month", 2),
    // audit: COACTUP-033
    ("expiry_day", 2),
    // audit: COACTUP-036
    ("cash_credit_limit", 15),
    // audit: COACTUP-039
    ("reissue_year", 4),
    // audit: COACTUP-041
    ("reissue_month", 2),
    // audit: COACTUP-043
    ("reissue_day", 2),
    // audit: COACTUP-046
    ("current_balance", 15),
    // audit: COACTUP-049
    ("current_cycle_credit", 15),
    // audit: COACTUP-052
    ("group_id", 10),
    // audit: COACTUP-055
    ("current_cycle_debit", 15),
    // audit: COACTUP-059
    ("customer_id", 9),
    // audit: COACTUP-062
    ("ssn_part1", 3),
    // audit: COACTUP-064
    ("ssn_part2", 2),
    // audit: COACTUP-066
    ("ssn_part3", 4),
    // audit: COACTUP-069
    ("dob_year", 4),
    // audit: COACTUP-071
    ("dob_month", 2),
    // audit: COACTUP-073
    ("dob_day", 2),
    // audit: COACTUP-076
    ("fico_score", 3),
    // audit: COACTUP-081
    ("first_name", 25),
    // audit: COACTUP-083
    ("middle_name", 25),
    // audit: COACTUP-085
    ("last_name", 25),
    // audit: COACTUP-087
    ("address_line_1", 50),
    // audit: COACTUP-090
    ("state", 2),
    // audit: COACTUP-092
    ("address_line_2", 50),
    // audit: COACTUP-095
    ("zip", 5),
    // audit: COACTUP-098
    ("city", 50),
    // audit: COACTUP-101
    ("country", 3),
    // audit: COACTUP-104
    ("phone_1_area", 3),
    // audit: COACTUP-105
    ("phone_1_prefix", 3),
    // audit: COACTUP-106
    ("phone_1_line", 4),
    // audit: COACTUP-109
    ("government_id", 20),
    // audit: COACTUP-112
    ("phone_2_area", 3),
    // audit: COACTUP-113
    ("phone_2_prefix", 3),
    // audit: COACTUP-114
    ("phone_2_line", 4),
    // audit: COACTUP-117
    ("eft_account_id", 10),
    // audit: COACTUP-120
    ("primary_card_holder", 1),
];

#[test]
fn coactupc_field_edits() -> TestResult {
    current_thread()?.block_on(async {
        let harness = Harness::start().await?;
        edits_change_check(&harness).await?;
        edits_failing(&harness).await?;
        edits_passing(&harness).await?;
        edits_keys_and_states(&harness).await?;
        edits_map_lengths(&harness).await?;
        edits_unstorable_text(&harness).await?;
        // Every request above was ENTER, a key processed as ENTER, or a restart: nothing written.
        // audit: COACTUPC-013
        // audit: COACTUPC-014
        // audit: COACTUPC-070
        assert!(
            harness.account(CLEAN).await? == test_account(CLEAN, "GOLD")?,
            "a validation rewrote the account"
        );
        assert!(
            harness.customer(CLEAN).await? == test_customer(CLEAN, "62701")?,
            "a validation rewrote the customer"
        );
        Ok(())
    })
}

/// Endpoint 6, ENTER on each echo of `id`'s fetch, with `touch` applied.
async fn enter(h: &Harness, id: u64, touch: Touch) -> Fallible<Reply> {
    let (_, mut fields, snapshot) = h.fetched(id).await?;
    touch(&mut fields);
    h.save(&acct(id), &update_body("ENTER", &fields, &snapshot)?)
        .await
}

/// `1205-COMPARE-OLD-NEW` on the test accounts and two fixture accounts (COACTUPC-040 to
/// COACTUPC-044, COACTUPC-064).
async fn edits_change_check(h: &Harness) -> TestResult {
    for (index, &touch) in UNCHANGED_ECHOES.iter().enumerate() {
        let reply = enter(h, CLEAN, touch).await?;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "unchanged echo {index}"
        );
        assert_screen_error(
            &reply,
            StatusCode::BAD_REQUEST,
            NO_CHANGE,
            Some("active_status"),
        );
    }
    // A blank group ID is received as low-values, which equal no stored blank: a change, and
    // the edits run and pass.
    // audit: COACTUPC-007
    // audit: COACTUPC-042
    assert_validated(&enter(h, BLANK_GROUP, |_| {}).await?);
    // A stored ZIP+4 never equals the five characters shown and sent back.
    // audit: COACTUPC-041
    assert_validated(&enter(h, ZIP_PLUS_4, |_| {}).await?);
    // Fixture account 1, unchanged: its blank group ID is a change, and its customer's FICO
    // score, 274, is the first edit that fails.
    let fixture_1 = enter(h, 1, |_| {}).await?;
    // audit: COACTUPC-026
    // audit: COACTUPC-042
    // audit: COACTUPC-066
    let message = "FICO Score: should be between 300 and 850";
    assert_screen_error(
        &fixture_1,
        StatusCode::BAD_REQUEST,
        message,
        Some("fico_score"),
    );
    // Fixture account 12, unchanged and the SSN parts omitted: the edits run on the stored
    // SSN's parts, whose first part is a forbidden value.
    let fixture_12 = enter(h, 12, |_| {}).await?;
    // audit: COACTUPC-061
    // audit: COACTUPC-064
    let message = "SSN: First 3 chars: should not be 000, 666, or between 900 and 999";
    assert_screen_error(
        &fixture_12,
        StatusCode::BAD_REQUEST,
        message,
        Some("ssn_part1"),
    );
    assert_eq!(acct(12), ACCOUNT_12);
    Ok(())
}

/// Every failing edit of [`EDIT_CASES`], on ENTER with a change (COACTUPC-013, COACTUPC-015 to
/// COACTUPC-038, COACTUPC-048 to COACTUPC-067, COACTUPC-117 to COACTUPC-130).
async fn edits_failing(h: &Harness) -> TestResult {
    let (_, clean, snapshot) = h.fetched(CLEAN).await?;
    for (index, case) in EDIT_CASES.iter().enumerate() {
        let mut fields = clean.clone();
        (case.touch)(&mut fields);
        let reply = h
            .save(&acct(CLEAN), &update_body("ENTER", &fields, &snapshot)?)
            .await?;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "edit case {index}: {}",
            reply.body
        );
        assert_eq!(reply.message(), case.message, "edit case {index}");
        assert_eq!(reply.body["field"], case.field, "edit case {index}");
        assert_screen_error(
            &reply,
            StatusCode::BAD_REQUEST,
            case.message,
            Some(case.field),
        );
    }
    Ok(())
}

/// The changes of [`PASSING_CHANGES`], and the amount texts `TEST-NUMVAL-C` accepts with the
/// values the screen then shows (COACTUPC-009, COACTUPC-014, COACTUPC-054, COACTUPC-078,
/// CSUTLDTC-002, CSUTLDTC-003, CSUTLDTC-005, CSUTLDTC-019, CSUTLDTC-020).
async fn edits_passing(h: &Harness) -> TestResult {
    for (index, &touch) in PASSING_CHANGES.iter().enumerate() {
        let reply = enter(h, CLEAN, touch).await?;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "passing change {index}: {}",
            reply.body
        );
        assert_validated(&reply);
    }
    let reply = enter(h, CLEAN, |f| {
        set(&mut f.credit_limit, "$1,234.56");
        set(&mut f.cash_credit_limit, "1,234.56");
        set(&mut f.current_balance, "-50");
        set(&mut f.current_cycle_credit, "50CR");
        set(&mut f.current_cycle_debit, "50-");
    })
    .await?;
    let amounts = &reply.body["amounts"];
    // audit: COACTUPC-009
    // audit: COACTUPC-014
    // audit: COACTUPC-054
    // audit: COACTUPC-078
    assert_validated(&reply);
    assert_eq!(amounts["credit_limit"], "+      1,234.56");
    assert_eq!(amounts["cash_credit_limit"], "+      1,234.56");
    assert_eq!(amounts["current_balance"], "-         50.00");
    assert_eq!(amounts["current_cycle_credit"], "-         50.00");
    assert_eq!(amounts["current_cycle_debit"], "-         50.00");

    // An amount left as fetched is shown as its fetched string.
    let reply = enter(h, CLEAN, |f| set(&mut f.first_name, "Jim")).await?;
    // audit: COACTUPC-078
    assert_validated(&reply);
    assert_eq!(reply.body["amounts"]["current_balance"], "+      1,234.56");
    assert_eq!(
        reply.body["amounts"]["current_cycle_debit"],
        "-         25.50"
    );
    // The four dates of that request passed the part edits and the CSUTLDTC call, one call per
    // date with the same working storage; no byte of the 80-byte result reaches the response.
    // audit: CSUTLDTC-002
    // audit: CSUTLDTC-003
    // audit: CSUTLDTC-005
    // audit: CSUTLDTC-019
    // audit: CSUTLDTC-020
    let text = reply.body.to_string();
    for label in [
        "Mesg Code:",
        "TstDate:",
        "Mask used:",
        "Date is valid",
        "YYYYMMDD",
    ] {
        assert!(
            !text.contains(label),
            "the response holds the result text {label:?}"
        );
    }
    Ok(())
}

/// The keys and echoed states of `2000-DECIDE-ACTION` that write nothing (COACTUPC-002,
/// COACTUPC-004, COACTUPC-012, COACTUPC-043, COACTUPC-070, COACTUPC-079, COACTUPC-082).
async fn edits_keys_and_states(h: &Harness) -> TestResult {
    let (_, clean, snapshot) = h.fetched(CLEAN).await?;
    let mut changed = clean.clone();
    set(&mut changed.first_name, "Jim");
    let mut refused = clean.clone();
    set(&mut refused.fico_score, "000");
    let path = acct(CLEAN);

    // In state `N` an ENTER runs no edit: a change, even one an edit would refuse, is
    // validated again; an echo equal to the snapshot gets the no-change message.
    let again = h
        .save(
            &path,
            &update_body("ENTER", &refused, &in_state(&snapshot, "N"))?,
        )
        .await?;
    let unchanged = h
        .save(
            &path,
            &update_body("ENTER", &clean, &in_state(&snapshot, "N"))?,
        )
        .await?;
    // audit: COACTUPC-012
    // audit: COACTUPC-070
    assert_validated(&again);
    // audit: COACTUPC-012
    // audit: COACTUPC-043
    assert_screen_error(
        &unchanged,
        StatusCode::BAD_REQUEST,
        NO_CHANGE,
        Some("active_status"),
    );

    // F5 outside state `N`, and every key text but ENTER and PF5, are processed as ENTER.
    for (aid, state) in [
        ("PF5", "S"),
        ("PF5", "E"),
        ("PF12", "S"),
        ("PF3", "E"),
        ("XYZ", "S"),
    ] {
        let snapshot = in_state(&snapshot, state);
        let passed = h
            .save(&path, &update_body(aid, &changed, &snapshot)?)
            .await?;
        let failed = h
            .save(&path, &update_body(aid, &refused, &snapshot)?)
            .await?;
        // audit: COACTUPC-004
        assert_validated(&passed);
        // audit: COACTUPC-004
        // audit: COACTUPC-013
        let message = "FICO Score must not be zero.";
        assert_screen_error(
            &failed,
            StatusCode::BAD_REQUEST,
            message,
            Some("fico_score"),
        );
    }

    // A screen left in state `C`, `L`, `F` or not fetched starts over, whatever the key.
    for state in ["C", "L", "F", " ", ""] {
        let reply = h
            .save(
                &path,
                &update_body("PF5", &changed, &in_state(&snapshot, state))?,
            )
            .await?;
        // audit: COACTUPC-079
        // audit: COACTUPC-082
        assert_eq!(reply.status, StatusCode::OK, "state {state:?}");
        assert_eq!(reply.body["message"], SEARCH_KEYS);
        assert_eq!(reply.body["state"], " ");
        assert_eq!(reply.body["save_enabled"], false);
        assert_eq!(reply.body["cancel_enabled"], false);
        assert_eq!(reply.body["committed"], false);
        assert!(
            reply.body["amounts"].is_null(),
            "a restart shows no amounts"
        );
    }

    // No paragraph reads the user type: both tokens get the same answer.
    let payload = update_body("ENTER", &changed, &snapshot)?;
    let as_user = h.save(&path, &payload).await?;
    let as_admin = h.save_as(&h.state, &h.admin, &path, &payload).await?;
    let unsigned = h
        .call(
            &h.state,
            Method::PUT,
            &format!("/api/v1/accounts/{path}"),
            None,
            Some(&payload),
        )
        .await?;
    // audit: COACTUPC-002
    assert_validated(&as_user);
    assert_eq!(
        as_user.body, as_admin.body,
        "endpoint 6 answers every user type alike"
    );
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// One character over each member's map `LENGTH`: 400 with the member's path, before any edit
/// runs (`screens.md` COACTUP-016 to COACTUP-120, the server half).
async fn edits_map_lengths(h: &Harness) -> TestResult {
    let (_, clean, snapshot) = h.fetched(CLEAN).await?;
    for &(name, length) in MAP_LENGTHS {
        let mut payload = update_body("ENTER", &clean, &snapshot)?;
        payload
            .get_mut("fields")
            .and_then(Value::as_object_mut)
            .ok_or("the body has no fields")?
            .insert(name.to_owned(), Value::from("X".repeat(length + 1)));
        let reply = h.save(&acct(CLEAN), &payload).await?;
        let path = format!("fields.{name}");
        assert_screen_error(
            &reply,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            Some(&path),
        );
    }
    Ok(())
}

/// Texts within their member's map `LENGTH` in characters that its `PIC X(n)` field cannot
/// hold byte for byte as printable ASCII: an accented letter, a TAB, an emoji (X-COUSR01C-3,
/// PIC-002).
const UNSTORABLE_TEXTS: &[(&str, &str)] = &[
    ("government_id", "PASSPORTé"),
    ("address_line_2", "Café 9"),
    ("city", "SPRING\tFIELD"),
    ("group_id", "GRPé"),
    ("address_line_1", "Emoji😀"),
];

/// A text of [`UNSTORABLE_TEXTS`] in its member, on ENTER and on F5 in state `N`: 400 with the
/// request validation's message, naming the field as the member's over-long rejection names it,
/// before any edit runs, and nothing written.
async fn edits_unstorable_text(h: &Harness) -> TestResult {
    let (_, clean, snapshot) = h.fetched(CLEAN).await?;
    let account = h.account(CLEAN).await?;
    let customer = h.customer(CLEAN).await?;
    let body_with = |member: &str, text: &str, aid: &str, state: &str| -> Fallible<Value> {
        let mut payload = update_body(aid, &clean, &in_state(&snapshot, state))?;
        payload
            .get_mut("fields")
            .and_then(Value::as_object_mut)
            .ok_or("the body has no fields")?
            .insert(member.to_owned(), Value::from(text));
        Ok(payload)
    };
    for &(member, text) in UNSTORABLE_TEXTS {
        let width = MAP_LENGTHS
            .iter()
            .find(|&&(name, _)| name == member)
            .map(|&(_, width)| width)
            .ok_or("the member has no map LENGTH")?;
        let over_long = "X".repeat(width + 1);
        let length = h
            .save(&acct(CLEAN), &body_with(member, &over_long, "ENTER", "S")?)
            .await?;
        let field = length.text("field").to_owned();
        assert!(field.ends_with(member), "field of {member}: {field:?}");
        assert_screen_error(
            &length,
            StatusCode::BAD_REQUEST,
            FIELD_LENGTH_MESSAGE,
            Some(&field),
        );
        for (aid, state) in [("ENTER", "S"), ("PF5", "N")] {
            let reply = h
                .save(&acct(CLEAN), &body_with(member, text, aid, state)?)
                .await?;
            assert_screen_error(
                &reply,
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some(&field),
            );
            assert!(
                !reply.body.to_string().contains(text),
                "the {aid} reply for {member} holds the rejected text"
            );
            assert_stored(h, CLEAN, &account, &customer).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Endpoint 6, F5: the save of COACTUPC-101 to COACTUPC-116
// ---------------------------------------------------------------------------------------------

/// The saves of endpoint 6, in a fixed order on one database: each step starts from what the
/// steps before it stored.
#[test]
fn coactupc_save_rows() -> TestResult {
    current_thread()?.block_on(async {
        let harness = Harness::start().await?;
        save_refusals(&harness).await?;
        save_round_trip(&harness).await?;
        save_reentered_ssn(&harness).await?;
        save_change_check(&harness).await?;
        save_other_user_terms(&harness).await?;
        save_lock_failures(&harness).await?;
        save_rewrite_failures(&harness).await?;
        save_narrowed_zip(&harness).await?;
        Ok(())
    })
}

/// Account `id` as fetched now with `touch` applied, validated with ENTER: the fields the screen
/// sends back with F5, each amount as the validation showed it (COACTUPC-078, COACTUPC-109),
/// and the fetch's snapshot.
async fn validated(
    h: &Harness,
    id: u64,
    touch: Touch,
) -> Fallible<(AccountUpdateFields, AccountUpdateSnapshot)> {
    let (_, mut fields, snapshot) = h.fetched(id).await?;
    touch(&mut fields);
    let reply = h
        .save(&acct(id), &update_body("ENTER", &fields, &snapshot)?)
        .await?;
    assert_validated(&reply);
    let amounts = &reply.body["amounts"];
    fields.credit_limit = member(amounts, "credit_limit")?;
    fields.cash_credit_limit = member(amounts, "cash_credit_limit")?;
    fields.current_balance = member(amounts, "current_balance")?;
    fields.current_cycle_credit = member(amounts, "current_cycle_credit")?;
    fields.current_cycle_debit = member(amounts, "current_cycle_debit")?;
    Ok((fields, snapshot))
}

/// F5 on account `id` with `fields`, the client echoing `snapshot` in state `N`.
async fn confirm(
    h: &Harness,
    id: u64,
    fields: &AccountUpdateFields,
    snapshot: &AccountUpdateSnapshot,
) -> Fallible<Reply> {
    h.save(
        &acct(id),
        &update_body("PF5", fields, &in_state(snapshot, "N"))?,
    )
    .await
}

/// The stored account `id` equals `account` and its customer equals `customer`. The customer
/// comparison prints nothing of the records, which hold an SSN.
async fn assert_stored(
    h: &Harness,
    id: u64,
    account: &AccountRecord,
    customer: &CustomerRecord,
) -> TestResult {
    assert_eq!(&h.account(id).await?, account, "stored account {id}");
    let customer_id = customer.id.get();
    let stored = h.customer(customer_id).await?;
    assert!(
        &stored == customer,
        "stored customer {customer_id} differs from the expected one"
    );
    Ok(())
}

/// Another user's rewrite of account `record.id`, between a fetch and its save.
async fn rewrite_account(h: &Harness, record: &AccountRecord) -> TestResult {
    let mut store = h.store.clone();
    assert!(
        AccountRepository::update(&mut store, record).await?,
        "account rewrite"
    );
    Ok(())
}

/// Another user's rewrite of customer `record.id`, between a fetch and its save.
async fn rewrite_customer(h: &Harness, record: &CustomerRecord) -> TestResult {
    let mut store = h.store.clone();
    assert!(
        CustomerRepository::update(&mut store, record).await?,
        "customer rewrite"
    );
    Ok(())
}

/// F5 in state `N` with fields a 3270 cannot send there: no change, or a change an edit
/// refuses. Either is answered as an ENTER in state `S` would be, and nothing is written
/// (COACTUPC-071, COACTUPC-110).
async fn save_refusals(h: &Harness) -> TestResult {
    let (_, clean, snapshot) = h.fetched(CLEAN).await?;
    let mut refused = clean.clone();
    set(&mut refused.fico_score, "000");
    let unchanged = confirm(h, CLEAN, &clean, &snapshot).await?;
    let failing = confirm(h, CLEAN, &refused, &snapshot).await?;
    // audit: COACTUPC-071
    // audit: COACTUPC-110
    assert_screen_error(
        &unchanged,
        StatusCode::BAD_REQUEST,
        NO_CHANGE,
        Some("active_status"),
    );
    // audit: COACTUPC-110
    let message = "FICO Score must not be zero.";
    assert_screen_error(
        &failing,
        StatusCode::BAD_REQUEST,
        message,
        Some("fico_score"),
    );
    assert_stored(
        h,
        CLEAN,
        &test_account(CLEAN, "GOLD")?,
        &test_customer(CLEAN, "62701")?,
    )
    .await
}

/// The changes of the first save of account [`CLEAN`]: no field blank, the SSN parts omitted,
/// and a credit limit with ten integer digits.
const FIRST_SAVE: Touch = |f| {
    set(&mut f.active_status, "N");
    set(&mut f.credit_limit, "1234567890.12");
    set(&mut f.cash_credit_limit, "$2,000.00");
    set(&mut f.current_balance, "-50");
    set_date(open_date(f), ["2015", "01", "02"]);
    set(&mut f.group_id, "PLATINUM");
    set(&mut f.first_name, "Jane");
    set(&mut f.last_name, "Doe");
    set(&mut f.zip, "60601");
    set(&mut f.phone_1_area, "212");
    set(&mut f.phone_1_line, "0123");
    set_date(dob(f), ["1979", "12", "31"]);
    set(&mut f.primary_card_holder, "N");
    set(&mut f.fico_score, "800");
};

/// A validation, then F5: both records rewritten as audited, every field not edited kept, the
/// SSN kept when its parts are omitted (COACTUPC-064, COACTUPC-071, COACTUPC-078, COACTUPC-103,
/// COACTUPC-104, COACTUPC-108, COACTUPC-109, COACTUPC-111, COACTUPC-112, COACTUPC-131,
/// CSUTLDTC-001).
async fn save_round_trip(h: &Harness) -> TestResult {
    let (fields, snapshot) = validated(h, CLEAN, FIRST_SAVE).await?;
    // The screen shows nine integer digits, so F5 sends the limit without its tenth.
    // audit: COACTUPC-109
    assert_eq!(fields.credit_limit, "+234,567,890.12");
    assert_eq!(fields.cash_credit_limit, "+      2,000.00");
    let reply = confirm(h, CLEAN, &fields, &snapshot).await?;
    // audit: COACTUPC-071
    // audit: COACTUPC-078
    // audit: COACTUPC-108
    assert_committed(&reply);
    assert_eq!(reply.body["amounts"]["credit_limit"], "+234,567,890.12");
    assert_eq!(reply.body["amounts"]["current_balance"], "-         50.00");

    // The account takes the status, the amounts and the dates received; the group ID received
    // lands in the ZIP field, and the stored group ID and filler stay.
    let account = AccountRecord {
        active_status: PicX::from_text("N")?,
        curr_bal: money(-5_000)?,
        credit_limit: money(23_456_789_012)?,
        cash_credit_limit: money(200_000)?,
        open_date: PicDate::new(Some(day(2015, 1, 2)?))?,
        addr_zip: PicX::from_text("PLATINUM")?,
        ..test_account(CLEAN, "GOLD")?
    };
    // The customer of the path's cross-reference takes every field received: the ZIP as five
    // characters and five spaces, the phone number as `(AAA)BBB-CCCC` and two spaces.
    let customer = CustomerRecord {
        first_name: PicX::from_text("Jane")?,
        last_name: PicX::from_text("Doe")?,
        addr_zip: PicX::from_text("60601")?,
        phone_num_1: PicX::from_text("(212)555-0123")?,
        dob_yyyy_mm_dd: PicDate::new(Some(day(1979, 12, 31)?))?,
        pri_card_holder_ind: PicX::from_text("N")?,
        fico_credit_score: PicU::new(800)?,
        ..test_customer(CLEAN, "62701")?
    };
    // audit: COACTUPC-103
    // audit: COACTUPC-104
    // audit: COACTUPC-109
    // audit: COACTUPC-111
    // audit: COACTUPC-112
    // audit: COACTUPC-131
    // audit: CSUTLDTC-001
    assert_stored(h, CLEAN, &account, &customer).await?;
    // Omitted SSN parts keep the stored SSN, compared through its last four digits.
    let stored = h.customer(CLEAN).await?;
    // audit: COACTUPC-064
    assert_eq!(stored.ssn.last4(), *b"8912");
    assert_eq!(stored.ssn.redacted(), "***-**-8912");

    // The next fetch shows what was stored: the limit as saved, the stored group ID.
    let view = h.fetch(&acct(CLEAN)).await?;
    // audit: COACTUPC-103
    assert_eq!(view.status, StatusCode::OK);
    assert_eq!(view.body["credit_limit"], "+234,567,890.12");
    assert_eq!(view.body["group_id"], "GOLD      ");
    assert_eq!(view.body["ssn"], "***-**-8912");
    Ok(())
}

/// A save with re-entered SSN parts and blank optional fields: the SSN received is stored, and
/// each blank field, the group ID included, is written as spaces (COACTUPC-064, COACTUPC-104,
/// COACTUPC-105).
async fn save_reentered_ssn(h: &Harness) -> TestResult {
    let (fields, snapshot) = validated(h, BLANK_GROUP, |f| {
        with_ssn(f, REENTERED_SSN_PARTS);
        set(&mut f.middle_name, "");
        set(&mut f.address_line_2, "");
        set(&mut f.government_id, "");
        set(&mut f.phone_2_area, "");
        set(&mut f.phone_2_prefix, "");
        set(&mut f.phone_2_line, "");
    })
    .await?;
    assert_committed(&confirm(h, BLANK_GROUP, &fields, &snapshot).await?);
    let account = AccountRecord {
        addr_zip: PicX::spaces(),
        ..test_account(BLANK_GROUP, "")?
    };
    let customer = CustomerRecord {
        middle_name: PicX::spaces(),
        addr_line_2: PicX::spaces(),
        govt_issued_id: PicX::spaces(),
        phone_num_2: PicX::from_text("(   )   -")?,
        ssn: Ssn::parse(REENTERED_SSN)?,
        ..test_customer(BLANK_GROUP, "62701")?
    };
    // audit: COACTUPC-064
    // audit: COACTUPC-104
    // audit: COACTUPC-105
    assert_stored(h, BLANK_GROUP, &account, &customer).await?;
    assert_eq!(h.customer(BLANK_GROUP).await?.ssn.redacted(), "***-**-8912");
    Ok(())
}

/// The 409 of `9700-CHECK-CHANGE-IN-REC`: F5 on a snapshot an earlier save made stale, then the
/// screen that 409 leaves, and a snapshot whose SSN term alone differs (COACTUPC-113 to
/// COACTUPC-116, E32). Each stored term another user changes alone is a case of
/// `coactupc_change_check_terms`.
async fn save_change_check(h: &Harness) -> TestResult {
    let (_, shown, stale) = h.fetched(CLEAN).await?;
    let mut first = shown.clone();
    set(&mut first.first_name, "Janet");
    assert_committed(&confirm(h, CLEAN, &first, &stale).await?);
    let account = h.account(CLEAN).await?;
    let customer = h.customer(CLEAN).await?;

    // The second F5 echoes the snapshot fetched before the first save.
    let mut second = shown.clone();
    set(&mut second.last_name, "Smith");
    let conflict = confirm(h, CLEAN, &second, &stale).await?;
    // audit: COACTUPC-071
    // audit: COACTUPC-114
    // audit: COACTUPC-116
    assert_screen_error(&conflict, StatusCode::CONFLICT, CHANGED, None);
    assert_stored(h, CLEAN, &account, &customer).await?;

    // That screen shows the stale snapshot again in state `S`: sent back as shown, it is no
    // change; F5 there is ENTER, which validates a change and neither saves nor answers 409.
    let path = acct(CLEAN);
    let again = h
        .save(
            &path,
            &update_body("ENTER", &shown, &in_state(&stale, "S"))?,
        )
        .await?;
    let f5 = h
        .save(&path, &update_body("PF5", &second, &in_state(&stale, "S"))?)
        .await?;
    // audit: COACTUPC-116
    assert_screen_error(
        &again,
        StatusCode::BAD_REQUEST,
        NO_CHANGE,
        Some("active_status"),
    );
    assert_validated(&f5);
    assert_stored(h, CLEAN, &account, &customer).await?;

    // A snapshot whose SSN last four digits differ from the stored ones is a change by another
    // user, every other term being equal.
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    set(&mut fields.fico_score, "801");
    let other_last4 = AccountUpdateSnapshot {
        ssn_last4: "0000".to_owned(),
        ..snapshot
    };
    let conflict = confirm(h, CLEAN, &fields, &other_last4).await?;
    // audit: COACTUPC-115
    assert_screen_error(&conflict, StatusCode::CONFLICT, CHANGED, None);
    assert_stored(h, CLEAN, &account, &customer).await
}

/// Another user's rewrite between the fetch and F5: the SSN's first five digits and the group
/// ID's letter case go unnoticed, the status's letter case does not (COACTUPC-113, COACTUPC-115).
/// Every other compared term, and the customer terms compared after `UPPER-CASE`, are the cases
/// of `coactupc_change_check_terms`.
async fn save_other_user_terms(h: &Harness) -> TestResult {
    // The stored SSN changes in its first five digits only: the save goes through and, its
    // parts omitted, keeps that SSN (E32).
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    let customer = CustomerRecord {
        ssn: Ssn::parse(OTHER_USER_SSN)?,
        ..h.customer(CLEAN).await?
    };
    rewrite_customer(h, &customer).await?;
    set(&mut fields.fico_score, "802");
    // audit: COACTUPC-115
    assert_committed(&confirm(h, CLEAN, &fields, &snapshot).await?);
    let stored = h.customer(CLEAN).await?;
    assert!(
        stored.ssn == Ssn::parse(OTHER_USER_SSN)?,
        "the other user's SSN is kept"
    );
    assert_eq!(stored.fico_credit_score, PicU::new(802)?);

    // The group ID compares after `LOWER-CASE`.
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    let lower = AccountRecord {
        group_id: PicX::from_text("gold")?,
        ..h.account(CLEAN).await?
    };
    rewrite_account(h, &lower).await?;
    set(&mut fields.fico_score, "803");
    // audit: COACTUPC-113
    assert_committed(&confirm(h, CLEAN, &fields, &snapshot).await?);
    assert_eq!(h.customer(CLEAN).await?.fico_credit_score, PicU::new(803)?);

    // The status compares exactly.
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    let stored = h.account(CLEAN).await?;
    let lowered = AccountRecord {
        active_status: PicX::from_text("n")?,
        ..stored.clone()
    };
    rewrite_account(h, &lowered).await?;
    let customer = h.customer(CLEAN).await?;
    set(&mut fields.fico_score, "804");
    let conflict = confirm(h, CLEAN, &fields, &snapshot).await?;
    // audit: COACTUPC-113
    assert_screen_error(&conflict, StatusCode::CONFLICT, CHANGED, None);
    assert_stored(h, CLEAN, &lowered, &customer).await?;
    rewrite_account(h, &stored).await
}

/// A read for update that finds no row or cannot have its lock: 500 with the lock message and
/// nothing written (COACTUPC-071, COACTUPC-101, COACTUPC-102).
async fn save_lock_failures(h: &Harness) -> TestResult {
    // Account 63's cross-reference names customer 60, and no account 63 is stored.
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    set(&mut fields.fico_score, "805");
    let customer = h.customer(CLEAN).await?;
    let missing = h
        .save(
            &acct(NO_ACCOUNT),
            &update_body("PF5", &fields, &in_state(&snapshot, "N"))?,
        )
        .await?;
    // audit: COACTUPC-071
    // audit: COACTUPC-101
    assert_screen_error(
        &missing,
        StatusCode::INTERNAL_SERVER_ERROR,
        LOCK_ACCOUNT,
        None,
    );
    assert!(
        h.customer(CLEAN).await? == customer,
        "customer {CLEAN} was rewritten"
    );
    let mut store = h.store.clone();
    let absent = AccountRepository::find_by_id(&mut store, PicU::new(NO_ACCOUNT)?).await?;
    assert!(absent.is_none(), "account {NO_ACCOUNT} was written");

    // Another transaction holds the row lock, and the API's statements give up waiting.
    let (fields, snapshot) = validated(h, BLANK_GROUP, |f| set(&mut f.fico_score, "777")).await?;
    let account = h.account(BLANK_GROUP).await?;
    let customer = h.customer(BLANK_GROUP).await?;
    let impatient = h.impatient_state().await?;
    let payload = update_body("PF5", &fields, &in_state(&snapshot, "N"))?;
    let key = PicU::<11>::new(BLANK_GROUP)?;

    let mut holder = h.store.begin().await?;
    let held = AccountRepository::lock_by_id(&mut holder, key).await?;
    let reply = h
        .save_as(&impatient, &h.user, &acct(BLANK_GROUP), &payload)
        .await?;
    holder.rollback().await?;
    assert!(held.is_some(), "the holder locked account {BLANK_GROUP}");
    // audit: COACTUPC-101
    assert_screen_error(
        &reply,
        StatusCode::INTERNAL_SERVER_ERROR,
        LOCK_ACCOUNT,
        None,
    );
    assert_stored(h, BLANK_GROUP, &account, &customer).await?;

    let mut holder = h.store.begin().await?;
    let held = CustomerRepository::lock_by_id(&mut holder, PicU::<9>::new(BLANK_GROUP)?).await?;
    let reply = h
        .save_as(&impatient, &h.user, &acct(BLANK_GROUP), &payload)
        .await?;
    holder.rollback().await?;
    assert!(held.is_some(), "the holder locked customer {BLANK_GROUP}");
    // audit: COACTUPC-071
    // audit: COACTUPC-102
    assert_screen_error(
        &reply,
        StatusCode::INTERNAL_SERVER_ERROR,
        LOCK_CUSTOMER,
        None,
    );
    assert_stored(h, BLANK_GROUP, &account, &customer).await
}

/// A save that cannot store a value, and one whose customer rewrite fails after the account
/// rewrite (COACTUPC-106, COACTUPC-107, COACTUPC-111, COACTUPC-112).
async fn save_rewrite_failures(h: &Harness) -> TestResult {
    let account = h.account(ZIP_PLUS_4).await?;
    let customer = h.customer(ZIP_PLUS_4).await?;
    let neighbour = h.customer(BLANK_GROUP).await?;

    // A day `3.` passes the edit, and its text is no date the column can hold: the save ends
    // before any write.
    let (fields, snapshot) = validated(h, ZIP_PLUS_4, |f| set(&mut f.open_day, "3.")).await?;
    let unwritable = confirm(h, ZIP_PLUS_4, &fields, &snapshot).await?;
    // audit: COACTUPC-106
    // audit: COACTUPC-111
    assert_screen_error(
        &unwritable,
        StatusCode::INTERNAL_SERVER_ERROR,
        UPDATE_FAILED,
        None,
    );
    assert_stored(h, ZIP_PLUS_4, &account, &customer).await?;

    // A customer ID received that is not the key read for update: the account rewrite stands,
    // the customer is not rewritten, and neither is the customer the ID names.
    let (fields, snapshot) = validated(h, ZIP_PLUS_4, |f| {
        set(&mut f.credit_limit, "6000");
        set(&mut f.customer_id, "000000061");
    })
    .await?;
    let key_changed = confirm(h, ZIP_PLUS_4, &fields, &snapshot).await?;
    // audit: COACTUPC-107
    // audit: COACTUPC-112
    assert_screen_error(
        &key_changed,
        StatusCode::INTERNAL_SERVER_ERROR,
        UPDATE_FAILED,
        None,
    );
    let rewritten = AccountRecord {
        credit_limit: money(600_000)?,
        addr_zip: PicX::from_text("GOLD")?,
        ..account
    };
    assert_stored(h, ZIP_PLUS_4, &rewritten, &customer).await?;
    assert!(
        h.customer(BLANK_GROUP).await? == neighbour,
        "customer {BLANK_GROUP} was rewritten"
    );
    Ok(())
}

/// A save of a customer holding a ZIP+4: the fetch shows five characters, so an unchanged echo
/// is a change, and the save writes those five characters and five spaces (COACTUPC-104,
/// X-COACTUPC-9).
async fn save_narrowed_zip(h: &Harness) -> TestResult {
    let account = h.account(ZIP_PLUS_4).await?;
    let (fields, snapshot) = validated(h, ZIP_PLUS_4, |_| {}).await?;
    assert_committed(&confirm(h, ZIP_PLUS_4, &fields, &snapshot).await?);
    // audit: COACTUPC-104
    assert_stored(
        h,
        ZIP_PLUS_4,
        &account,
        &test_customer(ZIP_PLUS_4, "62701")?,
    )
    .await
}

/// Another user's change to one term of the stored account [`CLEAN`].
type AccountTerm = fn(&mut AccountRecord) -> TestResult;
/// Another user's change to one term of the stored customer [`CLEAN`].
type CustomerTerm = fn(&mut CustomerRecord) -> TestResult;

/// `text` into the record field `field`, space-padded.
fn set_pic<const N: usize>(field: &mut PicX<N>, text: &str) -> TestResult {
    *field = PicX::from_text(text)?;
    Ok(())
}

/// The record field `field` in lower case, every other byte kept.
fn lowered<const N: usize>(field: &mut PicX<N>) -> TestResult {
    let text = std::str::from_utf8(field.as_bytes())?.to_ascii_lowercase();
    set_pic(field, &text)
}

/// The amount of `cents` into the record field `field`.
fn set_money(field: &mut Money<10>, cents: i64) -> TestResult {
    *field = money(cents)?;
    Ok(())
}

/// The date `year`-`month`-`date` into the record field `field`.
fn set_day(field: &mut PicDate, year: i32, month: u32, date: u32) -> TestResult {
    *field = PicDate::new(Some(day(year, month, date)?))?;
    Ok(())
}

/// The account terms `9700-CHECK-CHANGE-IN-REC` compares (COACTUPC-113), each changed alone from
/// what [`test_account`] stores: the status, each of the five amounts by one cent, each of the
/// three dates in its year, its month and its day, and the group ID to another word.
const ACCOUNT_TERMS: &[(&str, AccountTerm)] = &[
    ("active status", |a| set_pic(&mut a.active_status, "N")),
    ("current balance", |a| set_money(&mut a.curr_bal, 123_457)),
    ("credit limit", |a| set_money(&mut a.credit_limit, 500_001)),
    ("cash credit limit", |a| {
        set_money(&mut a.cash_credit_limit, 149_999)
    }),
    ("cycle credit", |a| set_money(&mut a.curr_cyc_credit, 1)),
    ("cycle debit", |a| set_money(&mut a.curr_cyc_debit, -2_551)),
    ("open date year", |a| set_day(&mut a.open_date, 2015, 11, 3)),
    ("open date month", |a| {
        set_day(&mut a.open_date, 2014, 12, 3)
    }),
    ("open date day", |a| set_day(&mut a.open_date, 2014, 11, 4)),
    ("expiry date year", |a| {
        set_day(&mut a.expiraion_date, 2026, 5, 20)
    }),
    ("expiry date month", |a| {
        set_day(&mut a.expiraion_date, 2025, 6, 20)
    }),
    ("expiry date day", |a| {
        set_day(&mut a.expiraion_date, 2025, 5, 21)
    }),
    ("reissue date year", |a| {
        set_day(&mut a.reissue_date, 2026, 5, 20)
    }),
    ("reissue date month", |a| {
        set_day(&mut a.reissue_date, 2025, 6, 20)
    }),
    ("reissue date day", |a| {
        set_day(&mut a.reissue_date, 2025, 5, 21)
    }),
    ("group ID", |a| set_pic(&mut a.group_id, "SILVER")),
];

/// The customer terms `9700-CHECK-CHANGE-IN-REC` compares (COACTUPC-114, COACTUPC-115), each
/// changed alone from what [`test_customer`] stores: the terms compared after `UPPER-CASE` by
/// more than their letter case; the ZIP in its first five characters and, apart, in its last
/// five; each phone number, and the first one in its positions 14 and 15 alone; the SSN in its
/// last four digits; the date of birth in its year, its month and its day; the EFT account ID,
/// the primary-holder indicator and the FICO score.
const CUSTOMER_TERMS: &[(&str, CustomerTerm)] = &[
    ("first name", |c| set_pic(&mut c.first_name, "Jack")),
    ("middle name", |c| set_pic(&mut c.middle_name, "Quentin")),
    ("last name", |c| set_pic(&mut c.last_name, "Private")),
    ("address line 1", |c| {
        set_pic(&mut c.addr_line_1, "2 Main Street")
    }),
    ("address line 2", |c| {
        set_pic(&mut c.addr_line_2, "Suite 300")
    }),
    ("address line 3", |c| {
        set_pic(&mut c.addr_line_3, "Shelbyville")
    }),
    ("state", |c| set_pic(&mut c.addr_state_cd, "WI")),
    ("country", |c| set_pic(&mut c.addr_country_cd, "CAN")),
    ("government ID", |c| {
        set_pic(&mut c.govt_issued_id, "GOVT-ID-0002")
    }),
    ("ZIP positions 1 to 5", |c| {
        set_pic(&mut c.addr_zip, "62702")
    }),
    ("ZIP positions 6 to 10", |c| {
        set_pic(&mut c.addr_zip, "62701-1234")
    }),
    ("phone 1", |c| set_pic(&mut c.phone_num_1, "(312)555-0101")),
    ("phone 2", |c| set_pic(&mut c.phone_num_2, "(415)555-0198")),
    ("phone 1 positions 14 and 15", |c| {
        set_pic(&mut c.phone_num_1, "(312)555-010099")
    }),
    // audit: COACTUPC-115
    ("SSN last four digits", |c| {
        c.ssn = Ssn::parse(OTHER_LAST4_SSN)?;
        Ok(())
    }),
    ("date of birth year", |c| {
        set_day(&mut c.dob_yyyy_mm_dd, 1981, 1, 15)
    }),
    ("date of birth month", |c| {
        set_day(&mut c.dob_yyyy_mm_dd, 1980, 2, 15)
    }),
    ("date of birth day", |c| {
        set_day(&mut c.dob_yyyy_mm_dd, 1980, 1, 16)
    }),
    ("EFT account ID", |c| {
        set_pic(&mut c.eft_account_id, "0000000043")
    }),
    ("primary card holder", |c| {
        set_pic(&mut c.pri_card_holder_ind, "N")
    }),
    ("FICO score", |c| {
        c.fico_credit_score = PicU::new(751)?;
        Ok(())
    }),
];

/// The customer terms `9700-CHECK-CHANGE-IN-REC` compares after `UPPER-CASE` (COACTUPC-114),
/// each lower-cased alone: a change of letter case only, which is no change by another user.
const CASE_ONLY_TERMS: &[(&str, CustomerTerm)] = &[
    ("first name in lower case", |c| lowered(&mut c.first_name)),
    ("middle name in lower case", |c| lowered(&mut c.middle_name)),
    ("last name in lower case", |c| lowered(&mut c.last_name)),
    ("address line 1 in lower case", |c| {
        lowered(&mut c.addr_line_1)
    }),
    ("address line 2 in lower case", |c| {
        lowered(&mut c.addr_line_2)
    }),
    ("address line 3 in lower case", |c| {
        lowered(&mut c.addr_line_3)
    }),
    ("state in lower case", |c| lowered(&mut c.addr_state_cd)),
    ("country in lower case", |c| lowered(&mut c.addr_country_cd)),
    ("government ID in lower case", |c| {
        lowered(&mut c.govt_issued_id)
    }),
];

/// The account term `9700-CHECK-CHANGE-IN-REC` does not compare (COACTUPC-113): `ACCT-ADDR-ZIP`,
/// changed from the group code [`test_account`] stores there.
const UNCOMPARED_ACCOUNT_TERMS: &[(&str, AccountTerm)] =
    &[("ACCT-ADDR-ZIP", |a| set_pic(&mut a.addr_zip, "B000000001"))];

/// The FICO score each case's request changes, so that its F5 carries a change.
const CASE_FICO: u64 = 800;

/// Another user's change to one term of the account or of its customer.
#[derive(Clone, Copy)]
enum Rewrite {
    Account(AccountTerm),
    Customer(CustomerTerm),
}

/// F5 of the case `name`: account [`CLEAN`] fetched and its FICO score changed in the request to
/// [`CASE_FICO`], then `rewrite` stored by another user before the save. The reply, then the
/// account and the customer as that user left them.
async fn save_over(
    h: &Harness,
    name: &str,
    rewrite: Rewrite,
) -> Fallible<(Reply, AccountRecord, CustomerRecord)> {
    let (_, mut fields, snapshot) = h.fetched(CLEAN).await?;
    set(&mut fields.fico_score, &CASE_FICO.to_string());
    let mut account = h.account(CLEAN).await?;
    let mut customer = h.customer(CLEAN).await?;
    match rewrite {
        Rewrite::Account(term) => {
            let fetched = account.clone();
            term(&mut account)?;
            assert_ne!(account, fetched, "{name}: the case changes no account term");
            rewrite_account(h, &account).await?;
        }
        Rewrite::Customer(term) => {
            let fetched = customer.clone();
            term(&mut customer)?;
            assert!(
                customer != fetched,
                "{name}: the case changes no customer term"
            );
            rewrite_customer(h, &customer).await?;
        }
    }
    let reply = confirm(h, CLEAN, &fields, &snapshot).await?;
    Ok((reply, account, customer))
}

/// After the case `name`, the stored account [`CLEAN`] equals `account` and its customer equals
/// `customer`. As in [`assert_stored`], the customer comparison prints nothing of the records.
async fn assert_case_stored(
    h: &Harness,
    name: &str,
    account: &AccountRecord,
    customer: &CustomerRecord,
) -> TestResult {
    assert_eq!(
        &h.account(CLEAN).await?,
        account,
        "{name}: stored account {CLEAN}"
    );
    assert!(
        &h.customer(CLEAN).await? == customer,
        "{name}: stored customer {CLEAN} differs from the expected one"
    );
    Ok(())
}

/// Account [`CLEAN`] and its customer rewritten as [`add_test_entities`] stores them.
async fn restore_clean(h: &Harness) -> TestResult {
    rewrite_account(h, &test_account(CLEAN, "GOLD")?).await?;
    rewrite_customer(h, &test_customer(CLEAN, "62701")?).await
}

/// The case `name` answered with the 409 of `9700-CHECK-CHANGE-IN-REC`, its message alone, and
/// nothing written: both records stay as the other user stored them. Then the originals return.
async fn conflicts(h: &Harness, name: &str, rewrite: Rewrite) -> TestResult {
    let (reply, account, customer) = save_over(h, name, rewrite).await?;
    assert_eq!(
        reply.status,
        StatusCode::CONFLICT,
        "{name}: another user's change went unnoticed"
    );
    assert_screen_error(&reply, StatusCode::CONFLICT, CHANGED, None);
    assert_case_stored(h, name, &account, &customer).await?;
    restore_clean(h).await
}

/// The case `name` committed over the other user's rewrite: the request's FICO score and every
/// other field as fetched are stored, the group ID received in `ACCT-ADDR-ZIP` (COACTUPC-103,
/// COACTUPC-104). Then the originals return.
async fn commits(h: &Harness, name: &str, rewrite: Rewrite) -> TestResult {
    let (reply, _, _) = save_over(h, name, rewrite).await?;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{name}: no change by another user, yet the save was refused"
    );
    assert_committed(&reply);
    let account = AccountRecord {
        addr_zip: PicX::from_text("GOLD")?,
        ..test_account(CLEAN, "GOLD")?
    };
    let customer = CustomerRecord {
        fico_credit_score: PicU::new(CASE_FICO)?,
        ..test_customer(CLEAN, "62701")?
    };
    assert_case_stored(h, name, &account, &customer).await?;
    restore_clean(h).await
}

/// `9700-CHECK-CHANGE-IN-REC` term by term on account [`CLEAN`] and its customer, which this test
/// owns: another user's rewrite of one compared term between the fetch and F5 is the 409 that
/// writes nothing (COACTUPC-113, COACTUPC-114, COACTUPC-115); a rewrite of a customer term
/// compared after `UPPER-CASE` in its letter case alone, or of `ACCT-ADDR-ZIP`, which is not
/// compared, is no change, and the save commits. Both records are restored after each case.
#[test]
fn coactupc_change_check_terms() -> TestResult {
    current_thread()?.block_on(async {
        let h = Harness::start().await?;
        let account = test_account(CLEAN, "GOLD")?;
        let customer = test_customer(CLEAN, "62701")?;
        assert_case_stored(&h, "before the cases", &account, &customer).await?;
        for &(name, term) in ACCOUNT_TERMS {
            // audit: COACTUPC-113
            conflicts(&h, name, Rewrite::Account(term)).await?;
        }
        for &(name, term) in CUSTOMER_TERMS {
            // audit: COACTUPC-114
            conflicts(&h, name, Rewrite::Customer(term)).await?;
        }
        for &(name, term) in CASE_ONLY_TERMS {
            // audit: COACTUPC-114
            commits(&h, name, Rewrite::Customer(term)).await?;
        }
        for &(name, term) in UNCOMPARED_ACCOUNT_TERMS {
            // audit: COACTUPC-113
            commits(&h, name, Rewrite::Account(term)).await?;
        }
        Ok(())
    })
}

/// How long another transaction holds a row lock while a save over the request pool waits for
/// it: longer than the 5 seconds after which a save once gave up, so a wait that ended before
/// the holder did fails the test.
const LOCK_HOLD: Duration = Duration::from_secs(6);

/// A current-thread runtime with every driver, the one `#[tokio::test]` builds, made here so
/// that no test macro expands into a lint suppression or an unchecked pin. Every test of this
/// file is a plain `#[test]` that blocks on it.
fn current_thread() -> Fallible<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

/// F5 in state `N` over the pool `carddemo-api` serves requests from, while another transaction
/// holds the row lock of account [`CLEAN`], then of its customer: the read for update waits for
/// as long as the holder keeps the lock, [`LOCK_HOLD`], as a CICS `READ UPDATE` waits for a
/// record another task holds, and once the holder commits, the save goes on: both records are
/// rewritten and the reply is the committed one (COACTUPC-101, COACTUPC-102, COACTUPC-108).
#[test]
fn coactupc_lock_wait_rows() -> TestResult {
    current_thread()?.block_on(async {
        let h = Harness::start().await?;
        let requests = h.request_state().await?;
        let cases: [(Touch, bool, u64); 2] = [
            (|f| set(&mut f.fico_score, "701"), false, 701),
            (|f| set(&mut f.fico_score, "702"), true, 702),
        ];
        for (touch, customer_held, fico) in cases {
            let (fields, snapshot) = validated(&h, CLEAN, touch).await?;
            let payload = update_body("PF5", &fields, &in_state(&snapshot, "N"))?;
            let mut holder = h.store.begin().await?;
            let held = if customer_held {
                CustomerRepository::lock_by_id(&mut holder, PicU::<9>::new(CLEAN)?)
                    .await?
                    .is_some()
            } else {
                AccountRepository::lock_by_id(&mut holder, PicU::<11>::new(CLEAN)?)
                    .await?
                    .is_some()
            };
            assert!(held, "the holder locked record {CLEAN}");
            let key = acct(CLEAN);
            let mut save = Box::pin(h.save_as(&requests, &h.user, &key, &payload));
            let waited = tokio::time::timeout(LOCK_HOLD, save.as_mut()).await;
            // audit: COACTUPC-101
            // audit: COACTUPC-102
            assert!(
                waited.is_err(),
                "the save answered while the holder kept the lock (customer: {customer_held})"
            );
            holder.commit().await?;
            let reply = tokio::time::timeout(LOCK_HOLD, save)
                .await
                .map_err(|_| "no answer once the holder committed")??;
            // audit: COACTUPC-101
            // audit: COACTUPC-102
            assert_committed(&reply);
            // The group ID received lands in the ZIP field (COACTUPC-103).
            let account = AccountRecord {
                addr_zip: PicX::from_text("GOLD")?,
                ..test_account(CLEAN, "GOLD")?
            };
            let customer = CustomerRecord {
                fico_credit_score: PicU::new(fico)?,
                ..test_customer(CLEAN, "62701")?
            };
            assert_stored(&h, CLEAN, &account, &customer).await?;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------------------------
// COCOM01Y: the COMMAREA's customer and account blocks
// ---------------------------------------------------------------------------------------------

/// The COMMAREA's customer and account blocks (COCOM01Y-008 to COCOM01Y-013), which COACTVWC
/// fills from its reads and the replacement never carries: endpoint 4 takes the account from its
/// path alone and reads the customer through the account's cross-reference. Account
/// [`CLEAN`]'s active status is rewritten and restored, so the test owns its database.
#[test]
fn cocom01y_account_rows() -> TestResult {
    current_thread()?.block_on(async {
        let harness = Harness::start().await?;
        commarea_selected_account(&harness).await?;
        commarea_account_status(&harness).await?;
        Ok(())
    })
}

/// One token views account 1, account [`CLEAN`] and account 1 again: each answer is the account
/// its path names (COCOM01Y-012), with the customer the account's cross-reference names
/// (COCOM01Y-008) and that customer's stored names (COCOM01Y-009 to COCOM01Y-011), though no
/// request names a customer.
async fn commarea_selected_account(h: &Harness) -> TestResult {
    let first = h.view(ACCOUNT_1).await?;
    let clean = h.view(&acct(CLEAN)).await?;
    let again = h.view(ACCOUNT_1).await?;
    for (reply, id) in [(&first, 1), (&clean, CLEAN), (&again, 1)] {
        assert_eq!(reply.status, StatusCode::OK, "view of account {id}");
        // audit: COCOM01Y-012
        assert_eq!(reply.body["acct_id"], acct(id), "view of account {id}");
    }
    // audit: COCOM01Y-012
    assert!(
        again.body == first.body,
        "account 1 answers otherwise after another account was viewed"
    );
    for (reply, id, customer_id) in [(&first, 1, 1), (&clean, CLEAN, CLEAN)] {
        let mut store = h.store.clone();
        let xref = CardXrefRepository::find_by_acct_id(&mut store, PicU::new(id)?)
            .await?
            .ok_or_else(|| format!("account {id} has no cross-reference"))?;
        assert_eq!(xref.cust_id.get(), customer_id, "customer of account {id}");
        // audit: COCOM01Y-008
        assert_eq!(
            reply.body["customer_id"],
            format!("{customer_id:09}"),
            "customer of account {id}"
        );
        let customer = h.customer(customer_id).await?;
        // audit: COCOM01Y-009
        assert_eq!(
            reply.body["first_name"],
            pad(&customer.first_name.to_text(), 25),
            "first name of account {id}"
        );
        // audit: COCOM01Y-010
        assert_eq!(
            reply.body["middle_name"],
            pad(&customer.middle_name.to_text(), 25),
            "middle name of account {id}"
        );
        // audit: COCOM01Y-011
        assert_eq!(
            reply.body["last_name"],
            pad(&customer.last_name.to_text(), 25),
            "last name of account {id}"
        );
    }
    Ok(())
}

/// The active status endpoint 4 shows is the stored account's, read by every request: another
/// user's rewrite between two identical requests changes the second answer, and restoring it
/// restores the answer (COCOM01Y-013).
async fn commarea_account_status(h: &Harness) -> TestResult {
    for id in [1, CLEAN] {
        let stored = h.account(id).await?;
        let reply = h.view(&acct(id)).await?;
        // audit: COCOM01Y-013
        assert_eq!(
            reply.body["active_status"],
            stored.active_status.to_text(),
            "active status of account {id}"
        );
    }
    let before = h.account(CLEAN).await?;
    let shown = h.view(&acct(CLEAN)).await?;
    assert_eq!(shown.body["active_status"], "Y");
    let inactive = AccountRecord {
        active_status: PicX::from_array(*b"N"),
        ..before.clone()
    };
    rewrite_account(h, &inactive).await?;
    let after = h.view(&acct(CLEAN)).await?;
    // audit: COCOM01Y-013
    assert_eq!(
        (after.status, &after.body["active_status"]),
        (StatusCode::OK, &json!("N"))
    );
    rewrite_account(h, &before).await?;
    let restored = h.view(&acct(CLEAN)).await?;
    // audit: COCOM01Y-013
    assert!(
        restored.body == shown.body,
        "the restored active status is not shown"
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Reported, never passed
// ---------------------------------------------------------------------------------------------

/// Reports as `BLOCKED` the CICS `RESP` and `RESP2` values inside the not-found and
/// `File Error:` texts of both programs: CICS defines them and has no counterpart, so no test
/// asserts them. The fixed pieces of those texts are asserted by the read-chain tests above.
// audit: COACTVWC-036
// audit: COACTVWC-038
// audit: COACTVWC-041
// audit: COACTVWC-043
// audit: COACTVWC-045
// audit: COACTVWC-047
// audit: COACTUPC-086
// audit: COACTUPC-088
// audit: COACTUPC-092
// audit: COACTUPC-094
// audit: COACTUPC-096
// audit: COACTUPC-098
#[test]
#[ignore = "BLOCKED: HALTED(H4) CICS RESP/RESP2 values in the COACTVWC and COACTUPC read texts; no parity assertion (AAP 0.8.5)"]
fn blocked_h4_resp_codes() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COACTVWC-036, -038, -041, -043, -045, -047 and COACTUPC-086, -088, -092, -094, \
         -096, -098 (H4): the RESP and RESP2 values in the not-found and File Error texts are \
         defined by CICS, which has no counterpart"
            .into(),
    )
}

/// Reports as `BLOCKED` the date-validity verdict of `CEEDAYS`, which COACTUPC reaches through
/// `EDIT-DATE-LE` and CSUTLDTC: Language Environment defines it, so no test asserts whether a
/// date that passed the part edits is accepted, nor the severity and message codes of
/// CSUTLDTC-024's message, which only a rejection produces.
// audit: COACTUPC-129
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
// audit: CSUTLDTC-021
// audit: CSUTLDTC-022
// audit: CSUTLDTC-023
// audit: CSUTLDTC-024
// audit: CSUTLDTC-025
#[test]
#[ignore = "BLOCKED: HALTED(H5) CEEDAYS date validity behind EDIT-DATE-LE; no parity assertion (AAP 0.8.5)"]
fn blocked_h5_date_validity() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COACTUPC-129 and CSUTLDTC-004, -006 to -018, -021 to -025 (H5, X-CSUTLDTC-H1): \
         whether CEEDAYS accepts a date that passed the part edits, and the severity and message \
         codes a rejection reports, are defined by Language Environment"
            .into(),
    )
}

/// Reports as `BLOCKED` the values that come from working storage no statement sets: the
/// account block shown after a failed account read (X-COACTVWC-H1, X-COACTUPC-H1), the
/// group ID and fillers the account and customer rewrites write (X-COACTUPC-H2), and the
/// first-entry test on an uninitialised COMMAREA (X-COCRDSLC-H2). The kept stored values the
/// replacement writes are asserted above, and they are no parity claim.
// audit: COACTVWC-009
// audit: COACTVWC-048
// audit: COACTUPC-099
// audit: COACTUPC-103
// audit: COACTUPC-104
#[test]
#[ignore = "BLOCKED: X-COACTVWC-H1, X-COACTUPC-H1, X-COACTUPC-H2, X-COCRDSLC-H2 uninitialised storage; no parity assertion (AAP 0.8.5)"]
fn blocked_uninitialised_storage() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COACTVWC-009, COACTVWC-048, COACTUPC-099, COACTUPC-103 and COACTUPC-104 \
         (X-COACTVWC-H1, X-COCRDSLC-H2, X-COACTUPC-H1, X-COACTUPC-H2): the values depend on \
         runtime storage initialisation, which the COBOL leaves undefined"
            .into(),
    )
}

/// Reports as `BLOCKED` the edits of date parts holding non-digit text, which test a numeric
/// redefinition of alphanumeric data (X-COACTUPC-H3), and the value of an amount typed with
/// more than two decimal places (X-ARITH-5). The digit cases of the same rows are asserted.
// audit: COACTUPC-009
// audit: COACTUPC-122
// audit: COACTUPC-126
// audit: COACTUPC-128
#[test]
#[ignore = "BLOCKED: X-COACTUPC-H3 non-digit date parts, X-ARITH-5 amounts past two decimals; no parity assertion (AAP 0.8.5)"]
fn blocked_numeric_data_in_text() -> TestResult {
    // A blocked case is reported, never passed, so a run that includes ignored tests fails here.
    Err(
        "BLOCKED: COACTUPC-122, -126, -128 (X-COACTUPC-H3) for date parts with non-digit text, and \
         COACTUPC-009 (X-ARITH-5) for amounts with more than two decimal places: the COBOL \
         result depends on the code generated for numeric items holding other data"
            .into(),
    )
}
