//! [`PgRewardStore`]: the `PostgreSQL` adapter of the reward store port
//! [`RewardStore`], the shared "Reward persistence" component (request
//! Rewards system; AAP 0.3.5, 0.6.4).
//!
//! `RewardEngine::accrue` (`carddemo_domain::rewards::engine`) is generic over
//! `S: RewardStore + Send`; this type is its only production store. It reads
//! the candidate rules of a (type, category) pair from `reward_rules`, inserts
//! the `reward_accruals` row of a qualifying transaction and upserts the
//! account's `reward_balances` row (`migrations/0005_rewards.sql`). Deciding
//! which rule wins and how many points it earns belongs to the engine alone.
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a roadmap
//! item [README.md:L357-369], so no parity audit row gates this module. The
//! keys it binds are the copybook fields a rule matches on, `TRAN-TYPE-CD PIC
//! X(02)` and `TRAN-CAT-CD PIC 9(04)` [app/cpy/CVTRA05Y.cpy:L6-7], and the
//! `ACCT-ID PIC 9(11)` [app/cpy/CVACT01Y.cpy:L5] a balance belongs to. The
//! category is stored as its integer value, so the fixture category `0001`
//! Regular Sales Draft [app/data/ASCII/trancatg.txt:L1] is the column value 1.
//!
//! # Borrowed transaction (AAP 0.5.3.3)
//!
//! The store borrows the caller's open `sqlx::Transaction`, as [`PgStore`]
//! does, and never begins, commits or rolls back the transaction itself. It
//! comes from one of three places:
//!
//! - [`UnitOfWork::rewards`], for an adapter that holds the unit of work;
//! - [`PgStore::reward_store`], which re-borrows the transaction of a
//!   [`PgStore`] a rule already works with, so one rule can post through the
//!   repository ports and then hand the same transaction to the engine;
//! - [`PgRewardStore::new`], for a caller that holds a transaction directly,
//!   such as a test.
//!
//! # Atomicity
//!
//! The caller owns commit and rollback. CT02 (`POST /transactions`) commits
//! the transaction insert, its accrual and the balance update together, and
//! rolls all three back on any error; POSTTRAN (`batch posttran`) does the same
//! for each posted record (AAP 0.3.5). Each statement here, the candidate-rule
//! read included, runs under its own savepoint through `crate::tx::savepoint`
//! and `crate::tx::settle`, so a failed read, accrual insert or balance upsert
//! is undone alone and never leaves the outer transaction aborted before the
//! caller decides what to do with it.
//!
//! # Values
//!
//! Points cross the `BIGINT` columns only as `RewardPoints` (`to_i64` and
//! `TryFrom<i64>`), amounts the `NUMERIC(11,2)` bounds only as `Money<9>`, and
//! rates the `NUMERIC(9,4)` column only as `RewardRate`; nothing is a
//! floating-point value. An accrual's `created_at` is bound only as the whole
//! microsecond its `TIMESTAMPTZ` column stores exactly, and is refused, never
//! narrowed, when it is finer. A stored value that does not fit its domain
//! type is [`PersistenceError::Corrupt`], which a port method reports as
//! `PortError::Unavailable` with the table and column named, never the value.
//!
//! The SQL names the reward tables only. The `transactions` row an accrual
//! belongs to is enforced by the foreign key
//! `reward_accruals_transaction_id_fkey`, so nothing here reads it.
//!
//! # Tracing
//!
//! Spans belong to the adapters that call the engine (AAP 0.6.2). This module
//! emits only `debug` events carrying keys, never a balance.

use carddemo_domain::entities::{NewAccrual, RewardBalance, RewardRule};
use carddemo_domain::numeric::{
    AccountId, AccrualId, RewardPoints, RuleId, TranCategoryCode, TranTypeCode,
};
use carddemo_domain::ports::{PortError, PortResult};
use carddemo_domain::rewards::RewardStore;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Postgres, Transaction};

use crate::repos::rewards::ACCRUALS_TABLE;
use crate::tx::{Ledger, PgStore, UnitOfWork, savepoint, settle};
// The row conversions go through the crate's shared column helpers in
// `crate::repos` rather than copies of them, so a stored reward value is held
// to its domain type exactly as every repository holds its columns.
use crate::{PersistenceError, repos};

