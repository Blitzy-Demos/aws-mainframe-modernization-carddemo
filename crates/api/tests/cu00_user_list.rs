//! CU00, the user list of `COUSR00C`, through the HTTP layer:
//! `GET /admin/users?start_key=&direction=&page_size=`.
//!
//! Every case is a real request through the router on a seeded
//! `postgres:16.15` container, signed on as the seeded administrator
//! `ADMIN001`, so it crosses the extractors, the bearer token, the status
//! mapping and the `{code, message, fields}` body the SPA receives.
//!
//! - The seeded first page, before any write: the `DUSRSECJ` users in
//!   `SEC-USR-ID` order with their names and types, right-trimmed, compared
//!   with the users the reviewed parity count row decodes with the reviewed
//!   `CSUSR01Y` layout, one screen of the reviewed `COUSR00C` page size, and
//!   with the `cu00_user_list__first_page` snapshot.
//! - The page-boundary texts `COUSR00C` shows without its error flag, each in
//!   `message` of a 200 page (E-54): PF8 without a next page, PF7 on the
//!   first page, the end and the start of the file, and a start key with no
//!   user at or after it (E-71).
//! - Paging over fifteen users, five of them added through
//!   `POST /admin/users`: forward pages followed by `next_key` concatenate to
//!   every user once, in order, and backward pages from `prev_key` give back
//!   the previous page, at the default screen size the reviewed `COUSR00C`
//!   row records and at 5.
//! - The `USRIDIN` answers: a start key the REST query can carry but a 3270
//!   field cannot (E-49), and the browse failures of `STARTBR`, `READNEXT`
//!   and `READPREV`, injected through SQL.
//! - No response body carries a password, a hash member or an argon2id hash
//!   (E-21).
//!
//! - The AID keys of `MAIN-PARA`: ENTER, PF7 and PF8 are the published
//!   requests the function map names for them, PF3 and the invalid-key
//!   answer send none, and the list takes no key input, so a request naming
//!   any other key is answered as ENTER, never with the invalid-key text.
//! - `RETURN-TO-PREV-SCREEN`: PF3 is navigation, and an entry without a
//!   session (`EIBCALEN = 0`) is 401 before any user is read.
//!
//! Expected texts are the constants of `carddemo_domain::messages`, each
//! checked against the reviewed `COUSR00C` rows of
//! `docs/parity/validation-audit.md` for its text, status and BMS field.
//! Docker must be running: each test starts its own container.

pub mod common;

use std::collections::BTreeSet;

use carddemo_api::dto::admin_users::UserListField;
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, Repo, Row, Table, code_span, id_tokens, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::fixtures;
use reqwest::StatusCode;
use serde_json::{Value, json};

use common::{Resp, SeededUser, TestApp};

/// The program whose audit rows this file proves.
const PROGRAM: &str = "COUSR00C";

/// The CU00 list route.
const ADMIN_USERS: &str = "/admin/users";

/// The BMS field every `COUSR00C` answer names: each path moves -1 to
/// `USRIDINL`, so the cursor stays on the search field.
const USRIDIN: &str = UserListField::UsrIdIn.as_str();

/// The parity row of `PROCESS-PAGE-FORWARD`, whose `page_size=` is the rows
/// of one `COUSR0A` screen: the default `page_size`.
const SCREEN_ROW: &str = "P-COUSR00C-003";

/// The count row of the `DUSRSECJ` `SYSUT1` users the seed loads
/// [app/jcl/DUSRSECJ.jcl:L35-44], whose cited records, decoded with the
/// reviewed `CSUSR01Y` layout, give the seeded users every page expects.
const SEEDED_USERS_ROW: &str = "P-CSUSR01Y-007";

/// The users added through `POST /admin/users`, as (user ID, first name,
/// last name, type): two sort between the seeded administrators and regular
/// users, three after every seeded user.
const ADDED_USERS: [(&str, &str, &str, &str); 5] = [
    ("BUSER001", "BERNARD", "BAKER", "U"),
    ("BUSER002", "BEATRICE", "BOYLE", "A"),
    ("ZUSER001", "ZELDA", "ZIMMER", "U"),
    ("ZUSER002", "ZACHARY", "ZANE", "U"),
    ("ZUSER003", "ZOE", "ZORN", "A"),
];

/// The member names that would carry a password or its hash.
const PASSWORD_MEMBERS: [&str; 3] = ["password", "password_hash", "sec_usr_pwd"];

/// The start of every argon2 PHC string, as `password_hash` stores it.
const ARGON2_PREFIX: &str = "$argon2";

/// Stores a user whose `SEC-USR-ID` holds `U+0100`, a character no `X(08)`
/// byte can hold, so reading the record fails. The CHECK that keeps such a
/// key out is dropped first; the other columns are copied from `ADMIN001`.
/// The key sorts after `ADMIN005` and [`UNREADABLE_START`] and before
/// `USER0001`.
const STORE_UNREADABLE_USER: &str = "\
    ALTER TABLE users DROP CONSTRAINT users_user_id_check; \
    INSERT INTO users (user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type) \
    SELECT 'A' || chr(256), sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type \
    FROM users WHERE user_id = 'ADMIN001'";

/// A start key no user holds whose `STARTBR` `GTEQ` position is the record
/// [`STORE_UNREADABLE_USER`] stores: above `ADMIN005`, below that record.
const UNREADABLE_START: &str = "AZ";

