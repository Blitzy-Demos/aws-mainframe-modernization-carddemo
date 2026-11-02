//! The reward store port: the reads and writes `RewardEngine::accrue` performs
//! on the reward tables, declared as a trait the persistence adapter implements
//! (request Rewards system; AAP 0.3.5, 0.5.3.3, R3).
//!
//! Rewards are a new feature (o8) with no COBOL oracle: the README lists DB2
//! Rewards only as a roadmap item [README.md:L357-369], so no COBOL paragraph
//! and no parity audit row applies to this module. The keys it takes are the
//! copybook fields a rule matches on, `TRAN-TYPE-CD PIC X(02)` and
//! `TRAN-CAT-CD PIC 9(04)` [app/cpy/CVTRA05Y.cpy:L6-7], and the account key
//! `ACCT-ID PIC 9(11)` [app/cpy/CVACT01Y.cpy:L5] the balance belongs to; the
//! tables are those of `migrations/0005_rewards.sql`.
//!
//! # Signature form
//!
//! As in [`crate::ports`], every method is declared as
//! `fn name(&mut self, arg: T) -> impl Future<Output = PortResult<R>> + Send + '_`:
//! arguments are passed by value so the future borrows only `self`, the
//! explicit `Send` is what the multi-threaded tokio runtime of the API and
//! batch binaries needs, and no method is an `async fn`, which in a public
//! trait would raise the `async_fn_in_trait` lint. The adapter
//! (`PgRewardStore` in `crates/persistence/src/rewards.rs`, which borrows the
//! caller's `sqlx::Transaction`) still writes each method as `async fn` in its
//! `impl`. The engine takes the store through generics
//! (`S: RewardStore + Send`), never as a trait object.
//!
//! # Transactions
//!
//! Every method runs inside the caller's open database transaction, and the
//! store never commits or rolls back. CT02 (`POST /transactions`) commits the
//! accrual with the transaction insert, and `batch posttran` with the posting
//! updates of each record; on any error either caller rolls back its whole
//! unit (AAP 0.3.5, 0.6.4).

use std::future::Future;

use crate::entities::{NewAccrual, RewardBalance, RewardRule};
use crate::numeric::{AccountId, AccrualId, RewardPoints, TranCategoryCode, TranTypeCode};
use crate::ports::PortResult;

/// The reward tables as `RewardEngine::accrue` reads and writes them:
/// `reward_rules`, `reward_accruals` and `reward_balances` (AAP 0.3.5).
///
/// One accrual calls the methods in order: [`candidate_rules`], then, when a
/// rule wins, [`insert_accrual`] and [`add_points`]. Deciding which rule wins
/// and how many points it earns belongs to the engine alone; the store only
/// reads and records.
///
/// [`candidate_rules`]: RewardStore::candidate_rules
/// [`insert_accrual`]: RewardStore::insert_accrual
/// [`add_points`]: RewardStore::add_points
pub trait RewardStore {
    /// Every rule whose `transaction_type_cd` equals `type_cd` and whose
    /// `transaction_category_cd` equals `category_cd`, in any order.
    ///
    /// Active, future and soft-deleted rules are returned alike: the store
    /// applies no activity window and no amount bound, so the match is decided
    /// in one place, the engine's rule selection, against the processing clock
    /// the caller supplies. No rule of the pair is `Ok` with an empty `Vec`.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] on any failure of the read.
    ///
    /// [`PortError::Unavailable`]: crate::ports::PortError::Unavailable
    fn candidate_rules(
        &mut self,
        type_cd: TranTypeCode,
        category_cd: TranCategoryCode,
    ) -> impl Future<Output = PortResult<Vec<RewardRule>>> + Send + '_;

    /// Inserts `accrual` as one `reward_accruals` row and returns the
    /// `accrual_id` the database assigned it.
    ///
    /// The row stores every field of `accrual` exactly. `created_at`, which
    /// the caller takes from the processing clock, must be a whole microsecond
    /// of UTC time ([`is_whole_microsecond`]), the resolution of its
    /// `TIMESTAMPTZ` column; the processing clock guarantees it. A store
    /// refuses an instant the column cannot hold exactly instead of narrowing
    /// it, so the accrual the engine returns and the row read back carry the
    /// same instant.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when an accrual for the same `transaction_id`
    /// already exists: the `UNIQUE (transaction_id)` constraint
    /// `reward_accruals_transaction_id_key`, one row per qualifying
    /// transaction. [`PortError::Unavailable`] when `created_at` is not a
    /// whole microsecond, with nothing written, and on any other failure.
    ///
    /// [`is_whole_microsecond`]: crate::clock::is_whole_microsecond
    /// [`PortError::Duplicate`]: crate::ports::PortError::Duplicate
    /// [`PortError::Unavailable`]: crate::ports::PortError::Unavailable
    fn insert_accrual(
        &mut self,
        accrual: NewAccrual,
    ) -> impl Future<Output = PortResult<AccrualId>> + Send + '_;

