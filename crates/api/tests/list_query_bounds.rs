//! Contract bounds of the list query parameters, proved through the HTTP
//! layer: the CT00 account filter of `GET /transactions`, the CCLI start
//! key of `GET /cards` and the CU00 start key of `GET /admin/users`.
//!
//! Every case is a real request through the router, so it crosses the
//! extractors, the bearer token, the status mapping and the
//! `{code, message, fields}` body the SPA receives:
//!
//! - `account_id` is published with `maxLength` 11, the 11 digits of
//!   `ACCT-ID 9(11)`. Within that width a blank filter (empty, spaces, or NULs
//!   as `LOW-VALUES`) lists every transaction, as the unfiltered browse does
//!   (E-17). A longer filter is refused whatever it holds, blank ones
//!   included: V-COTRN00C-001 sets no message (E-49), so the answer is 400
//!   `INVALID` with an empty message and no field.
//! - A card `start_key` is published with `maxLength` 16, the `X(16)` of
//!   `CARD-NUM`, and a NUL-free ASCII pattern. A key holding a NUL names no
//!   card and cannot be bound as the text of the `CARDDAT` key (E-07); a
//!   longer key would be cut to another card and a non-ASCII one moved in as
//!   its UTF-8 bytes, echoed back as other characters in `prev_key`. Each is
//!   refused with the same no-message answer before any browse, rather than
//!   failing the browse as a `CARDDAT` file error (500) or paging from a key
//!   the caller did not send.
//! - A user `start_key` holding a NUL is browsed as entered unless it is the 8
//!   NULs `COUSR00C` reads as `LOW-VALUES` once the key fills its `X(8)`
//!   field, and the `USRSEC` key cannot hold NUL as text (E-07). It is refused
//!   before any browse as an over-length or non-ASCII key is, 400 `INVALID`
//!   with an empty message on `USRIDIN` (E-49), rather than failing the browse
//!   with the `USRSEC` browse-failure text (500). The 8 NULs list the first
//!   page.
//!
//! The file is kept apart from the per-transaction suites, which follow the
//! audited COBOL paths of one program each. These bounds belong to the REST
//! query parameters themselves: a 3270 field holds no more characters than
//! its width, so no COBOL screen can send an over-width filter or a NUL in a
//! start key.
//!
//! The seeded transaction master is empty (E-07), so the in-width blank
//! filters are compared with the whole unfiltered body, not by item count.
//! Docker must be running: the test starts one `postgres:16.15` container.

pub mod common;

use carddemo_api::dto::admin_users::UserListField;
use carddemo_domain::error::ErrorKind;
use reqwest::StatusCode;
use serde_json::json;

use common::{Resp, TestApp};

/// The validation audit row of a CT00 account filter that is not 11 digits.
const FILTER_ROW: &str = "V-COTRN00C-001";

/// The published `maxLength` of the CT00 `account_id` filter: the 11 digits
/// of `ACCT-ID 9(11)`.
const FILTER_WIDTH: usize = 11;

/// The `message` of a rejection whose path sets no message (E-49).
const NO_MESSAGE: &str = "";

/// The `BMS field` cell of an audit row that names no field.
const NO_BMS_FIELD: &str = "—";

/// The CT00 list route.
const TRANSACTIONS: &str = "/transactions";

/// The CCLI list route.
const CARDS: &str = "/cards";

/// `CARD-NUM` of the first seeded card [app/data/ASCII/carddata.txt:L1], the
/// lowest key of the card browse.
const FIRST_CARD: &str = "0500024453765740";

/// The list resources hold their query bounds on one seeded database, so the
/// file starts a single container.
#[test]
fn list_query_parameters_hold_their_contract_bounds() {
    let app = TestApp::start();
    carddemo_testkit::block_on(async {
        let token = app.user_token().await;
        ct00_account_filter_bounds(&app, &token).await;
        ccli_start_key_bounds(&app, &token).await;
        cu00_start_key_bounds(&app).await;
    });
}

