//! The repository traits: one `<Entity>Repository` per table, the only way any other crate
//! reaches `PostgreSQL` (AAP 0.5.4, R4).
//!
//! # Role
//!
//! - Each child module holds the statements on one table and one trait over them, and is named
//!   after its table, as its codec B module in [`crate::rows`] is (`db2_*` for schema `db2`).
//! - Every trait is implemented by [`PgStore`](crate::PgStore), the pool, on which each
//!   statement commits on its own, and by [`PgTx`](crate::PgTx), one open transaction, on which
//!   every statement runs inside the caller's unit of work. Test doubles in other crates
//!   implement the same traits.
//! - The five traits the single-writer batch jobs use, [`AccountRepository`],
//!   [`CardXrefRepository`], [`DisclosureGroupRepository`], [`TranCatBalanceRepository`] and
//!   [`TransactionRepository`], are also implemented by [`PgSession`](crate::PgSession), the
//!   session that holds a job's writer claims: each statement commits on its own there too, on
//!   the session's one connection, so the claims fence every write (see "Single-writer claims"
//!   in the crate documentation).
//! - `api::services::*` (never the handlers: Thin Request Handlers), `messaging::workers::*`,
//!   `batch::jobs::*` and `test-support` reach the database only through these traits.
//! - Methods take and return `domain::records` values, `copybook` keys and the
//!   `domain::sensitive` wrappers. Rows are built and read back only through [`crate::rows`]
//!   (codec B), so no byte is trimmed, padded or re-encoded here.
//!
//! # Trait shape
//!
//! - Every method takes `&mut self`, because a `sqlx` transaction executes only through a
//!   mutable reference, and returns `impl Future<Output = Result<T, PersistenceError>> + Send`.
//! - No trait declares an `async fn`: on a public trait that trips rustc's `async_fn_in_trait`
//!   lint, warnings are denied, and the spelled-out return type makes `Send`, which services and
//!   workers rely on, part of every signature. Every trait also requires `Send` of its
//!   implementor.
//! - [`AccountRepository::find_with_customer`] is the one infallible method: its future yields an
//!   [`AccountCustomerChain`] whose [`ChainLink`]s carry each read's failure.
//!
//! # Statements (R4, Bounded Pagination)
//!
//! - Each statement is written once, in a private function over the executor (generic over
//!   `sqlx::PgExecutor`, or on one `&mut PgConnection` where several statements must share a
//!   connection), and every implementation calls it, so the offline cache in `modern/.sqlx/`
//!   holds one entry per statement.
//! - Statements are written only with the `sqlx` query macros (`query!`, `query_as!`,
//!   `query_scalar!`), checked at build time against the migrated schema or the offline cache.
//! - The nine paginated reads use `sqlx::query_file_as!` over `queries/*.sql`, the same files the
//!   `explain` test plans: `page_after` and `page_before` of [`CardRepository`],
//!   [`TransactionRepository`], [`UserRepository`] and [`Db2TransactionTypeRepository`], and
//!   [`PendingAuthDetailRepository::page_after`]. Each returns at most `limit` rows, bound
//!   verbatim. A page read's limit is the one its program's domain rules compute
//!   (`cocrdlic::ListPlan::limit`, `cotrn00c::Browse::limit`, `cousr00c::PageRead::fetch_limit`,
//!   `copaus0c::PageRequest::fetch_limit`, `cotrtlic::ReadQuery::limit`, in `domain::online`)
//!   from a page size the API has validated to `1..=100`: the page, plus at most the start or
//!   anchor record the program discards and the look-ahead row, so never more than the page size
//!   plus two. Two reads take a fixed limit of 2 instead: COPAUS1C's next detail, and the card
//!   list's look-ahead (COCRDLIC-019), which is not an extra row of the filtered page but a
//!   separate, unfiltered [`CardRepository::page_after`] from the page's last card
//!   (X-COCRDLIC-3).
//!
//! # Outcomes
//!
//! - A read of a missing key is `Ok(None)` (CICS `NOTFND`, IMS `GE`, `SQLCODE +100`).
//! - `update` and `delete` return `Ok(true)` only when exactly one row changed; `Ok(false)` is
//!   the same not-found condition.
//! - A duplicate key is [`PersistenceError::Duplicate`](crate::PersistenceError::Duplicate), a
//!   delete that a `RESTRICT` foreign key refuses is
//!   [`PersistenceError::Restrict`](crate::PersistenceError::Restrict); each names its
//!   constraint and carries no value.
//! - A failed statement aborts a [`PgTx`](crate::PgTx). A caller that must go on after an
//!   expected failure runs the statement inside [`PgTx::savepoint`](crate::PgTx::savepoint) and
//!   rolls back to it: CP00 for its detail insert (`I004`), COPAUS2C for the insert that its
//!   update follows on a duplicate key (COPAUS2C-008).
//! - `replace_all` empties a table and inserts every record in one transaction, its own on
//!   [`PgStore`](crate::PgStore) and [`PgSession`](crate::PgSession) and the caller's on
//!   [`PgTx`](crate::PgTx), as IDCAMS
//!   `DELETE`, `DEFINE CLUSTER` and `REPRO` replace a cluster (LOADS-014). No foreign key joins
//!   the tables the loads replace, so the loads run in any order (LOADS-025, AAP 0.3.3.4).
//!   Before the delete it locks the table `SHARE ROW EXCLUSIVE` until the transaction ends, so
//!   two replacements of one table run one after the other: the later one's delete removes the
//!   rows the earlier one committed, and the table ends with the input committed last, never
//!   the union of both (LOADS-040). Reads do not wait; row writes to the table wait for it.
//!
//! # Locking
//!
//! - `lock_by_id` of [`AccountRepository`], [`CustomerRepository`], [`CardRepository`] and
//!   [`UserRepository`] reads with `SELECT … FOR UPDATE`, as CICS `READ … UPDATE` holds a record
//!   for the task's `REWRITE` or `DELETE`.
//! - [`PendingAuthSummaryRepository::lock_or_create`] inserts the account's summary when it is
//!   absent and locks it, the i10 serialisation of every CP00 decision for the account
//!   (COPAUA0C-026, AAP 0.3.4.2).
//! - [`PendingAuthSummaryRepository::lock_next`] reads and locks the summary after a given key,
//!   as the purge's `GN` reads each root when it reaches it under IMS program isolation, so the
//!   purge's delete test sees the count CP00 last committed (CBPAUP0C-009).
//! - All three hold their row lock until the transaction ends only on a [`PgTx`](crate::PgTx); on
//!   [`PgStore`](crate::PgStore) and [`PgSession`](crate::PgSession) the lock ends with the
//!   call. No process-local lock is used: only the database serialises, so a second process
//!   sees every lock the first one holds.
//!
//! # Two transaction-type data versions (E23)
//!
//! [`TransactionTypeRepository`] and [`TransactionCategoryRepository`] serve the VSAM data
//! version, loaded from the fixtures and read only by `tranrept`.
//! [`Db2TransactionTypeRepository`] and [`Db2TransactionTypeCategoryRepository`] serve the Db2
//! data version in schema `db2`, read and written only by the ports of COTRTLIC, COTRTUPC and
//! COBTUPDT. Nothing copies between the two.
//!
//! # The error log (AAP 0.5.2)
//!
//! [`CsslErrorLogRepository`] is written through [`PgStore`](crate::PgStore), never through
//! the authorization transaction, so each row commits on its own, as a TDQ `CSSL` write stands
//! outside the CICS unit of work.
//!
//! # Rules
//!
//! - **Repository Method Naming:** no method name repeats the snake-case noun of its trait's
//!   entity (`find_by_id`, never one naming the entity), which
//!   `modern/crates/test-support/tests/repository_naming.rs` checks over every trait and every
//!   implementation.
//! - **Single Schema Executor:** this crate never applies its migrations. Only the migration
//!   entrypoint of the `runtime` crate does, run by `carddemo-migrate` and the `test-support`
//!   container harness.
//!
//! # Methods
//!
//! Keys are `copybook` pictures (`PicU`, `PicS`, `PicX`), `domain::sensitive::CardNumber` and
//! `chrono` dates; records are the `domain::records` types the traits name. Paginated reads take
//! `limit: i64`. `insert`, `update` and `replace_all` take the record, or a slice of records.
//!
//! | Trait | Table | Methods (key parameters) |
//! |---|---|---|
//! | [`AccountRepository`] | `accounts` | `find_with_customer(PicU<11>)` → [`AccountCustomerChain`]; `find_by_id(PicU<11>)`, `lock_by_id(PicU<11>)`; `find_all`; `insert`; `update` → `bool`; `replace_all` → `u64` |
//! | [`AuthFraudRepository`] | `auth_fraud` | `insert(&FraudRecord, NaiveDate)`; `update_flag(&CardNumber, NaiveDateTime, &PicX<1>, NaiveDate)` → `bool`; `find_by_id(&CardNumber, NaiveDateTime)` → record and report date |
//! | [`CardXrefRepository`] | `card_xref` | `find_by_id(&CardNumber)`; `find_by_acct_id(PicU<11>)`, `find_first_two_by_acct_id(PicU<11>)` → at most two rows; `find_all`; `insert`; `replace_all` → `u64` |
//! | [`CardRepository`] | `cards` | `find_by_id(&CardNumber)`, `lock_by_id(&CardNumber)`, `find_by_ref(CardRef)` → [`CardEntry`]; `page_after`, `page_before` (`Option<&CardNumber>` start, `Option<PicU<11>>` and `Option<&CardNumber>` filters, `limit`); `find_all`; `insert`; `update` → `bool`; `replace_all` → `u64` |
//! | [`CsslErrorLogRepository`] | `cssl_error_log` | `insert` → `i64` row ID; `find_all` → `(i64, ErrorLogRecord)` in write order |
//! | [`CustomerRepository`] | `customers` | `find_by_id(PicU<9>)`, `lock_by_id(PicU<9>)`; `find_all`; `insert`; `update` → `bool`; `replace_all` → `u64` |
//! | [`Db2TransactionTypeCategoryRepository`] | `db2.transaction_type_category` | `find_by_id(&PicX<2>, &PicX<4>)`; `find_all_by_type(&PicX<2>)`; `insert` |
//! | [`Db2TransactionTypeRepository`] | `db2.transaction_type` | `find_by_id(&PicX<2>)`; `page_after`, `page_before` (`&PicX<2>` start, `Option<&PicX<2>>` type filter, `Option<&str>` description pattern, `limit`); `count_matching` (the two filters) → `i64`; `insert`; `update` → `bool`; `delete(&PicX<2>)` → `bool` |
//! | [`DisclosureGroupRepository`] | `disclosure_groups` | `find_by_id(&PicX<10>, &PicX<2>, PicU<4>)`; `find_all`; `insert`; `replace_all` → `u64` |
//! | [`PendingAuthDetailRepository`] | `pending_auth_details` | `find_by_id(PicS<11>, PicS<5>, PicS<9>)`; `page_after(PicS<11>, PicS<5>, PicS<9>, limit)`; `find_all_by_acct_id(PicS<11>)`; `insert(PicS<11>, record)`; `update(PicS<11>, record)` → `bool`; `delete(PicS<11>, PicS<5>, PicS<9>)` → `bool` |
//! | [`PendingAuthSummaryRepository`] | `pending_auth_summaries` | `find_by_id(PicS<11>)`; `lock_or_create(template)` → locked record and whether this call created it; `lock_next(Option<PicS<11>>)` → the next record in key order, locked; `find_all`; `insert`; `update` → `bool`; `delete(PicS<11>)` → `bool`, its details deleted with it |
//! | [`TranCatBalanceRepository`] | `tran_cat_balances` | `find_by_id(PicU<11>, &PicX<2>, PicU<4>)`; `find_all`; `insert`; `update` → `bool`; `replace_all` → `u64` |
//! | [`TransactionCategoryRepository`] | `transaction_categories` | `find_by_id(&PicX<2>, PicU<4>)`; `find_all`; `insert`; `replace_all` → `u64` |
//! | [`TransactionTypeRepository`] | `transaction_types` | `find_by_id(&PicX<2>)`; `find_all`; `insert`; `replace_all` → `u64` |
//! | [`TransactionRepository`] | `transactions` | `find_by_id(&PicX<16>)`; `find_last`; `next_id` → `PicX<16>`; `insert`; `page_after(&PicX<16>, limit)`, `page_before(&PicX<16>, limit)`; `find_all_in_card_order`; `find_processed_between(NaiveDate, NaiveDate)`; `stream_processed_between(NaiveDate, NaiveDate)` → [`TransactionRows`] |
//! | [`UserRepository`] | `users` | `find_by_id(&PicX<8>)`, `lock_by_id(&PicX<8>)` → `UserEntry`; `page_after(&PicX<8>, limit)`, `page_before(&PicX<8>, limit)` → `UserListEntry`; `insert(&UserEntry)`; `update(&UserEntry)` → `bool`; `delete(&PicX<8>)` → `bool`; `replace_all(&[HashedUser])` → `u64` |
//!
//! Point reads return `Option` of the record, of [`CardEntry`] for cards. Multi-row reads return
//! a `Vec`, except `stream_processed_between`, whose [`TransactionRows`] yields the rows of
//! `find_processed_between` one at a time, each decoded as it is read: `find_all_in_card_order`,
//! `find_processed_between` and `stream_processed_between` by card number, then transaction ID;
//! `page_before` from its start downward, in descending key order; `page_after` and every other
//! `find_all` read in ascending key order.

