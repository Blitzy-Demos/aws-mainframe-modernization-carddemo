//! CT02 adds and a confirmed CB00 payment queued behind the TRAN-ID advisory
//! lock, admitted through the API's per-process TRAN-ID writer limit so that
//! they never hold the whole connection pool.
//!
//! `carddemo_persistence::ids` serializes the allocation on an advisory lock
//! held until the allocating unit of work ends, and a writer waiting for it
//! holds a pooled connection. The API therefore admits allocating writers
//! before they open their unit of work: half the pool's connections but one,
//! and at least 2, which on the testkit pool of 5 connections is 2. Here a
//! gate unit of work holds the advisory lock while more writers arrive than
//! the limit admits. Exactly the admitted ones wait in the database, never
//! more, the rest queue in the API without a connection, and the reads
//! `GET /transactions` and `GET /accounts/{id}` still answer at once. Once the
//! gate rolls back, every writer answers 201 with consecutive TRAN-IDs, the
//! adds with their reward accruals and the payment with the cleared balance.
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
/// `reward_balances` row.
const ACCOUNT_ID: &str = "00000000001";

/// The one live row the allocations count from. The seeded live master is
/// empty (E-07), and an add against an empty master allocates nothing (E-27).
const LIVE_ROW: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES ('0000000000000300', '01', 1, 'POS TERM', 'Admission base row', 10.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '9680294154603697', '2022-06-01 00:00:00.000000', '2022-06-01 00:00:00.000000')";

/// The TRAN-ID the gate allocates and rolls back: the live highest key plus
/// one, which the first writer then allocates again.
const GATE_TRAN_ID: &str = "0000000000000301";

/// The writers sent while the gate holds the lock, more than [`ADMITTED`]:
/// CT02 adds, with the one CB00 payment at [`PAYMENT_AT`] among them.
const WRITERS: u64 = 5;

/// The position of the CB00 payment among the [`WRITERS`]. Account 1 has one
/// balance to pay, so one payment is all that can succeed.
const PAYMENT_AT: u64 = 2;

/// The writers the API admits at once on the testkit pool of 5 connections:
/// `max(2, (5 - 1) / 2)`. With the gate's connection, they leave 2 of the 5
/// to the bearer checks and the reads.
const ADMITTED: u64 = 2;

/// The sessions waiting for a heavyweight lock: the advisory lock, or a row
/// lock, which a session waits for as the transaction ID of its holder.
const LOCK_WAITERS: &str =
    "SELECT count(*)::text FROM pg_stat_activity WHERE wait_event_type = 'Lock'";

/// The reward balance of account 1, as text.
const REWARD_BALANCE: &str = "SELECT total_points_earned::text, available_points::text \
     FROM reward_balances WHERE account_id = 1";

/// The current balance of account 1, as text.
const ACCOUNT_BALANCE: &str = "SELECT acct_curr_bal::text FROM accounts WHERE account_id = 1";

/// The live rows the writers wrote, with their amounts.
const ADDED_ROWS: &str = "SELECT tran_id, tran_amt::text FROM transaction_master \
     WHERE tran_id > '0000000000000300' ORDER BY tran_id";

/// How long the admitted writers may take to reach their lock wait.
const LOCK_WAIT_DEADLINE: Duration = Duration::from_secs(30);

/// How long the lock waiters are watched once the admitted writers wait.
const OBSERVATION: Duration = Duration::from_secs(1);

/// The bound on each read while the writers queue. Without the admission the
/// writers would hold every pooled connection and a read would wait for the
/// pool's 30-second acquire timeout.
const READ_DEADLINE: Duration = Duration::from_secs(5);

/// The pause between two reads of [`LOCK_WAITERS`].
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[test]
fn queued_tran_id_writers_wait_only_as_admitted_while_reads_answer() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROW).await;
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
        assert_eq!(gate_id.to_string(), GATE_TRAN_ID, "the gate's allocation");

        let payment_body = json!({ "account_id": ACCOUNT_ID, "confirm": "Y" });
        let writers: Vec<_> = (0..WRITERS)
            .map(|at| {
                if at == PAYMENT_AT {
                    spawn_post(&app, "/payments", &token, &payment_body)
                } else {
                    spawn_post(&app, "/transactions", &token, &ct02_body())
                }
            })
            .collect();
        wait_for_lock_waiters(&app, ADMITTED).await;
        let until = Instant::now() + OBSERVATION;
        while Instant::now() < until {
            let waiting = lock_waiters(&app).await;
            assert!(
                waiting <= ADMITTED,
                "{waiting} sessions wait for a lock; the API admits {ADMITTED} writers"
            );
            pause(POLL_INTERVAL).await;
        }

        for path in ["/transactions", "/accounts/00000000001"] {
            let read = app.client().get(app.url(path)).timeout(READ_DEADLINE);
            let (status, _, raw) = common::send(read, Some(&token)).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "GET {path} while the writers queue: {raw}"
            );
        }
        assert!(
            writers.iter().all(|writer| !writer.is_finished()),
            "no writer can answer while the gate holds the TRAN-ID lock"
        );

        gate.rollback()
            .await
            .unwrap_or_else(|error| panic!("rolling back the gate failed: {error}"));
        let mut answers = Vec::new();
        for writer in writers {
            answers.push(
                writer
                    .await
                    .unwrap_or_else(|error| panic!("a writer's request task failed: {error}")),
            );
        }
        assert_every_writer_completed(&app, answers).await;
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
        "tran_desc": "Admission purchase",
        "tran_amt": "+00000050.00",
        "tran_orig_date": "2022-07-01",
        "tran_proc_date": "2022-07-01",
        "tran_merchant_id": "000000001",
        "tran_merchant_name": "Admission merchant",
        "tran_merchant_city": "Admission city",
        "tran_merchant_zip": "12345",
        "confirm": "Y",
    })
}