    /// Credits `points` to the `reward_balances` row of `account_id`, creating
    /// the row when the account has none, and returns the balance after the
    /// update.
    ///
    /// `total_points_earned` grows by `points`; `total_points_redeemed` is left
    /// unchanged, and as nothing redeems points (AAP 0.2.5) it is never written
    /// above 0; `available_points` becomes
    /// `total_points_earned - total_points_redeemed`, the value the table's
    /// `reward_balances_available_points_check` requires and the engine's
    /// `credit` computes. A credit of zero points, from a winning rule of rate
    /// 0, still creates the row of an account that had none.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the new earned total exceeds the
    /// `BIGINT` column, and on any other failure.
    ///
    /// [`PortError::Unavailable`]: crate::ports::PortError::Unavailable
    fn add_points(
        &mut self,
        account_id: AccountId,
        points: RewardPoints,
    ) -> impl Future<Output = PortResult<RewardBalance>> + Send + '_;
}

/// An in-memory [`RewardStore`] for the unit tests of the rewards module, and
/// the helper that drives its futures without an async runtime: the domain
/// crate has none, not even as a dev-dependency (R3).
#[cfg(test)]
pub(crate) mod memory {
    use std::future::{self, Future};
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::RewardStore;
    use crate::entities::{NewAccrual, RewardAccrual, RewardBalance, RewardRule};
    use crate::numeric::{AccountId, AccrualId, RewardPoints, TranCategoryCode, TranTypeCode};
    use crate::ports::{PortError, PortResult};
    use crate::rewards::engine;

    /// The three reward tables in memory, with the constraints the store
    /// contract names: one accrual per `transaction_id`, and balances that
    /// hold `available = earned - redeemed`.
    ///
    /// Each method does its work when it is called and returns an already
    /// ready future, which is indistinguishable for a caller that awaits each
    /// call at once, as the engine does.
    #[derive(Debug)]
    pub(crate) struct MemoryStore {
        /// The `reward_rules` rows.
        pub(crate) rules: Vec<RewardRule>,
        /// The `reward_accruals` rows, in insertion order.
        pub(crate) accruals: Vec<RewardAccrual>,
        /// The `reward_balances` rows, one per account.
        pub(crate) balances: Vec<RewardBalance>,
        /// The number of port calls made, so a test can assert that a path
        /// reaches no store call.
        pub(crate) calls: usize,
        /// The identity the next inserted accrual receives.
        pub(crate) next_accrual_id: i64,
        /// An error the next [`RewardStore::insert_accrual`] returns, taken by
        /// that call.
        pub(crate) fail_insert: Option<PortError>,
    }

    impl MemoryStore {
        /// A store holding `rules`, without accruals or balances; accrual
        /// identities start at 1, as the `GENERATED ALWAYS AS IDENTITY` column
        /// does.
        pub(crate) fn with_rules(rules: Vec<RewardRule>) -> Self {
            Self {
                rules,
                accruals: Vec::new(),
                balances: Vec::new(),
                calls: 0,
                next_accrual_id: 1,
                fail_insert: None,
            }
        }

        fn insert(&mut self, accrual: &NewAccrual) -> PortResult<AccrualId> {
            if let Some(error) = self.fail_insert.take() {
                return Err(error);
            }
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

        fn credit(
            &mut self,
            account_id: AccountId,
            points: RewardPoints,
        ) -> PortResult<RewardBalance> {
            let current = self
                .balances
                .iter()
                .find(|balance| balance.account_id == account_id)
                .cloned()
                .unwrap_or_else(|| RewardBalance::zero(account_id));
            // The adapter reports a total beyond its BIGINT column as
            // `Unavailable`; the in-memory store reports the same.
            let updated = engine::credit(&current, points).map_err(|overflow| {
                PortError::Unavailable(format!("reward_balances.total_points_earned: {overflow}"))
            })?;
            if let Some(stored) = self
                .balances
                .iter_mut()
                .find(|balance| balance.account_id == account_id)
            {
                stored.clone_from(&updated);
            } else {
                self.balances.push(updated.clone());
            }
            Ok(updated)
        }
    }

    impl RewardStore for MemoryStore {
        fn candidate_rules(
            &mut self,
            type_cd: TranTypeCode,
            category_cd: TranCategoryCode,
        ) -> impl Future<Output = PortResult<Vec<RewardRule>>> + Send + '_ {
            self.calls += 1;
            let rules = self
                .rules
                .iter()
                .filter(|rule| {
                    rule.transaction_type_cd == type_cd
                        && rule.transaction_category_cd == category_cd
                })
                .cloned()
                .collect();
            future::ready(Ok(rules))
        }

