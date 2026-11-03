//! CT01 `GET /transactions/{id}` (`COTRN01C`, map `COTRN01`): the Tran ID
//! edit of `PROCESS-ENTER-KEY`, the keyed read of `READ-TRANSACT-FILE` and the
//! view of every `CVTRA05Y` field [app/cbl/COTRN01C.cbl:L144-296], and the
//! REST replacements of the map's AID keys, header and PF4 Clear and of the
//! transfer to sign-on without a session.
//!
//! Each error answer is the one its `COTRN01C` row of the validation audit
//! gives: the `Status` and `BMS field` cells, the error kind of the trigger
//! condition, and the message, which comes from `carddemo_domain::messages`
//! and must equal the row's `Message text`. The failing inputs of the blank
//! and not-found rows are the requests their `example=` names.
//!
//! The seeded live master is empty (E-07), and a CT02 add against an empty
//! master allocates no TRAN-ID (E-27). The found-transaction tests therefore
//! first write one live row through `transaction_master`, as a posting run
//! leaves one, and the add of `POST /transactions` then follows it.
//!
//! Docker must be running; the harness conventions are those of `common`.

pub mod common;

use carddemo_api::dto::transactions::{TransactionResponse, TransactionViewField};
use carddemo_audit::{Document, code_span, key_values};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::block_on;
use common::{Resp, TestApp, assert_error, validation_audit};
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Value, json};

/// The processing clock, inside the active period of the seeded retail reward
/// rule (`01`/`0001`, from 2022-01-01), so the CT02 add accrues and commits.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The number of live transactions, as text.
const LIVE_COUNT: &str = "SELECT count(*)::text FROM transaction_master";

/// The live row the CT02 allocation counts from, written as a posting run
/// leaves it: its origination timestamp in the `DALYTRAN` form and its
/// processing timestamp in the `DB2` form, each the full 26 characters of its
/// `X(26)` field. [`base_row`] is the view of exactly these values.
const BASE_ROW: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES ('0000000000000300', '01', 1, 'POS TERM', 'CT01 base purchase', 10.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '9680294154603697', '2022-06-01 10:11:12.000000', '2022-07-01-12.00.00.000000')";

/// The TRAN-ID of [`BASE_ROW`].
const BASE_TRAN_ID: &str = "0000000000000300";

/// Makes every read of the live master fail on the server with SQLSTATE
/// `42P01`, a database failure on the transaction read.
const HIDE_MASTER: &str = "ALTER VIEW transaction_master RENAME TO transaction_master_offline";

/// Puts the live master back.
const RESTORE_MASTER: &str = "ALTER VIEW transaction_master_offline RENAME TO transaction_master";

/// Account 1's card in the seeded cross-reference: the key of the CT02 add.
const CARD_NUMBER: &str = "9680294154603697";

/// The CT02 add's `TTYPCD`: the fixture's type `01`, Purchase.
const ADD_TYPE: &str = "01";

/// The CT02 add's `TCATCD`: the fixture's category `0001`.
const ADD_CATEGORY: &str = "0001";

/// The CT02 add's `TRNSRC`.
const ADD_SOURCE: &str = "POS TERM";

/// The CT02 add's `TDESC`.
const ADD_DESCRIPTION: &str = "CT01 view purchase";

/// The CT02 add's `TRNAMT` as entered: sign, eight digits, point, two digits.
/// [`added_row`] holds its value, 1234.56.
const ADD_AMOUNT: &str = "+00001234.56";

/// The CT02 add's `TORIGDT`, a day before [`ADD_PROC_DATE`], so a swap of the
/// two timestamps shows.
const ADD_ORIG_DATE: &str = "2022-06-30";

/// The CT02 add's `TPROCDT`.
const ADD_PROC_DATE: &str = "2022-07-01";

/// The CT02 add's `MID`: nine digits, leading zeros included.
const ADD_MERCHANT_ID: &str = "000012345";

