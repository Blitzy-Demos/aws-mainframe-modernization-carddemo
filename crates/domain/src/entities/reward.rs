//! The reward entities: rules, account balances and accruals (request Rewards
//! system; AAP 0.3.5).
//!
//! Rewards are a new feature with no COBOL oracle: the README lists DB2
//! Rewards only as a roadmap item [README.md:L357-369], so no parity audit row
//! gates this module and no paragraph name applies. Every field is named after
//! its column in `migrations/0005_rewards.sql` (tables `reward_rules`,
//! `reward_balances` and `reward_accruals`), and exactly the nullable columns
//! are an `Option`.
//!
//! The field types come from [`crate::numeric`] (AAP 0.5.3.1): amount bounds
//! are [`Money<9>`](Money), the `TRAN-AMT PIC S9(09)V99`
//! [app/cpy/CVTRA05Y.cpy:L10] they are compared with; rates are
//! [`RewardRate`]; points are [`RewardPoints`], whole and non-negative, and
//! changed only through their checked arithmetic. Instants are UTC.
//!
//! These types hold the values and the two invariants that are theirs: the
//! active window of a rule and the available points of a balance. Matching an
//! amount against a rule's bounds, choosing the winning rule and computing the
//! points belong to `crate::rewards::engine`; validating an admin rule belongs
//! to `crate::rewards`. Redemption and tiers are out of scope (AAP 0.2.5), so
//! nothing here spends points.

use chrono::{DateTime, Utc};

use crate::numeric::{
    AccountId, AccrualId, Money, Overflow, RewardPoints, RewardRate, RuleId, TranCategoryCode,
    TranTypeCode, TransactionRowId,
};

/// Whether instant `t` lies in the window from `active_from` (inclusive) to
/// `active_to` (exclusive); no `active_to` leaves the window open-ended
/// (AAP 0.3.5).
///
/// The one definition of "active at `t`" for stored and new rules alike. A
/// window whose `active_to` precedes its `active_from`, as a soft delete of a
/// rule that has not started leaves it, contains no instant.
fn window_contains(
    active_from: DateTime<Utc>,
    active_to: Option<DateTime<Utc>>,
    t: DateTime<Utc>,
) -> bool {
    active_from <= t && active_to.is_none_or(|end| t < end)
}

/// A reward rule: one `reward_rules` row (request Rewards system; AAP 0.3.5).
///
/// The rule earns `reward_rate` points per currency unit on a transaction whose
/// type and category codes equal its own, while it is active and the amount
/// lies within its bounds. Among the matching rules the highest rate wins, and
/// equal rates go to the lowest `rule_id`. `DELETE /admin/reward-rules/{id}`
/// soft-deletes a rule by setting `active_to`, so a rule that accruals
/// reference is never removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewardRule {
    /// The rule identity: column `rule_id`, `BIGINT` identity primary key,
    /// assigned by the database. The lowest wins a tie of equal rates.
    pub rule_id: RuleId,
    /// The transaction type code the rule matches: column
    /// `transaction_type_cd`, `VARCHAR(2) NOT NULL`, compared with the
    /// transaction's `TRAN-TYPE-CD PIC X(02)`.
    pub transaction_type_cd: TranTypeCode,
    /// The transaction category code the rule matches: column
    /// `transaction_category_cd`, `INTEGER NOT NULL` from 0 to 9999, compared
    /// with the transaction's `TRAN-CAT-CD PIC 9(04)`.
    pub transaction_category_cd: TranCategoryCode,
    /// Points per currency unit: column `reward_rate`, `NUMERIC(9,4) NOT NULL`,
    /// never negative.
    pub reward_rate: RewardRate,
    /// The inclusive lower amount bound: column `min_transaction_amount`,
    /// `NUMERIC(11,2) NULL`. `None` is unbounded.
    pub min_transaction_amount: Option<Money<9>>,
    /// The inclusive upper amount bound: column `max_transaction_amount`,
    /// `NUMERIC(11,2) NULL`. `None` is unbounded.
    pub max_transaction_amount: Option<Money<9>>,
    /// The first instant the rule is active, inclusive: column `active_from`,
    /// `TIMESTAMPTZ NOT NULL`.
    pub active_from: DateTime<Utc>,
    /// The end of the active window, exclusive: column `active_to`,
    /// `TIMESTAMPTZ NULL`. `None` is open-ended; a soft delete sets it to the
    /// processing clock (AAP 0.6.4).
    pub active_to: Option<DateTime<Utc>>,
}

