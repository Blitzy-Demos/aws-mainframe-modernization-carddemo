//! The live transaction master as the REPROC backup and the TRANBKP emptying
//! read and hold it, against a migrated `PostgreSQL` 16.15 (E-09).
//!
//! Units under test:
//!
//! - `PgStore::live_master_after`, TRANBKP STEP05R — P-TRANBKP-001,
//!   P-TRANBKP-002 and TRANREPT STEP05R — P-TRANREPT-001: the `REPRO` of
//!   `TRANSACT` into `TRANSACT.BKUP(+1)`, one keyset page at a time, in
//!   TRAN-ID order, live rows only;
//! - `PgStore::exclude_master_writers`, which holds the live master still for
//!   the unit of work TRANBKP backs it up and empties it in (STEP05R to
//!   STEP10, P-TRANBKP-003, P-TRANBKP-005), so that no record leaves the
//!   master without being in its backup;
//! - `Db::begin_snapshot`, the read-only unit on one snapshot the TRANREPT
//!   backup pages the master in.
//!
//! Every test starts its own container through `carddemo_testkit::TestStack`,
//! so it needs a running Docker daemon (AAP 0.10.4). The transaction master
//! starts empty, and the records are `CVTRA05Y` records
//! [app/cpy/CVTRA05Y.cpy:L4-18] written through the `TransactionWriter` port
//! on the card the first cross-reference record maps to account 50
//! [app/data/ASCII/cardxref.txt:L1]. Concurrent units of work run on their own
//! threads through `carddemo_testkit::block_on`, and a wait on a lock is
//! observed in `pg_stat_activity` rather than guessed from elapsed time.

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use carddemo_domain::entities::{NewAccrual, NewRewardRule, NewTransaction, Transaction};
use carddemo_domain::numeric::{
    AccountId, Alnum, CardNumber, MerchantId, Money, RewardPoints, RewardRate, TranCategoryCode,
    TranId, TranTypeCode,
};
use carddemo_domain::ports::{PortError, TransactionWriter};
use carddemo_domain::rewards::RewardStore;
use carddemo_persistence::{Db, PersistenceError, UnitOfWork};
use carddemo_testkit::{TestStack, block_on};
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;

/// `XREF-CARD-NUM` of the first cross-reference record
/// [app/data/ASCII/cardxref.txt:L1].
const CARD: &str = "0500024453765740";

/// `XREF-ACCT-ID` of the same record, account 50.
const ACCOUNT: u64 = 50;

/// `TRAN-ORIG-TS` and `TRAN-PROC-TS` of the written records: the 26
/// characters of `X(26)` [app/cpy/CVTRA05Y.cpy:L16-17].
const TIMESTAMP: &str = "2022-07-01 12:00:00.000000";

/// The page size of the reads that are not about the page size: two, so even
/// a few rows cross a page boundary.
const PAGE: u16 = 2;

/// How long a concurrent unit of work may take to reach or leave a lock wait
/// before the test fails instead of hanging.
const WAIT_LIMIT: Duration = Duration::from_secs(30);

/// The pause between two looks at a concurrent unit of work.
const POLL: Duration = Duration::from_millis(20);

/// 2022-07-01T12:00:00Z, the processing instant of the accrual and the start
/// of its rule.
fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2022, 7, 1, 12, 0, 0)
        .single()
        .expect("a valid UTC instant")
}

/// A `CVTRA05Y` record with the TRAN-ID `tran_id` and an amount of `cents`.
fn record(tran_id: &str, cents: i64) -> NewTransaction {
    NewTransaction {
        tran_id: TranId::move_from(tran_id),
        tran_type_cd: TranTypeCode::move_from("01"),
        tran_cat_cd: TranCategoryCode::new(1).expect("a 9(04) category code"),
        tran_source: Alnum::move_from("POS TERM"),
        tran_desc: Alnum::move_from("Live master test purchase"),
        tran_amt: Money::from_cents(cents).expect("an S9(09)V99 amount"),
        tran_merchant_id: MerchantId::new(800_000_001).expect("a 9(09) merchant ID"),
        tran_merchant_name: Alnum::move_from("Live Master Test Merchant"),
        tran_merchant_city: Alnum::move_from("Seattle"),
        tran_merchant_zip: Alnum::move_from("98101"),
        tran_card_num: CardNumber::move_from(CARD),
        tran_orig_ts: Alnum::move_from(TIMESTAMP),
        tran_proc_ts: Alnum::move_from(TIMESTAMP),
    }
}

