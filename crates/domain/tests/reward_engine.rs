//! `RewardEngine` behaviour through the crate's public API (request Rewards
//! system; AAP 0.3.5).
//!
//! The engine works in four steps, which these tests cover in turn:
//!
//! 1. An amount of zero or less accrues nothing.
//! 2. A rule matches when its type and category codes equal the transaction's,
//!    it is active at the processing instant *t* (`active_from <= t`, and no
//!    `active_to` or `t < active_to`), and the amount lies within its bounds;
//!    both bounds are inclusive and an absent bound is unbounded.
//! 3. The highest `reward_rate` among the matches wins, equal rates go to the
//!    lowest `rule_id`, and no match accrues nothing.
//! 4. `points = ceil(amount × rate)`, and the account's balance is credited
//!    with `available_points = total_points_earned - total_points_redeemed`.
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a
//! roadmap item [README.md:L357-369]. The tests therefore cite AAP 0.3.5
//! rather than parity audit rows, and take their codes from the fixtures the
//! seed rules bind to (AAP 0.3.6).
//!
//! `RewardEngine::accrue` is the one reward calculation (AAP 0.6.4), so every
//! test calls it, or the public step functions it is built from, and none
//! computes points itself. The store is `MemoryRewardStore`, a `RewardStore`
//! written here as an adapter outside the crate writes one. The domain crate
//! has no async runtime, not even as a dev-dependency (R3, AAP 0.10.4), and
//! the in-memory futures are always ready, so `block_on` polls them with a
//! no-op waker.

use std::collections::HashMap;
use std::future::{self, Future};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use rust_decimal::Decimal;

use carddemo_domain::entities::{NewAccrual, RewardAccrual, RewardBalance, RewardRule};
use carddemo_domain::numeric::{
    AccountId, AccrualId, Money, Overflow, RewardPoints, RewardRate, RuleId, TranCategoryCode,
    TranTypeCode, TransactionRowId,
};
use carddemo_domain::ports::{PortError, PortResult};
use carddemo_domain::rewards::engine::{credit, points_for, select_rule};
use carddemo_domain::rewards::{AccrualInput, RewardEngine, RewardError, RewardStore};

/// How often `block_on` polls a future before it gives up.
const POLL_LIMIT: usize = 8;

/// The output of `future`, polled with a no-op waker.
///
/// Every `MemoryRewardStore` future is ready when it is returned, and so is
/// the accrual that awaits only them, so the first poll completes. The bound
/// turns a future that never completes into a failed test instead of a hang.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..POLL_LIMIT {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("the future was still pending after {POLL_LIMIT} polls");
}

/// One method of the `RewardStore` port, named so that a test can make it fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreCall {
    CandidateRules,
    InsertAccrual,
    AddPoints,
}

impl StoreCall {
    /// The error the store returns when this call is made to fail.
    fn injected_error(self) -> PortError {
        PortError::Unavailable(format!("injected {self:?} failure"))
    }
}

/// The three reward tables in memory, with the constraints the port contract
/// names: one accrual per `transaction_id` (`UNIQUE (transaction_id)`) and a
/// balance per account that holds `available = earned - redeemed`.
///
/// Each method does its work when called and returns a ready future.
#[derive(Debug)]
struct MemoryRewardStore {
    /// The `reward_rules` rows.
    rules: Vec<RewardRule>,
    /// The `reward_accruals` rows, in insertion order.
    accruals: Vec<RewardAccrual>,
    /// The `reward_balances` rows, keyed by account.
    balances: HashMap<AccountId, RewardBalance>,
    /// The identity the next inserted accrual receives; the identity column
    /// starts at 1.
    next_accrual_id: i64,
    /// The number of port calls made, so a test can assert that a path
    /// reaches no store call, or stops after a failed one.
    calls: usize,
    /// The call that fails with `PortError::Unavailable` every time it is
    /// made.
    fail_on: Option<StoreCall>,
}

