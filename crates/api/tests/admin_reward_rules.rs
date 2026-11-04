//! Reward-rule administration proved through the HTTP layer: `GET` and
//! `POST /admin/reward-rules`, `PUT` and `DELETE /admin/reward-rules/{id}`
//! (AAP 0.3.5, o8).
//!
//! - The list answers every rule, active and soft-deleted, in `rule_id`
//!   order. A fresh seed holds the two seed rules of AAP 0.3.6: `01` Purchase
//!   with `0001` Regular Sales Draft at 1.0000 [app/data/ASCII/trantype.txt:L1]
//!   [app/data/ASCII/trancatg.txt:L1], and `04` Authorization with `0003`
//!   Travel booking authorization at 3.0000 [app/data/ASCII/trantype.txt:L4]
//!   [app/data/ASCII/trancatg.txt:L14], both active from 2022-01-01T00:00:00Z
//!   with no amount bounds and no end.
//! - Create and update apply three checks, each strict: a negative rate, a
//!   minimum above the maximum and an `active_to` before `active_from` are
//!   400 `INVALID` with the request-written message and the one field it
//!   names, and nothing is written. Equal bounds and an empty window
//!   (`active_to` equal to `active_from`) are accepted.
//! - A delete is soft: it sets `active_to` to the processing clock when the
//!   rule is open-ended or ends later, and leaves a rule that has already
//!   ended unchanged, and its answer names that `outcome` (`deleted` or
//!   `unchanged`). The clock is pinned to [`PINNED_CLOCK`] through
//!   `CARDDEMO_CLOCK_OVERRIDE`, so the stored instant is known.
//! - Concurrent deletes of one rule end it once: exactly one answers `deleted`
//!   and the others `unchanged`, all with the one `active_to` stored, and a
//!   delete that waited for another unit of work's lock on the rule answers
//!   `unchanged` with the end that unit committed. That test runs on the
//!   system clock, because a pinned clock gives every request the same
//!   instant and so cannot show a later request moving the end earlier.
//! - Every rule answered carries its `status` at the pinned clock, not at the
//!   wall clock: a rule starting after it is `scheduled`, one ending
//!   2022-07-02 is still `active`, and a soft-deleted rule is `ended`.
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a
//! roadmap item [README.md:L357-369], so no audit row gates these cases. The
//! 401 and 403 answers of these routes are proved by `admin_gate.rs`.
//!
//! Rates and amounts are compared as `Decimal` values, after asserting the
//! fraction digits the response publishes, and instants as parsed RFC 3339
//! date-times, so neither depends on how the server writes trailing zeros or
//! offsets. Docker must be running: each test starts one `postgres:16.15`
//! container.

pub mod common;

use std::time::{Duration, Instant};

use carddemo_api::dto::admin_reward_rules::RewardRuleField;
use carddemo_domain::clock::{Clock, SystemClock};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages::{
    REWARD_ACTIVE_TO_BEFORE_FROM, REWARD_MIN_EXCEEDS_MAX, REWARD_RATE_NEGATIVE,
    REWARD_RULE_NOT_FOUND,
};
use carddemo_domain::numeric::RuleId;
use carddemo_persistence::Db;
use carddemo_persistence::repos::SoftDeleteOutcome;
use carddemo_testkit::block_on;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;

use common::{Resp, TestApp, assert_error, send};

/// The list and create route.
const RULES: &str = "/admin/reward-rules";

/// The processing clock every test pins, `CARDDEMO_CLOCK_OVERRIDE`: the
/// instant a soft delete stores.
const PINNED_CLOCK: &str = "2022-07-01T12:00:00Z";

/// `active_from` of both seed rules (AAP 0.3.6).
const SEED_ACTIVE_FROM: &str = "2022-01-01T00:00:00Z";

/// The `rule_id` of the retail seed rule, `01`/`0001` at 1.0000.
const RETAIL_RULE_ID: &str = "1";

/// A rule ID no rule holds: the seed stores two and each test adds few.
const MISSING_RULE_ID: &str = "999";

/// Every stored rule in its `PostgreSQL` text form, in `rule_id` order: the
/// table state a rejected write must leave as it found it.
const RULE_ROWS: &str = "SELECT * FROM reward_rules ORDER BY rule_id";

/// The keys of a rule in every response: the 8 `reward_rules` columns and
/// `status`.
const RULE_KEYS: usize = 9;

