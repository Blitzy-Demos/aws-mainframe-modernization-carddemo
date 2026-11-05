//! The reward persistence against a migrated, seeded `PostgreSQL` 16.15
//! (request Rewards system; AAP 0.3.5, 0.10.1 o8).
//!
//! Units under test:
//!
//! - `PgRewardStore`, the adapter of the `RewardStore` port: the candidate-rule
//!   read, the accrual insert and the balance upsert, each write under its own
//!   savepoint;
//! - `UnitOfWork`, which commits or rolls back the store's writes together
//!   with everything else the unit holds;
//! - the inherent reward methods of `PgStore` (`repos::rewards`): the rule
//!   administration, the balance read and the reward history page;
//! - `RewardEngine::accrue` over the real store, the one reward calculation
//!   (AAP 0.6.4), against the unbounded seed rules and against inserted rules
//!   with both, one or no amount bounds, which reach the engine through the
//!   candidate read and its row conversion: amounts at each inclusive bound,
//!   one cent beyond it, and a higher-rate rule its range excludes. No test
//!   selects a rule or computes points itself: the expected values are
//!   constants.
//!
//! The invariant `available_points = total_points_earned -
//! total_points_redeemed` is checked on every balance the store returns, on the
//! stored rows, and against the table's own `CHECK`.
//!
//! Every test starts its own container through `carddemo_testkit::TestStack`,
//! so it needs a running Docker daemon (AAP 0.10.4), and loads it with
//! `seed::prepare` and `seed::run` in `SeedMode::Full`, the preparation on the
//! test thread before the async phase: the 50 fixture accounts, an empty
//! transaction master (E-07) and the two seed rules of AAP 0.3.6. Rule 1 pairs
//! type `01` Purchase [app/data/ASCII/trantype.txt:L1] with category `0001`
//! Regular Sales Draft [app/data/ASCII/trancatg.txt:L1] at rate 1.0000, rule 2
//! type `04` Authorization [app/data/ASCII/trantype.txt:L4] with category
//! `0003` Travel booking authorization [app/data/ASCII/trancatg.txt:L14] at
//! 3.0000; both are active from 2022-01-01T00:00:00Z with no end.
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a roadmap
//! item [README.md:L357-369], so no parity audit row applies. The transactions
//! an accrual references are `CVTRA05Y` records [app/cpy/CVTRA05Y.cpy:L4-18]
//! written through the `TransactionWriter` port on the card the
//! cross-reference [app/data/ASCII/cardxref.txt] gives the fixture account
//! [app/data/ASCII/acctdata.txt], because `reward_accruals.transaction_id` is a
//! unique foreign key to `transactions`.
//!
//! A concurrent unit of work runs on a thread of its own through
//! `carddemo_testkit::block_on`, and its wait on a row lock is observed in
//! `pg_stat_activity` rather than guessed from elapsed time.

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use carddemo_domain::entities::{
    NewAccrual, NewRewardRule, NewTransaction, RewardAccrual, RewardBalance, RewardRule,
    Transaction,
};
use carddemo_domain::numeric::{
    AccountId, AccrualId, Alnum, CardNumber, MerchantId, Money, RewardPoints, RewardRate, RuleId,
    TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
};
use carddemo_domain::ports::{PortError, TransactionWriter};
use carddemo_domain::rewards::{AccrualInput, RewardEngine, RewardError, RewardStore};
use carddemo_persistence::repos::SoftDeleteOutcome;
use carddemo_persistence::seed::{self, PreparedSeed, SeedMode, SeedSources};
use carddemo_persistence::{Db, PersistenceError, PgRewardStore, UnitOfWork};
use carddemo_testkit::{TestStack, block_on, fixtures};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use rust_decimal::Decimal;

/// A points total above 2^53 − 1, the largest integer a JSON number carries
/// exactly in the browser: the API sends points as digit strings so that this
/// value arrives unchanged (AAP 0.5.3.1).
const BEYOND_SAFE_INTEGER: i64 = 9_007_199_254_740_993;

/// `TRAN-ORIG-TS` and `TRAN-PROC-TS` of the written records: 26 characters,
/// the `X(26)` of both fields [app/cpy/CVTRA05Y.cpy:L16-17].
const TIMESTAMP: &str = "2022-07-01 12:00:00.000000";

/// The sessions of the test database waiting on a row lock, which a session
/// waits for as the transaction ID of the lock's holder.
const ROW_LOCK_WAITS: &str = "SELECT count(*) FROM pg_stat_activity \
     WHERE datname = current_database() \
       AND wait_event_type = 'Lock' AND wait_event = 'transactionid'";

/// How long a concurrent unit of work may take to reach or leave a lock wait
/// before the test fails instead of hanging.
const WAIT_LIMIT: Duration = Duration::from_secs(30);

/// The pause between two looks at a concurrent unit of work.
const POLL: Duration = Duration::from_millis(20);

/// The UTC instant at `hour`:00:00 on the given day.
fn instant(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
        .single()
        .expect("a valid UTC instant")
}

/// The processing instant of every accrual and soft delete here,
/// 2022-07-01T12:00:00Z: inside the window of both seed rules.
fn at() -> DateTime<Utc> {
    instant(2022, 7, 1, 12)
}

/// `active_from` of both seed rules, 2022-01-01T00:00:00Z (AAP 0.3.6).
fn seed_start() -> DateTime<Utc> {
    instant(2022, 1, 1, 0)
}

fn account(value: u64) -> AccountId {
    AccountId::new(value).expect("a 9(11) account number")
}

fn category(code: u16) -> TranCategoryCode {
    TranCategoryCode::new(code).expect("a 9(04) category code")
}

fn points(value: i64) -> RewardPoints {
    RewardPoints::try_from(value).expect("non-negative points")
}

fn amount(cents: i64) -> Money<9> {
    Money::from_cents(cents).expect("an S9(09)V99 amount")
}

/// A reward rate in units of 0.0001 points per currency unit.
fn rate(units: i64) -> RewardRate {
    RewardRate::from_decimal(Decimal::new(units, 4)).expect("a NUMERIC(9,4) reward rate")
}

/// An unbounded rule for the pair at `rate_units`, active from `active_from`
/// until `active_to`.
fn new_rule(
    type_cd: &str,
    category_cd: u16,
    rate_units: i64,
    active_from: DateTime<Utc>,
    active_to: Option<DateTime<Utc>>,
) -> NewRewardRule {
    NewRewardRule {
        transaction_type_cd: TranTypeCode::move_from(type_cd),
        transaction_category_cd: category(category_cd),
        reward_rate: rate(rate_units),
        min_transaction_amount: None,
        max_transaction_amount: None,
        active_from,
        active_to,
    }
}