        fn insert_accrual(
            &mut self,
            accrual: NewAccrual,
        ) -> impl Future<Output = PortResult<AccrualId>> + Send + '_ {
            self.calls += 1;
            future::ready(self.insert(&accrual))
        }

        fn add_points(
            &mut self,
            account_id: AccountId,
            points: RewardPoints,
        ) -> impl Future<Output = PortResult<RewardBalance>> + Send + '_ {
            self.calls += 1;
            future::ready(self.credit(account_id, points))
        }
    }

    /// The output of `future`, which must be ready on its first poll.
    ///
    /// The futures of [`MemoryStore`] always are, and so is any `async fn`
    /// that awaits only them, such as the engine's accrual, so no runtime and
    /// no real waker are needed.
    ///
    /// # Panics
    ///
    /// When `future` is still pending after its first poll.
    pub(crate) fn poll_ready<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        match pin!(future).poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => {
                panic!(
                    "the future was pending on its first poll; MemoryStore futures are always ready"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use rust_decimal_macros::dec;

    use super::RewardStore;
    use super::memory::{MemoryStore, poll_ready};
    use crate::entities::{NewAccrual, NewRewardRule, RewardAccrual, RewardBalance, RewardRule};
    use crate::numeric::{
        AccountId, AccrualId, RewardPoints, RewardRate, RuleId, TranCategoryCode, TranTypeCode,
        TransactionRowId,
    };
    use crate::ports::{PortError, PortResult};

    /// Compiles only when `T` is `Send`, as the multi-threaded runtime of the
    /// adapters requires of every port future.
    fn assert_send<T: Send>(_: &T) {}

    fn midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("a valid UTC instant")
    }

    fn type_code(text: &str) -> TranTypeCode {
        TranTypeCode::move_from(text)
    }

    fn category(code: u16) -> TranCategoryCode {
        TranCategoryCode::new(code).expect("a 9(04) category code")
    }

    fn account(value: u64) -> AccountId {
        AccountId::new(value).expect("a 9(11) account number")
    }

    fn points(value: i64) -> RewardPoints {
        RewardPoints::try_from(value).expect("non-negative points")
    }

    /// A rule of the fixture codes (`app/data/ASCII/trantype.txt`,
    /// `app/data/ASCII/trancatg.txt`), active from 2022-01-01 until `active_to`.
    fn rule(
        rule_id: i64,
        type_cd: &str,
        category_cd: u16,
        active_to: Option<DateTime<Utc>>,
    ) -> RewardRule {
        RewardRule::from_new(
            RuleId::from(rule_id),
            &NewRewardRule {
                transaction_type_cd: type_code(type_cd),
                transaction_category_cd: category(category_cd),
                reward_rate: RewardRate::from_decimal(dec!(1.0000)).expect("a reward rate"),
                min_transaction_amount: None,
                max_transaction_amount: None,
                active_from: midnight(2022, 1, 1),
                active_to,
            },
        )
    }

    fn accrual(account_id: u64, transaction_id: i64, points_earned: i64) -> NewAccrual {
        NewAccrual {
            account_id: account(account_id),
            transaction_id: TransactionRowId::from(transaction_id),
            rule_id: RuleId::from(1),
            points_earned: points(points_earned),
            created_at: midnight(2022, 7, 1),
        }
    }

    /// One accrual's calls in the engine's order, through the port alone: `S`
    /// is known only as a [`RewardStore`], so each `assert_send` compiles only
    /// because the trait declares its futures `Send`.
    fn accrue_through_the_port<S: RewardStore>(
        store: &mut S,
        type_cd: TranTypeCode,
        category_cd: TranCategoryCode,
        new_accrual: NewAccrual,
    ) -> PortResult<(Vec<RewardRule>, AccrualId, RewardBalance)> {
        let (account_id, points_earned) = (new_accrual.account_id, new_accrual.points_earned);

        let candidates = store.candidate_rules(type_cd, category_cd);
        assert_send(&candidates);
        let rules = poll_ready(candidates)?;

        let insert = store.insert_accrual(new_accrual);
        assert_send(&insert);
        let accrual_id = poll_ready(insert)?;

        let credit = store.add_points(account_id, points_earned);
        assert_send(&credit);
        let balance = poll_ready(credit)?;

        Ok((rules, accrual_id, balance))
    }

    #[test]
    fn candidate_rules_returns_every_rule_of_the_pair_whatever_its_window() {
        let retail = rule(1, "01", 1, None);
        let retail_deleted = rule(2, "01", 1, Some(midnight(2022, 6, 1)));
        let other_category = rule(3, "01", 2, None);
        let other_type = rule(4, "04", 1, None);
        let travel = rule(5, "04", 3, None);
        let mut store = MemoryStore::with_rules(vec![
            retail.clone(),
            retail_deleted.clone(),
            other_category,
            other_type,
            travel.clone(),
        ]);

        let cases = [
            ("01", 1, vec![retail, retail_deleted]),
            ("04", 3, vec![travel]),
            ("02", 1, Vec::new()),
        ];
        for (type_cd, category_cd, expected) in cases {
            let future = store.candidate_rules(type_code(type_cd), category(category_cd));
            assert_send(&future);
            assert_eq!(poll_ready(future), Ok(expected), "{type_cd}/{category_cd}");
        }
        assert_eq!(store.calls, 3);
        assert!(store.accruals.is_empty() && store.balances.is_empty());
    }

    #[test]
    fn a_second_accrual_for_the_same_transaction_is_a_duplicate() {
        let mut store = MemoryStore::with_rules(Vec::new());
        let first = accrual(1, 7, 51);

        let future = store.insert_accrual(first.clone());
        assert_send(&future);
        assert_eq!(poll_ready(future), Ok(AccrualId::from(1)));
        assert_eq!(
            poll_ready(store.insert_accrual(accrual(2, 7, 10))),
            Err(PortError::Duplicate)
        );
        assert_eq!(
            poll_ready(store.insert_accrual(accrual(1, 8, 300))),
            Ok(AccrualId::from(2))
        );

        assert_eq!(
            store.accruals,
            vec![
                RewardAccrual::from_new(AccrualId::from(1), &first),
                RewardAccrual::from_new(AccrualId::from(2), &accrual(1, 8, 300)),
            ]
        );
        assert_eq!(store.calls, 3);
    }

    #[test]
    fn a_failed_insert_returns_its_error_once_and_stores_nothing() {
        let mut store = MemoryStore::with_rules(Vec::new());
        let failure = PortError::Unavailable("connection reset".to_owned());
        store.fail_insert = Some(failure.clone());

        assert_eq!(
            poll_ready(store.insert_accrual(accrual(1, 7, 51))),
            Err(failure)
        );
        assert!(store.accruals.is_empty());
        assert_eq!(
            poll_ready(store.insert_accrual(accrual(1, 7, 51))),
            Ok(AccrualId::from(1))
        );
        assert_eq!(store.next_accrual_id, 2);
    }

    #[test]
    fn add_points_sums_the_earned_points_and_never_redeems() {
        let mut store = MemoryStore::with_rules(Vec::new());

        let future = store.add_points(account(1), points(51));
        assert_send(&future);
        let first = poll_ready(future).expect("a credit within the BIGINT range");
        assert_eq!(first.total_points_earned, points(51));

        let balance = poll_ready(store.add_points(account(1), points(300)))
            .expect("a credit within the BIGINT range");
        assert_eq!(balance.account_id, account(1));
        assert_eq!(balance.total_points_earned, points(351));
        assert_eq!(balance.total_points_redeemed, RewardPoints::ZERO);
        assert_eq!(balance.available_points, points(351));
        assert!(balance.is_consistent());

        // A rate-0 winner credits zero points and still creates the row.
        let zero =
            poll_ready(store.add_points(account(2), RewardPoints::ZERO)).expect("a zero credit");
        assert_eq!(zero, RewardBalance::zero(account(2)));

        assert_eq!(store.balances, vec![balance, zero]);
        assert_eq!(store.calls, 3);
    }

    #[test]
    fn an_earned_total_beyond_bigint_is_an_adapter_error() {
        let full = RewardBalance::new(account(1), points(i64::MAX), RewardPoints::ZERO)
            .expect("a consistent balance");
        let mut store = MemoryStore::with_rules(Vec::new());
        store.balances.push(full.clone());

        let result = poll_ready(store.add_points(account(1), points(1)));
        assert!(
            matches!(result, Err(PortError::Unavailable(_))),
            "{result:?}"
        );
        assert_eq!(store.balances, vec![full]);
    }

    #[test]
    fn the_port_futures_are_send_for_a_store_known_only_by_its_trait() {
        let retail = rule(1, "01", 1, None);
        let mut store = MemoryStore::with_rules(vec![retail.clone()]);

        let (rules, accrual_id, balance) =
            accrue_through_the_port(&mut store, type_code("01"), category(1), accrual(1, 7, 51))
                .expect("every call succeeds");

        assert_eq!(rules, vec![retail]);
        assert_eq!(accrual_id, AccrualId::from(1));
        assert_eq!(
            Ok(balance),
            RewardBalance::new(account(1), points(51), RewardPoints::ZERO)
        );
        assert_eq!(store.calls, 3);
    }
}
