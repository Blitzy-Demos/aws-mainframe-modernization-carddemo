//! g1a, the `OpenAPI` contract of the REST API (AAP 0.9.1): the document `api::app::openapi()`
//! emits holds exactly the 27 operations of AAP 0.3.4.1, and every one of them, called over a
//! socket against a schema-ready, fixture-loaded database with the administrator's token,
//! answers with a status that its own document entry declares.
//!
//! - `document_lists_exactly_the_27_operations` needs no Docker. It reads the emitted document
//!   and checks the operation set, the operation IDs, the bearer scheme and which operations
//!   require it, the schema names the frontend imports, the statuses Bounded Pagination and the
//!   admin gate reserve, the formats of the typed dates and timestamps, and that the committed
//!   `modern/openapi/openapi.json` is the same document.
//! - `every_operation_answers_with_a_declared_status` starts one fixture-loaded `postgres:16`
//!   container of `test_support`, serves `api::app::app` on a loopback port and calls the 27
//!   operations once each over HTTP, after signing on as `ADMIN001` through endpoint 1.
//!
//! This is a pattern-lane schema test, so it carries no audit tag, and it asserts a declared
//! status, never a parity result. Every live input writes nothing: the reads only read, ENTER
//! validates without saving, endpoint 15's blank confirmation only shows the balance, and every
//! other write names a record that does not exist or is refused by an edit, so no call depends
//! on another's effects.
//! Assertion messages name the endpoint, its method, its path template and the status, never a
//! request URI or a response body (Cardholder Data Redaction).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use api::app::{AppState, app, openapi};
use chrono::NaiveDate;
use domain::clock::{Clock, FixedClock};
use domain::online::copaus0c::AuthKey;
use persistence::PgStore;
use reqwest::header::CONTENT_TYPE;
use reqwest::{Client, Method, StatusCode};
use runtime::config::Config;
use serde_json::{Map, Value, json};
use test_support::fixtures::start_loaded_postgres;
use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn Error>>;
type Fallible<T> = Result<T, Box<dyn Error>>;

/// The 27 operations of AAP 0.3.4.1 in endpoint order, endpoint 1 first: method, path and
/// operation ID.
const OPERATIONS: [(&str, &str, &str); 27] = [
    ("POST", "/api/v1/signon", "cosgn00c_signon"),
    ("GET", "/api/v1/menus/main", "comen01c_main_menu"),
    ("GET", "/api/v1/menus/admin", "coadm01c_admin_menu"),
    ("GET", "/api/v1/accounts/{acct_id}/view", "coactvwc_view"),
    ("GET", "/api/v1/accounts/{acct_id}", "coactupc_fetch"),
    ("PUT", "/api/v1/accounts/{acct_id}", "coactupc_save"),
    ("GET", "/api/v1/cards", "cocrdlic_list"),
    ("GET", "/api/v1/cards/view", "cocrdslc_view"),
    ("GET", "/api/v1/cards/edit", "cocrdupc_fetch"),
    ("PUT", "/api/v1/cards/{card_ref}", "cocrdupc_save"),
    ("GET", "/api/v1/transactions", "cotrn00c_list"),
    ("GET", "/api/v1/transactions/{tran_id}", "cotrn01c_view"),
    ("POST", "/api/v1/transactions", "cotrn02c_add"),
    ("POST", "/api/v1/reports/transactions", "corpt00c_submit"),
    ("POST", "/api/v1/bill-payments", "cobil00c_pay"),
    ("GET", "/api/v1/users", "cousr00c_list"),
    ("POST", "/api/v1/users", "cousr01c_add"),
    ("GET", "/api/v1/users/{user_id}", "cousr02c_fetch"),
    ("PUT", "/api/v1/users/{user_id}", "cousr02c_update"),
    ("DELETE", "/api/v1/users/{user_id}", "cousr03c_delete"),
    (
        "GET",
        "/api/v1/accounts/{acct_id}/authorizations",
        "copaus0c_summary",
    ),
    (
        "GET",
        "/api/v1/authorizations/{acct_id}/{auth_key}",
        "copaus1c_detail",
    ),
    (
        "POST",
        "/api/v1/authorizations/{acct_id}/{auth_key}/fraud",
        "copaus1c_toggle_fraud",
    ),
    ("GET", "/api/v1/transaction-types", "cotrtlic_list"),
    (
        "POST",
        "/api/v1/transaction-types/list-actions",
        "cotrtlic_list_actions",
    ),
    (
        "GET",
        "/api/v1/transaction-types/{type_code}",
        "cotrtupc_fetch",
    ),
    (
        "POST",
        "/api/v1/transaction-types/{type_code}/actions",
        "cotrtupc_action",
    ),
];

