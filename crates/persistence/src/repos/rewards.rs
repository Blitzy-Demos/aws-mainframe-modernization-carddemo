//! The reward rule administration and the reward reads on [`PgStore`]: tables
//! `reward_rules`, `reward_balances` and `reward_accruals` of
//! `migrations/0005_rewards.sql` (request Rewards system; AAP 0.3.5).
//!
//! These are inherent methods: no port trait declares them, because only the
//! reward handlers of `carddemo-api` call them. The reward store port
//! `RewardStore` (`candidate_rules`, `insert_accrual`, `add_points`), the only
//! path by which accruals and balances are written, is
//! [`PgRewardStore`](crate::rewards::PgRewardStore), and none of its queries is
//! repeated here.
//!
//! | Method | Endpoint | SQL |
//! |---|---|---|
//! | `list_reward_rules` | `GET /admin/reward-rules` | every rule, `ORDER BY rule_id` |
//! | `reward_rule` | the second step of a soft delete | `WHERE rule_id = $1` |
//! | `insert_reward_rule` | `POST /admin/reward-rules` | `INSERT … RETURNING` |
//! | `update_reward_rule` | `PUT /admin/reward-rules/{id}` | `UPDATE … WHERE rule_id = $1 RETURNING` |
//! | `soft_delete_reward_rule` | `DELETE /admin/reward-rules/{id}` | conditional `UPDATE … SET active_to`, then a read |
//! | `reward_balance` | `GET /rewards/balance/{account_id}` | `WHERE account_id = $1` |
//! | `reward_history` | `GET /rewards/history/{account_id}` | newest first, inclusive start, `LIMIT` page + 1 |
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a
//! roadmap item [README.md:L357-369], so no parity audit row gates this module.
//! A rule matches the transaction's `TRAN-TYPE-CD PIC X(02)` and
//! `TRAN-CAT-CD PIC 9(04)` [app/cpy/CVTRA05Y.cpy:L6-7], and a balance or
//! accrual belongs to an `ACCT-ID PIC 9(11)` [app/cpy/CVACT01Y.cpy:L5].
//!
//! # Rows
//!
//! Rules and balances are read into the crate-internal `RewardRuleRow` and
//! `RewardBalanceRow` of [`crate::rewards`] and converted through their
//! `TryFrom`, so every reader of those tables holds a stored value to its
//! domain type the same way. Accruals, which only this module reads back, have
//! their own private row. A value that does not fit its type, or a balance
//! whose `available_points` differs from `total_points_earned` minus
//! `total_points_redeemed`, is [`PersistenceError::Corrupt`], which the methods
//! report as [`PortError::Unavailable`] naming the table and column, never the
//! value.
//!
//! # Writes
//!
//! The rule writes are not legacy writes: no `RECOVERY(NONE)` file stands
//! behind them, and the admin handler rolls its unit of work back on any error
//! and commits only on success. Each statement, reads and writes alike, still
//! runs under its own savepoint through `crate::tx::savepoint` and
//! `crate::tx::settle`, as every store statement does, so a failed one never
//! leaves the transaction aborted before the handler decides. No column
//! defaults to the current time: the soft-delete instant is the caller's
//! processing clock (`CARDDEMO_CLOCK_OVERRIDE` pins it, AAP 0.6.4), never SQL
//! `now()`. An instant is bound only when its `TIMESTAMPTZ` column stores it
//! exactly, as a whole microsecond of UTC time; any other is
//! [`PersistenceError::InexactInstant`], reported as
//! [`PortError::Unavailable`] before the statement runs, and is never
//! narrowed.
//!
//! The methods carry no message text and emit no span: the handlers choose the
//! `carddemo_domain::messages` text of a missing rule or account and record the
//! outcome (AAP 0.6.2).

use std::borrow::Cow;

use carddemo_domain::entities::{NewRewardRule, RewardAccrual, RewardBalance, RewardRule};
use carddemo_domain::numeric::{AccountId, AccrualId, Money, RuleId, TransactionRowId};
use carddemo_domain::ports::{PortError, PortResult};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use super::{account_id, exact_instant, limit_i64, opt_exact_instant, reward_points};
use crate::PersistenceError;
use crate::rewards::{RULES_TABLE, RewardBalanceRow, RewardRuleRow};
use crate::tx::{PgStore, savepoint, settle};

/// The accrual table, as [`PersistenceError::Corrupt`] and
/// [`PersistenceError::InexactInstant`] name it.
pub(crate) const ACCRUALS_TABLE: &str = "reward_accruals";

