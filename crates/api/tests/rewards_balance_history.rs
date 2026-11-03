//! The reward read API through the HTTP layer:
//! `GET /rewards/balance/{account_id}` and `GET /rewards/history/{account_id}`
//! (request Rewards system; AAP 0.3.5, o8).
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a
//! roadmap item [README.md:L357-369], so no audit row is cited here.
//!
//! - An existing account without accruals has the zero balance and an empty
//!   history, both 200; an account key that names no account, or is no
//!   account key at all, is 404 `Account not found`; a request without a
//!   bearer token is 401. A `U` token suffices: these are user functions.
//! - Three CT02 adds under the seeded retail rule (`01`/`0001`, rate 1.0000)
//!   accrue `ceil(amount × 1.0000)` points each, and the history pages them
//!   newest first by `start_key` and `page_size`, every accrual stamped with
//!   the pinned processing clock.
//! - Points and row identities are decimal digit strings, never JSON numbers,
//!   so a stored balance of `9007199254740993`, above the 2^53 − 1 a browser
//!   holds exactly, arrives as those exact digits (AAP 0.5.3.1).
//!
//! Points are compared as the strings they are sent as; no points value is
//! read as a number. Docker must be running; the harness conventions are
//! those of `common`.

pub mod common;

use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::block_on;
use chrono::{DateTime, Utc};
use common::{Resp, TestApp, assert_error};
use reqwest::StatusCode;
use serde_json::{Map, Value, json};

/// The processing clock, inside the active period of the seeded retail rule
/// (from 2022-01-01T00:00:00Z, open-ended). Every accrual is stamped with it.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The `code` of a 401 for a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// Account 1 of the fixture [app/data/ASCII/acctdata.txt:L1], with card
/// `9680294154603697` [app/data/ASCII/cardxref.txt:L49]: the account the CT02
/// adds post to.
const ACCRUING_ACCOUNT: &str = "00000000001";

/// Account 2 of the fixture [app/data/ASCII/acctdata.txt:L2], which never
/// accrues here.
const QUIET_ACCOUNT: &str = "00000000002";

/// Account 3 of the fixture [app/data/ASCII/acctdata.txt:L3], given the large
/// stored balance.
const LARGE_ACCOUNT: &str = "00000000003";

/// An 11-digit account key no fixture account holds: `acctdata.txt` holds
/// accounts 1 to 50.
const UNKNOWN_ACCOUNT: &str = "99999999999";

/// Path text that is no account key at all.
const MALFORMED_ACCOUNT: &str = "abc";

/// The seeded retail rule: type `01` Purchase [app/data/ASCII/trantype.txt:L1],
/// category `0001` Regular Sales Draft [app/data/ASCII/trancatg.txt:L1], rate
/// 1.0000, the first rule `batch seed` inserts (AAP 0.3.6).
const RETAIL_RULE: &str = "SELECT rule_id::text, reward_rate::text, active_to::text \
     FROM reward_rules WHERE transaction_type_cd = '01' AND transaction_category_cd = 1";

/// The `rule_id` of [`RETAIL_RULE`], as the history sends it.
const RETAIL_RULE_ID: &str = "1";

/// The one live row the CT02 TRAN-ID allocation counts from. The seeded live
/// master is empty (E-07), and an add against an empty master allocates
/// nothing (E-27). Written directly, it earns no accrual.
const LIVE_ROW: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES ('0000000000000300', '01', 1, 'POS TERM', 'Rewards base row', 10.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '9680294154603697', '2022-06-01 00:00:00.000000', '2022-06-01 00:00:00.000000')";

/// The three CT02 purchases on [`ACCRUING_ACCOUNT`], in posting order: the
/// `TRNAMT` text in the `COTRN02C` amount form `-99999999.99` (sign, eight
/// digits, point, two digits) and the points `ceil(amount × 1.0000)` it earns
/// under the retail rule. The cent rounds up to a whole point, and 99.50 to
/// 100.
// No audit ID is cited here: these purchases pass every edit, so citing one
// would count as coverage of an error path this file never sends.
const PURCHASES: [(&str, &str); 3] = [
    ("+00000010.00", "10"),
    ("+00000000.01", "1"),
    ("+00000099.50", "100"),
];

/// The sum of the [`PURCHASES`] points.
const PURCHASED_POINTS: &str = "111";

/// The accruals of account 1 newest first, with the TRAN-ID of the
/// transaction each one credits: the rows the history must show.
const ACCOUNT_1_ACCRUALS: &str = "SELECT a.accrual_id::text, a.transaction_id::text, \
            a.points_earned::text, m.tran_id \
     FROM reward_accruals a JOIN transaction_master m USING (transaction_id) \
     WHERE a.account_id = 1 \
     ORDER BY a.created_at DESC, a.accrual_id DESC";