/// The rule table, as [`PersistenceError::Corrupt`] and
/// [`PersistenceError::InexactInstant`] name it.
pub(crate) const RULES_TABLE: &str = "reward_rules";

/// The balance table, as [`PersistenceError::Corrupt`] names it.
const BALANCES_TABLE: &str = "reward_balances";

/// The reward store port over one borrowed `PostgreSQL` transaction.
///
/// `'t` is the borrow of the transaction and `'c` the connection the
/// transaction runs on. The store holds the `&mut` borrow of the transaction
/// and the record of its statement savepoint (`crate::tx`), both `Send`, so
/// the `async fn` port methods give the `Send` futures the [`RewardStore`]
/// trait declares.
#[derive(Debug)]
pub struct PgRewardStore<'t, 'c> {
    tx: &'t mut Transaction<'c, Postgres>,
    /// The record the store's calls settle and update: that of the unit of
    /// work or [`PgStore`] it was taken from, or its own.
    ledger: Ledger<'t>,
}

impl<'t, 'c> PgRewardStore<'t, 'c> {
    /// Binds the store to a transaction the caller already holds.
    ///
    /// Every call made through the store belongs to that transaction, and the
    /// caller commits or rolls it back. The store keeps its own record of the
    /// statement savepoint, as [`PgStore::new`] does, so a call abandoned
    /// through it is settled by its next call but not by another store bound
    /// to the same transaction.
    #[must_use]
    pub fn new(tx: &'t mut Transaction<'c, Postgres>) -> Self {
        Self {
            tx,
            ledger: Ledger::own(),
        }
    }
}

impl UnitOfWork {
    /// The reward store bound to this unit of work's transaction.
    ///
    /// The store borrows the unit of work, so its accrual and balance writes
    /// commit or roll back with everything else the unit holds, and it shares
    /// the unit of work's record of the statement savepoint.
    #[must_use]
    pub fn rewards(&mut self) -> PgRewardStore<'_, 'static> {
        let (tx, ledger) = self.bound();
        PgRewardStore { tx, ledger }
    }
}

impl<'c> PgStore<'_, 'c> {
    /// The reward store bound to the same transaction as this port adapter.
    ///
    /// It re-borrows the transaction and the store's record of its statement
    /// savepoint for as long as the reward store lives, so a rule that
    /// recorded a transaction through this [`PgStore`] can hand the reward
    /// store to `RewardEngine::accrue` and keep using the [`PgStore`]
    /// afterwards.
    #[must_use]
    pub fn reward_store(&mut self) -> PgRewardStore<'_, 'c> {
        PgRewardStore {
            tx: &mut *self.tx,
            ledger: self.ledger.share(),
        }
    }
}

/// One `reward_rules` row as the query macros read it: every column, with the
/// types `PostgreSQL` gives them.
///
/// Crate-internal so that every reader of the rule table (the candidate read
/// here and the admin reads in `crate::repos::rewards`) selects into the same
/// row and converts it through the same `TryFrom<RewardRuleRow>` for
/// [`RewardRule`].
#[derive(Debug)]
pub(crate) struct RewardRuleRow {
    /// `rule_id BIGINT`, the identity.
    pub(crate) rule_id: i64,
    /// `transaction_type_cd VARCHAR(2)`, stored right-trimmed.
    pub(crate) transaction_type_cd: String,
    /// `transaction_category_cd INTEGER`, 0 to 9999.
    pub(crate) transaction_category_cd: i32,
    /// `reward_rate NUMERIC(9,4)`.
    pub(crate) reward_rate: Decimal,
    /// `min_transaction_amount NUMERIC(11,2) NULL`.
    pub(crate) min_transaction_amount: Option<Decimal>,
    /// `max_transaction_amount NUMERIC(11,2) NULL`.
    pub(crate) max_transaction_amount: Option<Decimal>,
    /// `active_from TIMESTAMPTZ`.
    pub(crate) active_from: DateTime<Utc>,
    /// `active_to TIMESTAMPTZ NULL`.
    pub(crate) active_to: Option<DateTime<Utc>>,
}

/// The rule a stored row holds, with each column held to its domain type.
///
/// A type code longer than `X(02)` or outside ISO 8859-1, a category outside
/// `9(04)`, a negative rate or one with a fifth fraction digit, and an amount
/// bound outside `S9(09)V99` are [`PersistenceError::Corrupt`] naming
/// `reward_rules` and the column. The table's checks admit none of them, so
/// the error means the stored data was changed outside them.
impl TryFrom<RewardRuleRow> for RewardRule {
    type Error = PersistenceError;