/// What `DELETE /admin/reward-rules/{id}` found and did
/// ([`PgStore::soft_delete_reward_rule`]).
///
/// A rule is never removed, because accruals reference its `rule_id`; a soft
/// delete ends its active window instead (AAP 0.3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoftDeleteOutcome {
    /// No rule has the ID: the handler answers 404 with the
    /// `REWARD_RULE_NOT_FOUND` text.
    NotFound,
    /// The rule had already ended at the delete instant (`active_to` at or
    /// before it), so nothing was written. The rule is returned as stored, and
    /// the handler answers 200 with it.
    Unchanged(RewardRule),
    /// The rule's `active_to` was absent or later than the delete instant and
    /// is now that instant. The rule is returned as updated.
    Deleted(RewardRule),
}

/// One `reward_accruals` row as `query_as!` reads it.
struct RewardAccrualRow {
    /// `accrual_id BIGINT`, the identity.
    accrual_id: i64,
    /// `account_id BIGINT`, 0 to 99999999999.
    account_id: i64,
    /// `transaction_id BIGINT`, the `transactions` row identity.
    transaction_id: i64,
    /// `rule_id BIGINT`, the winning rule.
    rule_id: i64,
    /// `points_earned BIGINT`, never negative.
    points_earned: i64,
    /// `created_at TIMESTAMPTZ`, the processing clock of the accrual.
    created_at: DateTime<Utc>,
}

/// The accrual a stored row holds. An account key outside `9(11)` or negative
/// points are [`PersistenceError::Corrupt`] naming `reward_accruals` and the
/// column; the table's checks admit neither.
impl TryFrom<RewardAccrualRow> for RewardAccrual {
    type Error = PersistenceError;

    fn try_from(row: RewardAccrualRow) -> Result<Self, Self::Error> {
        Ok(Self {
            accrual_id: AccrualId::from(row.accrual_id),
            account_id: account_id(row.account_id, ACCRUALS_TABLE, "account_id")?,
            transaction_id: TransactionRowId::from(row.transaction_id),
            rule_id: RuleId::from(row.rule_id),
            points_earned: reward_points(row.points_earned, ACCRUALS_TABLE, "points_earned")?,
            created_at: row.created_at,
        })
    }
}

/// The column values a validated rule binds, in the form each column stores:
/// the type code right-trimmed, the category as its integer value, the rate
/// and the amount bounds as their exact decimals, and the window instants as
/// the whole microseconds `TIMESTAMPTZ` keeps.
struct RuleBinds<'a> {
    transaction_type_cd: Cow<'a, str>,
    transaction_category_cd: i32,
    reward_rate: Decimal,
    min_transaction_amount: Option<Decimal>,
    max_transaction_amount: Option<Decimal>,
    active_from: DateTime<Utc>,
    active_to: Option<DateTime<Utc>>,
}

/// The binds of `rule`. A window instant the column cannot store exactly,
/// finer than a microsecond or a leap second, is
/// [`PersistenceError::InexactInstant`] naming `reward_rules` and the column,
/// so the rule is refused before any statement rather than stored with a
/// narrowed start or end.
impl<'a> TryFrom<&'a NewRewardRule> for RuleBinds<'a> {
    type Error = PersistenceError;

    fn try_from(rule: &'a NewRewardRule) -> Result<Self, Self::Error> {
        Ok(Self {
            transaction_type_cd: rule.transaction_type_cd.trimmed(),
            transaction_category_cd: i32::from(rule.transaction_category_cd.get()),
            reward_rate: rule.reward_rate.to_decimal(),
            min_transaction_amount: rule.min_transaction_amount.map(Money::to_decimal),
            max_transaction_amount: rule.max_transaction_amount.map(Money::to_decimal),
            active_from: exact_instant(rule.active_from, RULES_TABLE, "active_from")?,
            active_to: opt_exact_instant(rule.active_to, RULES_TABLE, "active_to")?,
        })
    }
}

/// The rule of a stored row, or [`PortError::Unavailable`] when a column does
/// not fit its domain type.
fn rule_from_row(row: RewardRuleRow) -> PortResult<RewardRule> {
    RewardRule::try_from(row).map_err(PortError::from)
}

