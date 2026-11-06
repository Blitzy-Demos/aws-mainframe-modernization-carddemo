// The reward rule sets the reward benchmark times: the seeded rules and generated sets of
// 10, 100 and 1000 rules, the synthetic purchase accrued against them, the processing
// instant, and the grid of `points_for` amounts and rates.
//
// The benchmark (crates/domain/benches/reward_engine.rs) and the reward tests
// (crates/domain/tests/reward_engine.rs) both include this file as a module by path, so the
// rule sets the benchmark times are the rule sets the tests check, and the benchmark asserts
// nothing. Both including crates compile it with dead-code warnings denied, so every item
// here is used by each of them.
//
// Every amount, rate and points value is built from integers, never from a binary
// floating-point number.

use carddemo_domain::clock::{Clock, FixedClock};
use carddemo_domain::entities::RewardRule;
use carddemo_domain::numeric::{
    AccountId, Money, RewardRate, RuleId, TranCategoryCode, TranTypeCode, TransactionRowId,
};
use carddemo_domain::rewards::AccrualInput;
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;

/// The amount of the synthetic purchase in cents, 50.47. It is chosen, not
/// read from a fixture: record 1 of `app/data/ASCII/dailytran.txt`, whose type
/// and category the purchase takes, has `TRAN-AMT` `0000005047G`, that is
/// +504.77.
const PURCHASE_AMOUNT_CENTS: i64 = 5_047;

/// The largest `S9(09)V99` amount in cents: 999999999.99.
const MAX_AMOUNT_CENTS: i64 = 99_999_999_999;

/// The sizes of the generated rule sets, the two seed rules included.
const RULE_SET_SIZES: [usize; 3] = [10, 100, 1_000];

/// The retail seed rate, 1.0000, in units of 0.0001.
const RETAIL_RATE_UNITS: i64 = 10_000;

/// The travel seed rate, 3.0000, in units of 0.0001.
const TRAVEL_RATE_UNITS: i64 = 30_000;

/// The rates of the `points_for` grid in units of 0.0001: the two seeded
/// rates, 1.0000 and 3.0000, and 1.5000 between them.
pub const POINTS_FOR_RATE_UNITS: [i64; 3] = [RETAIL_RATE_UNITS, 15_000, TRAVEL_RATE_UNITS];

/// The amounts of the `points_for` grid in cents: the smallest positive
/// `S9(09)V99` amount, 0.01; the synthetic purchase, 50.47; and the largest,
/// 999999999.99.
pub const POINTS_FOR_AMOUNT_CENTS: [i64; 3] = [1, PURCHASE_AMOUNT_CENTS, MAX_AMOUNT_CENTS];

/// The retail pair, the pair of the retail seed rule: type `01` Purchase
/// (`trantype.txt` L1) with category `0001` Regular Sales Draft
/// (`trancatg.txt` L1).
const RETAIL_PAIR: Pair = ("01", 1);

/// No amount bound at either end.
const UNBOUNDED: Bounds = (None, None);

/// A rule's type code and category code.
type Pair = (&'static str, u16);

/// A rule's inclusive minimum and maximum amounts; `None` is unbounded.
type Bounds = (Option<Money<9>>, Option<Money<9>>);

/// A rule's `active_from` (inclusive) and `active_to` (exclusive) instants.
type Window = (DateTime<Utc>, Option<DateTime<Utc>>);

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
pub fn processing_instant() -> DateTime<Utc> {
    FixedClock(utc(2022, 7, 1, 12)).now()
}

/// The `S9(09)V99` amount of `cents` hundredths.
pub fn money(cents: i64) -> Money<9> {
    Money::from_cents(cents)
        .unwrap_or_else(|error| panic!("{cents} cents is not an S9(09)V99 amount: {error}"))
}

/// The reward rate of `units` units of 0.0001.
pub fn reward_rate(units: i64) -> RewardRate {
    RewardRate::from_decimal(Decimal::new(units, 4))
        .unwrap_or_else(|error| panic!("{units} units of 0.0001 is not a reward rate: {error}"))
}

/// The `9(04)` category code `code`.
fn category(code: u16) -> TranCategoryCode {
    TranCategoryCode::new(code)
        .unwrap_or_else(|error| panic!("{code} is not a 9(04) category code: {error}"))
}

/// The synthetic purchase every rule set is accrued against: 50.47 of the
/// retail pair on transaction row 1, credited to account `00000000001`.
///
/// Only its type `01` and category `0001` are those of record 1 of
/// `app/data/ASCII/dailytran.txt`; that record's own `TRAN-AMT`
/// `0000005047G` is +504.77.
pub fn purchase_input() -> AccrualInput {
    let (type_cd, category_cd) = RETAIL_PAIR;
    AccrualInput {
        transaction_id: TransactionRowId::from(1),
        account_id: AccountId::new(1)
            .unwrap_or_else(|error| panic!("1 is not a 9(11) account number: {error}")),
        type_cd: TranTypeCode::move_from(type_cd),
        category_cd: category(category_cd),
        amount: money(PURCHASE_AMOUNT_CENTS),
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
/// 1. retail purchase, the retail pair, at 1.0000;
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
/// every branch of the rule selection (AAP 0.3.5) against the synthetic
/// purchase:
///
/// - pair: three rules in four are on the retail pair, the rest on other
///   `trancatg.txt` pairs, which the store filters out;
/// - bounds, in a cycle of five: none; a minimum of 10.00; a maximum of 25.00,
///   below the amount; 100.00 to 1000.00, above it; and exactly the purchase
///   amount, 50.47, at both ends, which matches because both bounds are
///   inclusive;
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
            let exact = Some(money(PURCHASE_AMOUNT_CENTS));
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
pub struct RuleSet {
    /// `seed`, or the number of rules.
    pub label: String,
    /// The `reward_rules` rows of the set.
    pub rules: Vec<RewardRule>,
}

/// The seeded rules, labelled `seed`, followed by the generated sets of
/// `RULE_SET_SIZES`, each labelled with its number of rules.
pub fn rule_sets() -> Vec<RuleSet> {
    let seed = RuleSet {
        label: "seed".to_owned(),
        rules: seed_rules(),
    };
    let mut sets = vec![seed];
    sets.extend(RULE_SET_SIZES.map(|total| RuleSet {
        label: total.to_string(),
        rules: scaled_rules(total),
    }));
    sets
}
