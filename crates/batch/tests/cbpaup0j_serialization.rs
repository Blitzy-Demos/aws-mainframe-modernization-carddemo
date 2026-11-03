//! Two `batch cbpaup0j` runs purge one after the other: a run started while another purge
//! holds its unit of work waits for it, then sweeps the rows it left and ends normally.
//!
//! `CBPAUP0J` runs `CBPAUP0C` as an IMS BMP
//! [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L24-25], and IMS program isolation never
//! hands it a segment another program has deleted. The replacement keeps two purges apart with
//! the advisory lock of `PgStore::exclude_other_purges`, which the job takes before its sweep.
//!
//! A holder unit of work stands in for the first purge: it takes the same exclusion and deletes
//! expired details without committing them. The binary, started on a thread of its own, has to
//! wait on the advisory lock, as `pg_stat_activity` shows rather than elapsed time, and is still
//! running when the holder commits. It then exits 0, and its counts and the rows it leaves are
//! those of a purge of the state the holder left (P-CBPAUP0C-002 to P-CBPAUP0C-007, E-16,
//! E-35). Without the exclusion the run reads the held details, waits on their row locks, and
//! abends with `CBPAUP0C ABENDING ...` on a `DLET` that finds no row once the holder commits
//! (P-CBPAUP0C-005, P-CBPAUP0C-009).

pub mod common;

use std::thread;
use std::time::{Duration, Instant};

use carddemo_domain::entities::{
    FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary,
};
use carddemo_domain::messages::CBPAUP0C_ABENDING;
use carddemo_domain::numeric::{
    AccountId, Alnum, AuthCount, AuthId, CardNumber, CustomerId, Money, NumericCode,
};
use carddemo_domain::ports::AuthorizationStore;
use carddemo_persistence::{Db, UnitOfWork};
use carddemo_testkit::TestStack;
use common::{Harness, TRANREPT_CLOCK, block_on};

/// The CBPAUP0J command of the JCL `SYSIN` [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl]:
/// expiry 0 days, so every detail dated on or before the clock's day 22182 expires
/// (P-CBPAUP0C-001, P-CBPAUP0C-004).
const CBPAUP0J: [&str; 3] = ["cbpaup0j", "--sysin", "00,00001,00001,Y"];

/// The summaries the test commits, each as its account, its approved count and the `YYDDD`
/// dates of its approved details, all under the clock's day 22182:
///
/// - Account 1: two expired approvals under an approved count of 2. The holder deletes both,
///   so the run finds the summary without details, its in-memory count stays 2 and the summary
///   stays (E-35). A purge of the state before the holder would delete it (E-16).
/// - Account 2: one expired approval under an approved count of 1. The run deletes the detail,
///   then the summary (P-CBPAUP0C-005, P-CBPAUP0C-007).
/// - Account 3: one approval dated the day after the clock's, which never expires at expiry 0
///   (P-CBPAUP0C-004), so it and its summary stay.
const AUTHORIZATIONS: [(u64, i16, &[i32]); 3] = [
    (1, 2, &[22_180, 22_181]),
    (2, 1, &[22_181]),
    (3, 1, &[22_183]),
];

/// The account whose details the holder deletes.
const HELD_ACCOUNT: u64 = 1;

/// The sessions of the test database waiting on an advisory lock: the run behind the holder's
/// purge exclusion.
const ADVISORY_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() \
       AND wait_event_type = 'Lock' AND wait_event = 'advisory'";

/// Every summary's account and stored approved count, in account order.
const SUMMARIES_SQL: &str =
    "SELECT pa_acct_id, pa_approved_auth_cnt FROM pending_auth_summaries ORDER BY pa_acct_id";

/// Every detail's account and complemented date, in account and twin order.
const DETAILS_SQL: &str = "SELECT pa_acct_id, pa_auth_date_9c FROM pending_auth_details \
     ORDER BY pa_acct_id, pa_auth_date_9c, pa_auth_time_9c";