/// Seed rule 1, retail purchase at 1.0000 (AAP 0.3.6), as `batch seed` stores it.
fn retail_seed_rule() -> RewardRule {
    RewardRule::from_new(
        RuleId::from(1),
        &new_rule("01", 1, 10_000, seed_start(), None),
    )
}

/// Seed rule 2, travel at 3.0000 (AAP 0.3.6), as `batch seed` stores it.
fn travel_seed_rule() -> RewardRule {
    RewardRule::from_new(
        RuleId::from(2),
        &new_rule("04", 3, 30_000, seed_start(), None),
    )
}

/// The fixture seed of the repository's sources, prepared on the calling
/// thread: each test calls it before [`block_on`], outside every runtime, as
/// an adapter prepares the seed on its blocking pool, so the argon2id hashing
/// never runs inside the async phase.
fn fixture_seed() -> PreparedSeed {
    seed::prepare(
        SeedMode::Full,
        &SeedSources::with_defaults(&fixtures::repo_root()),
    )
}

/// A fresh migrated database in the seeded state `prepared` writes, and the
/// pool on it. The stack must outlive the pool's use: dropping it removes the
/// container.
async fn seeded(prepared: PreparedSeed) -> (TestStack, Db) {
    let stack = TestStack::postgres().await;
    let db = Db::from_pool(stack.pool().clone());
    let report = seed::run(&db, prepared)
        .await
        .expect("the fixture seed loads");
    assert_eq!(
        (report.accounts, report.reward_rules),
        (50, 2),
        "the seeded state of AAP 0.3.6"
    );
    (stack, db)
}

/// The card the cross-reference maps to `account_id`.
async fn card_of(db: &Db, account_id: AccountId) -> CardNumber {
    let card = sqlx::query!(
        "SELECT xref_card_num FROM card_xref WHERE xref_acct_id = $1 \
         ORDER BY xref_card_num LIMIT 1",
        account_id.to_i64()
    )
    .fetch_one(db.pool())
    .await
    .expect("every fixture account has a card in the cross-reference")
    .xref_card_num;
    CardNumber::from_trimmed(&card).expect("a stored X(16) card number")
}

/// The next TRAN-ID of the unit's live master. The seed leaves the master
/// empty (E-07), which `next_tran_id` reports as the `STARTBR` `NOTFND` of
/// `COTRN02C`; the first record then takes the key that follows `MOVE ZEROS`,
/// `0000000000000001`.
async fn next_key(uow: &mut UnitOfWork) -> TranId {
    match uow.next_tran_id().await {
        Ok(tran_id) => tran_id,
        Err(PersistenceError::Port(PortError::NotFound)) => {
            TranId::zeros().next().expect("sixteen zeros are numeric")
        }
        Err(other) => panic!("cannot allocate a TRAN-ID: {other}"),
    }
}

/// A `CVTRA05Y` record [app/cpy/CVTRA05Y.cpy:L4-18] of `cents` on `card`.
fn new_transaction(
    tran_id: TranId,
    card: CardNumber,
    type_cd: &str,
    category_cd: u16,
    cents: i64,
) -> NewTransaction {
    NewTransaction {
        tran_id,
        tran_type_cd: TranTypeCode::move_from(type_cd),
        tran_cat_cd: category(category_cd),
        tran_source: Alnum::move_from("POS TERM"),
        tran_desc: Alnum::move_from("Reward store test purchase"),
        tran_amt: amount(cents),
        tran_merchant_id: MerchantId::new(800_000_001).expect("a 9(09) merchant ID"),
        tran_merchant_name: Alnum::move_from("Rewards Test Merchant"),
        tran_merchant_city: Alnum::move_from("Seattle"),
        tran_merchant_zip: Alnum::move_from("98101"),
        tran_card_num: card,
        tran_orig_ts: Alnum::move_from(TIMESTAMP),
        tran_proc_ts: Alnum::move_from(TIMESTAMP),
    }
}

/// Writes and commits one live transaction of `cents` on the card of
/// `account_id` and returns the stored row. Each accrual needs a transaction
/// of its own, because `reward_accruals.transaction_id` is unique.
async fn live_transaction(
    db: &Db,
    account_id: AccountId,
    type_cd: &str,
    category_cd: u16,
    cents: i64,
) -> Transaction {
    let card = card_of(db, account_id).await;
    let mut uow = db.begin().await.expect("a unit of work");
    let tran_id = next_key(&mut uow).await;
    let record = new_transaction(tran_id, card, type_cd, category_cd, cents);
    let row_id = uow
        .store()
        .write_transaction(record.clone())
        .await
        .expect("a live insert");
    uow.commit().await.expect("the transaction commits");
    Transaction::from_new(row_id, record)
}

/// An accrual of `earned` points under `rule_id` for the row `tran`.
fn accrual(
    tran: &Transaction,
    account_id: AccountId,
    rule_id: i64,
    earned: i64,
    created_at: DateTime<Utc>,
) -> NewAccrual {
    NewAccrual {
        account_id,
        transaction_id: tran.transaction_id,
        rule_id: RuleId::from(rule_id),
        points_earned: points(earned),
        created_at,
    }
}

/// The committed `reward_accruals` rows of the transaction row `id`.
async fn stored_accruals(db: &Db, id: TransactionRowId) -> i64 {
    sqlx::query!(
        r#"SELECT count(*) AS "count!" FROM reward_accruals WHERE transaction_id = $1"#,
        id.get()
    )
    .fetch_one(db.pool())
    .await
    .expect("the accrual count reads")
    .count
}

/// The committed `reward_balances` row of `account_id` as (earned, redeemed,
/// available), or `None` when the account has none.
async fn stored_balance(db: &Db, account_id: AccountId) -> Option<(i64, i64, i64)> {
    sqlx::query!(
        "SELECT total_points_earned, total_points_redeemed, available_points \
         FROM reward_balances WHERE account_id = $1",
        account_id.to_i64()
    )
    .fetch_optional(db.pool())
    .await
    .expect("the balance row reads")
    .map(|row| {
        (
            row.total_points_earned,
            row.total_points_redeemed,
            row.available_points,
        )
    })
}