/// The CT02 add's `MNAME`.
const ADD_MERCHANT_NAME: &str = "CT01 merchant";

/// The CT02 add's `MCITY`.
const ADD_MERCHANT_CITY: &str = "Columbus";

/// The CT02 add's `MZIP`.
const ADD_MERCHANT_ZIP: &str = "43004";

/// The `code` of a 401 for a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The `code` of a 405 for a method a routed path does not serve.
const METHOD_NOT_ALLOWED: &str = "METHOD_NOT_ALLOWED";

/// The 13 display fields of map `COTRN1A` in the order `INITIALIZE-ALL-FIELDS`
/// clears them [app/cbl/COTRN01C.cbl:L309-326], each with the view member that
/// carries the record field `PROCESS-ENTER-KEY` moves into it
/// [app/cbl/COTRN01C.cbl:L176-192].
const DISPLAY_FIELDS: [(&str, &str); 13] = [
    ("TRNIDI", "tran_id"),
    ("CARDNUMI", "tran_card_num"),
    ("TTYPCDI", "tran_type_cd"),
    ("TCATCDI", "tran_cat_cd"),
    ("TRNSRCI", "tran_source"),
    ("TRNAMTI", "tran_amt"),
    ("TDESCI", "tran_desc"),
    ("TORIGDTI", "tran_orig_ts"),
    ("TPROCDTI", "tran_proc_ts"),
    ("MIDI", "tran_merchant_id"),
    ("MNAMEI", "tran_merchant_name"),
    ("MCITYI", "tran_merchant_city"),
    ("MZIPI", "tran_merchant_zip"),
];

/// The `Expected result` keys of the `COTRN01` header fields P-COTRN01C-009
/// records as fixed texts: the two titles, quoted, and the transaction and
/// program names.
const HEADER_KEYS: [&str; 4] = ["title01", "title02", "trnname", "pgmname"];

/// The answer one `COTRN01C` row of the validation audit gives, and the
/// request its `example=` names, when it names one.
struct Audited {
    id: &'static str,
    kind: ErrorKind,
    status: StatusCode,
    message: &'static str,
    field: &'static str,
    example: Option<String>,
}

/// The answer of the validation audit row `id`, whose message is the
/// `carddemo_domain::messages` constant `message` and whose kind is `kind`.
///
/// # Panics
///
/// Panics when the row is missing, when its `Message text` is not `message`
/// in double quotes, its `BMS field` is not `TRNIDIN`, its trigger condition's
/// `kind=` is not `kind`, or its `Status` is not an HTTP status.
fn audited(doc: &Document, id: &'static str, kind: ErrorKind, message: &'static str) -> Audited {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit has no {id} row"));
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
            .trim()
    };
    assert_eq!(
        code_span(cell("Message text")).map(|span| span.text),
        Some(format!("\"{message}\"")),
        "{id}: the audited message text and the messages constant"
    );
    let field = TransactionViewField::TrnIdIn.as_str();
    assert_eq!(cell("BMS field"), field, "{id}: the BMS field");
    let trigger = key_values(cell("Trigger condition"));
    let value_of = |key: &str| {
        trigger
            .iter()
            .find(|pair| pair.key == key)
            .map(|pair| pair.value.clone())
    };
    assert_eq!(
        value_of("kind"),
        Some(format!("{kind:?}")),
        "{id}: the error kind"
    );
    let status = cell("Status")
        .parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} has no HTTP status: {}", cell("Status")));
    let example = value_of("example").and_then(|text| {
        text.strip_prefix("GET ")
            .and_then(|request| request.split_whitespace().next())
            .map(str::to_owned)
    });
    Audited {
        id,
        kind,
        status,
        message,
        field,
        example,
    }
}

impl Audited {
    /// The path of the request the row's `example=` names.
    ///
    /// # Panics
    ///
    /// Panics when the row names no `GET` request.
    fn example_path(&self) -> &str {
        self.example
            .as_deref()
            .unwrap_or_else(|| panic!("{} names no GET example", self.id))
    }