/// How long the run may take to reach the lock wait before the test fails.
const WAIT_LIMIT: Duration = Duration::from_secs(30);

/// The pause between two looks at `pg_stat_activity`.
const POLL: Duration = Duration::from_millis(20);

fn account(value: u64) -> AccountId {
    AccountId::new(value).expect("a 9(11) account number")
}

fn cents<const D: u8>(value: i64) -> Money<D> {
    Money::from_cents(value).expect("an amount within the field")
}

/// A summary of `account_id` whose approved count is `approved` and declined count 0.
fn summary(account_id: u64, approved: i16) -> PendingAuthSummary {
    PendingAuthSummary {
        pa_acct_id: account(account_id),
        pa_cust_id: CustomerId::new(1).expect("a customer number"),
        pa_auth_status: Alnum::move_from("A"),
        pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
        pa_credit_limit: cents(2_020_000),
        pa_cash_limit: cents(1_020_000),
        pa_credit_balance: cents(10_000),
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: AuthCount::new(approved).expect("a count of at most four digits"),
        pa_declined_auth_cnt: AuthCount::new(0).expect("a count of at most four digits"),
        pa_approved_auth_amt: cents(10_000),
        pa_declined_auth_amt: Money::ZERO,
    }
}

/// An approved detail (`PA-AUTH-RESP-CODE` `00`) of `account_id` dated `yyddd`, keyed by the
/// complemented date `99999 - YYDDD` that `COPAUA0C` stores and `CBPAUP0C` complements back
/// (P-COPAUA0C-014, P-CBPAUP0C-004).
fn detail(account_id: u64, yyddd: i32) -> NewPendingAuthDetail {
    NewPendingAuthDetail {
        pa_acct_id: account(account_id),
        pa_auth_date_9c: 99_999 - yyddd,
        pa_auth_time_9c: 879_999_999,
        pa_auth_orig_date: Alnum::move_from("220701"),
        pa_auth_orig_time: Alnum::move_from("120000"),
        pa_card_num: CardNumber::move_from("4111000000000001"),
        pa_auth_type: Alnum::move_from("SALE"),
        pa_card_expiry_date: Alnum::move_from("0525"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("120000"),
        pa_auth_resp_code: Alnum::move_from("00"),
        pa_auth_resp_reason: Alnum::move_from("0000"),
        pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
        pa_transaction_amt: cents(4_000),
        pa_approved_amt: cents(3_000),
        pa_merchant_catagory_code: Alnum::move_from("5411"),
        pa_acqr_country_code: Alnum::move_from("840"),
        pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
        pa_merchant_id: Alnum::move_from("800000000"),
        pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
        pa_merchant_city: Alnum::move_from("Enoshaven"),
        pa_merchant_state: Alnum::move_from("NC"),
        pa_merchant_zip: Alnum::move_from("72112"),
        pa_transaction_id: Alnum::move_from("000000000000001"),
        pa_match_status: MatchStatus::Pending,
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    }
}

/// Commits [`AUTHORIZATIONS`] through the `AuthorizationStore` port, each summary before its
/// details as CP00 writes them (P-COPAUA0C-013, P-COPAUA0C-014), and returns the identities of
/// the [`HELD_ACCOUNT`] details.
async fn commit_authorizations(db: &Db) -> Vec<AuthId> {
    let mut uow = db.begin().await.expect("a unit of work");
    let mut held = Vec::new();
    for (account_id, approved, dates) in AUTHORIZATIONS {
        uow.store()
            .insert_summary(summary(account_id, approved))
            .await
            .expect("the summary is inserted");
        for &yyddd in dates {
            let auth_id = uow
                .store()
                .insert_detail(detail(account_id, yyddd))
                .await
                .expect("the detail is inserted");
            if account_id == HELD_ACCOUNT {
                held.push(auth_id);
            }
        }
    }
    uow.commit().await.expect("the authorizations commit");
    held
}

/// The first purge as the run under test meets it: a unit of work that holds the purge
/// exclusion and has deleted the `held` details without committing, so it also holds their row
/// locks (P-CBPAUP0C-005).
async fn holding_purge(db: &Db, held: &[AuthId]) -> UnitOfWork {
    let mut uow = db.begin().await.expect("a unit of work");
    uow.store()
        .exclude_other_purges()
        .await
        .expect("the holder excludes other purges");
    for &auth_id in held {
        uow.store()
            .delete_detail(auth_id)
            .await
            .expect("the holder deletes an expired detail");
    }
    uow
}

/// Whether a session of the test database waits on an advisory lock within [`WAIT_LIMIT`].
///
/// The pause between two looks blocks the calling thread: the run under observation is a
/// child process driven from a thread of its own.
fn advisory_wait_seen(stack: &TestStack) -> bool {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let rows = block_on(stack.fetch_text_rows(ADVISORY_WAITS));
        let waiting = rows.first().and_then(|row| row.first()).cloned().flatten();
        if waiting.is_some_and(|sessions| sessions != "0") {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// The text of every row of `sql`, a null as `NULL`.
fn text_rows(h: &Harness, sql: &str) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| "NULL".to_owned()))
                .collect()
        })
        .collect()
}