/// 2^53 + 1: the smallest points value a JSON number turns into another
/// (2^53) in a browser.
const ABOVE_JS_SAFE: &str = "9007199254740993";

/// The reward balance of [`LARGE_ACCOUNT`]: [`ABOVE_JS_SAFE`] earned and
/// available, nothing redeemed.
const LARGE_BALANCE: &str = "INSERT INTO reward_balances \
     (account_id, total_points_earned, total_points_redeemed, available_points) \
     VALUES (3, 9007199254740993, 0, 9007199254740993)";

/// The keys of one history item.
const ACCRUAL_KEYS: [&str; 6] = [
    "accrual_id",
    "account_id",
    "transaction_id",
    "rule_id",
    "points_earned",
    "created_at",
];

/// One accrual as the database holds it, in the text the history sends.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredAccrual {
    accrual_id: String,
    transaction_id: String,
    points_earned: String,
    tran_id: String,
}

/// The balance route of `account`.
fn balance_path(account: &str) -> String {
    format!("/rewards/balance/{account}")
}

/// The history route of `account`, with the query string `query` when it is
/// not empty.
fn history_path(account: &str, query: &str) -> String {
    if query.is_empty() {
        format!("/rewards/history/{account}")
    } else {
        format!("/rewards/history/{account}?{query}")
    }
}

/// Asserts that `resp` is 404 `NOT_FOUND` `Account not found` without fields.
fn assert_account_not_found(resp: &Resp) {
    assert_error(
        resp,
        StatusCode::NOT_FOUND,
        ErrorKind::NotFound.code(),
        messages::ACCOUNT_NOT_FOUND,
        &[],
    );
}

/// Asserts that `resp` is 401 `UNAUTHENTICATED` `Authentication required`.
fn assert_unauthenticated(resp: &Resp) {
    assert_error(
        resp,
        StatusCode::UNAUTHORIZED,
        UNAUTHENTICATED,
        messages::AUTHENTICATION_REQUIRED,
        &[],
    );
}

/// Asserts that `resp` is the 200 balance of `account` with exactly the three
/// points totals given, each a JSON string, and no other member.
fn assert_balance(resp: &Resp, account: &str, earned: &str, redeemed: &str, available: &str) {
    let (status, body, raw) = resp;
    assert_eq!(*status, StatusCode::OK, "balance of {account}: {raw}");
    assert_eq!(
        *body,
        json!({
            "account_id": account,
            "total_points_earned": earned,
            "total_points_redeemed": redeemed,
            "available_points": available,
        }),
        "balance of {account}, every points total a string: {raw}"
    );
}

/// The string member `key` of `value`.
fn text<'a>(value: &'a Value, key: &str, raw: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("no string `{key}` in {value}: {raw}"))
}

/// Asserts that `resp` is a 200 history page holding exactly `expected`, in
/// that order, with `next_key` as given and `prev_key` `null`, as the history
/// pages forward only, and no `message`.
fn assert_history_page(resp: &Resp, expected: &[StoredAccrual], next_key: Option<&str>) {
    let (status, body, raw) = resp;
    assert_eq!(*status, StatusCode::OK, "history page: {raw}");
    let Value::Object(page) = body else {
        panic!("the history page is not an object: {raw}");
    };
    let mut keys: Vec<&str> = page.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["items", "next_key", "prev_key"],
        "the members of the page: {raw}"
    );
    assert_eq!(
        page["next_key"],
        next_key.map_or(Value::Null, Value::from),
        "`next_key` of the page: {raw}"
    );
    assert_eq!(
        page["prev_key"],
        Value::Null,
        "`prev_key` of the page: {raw}"
    );
    let items = page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("`items` of the page is not an array: {raw}"));
    assert_eq!(items.len(), expected.len(), "accruals on the page: {raw}");
    for (item, stored) in items.iter().zip(expected) {
        assert_accrual(item, stored, raw);
    }
}