    fn try_from(row: RewardRuleRow) -> Result<Self, Self::Error> {
        Ok(Self {
            rule_id: RuleId::from(row.rule_id),
            transaction_type_cd: repos::tran_type_code(
                &row.transaction_type_cd,
                RULES_TABLE,
                "transaction_type_cd",
            )?,
            transaction_category_cd: repos::tran_category_code(
                row.transaction_category_cd,
                RULES_TABLE,
                "transaction_category_cd",
            )?,
            reward_rate: repos::reward_rate(row.reward_rate, RULES_TABLE, "reward_rate")?,
            min_transaction_amount: repos::opt_money(
                row.min_transaction_amount,
                RULES_TABLE,
                "min_transaction_amount",
            )?,
            max_transaction_amount: repos::opt_money(
                row.max_transaction_amount,
                RULES_TABLE,
                "max_transaction_amount",
            )?,
            active_from: row.active_from,
            active_to: row.active_to,
        })
    }
}

/// One `reward_balances` row as the query macros read it.
///
/// Crate-internal for the same reason as [`RewardRuleRow`]: the balance upsert
/// here and the balance read in `crate::repos::rewards` convert it through the
/// same `TryFrom<RewardBalanceRow>` for [`RewardBalance`].
#[derive(Debug)]
pub(crate) struct RewardBalanceRow {
    /// `account_id BIGINT`, the `ACCT-ID 9(11)` key.
    pub(crate) account_id: i64,
    /// `total_points_earned BIGINT`.
    pub(crate) total_points_earned: i64,
    /// `total_points_redeemed BIGINT`, never written above 0.
    pub(crate) total_points_redeemed: i64,
    /// `available_points BIGINT`.
    pub(crate) available_points: i64,
}

/// The balance a stored row holds, checked against the invariant
/// `available_points = total_points_earned - total_points_redeemed`.
///
/// The balance is built with [`RewardBalance::new`], which derives the
/// available points, and the stored `available_points` must equal them. An
/// account key outside `9(11)`, a negative total, redeemed points above the
/// earned points, or a stored `available_points` that differs from the derived
/// value is [`PersistenceError::Corrupt`] naming `reward_balances` and the
/// column. The table's checks admit none of them.
impl TryFrom<RewardBalanceRow> for RewardBalance {
    type Error = PersistenceError;

    fn try_from(row: RewardBalanceRow) -> Result<Self, Self::Error> {
        let account_id = repos::account_id(row.account_id, BALANCES_TABLE, "account_id")?;
        let earned = repos::reward_points(
            row.total_points_earned,
            BALANCES_TABLE,
            "total_points_earned",
        )?;
        let redeemed = repos::reward_points(
            row.total_points_redeemed,
            BALANCES_TABLE,
            "total_points_redeemed",
        )?;
        let available =
            repos::reward_points(row.available_points, BALANCES_TABLE, "available_points")?;
        let balance =
            Self::new(account_id, earned, redeemed).map_err(|_| PersistenceError::Corrupt {
                table: BALANCES_TABLE,
                column: "total_points_redeemed",
                reason: "redeemed points exceed the earned points".to_owned(),
            })?;
        if balance.available_points != available {
            return Err(PersistenceError::Corrupt {
                table: BALANCES_TABLE,
                column: "available_points",
                reason: "differs from total_points_earned - total_points_redeemed".to_owned(),
            });
        }
        Ok(balance)
    }
}