/// The lines of a child's output stream.
fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_run_started_during_another_purge_waits_and_then_ends_normally() {
    // P-CBPAUP0C-002 to P-CBPAUP0C-007, P-CBPAUP0C-009, E-16, E-35.
    let h = Harness::new();
    let db = h.db();
    let held = block_on(commit_authorizations(&db));
    assert_eq!(held.len(), 2, "account 1 has two details");
    let holder = block_on(holding_purge(&db, &held));

    let (waited, finished_while_held, output) = thread::scope(|scope| {
        let run = scope.spawn(|| h.run(&CBPAUP0J, Some(TRANREPT_CLOCK)));
        let waited = advisory_wait_seen(h.stack());
        let finished_while_held = run.is_finished();
        // The holder ends before any assertion, so a failed one never leaves the run waiting
        // on a lock the test still holds.
        block_on(holder.commit()).unwrap_or_else(|error| panic!("the holder commit: {error}"));
        let output = run
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (waited, finished_while_held, output)
    });

    let stderr = lines(&output.stderr);
    let stdout = lines(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the run behind the holder exits 0\n--- stderr ---\n{stderr:#?}\n--- stdout ---\n{stdout:#?}"
    );
    assert!(
        !stderr.iter().any(|line| line == CBPAUP0C_ABENDING),
        "the run abended: {stderr:#?}"
    );
    assert!(
        stderr.is_empty(),
        "a normal end writes no stderr: {stderr:#?}"
    );
    assert!(
        waited,
        "the run never waited on the purge's advisory lock within {WAIT_LIMIT:?}"
    );
    assert!(
        !finished_while_held,
        "the run ended while the holder still held the purge exclusion"
    );

    // The counts and rows of a purge of the state the holder left: account 1 without details
    // and kept, account 2's detail and summary deleted, account 3 untouched.
    assert!(
        stdout.iter().any(|line| {
            line.contains(r#""message":"expired pending authorizations purged""#)
                && line.contains(r#""summaries_read":3"#)
                && line.contains(r#""summaries_deleted":1"#)
                && line.contains(r#""details_read":2"#)
                && line.contains(r#""details_deleted":1"#)
        }),
        "the end-of-job counts are not those of the state the holder left: {stdout:#?}"
    );
    assert_eq!(
        text_rows(&h, SUMMARIES_SQL),
        [["1", "2"], ["3", "1"]],
        "summary 1 stays with its stored count (E-35); summary 2 is deleted"
    );
    assert_eq!(
        text_rows(&h, DETAILS_SQL),
        [["3", "77816"]],
        "only account 3's unexpired detail remains"
    );
}