impl RewardRule {
    /// The stored rule that `new_rule` becomes once the database has assigned
    /// it `rule_id`.
    ///
    /// Every field of `new_rule` is `Copy`, so it is taken by reference
    /// (`needless_pass_by_value`) and the caller keeps it.
    #[must_use]
    pub fn from_new(rule_id: RuleId, new_rule: &NewRewardRule) -> Self {
        let NewRewardRule {
            transaction_type_cd,
            transaction_category_cd,
            reward_rate,
            min_transaction_amount,
            max_transaction_amount,
            active_from,
            active_to,
        } = *new_rule;
        Self {
            rule_id,
            transaction_type_cd,
            transaction_category_cd,
            reward_rate,
            min_transaction_amount,
            max_transaction_amount,
            active_from,
            active_to,
        }
    }

    /// Whether the rule is active at instant `t`: `active_from <= t`, and
    /// either `active_to` is `None` or `t < active_to` (AAP 0.3.5).
    ///
    /// `t` is the processing clock (AAP 0.6.4), which the caller reads.
    #[must_use]
    pub fn is_active_at(&self, t: DateTime<Utc>) -> bool {
        window_contains(self.active_from, self.active_to, t)
    }
}

/// A reward rule before the database assigns its `rule_id`: the row that
/// `POST /admin/reward-rules` and `batch seed` insert into `reward_rules`
/// (request Rewards system; AAP 0.3.5, 0.3.6).
///
/// Its fields are those of [`RewardRule`] without `rule_id`, with the same
/// columns and meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRewardRule {
    /// Column `transaction_type_cd`, `VARCHAR(2) NOT NULL`: see
    /// [`RewardRule::transaction_type_cd`].
    pub transaction_type_cd: TranTypeCode,
    /// Column `transaction_category_cd`, `INTEGER NOT NULL`: see
    /// [`RewardRule::transaction_category_cd`].
    pub transaction_category_cd: TranCategoryCode,
    /// Column `reward_rate`, `NUMERIC(9,4) NOT NULL`: see
    /// [`RewardRule::reward_rate`].
    pub reward_rate: RewardRate,
    /// Column `min_transaction_amount`, `NUMERIC(11,2) NULL`: see
    /// [`RewardRule::min_transaction_amount`].
    pub min_transaction_amount: Option<Money<9>>,
    /// Column `max_transaction_amount`, `NUMERIC(11,2) NULL`: see
    /// [`RewardRule::max_transaction_amount`].
    pub max_transaction_amount: Option<Money<9>>,
    /// Column `active_from`, `TIMESTAMPTZ NOT NULL`: see
    /// [`RewardRule::active_from`].
    pub active_from: DateTime<Utc>,
    /// Column `active_to`, `TIMESTAMPTZ NULL`: see [`RewardRule::active_to`].
    pub active_to: Option<DateTime<Utc>>,
}

impl NewRewardRule {
    /// Whether the rule will be active at instant `t`, by the same window as
    /// [`RewardRule::is_active_at`].
    #[must_use]
    pub fn is_active_at(&self, t: DateTime<Utc>) -> bool {
        window_contains(self.active_from, self.active_to, t)
    }
}

/// The reward balance of one account: one `reward_balances` row (request
/// Rewards system; AAP 0.3.5).
///
/// Invariant: `available_points = total_points_earned - total_points_redeemed`,
/// the table's `reward_balances_available_points_check`, with every total in
/// the non-negative range of [`RewardPoints`]. The fields are public, as on
/// every entity, so a row read back can be built field by field;
/// [`RewardBalance::new`] is the invariant-preserving constructor, which
/// derives `available_points`, and [`RewardBalance::is_consistent`] checks a
/// balance built any other way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewardBalance {
    /// The account the balance belongs to: column `account_id`,
    /// `BIGINT NOT NULL`, primary key and foreign key to `accounts`; the
    /// `ACCT-ID PIC 9(11)` key [app/cpy/CVACT01Y.cpy:L5].
    pub account_id: AccountId,
    /// The sum of the account's accruals: column `total_points_earned`,
    /// `BIGINT NOT NULL DEFAULT 0`.
    pub total_points_earned: RewardPoints,
    /// The points spent: column `total_points_redeemed`,
    /// `BIGINT NOT NULL DEFAULT 0`. Redemption is out of scope (AAP 0.2.5), so
    /// the field exists only to state the invariant and is never written above
    /// 0.
    pub total_points_redeemed: RewardPoints,
    /// The points the account holds: column `available_points`,
    /// `BIGINT NOT NULL DEFAULT 0`, always `total_points_earned` minus
    /// `total_points_redeemed`.
    pub available_points: RewardPoints,
}