    /// Asserts that `resp`, the answer to `request`, is this row's error: its
    /// status, its kind's code, its message, and the one `TRNIDIN` entry the
    /// cursor placement gives every `COTRN01C` error (P-COTRN01C-007).
    fn assert_answers(&self, resp: &Resp, request: &str) {
        assert_eq!(
            resp.0, self.status,
            "{}: the status of {request}: {}",
            self.id, resp.2
        );
        assert_error(
            resp,
            self.status,
            self.kind.code(),
            self.message,
            &[(self.field, self.message)],
        );
    }
}

/// The view path of the Tran ID path segment `segment`, as entered.
fn view_path(segment: &str) -> String {
    format!("/transactions/{segment}")
}

/// The JSON a 200 view of `transaction` carries.
fn json_of(transaction: &TransactionResponse) -> Value {
    serde_json::to_value(transaction)
        .unwrap_or_else(|error| panic!("{transaction:?} does not serialize: {error}"))
}

/// The view of [`BASE_ROW`]: every `CVTRA05Y` field as stored, the `9(n)`
/// codes as their zero-padded digits and both timestamps as their 26
/// characters.
fn base_row() -> TransactionResponse {
    TransactionResponse {
        tran_id: BASE_TRAN_ID.to_owned(),
        tran_type_cd: "01".to_owned(),
        tran_cat_cd: "0001".to_owned(),
        tran_source: "POS TERM".to_owned(),
        tran_desc: "CT01 base purchase".to_owned(),
        tran_amt: Decimal::new(1_000, 2),
        tran_merchant_id: "000000001".to_owned(),
        tran_merchant_name: "Base merchant".to_owned(),
        tran_merchant_city: "Base city".to_owned(),
        tran_merchant_zip: "12345".to_owned(),
        tran_card_num: CARD_NUMBER.to_owned(),
        tran_orig_ts: "2022-06-01 10:11:12.000000".to_owned(),
        tran_proc_ts: "2022-07-01-12.00.00.000000".to_owned(),
    }
}

/// The view of the CT02 add of [`add_body`] under `tran_id`.
///
/// The add moves each entered date into its `X(26)` timestamp field followed
/// by 16 spaces, and reads no clock (P-COTRN02C-030); the view shows the field
/// without its trailing spaces, so each timestamp is the entered date.
fn added_row(tran_id: &str) -> TransactionResponse {
    TransactionResponse {
        tran_id: tran_id.to_owned(),
        tran_type_cd: ADD_TYPE.to_owned(),
        tran_cat_cd: ADD_CATEGORY.to_owned(),
        tran_source: ADD_SOURCE.to_owned(),
        tran_desc: ADD_DESCRIPTION.to_owned(),
        tran_amt: Decimal::new(123_456, 2),
        tran_merchant_id: ADD_MERCHANT_ID.to_owned(),
        tran_merchant_name: ADD_MERCHANT_NAME.to_owned(),
        tran_merchant_city: ADD_MERCHANT_CITY.to_owned(),
        tran_merchant_zip: ADD_MERCHANT_ZIP.to_owned(),
        tran_card_num: CARD_NUMBER.to_owned(),
        tran_orig_ts: ADD_ORIG_DATE.to_owned(),
        tran_proc_ts: ADD_PROC_DATE.to_owned(),
    }
}

/// The confirmed CT02 body: keyed by the card number alone, every field in
/// the form its edit accepts.
fn add_body() -> Value {
    json!({
        "card_number": CARD_NUMBER,
        "tran_type_cd": ADD_TYPE,
        "tran_cat_cd": ADD_CATEGORY,
        "tran_source": ADD_SOURCE,
        "tran_desc": ADD_DESCRIPTION,
        "tran_amt": ADD_AMOUNT,
        "tran_orig_date": ADD_ORIG_DATE,
        "tran_proc_date": ADD_PROC_DATE,
        "tran_merchant_id": ADD_MERCHANT_ID,
        "tran_merchant_name": ADD_MERCHANT_NAME,
        "tran_merchant_city": ADD_MERCHANT_CITY,
        "tran_merchant_zip": ADD_MERCHANT_ZIP,
        "confirm": "Y",
    })
}

