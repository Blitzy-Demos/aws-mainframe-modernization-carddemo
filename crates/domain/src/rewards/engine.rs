//! The reward engine: the one reward calculation of the system (request
//! Rewards system; AAP 0.3.5, and the shared component "Reward calculation"
//! of AAP 0.6.4).
//!
//! Rewards are a new feature (o8) with no COBOL oracle: the README lists DB2
//! Rewards only as a roadmap item [README.md:L357-369], so no parity audit row
//! gates this module, and each function names the rewards step it implements
//! instead of a COBOL paragraph.
//!
//! [`RewardEngine::accrue`] has exactly two callers, each running it inside the
//! database transaction that records the transaction itself:
//!
//! - CT02 `create_transaction` (`crates/api/src/handlers/transactions.rs`):
//!   allocate the TRAN-ID, insert the transaction, accrue, commit.
//! - POSTTRAN (`crates/batch/src/jobs/posttran.rs`): one database transaction
//!   per posted record, covering the posting updates and the accrual.
//!
//! Neither caller re-implements a step. [`select_rule`], [`points_for`] and
//! [`credit`] are public so that the tests, the bench and both reward stores
//! compute exactly what the engine computes.
//!
//! # Steps (AAP 0.3.5)
//!
//! 1. An amount of zero or less accrues nothing.
//! 2. A rule matches when its type and category codes equal the transaction's,
//!    it is active at the processing instant, and the amount lies within its
//!    bounds; both bounds are inclusive and an absent bound is unbounded.
//! 3. Among the matching rules the highest `reward_rate` wins, and equal rates
//!    go to the lowest `rule_id`. No match accrues nothing.
//! 4. `points = ceil(amount × reward_rate)`; the accrual is recorded and the
//!    account's balance credited.
//!
//! # Arithmetic
//!
//! The amount is the stored `TRAN-AMT PIC S9(09)V99` [app/cpy/CVTRA05Y.cpy:L10]
//! and the rate has at most 4 fraction digits, so their product is exact at
//! scale 6 ([`PointsExpr`](crate::numeric::PointsExpr)) and `ceil` is the only
//! rounding: no intermediate rounding and no other mode (AAP 0.9.2). Nothing
//! here redeems points or knows tiers (AAP 0.2.5).

use std::cmp::Reverse;

use chrono::{DateTime, Utc};

use super::RewardError;
use super::ports::RewardStore;
use crate::entities::{NewAccrual, RewardAccrual, RewardBalance, RewardRule, Transaction};
use crate::numeric::{
    AccountId, Money, Overflow, RewardPoints, RewardRate, TranCategoryCode, TranTypeCode,
    TransactionRowId,
};

/// The facts of one transaction the reward rules look at (AAP 0.3.5).
///
/// Every field is a key or value the caller already holds when it records the
/// transaction, so the engine reads nothing to build it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccrualInput {
    /// The row identity of the recorded transaction,
    /// `transactions.transaction_id`: the target of the
    /// `reward_accruals.transaction_id` foreign key. It is not the COBOL
    /// TRAN-ID, which a later add may issue again (E-09).
    pub transaction_id: TransactionRowId,
    /// The account credited: CT02's account resolved from the card
    /// cross-reference, POSTTRAN's `XREF-ACCT-ID`. The transaction record
    /// `CVTRA05Y` holds no account ID.
    pub account_id: AccountId,
    /// `TRAN-TYPE-CD PIC X(02)` [app/cpy/CVTRA05Y.cpy:L6], matched against
    /// `reward_rules.transaction_type_cd`.
    pub type_cd: TranTypeCode,
    /// `TRAN-CAT-CD PIC 9(04)` [app/cpy/CVTRA05Y.cpy:L7], matched against
    /// `reward_rules.transaction_category_cd`.
    pub category_cd: TranCategoryCode,
    /// `TRAN-AMT PIC S9(09)V99` [app/cpy/CVTRA05Y.cpy:L10]: the stored amount,
    /// compared with the rule bounds and multiplied by the winning rate.
    pub amount: Money<9>,
}