/// CT00: an `account_id` over its published width is refused, blank or not;
/// a blank one within the width lists as the unfiltered browse.
async fn ct00_account_filter_bounds(app: &TestApp, token: &str) {
    let refused = audited_no_message_status(FILTER_ROW);
    let over_width = [
        "%20".repeat(FILTER_WIDTH + 1),
        "%00".repeat(FILTER_WIDTH + 1),
        "9".repeat(FILTER_WIDTH + 1),
    ];
    for filter in over_width {
        let path = format!("{TRANSACTIONS}?account_id={filter}");
        let resp = app.get(&path, Some(token)).await;
        assert_rejected_without_message(&resp, refused, &path);
    }

    let (status, unfiltered, raw) = app.get(TRANSACTIONS, Some(token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "status of GET {TRANSACTIONS}: {raw}"
    );
    assert!(
        unfiltered["items"].is_array(),
        "GET {TRANSACTIONS} answers a page: {raw}"
    );
    let in_width_blank = [
        String::new(),
        "%20".repeat(FILTER_WIDTH),
        "%00".repeat(FILTER_WIDTH),
    ];
    for filter in in_width_blank {
        let path = format!("{TRANSACTIONS}?account_id={filter}");
        let (status, body, raw) = app.get(&path, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "status of GET {path}: {raw}");
        assert_eq!(
            body, unfiltered,
            "GET {path} lists as GET {TRANSACTIONS} does: {raw}"
        );
    }
}

/// CCLI: a `start_key` holding a NUL, a character outside ASCII or more than
/// the 16 characters of a card number is refused before the browse; a stored
/// card number pages from that card and comes back unchanged as `prev_key`.
async fn ccli_start_key_bounds(app: &TestApp, token: &str) {
    let refused = [
        "%00".to_owned(),
        format!("{FIRST_CARD}%00"),
        "%C3%A9%C3%A9%C3%A9".to_owned(),
        "%E2%80%99%20OR".to_owned(),
        format!("{FIRST_CARD}0"),
        "A".repeat(20_000),
    ];
    for start_key in refused {
        let path = format!("{CARDS}?start_key={start_key}");
        let resp = app.get(&path, Some(token)).await;
        assert_rejected_without_message(&resp, StatusCode::BAD_REQUEST, &path);
    }

    let path = format!("{CARDS}?start_key={FIRST_CARD}");
    let (status, body, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "status of GET {path}: {raw}");
    assert!(
        body["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "GET {path} pages from a stored card: {raw}"
    );
    assert_eq!(
        body["prev_key"], FIRST_CARD,
        "GET {path} echoes its key: {raw}"
    );
}

/// The CU00 list route.
const ADMIN_USERS: &str = "/admin/users";

/// The width of the CU00 start key, `USRIDINI PIC X(8)`
/// [app/cpy-bms/COUSR00.CPY:L66]: the number of NULs `COUSR00C` reads as
/// `LOW-VALUES`.
const USER_ID_WIDTH: usize = 8;

/// `SEC-USR-ID` of the first seeded user [app/jcl/DUSRSECJ.jcl:L35], the
/// lowest key of the user browse.
const FIRST_USER: &str = "ADMIN001";

/// CU00: a `start_key` holding a NUL, in either direction, is refused on
/// `USRIDIN` before the browse unless it is the 8 NULs of `LOW-VALUES`, which
/// list the first page as no start key does.
async fn cu00_start_key_bounds(app: &TestApp) {
    let token = app.admin_token().await;
    let refused = json!({
        "code": ErrorKind::Invalid.code(),
        "message": NO_MESSAGE,
        "fields": [{ "field": UserListField::UsrIdIn.as_str(), "message": NO_MESSAGE }],
    });
    for query in [
        "start_key=%00",
        "start_key=A%00",
        "start_key=%00&direction=backward",
    ] {
        let path = format!("{ADMIN_USERS}?{query}");
        let (status, body, raw) = app.get(&path, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "status of GET {path}: {raw}"
        );
        assert_eq!(body, refused, "body of GET {path}: {raw}");
    }

    let (status, first_page, raw) = app.get(ADMIN_USERS, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "status of GET {ADMIN_USERS}: {raw}");
    let path = format!("{ADMIN_USERS}?start_key={}", "%00".repeat(USER_ID_WIDTH));
    let (status, body, raw) = app.get(&path, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "status of GET {path}: {raw}");
    assert_eq!(
        body["items"][0]["user_id"],
        json!(FIRST_USER),
        "GET {path} starts at the first user: {raw}"
    );
    assert_eq!(
        body, first_page,
        "GET {path} lists as GET {ADMIN_USERS} does: {raw}"
    );
}

/// The status the validation audit assigns to the row `id`, once the row is
/// confirmed to set no message and to name no BMS field: the two facts an
/// expected body of only `code` and an empty `message` rests on.
fn audited_no_message_status(id: &str) -> StatusCode {
    let audit = common::validation_audit();
    let (table, row) = audit
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit defines no {id} row"));
    let message = table.cell(row, "Message text").unwrap_or_default();
    assert!(
        carddemo_audit::is_no_message(message),
        "{id} sets a message: {message:?}"
    );
    let field = table.cell(row, "BMS field").unwrap_or_default();
    assert_eq!(field, NO_BMS_FIELD, "{id} names a BMS field");
    let status = table.cell(row, "Status").unwrap_or_default();
    status
        .parse()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {status:?}"))
}

/// Asserts that `resp`, the answer to `GET path`, is `status` with exactly the
/// body `{"code":"INVALID","message":""}`: the no-message rejection, without
/// a `fields` entry.
fn assert_rejected_without_message(resp: &Resp, status: StatusCode, path: &str) {
    let (actual, body, raw) = resp;
    assert_eq!(*actual, status, "status of GET {path}: {raw}");
    let expected = json!({ "code": ErrorKind::Invalid.code(), "message": NO_MESSAGE });
    assert_eq!(*body, expected, "body of GET {path}: {raw}");
}