/// Writes `records` as live rows in one unit of work, commits it, and returns
/// the stored rows in the order of `records`.
async fn write_live(db: &Db, records: &[NewTransaction]) -> Vec<Transaction> {
    let mut uow = db.begin().await.expect("a unit of work");
    let mut rows = Vec::with_capacity(records.len());
    for rec in records {
        let row_id = uow
            .store()
            .write_transaction(rec.clone())
            .await
            .expect("a live insert");
        rows.push(Transaction::from_new(row_id, rec.clone()));
    }
    uow.commit().await.expect("the live rows commit");
    rows
}

/// Every page `live_master_after` gives in `uow`, `limit` rows at a time,
/// up to and including the first page shorter than `limit`.
async fn pages(uow: &mut UnitOfWork, limit: u16) -> Vec<Vec<Transaction>> {
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let page = uow
            .store()
            .live_master_after(after, limit)
            .await
            .expect("a page of the live master");
        let next = match page.last() {
            Some(last) if page.len() == usize::from(limit) => Some(last.tran_id),
            _ => None,
        };
        all.push(page);
        match next {
            Some(_) => after = next,
            None => return all,
        }
    }
}

/// The pages of the committed live master, read in a unit of work of their
/// own that is rolled back.
async fn committed_pages(db: &Db, limit: u16) -> Vec<Vec<Transaction>> {
    let mut uow = db.begin().await.expect("a unit of work");
    let read = pages(&mut uow, limit).await;
    uow.rollback()
        .await
        .expect("the read-only unit of work ends");
    read
}

/// Writes `rec` as a live row with a reward accrual, then empties the live
/// master in a later unit of work: the row stays in the table, outside the
/// live master, as the target of its accrual.
async fn retire_with_accrual(db: &Db, rec: &NewTransaction) {
    let mut uow = db.begin().await.expect("a unit of work");
    let row_id = uow
        .store()
        .write_transaction(rec.clone())
        .await
        .expect("a live insert");
    let rule = uow
        .store()
        .insert_reward_rule(NewRewardRule {
            transaction_type_cd: rec.tran_type_cd,
            transaction_category_cd: rec.tran_cat_cd,
            reward_rate: RewardRate::from_decimal(Decimal::ONE).expect("a NUMERIC(9,4) rate"),
            min_transaction_amount: None,
            max_transaction_amount: None,
            active_from: at(),
            active_to: None,
        })
        .await
        .expect("a reward rule");
    uow.rewards()
        .insert_accrual(NewAccrual {
            account_id: AccountId::new(ACCOUNT).expect("a 9(11) account number"),
            transaction_id: row_id,
            rule_id: rule.rule_id,
            points_earned: RewardPoints::try_from(42).expect("non-negative points"),
            created_at: at(),
        })
        .await
        .expect("an accrual");
    uow.commit().await.expect("the accrued row commits");

    let mut uow = db.begin().await.expect("a unit of work");
    let emptied = uow
        .store()
        .empty_live_master()
        .await
        .expect("the live master empties");
    uow.commit().await.expect("the emptying commits");
    assert_eq!(emptied, (1, 0), "the accrued row is demoted, not deleted");
}

/// A unit of work that holds the live master still, as `batch tranbkp` opens
/// it.
async fn excluding_unit(db: &Db) -> UnitOfWork {
    let mut uow = db.begin().await.expect("a unit of work");
    uow.store()
        .exclude_master_writers()
        .await
        .expect("the writers are excluded");
    uow
}

/// STEP05R and STEP05 in `uow`: every live row, paged, then the emptying.
/// Returns the rows read and the number of rows the emptying demoted.
async fn back_up_and_empty(uow: &mut UnitOfWork) -> (Vec<Transaction>, u64) {
    let backed_up = pages(uow, PAGE).await.concat();
    let (demoted, _deleted) = uow
        .store()
        .empty_live_master()
        .await
        .expect("the live master empties");
    (backed_up, demoted)
}

/// The row count as the emptying reports it.
fn count(rows: &[Transaction]) -> u64 {
    u64::try_from(rows.len()).expect("a row count fits u64")
}

/// The sessions of the test database waiting on an advisory lock: a TRAN-ID
/// allocation behind the lock `exclude_master_writers` holds.
const ADVISORY_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() \
       AND wait_event_type = 'Lock' AND wait_event = 'advisory'";

/// The sessions of the test database waiting on a table lock: a writer behind
/// the `SHARE ROW EXCLUSIVE` lock, or the exclusion behind a writer.
const RELATION_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() \
       AND wait_event_type = 'Lock' AND wait_event = 'relation'";