/// `candidate_rules` returns every rule of the (type, category) pair in
/// `rule_id` order, soft-deleted and future rules included: the active window
/// and the amount bounds are the engine's to apply (AAP 0.3.5). The store here
/// is bound to a transaction the caller holds directly.
#[test]
fn candidate_rules_returns_every_rule_for_the_pair() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let mut uow = db.begin().await.expect("a unit of work");
        let mut store = uow.store();
        let ended = store
            .insert_reward_rule(new_rule(
                "01",
                1,
                20_000,
                seed_start(),
                Some(instant(2022, 3, 1, 0)),
            ))
            .await
            .expect("a soft-deleted rule");
        let future = store
            .insert_reward_rule(new_rule("01", 1, 50_000, instant(2030, 1, 1, 0), None))
            .await
            .expect("a rule that has not started");
        let payment = store
            .insert_reward_rule(new_rule("02", 1, 10_000, seed_start(), None))
            .await
            .expect("a payment rule");
        uow.commit().await.expect("the rules commit");

        let mut tx = db.pool().begin().await.expect("a transaction");
        let mut rewards = PgRewardStore::new(&mut tx);
        let cases = [
            ("01", 1, vec![retail_seed_rule(), ended, future]),
            ("02", 1, vec![payment]),
            ("04", 3, vec![travel_seed_rule()]),
            ("03", 1, Vec::new()),
            ("01", 2, Vec::new()),
        ];
        for (type_cd, category_cd, expected) in cases {
            let rules = rewards
                .candidate_rules(TranTypeCode::move_from(type_cd), category(category_cd))
                .await
                .expect("the candidate read");
            assert_eq!(rules, expected, "{type_cd}/{category_cd}");
        }
        tx.rollback().await.expect("the read-only transaction ends");
    });
}

/// A second accrual for one transaction is `PortError::Duplicate`, through the
/// store and through the engine. Each rejected write is undone alone under its
/// savepoint, foreign-key rejections (`PortError::Restricted`) included, so the
/// unit stays usable and commits exactly the first accrual.
#[test]
fn duplicate_accrual_is_reported_and_tx_stays_usable() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let tran = live_transaction(&db, account(1), "01", 1, 10_000).await;
        let other = live_transaction(&db, account(1), "01", 1, 2_000).await;

        let mut uow = db.begin().await.expect("a unit of work");
        let mut rewards = uow.rewards();
        assert_eq!(
            rewards
                .insert_accrual(accrual(&tran, account(1), 1, 100, at()))
                .await,
            Ok(AccrualId::from(1)),
            "the seed restarts the accrual identity at 1"
        );
        assert_eq!(
            rewards
                .insert_accrual(accrual(&tran, account(1), 2, 7, at()))
                .await,
            Err(PortError::Duplicate)
        );
        let missing_row = NewAccrual {
            transaction_id: TransactionRowId::from(999_999),
            ..accrual(&other, account(1), 1, 1, at())
        };
        assert_eq!(
            rewards.insert_accrual(missing_row).await,
            Err(PortError::Restricted)
        );
        let missing_rule = accrual(&other, account(1), 999, 1, at());
        assert_eq!(
            rewards.insert_accrual(missing_rule).await,
            Err(PortError::Restricted)
        );
        assert_eq!(
            rewards.add_points(account(99_999_999_999), points(1)).await,
            Err(PortError::Restricted),
            "a balance needs an accounts row"
        );
        let balance = rewards
            .add_points(account(1), points(100))
            .await
            .expect("the unit is still usable after the rejected writes");
        assert_eq!(
            Ok(balance.clone()),
            RewardBalance::new(account(1), points(100), RewardPoints::ZERO)
        );
        uow.commit().await.expect("the unit commits");

        assert_eq!(stored_accruals(&db, tran.transaction_id).await, 1);
        assert_eq!(stored_accruals(&db, other.transaction_id).await, 0);
        let row = sqlx::query!(
            "SELECT accrual_id, account_id, rule_id, points_earned, created_at \
             FROM reward_accruals WHERE transaction_id = $1",
            tran.transaction_id.get()
        )
        .fetch_one(db.pool())
        .await
        .expect("the accrual row reads");
        assert_eq!(
            (
                row.accrual_id,
                row.account_id,
                row.rule_id,
                row.points_earned
            ),
            (1, 1, 1, 100)
        );
        assert_eq!(row.created_at, at());

        let mut uow = db.begin().await.expect("a unit of work");
        let input = AccrualInput::for_transaction(&tran, account(1));
        assert_eq!(
            RewardEngine::accrue(&mut uow.rewards(), input, at()).await,
            Err(RewardError::Store(PortError::Duplicate))
        );
        assert_eq!(
            uow.store().reward_balance(account(1)).await,
            Ok(Some(balance)),
            "a failed accrual insert credits no points"
        );
        uow.rollback().await.expect("the unit rolls back");
    });
}

/// `add_points` creates the balance row on the first credit and adds to it
/// afterwards; every returned balance, every stored row and the table's
/// `CHECK` hold `available_points = total_points_earned -
/// total_points_redeemed`.
#[test]
fn add_points_upserts_and_keeps_invariant() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        for (credit, total) in [(40, 40), (2, 42)] {
            let mut uow = db.begin().await.expect("a unit of work");
            let balance = uow
                .rewards()
                .add_points(account(1), points(credit))
                .await
                .expect("a credit within the BIGINT range");
            uow.commit().await.expect("the credit commits");
            assert_eq!(
                balance,
                RewardBalance {
                    account_id: account(1),
                    total_points_earned: points(total),
                    total_points_redeemed: RewardPoints::ZERO,
                    available_points: points(total),
                }
            );
            assert!(balance.is_consistent(), "{balance:?}");
            assert_eq!(
                stored_balance(&db, account(1)).await,
                Some((total, 0, total))
            );
        }

        let mut uow = db.begin().await.expect("a unit of work");
        let zero = uow
            .rewards()
            .add_points(account(2), RewardPoints::ZERO)
            .await
            .expect("a zero credit");
        assert_eq!(
            zero,
            RewardBalance::zero(account(2)),
            "a zero credit still creates the row"
        );
        let read_back = uow
            .store()
            .reward_balance(account(1))
            .await
            .expect("the balance read")
            .expect("account 1 has accrued");
        assert!(read_back.is_consistent(), "{read_back:?}");
        assert_eq!(read_back.available_points, points(42));
        uow.commit().await.expect("the zero credit commits");
        assert_eq!(stored_balance(&db, account(2)).await, Some((0, 0, 0)));

        let broken = sqlx::query!(
            "UPDATE reward_balances SET available_points = available_points + 1 \
             WHERE account_id = $1",
            1_i64
        )
        .execute(db.pool())
        .await
        .expect_err("the table rejects a balance that breaks the invariant");
        assert_eq!(
            broken.as_database_error().and_then(|err| err.constraint()),
            Some("reward_balances_available_points_check")
        );
        assert_eq!(stored_balance(&db, account(1)).await, Some((42, 0, 42)));
    });
}