/// The reward rule administration and the reward reads (request Rewards
/// system; AAP 0.3.5). Every statement runs under its own savepoint of the
/// store's transaction; the caller commits or rolls back.
impl PgStore<'_, '_> {
    /// Every reward rule, active, future and soft-deleted alike, in `rule_id`
    /// order: the `GET /admin/reward-rules` list. No rules is an empty `Vec`.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the read fails or a stored rule does not
    /// fit its domain type.
    pub async fn list_reward_rules(&mut self) -> PortResult<Vec<RewardRule>> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardRuleRow,
            "SELECT rule_id, transaction_type_cd, transaction_category_cd, reward_rate, \
                    min_transaction_amount, max_transaction_amount, active_from, active_to \
             FROM reward_rules \
             ORDER BY rule_id"
        )
        .fetch_all(&mut *sp)
        .await;
        let rows = settle(sp, result).await?;
        rows.into_iter().map(rule_from_row).collect()
    }

    /// The rule `id`, or `None` when no rule has it.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the read fails or the stored rule does
    /// not fit its domain type.
    pub async fn reward_rule(&mut self, id: RuleId) -> PortResult<Option<RewardRule>> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardRuleRow,
            "SELECT rule_id, transaction_type_cd, transaction_category_cd, reward_rate, \
                    min_transaction_amount, max_transaction_amount, active_from, active_to \
             FROM reward_rules \
             WHERE rule_id = $1",
            id.get(),
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        row.map(rule_from_row).transpose()
    }

    /// Inserts `rule`, which `carddemo_domain::rewards::validate_rule` has
    /// already checked, and returns it as stored, with the `rule_id` the
    /// identity column assigned: the body of the 201 answer to
    /// `POST /admin/reward-rules`.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when `active_from` or `active_to` is not a
    /// whole microsecond of UTC time, with nothing written; when the insert
    /// fails, for example on a table check that a validated rule never trips;
    /// or when the returned row does not fit its domain type.
    pub async fn insert_reward_rule(&mut self, rule: NewRewardRule) -> PortResult<RewardRule> {
        let binds = RuleBinds::try_from(&rule)?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardRuleRow,
            "INSERT INTO reward_rules \
                 (transaction_type_cd, transaction_category_cd, reward_rate, \
                  min_transaction_amount, max_transaction_amount, active_from, active_to) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             RETURNING rule_id, transaction_type_cd, transaction_category_cd, reward_rate, \
                       min_transaction_amount, max_transaction_amount, active_from, active_to",
            binds.transaction_type_cd.as_ref(),
            binds.transaction_category_cd,
            binds.reward_rate,
            binds.min_transaction_amount,
            binds.max_transaction_amount,
            binds.active_from,
            binds.active_to,
        )
        .fetch_one(&mut *sp)
        .await;
        rule_from_row(settle(sp, result).await?)
    }

    /// Replaces every column of the rule `id` but its `rule_id` with `rule`,
    /// already checked by `carddemo_domain::rewards::validate_rule`, and returns
    /// the rule as stored: the body of the `PUT /admin/reward-rules/{id}`
    /// answer.
    ///
    /// `None` when no rule has the ID; the handler answers 404 with the
    /// `REWARD_RULE_NOT_FOUND` text.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when `active_from` or `active_to` is not a
    /// whole microsecond of UTC time, with nothing written, or when the update
    /// fails or the returned row does not fit its domain type.
    pub async fn update_reward_rule(
        &mut self,
        id: RuleId,
        rule: NewRewardRule,
    ) -> PortResult<Option<RewardRule>> {
        let binds = RuleBinds::try_from(&rule)?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardRuleRow,
            "UPDATE reward_rules SET \
                 transaction_type_cd = $2, transaction_category_cd = $3, reward_rate = $4, \
                 min_transaction_amount = $5, max_transaction_amount = $6, \
                 active_from = $7, active_to = $8 \
             WHERE rule_id = $1 \
             RETURNING rule_id, transaction_type_cd, transaction_category_cd, reward_rate, \
                       min_transaction_amount, max_transaction_amount, active_from, active_to",
            id.get(),
            binds.transaction_type_cd.as_ref(),
            binds.transaction_category_cd,
            binds.reward_rate,
            binds.min_transaction_amount,
            binds.max_transaction_amount,
            binds.active_from,
            binds.active_to,
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        row.map(rule_from_row).transpose()
    }

    /// Soft-deletes the rule `id` at instant `at`, the processing clock of the
    /// `DELETE /admin/reward-rules/{id}` handler.
    ///
    /// 1. When the rule's `active_to` is absent or later than `at`, it is set
    ///    to `at` and the rule comes back as [`SoftDeleteOutcome::Deleted`].
    ///    The condition is the SQL form of
    ///    `carddemo_domain::rewards::soft_delete_at`. A rule that has not
    ///    started is ended the same way, leaving a window that contains no
    ///    instant; the table has no check tying `active_to` to `active_from`,
    ///    so that write succeeds.
    /// 2. Otherwise the rule is read: an already ended rule is
    ///    [`SoftDeleteOutcome::Unchanged`], with nothing written, and a missing
    ///    one [`SoftDeleteOutcome::NotFound`].
    ///
    /// `at` is compared and stored as given: the processing clock reads whole
    /// microseconds, the resolution of `active_to`.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when `at` is not a whole microsecond of UTC
    /// time, with nothing read or written, or when the update or the read
    /// fails or the rule does not fit its domain type.
    pub async fn soft_delete_reward_rule(
        &mut self,
        id: RuleId,
        at: DateTime<Utc>,
    ) -> PortResult<SoftDeleteOutcome> {
        let at = exact_instant(at, RULES_TABLE, "active_to")?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardRuleRow,
            "UPDATE reward_rules SET active_to = $2 \
             WHERE rule_id = $1 AND (active_to IS NULL OR active_to > $2) \
             RETURNING rule_id, transaction_type_cd, transaction_category_cd, reward_rate, \
                       min_transaction_amount, max_transaction_amount, active_from, active_to",
            id.get(),
            at,
        )
        .fetch_optional(&mut *sp)
        .await;
        if let Some(row) = settle(sp, result).await? {
            return rule_from_row(row).map(SoftDeleteOutcome::Deleted);
        }
        Ok(self
            .reward_rule(id)
            .await?
            .map_or(SoftDeleteOutcome::NotFound, SoftDeleteOutcome::Unchanged))
    }

    /// The `reward_balances` row of `account`, or `None` when the account has
    /// never accrued.
    ///
    /// The method does not check that the account exists: the handler reads it
    /// through `AccountReader` first and answers a zero balance for an
    /// existing account without a row.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the read fails, or when the stored row
    /// breaks the invariant `available_points = total_points_earned -
    /// total_points_redeemed` or a column's domain type.
    pub async fn reward_balance(
        &mut self,
        account: AccountId,
    ) -> PortResult<Option<RewardBalance>> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardBalanceRow,
            "SELECT account_id, total_points_earned, total_points_redeemed, available_points \
             FROM reward_balances \
             WHERE account_id = $1",
            account.to_i64(),
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        row.map(RewardBalance::try_from)
            .transpose()
            .map_err(PortError::from)
    }

    /// One page of the reward history of `account`, newest first: ordered by
    /// `created_at` descending, then `accrual_id` descending, and returning up
    /// to `limit + 1` rows, so the caller can tell from the extra row whether
    /// a next page exists and take its `accrual_id` as `next_key`.
    ///
    /// `start_key` `None` starts at the newest accrual. `Some` starts at that
    /// accrual inclusive, and continues with the older ones. A start key that
    /// names no accrual of `account` gives an empty page, as the comparison
    /// with its missing row is `NULL`; the API answers it 200 with
    /// `items: []`.
    ///
    /// Each case is its own static statement, so the start row's
    /// `(created_at, accrual_id)`, read once by an uncorrelated sub-select, is
    /// a bound of the index scan in a generic prepared plan too, where a
    /// `$2 IS NULL OR …` predicate would leave it a filter.
    /// `reward_accruals_account_created_accrual_idx`, scanned backward, serves
    /// the account filter, the newest-first order and that bound, so a page
    /// reads about `limit + 1` index entries even inside a tie group of equal
    /// `created_at`.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the read fails or a stored accrual does
    /// not fit its domain type.
    pub async fn reward_history(
        &mut self,
        account: AccountId,
        start_key: Option<AccrualId>,
        limit: u16,
    ) -> PortResult<Vec<RewardAccrual>> {
        let rows_with_look_ahead = limit_i64(limit)?.checked_add(1).ok_or_else(|| {
            PortError::Unavailable("row limit exceeds the BIGINT range".to_owned())
        })?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = match start_key {
            None => {
                sqlx::query_as!(
                    RewardAccrualRow,
                    "SELECT accrual_id, account_id, transaction_id, rule_id, points_earned, \
                            created_at \
                     FROM reward_accruals \
                     WHERE account_id = $1 \
                     ORDER BY created_at DESC, accrual_id DESC \
                     LIMIT $2",
                    account.to_i64(),
                    rows_with_look_ahead,
                )
                .fetch_all(&mut *sp)
                .await
            }
            Some(start) => {
                sqlx::query_as!(
                    RewardAccrualRow,
                    "SELECT accrual_id, account_id, transaction_id, rule_id, points_earned, \
                            created_at \
                     FROM reward_accruals \
                     WHERE account_id = $1 \
                       AND (created_at, accrual_id) <= \
                           (SELECT created_at, accrual_id FROM reward_accruals \
                            WHERE accrual_id = $2 AND account_id = $1) \
                     ORDER BY created_at DESC, accrual_id DESC \
                     LIMIT $3",
                    account.to_i64(),
                    start.get(),
                    rows_with_look_ahead,
                )
                .fetch_all(&mut *sp)
                .await
            }
        };
        let rows = settle(sp, result).await?;
        rows.into_iter()
            .map(|row| RewardAccrual::try_from(row).map_err(PortError::from))
            .collect()
    }
}