/// The reward tables as the engine reads and writes them, inside the caller's
/// transaction. The trait in `carddemo_domain::rewards::ports` states the
/// contract; each method below adds how this adapter meets it.
///
/// Failures map through the crate's SQLSTATE mapping (`crate::port_error`):
/// `23505` is `PortError::Duplicate`, `23503` `PortError::Restricted`, a lock
/// conflict `PortError::Contention`, and every other failure, a `BIGINT`
/// overflow (`22003`) included, `PortError::Unavailable`. The engine keeps the
/// error unchanged in `RewardError::Store`, and both callers roll back their
/// unit of work on any of them.
impl RewardStore for PgRewardStore<'_, '_> {
    /// Every `reward_rules` row of the pair, in `rule_id` order.
    ///
    /// Active, future and soft-deleted rules are returned alike, as the
    /// contract requires: the engine applies the active window at the
    /// processing clock, the amount bounds, and the highest-rate,
    /// lowest-`rule_id` choice (AAP 0.3.5). `reward_rules_match_idx` serves the
    /// equality on both codes. The type code is bound as its right-trimmed
    /// text, the form the column stores, and the category as its integer
    /// value. A pair without rules is an empty `Vec`.
    ///
    /// The read runs under its own savepoint, as every statement of the store
    /// does.
    ///
    /// # Errors
    ///
    /// The mapped error of a failed read, and `PortError::Unavailable` naming
    /// `reward_rules` and the column when a stored rule does not fit its
    /// domain type.
    async fn candidate_rules(
        &mut self,
        type_cd: TranTypeCode,
        category_cd: TranCategoryCode,
    ) -> PortResult<Vec<RewardRule>> {
        let type_text = type_cd.trimmed();
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardRuleRow,
            "SELECT rule_id, transaction_type_cd, transaction_category_cd, reward_rate, \
                    min_transaction_amount, max_transaction_amount, active_from, active_to \
             FROM reward_rules \
             WHERE transaction_type_cd = $1 AND transaction_category_cd = $2 \
             ORDER BY rule_id",
            type_text.as_ref(),
            i32::from(category_cd.get()),
        )
        .fetch_all(&mut *sp)
        .await;
        let rows = settle(sp, result).await?;
        rows.into_iter()
            .map(|row| RewardRule::try_from(row).map_err(PortError::from))
            .collect()
    }

    /// Inserts the `reward_accruals` row and returns the `accrual_id` the
    /// identity column assigned it.
    ///
    /// Every field of `accrual` is stored exactly as given. `created_at`, the
    /// caller's processing clock, must be a whole microsecond of UTC time, the
    /// resolution of the `TIMESTAMPTZ` column, which the processing clock
    /// guarantees; an instant the column cannot hold exactly is refused before
    /// the insert, never narrowed by the bind, so the accrual the engine
    /// returns and the row read back carry the same instant. The insert runs
    /// under its own savepoint: a rejected row is undone alone and leaves the
    /// outer transaction usable.
    ///
    /// # Errors
    ///
    /// - `PortError::Unavailable` naming `reward_accruals.created_at` when
    ///   `created_at` is not a whole microsecond of UTC time, with nothing
    ///   written.
    /// - `PortError::Duplicate` when the transaction already has an accrual:
    ///   the `UNIQUE (transaction_id)` constraint
    ///   `reward_accruals_transaction_id_key`, one row per qualifying
    ///   transaction.
    /// - `PortError::Restricted` when `transaction_id` names no `transactions`
    ///   row or `rule_id` no rule, the foreign keys
    ///   `reward_accruals_transaction_id_fkey` and
    ///   `reward_accruals_rule_id_fkey` (SQLSTATE `23503`).
    /// - The mapped error of any other failure, in practice
    ///   `PortError::Unavailable`.
    async fn insert_accrual(&mut self, accrual: NewAccrual) -> PortResult<AccrualId> {
        let NewAccrual {
            account_id,
            transaction_id,
            rule_id,
            points_earned,
            created_at,
        } = accrual;
        let created_at = repos::exact_instant(created_at, ACCRUALS_TABLE, "created_at")?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_scalar!(
            "INSERT INTO reward_accruals \
                 (account_id, transaction_id, rule_id, points_earned, created_at) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING accrual_id",
            account_id.to_i64(),
            transaction_id.get(),
            rule_id.get(),
            points_earned.to_i64(),
            created_at,
        )
        .fetch_one(&mut *sp)
        .await;
        let accrual_id = AccrualId::from(settle(sp, result).await?);
        tracing::debug!(
            %accrual_id,
            %transaction_id,
            %rule_id,
            "reward accrual inserted"
        );
        Ok(accrual_id)
    }

    /// Credits `points` to the `reward_balances` row of `account_id` in one
    /// upsert and returns the row as it stands afterwards.
    ///
    /// An account without a row gets one with `points` earned, nothing
    /// redeemed and `points` available; a credit of zero points, from a
    /// winning rule of rate 0, still creates it. An existing row adds `points`
    /// to `total_points_earned` and sets `available_points` to the new earned
    /// total minus `total_points_redeemed`, which the upsert leaves unchanged:
    /// nothing redeems points (AAP 0.2.5). The row lock the upsert takes holds
    /// a concurrent credit to the same account until this transaction ends, so
    /// no credit is lost.
    ///
    /// The returned row is rebuilt with [`RewardBalance::new`] and its stored
    /// `available_points` checked against the derived value, the invariant the
    /// table's `reward_balances_available_points_check` holds. The upsert runs
    /// under its own savepoint.
    ///
    /// # Errors
    ///
    /// - `PortError::Unavailable` when the new earned total exceeds the
    ///   `BIGINT` column (SQLSTATE `22003`), with nothing changed; the
    ///   addition happens in the database, so it never panics or wraps here.
    /// - `PortError::Restricted` when `account_id` names no `accounts` row,
    ///   the foreign key `reward_balances_account_id_fkey` (SQLSTATE `23503`).
    /// - `PortError::Unavailable` naming `reward_balances` and the column when
    ///   the returned row breaks the invariant or a column's domain type.
    /// - The mapped error of any other failure.
    async fn add_points(
        &mut self,
        account_id: AccountId,
        points: RewardPoints,
    ) -> PortResult<RewardBalance> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            RewardBalanceRow,
            "INSERT INTO reward_balances \
                 (account_id, total_points_earned, total_points_redeemed, available_points) \
             VALUES ($1, $2, 0, $2) \
             ON CONFLICT (account_id) DO UPDATE SET \
                 total_points_earned = \
                     reward_balances.total_points_earned + EXCLUDED.total_points_earned, \
                 available_points = \
                     reward_balances.total_points_earned + EXCLUDED.total_points_earned \
                     - reward_balances.total_points_redeemed \
             RETURNING account_id, total_points_earned, total_points_redeemed, available_points",
            account_id.to_i64(),
            points.to_i64(),
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        let balance = RewardBalance::try_from(row)?;
        tracing::debug!(%account_id, "reward balance credited");
        Ok(balance)
    }
}

