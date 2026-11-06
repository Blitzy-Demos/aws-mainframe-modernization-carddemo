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
//!   backup pages the master in;
//! - `PgStore::read_transaction`, COTRN01C READ-TRANSACT-FILE —
//!   P-COTRN01C-004, for a TRAN-ID holding a NUL byte, which no stored row
//!   can hold;
//! - `PgStore::browse_transactions` with the `account_id` filter the REST
//!   resource adds to COTRN00C (E-17), which reads the account's cards one by
//!   one: every start and direction against the filter of the whole master;
//! - `PgStore::transactions_by_proc_date`, TRANREPT STEP05R —
//!   P-TRANREPT-002, whose raw-column bounds must keep exactly the processing
//!   dates of the `INCLUDE COND`, page by page.
//!
//! Every test starts its own container through `carddemo_testkit::TestStack`,
//! so it needs a running Docker daemon (AAP 0.10.4). The transaction master
//! starts empty, and the records are `CVTRA05Y` records
//! [app/cpy/CVTRA05Y.cpy:L4-18] written through the `TransactionWriter` port,
//! by default on the card the first cross-reference record maps to account 50
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
use carddemo_domain::ports::{Browse, Direction, PortError, TransactionReader, TransactionWriter};
use carddemo_domain::rewards::RewardStore;
use carddemo_domain::rules::transaction_report::select_for_report;
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

/// The record [`record`] gives for `tran_id`, on the card `card` and processed
/// at the `X(26)` bytes `proc_ts`.
fn record_on(tran_id: &str, card: &str, proc_ts: &[u8]) -> NewTransaction {
    NewTransaction {
        tran_card_num: CardNumber::move_from(card),
        tran_proc_ts: Alnum::move_from_bytes(proc_ts),
        ..record(tran_id, 100)
    }
}

/// The cross-reference of the account-filter browse, `(XREF-CARD-NUM,
/// XREF-ACCT-ID)`: account 7 holds two cards, account 8 holds one card that
/// sorts between them and one after them, and account 9 holds the highest card,
/// which has no transactions. Account 10 has no card. [`XREF_ROWS`] inserts
/// exactly these rows.
const XREF: [(&str, u64); 5] = [
    ("7000000000000001", 7),
    ("7000000000000002", 8),
    ("7000000000000003", 7),
    ("7000000000000004", 8),
    ("7000000000000005", 9),
];

/// The `card_xref` rows of [`XREF`]; the customer is the account's number.
const XREF_ROWS: &str = "INSERT INTO card_xref (xref_card_num, xref_cust_id, xref_acct_id) VALUES \
     ('7000000000000001', 7, 7), ('7000000000000002', 8, 8), ('7000000000000003', 7, 7), \
     ('7000000000000004', 8, 8), ('7000000000000005', 9, 9)";

/// The live rows of the account-filter browse, `(TRAN-ID, TRAN-CARD-NUM)`: the
/// TRAN-IDs of every card interleave with those of the others, each card of
/// accounts 7 and 8 holds four, and the last row is on [`CARD`], which no
/// cross-reference row maps to an account.
const BROWSE_ROWS: [(&str, &str); 13] = [
    ("0000000000000001", "7000000000000001"),
    ("0000000000000002", "7000000000000002"),
    ("0000000000000003", "7000000000000003"),
    ("0000000000000004", "7000000000000001"),
    ("0000000000000005", "7000000000000004"),
    ("0000000000000006", "7000000000000003"),
    ("0000000000000007", "7000000000000003"),
    ("0000000000000008", "7000000000000002"),
    ("0000000000000009", "7000000000000001"),
    ("0000000000000010", "7000000000000003"),
    ("0000000000000011", "7000000000000004"),
    ("0000000000000012", "7000000000000001"),
    ("0000000000000013", CARD),
];

/// The page a filter of the whole live master gives: the rows of `live` on a
/// card [`XREF`] maps to `account`, from `start` inclusive in `direction`
/// (from the first or the last key when `None`), in TRAN-ID order, at most
/// `limit` of them.
fn filtered_page(
    live: &[Transaction],
    account: u64,
    direction: Direction,
    start: Option<TranId>,
    limit: u16,
) -> Vec<Transaction> {
    let cards: Vec<CardNumber> = XREF
        .iter()
        .filter(|(_, owner)| *owner == account)
        .map(|(card, _)| CardNumber::move_from(card))
        .collect();
    let mut kept: Vec<Transaction> = live
        .iter()
        .filter(|row| cards.contains(&row.tran_card_num))
        .filter(|row| match (direction, start) {
            (_, None) => true,
            (Direction::Forward, Some(start)) => row.tran_id >= start,
            (Direction::Backward, Some(start)) => row.tran_id <= start,
        })
        .cloned()
        .collect();
    kept.sort_by_key(|row| row.tran_id);
    if direction == Direction::Backward {
        kept.reverse();
    }
    kept.truncate(usize::from(limit));
    kept
}