/// Adds the transaction of [`add_body`] through `POST /transactions` and
/// returns its TRAN-ID, read from the 201 body, after checking that body: the
/// added record and the `COTRN02C` success text.
async fn add_transaction(app: &TestApp, token: &str) -> String {
    let (status, body, raw) = app.post("/transactions", Some(token), &add_body()).await;
    assert_eq!(status, StatusCode::CREATED, "the CT02 add: {raw}");
    let tran_id = body["transaction"]["tran_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the CT02 answer has no string tran_id: {raw}"))
        .to_owned();
    assert_eq!(
        body["transaction"],
        json_of(&added_row(&tran_id)),
        "the transaction of the CT02 answer: {raw}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(messages::transaction_added_message(&tran_id).as_str()),
        "the CT02 success text: {raw}"
    );
    tran_id
}

/// The 200 view of the Tran ID path segment `segment`.
async fn view(app: &TestApp, token: &str, segment: &str) -> Value {
    let path = view_path(segment);
    let (status, body, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    body
}

/// The keyed read names one record exactly: the entered text moved into
/// `TRAN-ID X(16)`, space-padded or cut on the right, with no numeric edit and
/// no zero padding (P-COTRN01C-003, P-COTRN01C-004).
async fn assert_key_moved_as_entered(app: &TestApp, token: &str, tran_id: &str, missing: &Audited) {
    let not_live = [
        missing.example_path().to_owned(),
        // The digits without their leading zeros, padded with spaces instead.
        view_path(tran_id.trim_start_matches('0')),
        // Below every live key: the read is keyed, not a browse from a start.
        view_path("0000000000000299"),
    ];
    for path in &not_live {
        missing.assert_answers(&app.get(path, Some(token)).await, path);
    }
    assert_eq!(
        view(app, token, &format!("{tran_id}XYZ")).await,
        json_of(&added_row(tran_id)),
        "a key longer than 16 characters is cut to its first 16, as the MOVE into TRAN-ID cuts it"
    );
}

/// The cell `name` of the parity row `id`, once every review of the row in
/// `doc`, the parity audit, passed.
fn reviewed_cell(doc: &Document, id: &str, name: &str) -> String {
    let mut verdicts = Vec::new();
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if table.cell(row, "Row ID") == Some(id) {
                verdicts.push(table.cell(row, "Verdict").unwrap_or_default());
            }
        }
    }
    assert!(
        !verdicts.is_empty() && verdicts.iter().all(|verdict| *verdict == "pass"),
        "{id} must have passed its review, found {verdicts:?}"
    );
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
        .to_owned()
}

/// The `Expected result` of the reviewed parity row `id`, once it records
/// every text of `recorded`.
fn reviewed_result(doc: &Document, id: &str, recorded: &[&str]) -> String {
    let result = reviewed_cell(doc, id, "Expected result");
    for text in recorded {
        assert!(result.contains(text), "{id} records no {text:?}: {result}");
    }
    result
}