/// Waits until `waits`, one of the counts above, finds a waiting session.
///
/// The pause between two looks blocks the calling thread rather than
/// yielding: the test has no timer of its own, and the units of work under
/// observation run on threads of their own.
///
/// # Panics
///
/// When no session waits so within [`WAIT_LIMIT`].
async fn await_lock_wait(stack: &TestStack, waits: &str) {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let rows = stack.fetch_text_rows(waits).await;
        let waiting = rows.first().and_then(|row| row.first()).cloned().flatten();
        if waiting.is_some_and(|sessions| sessions != "0") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no session waited within {WAIT_LIMIT:?}: {waits}"
        );
        thread::sleep(POLL);
    }
}

/// The result of the thread `handle`, once it has finished.
///
/// # Panics
///
/// When the thread does not finish within [`WAIT_LIMIT`], or with the
/// thread's own panic.
fn join_within<T>(handle: JoinHandle<T>, what: &str) -> T {
    let deadline = Instant::now() + WAIT_LIMIT;
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "{what} did not finish within {WAIT_LIMIT:?}"
        );
        thread::sleep(POLL);
    }
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// `live_master_after` gives every live row exactly once, in TRAN-ID byte
/// order, whatever the page size: the pages before the last are full and the
/// last is shorter. A row outside the live master is never read, even under
/// the TRAN-ID of a live row, and an all-space TRAN-ID, stored as the empty
/// text, comes first. An empty live master gives an empty first page.
#[test]
fn pages_hold_every_live_row_once_in_tran_id_order() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        assert_eq!(
            committed_pages(&db, PAGE).await,
            vec![Vec::new()],
            "an empty live master"
        );

        retire_with_accrual(&db, &record("0000000000000001", 4_200)).await;
        let live = write_live(
            &db,
            &[
                record("0000000000000004", 400),
                // The TRAN-ID of the retired row, issued again (E-09).
                record("0000000000000001", 100),
                record("", 50),
                record("0000000000000003", 300),
                record("0000000000000002", 200),
            ],
        )
        .await;
        let expected = [&live[2], &live[1], &live[4], &live[3], &live[0]].map(Clone::clone);

        for limit in [1, 2, 4, 5, 6] {
            let read = committed_pages(&db, limit).await;
            assert_eq!(read.concat(), expected, "limit {limit}");
            let (last, full) = read.split_last().expect("at least one page");
            assert!(
                full.iter().all(|page| page.len() == usize::from(limit)),
                "limit {limit}: every page before the last is full"
            );
            assert!(
                last.len() < usize::from(limit),
                "limit {limit}: the last page is short"
            );
        }
    });
}

/// TRANBKP's unit of work against the writers of the live master, in three
/// steps on one database: a CT02 or CB00 allocation started after the
/// exclusion, an insert started after it, and an insert already in flight.
/// In each, the rows the emptying takes out of the live master are exactly the
/// rows the backup read.
///
/// The concurrent units run on threads of their own; the test's own units
/// stay inside one `block_on`, so a failed assertion drops them inside the
/// runtime, which ends them and releases their locks.
#[test]
fn excluded_writers_never_lose_a_row_to_the_emptying() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        allocation_waits_for_the_emptied_master(&stack, &db).await;
        later_insert_waits_and_stays_live(&stack, &db).await;
        insert_in_flight_is_waited_for_and_backed_up(&stack, &db).await;
    });
}

/// COTRN02C ADD-TRANSACTION — P-COTRN02C-020, P-COTRN02C-021: an allocation
/// that starts while TRANBKP holds the master waits on the advisory lock until
/// the emptied master commits, then finds the `STARTBR` `NOTFND` of an empty
/// file.
async fn allocation_waits_for_the_emptied_master(stack: &TestStack, db: &Db) {
    let live = write_live(
        db,
        &[
            record("0000000000000001", 100),
            record("0000000000000002", 200),
        ],
    )
    .await;
    let mut tranbkp = excluding_unit(db).await;
    let allocation = {
        let db = db.clone();
        thread::spawn(move || {
            block_on(async {
                let mut uow = db.begin().await.expect("a unit of work");
                let next = uow.next_tran_id().await;
                uow.rollback().await.expect("the allocation unit ends");
                next
            })
        })
    };
    await_lock_wait(stack, ADVISORY_WAITS).await;
    assert!(!allocation.is_finished(), "the allocation waits");

    let (backed_up, demoted) = back_up_and_empty(&mut tranbkp).await;
    assert_eq!(backed_up, live);
    assert_eq!(demoted, count(&backed_up));
    tranbkp.commit().await.expect("the emptied master commits");

    let next = join_within(allocation, "the allocation");
    assert!(
        matches!(next, Err(PersistenceError::Port(PortError::NotFound))),
        "the allocation reads the emptied master: {next:?}"
    );
}

