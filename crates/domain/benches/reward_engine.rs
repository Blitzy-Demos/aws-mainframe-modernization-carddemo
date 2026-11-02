//! Criterion benchmarks of `RewardEngine::accrue`, the one reward calculation
//! of the system (request Rewards system; AAP 0.3.5, 0.6.4).
//!
//! Rewards have no COBOL oracle (AAP 0.9.2), so no parity result is asserted
//! here. Before any timing, each rule set is accrued once on a fresh store to
//! prove that the timed path is the matching path: a rule of the fixture pair
//! wins, its points are recorded, the balance is credited, and `select_rule`
//! picks the same rule.
//!
//! - `reward_engine/accrue`: the whole accrual, steps (1) to (4), against an
//!   in-memory store, for the seeded rules and for 10, 100 and 1000 rules.
//! - `reward_engine/select_rule`: the rule selection alone, steps (2) and (3),
//!   over the candidates the store returns for the fixture pair.
//! - `reward_engine/points_for`: `ceil(amount × rate)`, step (4), for the
//!   smallest, the fixture and the largest `Money<9>` amounts at the two
//!   seeded rates and an intermediate one.
//!
//! The fixture transaction is record 1 of `app/data/ASCII/dailytran.txt`:
//! type `01`, category `0001`, `TRAN-AMT` `00000005047G`, that is +50.47. The
//! seeded rules are those of AAP 0.3.6. Every amount, rate and points value is
//! built from integers, never from a binary floating-point number.
//!
//! The domain crate has no async runtime, not even as a dev-dependency (R3):
//! the store's futures are ready when created, and a busy-polling `block_on`
//! drives the accrual.

use std::future::{self, Future};
use std::hint::{self, black_box};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use carddemo_domain::clock::{Clock, FixedClock};
use carddemo_domain::entities::{NewAccrual, RewardAccrual, RewardBalance, RewardRule};
use carddemo_domain::numeric::{
    AccountId, AccrualId, Money, RewardPoints, RewardRate, RuleId, TranCategoryCode, TranTypeCode,
    TransactionRowId,
};
use carddemo_domain::ports::{PortError, PortResult};
use carddemo_domain::rewards::engine::{credit, points_for, select_rule};
use carddemo_domain::rewards::{AccrualInput, RewardEngine, RewardStore};
use chrono::{DateTime, TimeZone, Utc};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rust_decimal::Decimal;

/// `TRAN-AMT` of the fixture transaction in cents: `00000005047G` is +50.47.
const FIXTURE_AMOUNT_CENTS: i64 = 5_047;

/// The largest `S9(09)V99` amount in cents: 999999999.99.
const MAX_AMOUNT_CENTS: i64 = 99_999_999_999;

/// The points the fixture transaction earns under the retail seed rule:
/// `ceil(50.47 × 1.0000)`.
const SEED_POINTS: i64 = 51;

/// The sizes of the generated rule sets, the two seed rules included.
const RULE_SET_SIZES: [usize; 3] = [10, 100, 1_000];

/// The retail seed rate, 1.0000, in units of 0.0001.
const RETAIL_RATE_UNITS: i64 = 10_000;

/// The travel seed rate, 3.0000, in units of 0.0001.
const TRAVEL_RATE_UNITS: i64 = 30_000;

/// The fixture pair: type `01` Purchase (`trantype.txt` L1) with category
/// `0001` Regular Sales Draft (`trancatg.txt` L1).
const RETAIL_PAIR: Pair = ("01", 1);

/// No amount bound at either end.
const UNBOUNDED: Bounds = (None, None);

/// A rule's type code and category code.
type Pair = (&'static str, u16);

/// A rule's inclusive minimum and maximum amounts; `None` is unbounded.
type Bounds = (Option<Money<9>>, Option<Money<9>>);

/// A rule's `active_from` (inclusive) and `active_to` (exclusive) instants.
type Window = (DateTime<Utc>, Option<DateTime<Utc>>);

/// The output of `future`, polled with a no-op waker until it is ready.
///
/// Every `BenchStore` future is ready when it is created, so an accrual
/// completes on its first poll; the loop keeps the executor correct for a
/// future that returns `Pending` before it is done.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        hint::spin_loop();
    }
}