/// The member names of the JSON object `body`.
fn member_names(body: &Value) -> Vec<&str> {
    body.as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// The texts of the `COTRN01` header P-COTRN01C-009 records: each title
/// unquoted and trimmed, and the transaction and program names.
fn header_texts(parity: &Document) -> Vec<String> {
    let result = reviewed_result(
        parity,
        "P-COTRN01C-009",
        &["no API field carries the header"],
    );
    let pairs = key_values(&result);
    HEADER_KEYS
        .iter()
        .map(|key| {
            let Some(pair) = pairs.iter().find(|pair| pair.key == *key) else {
                panic!("P-COTRN01C-009 records no {key}: {result}");
            };
            let value = pair.value.as_str();
            code_span(value).map_or_else(
                || value.to_owned(),
                |span| span.text.trim_matches('"').trim().to_owned(),
            )
        })
        .collect()
}

/// The members of a view (P-COTRN01C-009, P-COTRN01C-011): one per display
/// field `INITIALIZE-ALL-FIELDS` clears, the 13 the row names, and no member
/// that carries a text of the 3270 header.
fn assert_view_members(view: &Value, parity: &Document) {
    let mut members = member_names(view);
    members.sort_unstable();
    let mut expected: Vec<&str> = DISPLAY_FIELDS.iter().map(|(_, member)| *member).collect();
    expected.sort_unstable();
    assert_eq!(members, expected, "the members of the view: {view}");
    let headers = header_texts(parity);
    let shown = view
        .as_object()
        .into_iter()
        .flat_map(|object| object.values())
        .filter_map(Value::as_str)
        .find(|text| headers.iter().any(|header| header == text.trim()));
    assert_eq!(shown, None, "the view carries a header text: {view}");
}

/// The keys of `COTRN01` once a transaction is viewed (P-COTRN01C-002):
///
/// - ENTER is the view request, whose key is its route parameter alone
///   (P-COTRN01C-008, E-56): a query naming an AID key or another Tran ID is
///   not read, so the answer is the view of the route's Tran ID, never the
///   invalid-key text.
/// - PF5 is the Browse transactions link (P-COTRN01C-006, F-COTRN01-004):
///   `GET /transactions` lists again from the first TRAN-ID, on page 1.
/// - PF4 has no control (P-COTRN01C-010, E-62): the view route serves no
///   other method, and those requests leave the view as it was.
async fn view_keys(app: &TestApp, token: &str, tran_id: &str, parity: &Document) {
    reviewed_result(parity, "P-COTRN01C-002", &[messages::CCDA_MSG_INVALID_KEY]);
    reviewed_result(
        parity,
        "P-COTRN01C-008",
        &["route parameter", "no key field"],
    );
    reviewed_result(
        parity,
        "P-COTRN01C-006",
        &["Browse transactions link (PF5)"],
    );
    reviewed_result(parity, "P-COTRN01C-010", &["no file access", "no control"]);
    let viewed = view(app, token, tran_id).await;
    assert_view_members(&viewed, parity);
    let other_id = format!("tran_id={BASE_TRAN_ID}");
    for query in ["aid=PF9", "aid=CLEAR", other_id.as_str()] {
        let path = format!("{}?{query}", view_path(tran_id));
        let (status, body, raw) = app.get(&path, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
        assert_eq!(body, viewed, "GET {path} is the view of {tran_id}");
    }

    let (status, page, raw) = app.get("/transactions", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "PF5, GET /transactions: {raw}");
    assert_eq!(page["prev_key"], Value::Null, "PF5 lists page 1: {raw}");
    assert_eq!(
        page["items"][0]["tran_id"].as_str(),
        Some(BASE_TRAN_ID),
        "PF5 lists from the first TRAN-ID: {raw}"
    );

    let path = view_path(tran_id);
    let refused = [
        app.put(&path, Some(token), &json!({})).await,
        app.post(&path, Some(token), &json!({})).await,
        app.delete(&path, Some(token)).await,
    ];
    for resp in &refused {
        assert_error(
            resp,
            StatusCode::METHOD_NOT_ALLOWED,
            METHOD_NOT_ALLOWED,
            messages::METHOD_NOT_ALLOWED,
            &[],
        );
    }
    assert_eq!(
        view(app, token, tran_id).await,
        viewed,
        "the view after PF4"
    );
}

#[test]
fn blank_and_unknown_tran_ids_on_the_seeded_empty_master_are_rejected() {
    let doc = validation_audit();
    let blank = audited(
        &doc,
        "V-COTRN01C-001",
        ErrorKind::Empty,
        messages::TRAN_ID_CAN_NOT_BE_EMPTY,
    );
    let missing = audited(
        &doc,
        "V-COTRN01C-002",
        ErrorKind::NotFound,
        messages::TRANSACTION_ID_NOT_FOUND,
    );
    let blank_segments = [
        blank.example_path().to_owned(),
        view_path("%20%20"),
        view_path(&"%20".repeat(16)),
        view_path("%00"),
        view_path(&"%00".repeat(16)),
    ];
    let unknown_keys = [
        missing.example_path().to_owned(),
        view_path("0000000000000001"),
        view_path("NOT-A-NUMBER"),
    ];
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        assert_eq!(
            app.stack().fetch_text_rows(LIVE_COUNT).await,
            [[Some("0".to_owned())]],
            "P-CVTRA05Y-016: the seeded live master is empty (E-07)"
        );
        let token = app.user_token().await;
        // P-COTRN01C-003: SPACES or LOW-VALUES, whatever their count, stop
        // before the read.
        for path in &blank_segments {
            blank.assert_answers(&app.get(path, Some(&token)).await, path);
        }
        // P-COTRN01C-004: RESP NOTFND for every key of the empty master,
        // numeric or not.
        for path in &unknown_keys {
            missing.assert_answers(&app.get(path, Some(&token)).await, path);
        }
    });
}

