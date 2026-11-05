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
//! - Six CT02 adds under the seeded retail rule (`01`/`0001`, rate 1.0000),
//!   alternating between two accounts in one database, accrue
//!   `ceil(amount × 1.0000)` points each. Each account's balance is the sum
//!   of its own points, and its history pages only its own accruals newest
//!   first by `start_key` and `page_size`, every accrual stamped with the
//!   pinned processing clock. A `start_key` naming the other account's
//!   accrual gives an empty page, and a third existing account that earned
//!   nothing still has the zero balance and an empty history after the
//!   other two have earned.
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

/// Account 1 of the fixture, active [app/data/ASCII/acctdata.txt:L1], with
/// card `9680294154603697` [app/data/ASCII/cardxref.txt:L49]: one of the two
/// accounts the CT02 adds post to.
const ACCOUNT_1: &str = "00000000001";

/// Account 2 of the fixture, active [app/data/ASCII/acctdata.txt:L2], with
/// card `0923877193247330` [app/data/ASCII/cardxref.txt:L3]: the other
/// account the CT02 adds post to.
const ACCOUNT_2: &str = "00000000002";

/// Account 4 of the fixture, active [app/data/ASCII/acctdata.txt:L4], with
/// card `2988091353094312` [app/data/ASCII/cardxref.txt:L13], which never
/// accrues here.
const QUIET_ACCOUNT: &str = "00000000004";

/// The `reward_balances` row of [`QUIET_ACCOUNT`], which must not exist.
const QUIET_BALANCE_ROW: &str = "SELECT account_id::text FROM reward_balances WHERE account_id = 4";

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

/// The six CT02 purchases in posting order, alternating between
/// [`ACCOUNT_1`] and [`ACCOUNT_2`], so each account's accruals interleave
/// with the other's: the account, the `TRNAMT` text in the `COTRN02C` amount
/// form `-99999999.99` (sign, eight digits, point, two digits) and the points
/// `ceil(amount × 1.0000)` it earns under the retail rule. The cent rounds up
/// to a whole point, 99.50 to 100, 20.25 to 21 and 33.10 to 34.
// No audit ID is cited here: these purchases pass every edit, so citing one
// would count as coverage of an error path this file never sends.
const PURCHASES: [(&str, &str, &str); 6] = [
    (ACCOUNT_1, "+00000010.00", "10"),
    (ACCOUNT_2, "+00000005.00", "5"),
    (ACCOUNT_1, "+00000000.01", "1"),
    (ACCOUNT_2, "+00000020.25", "21"),
    (ACCOUNT_1, "+00000099.50", "100"),
    (ACCOUNT_2, "+00000033.10", "34"),
];

/// Each accruing account with the sum of its [`PURCHASES`] points, which it
/// has earned and has available.
const ACCRUING_ACCOUNTS: [(&str, &str); 2] = [(ACCOUNT_1, "111"), (ACCOUNT_2, "60")];

/// The accruals on a page of the paged history reads.
const PAGE_ROWS: usize = 2;

/// Every accrual newest first, with the 11-digit key of the account it
/// credits, as the routes take it, and the TRAN-ID of the transaction it
/// credits.
const ACCRUALS: &str = "SELECT lpad(a.account_id::text, 11, '0'), a.accrual_id::text, \
            a.transaction_id::text, a.points_earned::text, m.tran_id \
     FROM reward_accruals a JOIN transaction_master m USING (transaction_id) \
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
    account_id: String,
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

/// Asserts that `resp` is a 200 history page of `account` holding exactly
/// `expected`, accruals of `account`, in that order, with `next_key` as given
/// and `prev_key` `null`, as the history pages forward only, and no
/// `message`.
fn assert_history_page(
    resp: &Resp,
    account: &str,
    expected: &[StoredAccrual],
    next_key: Option<&str>,
) {
    let (status, body, raw) = resp;
    assert_eq!(*status, StatusCode::OK, "history page of {account}: {raw}");
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
    assert_eq!(
        items.len(),
        expected.len(),
        "accruals on the page of {account}: {raw}"
    );
    for (item, stored) in items.iter().zip(expected) {
        assert_accrual(item, account, stored, raw);
    }
}

/// Asserts that the history `item` is the accrual `stored` of `account` under
/// the retail rule: every identity and the points as their decimal strings,
/// and `created_at` an RFC 3339 instant equal to [`CLOCK`].
fn assert_accrual(item: &Value, account: &str, stored: &StoredAccrual, raw: &str) {
    assert_eq!(
        stored.account_id, account,
        "the expected accrual of TRAN-ID {} credits {account}",
        stored.tran_id
    );
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
            "account_id": account,
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

/// The CT02 body of a confirmed retail purchase of `amount` on `account`:
/// type `01`, category `0001`, dated on the clock's day.
fn purchase(account: &str, amount: &str) -> Value {
    json!({
        "account_id": account,
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

/// Adds the purchase of `amount` on `account` through `POST /transactions`
/// and returns its TRAN-ID, asserting the 201 and the `COTRN02C` success
/// text.
async fn post_purchase(app: &TestApp, token: &str, account: &str, amount: &str) -> String {
    let (status, body, raw) = app
        .post("/transactions", Some(token), &purchase(account, amount))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "CT02 add of {amount} on {account}: {raw}"
    );
    let tran_id = text(&body["transaction"], "tran_id", &raw).to_owned();
    assert_eq!(
        body["message"].as_str(),
        Some(messages::transaction_added_message(&tran_id).as_str()),
        "CT02 message of {amount} on {account}: {raw}"
    );
    tran_id
}

/// Every accrual newest first, as [`ACCRUALS`] reads them.
async fn stored_accruals(app: &TestApp) -> Vec<StoredAccrual> {
    app.stack()
        .fetch_text_rows(ACCRUALS)
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
                account_id: column(0),
                accrual_id: column(1),
                transaction_id: column(2),
                points_earned: column(3),
                tran_id: column(4),
            }
        })
        .collect()
}