/// A total above 2^53 − 1 is stored and read back exactly, and a credit beyond
/// the `BIGINT` column (SQLSTATE `22003`) is an error, not a panic or a wrap;
/// it leaves the unit usable and, rolled back, the balance unchanged.
#[test]
fn large_points_round_trip_and_overflow_is_an_error() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let mut uow = db.begin().await.expect("a unit of work");
        uow.rewards()
            .add_points(account(2), points(BEYOND_SAFE_INTEGER))
            .await
            .expect("a credit within the BIGINT range");
        uow.commit().await.expect("the credit commits");
        let stored = Some((BEYOND_SAFE_INTEGER, 0, BEYOND_SAFE_INTEGER));
        assert_eq!(stored_balance(&db, account(2)).await, stored);

        let mut uow = db.begin().await.expect("a unit of work");
        let balance = uow
            .store()
            .reward_balance(account(2))
            .await
            .expect("the balance read")
            .expect("account 2 has a balance");
        assert_eq!(balance.total_points_earned.to_i64(), BEYOND_SAFE_INTEGER);
        assert_eq!(balance.total_points_earned.to_string(), "9007199254740993");
        assert_eq!(balance.available_points.to_string(), "9007199254740993");
        assert!(balance.is_consistent(), "{balance:?}");

        let overflow = uow.rewards().add_points(account(2), points(i64::MAX)).await;
        assert!(
            matches!(&overflow, Err(PortError::Unavailable(text)) if text.contains("22003")),
            "{overflow:?}"
        );
        assert_eq!(
            uow.store().reward_balance(account(2)).await,
            Ok(Some(balance)),
            "the failed upsert is undone alone"
        );
        uow.rollback().await.expect("the unit rolls back");
        assert_eq!(stored_balance(&db, account(2)).await, stored);
    });
}

/// `RewardEngine::accrue` over the real store, through `PgStore::reward_store`
/// as CT02 and POSTTRAN hand it the transaction they post in (AAP 0.3.5):
/// step (1), an amount of zero or less, and a pair without a rule accrue
/// nothing; step (4) earns `ceil(amount × rate)` and credits the balance.
/// Its rules are the unbounded seed rules; the amount bounds are
/// `engine_applies_amount_bounds_through_pg_store`'s.
#[test]
fn engine_accrues_through_pg_store() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let retail = live_transaction(&db, account(1), "01", 1, 10_000).await;
        let travel = live_transaction(&db, account(1), "04", 3, 1_001).await;
        let zero = live_transaction(&db, account(1), "01", 1, 0).await;
        let refund = live_transaction(&db, account(1), "01", 1, -500).await;
        let unmatched = live_transaction(&db, account(1), "02", 1, 5_000).await;

        let mut uow = db.begin().await.expect("a unit of work");
        let mut store = uow.store();
        let mut accrued = Vec::new();
        for tran in [&retail, &travel, &zero, &refund, &unmatched] {
            let input = AccrualInput::for_transaction(tran, account(1));
            let outcome = RewardEngine::accrue(&mut store.reward_store(), input, at())
                .await
                .expect("the accrual succeeds");
            accrued.push(outcome);
        }
        uow.commit().await.expect("the accruals commit");

        let expected = |accrual_id: i64, tran: &Transaction, rule_id: i64, earned: i64| {
            Some(RewardAccrual {
                accrual_id: AccrualId::from(accrual_id),
                account_id: account(1),
                transaction_id: tran.transaction_id,
                rule_id: RuleId::from(rule_id),
                points_earned: points(earned),
                created_at: at(),
            })
        };
        // 100.00 at 1.0000 earns 100; 10.01 at 3.0000 is ceil(30.03) = 31.
        assert_eq!(
            accrued,
            vec![
                expected(1, &retail, 1, 100),
                expected(2, &travel, 2, 31),
                None,
                None,
                None,
            ]
        );

        let rows = sqlx::query!(
            "SELECT transaction_id, rule_id, points_earned, created_at \
             FROM reward_accruals ORDER BY accrual_id"
        )
        .fetch_all(db.pool())
        .await
        .expect("the accrual rows read");
        let stored: Vec<(i64, i64, i64, DateTime<Utc>)> = rows
            .iter()
            .map(|row| {
                (
                    row.transaction_id,
                    row.rule_id,
                    row.points_earned,
                    row.created_at,
                )
            })
            .collect();
        assert_eq!(
            stored,
            vec![
                (retail.transaction_id.get(), 1, 100, at()),
                (travel.transaction_id.get(), 2, 31, at()),
            ]
        );
        for tran in [&zero, &refund, &unmatched] {
            assert_eq!(stored_accruals(&db, tran.transaction_id).await, 0);
        }
        assert_eq!(stored_balance(&db, account(1)).await, Some((131, 0, 131)));
    });
}

/// A rule for the pair at `rate_units`, active from [`seed_start`] with no
/// end, bounded below by `min_cents` and above by `max_cents` where given.
fn bounded_rule(
    type_cd: &str,
    category_cd: u16,
    rate_units: i64,
    min_cents: Option<i64>,
    max_cents: Option<i64>,
) -> NewRewardRule {
    NewRewardRule {
        min_transaction_amount: min_cents.map(amount),
        max_transaction_amount: max_cents.map(amount),
        ..new_rule(type_cd, category_cd, rate_units, seed_start(), None)
    }
}

/// Inserts and commits each draft of `rules` through the store, checking that
/// it takes the paired `rule_id` and is stored exactly as drafted, and returns
/// the stored rules in the same order.
async fn commit_rules(db: &Db, rules: Vec<(i64, NewRewardRule)>) -> Vec<RewardRule> {
    let mut uow = db.begin().await.expect("a unit of work");
    let mut store = uow.store();
    let mut created = Vec::new();
    for (rule_id, draft) in rules {
        let rule = store
            .insert_reward_rule(draft.clone())
            .await
            .expect("the insert");
        assert_eq!(rule, RewardRule::from_new(RuleId::from(rule_id), &draft));
        created.push(rule);
    }
    uow.commit().await.expect("the rules commit");
    created
}