/// The fraction digits of `reward_rate` in a response (`NUMERIC(9,4)`).
const RATE_SCALE: u32 = 4;

/// The fraction digits of an amount bound in a response (`S9(09)V99`).
const AMOUNT_SCALE: u32 = 2;

/// Every stored rule's `rule_id` and `active_to`, the end as an RFC 3339 UTC
/// instant with its microseconds, in `rule_id` order.
const RULE_ENDS: &str = r#"SELECT rule_id,
       to_char(active_to AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
  FROM reward_rules ORDER BY rule_id"#;

/// The `DELETE` requests of one rule the concurrency test sends at once.
const CONCURRENT_DELETES: usize = 8;

/// The rules the concurrency test deletes, one round of
/// [`CONCURRENT_DELETES`] each: whether two requests overlap is a matter of
/// timing, so one round alone could miss a race.
const CONCURRENT_ROUNDS: usize = 10;

/// The sessions waiting for a heavyweight lock: a delete waiting for the row
/// lock on a rule, which it waits for as the transaction ID of its holder.
const LOCK_WAITERS: &str =
    "SELECT count(*)::text FROM pg_stat_activity WHERE wait_event_type = 'Lock'";

/// How long a delete may take to reach its lock wait.
const LOCK_WAIT_DEADLINE: Duration = Duration::from_secs(30);

/// The pause between two reads of [`LOCK_WAITERS`].
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// One reward rule as a response carries it, with its decimals and instants
/// parsed, so that two rules compare by value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredRule {
    rule_id: String,
    transaction_type_cd: String,
    transaction_category_cd: String,
    reward_rate: Decimal,
    min_transaction_amount: Option<Decimal>,
    max_transaction_amount: Option<Decimal>,
    active_from: DateTime<Utc>,
    active_to: Option<DateTime<Utc>>,
    /// `scheduled`, `active` or `ended` at the pinned clock.
    status: String,
}

/// The `status` of a rule active at the pinned clock.
const ACTIVE: &str = "active";

/// The `status` of a rule that starts after the pinned clock.
const SCHEDULED: &str = "scheduled";

/// The `status` of a rule whose `active_to` is at or before the pinned clock.
const ENDED: &str = "ended";

/// The exact decimal `text`.
fn dec(text: &str) -> Decimal {
    Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text:?}: {error}"))
}

/// The UTC instant of the RFC 3339 date-time `text`.
fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|error| panic!("{text:?} is not RFC 3339: {error}"))
        .with_timezone(&Utc)
}

/// The value under `key` of the rule object `object`, whose raw body is
/// `raw`.
///
/// # Panics
///
/// Panics, showing `raw`, when the key is absent: a response writes every
/// key, `null` included.
fn member<'a>(object: &'a Map<String, Value>, key: &str, raw: &str) -> &'a Value {
    object
        .get(key)
        .unwrap_or_else(|| panic!("the rule has no `{key}`: {raw}"))
}

/// The string under `key`, or `None` for `null`.
fn text(object: &Map<String, Value>, key: &str, raw: &str) -> Option<String> {
    match member(object, key, raw) {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => panic!("`{key}` is neither a string nor null but {other}: {raw}"),
    }
}

/// The string under `key`, which must not be `null`.
fn required(object: &Map<String, Value>, key: &str, raw: &str) -> String {
    text(object, key, raw).unwrap_or_else(|| panic!("`{key}` is null: {raw}"))
}

/// The decimal string under `key`, written with exactly `scale` fraction
/// digits, or `None` for `null`.
fn decimal(object: &Map<String, Value>, key: &str, scale: u32, raw: &str) -> Option<Decimal> {
    text(object, key, raw).map(|text| {
        let value = dec(&text);
        assert_eq!(value.scale(), scale, "fraction digits of `{key}`: {raw}");
        value
    })
}

