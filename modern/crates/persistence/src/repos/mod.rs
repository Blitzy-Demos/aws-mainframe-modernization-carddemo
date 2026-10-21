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
//!   connection), and both implementations call it, so the offline cache in `modern/.sqlx/`
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
//!   [`PgStore`](crate::PgStore) and the caller's on [`PgTx`](crate::PgTx), as IDCAMS
//!   `DELETE`, `DEFINE CLUSTER` and `REPRO` replace a cluster (LOADS-014). No foreign key joins
//!   the tables the loads replace, so the loads run in any order (LOADS-025, AAP 0.3.3.4).
//!
//! # Locking
//!
//! - `lock_by_id` of [`AccountRepository`], [`CustomerRepository`], [`CardRepository`] and
//!   [`UserRepository`] reads with `SELECT … FOR UPDATE`, as CICS `READ … UPDATE` holds a record
//!   for the task's `REWRITE` or `DELETE`.
//! - [`PendingAuthSummaryRepository::lock_or_create`] inserts the account's summary when it is
//!   absent and locks it, the i10 serialisation of every CP00 decision for the account
//!   (COPAUA0C-026, AAP 0.3.4.2).
//! - Both hold their row lock until the transaction ends only on a [`PgTx`](crate::PgTx); on
//!   [`PgStore`](crate::PgStore) the lock ends with the call. No process-local lock is used:
//!   only the database serialises, so a second process sees every lock the first one holds.
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
//! | [`PendingAuthSummaryRepository`] | `pending_auth_summaries` | `find_by_id(PicS<11>)`; `lock_or_create(template)` → locked record and whether this call created it; `find_all`; `insert`; `update` → `bool`; `delete(PicS<11>)` → `bool`, its details deleted with it |
//! | [`TranCatBalanceRepository`] | `tran_cat_balances` | `find_by_id(PicU<11>, &PicX<2>, PicU<4>)`; `find_all`; `insert`; `update` → `bool`; `replace_all` → `u64` |
//! | [`TransactionCategoryRepository`] | `transaction_categories` | `find_by_id(&PicX<2>, PicU<4>)`; `find_all`; `insert`; `replace_all` → `u64` |
//! | [`TransactionTypeRepository`] | `transaction_types` | `find_by_id(&PicX<2>)`; `find_all`; `insert`; `replace_all` → `u64` |
//! | [`TransactionRepository`] | `transactions` | `find_by_id(&PicX<16>)`; `find_last`; `next_id` → `PicX<16>`; `insert`; `page_after(&PicX<16>, limit)`, `page_before(&PicX<16>, limit)`; `find_all_in_card_order`; `find_processed_between(NaiveDate, NaiveDate)` |
//! | [`UserRepository`] | `users` | `find_by_id(&PicX<8>)`, `lock_by_id(&PicX<8>)` → `UserEntry`; `page_after(&PicX<8>, limit)`, `page_before(&PicX<8>, limit)` → `UserListEntry`; `insert(&UserEntry)`; `update(&UserEntry)` → `bool`; `delete(&PicX<8>)` → `bool`; `replace_all(&[HashedUser])` → `u64` |
//!
//! Point reads return `Option` of the record, of [`CardEntry`] for cards. Multi-row reads return
//! a `Vec`: `find_all_in_card_order` and `find_processed_between` by card number, then
//! transaction ID; `page_before` from its start downward, in descending key order; `page_after`
//! and every other `find_all` read in ascending key order.

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
pub use transactions::TransactionRepository;
pub use users::UserRepository;