/// The parity row of `MAIN-PARA`'s `EVALUATE EIBAID`: ENTER, PF3, PF7, PF8
/// and any other key, each with its action.
const AID_ROW: &str = "P-COUSR00C-001";

/// The parity row of `RETURN-TO-PREV-SCREEN`: PF3, and the entry without a
/// COMMAREA that the bearer token replaces.
const RETURN_ROW: &str = "P-COUSR00C-012";

/// The `COUSR00C` screen, as the function map's `BMS map` cell names it.
const MAP: &str = "COUSR00";

/// The key the `MAIN-PARA` row names for every AID key it answers with the
/// invalid-key text.
const OTHER_KEY: &str = "other";

/// AID keys `MAIN-PARA` answers with the invalid-key text, each sent the one
/// way a request could name a key: a query parameter the list does not
/// publish.
const OTHER_KEYS: [&str; 3] = ["aid=PF9", "aid=PA1", "aid=CLEAR"];

/// The bearer of a request without a session: none, or one that does not
/// verify.
const NO_SESSION: [Option<&str>; 2] = [None, Some("not-a-token")];

/// The `code` of the 401 a request without a session answers.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The HTTP methods an `OpenAPI` path item can hold an operation under.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The request a function map `Endpoint` cell names: the lower-case method,
/// the path and the query, empty when it names none.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    method: String,
    path: String,
    query: String,
}

/// One AID key of a `MAIN-PARA` row: the key its input condition names, the
/// action its expected result gives the key, and the request the function
/// map replaces the key with, `None` for a key no request replaces.
#[derive(Debug)]
struct AidKey {
    key: String,
    action: String,
    endpoint: Option<Endpoint>,
}

impl AidKey {
    /// Whether the row's action for the key is screen handling that no
    /// request replaces: an `XCTL` to another program, the screen clear, or
    /// the invalid-key error.
    fn sends_no_request(&self) -> bool {
        ["XCTL", "clear", "error"]
            .iter()
            .any(|word| self.action.starts_with(word))
    }
}

/// The parsed `docs/parity/function-map.md` (`F-` rows).
///
/// # Panics
///
/// Panics, naming the path, if the document cannot be read.
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The request of a function map `Endpoint` cell: `None` for `—` and
/// `— (navigation)`, else the method, path and query of `METHOD /path?query`.
fn endpoint(cell: &str) -> Option<Endpoint> {
    if cell.starts_with('—') {
        return None;
    }
    let (method, target) = cell
        .split_once(' ')
        .unwrap_or_else(|| panic!("a function map endpoint is not `METHOD /path`: {cell}"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(Endpoint {
        method: method.to_ascii_lowercase(),
        path: path.to_owned(),
        query: query.to_owned(),
    })
}

/// The request the function map gives `key` on the `map` screen: the
/// `Endpoint` of the one row whose `Key or field` cell starts with the key.
fn key_endpoint(map_doc: &Document, map: &str, key: &str) -> Option<Endpoint> {
    let cells: Vec<&str> = map_doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| {
            table.cell(row, "BMS map") == Some(map)
                && table
                    .cell(row, "Key or field")
                    .and_then(|text| text.split_whitespace().next())
                    == Some(key)
        })
        .filter_map(|(table, row)| table.cell(row, "Endpoint"))
        .collect();
    match cells.as_slice() {
        [cell] => endpoint(cell),
        _ => panic!(
            "{FUNCTION_MAP_PATH} holds {} rows for {key} on {map}, not one",
            cells.len()
        ),
    }
}

/// The AID keys of the `MAIN-PARA` row `id` in the order its input condition
/// lists them, each with the action its expected result gives it and the
/// request the function map gives it on the `map` screen.
fn aid_keys(parity: &Document, map_doc: &Document, id: &str, map: &str) -> Vec<AidKey> {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the audit defines no {id} row"));
    let keys: Vec<&str> = cell(table, row, id, "Input condition")
        .split(" / ")
        .map(str::trim)
        .collect();
    let actions = key_values(cell(table, row, id, "Expected result"))
        .into_iter()
        .find(|pair| pair.key == "action")
        .unwrap_or_else(|| panic!("{id} gives no action= result"))
        .value;
    let actions: Vec<&str> = actions.split(" / ").map(str::trim).collect();
    assert_eq!(keys.len(), actions.len(), "{id} gives each key one action");
    keys.iter()
        .zip(actions)
        .map(|(key, action)| AidKey {
            key: (*key).to_owned(),
            action: action.to_owned(),
            endpoint: key_endpoint(map_doc, map, key),
        })
        .collect()
}

/// The published `OpenAPI` document.
fn spec() -> Value {
    serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"))
}

/// The HTTP methods `spec` publishes an operation under on `path`.
fn operations<'s>(spec: &'s Value, path: &str) -> BTreeSet<&'s str> {
    spec["paths"][path]
        .as_object()
        .map(|item| {
            item.keys()
                .map(String::as_str)
                .filter(|name| HTTP_METHODS.contains(name))
                .collect()
        })
        .unwrap_or_default()
}