/// The distinct paths of [`OPERATIONS`]: five paths carry two or three methods.
const PATH_COUNT: usize = 22;

/// Endpoint 1, sign-on: it issues the token, so it alone requires none.
const SIGNON: u8 = 1;

/// Endpoint 25, the one collection whose `page_size` travels in the request body.
const LIST_ACTIONS: u8 = 25;

/// The `[A]` operations of AAP 0.3.4.1, behind the `AdminUser` gate.
const ADMIN_ONLY: [u8; 10] = [3, 16, 17, 18, 19, 20, 24, 25, 26, 27];

/// The collection operations of Bounded Pagination, the only ones that may answer 422.
const COLLECTIONS: [u8; 6] = [7, 11, 16, 21, 24, 25];

/// The keys of an `OpenAPI` path item that name an operation.
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Where `api::app::app` serves the document, outside the document's own operations.
const DOCUMENT_PATH: &str = "/api-docs/openapi.json";

/// The prefix of every schema reference.
const SCHEMA_REF: &str = "#/components/schemas/";

/// The schemas the frontend names as `components["schemas"][…]` (AAP 0.6.2), with the shared
/// and query types.
const SCHEMAS: [&str; 44] = [
    "SignonRequest",
    "SignonResponse",
    "ScreenError",
    "ScreenResult",
    "PageDirection",
    "MenuResponse",
    "AccountViewResponse",
    "AccountUpdateView",
    "AccountUpdateRequest",
    "AccountUpdateResult",
    "CardListQuery",
    "CardListResponse",
    "CardLookupQuery",
    "CardViewResponse",
    "CardUpdateView",
    "CardUpdateRequest",
    "CardUpdateResult",
    "TranListQuery",
    "TranListResponse",
    "TranViewResponse",
    "TranAddRequest",
    "TranAddResult",
    "ReportRequest",
    "ReportResult",
    "BillPaymentRequest",
    "BillPaymentResult",
    "UserListQuery",
    "UserListResponse",
    "UserAddRequest",
    "UserView",
    "UserUpdateRequest",
    "AuthSummaryQuery",
    "AuthSummaryResponse",
    "AuthDetailQuery",
    "AuthDetailResponse",
    "FraudToggleResult",
    "TranTypeListQuery",
    "TranTypeListResponse",
    "TranTypeListState",
    "TranTypeListActionRequest",
    "TranTypeListActionResult",
    "TranTypeView",
    "TranTypeActionRequest",
    "TranTypeActionResult",
];

/// Typed dates and timestamps of the responses: schema, property and string format.
///
/// `X(10)` date fields are `date`, `X(26)` timestamps `date-time`. The row date of
/// `TranListResponse` is its `TranListRow` member `orig_ts`, from the `TRAN-ORIG-TS PIC X(26)`
/// timestamp, so it is a `date-time`; its `display_date` is the `MM/DD/YY` text the screen
/// shows and no date field. The raw date parts that `AccountUpdateRequest`, `ReportRequest` and
/// `TranAddRequest` carry to the programs' date edits stay strings and are not listed.
const DATE_PROPERTIES: [(&str, &str, &str); 8] = [
    ("AccountViewResponse", "open_date", "date"),
    ("AccountViewResponse", "expiration_date", "date"),
    ("AccountViewResponse", "reissue_date", "date"),
    ("AccountViewResponse", "date_of_birth", "date"),
    ("CardViewResponse", "expiration_date", "date"),
    ("TranListRow", "orig_ts", "date-time"),
    ("TranViewResponse", "orig_ts", "date-time"),
    ("TranViewResponse", "proc_ts", "date-time"),
];