impl MemoryRewardStore {
    /// A store holding `rules`, with no accruals, no balances and no failure.
    fn with_rules(rules: Vec<RewardRule>) -> Self {
        Self {
            rules,
            accruals: Vec::new(),
            balances: HashMap::new(),
            next_accrual_id: 1,
            calls: 0,
            fail_on: None,
        }
    }

    /// Counts a call to `call`, and fails it when the store was told to.
    fn enter(&mut self, call: StoreCall) -> PortResult<()> {
        self.calls += 1;
        if self.fail_on == Some(call) {
            Err(call.injected_error())
        } else {
            Ok(())
        }
    }

    /// Every rule of the pair, active or not and whatever its bounds: the
    /// store applies neither, as the port contract requires.
    fn rules_of(
        &mut self,
        type_cd: TranTypeCode,
        category_cd: TranCategoryCode,
    ) -> PortResult<Vec<RewardRule>> {
        self.enter(StoreCall::CandidateRules)?;
        Ok(self
            .rules
            .iter()
            .filter(|rule| {
                rule.transaction_type_cd == type_cd && rule.transaction_category_cd == category_cd
            })
            .cloned()
            .collect())
    }

    /// Inserts `accrual` under the next identity, or reports a duplicate
    /// `transaction_id`.
    fn insert(&mut self, accrual: &NewAccrual) -> PortResult<AccrualId> {
        self.enter(StoreCall::InsertAccrual)?;
        if self
            .accruals
            .iter()
            .any(|stored| stored.transaction_id == accrual.transaction_id)
        {
            return Err(PortError::Duplicate);
        }
        let accrual_id = AccrualId::from(self.next_accrual_id);
        self.next_accrual_id += 1;
        self.accruals
            .push(RewardAccrual::from_new(accrual_id, accrual));
        Ok(accrual_id)
    }

    /// Upserts the balance of `account_id` with the engine's own `credit`, so
    /// the invariant is computed exactly as the engine computes it.
    fn credit_account(
        &mut self,
        account_id: AccountId,
        points: RewardPoints,
    ) -> PortResult<RewardBalance> {
        self.enter(StoreCall::AddPoints)?;
        let current = self
            .balances
            .get(&account_id)
            .cloned()
            .unwrap_or_else(|| RewardBalance::zero(account_id));
        // The persistence adapter reports a total beyond its `BIGINT` column
        // as `Unavailable`; the in-memory store does the same.
        let updated = credit(&current, points).map_err(|overflow| {
            PortError::Unavailable(format!("reward_balances.total_points_earned: {overflow}"))
        })?;
        self.balances.insert(account_id, updated.clone());
        Ok(updated)
    }
}

impl RewardStore for MemoryRewardStore {
    fn candidate_rules(
        &mut self,
        type_cd: TranTypeCode,
        category_cd: TranCategoryCode,
    ) -> impl Future<Output = PortResult<Vec<RewardRule>>> + Send + '_ {
        future::ready(self.rules_of(type_cd, category_cd))
    }

    fn insert_accrual(
        &mut self,
        accrual: NewAccrual,
    ) -> impl Future<Output = PortResult<AccrualId>> + Send + '_ {
        future::ready(self.insert(&accrual))
    }

    fn add_points(
        &mut self,
        account_id: AccountId,
        points: RewardPoints,
    ) -> impl Future<Output = PortResult<RewardBalance>> + Send + '_ {
        future::ready(self.credit_account(account_id, points))
    }
}

/// A transaction type and category code pair of the fixtures.
#[derive(Debug, Clone, Copy)]
struct Pair {
    type_cd: &'static str,
    category_cd: u16,
}

/// The retail seed rule's pair (AAP 0.3.6): type `01` Purchase
/// [app/data/ASCII/trantype.txt:L1], category `0001` Regular Sales Draft
/// [app/data/ASCII/trancatg.txt:L1].
const RETAIL: Pair = Pair {
    type_cd: "01",
    category_cd: 1,
};