/// Asserts that `spec` publishes the request `endpoint` the function map
/// gives `key`: the operation, each query parameter it names, and each value
/// it gives an enumerated parameter.
fn assert_published(spec: &Value, key: &AidKey, endpoint: &Endpoint) {
    let Endpoint {
        method,
        path,
        query,
    } = endpoint;
    let operation = &spec["paths"][path.as_str()][method.as_str()];
    assert!(
        operation.is_object(),
        "{AID_ROW} {}: {method} {path} is not published",
        key.key
    );
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let parameter = operation["parameters"]
            .as_array()
            .and_then(|parameters| {
                parameters
                    .iter()
                    .find(|parameter| parameter["name"] == name && parameter["in"] == "query")
            })
            .unwrap_or_else(|| panic!("{AID_ROW} {}: {method} {path} has no {name}", key.key));
        let allowed = parameter["schema"]["enum"].as_array();
        if let Some(values) = allowed.filter(|_| !value.is_empty()) {
            assert!(
                values.iter().any(|allowed| *allowed == value),
                "{AID_ROW} {}: {name}={value} is not a published value",
                key.key
            );
        }
    }
}

/// The list row `COUSR00C` `POPULATE-USER-DATA` fills for the seeded `user`.
fn seeded_item(user: &SeededUser) -> Value {
    item(
        &user.user_id,
        &user.first_name,
        &user.last_name,
        &user.usr_type,
    )
}

/// One `UserSummary` as the list returns it.
fn item(user_id: &str, first_name: &str, last_name: &str, usr_type: &str) -> Value {
    json!({
        "user_id": user_id,
        "first_name": first_name,
        "last_name": last_name,
        "usr_type": usr_type,
    })
}

/// The `user_id` of the list row `item`.
fn id_of(item: &Value) -> &str {
    item["user_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a list row without a string user_id: {item}"))
}

/// The whole page body the list answers: `items`, both keys (`null` when
/// absent) and, only when the program shows one, the page-boundary `message`.
fn expected_page(
    items: &[Value],
    next_key: Option<&str>,
    prev_key: Option<&str>,
    message: Option<&str>,
) -> Value {
    let mut page = json!({ "items": items, "next_key": next_key, "prev_key": prev_key });
    if let Some(message) = message {
        page["message"] = json!(message);
    }
    page
}

/// The query string of the non-empty `parts`, joined with `&`.
fn query(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("&")
}

/// The first member path of `value` that exposes a password: a member named
/// as one of [`PASSWORD_MEMBERS`], or a string holding an argon2 hash.
fn password_exposure(value: &Value, at: &str) -> Option<String> {
    match value {
        Value::Object(members) => members.iter().find_map(|(name, member)| {
            let path = format!("{at}.{name}");
            if PASSWORD_MEMBERS.contains(&name.to_ascii_lowercase().as_str()) {
                Some(path)
            } else {
                password_exposure(member, &path)
            }
        }),
        Value::Array(entries) => entries
            .iter()
            .enumerate()
            .find_map(|(index, entry)| password_exposure(entry, &format!("{at}[{index}]"))),
        Value::String(text) if text.starts_with(ARGON2_PREFIX) => Some(at.to_owned()),
        _ => None,
    }
}

/// Asserts that the answer `resp` to `request` carries no password: no
/// member named as one, and no argon2 hash anywhere in the raw body (E-21).
fn assert_no_password(resp: &Resp, request: &str) {
    let (_, body, raw) = resp;
    if let Some(path) = password_exposure(body, "$") {
        panic!("{request} exposes a password at {path}: {raw}");
    }
    assert!(
        !raw.contains(ARGON2_PREFIX),
        "{request} carries an argon2 hash: {raw}"
    );
}

/// The cell `name` of the audit row `row` of `table`, whose ID is `id`.
fn cell<'a>(table: &'a Table, row: &'a Row, id: &str, name: &str) -> &'a str {
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
}

/// Asserts that `doc` defines the `COUSR00C` row `id` and that the last
/// review-log verdict on it is `pass`: only reviewed rows gate a test.
fn assert_reviewed(doc: &Document, id: &str) {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the audit defines no {id} row"));
    assert_eq!(
        cell(table, row, id, "Program"),
        PROGRAM,
        "{id} is a {PROGRAM} row"
    );
    let verdict = doc
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| table.cell(row, "Row ID") == Some(id))
        .filter_map(|(table, row)| table.cell(row, "Verdict"))
        .next_back();
    assert_eq!(verdict, Some("pass"), "the last review of {id} passed it");
}

/// What a validation audit row on `USRIDIN` assigns: the status and the exact
/// message text.
struct Audited {
    status: StatusCode,
    message: String,
}

/// The CU00 requests of one test: the app, the administrator's token and the
/// two audits.
struct Cu00<'a> {
    app: &'a TestApp,
    token: String,
    validation: Document,
    parity: Document,
}

impl<'a> Cu00<'a> {
    /// Signs on as the seeded administrator and loads the audits.
    async fn new(app: &'a TestApp) -> Self {
        Self {
            app,
            token: app.admin_token().await,
            validation: common::validation_audit(),
            parity: common::parity_audit(),
        }
    }

    /// `GET /admin/users?<query>`, checked for password exposure.
    async fn get(&self, query: &str) -> Resp {
        let path = if query.is_empty() {
            ADMIN_USERS.to_owned()
        } else {
            format!("{ADMIN_USERS}?{query}")
        };
        let resp = self.app.get(&path, Some(&self.token)).await;
        assert_no_password(&resp, &format!("GET {path}"));
        resp
    }

    /// The 200 page of `GET /admin/users?<query>`.
    async fn page(&self, query: &str) -> Value {
        let (status, body, raw) = self.get(query).await;
        assert_eq!(status, StatusCode::OK, "status of the list ?{query}: {raw}");
        body
    }