/// The emitted document as JSON.
fn document() -> Fallible<Value> {
    Ok(serde_json::to_value(openapi())?)
}

/// The `paths` object of `doc`.
fn paths(doc: &Value) -> Fallible<&Map<String, Value>> {
    Ok(doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or("the document has no paths object")?)
}

/// The operation `method` (upper case) of `path`.
fn operation<'d>(doc: &'d Value, method: &str, path: &str) -> Fallible<&'d Value> {
    paths(doc)?
        .get(path)
        .and_then(|item| item.get(method.to_ascii_lowercase()))
        .ok_or_else(|| format!("the document has no {method} {path}").into())
}

/// The schema `name` of `components.schemas`.
fn schema<'d>(doc: &'d Value, name: &str) -> Fallible<&'d Value> {
    doc.pointer("/components/schemas")
        .and_then(|schemas| schemas.get(name))
        .ok_or_else(|| format!("components.schemas has no {name}").into())
}

/// The property names of the object schema `schema`.
fn property_names(schema: &Value) -> Fallible<BTreeSet<&str>> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("the schema has no properties")?;
    Ok(properties.keys().map(String::as_str).collect())
}

/// The response statuses `operation` declares, as the keys of its `responses` object.
fn declared_statuses(operation: &Value) -> Fallible<BTreeSet<String>> {
    let responses = operation
        .get("responses")
        .and_then(Value::as_object)
        .ok_or("the operation has no responses object")?;
    Ok(responses.keys().cloned().collect())
}

/// The page-size schema of the collection `operation`: its `page_size` query parameter or, when
/// `in_body`, the `page_size` property of its JSON request body.
fn page_size_schema<'d>(
    doc: &'d Value,
    operation: &'d Value,
    in_body: bool,
) -> Fallible<&'d Value> {
    if in_body {
        let target = operation
            .pointer("/requestBody/content/application~1json/schema/$ref")
            .and_then(Value::as_str)
            .ok_or("the operation has no JSON request body schema")?;
        let name = target
            .strip_prefix(SCHEMA_REF)
            .ok_or("the request body is no components.schemas reference")?;
        schema(doc, name)?
            .pointer("/properties/page_size")
            .ok_or_else(|| format!("{name} has no page_size property").into())
    } else {
        operation
            .get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|parameter| {
                parameter.get("name").and_then(Value::as_str) == Some("page_size")
                    && parameter.get("in").and_then(Value::as_str) == Some("query")
            })
            .and_then(|parameter| parameter.get("schema"))
            .ok_or_else(|| "the operation has no page_size query parameter".into())
    }
}

/// Whether `schema` is a reference to the schema `name`, alone or as one of its `oneOf`
/// variants.
fn references(schema: &Value, name: &str) -> bool {
    let target = format!("{SCHEMA_REF}{name}");
    let refers =
        |member: &Value| member.get("$ref").and_then(Value::as_str) == Some(target.as_str());
    refers(schema)
        || schema
            .get("oneOf")
            .and_then(Value::as_array)
            .is_some_and(|variants| variants.iter().any(refers))
}

/// Whether the property schema `member` is a string, alone or with `null`.
fn is_string_type(member: &Value) -> bool {
    match member.get("type") {
        Some(Value::String(name)) => name == "string",
        Some(Value::Array(names)) => names.iter().any(|name| name.as_str() == Some("string")),
        _ => false,
    }
}

/// The paths, the (method, path) pairs and the operation IDs.
fn check_operations(doc: &Value) -> TestResult {
    let paths = paths(doc)?;
    assert_eq!(paths.len(), PATH_COUNT, "distinct paths of the document");
    assert!(
        !paths.contains_key(DOCUMENT_PATH),
        "the served document is listed as one of its own operations"
    );
    let mut found = BTreeSet::new();
    for (path, item) in paths {
        let item = item
            .as_object()
            .ok_or_else(|| format!("{path} is not a path item"))?;
        for method in METHODS
            .into_iter()
            .filter(|method| item.contains_key(*method))
        {
            found.insert((method.to_ascii_uppercase(), path.clone()));
        }
    }
    let expected: BTreeSet<(String, String)> = OPERATIONS
        .iter()
        .map(|&(method, path, _)| (method.to_owned(), path.to_owned()))
        .collect();
    assert_eq!(
        expected.len(),
        OPERATIONS.len(),
        "the expected table repeats a pair"
    );
    let missing: Vec<_> = expected.difference(&found).collect();
    let extra: Vec<_> = found.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the document lacks the operations {missing:?} and adds {extra:?}"
    );
    for (number, (method, path, id)) in (1_u8..).zip(OPERATIONS) {
        let found_id = operation(doc, method, path)?
            .get("operationId")
            .and_then(Value::as_str);
        assert_eq!(
            found_id,
            Some(id),
            "endpoint {number} ({method} {path}): operationId"
        );
    }
    Ok(())
}