impl RewardBalance {
    /// The balance of `account_id` with the given totals; `available_points`
    /// is derived as `total_points_earned - total_points_redeemed` by the
    /// checked points subtraction, so the result always holds the invariant.
    ///
    /// # Errors
    ///
    /// [`Overflow`] when `total_points_redeemed` exceeds
    /// `total_points_earned`: the available points would be negative, which
    /// neither [`RewardPoints`] nor the table's checks admit.
    pub fn new(
        account_id: AccountId,
        total_points_earned: RewardPoints,
        total_points_redeemed: RewardPoints,
    ) -> Result<Self, Overflow> {
        let available_points = total_points_earned.checked_sub(total_points_redeemed)?;
        Ok(Self {
            account_id,
            total_points_earned,
            total_points_redeemed,
            available_points,
        })
    }

    /// The balance of an account without accruals: all three totals zero.
    ///
    /// `GET /rewards/balance/{account_id}` answers it (200) for an existing
    /// account that has no `reward_balances` row.
    #[must_use]
    pub fn zero(account_id: AccountId) -> Self {
        Self {
            account_id,
            total_points_earned: RewardPoints::ZERO,
            total_points_redeemed: RewardPoints::ZERO,
            available_points: RewardPoints::ZERO,
        }
    }

    /// Whether `available_points` equals
    /// `total_points_earned - total_points_redeemed`: the invariant check for a
    /// row read back from the database.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.total_points_earned
            .checked_sub(self.total_points_redeemed)
            .is_ok_and(|available| available == self.available_points)
    }
}

/// The points one qualifying transaction earned: one `reward_accruals` row
/// (request Rewards system; AAP 0.3.5).
///
/// `RewardEngine::accrue` creates it, on CT02 and POSTTRAN only, in the
/// database transaction that records the transaction itself. The reward history
/// of an account lists these rows newest first and pages by `accrual_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewardAccrual {
    /// The accrual identity: column `accrual_id`, `BIGINT` identity primary
    /// key, assigned by the database; the reward history page key.
    pub accrual_id: AccrualId,
    /// The account credited: column `account_id`, `BIGINT NOT NULL` from 0 to
    /// 99999999999, the `ACCT-ID PIC 9(11)` range.
    pub account_id: AccountId,
    /// The transaction row that earned the points: column `transaction_id`,
    /// `BIGINT NOT NULL`, unique, foreign key to `transactions` with
    /// `ON DELETE RESTRICT`. It is the row identity of `transactions`, not the
    /// COBOL `tran_id`, so the accrual outlives `batch tranbkp` emptying the
    /// live master (E-09).
    pub transaction_id: TransactionRowId,
    /// The winning rule: column `rule_id`, `BIGINT NOT NULL`, foreign key to
    /// `reward_rules`.
    pub rule_id: RuleId,
    /// The points earned, `ceil(amount × reward_rate)`: column
    /// `points_earned`, `BIGINT NOT NULL`, never negative. A winning rule of
    /// rate 0 records 0 points.
    pub points_earned: RewardPoints,
    /// The instant of the accrual: column `created_at`, `TIMESTAMPTZ NOT NULL`,
    /// taken from the processing clock (AAP 0.6.4).
    pub created_at: DateTime<Utc>,
}

impl RewardAccrual {
    /// The stored accrual that `new_accrual` becomes once the database has
    /// assigned it `accrual_id`.
    ///
    /// Every field of `new_accrual` is `Copy`, so it is taken by reference
    /// (`needless_pass_by_value`) and the caller keeps it.
    #[must_use]
    pub fn from_new(accrual_id: AccrualId, new_accrual: &NewAccrual) -> Self {
        let NewAccrual {
            account_id,
            transaction_id,
            rule_id,
            points_earned,
            created_at,
        } = *new_accrual;
        Self {
            accrual_id,
            account_id,
            transaction_id,
            rule_id,
            points_earned,
            created_at,
        }
    }
}