/// The rule a 2xx response body `value`, whose raw text is `raw`, carries.
///
/// # Panics
///
/// Panics, showing `raw`, unless `value` is an object of exactly the
/// [`RULE_KEYS`] rule keys, with the published types and fraction digits.
fn rule_from(value: &Value, raw: &str) -> StoredRule {
    let Value::Object(object) = value else {
        panic!("the rule is not a JSON object: {raw}");
    };
    assert_eq!(object.len(), RULE_KEYS, "keys of the rule: {raw}");
    let instant = |key: &str| text(object, key, raw).map(|text| at(&text));
    StoredRule {
        rule_id: required(object, "rule_id", raw),
        transaction_type_cd: required(object, "transaction_type_cd", raw),
        transaction_category_cd: required(object, "transaction_category_cd", raw),
        reward_rate: decimal(object, "reward_rate", RATE_SCALE, raw)
            .unwrap_or_else(|| panic!("`reward_rate` is null: {raw}")),
        min_transaction_amount: decimal(object, "min_transaction_amount", AMOUNT_SCALE, raw),
        max_transaction_amount: decimal(object, "max_transaction_amount", AMOUNT_SCALE, raw),
        active_from: instant("active_from")
            .unwrap_or_else(|| panic!("`active_from` is null: {raw}")),
        active_to: instant("active_to"),
        status: required(object, "status", raw),
    }
}

/// The rule of `resp`, which must answer `status`.
fn rule_of(resp: &Resp, status: StatusCode) -> StoredRule {
    let (actual, body, raw) = resp;
    assert_eq!(*actual, status, "status of the rule answer: {raw}");
    rule_from(body, raw)
}

/// The rule and the `outcome` of the 200 answer `resp` to a delete: the rule
/// keys of [`rule_from`] with `outcome` beside them.
fn deletion_of(resp: &Resp) -> (StoredRule, String) {
    let (actual, body, raw) = resp;
    assert_eq!(
        *actual,
        StatusCode::OK,
        "status of the delete answer: {raw}"
    );
    let Value::Object(object) = body else {
        panic!("the delete answer is not a JSON object: {raw}");
    };
    let outcome = required(object, "outcome", raw);
    let mut rule = object.clone();
    rule.remove("outcome");
    (rule_from(&Value::Object(rule), raw), outcome)
}

/// The path of the rule `rule_id`.
fn rule_path(rule_id: &str) -> String {
    format!("{RULES}/{rule_id}")
}

/// The seed rules of AAP 0.3.6 as a fresh seed stores them.
fn seed_rules() -> Vec<StoredRule> {
    let seed = |rule_id: &str, type_cd: &str, category_cd: &str, rate: &str| StoredRule {
        rule_id: rule_id.to_owned(),
        transaction_type_cd: type_cd.to_owned(),
        transaction_category_cd: category_cd.to_owned(),
        reward_rate: dec(rate),
        min_transaction_amount: None,
        max_transaction_amount: None,
        active_from: at(SEED_ACTIVE_FROM),
        active_to: None,
        status: ACTIVE.to_owned(),
    };
    vec![
        seed(RETAIL_RULE_ID, "01", "0001", "1.0000"),
        seed("2", "04", "0003", "3.0000"),
    ]
}

/// A valid new rule: type `01` Purchase with category `2`, the one-digit form
/// of `0002` Regular Cash Advance [app/data/ASCII/trancatg.txt:L2], at 2.5000
/// points per dollar for amounts from 10.00 to 500.00, open-ended from
/// 2022-01-01.
fn new_rule_body() -> Value {
    json!({
        "transaction_type_cd": "01",
        "transaction_category_cd": "2",
        "reward_rate": "2.5000",
        "min_transaction_amount": "10.00",
        "max_transaction_amount": "500.00",
        "active_from": SEED_ACTIVE_FROM,
        "active_to": null,
    })
}

/// The rule [`new_rule_body`] stores under `rule_id`.
fn new_rule(rule_id: &str) -> StoredRule {
    StoredRule {
        rule_id: rule_id.to_owned(),
        transaction_type_cd: "01".to_owned(),
        transaction_category_cd: "0002".to_owned(),
        reward_rate: dec("2.5000"),
        min_transaction_amount: Some(dec("10.00")),
        max_transaction_amount: Some(dec("500.00")),
        active_from: at(SEED_ACTIVE_FROM),
        active_to: None,
        status: ACTIVE.to_owned(),
    }
}

/// `body` with each `(field, value)` of `changes` set.
fn with(mut body: Value, changes: &[(&str, Value)]) -> Value {
    for (field, value) in changes {
        body[*field] = value.clone();
    }
    body
}