/// The bearer scheme, and the requirement every operation but sign-on declares.
fn check_security(doc: &Value) -> TestResult {
    assert_eq!(
        doc.get("security"),
        None,
        "a document-wide requirement would bind sign-on too"
    );
    let scheme = doc
        .pointer("/components/securitySchemes/bearer_auth")
        .ok_or("components.securitySchemes has no bearer_auth")?;
    assert_eq!(scheme.get("type"), Some(&json!("http")), "bearer_auth type");
    assert_eq!(
        scheme.get("scheme"),
        Some(&json!("bearer")),
        "bearer_auth scheme"
    );
    let bearer = json!([{"bearer_auth": []}]);
    for (number, (method, path, _)) in (1_u8..).zip(OPERATIONS) {
        let security = operation(doc, method, path)?.get("security");
        if number == SIGNON {
            assert_eq!(
                security, None,
                "sign-on issues the token, so it requires none"
            );
        } else {
            assert_eq!(
                security,
                Some(&bearer),
                "endpoint {number} ({method} {path}): security requirement"
            );
        }
    }
    Ok(())
}

/// The schema names, and the members of `ReportResult` and `ScreenError`.
fn check_schemas(doc: &Value) -> TestResult {
    let schemas = doc
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .ok_or("the document has no components.schemas")?;
    let names: BTreeSet<&str> = SCHEMAS.into_iter().collect();
    assert_eq!(
        names.len(),
        SCHEMAS.len(),
        "the expected schema list repeats a name"
    );
    let missing: Vec<&str> = names
        .into_iter()
        .filter(|name| !schemas.contains_key(*name))
        .collect();
    assert!(missing.is_empty(), "components.schemas lacks {missing:?}");

    let report = schema(doc, "ReportResult")?;
    assert_eq!(
        property_names(report)?,
        BTreeSet::from(["job_deck", "message"]),
        "ReportResult properties"
    );
    assert_eq!(
        report.get("required"),
        Some(&json!(["message"])),
        "ReportResult: only message is required, job_deck is optional"
    );
    let error = schema(doc, "ScreenError")?;
    assert_eq!(
        property_names(error)?,
        BTreeSet::from(["field", "message"]),
        "ScreenError properties: the status travels as the response status only"
    );
    assert_eq!(
        error.get("required"),
        Some(&json!(["message"])),
        "ScreenError: only message is required, field is optional"
    );
    Ok(())
}

/// The statuses Bounded Pagination and the gates reserve, and the bounded `page_size` of every
/// collection.
fn check_statuses(doc: &Value) -> TestResult {
    for (number, (method, path, _)) in (1_u8..).zip(OPERATIONS) {
        let label = format!("endpoint {number} ({method} {path})");
        let entry = operation(doc, method, path)?;
        let codes = declared_statuses(entry)?;
        let collection = COLLECTIONS.contains(&number);
        assert_eq!(
            codes.contains("422"),
            collection,
            "{label}: 422 is declared on the collections and on nothing else"
        );
        assert_eq!(
            codes.contains("403"),
            ADMIN_ONLY.contains(&number),
            "{label}: 403 is declared on the admin operations and on nothing else"
        );
        if number != SIGNON {
            assert!(
                codes.contains("401"),
                "{label}: a bearer operation declares 401"
            );
        }
        if collection {
            let page_size = page_size_schema(doc, entry, number == LIST_ACTIONS)?;
            assert_eq!(
                page_size.get("minimum"),
                Some(&json!(1)),
                "{label}: page_size minimum"
            );
            assert_eq!(
                page_size.get("maximum"),
                Some(&json!(100)),
                "{label}: page_size maximum"
            );
        }
    }
    Ok(())
}