impl AccrualInput {
    /// The input for the stored row `transaction`, credited to `account_id`.
    ///
    /// The row identity, type code, category code and amount move across
    /// unchanged. The account comes from the caller because `CVTRA05Y` holds
    /// none: CT02 passes the account of the card cross-reference, POSTTRAN
    /// the `XREF-ACCT-ID` its lookup read.
    #[must_use]
    pub fn for_transaction(transaction: &Transaction, account_id: AccountId) -> Self {
        Self {
            transaction_id: transaction.transaction_id,
            account_id,
            type_cd: transaction.tran_type_cd,
            category_cd: transaction.tran_cat_cd,
            amount: transaction.tran_amt,
        }
    }
}

/// The reward engine (AAP 0.3.5): a pure domain service whose only state is
/// the store its caller lends it for one accrual.
#[derive(Debug, Clone, Copy, Default)]
pub struct RewardEngine;

impl RewardEngine {
    /// Rewards steps (1) to (4): accrues the points `input` earns at instant
    /// `at`, records the accrual and credits the account's balance through
    /// `store` (AAP 0.3.5).
    ///
    /// `at` is the processing clock, which the caller reads with
    /// [`Clock::now`](crate::clock::Clock::now), so `CARDDEMO_CLOCK_OVERRIDE`
    /// pins it (AAP 0.6.4). The engine reads no clock itself: `at` decides
    /// which rules are active and becomes the accrual's `created_at`.
    ///
    /// Returns `Ok(None)`, with nothing written, when the amount is zero or
    /// less (before any store call) or when no rule matches. Otherwise it
    /// returns the stored accrual. A winning rule of rate 0 still records an
    /// accrual of zero points and credits zero points, because no step drops
    /// a zero result.
    ///
    /// The store calls run in the caller's open database transaction, and the
    /// engine neither commits nor rolls back.
    ///
    /// # Errors
    ///
    /// - [`RewardError::Store`] with the port's error, unchanged, when a store
    ///   call fails: [`PortError::Duplicate`] when the transaction already has
    ///   an accrual, [`PortError::Unavailable`] on any other failure. A failed
    ///   accrual insert credits no points.
    /// - [`RewardError::PointsOverflow`] when the points fall outside the
    ///   range of [`RewardPoints`].
    ///
    /// On any error CT02 rolls back the whole request and answers 500, and
    /// POSTTRAN rolls back the record, prints its record number and TRAN-ID to
    /// stderr and exits with code 8, adding nothing to `DALYREJS`
    /// (AAP 0.3.5, 0.5.3.5).
    ///
    /// [`PortError::Duplicate`]: crate::ports::PortError::Duplicate
    /// [`PortError::Unavailable`]: crate::ports::PortError::Unavailable
    pub async fn accrue<S: RewardStore + Send>(
        store: &mut S,
        input: AccrualInput,
        at: DateTime<Utc>,
    ) -> Result<Option<RewardAccrual>, RewardError> {
        // Step (1): credits and refunds never accrue, and reach no store call.
        if input.amount <= Money::<9>::ZERO {
            return Ok(None);
        }

        // Steps (2) and (3). Only the winner's identity and rate are kept, so
        // nothing borrowed from the candidates lives across the awaits below.
        let rules = store
            .candidate_rules(input.type_cd, input.category_cd)
            .await?;
        let Some((rule_id, reward_rate)) =
            select_rule(&rules, &input, at).map(|rule| (rule.rule_id, rule.reward_rate))
        else {
            return Ok(None);
        };

        // Step (4).
        let points_earned = points_for(input.amount, reward_rate)?;
        let accrual = NewAccrual {
            account_id: input.account_id,
            transaction_id: input.transaction_id,
            rule_id,
            points_earned,
            created_at: at,
        };
        let accrual_id = store.insert_accrual(accrual.clone()).await?;
        store.add_points(input.account_id, points_earned).await?;
        Ok(Some(RewardAccrual::from_new(accrual_id, &accrual)))
    }
}