/// Every committed `reward_accruals` row in `accrual_id` order as
/// (`transaction_id`, `rule_id`, `points_earned`, `created_at`).
async fn all_accruals(db: &Db) -> Vec<(i64, i64, i64, DateTime<Utc>)> {
    sqlx::query!(
        "SELECT transaction_id, rule_id, points_earned, created_at \
         FROM reward_accruals ORDER BY accrual_id"
    )
    .fetch_all(db.pool())
    .await
    .expect("the accrual rows read")
    .into_iter()
    .map(|row| {
        (
            row.transaction_id,
            row.rule_id,
            row.points_earned,
            row.created_at,
        )
    })
    .collect()
}

/// The committed `reward_accruals` rows of the transaction row `id` as
/// (`accrual_id`, `account_id`, `rule_id`, `points_earned`, `created_at`).
async fn accrual_rows(db: &Db, id: TransactionRowId) -> Vec<(i64, i64, i64, i64, DateTime<Utc>)> {
    sqlx::query!(
        "SELECT accrual_id, account_id, rule_id, points_earned, created_at \
         FROM reward_accruals WHERE transaction_id = $1",
        id.get()
    )
    .fetch_all(db.pool())
    .await
    .expect("the accrual rows read")
    .into_iter()
    .map(|row| {
        (
            row.accrual_id,
            row.account_id,
            row.rule_id,
            row.points_earned,
            row.created_at,
        )
    })
    .collect()
}

/// `RewardEngine::accrue` over the real store against rules with amount
/// bounds (AAP 0.3.5). Each bound is written by the rule insert, read back by
/// `candidate_rules`, which returns every rule of the pair whatever the
/// amount, and applied by the engine: both bounds inclusive, an absent bound
/// unbounded, and the highest rate among the matches winning. The rules sit
/// on pairs no seed rule uses, so each pair's candidates are the rules
/// inserted here:
///
/// - rule 3, `03`/`0001` at 1.2345 from 10.00 to 50.00: each bound accrues,
///   one cent beyond either does not;
/// - rule 4, `03`/`0002` at 2.0000 from 25.00 with no maximum;
/// - rule 5, `05`/`0001` at 0.5000 up to 20.00 with no minimum;
/// - rule 6, `06`/`0002` at 0.2500 with no bounds, from 0.01 to the largest
///   `S9(09)V99` amount;
/// - rules 7 and 8, both `07`/`0001`: 5.0000 from 100.00 to 200.00 and 1.0000
///   from 50.00 to 500.00. The higher rate wins at either of its bounds and,
///   one cent outside its range, loses to the lower rate, still in range.
///
/// Each case posts a live transaction for an account of its own and commits
/// the accrual in a unit of its own, so the returned accrual, the stored row
/// of the transaction and the stored balance are exact per case. A miss
/// stores no accrual and no balance row.
#[test]
fn engine_applies_amount_bounds_through_pg_store() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let rules = vec![
            (3, bounded_rule("03", 1, 12_345, Some(1_000), Some(5_000))),
            (4, bounded_rule("03", 2, 20_000, Some(2_500), None)),
            (5, bounded_rule("05", 1, 5_000, None, Some(2_000))),
            (6, bounded_rule("06", 2, 2_500, None, None)),
            (7, bounded_rule("07", 1, 50_000, Some(10_000), Some(20_000))),
            (8, bounded_rule("07", 1, 10_000, Some(5_000), Some(50_000))),
        ];
        let created = commit_rules(&db, rules).await;

        let mut uow = db.begin().await.expect("a unit of work");
        let mut rewards = uow.rewards();
        for (type_cd, category_cd, expected) in [
            ("03", 1, &created[..1]),
            ("03", 2, &created[1..2]),
            ("05", 1, &created[2..3]),
            ("06", 2, &created[3..4]),
            ("07", 1, &created[4..]),
        ] {
            let candidates = rewards
                .candidate_rules(TranTypeCode::move_from(type_cd), category(category_cd))
                .await
                .expect("the candidate read");
            assert_eq!(candidates, expected, "{type_cd}/{category_cd}");
        }
        uow.rollback().await.expect("the read-only unit ends");

        // (account, type, category, amount in cents, and the accrual as
        // (accrual_id, rule_id, points), or None for no accrual)
        let cases = [
            // Rule 3: 10.00 earns ceil(12.345) = 13, 50.00 ceil(61.725) = 62.
            (11, "03", 1, 1_000, Some((1, 3, 13))),
            (12, "03", 1, 5_000, Some((2, 3, 62))),
            (13, "03", 1, 999, None),
            (14, "03", 1, 5_001, None),
            // Rule 4: 25.00 earns 50, 999,999,999.99 ceil(1,999,999,999.98).
            (15, "03", 2, 2_499, None),
            (16, "03", 2, 2_500, Some((3, 4, 50))),
            (17, "03", 2, 99_999_999_999, Some((4, 4, 2_000_000_000))),
            // Rule 5: 0.01 earns ceil(0.005) = 1, 20.00 earns 10.
            (18, "05", 1, 1, Some((5, 5, 1))),
            (19, "05", 1, 2_000, Some((6, 5, 10))),
            (20, "05", 1, 2_001, None),
            // Rule 6: 0.01 earns ceil(0.0025) = 1, 999,999,999.99
            // ceil(249,999,999.9975).
            (21, "06", 2, 1, Some((7, 6, 1))),
            (22, "06", 2, 99_999_999_999, Some((8, 6, 250_000_000))),
            // Rules 7 and 8: 99.99 and 200.01 fall outside rule 7 and earn
            // ceil(99.99) = 100 and ceil(200.01) = 201 under rule 8; 100.00
            // and 200.00 earn 500 and 1000 under rule 7; 49.99 matches neither.
            (23, "07", 1, 9_999, Some((9, 8, 100))),
            (24, "07", 1, 10_000, Some((10, 7, 500))),
            (25, "07", 1, 20_000, Some((11, 7, 1_000))),
            (26, "07", 1, 20_001, Some((12, 8, 201))),
            (27, "07", 1, 4_999, None),
        ];
        let mut hits = Vec::new();
        for (holder_id, type_cd, category_cd, cents, expected) in cases {
            let case = format!("{type_cd}/{category_cd} at {cents} cents");
            let holder = account(holder_id);
            let tran = live_transaction(&db, holder, type_cd, category_cd, cents).await;
            let mut uow = db.begin().await.expect("a unit of work");
            let accrued = accrue_at(&mut uow, &tran, holder, at()).await;
            uow.commit().await.expect("the accrual commits");

            assert_eq!(
                accrued,
                expected.map(|(accrual_id, rule_id, earned)| RewardAccrual {
                    accrual_id: AccrualId::from(accrual_id),
                    account_id: holder,
                    transaction_id: tran.transaction_id,
                    rule_id: RuleId::from(rule_id),
                    points_earned: points(earned),
                    created_at: at(),
                }),
                "{case}"
            );
            let row = expected.map(|(accrual_id, rule_id, earned)| {
                (accrual_id, holder.to_i64(), rule_id, earned, at())
            });
            assert_eq!(
                accrual_rows(&db, tran.transaction_id).await,
                row.into_iter().collect::<Vec<_>>(),
                "{case}"
            );
            assert_eq!(
                stored_balance(&db, holder).await,
                expected.map(|(_, _, earned)| (earned, 0, earned)),
                "{case}: available = earned - redeemed"
            );
            if let Some((_, rule_id, earned)) = expected {
                hits.push((tran.transaction_id.get(), rule_id, earned, at()));
            }
        }
        assert_eq!(
            all_accruals(&db).await,
            hits,
            "the misses stored no accrual"
        );
    });
}