/// A reward accrual before the database assigns its `accrual_id`: the
/// argument of `RewardStore::insert_accrual` (request Rewards system;
/// AAP 0.3.5).
///
/// Its fields are those of [`RewardAccrual`] without `accrual_id`, with the
/// same columns and meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccrual {
    /// Column `account_id`, `BIGINT NOT NULL`: see
    /// [`RewardAccrual::account_id`].
    pub account_id: AccountId,
    /// Column `transaction_id`, `BIGINT NOT NULL`, unique: see
    /// [`RewardAccrual::transaction_id`].
    pub transaction_id: TransactionRowId,
    /// Column `rule_id`, `BIGINT NOT NULL`: see [`RewardAccrual::rule_id`].
    pub rule_id: RuleId,
    /// Column `points_earned`, `BIGINT NOT NULL`: see
    /// [`RewardAccrual::points_earned`].
    pub points_earned: RewardPoints,
    /// Column `created_at`, `TIMESTAMPTZ NOT NULL`: see
    /// [`RewardAccrual::created_at`].
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeDelta, TimeZone, Utc};
    use rust_decimal_macros::dec;

    use super::{NewAccrual, NewRewardRule, RewardAccrual, RewardBalance, RewardRule};
    use crate::numeric::{
        AccountId, AccrualId, Money, Overflow, RewardPoints, RewardRate, RuleId, TranCategoryCode,
        TranTypeCode, TransactionRowId,
    };

    fn midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("a valid UTC instant")
    }

    fn points(value: i64) -> RewardPoints {
        RewardPoints::try_from(value).expect("non-negative points")
    }

    fn account() -> AccountId {
        "00000000001".parse().expect("a 9(11) account number")
    }

    /// The seeded retail rule (AAP 0.3.6): type `01` Purchase, category `0001`
    /// Regular Sales Draft, 1 point per currency unit from 2022-01-01, with
    /// bounds so that every field carries a value.
    fn retail_rule(active_to: Option<DateTime<Utc>>) -> NewRewardRule {
        NewRewardRule {
            transaction_type_cd: TranTypeCode::move_from("01"),
            transaction_category_cd: TranCategoryCode::new(1).expect("a 9(04) code"),
            reward_rate: RewardRate::from_decimal(dec!(1.0000)).expect("a reward rate"),
            min_transaction_amount: Some(Money::from_cents(1).expect("an S9(09)V99 amount")),
            max_transaction_amount: Some(
                Money::from_cents(99_999_999_999).expect("an S9(09)V99 amount"),
            ),
            active_from: midnight(2022, 1, 1),
            active_to,
        }
    }

    #[test]
    fn a_rule_is_active_from_its_start_until_just_before_its_end() {
        let start = midnight(2022, 1, 1);
        let end = midnight(2022, 7, 1);
        let tick = TimeDelta::nanoseconds(1);
        let new_rule = retail_rule(Some(end));
        let rule = RewardRule::from_new(RuleId::from(1), &new_rule);
        let cases = [
            (start - tick, false),
            (start, true),
            (end - tick, true),
            (end, false),
            (end + tick, false),
        ];
        for (t, active) in cases {
            assert_eq!(rule.is_active_at(t), active, "stored rule at {t}");
            assert_eq!(new_rule.is_active_at(t), active, "new rule at {t}");
        }
    }

    #[test]
    fn a_rule_without_an_end_is_active_from_its_start_on() {
        let new_rule = retail_rule(None);
        let rule = RewardRule::from_new(RuleId::from(2), &new_rule);
        let start = midnight(2022, 1, 1);
        for t in [start, midnight(2022, 7, 18), midnight(9999, 12, 31)] {
            assert!(rule.is_active_at(t), "stored rule at {t}");
            assert!(new_rule.is_active_at(t), "new rule at {t}");
        }
        let before = start - TimeDelta::nanoseconds(1);
        assert!(!rule.is_active_at(before));
        assert!(!new_rule.is_active_at(before));
    }

    #[test]
    fn a_window_ending_before_it_starts_contains_no_instant() {
        // A soft delete of a rule that has not started yet sets `active_to` to
        // the processing clock, ahead of `active_from`.
        let mut new_rule = retail_rule(Some(midnight(2021, 12, 1)));
        new_rule.active_from = midnight(2022, 1, 1);
        let rule = RewardRule::from_new(RuleId::from(3), &new_rule);
        for t in [
            midnight(2021, 11, 30),
            midnight(2021, 12, 1),
            midnight(2022, 1, 1),
        ] {
            assert!(!rule.is_active_at(t), "{t}");
        }
    }

    #[test]
    fn rule_from_new_keeps_every_field() {
        let new_rule = retail_rule(Some(midnight(2023, 1, 1)));
        let rule = RewardRule::from_new(RuleId::from(42), &new_rule);
        assert_eq!(rule.rule_id, RuleId::from(42));
        assert_eq!(rule.transaction_type_cd, new_rule.transaction_type_cd);
        assert_eq!(
            rule.transaction_category_cd,
            new_rule.transaction_category_cd
        );
        assert_eq!(rule.reward_rate, new_rule.reward_rate);
        assert_eq!(rule.min_transaction_amount, new_rule.min_transaction_amount);
        assert_eq!(rule.max_transaction_amount, new_rule.max_transaction_amount);
        assert_eq!(rule.active_from, new_rule.active_from);
        assert_eq!(rule.active_to, new_rule.active_to);
    }

    #[test]
    fn balance_new_derives_the_available_points() {
        let cases = [
            (points(10), RewardPoints::ZERO, Ok(points(10))),
            (points(10), points(4), Ok(points(6))),
            (points(5), points(5), Ok(RewardPoints::ZERO)),
            (RewardPoints::ZERO, points(1), Err(Overflow)),
            (points(4), points(10), Err(Overflow)),
        ];
        for (earned, redeemed, available) in cases {
            let balance = RewardBalance::new(account(), earned, redeemed);
            assert_eq!(
                balance.as_ref().map(|balance| balance.available_points),
                available.as_ref().copied(),
                "{earned} - {redeemed}"
            );
            if let Ok(balance) = balance {
                assert_eq!(balance.account_id, account());
                assert_eq!(balance.total_points_earned, earned);
                assert_eq!(balance.total_points_redeemed, redeemed);
                assert!(balance.is_consistent(), "{earned} - {redeemed}");
            }
        }
    }

    #[test]
    fn a_balance_above_two_to_the_fifty_third_is_held_exactly() {
        let earned = points(9_007_199_254_740_993);
        let balance =
            RewardBalance::new(account(), earned, RewardPoints::ZERO).expect("a consistent total");
        assert_eq!(balance.available_points.to_i64(), 9_007_199_254_740_993);
        assert_eq!(balance.available_points.to_string(), "9007199254740993");
        assert_eq!(balance.total_points_earned, earned);
        assert!(balance.is_consistent());
    }

    #[test]
    fn the_zero_balance_is_consistent() {
        let balance = RewardBalance::zero(account());
        assert_eq!(balance.account_id, account());
        assert_eq!(balance.total_points_earned, RewardPoints::ZERO);
        assert_eq!(balance.total_points_redeemed, RewardPoints::ZERO);
        assert_eq!(balance.available_points, RewardPoints::ZERO);
        assert!(balance.is_consistent());
        assert_eq!(
            Ok(balance),
            RewardBalance::new(account(), RewardPoints::ZERO, RewardPoints::ZERO)
        );
    }

    #[test]
    fn a_balance_read_back_with_a_broken_invariant_is_inconsistent() {
        let cases = [
            (points(10), RewardPoints::ZERO, points(9)),
            (points(10), points(3), points(10)),
            (RewardPoints::ZERO, points(1), RewardPoints::ZERO),
        ];
        for (earned, redeemed, available) in cases {
            let balance = RewardBalance {
                account_id: account(),
                total_points_earned: earned,
                total_points_redeemed: redeemed,
                available_points: available,
            };
            assert!(
                !balance.is_consistent(),
                "{earned} - {redeemed} = {available}"
            );
        }
    }

    #[test]
    fn accrual_from_new_keeps_every_field() {
        let new_accrual = NewAccrual {
            account_id: account(),
            transaction_id: TransactionRowId::from(7),
            rule_id: RuleId::from(1),
            points_earned: points(51),
            created_at: midnight(2022, 7, 1) + TimeDelta::hours(12),
        };
        let accrual = RewardAccrual::from_new(AccrualId::from(3), &new_accrual);
        assert_eq!(accrual.accrual_id, AccrualId::from(3));
        assert_eq!(accrual.account_id, new_accrual.account_id);
        assert_eq!(accrual.transaction_id, new_accrual.transaction_id);
        assert_eq!(accrual.rule_id, new_accrual.rule_id);
        assert_eq!(accrual.points_earned, new_accrual.points_earned);
        assert_eq!(accrual.created_at, new_accrual.created_at);
    }
}