/// The body of each strict rule violation, with the message and the field
/// it is answered with. Each body breaks one check only, so the answer names
/// one field.
fn violations() -> [(Value, &'static str, RewardRuleField); 3] {
    [
        (
            with(new_rule_body(), &[("reward_rate", json!("-0.0001"))]),
            REWARD_RATE_NEGATIVE,
            RewardRuleField::RewardRate,
        ),
        (
            with(
                new_rule_body(),
                &[
                    ("min_transaction_amount", json!("100.00")),
                    ("max_transaction_amount", json!("50.00")),
                ],
            ),
            REWARD_MIN_EXCEEDS_MAX,
            RewardRuleField::MinTransactionAmount,
        ),
        (
            // One microsecond before `active_from`, the finest instant stored.
            with(
                new_rule_body(),
                &[("active_to", json!("2021-12-31T23:59:59.999999Z"))],
            ),
            REWARD_ACTIVE_TO_BEFORE_FROM,
            RewardRuleField::ActiveTo,
        ),
    ]
}

/// The replacement a valid `PUT` sends: every column changed, the minimum
/// cleared, and instants and decimals written in other accepted forms (an
/// offset, a one-digit fraction).
fn replacement_body() -> Value {
    json!({
        "transaction_type_cd": "04",
        "transaction_category_cd": "0003",
        "reward_rate": "0.5",
        "min_transaction_amount": null,
        "max_transaction_amount": "75.5",
        "active_from": "2022-02-01T01:00:00+01:00",
        "active_to": "2023-01-01T00:00:00Z",
    })
}

/// The rule [`replacement_body`] stores under `rule_id`.
fn replaced_rule(rule_id: &str) -> StoredRule {
    StoredRule {
        rule_id: rule_id.to_owned(),
        transaction_type_cd: "04".to_owned(),
        transaction_category_cd: "0003".to_owned(),
        reward_rate: dec("0.5000"),
        min_transaction_amount: None,
        max_transaction_amount: Some(dec("75.50")),
        active_from: at("2022-02-01T00:00:00Z"),
        active_to: Some(at("2023-01-01T00:00:00Z")),
        status: ACTIVE.to_owned(),
    }
}

/// The numeric value of the decimal string `rule_id`.
fn id_number(rule_id: &str) -> i64 {
    rule_id
        .parse()
        .unwrap_or_else(|error| panic!("`rule_id` {rule_id:?} is not an integer: {error}"))
}

/// `GET /admin/reward-rules`, which must answer 200 with its rules in
/// ascending `rule_id` order.
async fn list(app: &TestApp, token: &str) -> Vec<StoredRule> {
    let (status, body, raw) = app.get(RULES, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "status of the list: {raw}");
    let Value::Array(items) = &body else {
        panic!("the list is not a JSON array: {raw}");
    };
    let rules: Vec<StoredRule> = items.iter().map(|item| rule_from(item, &raw)).collect();
    assert!(
        rules
            .windows(2)
            .all(|pair| id_number(&pair[0].rule_id) < id_number(&pair[1].rule_id)),
        "the list is not in ascending `rule_id` order: {raw}"
    );
    rules
}

/// `POST /admin/reward-rules` with `body`, which must answer 201 with the
/// rule as stored.
async fn create(app: &TestApp, token: &str, body: &Value) -> StoredRule {
    rule_of(
        &app.post(RULES, Some(token), body).await,
        StatusCode::CREATED,
    )
}

/// Asserts that `resp` is the 404 answer for a rule ID that names no rule.
fn assert_rule_not_found(resp: &Resp) {
    assert_error(
        resp,
        StatusCode::NOT_FOUND,
        ErrorKind::NotFound.code(),
        REWARD_RULE_NOT_FOUND,
        &[],
    );
}

/// The `active_to` stored for the rule `rule_id`, `None` when it is null.
///
/// # Panics
///
/// Panics, showing every stored end, when no rule has the ID.
async fn stored_end(app: &TestApp, rule_id: &str) -> Option<DateTime<Utc>> {
    let rows = app.stack().fetch_text_rows(RULE_ENDS).await;
    let row = rows
        .iter()
        .find(|row| row.first().cloned().flatten().as_deref() == Some(rule_id))
        .unwrap_or_else(|| panic!("no stored rule has `rule_id` {rule_id}: {rows:?}"));
    row.get(1).cloned().flatten().map(|end| at(&end))
}

/// Sends `DELETE path` with `token` as the bearer token on a task of its own,
/// so that requests overlap, and one can wait for a lock while the test goes
/// on.
fn spawn_delete(app: &TestApp, path: &str, token: &str) -> JoinHandle<Resp> {
    let request = app.client().delete(app.url(path));
    let token = token.to_owned();
    tokio::spawn(async move { send(request, Some(&token)).await })
}

/// The response of the delete task `handle`.
async fn joined(handle: JoinHandle<Resp>) -> Resp {
    handle
        .await
        .unwrap_or_else(|error| panic!("a delete request task failed: {error}"))
}

/// Waits until exactly one session waits for a lock, panicking when that does
/// not happen within [`LOCK_WAIT_DEADLINE`].
async fn wait_for_one_lock_waiter(app: &TestApp) {
    let expected = vec![vec![Some("1".to_owned())]];
    let until = Instant::now() + LOCK_WAIT_DEADLINE;
    loop {
        let waiting = app.stack().fetch_text_rows(LOCK_WAITERS).await;
        if waiting == expected {
            return;
        }
        assert!(
            Instant::now() < until,
            "no delete waited for the rule's lock within {LOCK_WAIT_DEADLINE:?}, last saw {waiting:?}"
        );
        pause(POLL_INTERVAL).await;
    }
}

/// Sleeps for `period` on a blocking thread, leaving the runtime's workers to
/// the server and the request tasks.
async fn pause(period: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(period))
        .await
        .unwrap_or_else(|error| panic!("the pause task failed: {error}"));
}