/// Asserts that the history `item` is the accrual `stored` of
/// [`ACCRUING_ACCOUNT`] under the retail rule: every identity and the points
/// as their decimal strings, and `created_at` an RFC 3339 instant equal to
/// [`CLOCK`].
fn assert_accrual(item: &Value, stored: &StoredAccrual, raw: &str) {
    let Value::Object(members) = item else {
        panic!("a history item is not an object but {item}: {raw}");
    };
    let mut keys: Vec<&str> = members.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected_keys = ACCRUAL_KEYS;
    expected_keys.sort_unstable();
    assert_eq!(keys, expected_keys, "the members of {item}: {raw}");
    let strings: Map<String, Value> = members
        .iter()
        .filter(|(key, _)| key.as_str() != "created_at")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    assert_eq!(
        Value::Object(strings),
        json!({
            "accrual_id": stored.accrual_id,
            "account_id": ACCRUING_ACCOUNT,
            "transaction_id": stored.transaction_id,
            "rule_id": RETAIL_RULE_ID,
            "points_earned": stored.points_earned,
        }),
        "the accrual of TRAN-ID {}, identities and points as strings: {raw}",
        stored.tran_id
    );
    let created_at = text(item, "created_at", raw);
    let instant = DateTime::parse_from_rfc3339(created_at)
        .unwrap_or_else(|error| panic!("`created_at` {created_at:?} is not RFC 3339: {error}"));
    assert_eq!(
        instant.with_timezone(&Utc),
        pinned_clock(),
        "`created_at` of the accrual of TRAN-ID {}: {raw}",
        stored.tran_id
    );
}

/// The instant [`CLOCK`] names.
fn pinned_clock() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(CLOCK)
        .unwrap_or_else(|error| panic!("{CLOCK} is not RFC 3339: {error}"))
        .with_timezone(&Utc)
}

/// The CT02 body of a confirmed retail purchase of `amount` on
/// [`ACCRUING_ACCOUNT`]: type `01`, category `0001`, dated on the clock's day.
fn purchase(amount: &str) -> Value {
    json!({
        "account_id": ACCRUING_ACCOUNT,
        "tran_type_cd": "01",
        "tran_cat_cd": "0001",
        "tran_source": "POS TERM",
        "tran_desc": "Rewards history purchase",
        "tran_amt": amount,
        "tran_orig_date": "2022-07-01",
        "tran_proc_date": "2022-07-01",
        "tran_merchant_id": "000000001",
        "tran_merchant_name": "Rewards merchant",
        "tran_merchant_city": "Rewards city",
        "tran_merchant_zip": "12345",
        "confirm": "Y",
    })
}

/// Adds the purchase of `amount` through `POST /transactions` and returns
/// its TRAN-ID, asserting the 201 and the `COTRN02C` success text.
async fn post_purchase(app: &TestApp, token: &str, amount: &str) -> String {
    let (status, body, raw) = app
        .post("/transactions", Some(token), &purchase(amount))
        .await;
    assert_eq!(status, StatusCode::CREATED, "CT02 add of {amount}: {raw}");
    let tran_id = text(&body["transaction"], "tran_id", &raw).to_owned();
    assert_eq!(
        body["message"].as_str(),
        Some(messages::transaction_added_message(&tran_id).as_str()),
        "CT02 message of {amount}: {raw}"
    );
    tran_id
}

/// The accruals of account 1 newest first, as [`ACCOUNT_1_ACCRUALS`] reads
/// them.
async fn stored_accruals(app: &TestApp) -> Vec<StoredAccrual> {
    app.stack()
        .fetch_text_rows(ACCOUNT_1_ACCRUALS)
        .await
        .into_iter()
        .map(|row| {
            let column = |index: usize| {
                row.get(index)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| panic!("column {index} of an accrual row is null: {row:?}"))
            };
            StoredAccrual {
                accrual_id: column(0),
                transaction_id: column(1),
                points_earned: column(2),
                tran_id: column(3),
            }
        })
        .collect()
}

/// `raw` without the JSON whitespace (space, tab, line feed, carriage return)
/// outside string literals, so a member can be matched in its exact
/// serialization however the body is laid out.
fn without_json_whitespace(raw: &str) -> String {
    let mut compact = String::with_capacity(raw.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if in_string {
            compact.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if !matches!(ch, ' ' | '\t' | '\n' | '\r') {
            in_string = ch == '"';
            compact.push(ch);
        }
    }
    compact
}

#[test]
fn accounts_without_accruals_answer_zero_and_unknown_accounts_or_no_token_are_refused() {
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        // A `U` token suffices: both routes are user functions.
        let token = app.user_token().await;
        assert!(
            app.stack()
                .fetch_text_rows("SELECT account_id FROM reward_balances WHERE account_id = 2")
                .await
                .is_empty(),
            "account 2 must start without a reward balance row"
        );
        let balance = app.get(&balance_path(QUIET_ACCOUNT), Some(&token)).await;
        assert_balance(&balance, QUIET_ACCOUNT, "0", "0", "0");
        let history = app
            .get(&history_path(QUIET_ACCOUNT, ""), Some(&token))
            .await;
        assert_history_page(&history, &[], None);

        for account in [UNKNOWN_ACCOUNT, MALFORMED_ACCOUNT] {
            assert_account_not_found(&app.get(&balance_path(account), Some(&token)).await);
            assert_account_not_found(&app.get(&history_path(account, ""), Some(&token)).await);
        }
        for path in [balance_path(QUIET_ACCOUNT), history_path(QUIET_ACCOUNT, "")] {
            assert_unauthenticated(&app.get(&path, None).await);
        }
    });
}