    /// Asserts that `GET /admin/users?<query>` answers 200 with exactly the
    /// page `expected`, and returns it.
    async fn expect_page(&self, query: &str, expected: &Value) -> Value {
        let actual = self.page(query).await;
        assert_eq!(&actual, expected, "the list ?{query}");
        actual
    }

    /// `POST /admin/users` adding `user`, checked for password exposure: 201
    /// with the user as listed and `User <id> has been added ...`.
    async fn add(
        &self,
        (user_id, first_name, last_name, usr_type): (&str, &str, &str, &str),
        password: &str,
    ) -> Value {
        let body = json!({
            "first_name": first_name,
            "last_name": last_name,
            "user_id": user_id,
            "password": password,
            "usr_type": usr_type,
        });
        let resp = self.app.post(ADMIN_USERS, Some(&self.token), &body).await;
        assert_no_password(&resp, &format!("POST {ADMIN_USERS} {user_id}"));
        let (status, answer, raw) = resp;
        assert_eq!(status, StatusCode::CREATED, "adding {user_id}: {raw}");
        let listed = item(user_id, first_name, last_name, usr_type);
        let added = json!({ "user": listed, "message": messages::user_added_message(user_id) });
        assert_eq!(answer, added, "the answer to adding {user_id}");
        listed
    }

    /// The validation audit row `id`, once it is a reviewed `COUSR00C` row
    /// naming `USRIDIN`.
    fn audited(&self, id: &str) -> Audited {
        assert_reviewed(&self.validation, id);
        let (table, row) = self
            .validation
            .find_row(id)
            .unwrap_or_else(|| panic!("the validation audit defines no {id} row"));
        let text = cell(table, row, id, "Message text");
        let message = carddemo_audit::code_span(text)
            .and_then(|span| {
                let quoted = span.text.strip_prefix('"')?.strip_suffix('"')?;
                Some(quoted.to_owned())
            })
            .unwrap_or_else(|| panic!("{id} quotes no message text: {text}"));
        let status = cell(table, row, id, "Status");
        let status = status
            .parse()
            .ok()
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {status:?}"));
        assert_eq!(
            cell(table, row, id, "BMS field"),
            USRIDIN,
            "{id} names the search field"
        );
        Audited { status, message }
    }

    /// `text`, once the audit row `id` assigns it as a page-boundary text: a
    /// 200 page that carries it in `message`.
    fn boundary_text(&self, id: &str, text: &'static str) -> &'static str {
        let row = self.audited(id);
        assert_eq!(row.status, StatusCode::OK, "{id} answers with a page");
        assert_eq!(row.message, text, "the message text of {id}");
        text
    }

    /// Asserts that each parity row of `ids` is a reviewed `COUSR00C` row.
    fn proves(&self, ids: &[&str]) {
        for id in ids {
            assert_reviewed(&self.parity, id);
        }
    }

    /// The rows of one `COUSR0A` screen, the default `page_size`: the
    /// `page_size=` of [`SCREEN_ROW`], once it is a reviewed `COUSR00C` row.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, if it has not passed review or records no
    /// `page_size=` row count.
    fn screen_rows(&self) -> usize {
        assert_reviewed(&self.parity, SCREEN_ROW);
        let (table, row) = self
            .parity
            .find_row(SCREEN_ROW)
            .unwrap_or_else(|| panic!("the audit defines no {SCREEN_ROW} row"));
        let rows = key_values(cell(table, row, SCREEN_ROW, "Expected result"))
            .into_iter()
            .find(|pair| pair.key == "page_size")
            .unwrap_or_else(|| panic!("{SCREEN_ROW} gives no page_size= result"))
            .value;
        rows.parse()
            .unwrap_or_else(|_| panic!("{SCREEN_ROW} records no row count: {rows:?}"))
    }
}

/// The `message` of a rejection whose path sets no message (E-49).
const NO_MESSAGE: &str = "";

/// `items` sorted by `user_id`, the byte order of the 8-byte `SEC-USR-ID`
/// keys, all of which fill their 8 bytes here.
fn in_key_order(mut items: Vec<Value>) -> Vec<Value> {
    items.sort_by(|left, right| id_of(left).cmp(id_of(right)));
    items
}

/// The rows of `items`, in key order, whose user ID is at or after `key`:
/// where `STARTBR` positions `GTEQ`.
fn at_or_after<'v>(items: &'v [Value], key: &str) -> &'v [Value] {
    &items[items.partition_point(|item| id_of(item) < key)..]
}

/// The rows of `items`, in key order, whose user ID is before `key`.
fn before<'v>(items: &'v [Value], key: &str) -> &'v [Value] {
    &items[..items.partition_point(|item| id_of(item) < key)]
}

/// The first user ID of the page rows `rows`.
fn first_id(rows: &[Value]) -> &str {
    rows.first()
        .map_or_else(|| panic!("an expected page without rows"), id_of)
}

/// The seeded list before any write, then fifteen users, five of them added
/// through `POST /admin/users`, paged forward and backward at the default
/// screen size and at 5. One container serves the whole sequence, because
/// each step builds on the users the one before left.
#[test]
fn cu00_pages_the_user_file_in_key_order() {
    let app = TestApp::start();
    carddemo_testkit::block_on(async {
        let cu00 = Cu00::new(&app).await;
        let seeded = common::seeded_users(&cu00.parity, SEEDED_USERS_ROW);
        let seeded_items = in_key_order(seeded.iter().map(seeded_item).collect());
        let first_page = seeded_first_page(&cu00, &seeded_items).await;
        seeded_boundaries(&cu00, &seeded_items, &first_page).await;
        no_row_selector(&cu00, &first_page).await;
        aid_keys_map_to_requests(&cu00, &first_page).await;

        let users = add_users(&cu00, &seeded).await;
        for page_size in [None, Some(5)] {
            pages_both_ways(&cu00, &users, page_size).await;
        }
        start_key_positions(&cu00, &users).await;
        start_key_bounds(&cu00).await;
    });
}