#[test]
fn a_live_transaction_is_viewed_with_every_field_as_written_and_as_listed() {
    let doc = validation_audit();
    let missing = audited(
        &doc,
        "V-COTRN01C-002",
        ErrorKind::NotFound,
        messages::TRANSACTION_ID_NOT_FOUND,
    );
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        app.stack().exec_sql(BASE_ROW).await;
        let token = app.user_token().await;
        let tran_id = add_transaction(&app, &token).await;

        let added = view(&app, &token, &tran_id).await;
        assert_eq!(
            added,
            json_of(&added_row(&tran_id)),
            "P-COTRN01C-005: the view of the CT02 add holds the 13 fields written"
        );
        let base = view(&app, &token, BASE_TRAN_ID).await;
        assert_eq!(
            base,
            json_of(&base_row()),
            "P-COTRN01C-005: the view of the posted row, each timestamp its 26 stored characters"
        );

        // P-COTRN00C-003, P-COTRN01C-001: a row selected from the list is
        // viewed as the list shows it.
        let (status, page, raw) = app.get("/transactions", Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "GET /transactions: {raw}");
        assert_eq!(
            page["items"],
            json!([base, added]),
            "the list rows and the views of the two live transactions: {raw}"
        );

        assert_key_moved_as_entered(&app, &token, &tran_id, &missing).await;
        view_keys(&app, &token, &tran_id, &common::parity_audit()).await;
    });
}

#[test]
fn a_failed_transaction_read_answers_unable_to_lookup_and_a_blank_id_reads_nothing() {
    let doc = validation_audit();
    let blank = audited(
        &doc,
        "V-COTRN01C-001",
        ErrorKind::Empty,
        messages::TRAN_ID_CAN_NOT_BE_EMPTY,
    );
    let failed = audited(
        &doc,
        "V-COTRN01C-003",
        ErrorKind::Unexpected,
        messages::UNABLE_TO_LOOKUP_TRANSACTION,
    );
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        app.stack().exec_sql(BASE_ROW).await;
        let token = app.user_token().await;
        let before = view(&app, &token, BASE_TRAN_ID).await;

        app.stack().exec_sql(HIDE_MASTER).await;
        // P-COTRN01C-004: any RESP but NORMAL and NOTFND, for a live key and
        // for a missing one alike, since the read itself fails.
        for key in [BASE_TRAN_ID, "9999999999999999"] {
            let path = view_path(key);
            failed.assert_answers(&app.get(&path, Some(&token)).await, &path);
        }
        // P-COTRN01C-003: a blank Tran ID is answered before any read, so the
        // failing read does not reach it.
        let path = blank.example_path();
        blank.assert_answers(&app.get(path, Some(&token)).await, path);
        let signed_off = app.get(&view_path(BASE_TRAN_ID), None).await;

        app.stack().exec_sql(RESTORE_MASTER).await;
        // P-COTRN01C-006: without a session (EIBCALEN = 0) the transfer to
        // sign-on comes before any file access, so the failing read does not
        // reach it either.
        reviewed_result(
            &common::parity_audit(),
            "P-COTRN01C-006",
            &["no file access", "no token gives 401"],
        );
        assert_error(
            &signed_off,
            StatusCode::UNAUTHORIZED,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
        assert_eq!(
            view(&app, &token, BASE_TRAN_ID).await,
            before,
            "the view once the live master is readable again"
        );
    });
}

