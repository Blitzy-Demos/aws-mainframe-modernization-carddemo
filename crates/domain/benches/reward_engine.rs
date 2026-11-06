//! Criterion benchmarks of `RewardEngine::accrue`, the one reward calculation
//! of the system (request Rewards system; AAP 0.3.5, 0.6.4).
//!
//! Rewards have no COBOL oracle (AAP 0.9.2), and this benchmark only times: it
//! checks no result. Its rule sets, purchase and `points_for` grid come from
//! `tests/support/reward_rule_sets.rs`, which `tests/reward_engine.rs` includes
//! too, so the matching-path checks of these exact rule sets live in that test:
//! on each set the purchase accrues under its hand-derived rule and points, the
//! balance is credited with them and `select_rule` picks the same rule, and each
//! point of the grid earns its rounded-up product.
//!
//! - `reward_engine/accrue`: the whole accrual, steps (1) to (4), against an
//!   in-memory store, for the seeded rules and for 10, 100 and 1000 rules.
//! - `reward_engine/select_rule`: the rule selection alone, steps (2) and (3),
//!   over the candidates the store returns for the retail pair.
//! - `reward_engine/points_for`: `ceil(amount × rate)`, step (4), for the
//!   smallest `Money<9>` amount, the purchase amount and the largest at the
//!   two seeded rates and an intermediate one.
//!
//! The purchase is synthetic: 50.47 of type `01`, category `0001`. It takes
//! only its type and category from record 1 of `app/data/ASCII/dailytran.txt`,
//! whose own `TRAN-AMT` `0000005047G` is +504.77. The seeded rules are those
//! of AAP 0.3.6.
//!
//! The domain crate has no async runtime, not even as a dev-dependency (R3):
//! the store's futures are ready when created, and a busy-polling `block_on`
//! drives the accrual.

#[path = "../tests/support/reward_rule_sets.rs"]
mod reward_rule_sets;

use std::future::{self, Future};
use std::hint::{self, black_box};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use carddemo_domain::entities::{NewAccrual, RewardBalance, RewardRule};
use carddemo_domain::numeric::{
    AccountId, AccrualId, RewardPoints, TranCategoryCode, TranTypeCode,
};
use carddemo_domain::ports::{PortError, PortResult};
use carddemo_domain::rewards::engine::{credit, points_for, select_rule};
use carddemo_domain::rewards::{AccrualInput, RewardEngine, RewardStore};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use reward_rule_sets::{
    POINTS_FOR_AMOUNT_CENTS, POINTS_FOR_RATE_UNITS, RuleSet, money, processing_instant,
    purchase_input, reward_rate, rule_sets,
};

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

/// The rules of `set` that the store returns for the pair of `input`.
fn candidates(set: &RuleSet, input: AccrualInput) -> Vec<RewardRule> {
    let mut store = BenchStore::with_rules(set.rules.clone());
    block_on(store.candidate_rules(input.type_cd, input.category_cd))
        .unwrap_or_else(|error| panic!("{}: the candidate read failed: {error}", set.label))
}

/// `reward_engine/accrue/{seed,10,100,1000}`: one whole accrual per iteration.
fn bench_accrue(c: &mut Criterion) {
    let input = purchase_input();
    let at = processing_instant();
    let mut group = c.benchmark_group("reward_engine/accrue");
    group.throughput(Throughput::Elements(1));
    for set in rule_sets() {
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
/// retail pair's candidates.
fn bench_select_rule(c: &mut Criterion) {
    let input = purchase_input();
    let at = processing_instant();
    let mut group = c.benchmark_group("reward_engine/select_rule");
    for set in rule_sets() {
        let candidates = candidates(&set, input);
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

/// `reward_engine/points_for/<rate>/<amount>`: the points of the smallest
/// `S9(09)V99` amount, the purchase amount and the largest at 1.0000, 1.5000
/// and 3.0000.
fn bench_points_for(c: &mut Criterion) {
    let mut group = c.benchmark_group("reward_engine/points_for");
    for rate in POINTS_FOR_RATE_UNITS.map(reward_rate) {
        for amount in POINTS_FOR_AMOUNT_CENTS.map(money) {
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