/// The accrual and the balance credit commit or roll back with their unit of
/// work, and so does the transaction insert of the same unit: the reward
/// atomicity the CT02 and POSTTRAN call sites rely on (AAP 0.3.5).
#[test]
fn engine_rollback_and_commit_are_atomic() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let tran = live_transaction(&db, account(1), "01", 1, 10_000).await;
        let input = AccrualInput::for_transaction(&tran, account(1));

        let mut uow = db.begin().await.expect("a unit of work");
        let rolled_back = RewardEngine::accrue(&mut uow.rewards(), input, at())
            .await
            .expect("the accrual succeeds");
        assert_eq!(
            rolled_back.map(|accrual| accrual.points_earned),
            Some(points(100))
        );
        uow.rollback().await.expect("the unit rolls back");
        assert_eq!(stored_accruals(&db, tran.transaction_id).await, 0);
        assert_eq!(stored_balance(&db, account(1)).await, None);

        let mut uow = db.begin().await.expect("a unit of work");
        let committed = RewardEngine::accrue(&mut uow.rewards(), input, at())
            .await
            .expect("the accrual succeeds")
            .expect("100.00 on 01/0001 earns points");
        uow.commit().await.expect("the unit commits");
        assert_eq!(committed.points_earned, points(100));
        assert_eq!(stored_accruals(&db, tran.transaction_id).await, 1);
        assert_eq!(stored_balance(&db, account(1)).await, Some((100, 0, 100)));

        // The CT02 shape: allocate, insert, accrue in one unit, then roll back.
        let card = card_of(&db, account(1)).await;
        let mut uow = db.begin().await.expect("a unit of work");
        let tran_id = next_key(&mut uow).await;
        let mut store = uow.store();
        let record = new_transaction(tran_id, card, "01", 1, 5_000);
        let row_id = store
            .write_transaction(record.clone())
            .await
            .expect("a live insert");
        let added = Transaction::from_new(row_id, record);
        let input = AccrualInput::for_transaction(&added, account(1));
        let accrued = RewardEngine::accrue(&mut store.reward_store(), input, at())
            .await
            .expect("the accrual succeeds");
        assert_eq!(
            accrued.map(|accrual| accrual.points_earned),
            Some(points(50))
        );
        uow.rollback().await.expect("the unit rolls back");

        let kept = sqlx::query!(
            r#"SELECT count(*) AS "count!" FROM transactions WHERE transaction_id = $1"#,
            row_id.get()
        )
        .fetch_one(db.pool())
        .await
        .expect("the transaction count reads")
        .count;
        assert_eq!(
            kept, 0,
            "the transaction insert rolls back with its accrual"
        );
        assert_eq!(stored_accruals(&db, row_id).await, 0);
        assert_eq!(stored_balance(&db, account(1)).await, Some((100, 0, 100)));
    });
}

/// The rule administration on `PgStore` (`POST`, `PUT`, `DELETE` and `GET
/// /admin/reward-rules`): identities continue after the seed rules, an
/// unknown ID is `None` or `NotFound`, and a soft delete ends an open, future
/// or not yet started window at the delete instant once, then leaves it
/// unchanged (AAP 0.3.5).
#[test]
fn reward_rule_admin_methods() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let mut uow = db.begin().await.expect("a unit of work");
        let mut store = uow.store();
        let draft = NewRewardRule {
            min_transaction_amount: Some(amount(1_000)),
            max_transaction_amount: Some(amount(50_000)),
            ..new_rule("02", 2, 15_000, seed_start(), None)
        };
        let created = store
            .insert_reward_rule(draft.clone())
            .await
            .expect("the insert");
        assert_eq!(created, RewardRule::from_new(RuleId::from(3), &draft));

        let changed = NewRewardRule {
            reward_rate: rate(25_000),
            max_transaction_amount: None,
            ..draft
        };
        let updated = RewardRule::from_new(RuleId::from(3), &changed);
        assert_eq!(
            store
                .update_reward_rule(RuleId::from(3), changed.clone())
                .await,
            Ok(Some(updated.clone()))
        );
        assert_eq!(
            store.update_reward_rule(RuleId::from(999), changed).await,
            Ok(None)
        );
        assert_eq!(
            store.reward_rule(RuleId::from(3)).await,
            Ok(Some(updated.clone()))
        );

        let deleted_at = at();
        let deleted = RewardRule {
            active_to: Some(deleted_at),
            ..updated
        };
        assert_eq!(
            store
                .soft_delete_reward_rule(RuleId::from(3), deleted_at)
                .await,
            Ok(SoftDeleteOutcome::Deleted(deleted.clone()))
        );
        assert_eq!(
            store
                .soft_delete_reward_rule(RuleId::from(3), deleted_at + TimeDelta::days(1))
                .await,
            Ok(SoftDeleteOutcome::Unchanged(deleted.clone()))
        );
        assert_eq!(
            store
                .soft_delete_reward_rule(RuleId::from(999), deleted_at)
                .await,
            Ok(SoftDeleteOutcome::NotFound)
        );

        let later = instant(2030, 1, 1, 0);
        for rule in [
            new_rule("01", 2, 10_000, seed_start(), Some(later)),
            new_rule("05", 1, 10_000, later, None),
        ] {
            let rule = store.insert_reward_rule(rule).await.expect("the insert");
            let rule_id = rule.rule_id;
            let ended = RewardRule {
                active_to: Some(deleted_at),
                ..rule
            };
            assert_eq!(
                store.soft_delete_reward_rule(rule_id, deleted_at).await,
                Ok(SoftDeleteOutcome::Deleted(ended))
            );
        }
        uow.commit().await.expect("the rule changes commit");

        let mut uow = db.begin().await.expect("a unit of work");
        let listed = uow.store().list_reward_rules().await.expect("the list");
        let ids: Vec<i64> = listed.iter().map(|rule| rule.rule_id.get()).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5]);
        assert_eq!(
            listed[..3],
            [retail_seed_rule(), travel_seed_rule(), deleted]
        );
        assert!(
            listed[2..]
                .iter()
                .all(|rule| rule.active_to == Some(deleted_at)),
            "soft-deleted rules stay listed: {listed:?}"
        );
        uow.rollback().await.expect("the read-only unit ends");
    });
}