/// The view route answers ENTER alone (P-COTRN01C-002, P-COTRN01C-008): one
/// `GET` whose only parameter is the Tran ID in the path and which takes no
/// body, so no AID key and no key field reach it. PF4 Clear and
/// `INITIALIZE-ALL-FIELDS` have no request (P-COTRN01C-010, P-COTRN01C-011,
/// E-62): the route serves no other method, and the view's schema has one
/// property per display field the paragraph clears, the 13 the row names.
#[test]
fn ct01_takes_the_tran_id_from_the_route_and_has_no_clear() {
    let parity = common::parity_audit();
    reviewed_result(&parity, "P-COTRN01C-002", &[messages::CCDA_MSG_INVALID_KEY]);
    reviewed_result(
        &parity,
        "P-COTRN01C-008",
        &["route parameter", "no key field"],
    );
    reviewed_result(
        &parity,
        "P-COTRN01C-010",
        &["(P-COTRN01C-011)", "no control"],
    );
    reviewed_result(
        &parity,
        "P-COTRN01C-011",
        &["the 13 display fields", "no control"],
    );
    let cleared = reviewed_cell(&parity, "P-COTRN01C-011", "Copybook field or PIC clause");
    let fields: Vec<&str> = cleared
        .split(';')
        .nth(1)
        .map(|part| {
            part.trim()
                .trim_end_matches("(COTRN1AI)")
                .split(',')
                .map(str::trim)
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<&str> = DISPLAY_FIELDS.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        fields.split_first(),
        Some((&"TRNIDINI", names.as_slice())),
        "the key and the display fields P-COTRN01C-011 clears: {cleared}"
    );

    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the API document is no JSON: {error}"));
    let item = &spec["paths"]["/transactions/{id}"];
    assert_eq!(
        member_names(item),
        ["get"],
        "the operations of the view route: {item}"
    );
    let get = &item["get"];
    let parameters: Vec<(&str, &str)> = get["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|parameter| {
            (
                parameter["name"].as_str().unwrap_or_default(),
                parameter["in"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(parameters, [("id", "path")], "the view's parameters: {get}");
    assert!(
        get.get("requestBody").is_none(),
        "the view takes a body: {get}"
    );
    let schema = &spec["components"]["schemas"]["TransactionResponse"]["properties"];
    let mut properties = member_names(schema);
    properties.sort_unstable();
    let mut members: Vec<&str> = DISPLAY_FIELDS.iter().map(|(_, member)| *member).collect();
    members.sort_unstable();
    assert_eq!(properties, members, "the view's properties: {schema}");
}

/// The `COTRN01C` rows of the validation audit are exactly the three the tests
/// above answer, so a row added to the audit fails here until a test sends its
/// input.
#[test]
fn the_tests_here_answer_every_cotrn01c_validation_row() {
    assert_eq!(
        common::validation_ids("COTRN01C"),
        ["V-COTRN01C-001", "V-COTRN01C-002", "V-COTRN01C-003"],
        "the COTRN01C rows of the validation audit"
    );
}