/// Sends `POST path` with the JSON `body` and `token` as the bearer token on a
/// task of its own, so the request can queue while the test goes on.
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

/// The number of sessions waiting for a lock now.
async fn lock_waiters(app: &TestApp) -> u64 {
    let rows = app.stack().fetch_text_rows(LOCK_WAITERS).await;
    let [row] = rows.as_slice() else {
        panic!("one count row expected, got {rows:?}");
    };
    row.first()
        .and_then(Option::as_deref)
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("a count expected, got {row:?}"))
}

/// Waits until exactly `count` sessions wait for a lock, panicking when that
/// does not happen within [`LOCK_WAIT_DEADLINE`].
async fn wait_for_lock_waiters(app: &TestApp, count: u64) {
    let until = Instant::now() + LOCK_WAIT_DEADLINE;
    loop {
        let waiting = lock_waiters(app).await;
        if waiting == count {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {count} lock waiter(s) within {LOCK_WAIT_DEADLINE:?}, last saw {waiting}"
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

/// Asserts that every writer answered 201 with its success text, that
/// together they hold the consecutive TRAN-IDs from [`GATE_TRAN_ID`] on, each
/// written once with its amount, that the payment cleared the balance of
/// 194.00, and that each add accrued its 50 points.
async fn assert_every_writer_completed(app: &TestApp, mut answers: Vec<Resp>) {
    let at = usize::try_from(PAYMENT_AT).expect("a position fits usize");
    let (pay_status, pay_json, pay_raw) = answers.remove(at);
    assert_eq!(pay_status, StatusCode::CREATED, "CB00 answer: {pay_raw}");
    let pay_id = pay_json["tran_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the CB00 answer has no tran_id: {pay_raw}"));
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

    let mut tran_ids = BTreeSet::from([pay_id.to_owned()]);
    for (status, json, raw) in &answers {
        assert_eq!(*status, StatusCode::CREATED, "CT02 answer: {raw}");
        let tran_id = json["transaction"]["tran_id"]
            .as_str()
            .unwrap_or_else(|| panic!("the CT02 answer has no tran_id: {raw}"));
        assert_eq!(
            json["message"].as_str(),
            Some(messages::transaction_added_message(tran_id).as_str()),
            "CT02 message: {raw}"
        );
        assert!(
            tran_ids.insert(tran_id.to_owned()),
            "{tran_id} allocated twice"
        );
    }
    let first: u64 = GATE_TRAN_ID.parse().expect("a numeric TRAN-ID");
    let expected: BTreeSet<String> = (first..first + WRITERS)
        .map(|tran_id| format!("{tran_id:016}"))
        .collect();
    assert_eq!(tran_ids, expected, "the TRAN-IDs of the writers");

    let stack = app.stack();
    let rows: Vec<Vec<Option<String>>> = expected
        .iter()
        .map(|tran_id| {
            let amount = if tran_id == pay_id { "194.00" } else { "50.00" };
            vec![Some(tran_id.clone()), Some(amount.to_owned())]
        })
        .collect();
    assert_eq!(
        stack.fetch_text_rows(ADDED_ROWS).await,
        rows,
        "the live rows the writers wrote, by TRAN-ID, with their amounts"
    );
    assert_eq!(
        stack.fetch_text_rows(ACCOUNT_BALANCE).await,
        [[Some("0.00".to_owned())]],
        "the balance of account 1 after the payment"
    );
    let points = ((WRITERS - 1) * 50).to_string();
    assert_eq!(
        stack.fetch_text_rows(REWARD_BALANCE).await,
        [[Some(points.clone()), Some(points)]],
        "total_points_earned and available_points of account 1"
    );
}