// The SQL paths need a database and are covered by
// `crates/persistence/tests/rewards_store.rs`. These tests cover the row
// conversions, whose failure paths the table checks keep out of reach of any
// stored row.
#[cfg(test)]
mod tests {
    use carddemo_domain::entities::{RewardBalance, RewardRule};
    use carddemo_domain::numeric::{AccountId, Money, RewardPoints, RuleId, TranCategoryCode};
    use chrono::{DateTime, TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{RewardBalanceRow, RewardRuleRow};
    use crate::PersistenceError;

    fn midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("a valid UTC instant")
    }

    /// The retail seed rule (AAP 0.3.6): type `01`, category `0001`, rate
    /// 1.0000, from 2022-01-01, here with a 10.00 lower bound.
    fn retail_row() -> RewardRuleRow {
        RewardRuleRow {
            rule_id: 1,
            transaction_type_cd: "01".to_owned(),
            transaction_category_cd: 1,
            reward_rate: Decimal::new(10_000, 4),
            min_transaction_amount: Some(Decimal::new(1_000, 2)),
            max_transaction_amount: None,
            active_from: midnight(2022, 1, 1),
            active_to: Some(midnight(2022, 6, 1)),
        }
    }

    fn balance_row(earned: i64, redeemed: i64, available: i64) -> RewardBalanceRow {
        RewardBalanceRow {
            account_id: 1,
            total_points_earned: earned,
            total_points_redeemed: redeemed,
            available_points: available,
        }
    }