/// A CT02 insert of a negative amount, which earns no accrual and so would be
/// deleted by the emptying, starts after TRANBKP took the master: it waits
/// until the emptied master commits, is neither backed up nor demoted, and is
/// the one live row afterwards.
async fn later_insert_waits_and_stays_live(stack: &TestStack, db: &Db) {
    let live = write_live(
        db,
        &[
            record("0000000000000003", 300),
            record("0000000000000001", 100),
            record("0000000000000002", 200),
        ],
    )
    .await;
    let mut tranbkp = excluding_unit(db).await;
    let insert = {
        let db = db.clone();
        thread::spawn(move || block_on(write_live(&db, &[record("0000000000000009", -2_500)])))
    };
    await_lock_wait(stack, RELATION_WAITS).await;
    assert!(!insert.is_finished(), "the insert waits");

    let (backed_up, demoted) = back_up_and_empty(&mut tranbkp).await;
    assert_eq!(backed_up, [&live[1], &live[2], &live[0]].map(Clone::clone));
    assert_eq!(demoted, count(&backed_up));
    tranbkp.commit().await.expect("the emptied master commits");

    let inserted = join_within(insert, "the insert");
    assert_eq!(committed_pages(db, PAGE).await.concat(), inserted);
}

/// A POSTTRAN record whose insert is still uncommitted when TRANBKP starts:
/// the exclusion waits for it to commit, and the backup then holds it.
async fn insert_in_flight_is_waited_for_and_backed_up(stack: &TestStack, db: &Db) {
    let before = committed_pages(db, PAGE).await.concat();
    let posting_record = record("0000000000000010", 1_000);
    let mut posting = db.begin().await.expect("a unit of work");
    let row_id = posting
        .store()
        .write_transaction(posting_record.clone())
        .await
        .expect("an uncommitted live insert");
    let tranbkp = {
        let db = db.clone();
        thread::spawn(move || block_on(excluding_unit(&db)))
    };
    await_lock_wait(stack, RELATION_WAITS).await;
    assert!(!tranbkp.is_finished(), "the exclusion waits for the insert");
    posting.commit().await.expect("the posting commits");

    let mut tranbkp = join_within(tranbkp, "the exclusion");
    let (backed_up, demoted) = back_up_and_empty(&mut tranbkp).await;
    let posted = Transaction::from_new(row_id, posting_record);
    assert_eq!(backed_up, [before, vec![posted]].concat());
    assert_eq!(demoted, count(&backed_up));
    tranbkp.commit().await.expect("the emptied master commits");
    assert_eq!(committed_pages(db, PAGE).await, vec![Vec::new()]);
}

/// `Db::begin_snapshot` reads one snapshot: a row another unit commits after
/// the first page is not on the later pages, as it would not be in a single
/// read of the whole file. The unit writes nothing: an insert in it fails with
/// SQLSTATE 25006, `read_only_sql_transaction`.
#[test]
fn snapshot_pages_read_one_committed_state() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        let before = write_live(
            &db,
            &[
                record("0000000000000001", 100),
                record("0000000000000002", 200),
                record("0000000000000003", 300),
            ],
        )
        .await;

        let mut snapshot = db.begin_snapshot().await.expect("a snapshot unit of work");
        let first = snapshot
            .store()
            .live_master_after(None, PAGE)
            .await
            .expect("the first page");
        assert_eq!(first, before[..2]);
        let later = write_live(&db, &[record("0000000000000004", 400)]).await;
        let rest = snapshot
            .store()
            .live_master_after(Some(first[1].tran_id), PAGE)
            .await
            .expect("the next page");
        assert_eq!(rest, before[2..], "the row committed after the first page");

        let refused = snapshot
            .store()
            .write_transaction(record("0000000000000005", 500))
            .await;
        assert!(
            matches!(&refused, Err(PortError::Unavailable(reason)) if reason.starts_with("sqlstate 25006")),
            "{refused:?}"
        );
        snapshot
            .rollback()
            .await
            .expect("the snapshot unit of work ends");
        assert_eq!(
            committed_pages(&db, PAGE).await.concat(),
            [before, later].concat()
        );
    });
}