/// The reward tables in memory, as `RewardEngine::accrue` reads and writes
/// them through `RewardStore`.
#[derive(Debug)]
struct BenchStore {
    /// The `reward_rules` rows.
    rules: Vec<RewardRule>,
    /// The `reward_balances` rows, one per account.
    balances: Vec<RewardBalance>,
    /// The `accrual_id` the next inserted accrual receives.
    next_accrual_id: i64,
}

impl BenchStore {
    /// A store holding `rules` and no balance; accrual identities start at 1,
    /// as the identity column's do.
    fn with_rules(rules: Vec<RewardRule>) -> Self {
        Self {
            rules,
            balances: Vec::new(),
            next_accrual_id: 1,
        }
    }

    /// Credits `points` to the balance of `account_id`, which starts at zero
    /// when the account has none, and returns the updated balance.
    fn credit_balance(
        &mut self,
        account_id: AccountId,
        points: RewardPoints,
    ) -> PortResult<RewardBalance> {
        let position = self
            .balances
            .iter()
            .position(|balance| balance.account_id == account_id);
        let current = position.map_or_else(
            || RewardBalance::zero(account_id),
            |index| self.balances[index].clone(),
        );
        // The port contract reports an earned total beyond the BIGINT column
        // as `Unavailable`.
        let updated = credit(&current, points).map_err(|overflow| {
            PortError::Unavailable(format!("reward_balances.total_points_earned: {overflow}"))
        })?;
        match position {
            Some(index) => self.balances[index].clone_from(&updated),
            None => self.balances.push(updated.clone()),
        }
        Ok(updated)
    }
}

impl RewardStore for BenchStore {
    /// Every rule of the pair, whatever its window and bounds: the port
    /// contract leaves the match to the engine.
    fn candidate_rules(
        &mut self,
        type_cd: TranTypeCode,
        category_cd: TranCategoryCode,
    ) -> impl Future<Output = PortResult<Vec<RewardRule>>> + Send + '_ {
        let rules = self
            .rules
            .iter()
            .filter(|rule| {
                rule.transaction_type_cd == type_cd && rule.transaction_category_cd == category_cd
            })
            .cloned()
            .collect();
        future::ready(Ok(rules))
    }

    /// Assigns the next `accrual_id` and keeps nothing else, so memory stays
    /// flat however many iterations run.
    ///
    /// Every iteration accrues the same `transaction_id`, so this store
    /// deliberately does not enforce the `UNIQUE (transaction_id)` constraint
    /// that `PgRewardStore` reports as `PortError::Duplicate`.
    fn insert_accrual(
        &mut self,
        _accrual: NewAccrual,
    ) -> impl Future<Output = PortResult<AccrualId>> + Send + '_ {
        let accrual_id = AccrualId::from(self.next_accrual_id);
        self.next_accrual_id += 1;
        future::ready(Ok(accrual_id))
    }

    fn add_points(
        &mut self,
        account_id: AccountId,
        points: RewardPoints,
    ) -> impl Future<Output = PortResult<RewardBalance>> + Send + '_ {
        future::ready(self.credit_balance(account_id, points))
    }
}

/// The UTC instant at `hour`:00:00 of the given day.
fn utc(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
        .single()
        .unwrap_or_else(|| {
            panic!("{year}-{month:02}-{day:02}T{hour:02}:00:00Z is not a valid UTC instant")
        })
}

/// The processing instant of every accrual, read from a `FixedClock` as the
/// callers read theirs: 2022-07-01T12:00:00Z, the instant the TRANREPT parity
/// test pins (AAP 0.9.3), inside the seeded rules' window.
fn processing_instant() -> DateTime<Utc> {
    FixedClock(utc(2022, 7, 1, 12)).now()
}