    /// The table and column of a `Corrupt` error, or a panic naming `case`.
    fn corrupt_place<T: std::fmt::Debug>(
        case: &str,
        result: Result<T, PersistenceError>,
    ) -> (&'static str, &'static str, String) {
        match result {
            Err(PersistenceError::Corrupt {
                table,
                column,
                reason,
            }) => (table, column, reason),
            other => panic!("{case}: expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn a_stored_rule_converts_field_by_field() {
        let rule = RewardRule::try_from(retail_row()).expect("a rule within its types");
        assert_eq!(rule.rule_id, RuleId::from(1));
        assert_eq!(rule.transaction_type_cd.as_bytes(), b"01");
        assert_eq!(
            Ok(rule.transaction_category_cd),
            TranCategoryCode::new(1),
            "category 0001 is stored as 1"
        );
        assert_eq!(rule.reward_rate.to_string(), "1.0000");
        assert_eq!(
            rule.min_transaction_amount,
            Money::<9>::from_cents(1_000).ok()
        );
        assert_eq!(rule.max_transaction_amount, None);
        assert_eq!(rule.active_from, midnight(2022, 1, 1));
        assert_eq!(rule.active_to, Some(midnight(2022, 6, 1)));

        let short = RewardRuleRow {
            transaction_type_cd: "1".to_owned(),
            ..retail_row()
        };
        let rule = RewardRule::try_from(short).expect("a one-character type code");
        assert_eq!(rule.transaction_type_cd.as_bytes(), b"1 ");
    }

    #[test]
    fn a_rule_column_outside_its_type_is_corrupt() {
        let cases = [
            (
                RewardRuleRow {
                    transaction_type_cd: "012".to_owned(),
                    ..retail_row()
                },
                "transaction_type_cd",
            ),
            (
                RewardRuleRow {
                    transaction_category_cd: 10_000,
                    ..retail_row()
                },
                "transaction_category_cd",
            ),
            (
                RewardRuleRow {
                    transaction_category_cd: -1,
                    ..retail_row()
                },
                "transaction_category_cd",
            ),
            (
                RewardRuleRow {
                    reward_rate: Decimal::new(-1, 0),
                    ..retail_row()
                },
                "reward_rate",
            ),
            (
                RewardRuleRow {
                    reward_rate: Decimal::new(100_001, 5),
                    ..retail_row()
                },
                "reward_rate",
            ),
            (
                RewardRuleRow {
                    min_transaction_amount: Some(Decimal::new(100_000_000_000, 2)),
                    ..retail_row()
                },
                "min_transaction_amount",
            ),
            (
                RewardRuleRow {
                    max_transaction_amount: Some(Decimal::new(1_001, 3)),
                    ..retail_row()
                },
                "max_transaction_amount",
            ),
        ];
        for (row, expected_column) in cases {
            let (table, column, _) = corrupt_place(expected_column, RewardRule::try_from(row));
            assert_eq!((table, column), ("reward_rules", expected_column));
        }
    }

    #[test]
    fn a_consistent_balance_row_converts_and_keeps_large_totals_exact() {
        let points = |value: i64| RewardPoints::try_from(value).expect("non-negative points");
        let account = AccountId::new(1).expect("a 9(11) account number");
        for (earned, available) in [
            (0, 0),
            (42, 42),
            (9_007_199_254_740_993, 9_007_199_254_740_993),
        ] {
            let balance = RewardBalance::try_from(balance_row(earned, 0, available))
                .expect("a consistent row");
            assert_eq!(
                Ok(balance.clone()),
                RewardBalance::new(account, points(earned), RewardPoints::ZERO)
            );
            assert_eq!(balance.available_points.to_string(), available.to_string());
        }
    }

    #[test]
    fn a_balance_row_that_breaks_the_invariant_or_a_type_is_corrupt() {
        let derived = "differs from total_points_earned - total_points_redeemed";
        let negative_redeemed = "redeemed points exceed the earned points";
        let cases = [
            (
                "available too high",
                balance_row(40, 0, 41),
                "available_points",
                Some(derived),
            ),
            (
                "available too low",
                balance_row(40, 0, 39),
                "available_points",
                Some(derived),
            ),
            (
                "redeemed above earned",
                balance_row(1, 2, 0),
                "total_points_redeemed",
                Some(negative_redeemed),
            ),
            (
                "negative earned",
                balance_row(-1, 0, 0),
                "total_points_earned",
                None,
            ),
            (
                "negative available",
                balance_row(0, 0, -1),
                "available_points",
                None,
            ),
            (
                "account beyond 9(11)",
                RewardBalanceRow {
                    account_id: 100_000_000_000,
                    ..balance_row(0, 0, 0)
                },
                "account_id",
                None,
            ),
        ];
        for (case, row, expected_column, expected_reason) in cases {
            let (table, column, reason) = corrupt_place(case, RewardBalance::try_from(row));
            assert_eq!(
                (table, column),
                ("reward_balances", expected_column),
                "{case}"
            );
            if let Some(expected) = expected_reason {
                assert_eq!(reason, expected, "{case}");
            }
        }
    }
}