/// The live rows of the report selection, `(TRAN-ID, TRAN-CARD-NUM,
/// TRAN-PROC-TS)`: processing timestamps of 26, 10 and fewer characters on
/// both sides of the bounds the selection test reads, on three cards whose
/// order differs from the TRAN-ID order.
const PROC_TS_ROWS: [(&str, &str, &[u8]); 12] = [
    (
        "0000000000000001",
        "4000000000000003",
        b"2022-06-30-23.59.59.999999",
    ),
    ("0000000000000002", "4000000000000001", b"2022-06-30"),
    ("0000000000000003", "4000000000000002", b"2022-07"),
    ("0000000000000004", "4000000000000003", b"2022-07-0"),
    ("0000000000000005", "4000000000000001", b"2022-07-01"),
    (
        "0000000000000006",
        "4000000000000002",
        b"2022-07-01-00.00.00.000000",
    ),
    ("0000000000000007", "4000000000000003", b"2022-07-03"),
    ("0000000000000008", "4000000000000001", b"2022-07-06"),
    (
        "0000000000000009",
        "4000000000000002",
        b"2022-07-06-23.59.59.999999",
    ),
    (
        "0000000000000010",
        "4000000000000001",
        b"2022-07-06\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
    ),
    ("0000000000000011", "4000000000000003", b"2022-07-07"),
    (
        "0000000000000012",
        "4000000000000002",
        b"2022-07-07-00.00.00.000000",
    ),
];

/// Every page `transactions_by_proc_date` gives in `uow` for the processing
/// dates `start` to `end`, `limit` rows at a time from the first, each page
/// after the card number and TRAN-ID of the last row of the page before, up
/// to and including the first page shorter than `limit`.
async fn proc_date_pages(
    uow: &mut UnitOfWork,
    (start, end): (Alnum<10>, Alnum<10>),
    limit: u16,
) -> Vec<Vec<Transaction>> {
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let page = uow
            .store()
            .transactions_by_proc_date(start, end, after, limit)
            .await
            .expect("a page of the report selection");
        let next = match page.last() {
            Some(last) if page.len() == usize::from(limit) => {
                Some((last.tran_card_num, last.tran_id))
            }
            _ => None,
        };
        all.push(page);
        match next {
            Some(_) => after = next,
            None => return all,
        }
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

/// COTRN01C READ-TRANSACT-FILE — P-COTRN01C-004: a TRAN-ID holding a NUL
/// byte between other characters, first, after a live TRAN-ID's first 15
/// characters, alone or in every byte names no stored transaction. The read
/// answers `NotFound`, the `NOTFND` of the COBOL `READ`, and fails no
/// statement on the server, so the unit of work still reads the live row
/// afterwards.
#[test]
fn a_tran_id_holding_nul_is_not_found() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        let live = write_live(&db, &[record("0000000000000001", 100)]).await;
        let mut uow = db.begin().await.expect("a unit of work");
        let keys = [
            TranId::move_from("A\0B"),
            TranId::move_from("\0AAAAAAAAAAAAAAA"),
            TranId::move_from("000000000000000\0"),
            TranId::move_from("\0"),
            TranId::from_alnum(Alnum::from_array([0; 16])),
        ];
        for key in keys {
            let read = uow.store().read_transaction(key).await;
            assert_eq!(read, Err(PortError::NotFound), "{key:?}");
        }
        let read = uow.store().read_transaction(live[0].tran_id).await;
        assert_eq!(read, Ok(live[0].clone()), "the live row after the NUL keys");
        uow.rollback()
            .await
            .expect("the read-only unit of work ends");
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

/// COTRN00C STARTBR-TRANSACT-FILE, READNEXT-TRANSACT-FILE,
/// READPREV-TRANSACT-FILE — P-COTRN00C-006, P-COTRN00C-007, P-COTRN00C-011,
/// with the `account_id` filter (E-17): for every account of [`XREF`] and one
/// without a card, every start (from `LOW-VALUES`, from a key of the account
/// or of another, beyond the last key, below the first, and from
/// `HIGH-VALUES`) and every limit below, at and above the four rows of one
/// card and above the eight of the account, the page holds exactly the rows a
/// filter of the whole master gives, the start key included. Each card is read
/// on its own, so the rows of the other account's card between the two cards,
/// of the card after them, and of the card no account holds must neither
/// appear nor repeat.
#[test]
fn the_account_filter_pages_equal_a_filter_of_the_whole_master() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        stack.exec_sql(XREF_ROWS).await;
        let stored = stack
            .fetch_text_rows(
                "SELECT xref_card_num, xref_acct_id::text FROM card_xref ORDER BY xref_card_num",
            )
            .await;
        let xref: Vec<Vec<Option<String>>> = XREF
            .iter()
            .map(|(card, account)| vec![Some((*card).to_owned()), Some(account.to_string())])
            .collect();
        assert_eq!(stored, xref, "card_xref holds exactly XREF");
        let records: Vec<NewTransaction> = BROWSE_ROWS
            .iter()
            .map(|(tran_id, card)| record_on(tran_id, card, TIMESTAMP.as_bytes()))
            .collect();
        let live = write_live(&db, &records).await;

        let starts = [
            (Direction::Forward, None),
            (Direction::Forward, Some("0000000000000005")),
            (Direction::Forward, Some("0000000000000006")),
            (Direction::Forward, Some("0000000000000014")),
            (Direction::Backward, Some("0000000000000009")),
            (Direction::Backward, Some("0000000000000008")),
            (Direction::Backward, Some("0000000000000000")),
            (Direction::Backward, None),
        ];
        let mut uow = db.begin().await.expect("a unit of work");
        for account in [7, 8, 9, 10] {
            let account_id = AccountId::new(account).expect("a 9(11) account number");
            for (direction, start) in starts {
                let start = start.map(TranId::move_from);
                for limit in [1, 3, 4, 5, 9] {
                    let browse = Browse {
                        start_key: start,
                        direction,
                        limit,
                    };
                    let page = uow
                        .store()
                        .browse_transactions(browse, Some(account_id))
                        .await
                        .expect("a page of the filtered browse");
                    assert_eq!(
                        page,
                        filtered_page(&live, account, direction, start, limit),
                        "account {account}, {direction:?} from {start:?}, limit {limit}"
                    );
                }
            }
        }
        uow.rollback()
            .await
            .expect("the read-only unit of work ends");
    });
}