/// The string formats of the typed dates and timestamps, reached from `TranListResponse` for
/// its row.
fn check_dates(doc: &Value) -> TestResult {
    let rows = schema(doc, "TranListResponse")?
        .pointer("/properties/rows/items")
        .ok_or("TranListResponse has no rows items")?;
    assert!(
        references(rows, "TranListRow"),
        "TranListResponse rows are TranListRow"
    );
    for (name, property, format) in DATE_PROPERTIES {
        let member = schema(doc, name)?
            .pointer(&format!("/properties/{property}"))
            .ok_or_else(|| format!("{name} has no {property}"))?;
        assert!(is_string_type(member), "{name}.{property} is a string");
        assert_eq!(
            member.get("format").and_then(Value::as_str),
            Some(format),
            "{name}.{property} format"
        );
    }
    Ok(())
}

/// The committed contract, compared as JSON values; `scripts/check.sh` compares the bytes.
fn check_committed(doc: &Value) -> TestResult {
    let path = test_support::modern_dir().join("openapi/openapi.json");
    let committed: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    assert!(
        committed == *doc,
        "modern/openapi/openapi.json differs from api::app::openapi(): run scripts/regen-api.sh \
         and commit the regenerated files"
    );
    Ok(())
}

#[test]
fn document_lists_exactly_the_27_operations() -> TestResult {
    let doc = document()?;
    check_operations(&doc)?;
    check_security(&doc)?;
    check_schemas(&doc)?;
    check_statuses(&doc)?;
    check_dates(&doc)?;
    check_committed(&doc)
}

/// The HS256 secret of the test state: a local-only value.
const SECRET: &str = "openapi-contract-test-secret";

/// The administrator of the `DUSRSECJ` deck and the password of every deck user (README).
const ADMIN_USER: &str = "ADMIN001";
const PASSWORD: &str = "PASSWORD";

/// Fixture account 1 (`acctdata.txt` line 1), which endpoint 6 validates and endpoint 15 shows.
const FIXTURE_ACCOUNT: &str = "00000000001";

/// The account of the first `cardxref.txt` record, whose authorizations endpoints 22 and 23
/// look up. No authorization is loaded, so every key names nothing.
const AUTH_ACCOUNT: &str = "00000000050";

/// A user of the `DUSRSECJ` deck, whose unchanged names endpoint 19 sends back.
const FIXTURE_USER: &str = "USER0002";

/// The Db2 transaction type endpoint 27 validates, seeded by the schema's `db2` version.
const TYPE_CODE: &str = "01";

/// The input rows of COTRTLIC's map, all of which endpoint 25 receives.
const MAP_ROWS: usize = 7;

/// The date members of COACTUPC's view and the prefix of their year, month and day inputs.
const ACCOUNT_DATES: [(&str, &str); 4] = [
    ("open_date", "open"),
    ("expiry_date", "expiry"),
    ("reissue_date", "reissue"),
    ("dob", "dob"),
];