/// Rewards steps (2) and (3): the rule among `rules` that `input` accrues
/// under at instant `at`, or `None` when no rule matches (AAP 0.3.5).
///
/// A rule matches when its `transaction_type_cd` and
/// `transaction_category_cd` equal the input's codes, it
/// [is active](RewardRule::is_active_at) at `at`, its `min_transaction_amount`
/// is absent or at most the amount, and its `max_transaction_amount` is absent
/// or at least the amount. The pair is checked here even though the store
/// already filters by it, so the match is decided in this one place.
///
/// Among the matches the highest `reward_rate` wins, and equal rates go to
/// the lowest `rule_id`; the result does not depend on the order of `rules`.
#[must_use]
pub fn select_rule<'r>(
    rules: &'r [RewardRule],
    input: &AccrualInput,
    at: DateTime<Utc>,
) -> Option<&'r RewardRule> {
    rules
        .iter()
        .filter(|rule| rule_matches(rule, input, at))
        .min_by_key(|rule| (Reverse(rule.reward_rate), rule.rule_id))
}

/// Rewards step (2) for one rule: the pair, the active window and the
/// inclusive amount bounds.
fn rule_matches(rule: &RewardRule, input: &AccrualInput, at: DateTime<Utc>) -> bool {
    rule.transaction_type_cd == input.type_cd
        && rule.transaction_category_cd == input.category_cd
        && rule.is_active_at(at)
        && rule
            .min_transaction_amount
            .is_none_or(|min| min <= input.amount)
        && rule
            .max_transaction_amount
            .is_none_or(|max| max >= input.amount)
}

/// Rewards step (4): the points `amount` earns at `rate`,
/// `ceil(amount × rate)` (AAP 0.3.5).
///
/// The product is exact and `ceil` is the only rounding (AAP 0.9.2): 50.47 at
/// rate 1.0000 earns 51 points, 0.01 at rate 0.0001 earns 1. Callers pass a
/// positive amount, as [`RewardEngine::accrue`] does after step (1).
///
/// # Errors
///
/// [`Overflow`] when the rounded product lies outside the range of
/// [`RewardPoints`]: above `i64::MAX`, which no `S9(09)V99` amount reaches at
/// any reward rate, or below zero, which only an amount of -1.00 or less
/// reaches.
pub fn points_for(amount: Money<9>, rate: RewardRate) -> Result<RewardPoints, Overflow> {
    (amount * rate).ceil()
}