/// TRANREPT STEP05R — P-TRANREPT-002: the pages of
/// `transactions_by_proc_date` hold exactly the records `select_for_report`
/// keeps, in its card-number and TRAN-ID order, for processing timestamps on
/// both sides of every bound: a date equal to the end, the end followed by a
/// time, the end followed by sixteen U+00FF (the highest timestamp of that
/// date), the day after, timestamps of 10 characters and shorter, and a start
/// and an end shorter than 10 characters. The store's own bounds decide, so a
/// row they wrongly kept would show here even though the report rule drops it.
/// The pages are read, as CR00 reads them, in one read-only snapshot, with
/// more than five reads of the one statement at the smallest limit.
#[test]
fn proc_date_pages_keep_exactly_the_dates_of_the_include_cond() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        let records =
            PROC_TS_ROWS.map(|(tran_id, card, proc_ts)| record_on(tran_id, card, proc_ts));
        let live = write_live(&db, &records).await;

        let ranges = [
            ("2022-07-01", "2022-07-06", 6),
            ("2022-07", "2022-07-06", 8),
            ("2022-06-30", "2022-07", 3),
            ("2022-07-06", "2022-07-06", 3),
            ("2022-07-07", "2022-12-31", 2),
        ];
        let mut snapshot = db.begin_snapshot().await.expect("a snapshot unit of work");
        for (start, end, count) in ranges {
            let range = (Alnum::move_from(start), Alnum::move_from(end));
            let expected = select_for_report(&range.0, &range.1, live.clone())
                .expect("every card number is numeric");
            assert_eq!(expected.len(), count, "{start} to {end}: the records kept");
            for limit in [1, 2, 4, 100] {
                let read = proc_date_pages(&mut snapshot, range, limit).await;
                let (last, full) = read.split_last().expect("at least one page");
                assert!(
                    full.iter().all(|page| page.len() == usize::from(limit))
                        && last.len() < usize::from(limit),
                    "{start} to {end}, limit {limit}: full pages, then a short one"
                );
                assert_eq!(read.concat(), expected, "{start} to {end}, limit {limit}");
            }
        }
        snapshot
            .rollback()
            .await
            .expect("the snapshot unit of work ends");
    });
}