/// The travel seed rule's pair (AAP 0.3.6): type `04` Authorization
/// [app/data/ASCII/trantype.txt:L4], category `0003` Travel booking
/// authorization [app/data/ASCII/trancatg.txt:L14].
const TRAVEL: Pair = Pair {
    type_cd: "04",
    category_cd: 3,
};

fn instant(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
        .single()
        .expect("a valid UTC instant")
}

/// `2022-01-01T00:00:00Z`, the `active_from` of the seed rules (AAP 0.3.6).
fn seed_start() -> DateTime<Utc> {
    instant(2022, 1, 1, 0)
}

/// The processing instant *t* every accrual runs at, `2022-07-01T12:00:00Z`:
/// the clock the parity runs pin through `CARDDEMO_CLOCK_OVERRIDE`
/// (AAP 0.9.3).
fn processing_clock() -> DateTime<Utc> {
    instant(2022, 7, 1, 12)
}

/// A `TRAN-AMT PIC S9(09)V99` amount [app/cpy/CVTRA05Y.cpy:L10] of `cents`.
fn amount(cents: i64) -> Money<9> {
    Money::from_cents(cents).expect("an S9(09)V99 amount")
}

/// A reward rate of `ten_thousandths` units of 0.0001, the scale of its
/// `NUMERIC(9,4)` column.
fn rate(ten_thousandths: i64) -> RewardRate {
    RewardRate::from_decimal(Decimal::new(ten_thousandths, 4)).expect("a reward rate")
}

fn points(value: i64) -> RewardPoints {
    RewardPoints::try_from(value).expect("non-negative points")
}

/// An `ACCT-ID PIC 9(11)` account number; account `00000000001` is a fixture
/// account (AAP 0.9.3).
fn account(number: u64) -> AccountId {
    AccountId::new(number).expect("a 9(11) account number")
}

fn category(code: u16) -> TranCategoryCode {
    TranCategoryCode::new(code).expect("a 9(04) category code")
}

/// An unbounded, open-ended rule of `pair` at `rate_ten_thousandths`, active
/// from the seed start.
///
/// Bounds and windows are set with struct update syntax on this rule rather
/// than as further builder arguments: an eight-argument builder fails
/// `clippy::too_many_arguments`, and the workspace allows no lint suppression.
fn rule(rule_id: i64, pair: Pair, rate_ten_thousandths: i64) -> RewardRule {
    RewardRule {
        rule_id: RuleId::from(rule_id),
        transaction_type_cd: TranTypeCode::move_from(pair.type_cd),
        transaction_category_cd: category(pair.category_cd),
        reward_rate: rate(rate_ten_thousandths),
        min_transaction_amount: None,
        max_transaction_amount: None,
        active_from: seed_start(),
        active_to: None,
    }
}

/// A transaction of `pair` and `cents` on row `transaction_id`, credited to
/// `account_id`.
fn input(transaction_id: i64, account_id: AccountId, pair: Pair, cents: i64) -> AccrualInput {
    AccrualInput {
        transaction_id: TransactionRowId::from(transaction_id),
        account_id,
        type_cd: TranTypeCode::move_from(pair.type_cd),
        category_cd: category(pair.category_cd),
        amount: amount(cents),
    }
}

/// `RewardEngine::accrue` of `input` at *t* against `store`.
fn accrue(
    store: &mut MemoryRewardStore,
    input: AccrualInput,
) -> Result<Option<RewardAccrual>, RewardError> {
    block_on(RewardEngine::accrue(store, input, processing_clock()))
}

/// The winning `rule_id` and the points of the accrual `input` earns at *t* in
/// a store holding `rules`, or `None` when it accrues nothing.
fn outcome(rules: Vec<RewardRule>, input: AccrualInput) -> Option<(i64, i64)> {
    let mut store = MemoryRewardStore::with_rules(rules);
    accrue(&mut store, input)
        .expect("a store without an injected failure accrues")
        .map(|accrual| (accrual.rule_id.get(), accrual.points_earned.to_i64()))
}