/// Waits until a session of the test database waits on a row lock.
///
/// The pause between two looks blocks the calling thread rather than
/// yielding: the test has no timer of its own, and the unit of work under
/// observation runs on a thread of its own.
///
/// # Panics
///
/// When no session waits so within [`WAIT_LIMIT`].
async fn await_row_lock_wait(stack: &TestStack) {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let rows = stack.fetch_text_rows(ROW_LOCK_WAITS).await;
        let waiting = rows.first().and_then(|row| row.first()).cloned().flatten();
        if waiting.is_some_and(|sessions| sessions != "0") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no session waited on a row lock within {WAIT_LIMIT:?}"
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

/// `lock_reward_rule` holds the rule until its unit of work ends, the order
/// the soft delete of `DELETE /admin/reward-rules/{id}` relies on (AAP
/// 0.3.5). While a first unit has locked the retail seed rule and ended it at
/// [`at`] without committing, a second unit's lock of the rule waits. Once
/// the first commits, the second's lock returns the rule with that end, and
/// the second's soft delete at a later instant is `Unchanged`, so the end
/// committed first stays stored. A rule ID no rule holds is `None`.
#[test]
fn a_locked_rule_holds_a_second_delete_until_the_first_unit_ends() {
    let prepared = fixture_seed();
    block_on(async {
        let (stack, db) = seeded(prepared).await;
        let retail = RuleId::from(1);
        let first_end = at();
        let ended = RewardRule {
            active_to: Some(first_end),
            ..retail_seed_rule()
        };

        let mut first = db.begin().await.expect("a unit of work");
        let mut store = first.store();
        assert_eq!(
            store.lock_reward_rule(retail).await,
            Ok(Some(retail_seed_rule()))
        );
        assert_eq!(
            store.soft_delete_reward_rule(retail, first_end).await,
            Ok(SoftDeleteOutcome::Deleted(ended.clone()))
        );
        assert_eq!(store.lock_reward_rule(RuleId::from(999)).await, Ok(None));

        let second = {
            let db = db.clone();
            thread::spawn(move || {
                block_on(async move {
                    let mut uow = db.begin().await.expect("a unit of work");
                    let locked = uow.store().lock_reward_rule(retail).await;
                    (uow, locked)
                })
            })
        };
        await_row_lock_wait(&stack).await;
        assert!(
            !second.is_finished(),
            "the second lock waits for the first unit"
        );
        first.commit().await.expect("the first delete commits");

        let (mut second, locked) = join_within(second, "the second lock");
        assert_eq!(
            locked,
            Ok(Some(ended.clone())),
            "the second lock reads the end committed first"
        );
        assert_eq!(
            second
                .store()
                .soft_delete_reward_rule(retail, first_end + TimeDelta::seconds(1))
                .await,
            Ok(SoftDeleteOutcome::Unchanged(ended.clone()))
        );
        second.commit().await.expect("the second unit commits");

        let mut uow = db.begin().await.expect("a unit of work");
        assert_eq!(
            uow.store().reward_rule(retail).await,
            Ok(Some(ended)),
            "the end committed first stays stored"
        );
        uow.rollback().await.expect("the read-only unit ends");
    });
}

/// [`at`] plus `micros` microseconds.
fn at_plus_micros(micros: i64) -> DateTime<Utc> {
    at() + TimeDelta::microseconds(micros)
}

/// `RewardEngine::accrue` of `tran` for `account_id` at processing instant
/// `t`, over the reward store of `uow`.
async fn accrue_at(
    uow: &mut UnitOfWork,
    tran: &Transaction,
    account_id: AccountId,
    t: DateTime<Utc>,
) -> Option<RewardAccrual> {
    let input = AccrualInput::for_transaction(tran, account_id);
    RewardEngine::accrue(&mut uow.rewards(), input, t)
        .await
        .expect("the accrual succeeds")
}

/// The `PortError` a reward write reports for an instant `column` cannot store
/// exactly.
fn inexact<T>(column: &str) -> Result<T, PortError> {
    Err(PortError::Unavailable(format!(
        "{column} cannot store the instant exactly: it is not a whole microsecond of UTC time"
    )))
}

/// A rule window one microsecond wide is stored exactly as submitted, and the
/// engine over the real store decides its start and end at those instants:
/// the start inclusive, the end exclusive (AAP 0.3.5). The accrual the engine
/// returns carries the same `created_at` as the row the history reads back.
#[test]
fn microsecond_windows_and_accrual_instants_round_trip_exactly() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let start = at_plus_micros(1);
        let end = at_plus_micros(2);
        let draft = new_rule("02", 1, 10_000, start, Some(end));
        let mut uow = db.begin().await.expect("a unit of work");
        let created = uow
            .store()
            .insert_reward_rule(draft.clone())
            .await
            .expect("the insert");
        assert_eq!(created, RewardRule::from_new(RuleId::from(3), &draft));
        uow.commit().await.expect("the rule commits");

        let holder = account(5);
        let before_start = live_transaction(&db, holder, "02", 1, 10_000).await;
        let at_start = live_transaction(&db, holder, "02", 1, 10_000).await;
        let at_end = live_transaction(&db, holder, "02", 1, 10_000).await;

        let mut uow = db.begin().await.expect("a unit of work");
        assert_eq!(
            uow.store().reward_rule(created.rule_id).await,
            Ok(Some(created.clone())),
            "the reloaded window is the submitted one"
        );
        let before = accrue_at(&mut uow, &before_start, holder, at_plus_micros(0)).await;
        let first = accrue_at(&mut uow, &at_start, holder, start).await;
        let after = accrue_at(&mut uow, &at_end, holder, end).await;
        uow.commit().await.expect("the accruals commit");

        assert_eq!(before, None, "one microsecond before active_from");
        assert_eq!(after, None, "at active_to, which is exclusive");
        let accrued = first.expect("at active_from, which is inclusive");
        assert_eq!(
            (accrued.rule_id, accrued.points_earned, accrued.created_at),
            (created.rule_id, points(100), start)
        );

        let mut uow = db.begin().await.expect("a unit of work");
        let history = uow
            .store()
            .reward_history(holder, None, 10)
            .await
            .expect("the history read");
        assert_eq!(history, vec![accrued], "returned and reloaded agree");
        uow.rollback().await.expect("the read-only unit ends");
        for tran in [&before_start, &at_end] {
            assert_eq!(stored_accruals(&db, tran.transaction_id).await, 0);
        }
    });
}