/// The `S9(09)V99` amount of `cents` hundredths.
fn money(cents: i64) -> Money<9> {
    Money::from_cents(cents)
        .unwrap_or_else(|error| panic!("{cents} cents is not an S9(09)V99 amount: {error}"))
}

/// The reward rate of `units` units of 0.0001.
fn reward_rate(units: i64) -> RewardRate {
    RewardRate::from_decimal(Decimal::new(units, 4))
        .unwrap_or_else(|error| panic!("{units} units of 0.0001 is not a reward rate: {error}"))
}

/// The `9(04)` category code `code`.
fn category(code: u16) -> TranCategoryCode {
    TranCategoryCode::new(code)
        .unwrap_or_else(|error| panic!("{code} is not a 9(04) category code: {error}"))
}

/// The fixture transaction, record 1 of `dailytran.txt` (type `01`, category
/// `0001`, 50.47), as row 1 credited to account `00000000001`.
fn fixture_input() -> AccrualInput {
    let (type_cd, category_cd) = RETAIL_PAIR;
    AccrualInput {
        transaction_id: TransactionRowId::from(1),
        account_id: AccountId::new(1)
            .unwrap_or_else(|error| panic!("1 is not a 9(11) account number: {error}")),
        type_cd: TranTypeCode::move_from(type_cd),
        category_cd: category(category_cd),
        amount: money(FIXTURE_AMOUNT_CENTS),
    }
}

/// A reward rule of `pair` at `rate_units` units of 0.0001.
fn rule(
    rule_id: i64,
    (type_cd, category_cd): Pair,
    rate_units: i64,
    (min_transaction_amount, max_transaction_amount): Bounds,
    (active_from, active_to): Window,
) -> RewardRule {
    RewardRule {
        rule_id: RuleId::from(rule_id),
        transaction_type_cd: TranTypeCode::move_from(type_cd),
        transaction_category_cd: category(category_cd),
        reward_rate: reward_rate(rate_units),
        min_transaction_amount,
        max_transaction_amount,
        active_from,
        active_to,
    }
}

/// The rules `batch seed` inserts (AAP 0.3.6), both unbounded and active from
/// 2022-01-01T00:00:00Z without end:
///
/// 1. retail purchase, the fixture pair, at 1.0000;
/// 2. travel, type `04` Authorization (`trantype.txt` L4) with category
///    `0003` Travel booking authorization (`trancatg.txt` L14), at 3.0000.
fn seed_rules() -> Vec<RewardRule> {
    let window = (utc(2022, 1, 1, 0), None);
    vec![
        rule(1, RETAIL_PAIR, RETAIL_RATE_UNITS, UNBOUNDED, window),
        rule(2, ("04", 3), TRAVEL_RATE_UNITS, UNBOUNDED, window),
    ]
}

/// The seed rules followed by generated rules 3 to `total`, built to drive
/// every branch of the rule selection (AAP 0.3.5) against the fixture
/// transaction:
///
/// - pair: three rules in four are on the fixture pair, the rest on other
///   `trancatg.txt` pairs, which the store filters out;
/// - bounds, in a cycle of five: none; a minimum of 10.00; a maximum of 25.00,
///   below the amount; 100.00 to 1000.00, above it; and exactly 50.47 at both
///   ends, which matches because both bounds are inclusive;
/// - window, in a cycle of three: active from 2022-01-01; soft-deleted, from
///   2021-01-01 to 2022-01-01; and future, from 2023-01-01;
/// - rate: a rule in range and active gets 0.5000 to 1.3750 in steps of
///   0.1250, any other rule 2.0000 to 2.8750, so the engine must reject every
///   higher-rated rule; the cycle of eight repeats rates, so ties between
///   equal rates go to the lowest `rule_id`.
fn scaled_rules(total: usize) -> Vec<RewardRule> {
    let last_rule_id = i64::try_from(total)
        .unwrap_or_else(|error| panic!("{total} rules exceed the i64 rule identities: {error}"));
    let mut rules = seed_rules();
    rules.extend((3..=last_rule_id).map(generated_rule));
    rules
}