/// Compiles only when `T` is `Send`.
fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_non_positive_amount_accrues_nothing_and_reaches_no_store_call() {
    // Step (1): 0.00 and -5.00, under a rule that matches every amount.
    for cents in [0, -500] {
        let mut store = MemoryRewardStore::with_rules(vec![rule(1, RETAIL, 10_000)]);
        let result = accrue(&mut store, input(1, account(1), RETAIL, cents));
        assert_eq!(result, Ok(None), "{cents}");
        assert_eq!(store.calls, 0, "{cents}");
        assert!(store.accruals.is_empty(), "{cents}");
        assert!(store.balances.is_empty(), "{cents}");
    }
}

#[test]
fn a_transaction_without_a_rule_of_its_pair_accrues_nothing() {
    let mut store = MemoryRewardStore::with_rules(Vec::new());
    let result = accrue(&mut store, input(1, account(1), RETAIL, 5_047));
    assert_eq!(result, Ok(None));
    assert_eq!(store.calls, 1, "only the candidate read");
    assert!(store.accruals.is_empty());
    assert!(store.balances.is_empty());
}

#[test]
fn amount_bounds_are_inclusive_and_an_absent_bound_is_unbounded() {
    // (min cents, max cents, amount cents, points; `None` accrues nothing)
    let cases = [
        // Absent bounds: the smallest and the largest S9(09)V99 amount.
        (None, None, 1, Some(1)),
        (None, None, 99_999_999_999, Some(1_000_000_000)),
        // Minimum 10.00: below it, and exactly at it.
        (Some(1_000), None, 999, None),
        (Some(1_000), None, 1_000, Some(10)),
        // Maximum 100.00: above it, and exactly at it.
        (None, Some(10_000), 10_001, None),
        (None, Some(10_000), 10_000, Some(100)),
        // Both bounds, the amount between them.
        (Some(1_000), Some(10_000), 5_047, Some(51)),
    ];
    for (min, max, cents, expected) in cases {
        let bounded = RewardRule {
            min_transaction_amount: min.map(amount),
            max_transaction_amount: max.map(amount),
            ..rule(1, RETAIL, 10_000)
        };
        assert_eq!(
            outcome(vec![bounded], input(1, account(1), RETAIL, cents)),
            expected.map(|earned| (1, earned)),
            "{min:?}..={max:?} with {cents} cents"
        );
    }
}

#[test]
fn a_rule_accrues_only_inside_its_active_window() {
    let t = processing_clock();
    let tick = TimeDelta::nanoseconds(1);
    // (active_from, active_to, accrues)
    let cases = [
        (t, None, true),
        (seed_start(), Some(t), false),
        (t + tick, None, false),
        (seed_start(), None, true),
        (seed_start(), Some(t + tick), true),
    ];
    for (active_from, active_to, accrues) in cases {
        let windowed = RewardRule {
            active_from,
            active_to,
            ..rule(1, RETAIL, 10_000)
        };
        assert_eq!(
            outcome(vec![windowed], input(1, account(1), RETAIL, 5_047)).is_some(),
            accrues,
            "from {active_from} to {active_to:?} at {t}"
        );
    }
}

#[test]
fn the_highest_matching_rate_wins() {
    // 50.47, the amount of the first `app/data/ASCII/dailytran.txt` record, at
    // 2.0000 earns ceil(100.94) = 101 points.
    let rules = vec![rule(1, RETAIL, 10_000), rule(2, RETAIL, 20_000)];
    assert_eq!(
        outcome(rules, input(1, account(1), RETAIL, 5_047)),
        Some((2, 101))
    );
}

#[test]
fn equal_rates_go_to_the_lowest_rule_id() {
    // The higher ID is stored first, so the store's order does not decide.
    let rules = vec![rule(7, RETAIL, 15_000), rule(3, RETAIL, 15_000)];
    assert_eq!(
        outcome(rules, input(1, account(1), RETAIL, 1_000)),
        Some((3, 15))
    );
}