#[test]
fn ct02_accruals_make_the_balance_and_a_newest_first_paged_history() {
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let stack = app.stack();
        assert_eq!(
            stack.fetch_text_rows(RETAIL_RULE).await,
            [[
                Some(RETAIL_RULE_ID.to_owned()),
                Some("1.0000".to_owned()),
                None
            ]],
            "the seeded retail rule: its ID, its rate and no end"
        );
        stack.exec_sql(LIVE_ROW).await;
        let token = app.user_token().await;
        let mut tran_ids = Vec::new();
        for (amount, _) in PURCHASES {
            tran_ids.push(post_purchase(&app, &token, amount).await);
        }

        // Newest first: the last purchase's accrual leads.
        let stored = stored_accruals(&app).await;
        let posted: Vec<(String, String)> = tran_ids
            .iter()
            .zip(PURCHASES)
            .rev()
            .map(|(tran_id, (_, points))| (tran_id.clone(), points.to_owned()))
            .collect();
        let accrued: Vec<(String, String)> = stored
            .iter()
            .map(|accrual| (accrual.tran_id.clone(), accrual.points_earned.clone()))
            .collect();
        assert_eq!(
            accrued, posted,
            "the accruals of account 1 newest first, as (TRAN-ID, points)"
        );

        let balance = app.get(&balance_path(ACCRUING_ACCOUNT), Some(&token)).await;
        assert_balance(
            &balance,
            ACCRUING_ACCOUNT,
            PURCHASED_POINTS,
            "0",
            PURCHASED_POINTS,
        );

        let first = app
            .get(&history_path(ACCRUING_ACCOUNT, "page_size=2"), Some(&token))
            .await;
        assert_history_page(&first, &stored[..2], Some(&stored[2].accrual_id));
        let next_key = text(&first.1, "next_key", &first.2);
        let second_path = history_path(
            ACCRUING_ACCOUNT,
            &format!("start_key={next_key}&page_size=2"),
        );
        let second = app.get(&second_path, Some(&token)).await;
        assert_history_page(&second, &stored[2..], None);

        // Without `page_size`, one page of up to 10 holds all three.
        let whole = app
            .get(&history_path(ACCRUING_ACCOUNT, ""), Some(&token))
            .await;
        assert_history_page(&whole, &stored, None);
    });
}

#[test]
fn a_points_balance_above_2_pow_53_arrives_as_its_exact_digit_string() {
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        app.stack().exec_sql(LARGE_BALANCE).await;
        let token = app.user_token().await;
        let resp = app.get(&balance_path(LARGE_ACCOUNT), Some(&token)).await;
        assert_balance(&resp, LARGE_ACCOUNT, ABOVE_JS_SAFE, "0", ABOVE_JS_SAFE);
        let (_, body, raw) = &resp;
        let compact = without_json_whitespace(raw);
        for key in ["total_points_earned", "available_points"] {
            let member = format!("\"{key}\":\"{ABOVE_JS_SAFE}\"");
            assert!(
                compact.contains(&member),
                "the body must carry {member} exactly: {raw}"
            );
            assert_eq!(
                body[key],
                Value::String(ABOVE_JS_SAFE.into()),
                "`{key}` of the balance: {raw}"
            );
        }
    });
}

#[test]
fn the_whitespace_filter_keeps_string_contents_and_drops_json_whitespace() {
    // (raw body, without JSON whitespace)
    let cases = [
        (r#"{"a":"1"}"#, r#"{"a":"1"}"#),
        (
            "{ \"a\" :\t\"1\" ,\r\n \"b\": \"x y\" }",
            r#"{"a":"1","b":"x y"}"#,
        ),
        (r#"{"a": "q\" z", "b": 2}"#, r#"{"a":"q\" z","b":2}"#),
        (r#"{"a": "\\", "b": " "}"#, r#"{"a":"\\","b":" "}"#),
    ];
    for (raw, compact) in cases {
        assert_eq!(without_json_whitespace(raw), compact, "{raw}");
    }
}