/// Generated rule `rule_id` of `scaled_rules`.
fn generated_rule(rule_id: i64) -> RewardRule {
    let index = rule_id - 3;
    let pair = if index % 4 == 3 {
        match (index / 4) % 4 {
            0 => ("01", 2),
            1 => ("02", 1),
            2 => ("03", 1),
            _ => ("05", 1),
        }
    } else {
        RETAIL_PAIR
    };
    let (bounds, in_range) = match index % 5 {
        0 => (UNBOUNDED, true),
        1 => ((Some(money(1_000)), None), true),
        2 => ((None, Some(money(2_500))), false),
        3 => ((Some(money(10_000)), Some(money(100_000))), false),
        _ => {
            let exact = Some(money(FIXTURE_AMOUNT_CENTS));
            ((exact, exact), true)
        }
    };
    let (window, active) = match index % 3 {
        0 => ((utc(2022, 1, 1, 0), None), true),
        1 => ((utc(2021, 1, 1, 0), Some(utc(2022, 1, 1, 0))), false),
        _ => ((utc(2023, 1, 1, 0), None), false),
    };
    let base_units = if in_range && active { 5_000 } else { 20_000 };
    let rate_units = base_units + (index % 8) * 1_250;
    rule(rule_id, pair, rate_units, bounds, window)
}

/// One benchmarked rule set: its benchmark parameter and its rules.
struct RuleSet {
    /// `seed`, or the number of rules.
    label: String,
    /// The `reward_rules` rows of the set.
    rules: Vec<RewardRule>,
}

impl RuleSet {
    /// The rules of the set that the store returns for the pair of `input`.
    fn candidates(&self, input: AccrualInput) -> Vec<RewardRule> {
        let mut store = BenchStore::with_rules(self.rules.clone());
        block_on(store.candidate_rules(input.type_cd, input.category_cd))
            .unwrap_or_else(|error| panic!("{}: the candidate read failed: {error}", self.label))
    }

    /// Accrues `input` at `at` once, on a fresh store holding the set, and
    /// checks that the timed path is the matching path: the winning rule has
    /// the input's pair, is active at `at` and bounds the amount; it earns at
    /// least the retail rule's points; the balance is credited with them; and
    /// `select_rule` picks the same rule from the store's candidates.
    fn check_matching_path(&self, input: AccrualInput, at: DateTime<Utc>) -> RewardAccrual {
        let label = &self.label;
        let mut store = BenchStore::with_rules(self.rules.clone());
        let accrual = block_on(RewardEngine::accrue(&mut store, input, at))
            .unwrap_or_else(|error| panic!("{label}: the accrual failed: {error}"))
            .unwrap_or_else(|| panic!("{label}: no rule matched the fixture transaction"));

        let rule_id = accrual.rule_id;
        let winner = self
            .rules
            .iter()
            .find(|rule| rule.rule_id == rule_id)
            .unwrap_or_else(|| panic!("{label}: the set holds no rule {rule_id}"));
        assert!(
            winner.transaction_type_cd == input.type_cd
                && winner.transaction_category_cd == input.category_cd
                && winner.is_active_at(at)
                && winner
                    .min_transaction_amount
                    .is_none_or(|min| min <= input.amount)
                && winner
                    .max_transaction_amount
                    .is_none_or(|max| max >= input.amount),
            "{label}: rule {rule_id} won without matching the fixture transaction"
        );

        let points = accrual.points_earned.to_i64();
        assert!(
            points >= SEED_POINTS,
            "{label}: {points} points, fewer than the retail rule's {SEED_POINTS}"
        );
        let credited = store
            .balances
            .iter()
            .find(|balance| balance.account_id == input.account_id)
            .map(|balance| balance.available_points);
        assert_eq!(
            credited,
            Some(accrual.points_earned),
            "{label}: the balance was not credited with the accrual"
        );

        let candidates = self.candidates(input);
        let selected = select_rule(&candidates, &input, at).map(|rule| rule.rule_id);
        assert_eq!(
            selected,
            Some(rule_id),
            "{label}: select_rule disagrees with the accrual"
        );
        accrual
    }
}