/// The list answers the two seed rules; a create answers 201 with the rule
/// as stored under a new `rule_id`, and the boundary of each strict check is
/// accepted: a minimum equal to the maximum, and an `active_to` equal to
/// `active_from`. Each rule's `status` places the pinned clock against its
/// window, in the create answer and in the list alike.
#[test]
fn list_shows_the_seed_rules_and_create_stores_new_rules() {
    let app = TestApp::start_with_clock(PINNED_CLOCK);
    block_on(async {
        let token = app.admin_token().await;
        let seeds = seed_rules();
        assert_eq!(list(&app, &token).await, seeds, "the seed rules");

        let created = create(&app, &token, &new_rule_body()).await;
        assert!(
            seeds
                .iter()
                .all(|seed| id_number(&seed.rule_id) < id_number(&created.rule_id)),
            "the new rule did not get a new `rule_id`: {created:?}"
        );
        assert_eq!(created, new_rule(&created.rule_id), "the created rule");

        let boundary_body = with(
            new_rule_body(),
            &[
                ("min_transaction_amount", json!("25.00")),
                ("max_transaction_amount", json!("25.00")),
                ("active_to", json!(SEED_ACTIVE_FROM)),
            ],
        );
        let boundary = create(&app, &token, &boundary_body).await;
        // The empty window holds no instant and lies before the pinned clock.
        let expected_boundary = StoredRule {
            min_transaction_amount: Some(dec("25.00")),
            max_transaction_amount: Some(dec("25.00")),
            active_to: Some(at(SEED_ACTIVE_FROM)),
            status: ENDED.to_owned(),
            ..new_rule(&boundary.rule_id)
        };
        assert_eq!(boundary, expected_boundary, "the boundary rule");

        let mut expected = seeds;
        expected.extend([created, boundary]);

        // The status is taken at the pinned clock, whatever the wall clock
        // reads: (`active_from`, `active_to`, status).
        let windows = [
            ("2030-01-01T00:00:00Z", None, SCHEDULED),
            ("2022-07-01T12:00:00.000001Z", None, SCHEDULED),
            (PINNED_CLOCK, None, ACTIVE),
            (SEED_ACTIVE_FROM, Some("2022-07-02T00:00:00Z"), ACTIVE),
            (SEED_ACTIVE_FROM, Some(PINNED_CLOCK), ENDED),
        ];
        for (from, to, status) in windows {
            let body = with(
                new_rule_body(),
                &[("active_from", json!(from)), ("active_to", json!(to))],
            );
            let rule = create(&app, &token, &body).await;
            let stored = StoredRule {
                active_from: at(from),
                active_to: to.map(at),
                status: status.to_owned(),
                ..new_rule(&rule.rule_id)
            };
            assert_eq!(rule, stored, "the rule from {from} to {to:?}");
            expected.push(stored);
        }
        assert_eq!(
            list(&app, &token).await,
            expected,
            "the list after the creates"
        );
    });
}

