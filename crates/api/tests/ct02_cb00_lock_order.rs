//! A CT02 add and a CB00 payment on one account, each made to wait behind the
//! other's lock: the lock order `carddemo_persistence::ids` documents.
//!
//! The CT02 accrual that inserts the account's first `reward_balances` row
//! needs `FOR KEY SHARE` on the `accounts` row, the lock of the foreign key
//! `reward_balances_account_id_fkey`. A confirmed CB00 holds that row `FOR
//! UPDATE` from its `READ UPDATE` until it commits. Both also take the TRAN-ID
//! advisory lock. Here a gate unit of work holds the advisory lock while CT02,
//! then CB00, queue behind it. Once the gate rolls back, both requests must
//! complete, rather than one failing on a deadlock, with consecutive
//! TRAN-IDs, the paid balance and the accrued points.
//!
//! Docker must be running; the harness conventions are those of `common`.

pub mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use carddemo_domain::messages;
use carddemo_persistence::Db;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use common::{Resp, TestApp};

/// The processing clock, inside the active period of the seeded retail reward
/// rule (`01`/`0001`, rate 1.0000, from 2022-01-01).
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// Account 1 of the seed: balance 194.00, card `9680294154603697`, and no
/// `reward_balances` row, so the CT02 accrual inserts the first one.
const ACCOUNT_ID: &str = "00000000001";

/// The one live row the allocations count from. The seeded live master is
/// empty (E-07), and an add against an empty master allocates nothing (E-27).
const LIVE_ROW: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES ('0000000000000300', '01', 1, 'POS TERM', 'Lock order base row', 10.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '9680294154603697', '2022-06-01 00:00:00.000000', '2022-06-01 00:00:00.000000')";

/// The TRAN-IDs the gate and then the two requests allocate: the live highest
/// key plus one, and plus two.
const ALLOCATED: [&str; 2] = ["0000000000000301", "0000000000000302"];

/// The sessions waiting for a heavyweight lock: the advisory lock, or a row
/// lock, which a session waits for as the transaction ID of its holder.
const LOCK_WAITERS: &str =
    "SELECT count(*)::text FROM pg_stat_activity WHERE wait_event_type = 'Lock'";

/// The reward balance of account 1, as text.
const REWARD_BALANCE: &str = "SELECT total_points_earned::text, available_points::text \
     FROM reward_balances WHERE account_id = 1";

/// The current balance of account 1, as text.
const ACCOUNT_BALANCE: &str = "SELECT acct_curr_bal::text FROM accounts WHERE account_id = 1";

/// The live rows the two requests added, with their amounts.
const ADDED_ROWS: &str = "SELECT tran_id, tran_amt::text FROM transaction_master \
     WHERE tran_id > '0000000000000300' ORDER BY tran_id";

/// How long a request may take to reach its lock wait.
const LOCK_WAIT_DEADLINE: Duration = Duration::from_secs(30);

/// The pause between two reads of [`LOCK_WAITERS`].
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[test]
fn ct02_first_accrual_and_cb00_queued_on_one_account_both_complete() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROW).await;
        assert!(
            app.stack().fetch_text_rows(REWARD_BALANCE).await.is_empty(),
            "account 1 must start without a reward balance row"
        );
        let token = app.user_token().await;

        let db = Db::from_pool(app.stack().pool().clone());
        let mut gate = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("opening the gate unit of work failed: {error}"));
        let gate_id = gate
            .next_tran_id()
            .await
            .unwrap_or_else(|error| panic!("the gate allocation failed: {error}"));
        assert_eq!(gate_id.to_string(), ALLOCATED[0], "the gate's allocation");

        // CT02 waits for the gate's advisory lock, holding `FOR KEY SHARE` on
        // account 1.
        let add = spawn_post(&app, "/transactions", &token, &ct02_body());
        wait_for_lock_waiters(&app, 1).await;
        // CB00 waits too: its `FOR UPDATE` of account 1 for CT02's `FOR KEY
        // SHARE`. Without that lock taken first, it would hold the row and
        // wait for the advisory lock, and CT02's accrual would then deadlock.
        let payment_body = json!({ "account_id": ACCOUNT_ID, "confirm": "Y" });
        let payment = spawn_post(&app, "/payments", &token, &payment_body);
        wait_for_lock_waiters(&app, 2).await;
        gate.rollback()
            .await
            .unwrap_or_else(|error| panic!("rolling back the gate failed: {error}"));

        let added = joined("CT02", add).await;
        let paid = joined("CB00", payment).await;
        assert_both_completed(&app, &added, &paid).await;
    });
}