/// An instant finer than a microsecond is refused by every reward write that
/// binds one, naming the table and column, and nothing is stored: the rule
/// insert and update, the soft delete and the accrual insert, directly and
/// through the engine. The bind never narrows it to the microsecond
/// `TIMESTAMPTZ` keeps.
#[test]
fn nanosecond_instants_are_refused_and_nothing_is_stored() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let tran = live_transaction(&db, account(1), "01", 1, 10_000).await;
        let nanos = at() + TimeDelta::nanoseconds(900);

        let mut uow = db.begin().await.expect("a unit of work");
        let mut store = uow.store();
        assert_eq!(
            store
                .insert_reward_rule(new_rule("02", 1, 10_000, nanos, None))
                .await,
            inexact("reward_rules.active_from")
        );
        assert_eq!(
            store
                .insert_reward_rule(new_rule("02", 1, 10_000, seed_start(), Some(nanos)))
                .await,
            inexact("reward_rules.active_to")
        );
        assert_eq!(
            store
                .update_reward_rule(RuleId::from(1), new_rule("01", 1, 20_000, nanos, None))
                .await,
            inexact("reward_rules.active_from")
        );
        assert_eq!(
            store.soft_delete_reward_rule(RuleId::from(1), nanos).await,
            inexact("reward_rules.active_to")
        );
        assert_eq!(
            store
                .reward_store()
                .insert_accrual(accrual(&tran, account(1), 1, 100, nanos))
                .await,
            inexact("reward_accruals.created_at")
        );
        let input = AccrualInput::for_transaction(&tran, account(1));
        assert_eq!(
            RewardEngine::accrue(&mut store.reward_store(), input, nanos).await,
            inexact("reward_accruals.created_at").map_err(RewardError::Store)
        );
        uow.commit()
            .await
            .expect("no statement ran, so the unit commits");

        let mut uow = db.begin().await.expect("a unit of work");
        assert_eq!(
            uow.store().list_reward_rules().await,
            Ok(vec![retail_seed_rule(), travel_seed_rule()]),
            "no rule was added or changed"
        );
        uow.rollback().await.expect("the read-only unit ends");
        assert_eq!(stored_accruals(&db, tran.transaction_id).await, 0);
        assert_eq!(stored_balance(&db, account(1)).await, None);
    });
}

/// The balance read and the reward history page: no balance before any
/// accrual; history newest first by `created_at`, equal instants by
/// `accrual_id`, one look-ahead row beyond `limit`, an inclusive start key,
/// and an empty page for a start key that names no accrual of the account.
#[test]
fn reward_balance_and_history() {
    let prepared = fixture_seed();
    block_on(async {
        let (_stack, db) = seeded(prepared).await;
        let holder = account(3);
        let mut purchases = Vec::new();
        for cents in [1_000, 2_000, 3_000, 4_000] {
            purchases.push(live_transaction(&db, holder, "01", 1, cents).await);
        }
        let elsewhere = live_transaction(&db, account(1), "01", 1, 1_000).await;
        let tick = TimeDelta::seconds(1);
        let instants = [at(), at() + tick, at() + tick * 2, at() + tick * 2];

        let mut uow = db.begin().await.expect("a unit of work");
        assert_eq!(uow.store().reward_balance(holder).await, Ok(None));
        let mut rewards = uow.rewards();
        let mut new_accruals = Vec::new();
        let mut ids = Vec::new();
        for (tran, created_at) in purchases.iter().zip(instants) {
            let new_accrual = accrual(tran, holder, 1, 10, created_at);
            ids.push(
                rewards
                    .insert_accrual(new_accrual.clone())
                    .await
                    .expect("the insert"),
            );
            new_accruals.push(new_accrual);
        }
        let foreign = rewards
            .insert_accrual(accrual(&elsewhere, account(1), 1, 10, at() + tick * 3))
            .await
            .expect("the insert");
        rewards
            .add_points(holder, points(40))
            .await
            .expect("the credit");
        uow.commit().await.expect("the accruals commit");

        let mut uow = db.begin().await.expect("a unit of work");
        let mut store = uow.store();
        assert_eq!(
            store.reward_balance(holder).await,
            Ok(Some(
                RewardBalance::new(holder, points(40), RewardPoints::ZERO).expect("consistent")
            ))
        );
        let whole: Vec<RewardAccrual> = ids
            .iter()
            .zip(&new_accruals)
            .rev()
            .map(|(id, new_accrual)| RewardAccrual::from_new(*id, new_accrual))
            .collect();
        assert_eq!(store.reward_history(holder, None, 10).await, Ok(whole));

        let cases = [
            (None, 2, vec![ids[3], ids[2], ids[1]]),
            (Some(ids[2]), 2, vec![ids[2], ids[1], ids[0]]),
            (Some(ids[3]), 1, vec![ids[3], ids[2]]),
            (Some(ids[0]), 2, vec![ids[0]]),
            (Some(foreign), 2, Vec::new()),
            (Some(AccrualId::from(999)), 2, Vec::new()),
        ];
        for (start_key, limit, expected) in cases {
            let page = store
                .reward_history(holder, start_key, limit)
                .await
                .expect("the history read");
            let keys: Vec<AccrualId> = page.iter().map(|row| row.accrual_id).collect();
            assert_eq!(keys, expected, "start {start_key:?}, limit {limit}");
        }
        assert_eq!(
            store.reward_history(account(4), None, 10).await,
            Ok(Vec::new())
        );
        uow.rollback().await.expect("the read-only unit ends");
    });
}