/// The accruals of `account` among `stored`, in the order of `stored`.
fn accruals_of(stored: &[StoredAccrual], account: &str) -> Vec<StoredAccrual> {
    stored
        .iter()
        .filter(|accrual| accrual.account_id == account)
        .cloned()
        .collect()
}

/// Reads the whole history of `account` [`PAGE_ROWS`] accruals a page,
/// starting each page at the previous page's `next_key` until it is `null`,
/// and asserts that every page is the next slice of `expected`, the stored
/// accruals of `account` newest first, and that the pages together hold
/// exactly those accruals.
async fn assert_paged_history(
    app: &TestApp,
    token: &str,
    account: &str,
    expected: &[StoredAccrual],
) {
    let mut shown: Vec<String> = Vec::new();
    let mut start_key: Option<String> = None;
    loop {
        let query = match &start_key {
            None => format!("page_size={PAGE_ROWS}"),
            Some(key) => format!("start_key={key}&page_size={PAGE_ROWS}"),
        };
        let page = app.get(&history_path(account, &query), Some(token)).await;
        let from = shown.len().min(expected.len());
        let to = (from + PAGE_ROWS).min(expected.len());
        let next_key = expected.get(to).map(|accrual| accrual.accrual_id.as_str());
        assert_history_page(&page, account, &expected[from..to], next_key);
        let (_, body, raw) = &page;
        for item in body["items"].as_array().into_iter().flatten() {
            shown.push(text(item, "accrual_id", raw).to_owned());
        }
        match body["next_key"].as_str() {
            Some(key) => start_key = Some(key.to_owned()),
            None => break,
        }
    }
    let stored_ids: Vec<&str> = expected
        .iter()
        .map(|accrual| accrual.accrual_id.as_str())
        .collect();
    assert_eq!(
        shown, stored_ids,
        "the pages of {account} together hold exactly its accruals, newest first"
    );
}

/// Asserts that [`QUIET_ACCOUNT`] has no `reward_balances` row and answers
/// the zero balance and an empty history page, both 200; `when` names the
/// state of the database in the messages.
async fn assert_quiet(app: &TestApp, token: &str, when: &str) {
    assert!(
        app.stack()
            .fetch_text_rows(QUIET_BALANCE_ROW)
            .await
            .is_empty(),
        "{QUIET_ACCOUNT} has no reward balance row {when}"
    );
    let balance = app.get(&balance_path(QUIET_ACCOUNT), Some(token)).await;
    assert_balance(&balance, QUIET_ACCOUNT, "0", "0", "0");
    let history = app.get(&history_path(QUIET_ACCOUNT, ""), Some(token)).await;
    assert_history_page(&history, QUIET_ACCOUNT, &[], None);
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
        assert_quiet(&app, &token, "on a fresh seed").await;

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
fn ct02_accruals_of_two_accounts_make_separate_balances_and_newest_first_paged_histories() {
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
        assert_quiet(&app, &token, "before any purchase").await;
        let mut tran_ids = Vec::new();
        for (account, amount, _) in PURCHASES {
            tran_ids.push(post_purchase(&app, &token, account, amount).await);
        }

        let stored = stored_accruals(&app).await;
        let mut histories = Vec::new();
        for (account, points) in ACCRUING_ACCOUNTS {
            // Newest first: the account's last purchase's accrual leads.
            let own = accruals_of(&stored, account);
            let posted: Vec<(String, String)> = tran_ids
                .iter()
                .zip(PURCHASES)
                .filter(|(_, (to, _, _))| *to == account)
                .rev()
                .map(|(tran_id, (_, _, earned))| (tran_id.clone(), earned.to_owned()))
                .collect();
            let accrued: Vec<(String, String)> = own
                .iter()
                .map(|accrual| (accrual.tran_id.clone(), accrual.points_earned.clone()))
                .collect();
            assert_eq!(
                accrued, posted,
                "the accruals of {account} newest first, as (TRAN-ID, points)"
            );
            let balance = app.get(&balance_path(account), Some(&token)).await;
            assert_balance(&balance, account, points, "0", points);
            histories.push((account, own));
        }
        let mut split_ids: Vec<&str> = histories
            .iter()
            .flat_map(|(_, own)| own.iter().map(|accrual| accrual.accrual_id.as_str()))
            .collect();
        let mut all_ids: Vec<&str> = stored
            .iter()
            .map(|accrual| accrual.accrual_id.as_str())
            .collect();
        split_ids.sort_unstable();
        all_ids.sort_unstable();
        assert_eq!(
            split_ids, all_ids,
            "every accrual credits exactly one of the two accruing accounts"
        );

        for (account, own) in &histories {
            assert_paged_history(&app, &token, account, own).await;
            // Without `page_size`, one page of up to 10 holds all three of the
            // account's accruals.
            let whole = app.get(&history_path(account, ""), Some(&token)).await;
            assert_history_page(&whole, account, own, None);
        }
        for ((account, _), (_, other_own)) in histories.iter().zip(histories.iter().rev()) {
            // The other account's newest accrual, newer than accruals of this
            // account, starts no page of this account: the page is empty.
            let foreign = &other_own[0].accrual_id;
            let query = format!("start_key={foreign}");
            let page = app.get(&history_path(account, &query), Some(&token)).await;
            assert_history_page(&page, account, &[], None);
        }

        assert_quiet(&app, &token, "after the other accounts have earned").await;
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