/// The seeded rules and the generated sets of `RULE_SET_SIZES`, each checked
/// on the matching path; the seed set must accrue 51 points under rule 1.
fn rule_sets(input: AccrualInput, at: DateTime<Utc>) -> Vec<RuleSet> {
    let seed = RuleSet {
        label: "seed".to_owned(),
        rules: seed_rules(),
    };
    let accrual = seed.check_matching_path(input, at);
    assert_eq!(
        accrual.rule_id,
        RuleId::from(1),
        "seed: the retail rule must win"
    );
    assert_eq!(
        accrual.points_earned.to_i64(),
        SEED_POINTS,
        "seed: the retail rule earns ceil(50.47 × 1.0000) points"
    );

    let mut sets = vec![seed];
    for total in RULE_SET_SIZES {
        let set = RuleSet {
            label: total.to_string(),
            rules: scaled_rules(total),
        };
        set.check_matching_path(input, at);
        sets.push(set);
    }
    sets
}

/// `reward_engine/accrue/{seed,10,100,1000}`: one whole accrual per iteration.
fn bench_accrue(c: &mut Criterion) {
    let input = fixture_input();
    let at = processing_instant();
    let sets = rule_sets(input, at);
    let mut group = c.benchmark_group("reward_engine/accrue");
    group.throughput(Throughput::Elements(1));
    for set in sets {
        let mut store = BenchStore::with_rules(set.rules);
        group.bench_function(BenchmarkId::from_parameter(&set.label), |b| {
            b.iter(|| {
                black_box(block_on(RewardEngine::accrue(
                    black_box(&mut store),
                    black_box(input),
                    black_box(at),
                )))
            });
        });
    }
    group.finish();
}

/// `reward_engine/select_rule/{seed,10,100,1000}`: the rule selection over the
/// fixture pair's candidates.
fn bench_select_rule(c: &mut Criterion) {
    let input = fixture_input();
    let at = processing_instant();
    let sets = rule_sets(input, at);
    let mut group = c.benchmark_group("reward_engine/select_rule");
    for set in sets {
        let candidates = set.candidates(input);
        group.bench_with_input(
            BenchmarkId::from_parameter(&set.label),
            candidates.as_slice(),
            |b, candidates| {
                b.iter(|| {
                    select_rule(black_box(candidates), black_box(&input), black_box(at))
                        .map(|rule| rule.rule_id)
                });
            },
        );
    }
    group.finish();
}

/// `reward_engine/points_for/<rate>/<amount>`: the points of the smallest, the
/// fixture and the largest `S9(09)V99` amounts at 1.0000, 1.5000 and 3.0000.
fn bench_points_for(c: &mut Criterion) {
    let mut group = c.benchmark_group("reward_engine/points_for");
    for rate in [RETAIL_RATE_UNITS, 15_000, TRAVEL_RATE_UNITS].map(reward_rate) {
        for amount in [1, FIXTURE_AMOUNT_CENTS, MAX_AMOUNT_CENTS].map(money) {
            let points = points_for(amount, rate)
                .unwrap_or_else(|overflow| panic!("{amount} at {rate}: {overflow}"));
            assert!(
                points > RewardPoints::ZERO,
                "{amount} at {rate} earned no points"
            );
            group.bench_with_input(
                BenchmarkId::new(rate.to_string(), amount),
                &(amount, rate),
                |b, &(amount, rate)| {
                    b.iter(|| points_for(black_box(amount), black_box(rate)));
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_accrue, bench_select_rule, bench_points_for);
criterion_main!(benches);