/// Each strict violation is 400 `INVALID` with its message on its one field,
/// on `POST` and on `PUT` of the retail seed rule alike, and writes nothing;
/// a `PUT` of a missing rule is 404 and writes nothing; a valid `PUT` of a
/// created rule replaces every column but its `rule_id` and answers 200.
#[test]
fn create_and_update_reject_strict_violations_and_update_replaces_the_rule() {
    let app = TestApp::start_with_clock(PINNED_CLOCK);
    block_on(async {
        let token = app.admin_token().await;
        let retail = rule_path(RETAIL_RULE_ID);
        let before = app.stack().fetch_text_rows(RULE_ROWS).await;
        for (body, message, field) in violations() {
            let fields = [(field.as_str(), message)];
            let invalid = ErrorKind::Invalid.code();
            let created = app.post(RULES, Some(&token), &body).await;
            assert_error(&created, StatusCode::BAD_REQUEST, invalid, message, &fields);
            let updated = app.put(&retail, Some(&token), &body).await;
            assert_error(&updated, StatusCode::BAD_REQUEST, invalid, message, &fields);
        }
        let missing = app
            .put(&rule_path(MISSING_RULE_ID), Some(&token), &new_rule_body())
            .await;
        assert_rule_not_found(&missing);
        assert_eq!(
            app.stack().fetch_text_rows(RULE_ROWS).await,
            before,
            "a rejected create or update changed `reward_rules`"
        );
        assert_eq!(
            list(&app, &token).await,
            seed_rules(),
            "the list after the rejections"
        );

        let created = create(&app, &token, &new_rule_body()).await;
        let path = rule_path(&created.rule_id);
        let updated = app.put(&path, Some(&token), &replacement_body()).await;
        let expected = replaced_rule(&created.rule_id);
        assert_eq!(
            rule_of(&updated, StatusCode::OK),
            expected,
            "the replaced rule"
        );

        let mut rules = seed_rules();
        rules.push(expected);
        assert_eq!(list(&app, &token).await, rules, "the list after the update");
    });
}

/// A delete sets `active_to` to the pinned clock for an open-ended rule, for
/// one that ends later and for one that has not started, answering `outcome`
/// `deleted`; it answers a rule that has already ended unchanged (a repeated
/// delete included) with `outcome` `unchanged`. Every deleted rule is
/// `ended`, and no row is removed: the list still shows every soft-deleted
/// rule in `rule_id` order. A missing rule is 404.
#[test]
fn delete_ends_the_window_at_the_pinned_clock_and_keeps_every_rule() {
    let app = TestApp::start_with_clock(PINNED_CLOCK);
    block_on(async {
        let token = app.admin_token().await;
        let retail = rule_path(RETAIL_RULE_ID);
        let mut expected = seed_rules();
        expected[0].active_to = Some(at(PINNED_CLOCK));
        expected[0].status = ENDED.to_owned();
        for (attempt, outcome) in [("first", "deleted"), ("repeated", "unchanged")] {
            let deleted = app.delete(&retail, Some(&token)).await;
            assert_eq!(
                deletion_of(&deleted),
                (expected[0].clone(), outcome.to_owned()),
                "the {attempt} delete of the open-ended retail rule"
            );
        }

        // (`active_from`, `active_to` at creation, its status then,
        // `active_to` after the delete, the delete's outcome)
        let cases = [
            (
                SEED_ACTIVE_FROM,
                Some("2030-01-01T00:00:00Z"),
                ACTIVE,
                PINNED_CLOCK,
                "deleted",
            ),
            (
                SEED_ACTIVE_FROM,
                Some("2022-03-01T00:00:00Z"),
                ENDED,
                "2022-03-01T00:00:00Z",
                "unchanged",
            ),
            (
                "2030-01-01T00:00:00Z",
                None,
                SCHEDULED,
                PINNED_CLOCK,
                "deleted",
            ),
        ];
        for (from, end, before, after, outcome) in cases {
            let body = with(
                new_rule_body(),
                &[("active_from", json!(from)), ("active_to", json!(end))],
            );
            let created = create(&app, &token, &body).await;
            assert_eq!(created.active_to, end.map(at), "the rule ending {end:?}");
            assert_eq!(created.status, before, "the rule from {from} to {end:?}");
            let deleted = app.delete(&rule_path(&created.rule_id), Some(&token)).await;
            let ended = StoredRule {
                active_to: Some(at(after)),
                status: ENDED.to_owned(),
                ..created
            };
            assert_eq!(
                deletion_of(&deleted),
                (ended.clone(), outcome.to_owned()),
                "the delete of the rule from {from} to {end:?}"
            );
            expected.push(ended);
        }

        assert_rule_not_found(&app.delete(&rule_path(MISSING_RULE_ID), Some(&token)).await);
        assert_eq!(
            list(&app, &token).await,
            expected,
            "the list after the deletes"
        );
    });
}

