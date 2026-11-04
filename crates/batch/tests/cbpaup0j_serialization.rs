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
//!
//! A CP00 write and a running purge give a serial result too: `COPAUA0C` runs under the same
//! PSB `PSBPAUTB` with `PROCOPT=AP` [app/app-authorization-ims-db2-mq/ims/PSBPAUTB.psb:L17]. A
//! second holder does what CP00's `8000-WRITE-AUTH-TO-DB` unit of work does: it reads a summary
//! under its lock (`read_summary_for_update`), replaces it with one more approval and inserts
//! that approval's unexpired detail under it, without committing (P-COPAUA0C-008,
//! P-COPAUA0C-013, P-COPAUA0C-014). The run's sweep reads `FOR UPDATE`, so it waits on that row
//! lock in its summary read, as `pg_stat_activity` shows, and then purges the summary as CP00
//! committed it: its in-memory count stays above zero, so the summary is kept with its stored
//! count (E-35) and the new detail, and the counts the run reports are the rows it deleted.
//! Without the lock the sweep judges the summary on the count before the write, deletes it, and
//! the cascade takes the committed detail with it, uncounted (P-CBPAUP0C-004, P-CBPAUP0C-006).

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

/// The summaries the CP00 case commits, as [`AUTHORIZATIONS`] gives them, under the clock's day
/// 22182:
///
/// - Account 1: two expired approvals under an approved count of 2. CP00's write unit raises
///   the count to 3 and inserts an approval dated [`CP00_DAY`]. A sweep of the summary as it
///   stood before that write counts 2 - 2 = 0 and deletes it, the new detail with it
///   (P-CBPAUP0C-006); a sweep of the summary CP00 committed counts 3 - 2 = 1 and keeps it with
///   its stored count 3 (E-35) and the new detail.
/// - Account 2: one expired approval under an approved count of 1. The run deletes the detail,
///   then the summary (P-CBPAUP0C-005, P-CBPAUP0C-007).
const CP00_AUTHORIZATIONS: [(u64, i16, &[i32]); 2] = [(1, 2, &[22_180, 22_181]), (2, 1, &[22_181])];

/// The account whose summary CP00's write unit holds.
const CP00_ACCOUNT: u64 = 1;

/// The `YYDDD` of the approval CP00 writes: the day after the clock's, which never expires at
/// expiry 0 (P-CBPAUP0C-004).
const CP00_DAY: i32 = 22_183;

/// The sessions of the test database waiting on an advisory lock: the run behind the holder's
/// purge exclusion.
const ADVISORY_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() \
       AND wait_event_type = 'Lock' AND wait_event = 'advisory'";

/// The sessions of the test database whose locking summary read waits on a lock: the sweep's
/// `GN`, `SELECT … FOR UPDATE` of `pending_auth_summaries`, behind the row lock of CP00's write
/// unit. The session asking is left out, since its own statement names the same words.
const SWEEP_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() AND pid <> pg_backend_pid() \
       AND wait_event_type = 'Lock' \
       AND query LIKE '%FROM pending_auth_summaries%FOR UPDATE%'";

/// The rows of both pending-authorization tables, summaries and details together.
const ROWS_SQL: &str = "SELECT (SELECT count(*) FROM pending_auth_summaries) \
     + (SELECT count(*) FROM pending_auth_details)";

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