/// The browse failures `COUSR00C` answers with `Unable to lookup User...` on
/// `USRIDIN`, injected through SQL on a container of their own, so no other
/// case runs against the altered user file.
#[test]
fn cu00_answers_a_failed_user_browse_with_the_lookup_text() {
    let app = TestApp::start();
    carddemo_testkit::block_on(async {
        let cu00 = Cu00::new(&app).await;
        browse_failures(&cu00).await;
        return_to_prev_screen(&cu00).await;
    });
}

/// The first page, ENTER with no search key — P-COUSR00C-003,
/// P-COUSR00C-007, P-COUSR00C-010, P-COUSR00C-013, P-COUSR00C-015;
/// V-COUSR00C-005.
///
/// `STARTBR` at `LOW-VALUES`, ten `READNEXT`s in `SEC-USR-ID` order filling
/// the ten rows, then the look-ahead `READNEXT` meets the end of the file, so
/// no next page follows and the page carries the bottom text. Each row is the
/// user's ID, names and type, right-trimmed; the body holds nothing but the
/// rows, the two keys and the message, no 3270 header field. The page is also
/// the `first_page` snapshot.
async fn seeded_first_page(cu00: &Cu00<'_>, seeded: &[Value]) -> Value {
    cu00.proves(&[
        "P-COUSR00C-003",
        "P-COUSR00C-007",
        "P-COUSR00C-010",
        "P-COUSR00C-013",
        "P-COUSR00C-015",
    ]);
    let bottom = cu00.boundary_text(
        "V-COUSR00C-005",
        messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE,
    );
    assert_eq!(
        seeded.len(),
        cu00.screen_rows(),
        "{SEEDED_USERS_ROW}: the seed fills one {SCREEN_ROW} screen"
    );
    let expected = expected_page(seeded, None, Some(first_id(seeded)), Some(bottom));
    let page = cu00.expect_page("", &expected).await;
    insta::assert_yaml_snapshot!("first_page", page);
    page
}

/// The page-boundary texts on the ten seeded users, each the example input
/// of its audit row.
///
/// - PF8 without a next page, the forward request from the last user ID —
///   P-COUSR00C-005; V-COUSR00C-002: nothing more is browsed and the last
///   user stays listed with the already-at-the-bottom text.
/// - PF7 on the first page, the backward request from its `prev_key` or
///   from the top of the file — P-COUSR00C-004; V-COUSR00C-007: an empty
///   page with the already-at-the-top text.
/// - PF7 from `ADMIN003` — P-COUSR00C-008, P-COUSR00C-009, P-COUSR00C-011;
///   V-COUSR00C-008: the record at the browse position is discarded,
///   `READPREV` finds the two users before it and meets the start of the
///   file, so the page holds those two rows, ascending, and no blank row.
/// - A start key with no user at or after it, in either direction —
///   P-COUSR00C-006, P-COUSR00C-016, P-COUSR00C-017; V-COUSR00C-003: an empty
///   page with the top-of-the-page text, and no abend (E-71).
/// - ENTER on `USER0001` — V-COUSR00C-005: the five users from it, then the
///   end of the file.
async fn seeded_boundaries(cu00: &Cu00<'_>, seeded: &[Value], first_page: &Value) {
    cu00.proves(&[
        "P-COUSR00C-004",
        "P-COUSR00C-005",
        "P-COUSR00C-006",
        "P-COUSR00C-008",
        "P-COUSR00C-009",
        "P-COUSR00C-011",
        "P-COUSR00C-016",
        "P-COUSR00C-017",
    ]);
    let already_bottom = cu00.boundary_text(
        "V-COUSR00C-002",
        messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE,
    );
    let last = at_or_after(seeded, "USER0005");
    let expected = expected_page(last, None, Some(first_id(last)), Some(already_bottom));
    cu00.expect_page("start_key=USER0005&direction=forward", &expected)
        .await;

    let already_top = cu00.boundary_text(
        "V-COUSR00C-007",
        messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE,
    );
    let prev_key = first_page["prev_key"]
        .as_str()
        .unwrap_or_else(|| panic!("the first page offers PF7: {first_page}"));
    let empty = expected_page(&[], None, None, Some(already_top));
    for query in [
        format!("start_key={prev_key}&direction=backward"),
        "direction=backward".to_owned(),
    ] {
        cu00.expect_page(&query, &empty).await;
    }

    let reached_top = cu00.boundary_text(
        "V-COUSR00C-008",
        messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE,
    );
    let earlier = before(seeded, "ADMIN003");
    let expected = expected_page(
        earlier,
        Some("ADMIN003"),
        Some(first_id(earlier)),
        Some(reached_top),
    );
    cu00.expect_page("start_key=ADMIN003&direction=backward", &expected)
        .await;

    let at_top = cu00.boundary_text("V-COUSR00C-003", messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE);
    let empty = expected_page(&[], None, None, Some(at_top));
    for direction in ["forward", "backward"] {
        cu00.expect_page(&format!("start_key=ZZZZZZZZ&direction={direction}"), &empty)
            .await;
    }

    let bottom = cu00.boundary_text(
        "V-COUSR00C-005",
        messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE,
    );
    let following = at_or_after(seeded, "USER0001");
    let expected = expected_page(following, None, Some(first_id(following)), Some(bottom));
    cu00.expect_page("start_key=USER0001&page_size=10", &expected)
        .await;
}