#[test]
fn a_higher_rate_excluded_by_its_range_loses_to_an_in_range_lower_rate() {
    let capped = RewardRule {
        max_transaction_amount: Some(amount(5_000)),
        ..rule(1, RETAIL, 30_000)
    };
    let rules = vec![capped, rule(2, RETAIL, 10_000)];
    assert_eq!(
        outcome(rules.clone(), input(1, account(1), RETAIL, 6_000)),
        Some((2, 60))
    );
    // Within its range the higher rate wins again.
    assert_eq!(
        outcome(rules, input(1, account(1), RETAIL, 5_000)),
        Some((1, 150))
    );
}

#[test]
fn a_rule_of_another_pair_never_applies() {
    let travel = rule(1, TRAVEL, 30_000);
    let other_category = rule(
        2,
        Pair {
            type_cd: "01",
            category_cd: 3,
        },
        30_000,
    );
    let other_type = rule(
        3,
        Pair {
            type_cd: "04",
            category_cd: 1,
        },
        30_000,
    );
    let retail = input(1, account(1), RETAIL, 5_047);

    let mut store = MemoryRewardStore::with_rules(vec![travel.clone()]);
    assert_eq!(accrue(&mut store, retail), Ok(None));
    assert!(store.accruals.is_empty());

    // The in-memory store already filters by pair, so the engine's own match
    // is checked on rules no store has filtered.
    let unfiltered = [travel, other_category, other_type];
    assert_eq!(select_rule(&unfiltered, &retail, processing_clock()), None);
    let travel_input = input(1, account(1), TRAVEL, 5_047);
    assert_eq!(
        select_rule(&unfiltered, &travel_input, processing_clock()).map(|rule| rule.rule_id),
        Some(RuleId::from(1))
    );
}

#[test]
fn points_are_the_exact_product_rounded_up() {
    // (pair, rate in units of 0.0001, amount cents, points)
    let cases = [
        (RETAIL, 10_000, 1_001, 11),
        (RETAIL, 15_000, 1_000, 15),
        (RETAIL, 10_000, 1, 1),
        // 33.33 × 3.0000 = 99.99, rounded up.
        (TRAVEL, 30_000, 3_333, 100),
    ];
    for (pair, rate_ten_thousandths, cents, expected) in cases {
        assert_eq!(
            points_for(amount(cents), rate(rate_ten_thousandths)),
            Ok(points(expected)),
            "{cents} cents at {rate_ten_thousandths}"
        );
        assert_eq!(
            outcome(
                vec![rule(1, pair, rate_ten_thousandths)],
                input(1, account(1), pair, cents)
            ),
            Some((1, expected)),
            "{cents} cents at {rate_ten_thousandths}"
        );
    }
}

#[test]
fn an_accrual_records_the_winning_rule_the_transaction_and_the_instant() {
    let mut store = MemoryRewardStore::with_rules(vec![
        rule(4, RETAIL, 10_000),
        rule(5, RETAIL, 5_000),
        rule(6, TRAVEL, 30_000),
    ]);
    let accrual = accrue(&mut store, input(41, account(1), RETAIL, 5_047))
        .expect("the accrual succeeds")
        .expect("the retail rule matches");

    let expected = RewardAccrual {
        accrual_id: AccrualId::from(1),
        account_id: account(1),
        transaction_id: TransactionRowId::from(41),
        rule_id: RuleId::from(4),
        points_earned: points(51),
        created_at: processing_clock(),
    };
    assert_eq!(accrual, expected);
    assert_eq!(store.accruals, vec![expected]);
    assert_eq!(store.calls, 3, "candidates, insert, credit");
}