/// Commits `authorizations`, each as its account, approved count and detail dates, through the
/// `AuthorizationStore` port, each summary before its details as CP00 writes them
/// (P-COPAUA0C-013, P-COPAUA0C-014), and returns the identities of the `held_account` details.
async fn commit_authorizations(
    db: &Db,
    authorizations: &[(u64, i16, &[i32])],
    held_account: u64,
) -> Vec<AuthId> {
    let mut uow = db.begin().await.expect("a unit of work");
    let mut held = Vec::new();
    for &(account_id, approved, dates) in authorizations {
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
            if account_id == held_account {
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

/// CP00's `8000-WRITE-AUTH-TO-DB` unit of work as the run under test meets it: it has read the
/// [`CP00_ACCOUNT`] summary under its lock, replaced it with one more approval of the new
/// detail's amount and inserted that detail, dated [`CP00_DAY`], without committing
/// (P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-014). It holds the summary's row lock.
async fn holding_cp00_write(db: &Db) -> UnitOfWork {
    let mut uow = db.begin().await.expect("a unit of work");
    let mut held = uow
        .store()
        .read_summary_for_update(account(CP00_ACCOUNT))
        .await
        .expect("CP00 reads the summary under its lock");
    let approval = detail(CP00_ACCOUNT, CP00_DAY);
    held.pa_approved_auth_cnt = held
        .pa_approved_auth_cnt
        .checked_increment()
        .expect("a count below 9999");
    held.pa_approved_auth_amt = (held.pa_approved_auth_amt + approval.pa_approved_amt).store::<9>();
    held.pa_credit_balance = (held.pa_credit_balance + approval.pa_approved_amt).store::<9>();
    held.pa_cash_balance = Money::ZERO;
    uow.store()
        .update_summary(held)
        .await
        .expect("CP00 replaces the summary");
    uow.store()
        .insert_detail(approval)
        .await
        .expect("CP00 inserts the detail");
    uow
}

/// Whether a session of the test database is one `waits` counts within [`WAIT_LIMIT`].
///
/// The pause between two looks blocks the calling thread: the run under observation is a
/// child process driven from a thread of its own.
fn lock_wait_seen(stack: &TestStack, waits: &str) -> bool {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let rows = block_on(stack.fetch_text_rows(waits));
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
    let held = block_on(commit_authorizations(&db, &AUTHORIZATIONS, HELD_ACCOUNT));
    assert_eq!(held.len(), 2, "account 1 has two details");
    let holder = block_on(holding_purge(&db, &held));

    let (waited, finished_while_held, output) = thread::scope(|scope| {
        let run = scope.spawn(|| h.run(&CBPAUP0J, Some(TRANREPT_CLOCK)));
        let waited = lock_wait_seen(h.stack(), ADVISORY_WAITS);
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

#[test]
fn a_cp00_write_held_during_the_sweep_is_purged_as_cp00_committed_it() {
    // P-CBPAUP0C-002 to P-CBPAUP0C-007, E-35; P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-014.
    let h = Harness::new();
    let db = h.db();
    let held = block_on(commit_authorizations(
        &db,
        &CP00_AUTHORIZATIONS,
        CP00_ACCOUNT,
    ));
    assert_eq!(held.len(), 2, "account 1 has two expired details");
    let holder = block_on(holding_cp00_write(&db));

    let (waited, finished_while_held, output) = thread::scope(|scope| {
        let run = scope.spawn(|| h.run(&CBPAUP0J, Some(TRANREPT_CLOCK)));
        let waited = lock_wait_seen(h.stack(), SWEEP_WAITS);
        let finished_while_held = run.is_finished();
        // The holder ends before any assertion, so a failed one never leaves the run waiting
        // on a lock the test still holds.
        block_on(holder.commit()).unwrap_or_else(|error| panic!("the CP00 commit: {error}"));
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
        "the run behind CP00's write exits 0\n--- stderr ---\n{stderr:#?}\n--- stdout ---\n{stdout:#?}"
    );
    assert!(
        stderr.is_empty(),
        "a normal end writes no stderr: {stderr:#?}"
    );
    assert!(
        waited,
        "the sweep's summary read never waited on CP00's row lock within {WAIT_LIMIT:?}"
    );
    assert!(
        !finished_while_held,
        "the run ended while CP00's write unit still held the summary"
    );

    // The counts and rows of a purge of the state CP00 committed: account 1 with count 3, its
    // two expired details deleted, kept (3 - 2 > 0) with CP00's detail; account 2's detail and
    // summary deleted.
    assert!(
        stdout.iter().any(|line| {
            line.contains(r#""message":"expired pending authorizations purged""#)
                && line.contains(r#""summaries_read":2"#)
                && line.contains(r#""summaries_deleted":1"#)
                && line.contains(r#""details_read":4"#)
                && line.contains(r#""details_deleted":3"#)
        }),
        "the end-of-job counts are not those of the state CP00 committed: {stdout:#?}"
    );
    assert_eq!(
        text_rows(&h, SUMMARIES_SQL),
        [["1", "3"]],
        "summary 1 stays with the count CP00 stored (E-35); summary 2 is deleted"
    );
    assert_eq!(
        text_rows(&h, DETAILS_SQL),
        [["1", "77816"]],
        "only CP00's unexpired detail remains"
    );
    // Every row the run removed is one it counted: the summaries and details CP00's write left
    // committed, less those remaining, are the one summary and three details deleted.
    let committed: usize = CP00_AUTHORIZATIONS
        .iter()
        .map(|&(_, _, dates)| 1 + dates.len())
        .sum::<usize>()
        + 1;
    let remaining: usize = text_rows(&h, ROWS_SQL)[0][0].parse().expect("a row count");
    assert_eq!(
        committed - remaining,
        1 + 3,
        "rows removed beyond the counted deletes"
    );
}