/// The row selectors `SEL0001`..`SEL0010` have no REST parameter — the
/// selection text of V-COUSR00C-001 is unreachable (E-54) — P-COUSR00C-002,
/// P-COUSR00C-014.
///
/// The published operation takes exactly the query parameters that replace
/// the received `COUSR0A` map: `start_key` (`USRIDINI`), `direction` (ENTER,
/// PF7, PF8) and `page_size`. A selector a client sends anyway is no
/// parameter: the page is browsed as without it and carries no selection
/// text, the 200 and missing error flag the row records.
async fn no_row_selector(cu00: &Cu00<'_>, first_page: &Value) {
    cu00.proves(&["P-COUSR00C-002", "P-COUSR00C-014"]);
    let selection = cu00.audited("V-COUSR00C-001");
    assert_eq!(
        selection.message,
        messages::INVALID_SELECTION_VALID_VALUES_ARE_U_AND_D,
        "the message text of the selection row"
    );
    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let operation = &spec["paths"][ADMIN_USERS]["get"];
    let parameters: BTreeSet<(&str, &str)> = operation["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("GET {ADMIN_USERS} publishes no parameters: {operation}"))
        .iter()
        .map(|parameter| {
            let text = |name: &str| parameter[name].as_str().unwrap_or_default();
            (text("name"), text("in"))
        })
        .collect();
    let published = BTreeSet::from([
        ("direction", "query"),
        ("page_size", "query"),
        ("start_key", "query"),
    ]);
    assert_eq!(parameters, published, "the parameters of GET {ADMIN_USERS}");

    let page = cu00.page("SEL0001=X").await;
    assert_eq!(&page, first_page, "a selector leaves the page unchanged");
    assert_eq!(
        selection.status,
        StatusCode::OK,
        "the selection row answers 200"
    );
}

/// `MAIN-PARA`'s `EVALUATE EIBAID` — P-COUSR00C-001.
///
/// Each key the row lists has the action the row gives it: ENTER (PF8 from
/// the top) is the first page and PF7 from the top the empty page with the
/// already-at-the-top text, each through the published `GET` the function
/// map names for the key; PF3's `XCTL COADM01C` and the invalid-key error
/// send no request. The list takes no key input, so a request naming any
/// other AID key is answered as ENTER, with the first page, and no answer
/// carries the invalid-key text, whose audited 49 bytes fit its `X(50)`.
async fn aid_keys_map_to_requests(cu00: &Cu00<'_>, first_page: &Value) {
    cu00.proves(&[AID_ROW]);
    let keys = aid_keys(&cu00.parity, &function_map(), AID_ROW, MAP);
    let spec = spec();
    assert_eq!(
        operations(&spec, ADMIN_USERS),
        BTreeSet::from(["get", "post"]),
        "{AID_ROW}: the CU00 list and the CU01 add are the operations on {ADMIN_USERS}"
    );
    let already_top = cu00.boundary_text(
        "V-COUSR00C-007",
        messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE,
    );
    let empty = expected_page(&[], None, None, Some(already_top));
    for key in &keys {
        match (&key.endpoint, key.sends_no_request()) {
            (None, true) => {}
            (Some(endpoint), false) => {
                assert_published(&spec, key, endpoint);
                assert_eq!(
                    (endpoint.method.as_str(), endpoint.path.as_str()),
                    ("get", ADMIN_USERS),
                    "{AID_ROW} {}: the list request",
                    key.key
                );
                let backward = endpoint
                    .query
                    .split('&')
                    .any(|pair| pair == "direction=backward");
                let expected = if backward { &empty } else { first_page };
                cu00.expect_page(&endpoint.query, expected).await;
            }
            (endpoint, _) => panic!(
                "{AID_ROW} {}: the action {:?} and the function map request {endpoint:?} disagree",
                key.key, key.action
            ),
        }
    }
    let other = keys
        .iter()
        .find(|key| key.key == OTHER_KEY)
        .unwrap_or_else(|| panic!("{AID_ROW} lists no {OTHER_KEY} key: {keys:?}"));
    let invalid = code_span(&other.action)
        .unwrap_or_else(|| panic!("{AID_ROW} quotes no invalid-key text: {}", other.action));
    assert_eq!(
        invalid.text,
        format!("\"{}\"", messages::CCDA_MSG_INVALID_KEY),
        "{AID_ROW}: the invalid-key text"
    );
    let width = invalid
        .width
        .unwrap_or_else(|| panic!("{AID_ROW} gives the invalid-key text no width"));
    assert!(
        messages::CCDA_MSG_INVALID_KEY.len() <= width,
        "{AID_ROW}: the invalid-key text fits its X({width})"
    );
    for aid in OTHER_KEYS {
        let (status, body, raw) = cu00.get(aid).await;
        assert_eq!(status, StatusCode::OK, "{AID_ROW} ?{aid}: {raw}");
        assert_eq!(&body, first_page, "{AID_ROW}: ?{aid} is answered as ENTER");
        assert!(
            !raw.contains(messages::CCDA_MSG_INVALID_KEY.trim_end()),
            "{AID_ROW}: ?{aid} answers the invalid-key text: {raw}"
        );
    }
}

/// `RETURN-TO-PREV-SCREEN` — P-COUSR00C-012.
///
/// PF3's `XCTL COADM01C` is navigation: each function map row the audit row
/// names sends no request. The entry without a COMMAREA (`EIBCALEN = 0`,
/// `XCTL COSGN00C`) is a request without a session: 401 `Authentication
/// required` with no field, before any user is read, so it answers so while
/// the user browse of [`browse_failures`] still fails for the administrator.
async fn return_to_prev_screen(cu00: &Cu00<'_>) {
    cu00.proves(&[RETURN_ROW]);
    let (table, row) = cu00
        .parity
        .find_row(RETURN_ROW)
        .unwrap_or_else(|| panic!("the audit defines no {RETURN_ROW} row"));
    let map_doc = function_map();
    let named: Vec<String> = id_tokens(cell(table, row, RETURN_ROW, "Expected result"))
        .into_iter()
        .filter(|(kind, _)| *kind == IdKind::Function)
        .map(|(_, id)| id)
        .collect();
    assert!(!named.is_empty(), "{RETURN_ROW} names no function map row");
    for id in &named {
        let (table, row) = map_doc
            .find_row(id)
            .unwrap_or_else(|| panic!("{FUNCTION_MAP_PATH} defines no {id} row"));
        let request = endpoint(cell(table, row, id, "Endpoint"));
        assert_eq!(request, None, "{RETURN_ROW}: {id} sends no request");
    }
    let readnext = lookup_failure(cu00, "V-COUSR00C-006");
    assert_lookup_failed(&cu00.get("").await, readnext);
    for start in ["", UNREADABLE_START] {
        let path = format!("{ADMIN_USERS}?start_key={start}");
        for token in NO_SESSION {
            let resp = common::send(cu00.app.client().get(cu00.app.url(&path)), token).await;
            assert_no_password(&resp, &format!("GET {path} without a session"));
            common::assert_error(
                &resp,
                StatusCode::UNAUTHORIZED,
                UNAUTHENTICATED,
                messages::AUTHENTICATION_REQUIRED,
                &[],
            );
        }
    }
}

/// Adds [`ADDED_USERS`] through `POST /admin/users`, with the password the
/// seed stores, and returns every user's list row in key order.
async fn add_users(cu00: &Cu00<'_>, seeded: &[SeededUser]) -> Vec<Value> {
    let password = seeded.first().map_or_else(
        || panic!("the seed holds no user"),
        |user| user.password.as_str(),
    );
    let mut users: Vec<Value> = seeded.iter().map(seeded_item).collect();
    for user in ADDED_USERS {
        users.push(cu00.add(user, password).await);
    }
    in_key_order(users)
}

/// Pages of `page_size` rows (the default screen size when `None`) over
/// every user `users` holds, in key order — P-COUSR00C-003, P-COUSR00C-007,
/// P-COUSR00C-008, P-COUSR00C-009, P-COUSR00C-011; V-COUSR00C-005,
/// V-COUSR00C-007, V-COUSR00C-008.
///
/// Forward from the top, each page from the `next_key` of the one before:
/// full pages carry no text, the last carries the bottom text and no
/// `next_key`, and together they list every user once, in order. Then
/// backward, each page from the `prev_key` of the page after it: the page
/// before comes back unchanged, the first one with the reached-the-top text,
/// and PF7 from the first page is the empty page with the already-at-the-top
/// text.
async fn pages_both_ways(cu00: &Cu00<'_>, users: &[Value], page_size: Option<usize>) {
    cu00.proves(&[
        "P-COUSR00C-003",
        "P-COUSR00C-007",
        "P-COUSR00C-008",
        "P-COUSR00C-009",
        "P-COUSR00C-011",
    ]);
    let bottom = cu00.boundary_text(
        "V-COUSR00C-005",
        messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE,
    );
    let reached_top = cu00.boundary_text(
        "V-COUSR00C-008",
        messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE,
    );
    let already_top = cu00.boundary_text(
        "V-COUSR00C-007",
        messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE,
    );
    let size = page_size
        .map(|rows| format!("page_size={rows}"))
        .unwrap_or_default();
    let pages: Vec<&[Value]> = users
        .chunks(page_size.unwrap_or_else(|| cu00.screen_rows()))
        .collect();
    assert!(
        pages.len() > 1,
        "{} users fill more than one page",
        users.len()
    );

    let mut start = String::new();
    let mut listed: Vec<Value> = Vec::new();
    let mut prev_keys: Vec<String> = Vec::new();
    for (index, rows) in pages.iter().enumerate() {
        let next_key = pages.get(index + 1).map(|next| first_id(next));
        let message = next_key.is_none().then_some(bottom);
        let expected = expected_page(rows, next_key, Some(first_id(rows)), message);
        let body = cu00.expect_page(&query(&[&size, &start]), &expected).await;
        listed.extend(body["items"].as_array().cloned().unwrap_or_default());
        prev_keys.push(body["prev_key"].as_str().unwrap_or_default().to_owned());
        start = body["next_key"]
            .as_str()
            .map(|key| format!("start_key={key}&direction=forward"))
            .unwrap_or_default();
    }
    assert_eq!(
        listed, users,
        "the forward pages list every user once, in order"
    );

    for index in (0..pages.len() - 1).rev() {
        let from = &prev_keys[index + 1];
        let rows = pages[index];
        let message = (index == 0).then_some(reached_top);
        let expected = expected_page(rows, Some(from), Some(first_id(rows)), message);
        let backward = format!("start_key={from}&direction=backward");
        cu00.expect_page(&query(&[&size, &backward]), &expected)
            .await;
    }
    let backward = format!("start_key={}&direction=backward", prev_keys[0]);
    let empty = expected_page(&[], None, None, Some(already_top));
    cu00.expect_page(&query(&[&size, &backward]), &empty).await;
}

/// `STARTBR` positions at the first user ID at or after the search key, the
/// `GTEQ` default — P-COUSR00C-003; V-COUSR00C-005.
///
/// `start_key=USER0001` starts the page at `USER0001`, and a key no user
/// holds, `USER`, at the first user after it, the same page: the users from
/// `USER0001` on, then the end of the file.
async fn start_key_positions(cu00: &Cu00<'_>, users: &[Value]) {
    cu00.proves(&["P-COUSR00C-003"]);
    let bottom = cu00.boundary_text(
        "V-COUSR00C-005",
        messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE,
    );
    let screen_rows = cu00.screen_rows();
    let following = at_or_after(users, "USER0001");
    let shown = &following[..following.len().min(screen_rows)];
    let next_key = following.get(screen_rows).map(id_of);
    let message = next_key.is_none().then_some(bottom);
    let expected = expected_page(shown, next_key, Some(first_id(shown)), message);
    assert_eq!(first_id(shown), "USER0001", "the page starts at USER0001");
    for key in ["USER0001", "USER"] {
        cu00.expect_page(&format!("start_key={key}"), &expected)
            .await;
    }
}

/// A start key the REST query can carry but `USRIDINI PIC X(8)` cannot:
/// more than 8 characters, or a character outside ASCII. No `COUSR00C` path
/// receives one, so it is refused before any browse, in either direction,
/// with the no-message answer on `USRIDIN` (E-49).
async fn start_key_bounds(cu00: &Cu00<'_>) {
    for key in ["USER00010", "%C3%89"] {
        for direction in ["forward", "backward"] {
            let resp = cu00
                .get(&format!("start_key={key}&direction={direction}"))
                .await;
            common::assert_error(
                &resp,
                StatusCode::BAD_REQUEST,
                ErrorKind::Invalid.code(),
                NO_MESSAGE,
                &[(USRIDIN, NO_MESSAGE)],
            );
        }
    }
}

/// The status the validation audit row `id` assigns to a failed browse, once
/// the row is the `Unable to lookup User...` text on `USRIDIN`.
fn lookup_failure(cu00: &Cu00<'_>, id: &str) -> StatusCode {
    let row = cu00.audited(id);
    assert_eq!(
        row.message,
        messages::UNABLE_TO_LOOKUP_USER,
        "the message text of {id}"
    );
    row.status
}

/// Asserts that `resp` is the failed-browse answer `status`: code
/// `UNEXPECTED`, `Unable to lookup User...`, on `USRIDIN`.
fn assert_lookup_failed(resp: &Resp, status: StatusCode) {
    common::assert_error(
        resp,
        status,
        ErrorKind::Unexpected.code(),
        messages::UNABLE_TO_LOOKUP_USER,
        &[(USRIDIN, messages::UNABLE_TO_LOOKUP_USER)],
    );
}

/// Each browse step meets the record [`STORE_UNREADABLE_USER`] stores. A
/// missing user file would instead fail the bearer token's subject check,
/// which reads the acting administrator by key, before the list runs; one
/// unreadable record leaves that read working, so only the browse fails.
///
/// - `STARTBR` — P-COUSR00C-006; V-COUSR00C-004: a backward page first
///   positions by reading the user at or after its key, which from
///   [`UNREADABLE_START`] is the record, so the page fails before any
///   `READPREV`.
/// - `READNEXT` — P-COUSR00C-007; V-COUSR00C-006: forward from the top, from
///   `ADMIN005` or from [`UNREADABLE_START`], a read of the page meets the
///   record.
/// - `READPREV` — P-COUSR00C-008; V-COUSR00C-009: backward from `USER0001`,
///   the position is `USER0001`, the first `READPREV` returns it and is
///   discarded, and the next one meets the record.
///
/// A browse that never reaches the record, forward from `USER0001`, still
/// answers its page.
async fn browse_failures(cu00: &Cu00<'_>) {
    cu00.proves(&["P-COUSR00C-006", "P-COUSR00C-007", "P-COUSR00C-008"]);
    cu00.app.stack().exec_sql(STORE_UNREADABLE_USER).await;

    let startbr = lookup_failure(cu00, "V-COUSR00C-004");
    let resp = cu00
        .get(&format!("start_key={UNREADABLE_START}&direction=backward"))
        .await;
    assert_lookup_failed(&resp, startbr);

    let readnext = lookup_failure(cu00, "V-COUSR00C-006");
    for start in ["", "ADMIN005", UNREADABLE_START] {
        let resp = cu00.get(&format!("start_key={start}")).await;
        assert_lookup_failed(&resp, readnext);
    }

    let readprev = lookup_failure(cu00, "V-COUSR00C-009");
    let resp = cu00.get("start_key=USER0001&direction=backward").await;
    assert_lookup_failed(&resp, readprev);

    let page = cu00.page("start_key=USER0001").await;
    assert_eq!(
        page["prev_key"], "USER0001",
        "a browse past the record lists: {page}"
    );
}