/// Rewards step (4) for the balance: `balance` after a credit of `points`
/// (AAP 0.3.5).
///
/// `total_points_earned` grows by `points`, `total_points_redeemed` is kept
/// (nothing redeems points, so it is never written above 0), and
/// `available_points` becomes `total_points_earned - total_points_redeemed`.
/// This is the engine's computation of the invariant the
/// `reward_balances_available_points_check` constraint enforces, which the
/// stores apply in `RewardStore::add_points`.
///
/// # Errors
///
/// [`Overflow`] when the earned total would exceed `i64::MAX`, or when the
/// balance's redeemed total exceeds its new earned total.
pub fn credit(balance: &RewardBalance, points: RewardPoints) -> Result<RewardBalance, Overflow> {
    let earned = balance.total_points_earned.checked_add(points)?;
    RewardBalance::new(balance.account_id, earned, balance.total_points_redeemed)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeDelta, TimeZone, Utc};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::{AccrualInput, RewardEngine, credit, points_for, select_rule};
    use crate::entities::{RewardAccrual, RewardBalance, RewardRule, Transaction};
    use crate::numeric::{
        AccountId, AccrualId, Alnum, CardNumber, MerchantId, Money, Overflow, RewardPoints,
        RewardRate, RuleId, TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
    };
    use crate::ports::PortError;
    use crate::rewards::RewardError;
    use crate::rewards::ports::memory::{MemoryStore, poll_ready};

    /// Compiles only when `T` is `Send`, as the multi-threaded runtime of both
    /// callers requires of the accrual future.
    fn assert_send<T: Send>(_: &T) {}

    fn instant(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .expect("a valid UTC instant")
    }

    /// 2022-01-01T00:00:00Z, the `active_from` of the seed rules (AAP 0.3.6).
    fn seed_start() -> DateTime<Utc> {
        instant(2022, 1, 1, 0)
    }

    /// The processing clock the parity runs pin, 2022-07-01T12:00:00Z.
    fn processing_clock() -> DateTime<Utc> {
        instant(2022, 7, 1, 12)
    }

    fn amount(cents: i64) -> Money<9> {
        Money::from_cents(cents).expect("an S9(09)V99 amount")
    }

    fn points(value: i64) -> RewardPoints {
        RewardPoints::try_from(value).expect("non-negative points")
    }

    fn category(code: u16) -> TranCategoryCode {
        TranCategoryCode::new(code).expect("a 9(04) category code")
    }

    /// Account 00000000007, which the card of the first `dailytran.txt` record
    /// belongs to (`app/data/ASCII/cardxref.txt`).
    fn account() -> AccountId {
        AccountId::new(7).expect("a 9(11) account number")
    }

    /// An unbounded, open-ended rule of the fixture pair `type_cd` /
    /// `category_cd` (`app/data/ASCII/trantype.txt`, `trancatg.txt`), active
    /// from the seed start.
    fn rule(rule_id: i64, type_cd: &str, category_cd: u16, reward_rate: Decimal) -> RewardRule {
        RewardRule {
            rule_id: RuleId::from(rule_id),
            transaction_type_cd: TranTypeCode::move_from(type_cd),
            transaction_category_cd: category(category_cd),
            reward_rate: RewardRate::from_decimal(reward_rate).expect("a reward rate"),
            min_transaction_amount: None,
            max_transaction_amount: None,
            active_from: seed_start(),
            active_to: None,
        }
    }

    /// A rule of type `01` Purchase, category `0001` Regular Sales Draft.
    fn retail(rule_id: i64, reward_rate: Decimal) -> RewardRule {
        rule(rule_id, "01", 1, reward_rate)
    }

    /// A `01`/`0001` purchase of `cents` on transaction row `transaction_id`.
    fn purchase(transaction_id: i64, cents: i64) -> AccrualInput {
        AccrualInput {
            transaction_id: TransactionRowId::from(transaction_id),
            account_id: account(),
            type_cd: TranTypeCode::move_from("01"),
            category_cd: category(1),
            amount: amount(cents),
        }
    }

    fn selected_id(rules: &[RewardRule], input: &AccrualInput, at: DateTime<Utc>) -> Option<i64> {
        select_rule(rules, input, at).map(|rule| rule.rule_id.get())
    }

    fn earned(total: i64) -> RewardBalance {
        RewardBalance::new(account(), points(total), RewardPoints::ZERO)
            .expect("a consistent balance")
    }

    #[test]
    fn for_transaction_maps_the_row_identity_codes_and_amount() {
        // The first `app/data/ASCII/dailytran.txt` record as a stored row.
        let transaction = Transaction {
            transaction_id: TransactionRowId::from(41),
            tran_id: TranId::move_from("0000000000683580"),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: category(1),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: amount(5047),
            tran_merchant_id: MerchantId::new(800_000_000).expect("a 9(09) merchant ID"),
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from("4859452612877065"),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
        };
        assert_eq!(
            AccrualInput::for_transaction(&transaction, account()),
            purchase(41, 5047)
        );
    }

    #[test]
    fn a_non_positive_amount_accrues_nothing_and_reaches_no_store_call() {
        // Zero, the -919.00 of the fixture's `03/0001` return (`0000009190}`),
        // and the smallest debit: none accrues, even under a matching rule.
        for cents in [0, -91_900, -1] {
            let mut store = MemoryStore::with_rules(vec![retail(1, dec!(1.0000))]);
            let result = poll_ready(RewardEngine::accrue(
                &mut store,
                purchase(1, cents),
                processing_clock(),
            ));
            assert!(matches!(result, Ok(None)), "{cents}: {result:?}");
            assert_eq!(store.calls, 0, "{cents}");
            assert!(store.accruals.is_empty() && store.balances.is_empty());
        }
    }

    #[test]
    fn a_transaction_without_a_rule_of_its_pair_accrues_nothing() {
        let other_pairs = vec![
            rule(1, "04", 3, dec!(3.0000)),
            rule(2, "01", 2, dec!(1.0000)),
            rule(3, "03", 1, dec!(1.0000)),
        ];
        for rules in [Vec::new(), other_pairs] {
            let mut store = MemoryStore::with_rules(rules);
            let result = poll_ready(RewardEngine::accrue(
                &mut store,
                purchase(1, 5047),
                processing_clock(),
            ));
            assert!(matches!(result, Ok(None)), "{result:?}");
            assert_eq!(store.calls, 1, "only the candidate read");
            assert!(store.accruals.is_empty() && store.balances.is_empty());
        }
    }

    #[test]
    fn select_rule_itself_rejects_a_rule_of_another_pair() {
        let rules = [
            rule(1, "04", 3, dec!(3.0000)),
            rule(2, "01", 2, dec!(1.0000)),
            rule(3, "02", 1, dec!(1.0000)),
        ];
        assert_eq!(
            selected_id(&rules, &purchase(1, 5047), processing_clock()),
            None
        );
        let travel = AccrualInput {
            type_cd: TranTypeCode::move_from("04"),
            category_cd: category(3),
            ..purchase(1, 5047)
        };
        assert_eq!(selected_id(&rules, &travel, processing_clock()), Some(1));
    }

    #[test]
    fn amount_bounds_are_inclusive_and_an_absent_bound_is_unbounded() {
        // (min cents, max cents, amount cents, matches)
        let cases = [
            (None, None, 5047, true),
            (None, None, 1, true),
            (Some(1_000), None, 999, false),
            (Some(1_000), None, 1_000, true),
            (None, Some(10_000), 10_001, false),
            (None, Some(10_000), 10_000, true),
            (Some(1_000), Some(10_000), 5047, true),
            (Some(1_000), Some(10_000), 999, false),
            (Some(1_000), Some(10_000), 10_001, false),
            (Some(5047), Some(5047), 5047, true),
        ];
        for (min, max, cents, matches) in cases {
            let bounded = RewardRule {
                min_transaction_amount: min.map(amount),
                max_transaction_amount: max.map(amount),
                ..retail(1, dec!(1.0000))
            };
            assert_eq!(
                selected_id(&[bounded], &purchase(1, cents), processing_clock()).is_some(),
                matches,
                "{min:?}..={max:?} with {cents}"
            );
        }
    }

    #[test]
    fn a_higher_rate_excluded_by_its_range_loses_to_an_in_range_lower_rate() {
        let large_only = RewardRule {
            min_transaction_amount: Some(amount(10_000)),
            ..retail(1, dec!(3.0000))
        };
        let small_only = RewardRule {
            max_transaction_amount: Some(amount(1_000)),
            ..retail(2, dec!(2.0000))
        };
        let any = retail(3, dec!(1.0000));
        let forward = [large_only.clone(), small_only.clone(), any.clone()];
        let backward = [any, small_only, large_only];
        // (amount cents, winner)
        for (cents, winner) in [(5047, 3), (10_000, 1), (1_000, 2)] {
            for rules in [&forward, &backward] {
                assert_eq!(
                    selected_id(rules, &purchase(1, cents), processing_clock()),
                    Some(winner),
                    "{cents}"
                );
            }
        }
    }

    #[test]
    fn the_highest_rate_wins_and_equal_rates_go_to_the_lowest_rule_id_in_any_order() {
        let rules = [
            retail(7, dec!(1.5000)),
            retail(5, dec!(2.0000)),
            retail(3, dec!(2.0000)),
            retail(9, dec!(1.0000)),
            retail(4, dec!(0.5000)),
        ];
        for shift in 0..rules.len() {
            let mut order = rules.to_vec();
            order.rotate_left(shift);
            let input = purchase(1, 5047);
            assert_eq!(
                selected_id(&order, &input, processing_clock()),
                Some(3),
                "rotation {shift}"
            );
            order.reverse();
            assert_eq!(
                selected_id(&order, &input, processing_clock()),
                Some(3),
                "reversed rotation {shift}"
            );
        }
    }

    #[test]
    fn a_rule_matches_only_inside_its_active_window() {
        let start = seed_start();
        let end = instant(2022, 7, 1, 0);
        let tick = TimeDelta::nanoseconds(1);
        // (active_to, at, matches)
        let cases = [
            (None, start, true),
            (None, start - tick, false),
            (None, instant(9999, 12, 31, 0), true),
            (Some(end), end - tick, true),
            (Some(end), end, false),
            (Some(end), end + tick, false),
        ];
        for (active_to, at, matches) in cases {
            let windowed = RewardRule {
                active_to,
                ..retail(1, dec!(1.0000))
            };
            assert_eq!(
                selected_id(&[windowed], &purchase(1, 5047), at).is_some(),
                matches,
                "{active_to:?} at {at}"
            );
        }
    }

    #[test]
    fn a_soft_deleted_higher_rate_loses_to_an_active_lower_rate() {
        let deleted = RewardRule {
            active_to: Some(instant(2022, 6, 1, 0)),
            ..retail(1, dec!(3.0000))
        };
        let rules = [deleted, retail(2, dec!(1.0000))];
        let input = purchase(1, 5047);
        assert_eq!(
            selected_id(&rules, &input, instant(2022, 5, 31, 0)),
            Some(1)
        );
        assert_eq!(selected_id(&rules, &input, processing_clock()), Some(2));
    }

    #[test]
    fn points_are_the_exact_product_rounded_up() {
        // (amount cents, rate, points)
        let cases = [
            (5047, dec!(1.0000), 51),
            (1, dec!(1.0000), 1),
            (1_000, dec!(1.5000), 15),
            (10_000, dec!(3.0000), 300),
            (1, dec!(0.0001), 1),
            (5_000, dec!(1.0000), 50),
            (3_333, dec!(1.5000), 50),
            (5047, dec!(0), 0),
            (99_999_999_999, dec!(99999.9999), 99_999_999_899_001),
        ];
        for (cents, reward_rate, expected) in cases {
            let rate = RewardRate::from_decimal(reward_rate).expect("a reward rate");
            assert_eq!(
                points_for(amount(cents), rate),
                Ok(points(expected)),
                "{cents} cents at {reward_rate}"
            );
        }
    }

    #[test]
    fn an_accrual_records_the_winning_rule_and_credits_the_balance() {
        let at = processing_clock();
        let mut store = MemoryStore::with_rules(vec![
            retail(1, dec!(1.0000)),
            rule(2, "04", 3, dec!(3.0000)),
        ]);

        let first = poll_ready(RewardEngine::accrue(&mut store, purchase(11, 5047), at));
        let expected_first = RewardAccrual {
            accrual_id: AccrualId::from(1),
            account_id: account(),
            transaction_id: TransactionRowId::from(11),
            rule_id: RuleId::from(1),
            points_earned: points(51),
            created_at: at,
        };
        assert_eq!(
            first.expect("the accrual succeeds"),
            Some(expected_first.clone())
        );
        assert_eq!(store.accruals, vec![expected_first.clone()]);
        assert_eq!(store.balances, vec![earned(51)]);
        assert_eq!(store.calls, 3);

        // A second transaction on the same account adds to the same balance.
        let second = poll_ready(RewardEngine::accrue(&mut store, purchase(12, 10_000), at));
        let expected_second = RewardAccrual {
            accrual_id: AccrualId::from(2),
            transaction_id: TransactionRowId::from(12),
            points_earned: points(100),
            ..expected_first.clone()
        };
        assert_eq!(
            second.expect("the accrual succeeds"),
            Some(expected_second.clone())
        );
        assert_eq!(store.accruals, vec![expected_first, expected_second]);
        assert_eq!(store.balances, vec![earned(151)]);
        assert_eq!(store.balances[0].total_points_redeemed, RewardPoints::ZERO);
        assert_eq!(store.balances[0].available_points, points(151));
        assert_eq!(store.calls, 6);
    }

    #[test]
    fn a_second_accrual_for_the_same_transaction_is_a_store_duplicate() {
        let at = processing_clock();
        let mut store = MemoryStore::with_rules(vec![retail(1, dec!(1.0000))]);
        poll_ready(RewardEngine::accrue(&mut store, purchase(11, 5047), at))
            .expect("the first accrual succeeds");

        let again = poll_ready(RewardEngine::accrue(&mut store, purchase(11, 5047), at));
        assert!(
            matches!(again, Err(RewardError::Store(PortError::Duplicate))),
            "{again:?}"
        );
        assert_eq!(store.accruals.len(), 1);
        assert_eq!(store.balances, vec![earned(51)]);
        assert_eq!(store.calls, 5, "no credit after the failed insert");
    }

    #[test]
    fn a_failed_insert_propagates_unchanged_and_credits_nothing() {
        let failure = PortError::Unavailable("connection reset".to_owned());
        let mut store = MemoryStore::with_rules(vec![retail(1, dec!(1.0000))]);
        store.fail_insert = Some(failure.clone());

        let result = poll_ready(RewardEngine::accrue(
            &mut store,
            purchase(11, 5047),
            processing_clock(),
        ));
        assert!(
            matches!(&result, Err(RewardError::Store(error)) if *error == failure),
            "{result:?}"
        );
        assert_eq!(store.calls, 2, "no credit after the failed insert");
        assert!(store.accruals.is_empty() && store.balances.is_empty());
    }

    #[test]
    fn a_winning_rule_of_rate_zero_records_zero_points() {
        let at = processing_clock();
        let mut store = MemoryStore::with_rules(vec![retail(1, dec!(0.0000))]);

        let result = poll_ready(RewardEngine::accrue(&mut store, purchase(11, 5047), at));
        let accrual = result
            .expect("the accrual succeeds")
            .expect("the rate-0 rule wins");
        assert_eq!(accrual.rule_id, RuleId::from(1));
        assert_eq!(accrual.points_earned, RewardPoints::ZERO);
        assert_eq!(store.accruals, vec![accrual]);
        assert_eq!(store.balances, vec![RewardBalance::zero(account())]);
        assert_eq!(store.calls, 3);
    }

    #[test]
    fn credit_grows_the_earned_total_and_keeps_the_redeemed_total() {
        let start =
            RewardBalance::new(account(), points(10), points(4)).expect("a consistent balance");
        let credited = credit(&start, points(5)).expect("a credit within range");
        assert_eq!(
            credited,
            RewardBalance {
                account_id: account(),
                total_points_earned: points(15),
                total_points_redeemed: points(4),
                available_points: points(11),
            }
        );
        assert!(credited.is_consistent());

        assert_eq!(credit(&start, RewardPoints::ZERO), Ok(start));
        assert_eq!(
            credit(&RewardBalance::zero(account()), points(51)),
            Ok(earned(51))
        );
        assert_eq!(credit(&earned(i64::MAX), points(1)), Err(Overflow));
    }

    #[test]
    fn the_accrual_future_is_send() {
        let mut store = MemoryStore::with_rules(vec![retail(1, dec!(1.0000))]);
        let future = RewardEngine::accrue(&mut store, purchase(11, 5047), processing_clock());
        assert_send(&future);
        assert!(matches!(poll_ready(future), Ok(Some(_))));
    }
}