/// The calls whose input needs no earlier response: endpoint number, request URI and JSON body.
///
/// Endpoints 8 and 9 name the first `cardxref.txt` card under its account. Endpoint 12 names
/// a transaction the empty table does not hold, 20 a user the deck does not hold, and 13, 14
/// and 17 send blank input the programs refuse; 15 sends a blank confirmation, which shows the
/// balance and pays nothing.
const INDEPENDENT: [(u8, &str, Option<&str>); 19] = [
    (2, "/api/v1/menus/main", None),
    (3, "/api/v1/menus/admin", None),
    (4, "/api/v1/accounts/00000000001/view", None),
    (5, "/api/v1/accounts/00000000001", None),
    (7, "/api/v1/cards", None),
    (
        8,
        "/api/v1/cards/view?acct_id=00000000050&card_num=0500024453765740",
        None,
    ),
    (
        9,
        "/api/v1/cards/edit?acct_id=00000000050&card_num=0500024453765740",
        None,
    ),
    (11, "/api/v1/transactions", None),
    (12, "/api/v1/transactions/0000000000000001", None),
    (
        13,
        "/api/v1/transactions",
        Some(r#"{"aid":"ENTER","acct_id":"","card_num":"","fields":{},"confirm":""}"#),
    ),
    (
        14,
        "/api/v1/reports/transactions",
        Some(r#"{"monthly":"","yearly":"","custom":"","confirm":""}"#),
    ),
    (
        15,
        "/api/v1/bill-payments",
        Some(r#"{"acct_id":"00000000001","confirm":""}"#),
    ),
    (16, "/api/v1/users", None),
    (
        17,
        "/api/v1/users",
        Some(r#"{"first_name":"","last_name":"","user_id":"","password":"","user_type":""}"#),
    ),
    (18, "/api/v1/users/USER0002", None),
    (20, "/api/v1/users/NOSUCHU1", None),
    (21, "/api/v1/accounts/00000000001/authorizations", None),
    (24, "/api/v1/transaction-types", None),
    (26, "/api/v1/transaction-types/01", None),
];

/// One live request: its endpoint number, the URI under the server's origin and the JSON body.
struct Call {
    number: u8,
    uri: String,
    body: Option<Value>,
}

/// The calls of [`INDEPENDENT`], then endpoints 22 and 23 for the lowest authorization key,
/// which is well formed and which no detail holds.
fn independent_calls() -> Fallible<Vec<Call>> {
    let mut calls = Vec::with_capacity(INDEPENDENT.len() + 2);
    for (number, uri, body) in INDEPENDENT {
        let body = body.map(serde_json::from_str::<Value>).transpose()?;
        calls.push(Call {
            number,
            uri: uri.to_owned(),
            body,
        });
    }
    let absent = AuthKey::lowest().render();
    calls.push(Call {
        number: 22,
        uri: format!("/api/v1/authorizations/{AUTH_ACCOUNT}/{absent}"),
        body: None,
    });
    calls.push(Call {
        number: 23,
        uri: format!("/api/v1/authorizations/{AUTH_ACCOUNT}/{absent}/fraud"),
        body: None,
    });
    Ok(calls)
}

/// The response body endpoint `number` returned.
fn response(bodies: &BTreeMap<u8, Value>, number: u8) -> Fallible<&Value> {
    Ok(bodies
        .get(&number)
        .ok_or_else(|| format!("endpoint {number} was not called"))?)
}

/// The member `key` of the response `body`, which must be present.
fn member(body: &Value, key: &str) -> Fallible<Value> {
    Ok(body
        .get(key)
        .cloned()
        .ok_or_else(|| format!("the response has no {key}"))?)
}

/// The typed date `key` of `view` as the screen's year, month and day parts, blank for `null`.
fn date_parts(view: &Value, key: &str) -> Fallible<[String; 3]> {
    match view.get(key).and_then(Value::as_str) {
        None => Ok(<[String; 3]>::default()),
        Some(text) => {
            let date = NaiveDate::parse_from_str(text, "%Y-%m-%d")?;
            Ok(["%Y", "%m", "%d"].map(|part| date.format(part).to_string()))
        }
    }
}

/// The `fields` a screen sends back unchanged after endpoint 5's `view`: every
/// `AccountUpdateFields` member the view shows as text, the dates as their parts, and no SSN
/// part, which leaves the SSN unchanged.
fn account_fields(doc: &Value, view: &Value) -> Fallible<Value> {
    let names = property_names(schema(doc, "AccountUpdateFields")?)?;
    let mut fields = Map::new();
    for name in names {
        if let Some(text) = view.get(name).filter(|value| value.is_string()) {
            fields.insert(name.to_owned(), text.clone());
        }
    }
    for (date, prefix) in ACCOUNT_DATES {
        let parts = date_parts(view, date)?;
        for (part, text) in ["year", "month", "day"].into_iter().zip(parts) {
            fields.insert(format!("{prefix}_{part}"), Value::from(text));
        }
    }
    Ok(Value::Object(fields))
}

/// The calls built from earlier responses, each an ENTER or an unchanged update: endpoint 6
/// over the view of 5, 10 over the card of 9, 19 over the user of 18, 25 over the state of 24
/// and 27 over the type of 26.
fn dependent_calls(doc: &Value, bodies: &BTreeMap<u8, Value>) -> Fallible<Vec<Call>> {
    let account = response(bodies, 5)?;
    let card = response(bodies, 9)?;
    let user = response(bodies, 18)?;
    let type_list = response(bodies, 24)?;
    let type_view = response(bodies, 26)?;
    let card_ref = card
        .get("card_ref")
        .and_then(Value::as_i64)
        .ok_or("endpoint 9 returned no card_ref")?;
    let [expiry_year, expiry_month, _] = date_parts(card, "expiration_date")?;
    let card_fields = json!({
        "embossed_name": member(card, "embossed_name")?,
        "status": member(card, "status")?,
        "expiry_month": expiry_month,
        "expiry_year": expiry_year,
    });
    let blank_rows = vec![json!({"selection": "", "description": ""}); MAP_ROWS];
    let payloads = [
        json!({
            "aid": "ENTER",
            "fields": account_fields(doc, account)?,
            "snapshot": member(account, "snapshot")?,
        }),
        json!({"aid": "ENTER", "fields": card_fields, "snapshot": member(card, "snapshot")?}),
        json!({
            "first_name": member(user, "first_name")?,
            "last_name": member(user, "last_name")?,
            "user_type": member(user, "user_type")?,
        }),
        json!({
            "aid": "ENTER",
            "type_filter": "",
            "desc_filter": "",
            "rows": blank_rows,
            "state": member(type_list, "state")?,
        }),
        json!({
            "aid": "ENTER",
            "description": type_view.get("description").cloned().unwrap_or_default(),
        }),
    ];
    let targets = [
        (6, format!("/api/v1/accounts/{FIXTURE_ACCOUNT}")),
        (10, format!("/api/v1/cards/{card_ref}")),
        (19, format!("/api/v1/users/{FIXTURE_USER}")),
        (25, "/api/v1/transaction-types/list-actions".to_owned()),
        (27, format!("/api/v1/transaction-types/{TYPE_CODE}/actions")),
    ];
    Ok(targets
        .into_iter()
        .zip(payloads)
        .map(|((number, uri), body)| Call {
            number,
            uri,
            body: Some(body),
        })
        .collect())
}

/// Whether the path of `uri` instantiates the path template `template`: the same segments, a
/// `{param}` segment standing for any non-empty one.
fn instantiates(uri: &str, template: &str) -> bool {
    let path = uri.split_once('?').map_or(uri, |(path, _)| path);
    let given: Vec<&str> = path.split('/').collect();
    let expected: Vec<&str> = template.split('/').collect();
    given.len() == expected.len()
        && given.iter().zip(&expected).all(|(segment, pattern)| {
            segment == pattern
                || (pattern.starts_with('{') && pattern.ends_with('}') && !segment.is_empty())
        })
}

/// Whether `body` has the `ScreenError` shape: a string `message`, an optional string `field`,
/// and no other member.
fn is_screen_error(body: &Value) -> bool {
    body.as_object().is_some_and(|members| {
        members.get("message").is_some_and(Value::is_string)
            && members.get("field").is_none_or(Value::is_string)
            && members.keys().all(|key| key == "message" || key == "field")
    })
}

/// The test clock: 2022-07-18 10:30:00.000.
fn clock() -> Fallible<Arc<dyn Clock>> {
    let at = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|day| day.and_hms_milli_opt(10, 30, 0, 0))
        .ok_or("an invalid test instant")?;
    Ok(Arc::new(FixedClock::new(at)))
}

/// An HTTP client of the API served at `base`, with the document every response is checked
/// against and the endpoints called so far.
struct Live {
    client: Client,
    base: String,
    doc: Value,
    /// The administrator's bearer token, once endpoint 1 has issued it.
    token: Option<String>,
    called: Vec<u8>,
}

impl Live {
    /// The document route answers without a token, and serves `api::app::openapi()`.
    async fn document_is_served(&self) -> TestResult {
        let response = self
            .client
            .get(format!("{}{DOCUMENT_PATH}", self.base))
            .send()
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "GET {DOCUMENT_PATH} without a token"
        );
        let served: Value = response.json().await?;
        assert!(
            served == self.doc,
            "GET {DOCUMENT_PATH} serves a document other than api::app::openapi()"
        );
        Ok(())
    }

    /// Endpoint 1 for the administrator, keeping the token it issues.
    async fn sign_on(&mut self) -> TestResult {
        let credentials = json!({"user_id": ADMIN_USER, "password": PASSWORD});
        let call = Call {
            number: SIGNON,
            uri: "/api/v1/signon".to_owned(),
            body: Some(credentials),
        };
        let body = self.call(&call).await?;
        assert_eq!(
            body.get("user_type").and_then(Value::as_str),
            Some("A"),
            "{ADMIN_USER} signs on with user type A"
        );
        let token = body
            .get("token")
            .and_then(Value::as_str)
            .ok_or("sign-on returned no token")?;
        self.token = Some(token.to_owned());
        Ok(())
    }

    /// Sends `call` and checks its answer against the document: a status its operation
    /// declares, never 401 or 403, a JSON body, and a `ScreenError` for every status outside
    /// 2xx. Returns the body.
    async fn call(&mut self, call: &Call) -> Fallible<Value> {
        let number = call.number;
        let &(method, path, _) = usize::from(number)
            .checked_sub(1)
            .and_then(|index| OPERATIONS.get(index))
            .ok_or_else(|| format!("no endpoint {number}"))?;
        let label = format!("endpoint {number} ({method} {path})");
        let declared = declared_statuses(operation(&self.doc, method, path)?)?;
        assert!(
            instantiates(&call.uri, path),
            "{label}: the request URI does not instantiate the path"
        );

        let mut request = self.client.request(
            Method::from_bytes(method.as_bytes())?,
            format!("{}{}", self.base, call.uri),
        );
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = &call.body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        let json_content = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        let bytes = response.bytes().await?;

        assert!(
            declared.contains(&status.as_u16().to_string()),
            "{label} answered {status}, a status its document entry does not declare"
        );
        assert!(
            !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "{label} refused the administrator's valid token with {status}"
        );
        assert!(
            json_content,
            "{label}: the {status} response is not application/json"
        );
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|_| format!("{label}: the {status} body is not JSON"))?;
        if !status.is_success() {
            assert!(
                is_screen_error(&body),
                "{label}: the {status} body is not a ScreenError"
            );
        }
        self.called.push(number);
        Ok(body)
    }
}

/// The live calls against the API served at `base`: the document route, sign-on, then every
/// other operation once.
async fn exercise(base: String) -> TestResult {
    let mut live = Live {
        client: Client::builder().no_proxy().build()?,
        base,
        doc: document()?,
        token: None,
        called: Vec::new(),
    };
    live.document_is_served().await?;
    live.sign_on().await?;
    let mut bodies = BTreeMap::new();
    for call in independent_calls()? {
        let body = live.call(&call).await?;
        bodies.insert(call.number, body);
    }
    for call in dependent_calls(&live.doc, &bodies)? {
        live.call(&call).await?;
    }
    live.called.sort_unstable();
    let every: Vec<u8> = (1..=27).collect();
    assert_eq!(
        live.called, every,
        "each of the 27 operations is called exactly once"
    );
    Ok(())
}

#[tokio::test]
async fn every_operation_answers_with_a_declared_status() -> TestResult {
    // Declared first, so it drops last: dropping the guard removes the container.
    let database = start_loaded_postgres().await?;
    let config = Config {
        jwt_secret: SECRET.to_owned(),
        intrdr_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("openapi-contract-jobs"),
        ..Config::default()
    };
    let store = PgStore::new(database.pool().clone());
    let state = AppState::from_config(store, &config, clock()?)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(axum::serve(listener, app(state)).into_future());

    let outcome = exercise(base).await;
    server.abort();
    // Awaiting the aborted task drops the state and its pool before the container goes.
    let stopped = server.await;
    outcome?;
    match stopped {
        Err(join) if join.is_cancelled() => Ok(()),
        Err(join) => Err(join.into()),
        Ok(ended) => ended.map_err(Into::into),
    }
}