#[test]
fn accruals_credit_the_balance_of_their_account() {
    let mut store = MemoryRewardStore::with_rules(vec![rule(1, RETAIL, 10_000)]);
    for (transaction_id, account_number, cents) in [(1, 1, 1_100), (2, 1, 1_500), (3, 2, 500)] {
        accrue(
            &mut store,
            input(transaction_id, account(account_number), RETAIL, cents),
        )
        .expect("the accrual succeeds")
        .expect("the retail rule matches");
    }

    let balance = &store.balances[&account(1)];
    assert_eq!(balance.account_id, account(1));
    assert_eq!(balance.total_points_earned, points(26));
    assert_eq!(balance.total_points_redeemed, RewardPoints::ZERO);
    assert_eq!(balance.available_points, points(26));
    assert!(balance.is_consistent());

    // Another account's accrual credits its own balance only.
    assert_eq!(
        store.balances.get(&account(2)),
        Some(&RewardBalance::new(account(2), points(5), RewardPoints::ZERO).expect("a balance"))
    );
    assert_eq!(store.balances.len(), 2);
}

#[test]
fn credit_keeps_the_redeemed_total_and_derives_the_available_points() {
    let start =
        RewardBalance::new(account(1), points(100), points(40)).expect("a consistent balance");
    assert_eq!(start.available_points, points(60));

    let credited = credit(&start, points(5)).expect("a credit within range");
    assert_eq!(credited.account_id, account(1));
    assert_eq!(credited.total_points_earned, points(105));
    assert_eq!(credited.total_points_redeemed, points(40));
    assert_eq!(credited.available_points, points(65));
    assert!(credited.is_consistent());

    // Available points are never negative.
    assert_eq!(
        RewardBalance::new(account(1), points(40), points(100)),
        Err(Overflow)
    );
}

#[test]
fn a_failed_store_call_propagates_unchanged_and_stops_the_accrual() {
    // (failing call, calls made, accruals left in the store)
    let cases = [
        (StoreCall::CandidateRules, 1, 0),
        (StoreCall::InsertAccrual, 2, 0),
        // The engine neither commits nor rolls back: the inserted accrual is
        // left for the caller's database transaction to roll back.
        (StoreCall::AddPoints, 3, 1),
    ];
    for (failing, calls, accruals) in cases {
        let mut store = MemoryRewardStore::with_rules(vec![rule(1, RETAIL, 10_000)]);
        store.fail_on = Some(failing);
        let result = accrue(&mut store, input(1, account(1), RETAIL, 5_047));
        assert!(
            matches!(
                &result,
                Err(RewardError::Store(error @ PortError::Unavailable(_)))
                    if *error == failing.injected_error()
            ),
            "{failing:?}: {result:?}"
        );
        assert_eq!(store.calls, calls, "{failing:?}");
        assert_eq!(store.accruals.len(), accruals, "{failing:?}");
        assert!(store.balances.is_empty(), "{failing:?}");
    }
}

#[test]
fn a_second_accrual_for_the_same_transaction_is_a_store_duplicate() {
    let mut store = MemoryRewardStore::with_rules(vec![rule(1, RETAIL, 10_000)]);
    accrue(&mut store, input(41, account(1), RETAIL, 1_100))
        .expect("the first accrual succeeds")
        .expect("the retail rule matches");

    let again = accrue(&mut store, input(41, account(1), RETAIL, 1_500));
    assert!(
        matches!(again, Err(RewardError::Store(PortError::Duplicate))),
        "{again:?}"
    );
    assert_eq!(store.accruals.len(), 1);
    assert_eq!(store.calls, 5, "no credit after the rejected insert");
    assert_eq!(
        store
            .balances
            .get(&account(1))
            .map(|balance| balance.total_points_earned),
        Some(points(11))
    );
}

#[test]
fn the_accrual_future_is_send_for_a_store_outside_the_crate() {
    // CT02 and POSTTRAN run the accrual on a multi-threaded runtime, through
    // a store the persistence crate implements outside this one.
    let mut store = MemoryRewardStore::with_rules(vec![rule(1, RETAIL, 10_000)]);
    let future = RewardEngine::accrue(
        &mut store,
        input(1, account(1), RETAIL, 5_047),
        processing_clock(),
    );
    assert_send(&future);
    assert!(matches!(block_on(future), Ok(Some(_))));
}