pub mod accounts;
pub mod auth_fraud;
pub mod card_xref;
pub mod cards;
pub mod cssl_error_log;
pub mod customers;
pub mod db2_transaction_type_categories;
pub mod db2_transaction_types;
pub mod disclosure_groups;
pub mod pending_auth_details;
pub mod pending_auth_summaries;
pub mod tran_cat_balances;
pub mod transaction_categories;
pub mod transaction_types;
pub mod transactions;
pub mod users;

pub use accounts::{AccountCustomerChain, AccountRepository, ChainLink};
pub use auth_fraud::AuthFraudRepository;
pub use card_xref::CardXrefRepository;
pub use cards::{CardEntry, CardRepository};
pub use cssl_error_log::CsslErrorLogRepository;
pub use customers::CustomerRepository;
pub use db2_transaction_type_categories::Db2TransactionTypeCategoryRepository;
pub use db2_transaction_types::Db2TransactionTypeRepository;
pub use disclosure_groups::DisclosureGroupRepository;
pub use pending_auth_details::PendingAuthDetailRepository;
pub use pending_auth_summaries::PendingAuthSummaryRepository;
pub use tran_cat_balances::TranCatBalanceRepository;
pub use transaction_categories::TransactionCategoryRepository;
pub use transaction_types::TransactionTypeRepository;
pub use transactions::{TransactionRepository, TransactionRows};
pub use users::UserRepository;

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::time::Duration;

    use copybook::{FixedRecord, PicX};
    use domain::batch::loads::{HashedUser, LoadJob, decode_fixture, decode_usrsec};
    use domain::records::{
        AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
        TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord,
    };
    use test_support::fixtures::{dusrsecj_path, fixture_path};
    use test_support::postgres::start_postgres;

    use super::{
        AccountRepository, CardRepository, CardXrefRepository, CustomerRepository,
        DisclosureGroupRepository, TranCatBalanceRepository, TransactionCategoryRepository,
        TransactionTypeRepository, UserRepository,
    };
    use crate::{PersistenceError, PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// How long the later replacement is left running while the earlier one's transaction stays
    /// open; it must still be waiting when this has passed.
    const HELD: Duration = Duration::from_millis(300);

    /// Bound on a read made while a replacement holds its table: the read must not wait for the
    /// replacement, and the bound turns a wait into a failure instead of a test that never ends.
    const READ_BOUND: Duration = Duration::from_secs(10);

    /// PHC-shaped stand-in for the hash `domain::password::hash` computes: the repository stores
    /// whatever hash it is given, so no test calls argon2.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$Y29uY3VycmVudA$Y29uY3VycmVudGhhc2g";

    /// More rows than the `DUSRSECJ.jcl` deck holds, so one page lists every stored user.
    const USER_PAGE: i64 = 100;

    /// One of the nine tables the loads replace, driven through its repository's `replace_all`.
    ///
    /// Every repository trait names the method `replace_all`, so each implementation names its
    /// trait to choose one.
    trait Replaced: Clone + Send + Sync + 'static {
        /// `replace_all` inside the open transaction `tx`, the way `carddemo-batch` loads run.
        fn replace_in(
            tx: &mut PgTx<'_>,
            records: &[Self],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;

        /// `replace_all` on `store`, in a transaction of its own.
        fn replace(
            store: &mut PgStore,
            records: &[Self],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;

        /// Whether the committed table holds exactly `records`: one row for each, equal in
        /// every copybook-derived column, and no other row.
        fn holds_exactly(
            store: &mut PgStore,
            records: &[Self],
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;
    }

    /// Implements [`Replaced`] for a layout whose repository's `find_all` returns its records.
    macro_rules! replaced {
        ($($record:ty => $repository:ident;)+) => {$(
            impl Replaced for $record {
                fn replace_in(
                    tx: &mut PgTx<'_>,
                    records: &[Self],
                ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
                    $repository::replace_all(tx, records)
                }

                fn replace(
                    store: &mut PgStore,
                    records: &[Self],
                ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
                    $repository::replace_all(store, records)
                }

                async fn holds_exactly(
                    store: &mut PgStore,
                    records: &[Self],
                ) -> Result<bool, PersistenceError> {
                    Ok(same_rows(&$repository::find_all(store).await?, records))
                }
            }
        )+};
    }

    replaced! {
        AccountRecord => AccountRepository;
        CardXrefRecord => CardXrefRepository;
        CustomerRecord => CustomerRepository;
        DisclosureGroupRecord => DisclosureGroupRepository;
        TranCatBalanceRecord => TranCatBalanceRepository;
        TransactionCategoryRecord => TransactionCategoryRepository;
        TransactionTypeRecord => TransactionTypeRepository;
    }

    /// Cards are read back with their `card_ref` handles, which the comparison leaves out: the
    /// database assigns them anew on every insert (LOADS-027).
    impl Replaced for CardRecord {
        fn replace_in(
            tx: &mut PgTx<'_>,
            records: &[Self],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            CardRepository::replace_all(tx, records)
        }

        fn replace(
            store: &mut PgStore,
            records: &[Self],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            CardRepository::replace_all(store, records)
        }

        async fn holds_exactly(
            store: &mut PgStore,
            records: &[Self],
        ) -> Result<bool, PersistenceError> {
            let stored: Vec<CardRecord> = CardRepository::find_all(store)
                .await?
                .into_iter()
                .map(|(_, record)| record)
                .collect();
            Ok(same_rows(&stored, records))
        }
    }

    /// Users have no `find_all`: one page lists every stored user, and each is read back by its
    /// key with its hash.
    impl Replaced for HashedUser {
        fn replace_in(
            tx: &mut PgTx<'_>,
            records: &[Self],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            UserRepository::replace_all(tx, records)
        }

        fn replace(
            store: &mut PgStore,
            records: &[Self],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            UserRepository::replace_all(store, records)
        }

        async fn holds_exactly(
            store: &mut PgStore,
            records: &[Self],
        ) -> Result<bool, PersistenceError> {
            let listed = UserRepository::page_after(store, &PicX::spaces(), USER_PAGE).await?;
            if listed.len() != records.len() {
                return Ok(false);
            }
            for user in records {
                let record = &user.record;
                let Some(stored) = UserRepository::find_by_id(store, &record.id).await? else {
                    return Ok(false);
                };
                let same = stored.fname == record.fname
                    && stored.lname == record.lname
                    && stored.password_hash == user.password_hash
                    && stored.usr_type == record.usr_type
                    && stored.filler == record.filler;
                if !same {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }

    /// Whether `stored` holds exactly `expected`, in any order. Keys are unique on both sides,
    /// so equal lengths and every expected record present make the two the same set.
    fn same_rows<R: PartialEq>(stored: &[R], expected: &[R]) -> bool {
        stored.len() == expected.len() && expected.iter().all(|record| stored.contains(record))
    }

    /// The records of `job`'s fixture in `app/data/ASCII/`, read in place and decoded by
    /// codec A, in file order.
    fn fixture<R: FixedRecord>(job: LoadJob) -> Result<Vec<R>, Box<dyn StdError>> {
        let file = job.fixture_file().ok_or("the job loads no fixture file")?;
        let bytes = std::fs::read(fixture_path(file))?;
        Ok(decode_fixture::<R>(&bytes)?
            .into_iter()
            .map(|framed| framed.record)
            .collect())
    }

    /// The ten in-stream records of `app/jcl/DUSRSECJ.jcl`, read in place and decoded as
    /// `dusrsecj` decodes them (LOADS-012), each with [`HASH`] for its password.
    fn deck() -> Result<Vec<HashedUser>, Box<dyn StdError>> {
        let jcl = std::fs::read(dusrsecj_path())?;
        Ok(decode_usrsec(&jcl)?
            .into_iter()
            .map(|framed| HashedUser {
                record: framed.record,
                password_hash: HASH.to_owned(),
            })
            .collect())
    }

    /// The row count a replacement with `records` reports.
    fn count<R>(records: &[R]) -> Result<u64, Box<dyn StdError>> {
        Ok(u64::try_from(records.len())?)
    }

    /// Two loads of `job`'s table at once, on a loaded database: the earlier one replaces the
    /// table with the first half of `all` inside a transaction it keeps open, the later one with
    /// the second half in a transaction of its own. The later one waits until the earlier
    /// commits, a reader meanwhile sees the previous contents without waiting, and once both
    /// have committed the table holds exactly the second half: no row of the first half or of
    /// the previous contents, and no duplicate-key failure.
    async fn last_writer_wins<R: Replaced>(store: &PgStore, job: LoadJob, all: &[R]) -> TestResult {
        let table = job.subcommand();
        let (first, second) = all.split_at(all.len() / 2);
        assert!(!first.is_empty(), "{table}: the first half holds records");
        let mut reader = store.clone();
        assert_eq!(R::replace(&mut reader, all).await?, count(all)?);

        let mut earlier = store.begin().await?;
        assert_eq!(R::replace_in(&mut earlier, first).await?, count(first)?);
        let mut writer = store.clone();
        let input = second.to_vec();
        let later = tokio::spawn(async move { R::replace(&mut writer, &input).await });
        tokio::time::sleep(HELD).await;
        assert!(
            !later.is_finished(),
            "{table}: the later load waits for the earlier one's transaction"
        );
        let unblocked = tokio::time::timeout(READ_BOUND, R::holds_exactly(&mut reader, all));
        assert!(
            unblocked.await??,
            "{table}: a reader sees the previous contents while both loads are open"
        );

        earlier.commit().await?;
        assert_eq!(later.await??, count(second)?);
        assert!(
            R::holds_exactly(&mut reader, second).await?,
            "{table}: the table holds exactly the input of the load that committed last"
        );
        Ok(())
    }

    /// Two replacements of one table at once, for each of the nine tables the loads replace: the
    /// later waits for the earlier, then replaces its committed contents, so the table ends with
    /// the input of the one that committed last, never the union of both.
    // audit: LOADS-014
    // audit: LOADS-040
    #[tokio::test]
    async fn concurrent_replacements_leave_the_input_committed_last() -> TestResult {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        for job in LoadJob::ALL {
            match job {
                LoadJob::Acctfile => {
                    last_writer_wins(&store, job, &fixture::<AccountRecord>(job)?).await?;
                }
                LoadJob::Cardfile => {
                    last_writer_wins(&store, job, &fixture::<CardRecord>(job)?).await?;
                }
                LoadJob::Xreffile => {
                    last_writer_wins(&store, job, &fixture::<CardXrefRecord>(job)?).await?;
                }
                LoadJob::Custfile => {
                    last_writer_wins(&store, job, &fixture::<CustomerRecord>(job)?).await?;
                }
                LoadJob::Discgrp => {
                    let records = fixture::<DisclosureGroupRecord>(job)?;
                    last_writer_wins(&store, job, &records).await?;
                }
                LoadJob::Tcatbalf => {
                    let records = fixture::<TranCatBalanceRecord>(job)?;
                    last_writer_wins(&store, job, &records).await?;
                }
                LoadJob::Trancatg => {
                    let records = fixture::<TransactionCategoryRecord>(job)?;
                    last_writer_wins(&store, job, &records).await?;
                }
                LoadJob::Trantype => {
                    let records = fixture::<TransactionTypeRecord>(job)?;
                    last_writer_wins(&store, job, &records).await?;
                }
                LoadJob::Dusrsecj => last_writer_wins(&store, job, &deck()?).await?,
            }
        }
        Ok(())
    }
}