/// The CT02 body: a confirmed 50.00 purchase on account 1 under the retail
/// rule, which accrues `ceil(50.00 × 1.0)` = 50 points.
fn ct02_body() -> Value {
    json!({
        "account_id": ACCOUNT_ID,
        "tran_type_cd": "01",
        "tran_cat_cd": "0001",
        "tran_source": "POS TERM",
        "tran_desc": "Lock order purchase",
        "tran_amt": "+00000050.00",
        "tran_orig_date": "2022-07-01",
        "tran_proc_date": "2022-07-01",
        "tran_merchant_id": "000000001",
        "tran_merchant_name": "Lock order merchant",
        "tran_merchant_city": "Lock order city",
        "tran_merchant_zip": "12345",
        "confirm": "Y",
    })
}

/// Sends `POST path` with the JSON `body` and `token` as the bearer token on a
/// task of its own, so the request can wait for a lock while the test goes on.
fn spawn_post(app: &TestApp, path: &str, token: &str, body: &Value) -> JoinHandle<Resp> {
    let request = app
        .client()
        .post(app.url(path))
        .bearer_auth(token)
        .json(body);
    let path = path.to_owned();
    tokio::spawn(async move {
        let response = request
            .send()
            .await
            .unwrap_or_else(|error| panic!("POST {path} failed: {error}"));
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|error| panic!("reading the POST {path} body failed: {error}"));
        let json = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, json, text)
    })
}

/// The response of the request task `handle`, named `name` in a failure.
async fn joined(name: &str, handle: JoinHandle<Resp>) -> Resp {
    handle
        .await
        .unwrap_or_else(|error| panic!("the {name} request task failed: {error}"))
}

/// Waits until exactly `count` sessions wait for a lock, panicking when that
/// does not happen within [`LOCK_WAIT_DEADLINE`].
async fn wait_for_lock_waiters(app: &TestApp, count: usize) {
    let expected = vec![vec![Some(count.to_string())]];
    let until = Instant::now() + LOCK_WAIT_DEADLINE;
    loop {
        let waiting = app.stack().fetch_text_rows(LOCK_WAITERS).await;
        if waiting == expected {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {count} lock waiter(s) within {LOCK_WAIT_DEADLINE:?}, last saw {waiting:?}"
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

/// Asserts that the CT02 add and the CB00 payment both answered 201 with
/// their success texts and the two consecutive TRAN-IDs, that the payment
/// cleared the balance, and that the add accrued its 50 points.
async fn assert_both_completed(app: &TestApp, added: &Resp, paid: &Resp) {
    let (add_status, add_json, add_raw) = added;
    let (pay_status, pay_json, pay_raw) = paid;
    assert_eq!(*add_status, StatusCode::CREATED, "CT02 answer: {add_raw}");
    assert_eq!(*pay_status, StatusCode::CREATED, "CB00 answer: {pay_raw}");

    let add_id = add_json["transaction"]["tran_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the CT02 answer has no tran_id: {add_raw}"));
    let pay_id = pay_json["tran_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the CB00 answer has no tran_id: {pay_raw}"));
    assert_eq!(
        BTreeSet::from([add_id, pay_id]),
        BTreeSet::from(ALLOCATED),
        "the two TRAN-IDs"
    );
    assert_eq!(
        add_json["message"].as_str(),
        Some(messages::transaction_added_message(add_id).as_str()),
        "CT02 message: {add_raw}"
    );
    assert_eq!(
        pay_json["message"].as_str(),
        Some(messages::payment_successful_message(pay_id).as_str()),
        "CB00 message: {pay_raw}"
    );
    assert_eq!(
        pay_json["current_balance"].as_str(),
        Some("0.00"),
        "CB00 balance after the payment: {pay_raw}"
    );

    let stack = app.stack();
    assert_eq!(
        stack.fetch_text_rows(ACCOUNT_BALANCE).await,
        [[Some("0.00".to_owned())]],
        "the balance of account 1 after the payment"
    );
    assert_eq!(
        stack.fetch_text_rows(REWARD_BALANCE).await,
        [[Some("50".to_owned()), Some("50".to_owned())]],
        "total_points_earned and available_points of account 1"
    );
    let mut expected_rows = vec![
        vec![Some(add_id.to_owned()), Some("50.00".to_owned())],
        vec![Some(pay_id.to_owned()), Some("194.00".to_owned())],
    ];
    expected_rows.sort();
    assert_eq!(
        stack.fetch_text_rows(ADDED_ROWS).await,
        expected_rows,
        "the live rows the two requests added, by TRAN-ID, with their amounts"
    );
}