/// Concurrent deletes of one open-ended rule, on the system clock, end it
/// once and never move its end earlier:
///
/// - [`CONCURRENT_DELETES`] deletes sent at once: exactly one answers
///   `deleted`, every other `unchanged`, all 200 with the same ended rule,
///   whose `active_to` is the one stored. Each of [`CONCURRENT_ROUNDS`] rounds
///   deletes a rule of its own.
/// - A delete that waits while another unit of work holds the rule's lock,
///   ends it at an instant read after the request began to wait, and commits,
///   answers `unchanged` with that end, which stays stored: the request reads
///   its clock only once it holds the lock.
#[test]
fn concurrent_deletes_end_a_rule_once_on_the_system_clock() {
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        for round in 1..=CONCURRENT_ROUNDS {
            let created = create(&app, &token, &new_rule_body()).await;
            let path = rule_path(&created.rule_id);
            let requests: Vec<JoinHandle<Resp>> = (0..CONCURRENT_DELETES)
                .map(|_| spawn_delete(&app, &path, &token))
                .collect();
            let mut answers = Vec::with_capacity(CONCURRENT_DELETES);
            for request in requests {
                answers.push(deletion_of(&joined(request).await));
            }

            let deleted: Vec<&StoredRule> = answers
                .iter()
                .filter(|(_, outcome)| outcome == "deleted")
                .map(|(rule, _)| rule)
                .collect();
            let [ended] = deleted[..] else {
                panic!("round {round}: not exactly one `deleted` answer: {answers:?}");
            };
            assert!(
                ended.active_to.is_some(),
                "round {round}: the deleted rule has an end: {ended:?}"
            );
            let expected = StoredRule {
                active_to: ended.active_to,
                status: ENDED.to_owned(),
                ..created.clone()
            };
            assert_eq!(
                *ended, expected,
                "round {round}: the delete changed only the end"
            );
            let unchanged = answers
                .iter()
                .filter(|(rule, outcome)| outcome == "unchanged" && *rule == expected)
                .count();
            assert_eq!(
                unchanged,
                CONCURRENT_DELETES - 1,
                "round {round}: every other answer is `unchanged` with the one end: {answers:?}"
            );
            assert_eq!(
                stored_end(&app, &created.rule_id).await,
                expected.active_to,
                "round {round}: the stored end is the `deleted` answer's"
            );
        }

        let held = create(&app, &token, &new_rule_body()).await;
        let rule_id = RuleId::from(id_number(&held.rule_id));
        let db = Db::from_pool(app.stack().pool().clone());
        let mut holder = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("opening the holding unit of work failed: {error}"));
        let locked = holder
            .store()
            .lock_reward_rule(rule_id)
            .await
            .unwrap_or_else(|error| panic!("locking the rule failed: {error}"));
        assert!(locked.is_some(), "the created rule {rule_id:?} is locked");
        let waiting = spawn_delete(&app, &rule_path(&held.rule_id), &token);
        wait_for_one_lock_waiter(&app).await;
        // Read after the request began to wait, so later than any instant it
        // could have read before its lock.
        let ended_at = SystemClock.now();
        let ended = holder
            .store()
            .soft_delete_reward_rule(rule_id, ended_at)
            .await
            .unwrap_or_else(|error| panic!("the holding unit's delete failed: {error}"));
        assert!(
            matches!(ended, SoftDeleteOutcome::Deleted(_)),
            "the holding unit ends the open rule: {ended:?}"
        );
        holder
            .commit()
            .await
            .unwrap_or_else(|error| panic!("committing the holding unit failed: {error}"));
        let expected = StoredRule {
            active_to: Some(ended_at),
            status: ENDED.to_owned(),
            ..held
        };
        assert_eq!(
            deletion_of(&joined(waiting).await),
            (expected.clone(), "unchanged".to_owned()),
            "the delete that waited for the lock"
        );
        assert_eq!(
            stored_end(&app, &expected.rule_id).await,
            Some(ended_at),
            "the end committed first stays stored"
        );
    });
}
