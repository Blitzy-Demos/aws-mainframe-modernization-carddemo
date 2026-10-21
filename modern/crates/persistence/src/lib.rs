#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! The only `PostgreSQL` client of `CardDemo`: the repository traits of [`repos`], codec B in
//! [`rows`], the value-free errors of [`error`], and the two executors that implement every
//! repository trait, [`PgStore`] (the pool) and [`PgTx`] (one open transaction).
//!
//! # Layering (R4)
//!
//! - This crate is the only SQL issuer. The `api` services, the `messaging` workers, the
//!   `carddemo-batch` jobs and `test-support` reach the database only through the repository
//!   traits; test doubles in other crates implement the same traits.
//! - Statements are written only with `sqlx::query!`, `sqlx::query_as!` and
//!   `sqlx::query_scalar!`, checked at build time against `DATABASE_URL` or, with
//!   `SQLX_OFFLINE=true`, against the committed `modern/.sqlx/` metadata. The nine paginated
//!   reads use `sqlx::query_file_as!` over `queries/*.sql`, the same files the `explain` test
//!   plans (Bounded Pagination).
//!
//! # No migrator (Single Schema Executor)
//!
//! `migrations/*.sql` is the schema's only source, and this crate never applies it. The one
//! schema executor is the migration entrypoint of the `runtime` crate, called only by the
//! `carddemo-migrate` binary and the `test-support` container harness. Nothing here embeds the
//! migrations or reaches that entrypoint, so a service built on this crate fails at its first
//! query against an unmigrated database rather than changing the schema.
//!
//! # Domain types in, domain types out
//!
//! Repository methods take and return `domain::records` values and `copybook` keys. The row
//! structs of [`rows`] are codec B, the lossless mapping between a record and a row, and only
//! the repositories build and bind them. [`domain`] is re-exported so that a crate depending on
//! this one alone, such as `test-support`, names records as `persistence::domain::records`.
//!
//! # Redaction (Cardholder Data Redaction)
//!
//! - A [`PersistenceError`] carries only an SQLSTATE and a constraint name, never a server
//!   message, a detail text or a bound value: `PostgreSQL` texts can quote the key of the
//!   failing row, such as a card number.
//! - Row structs derive neither `Debug` nor `Serialize`.
//! - A protected value reaches a bind parameter only through the `expose()` of its
//!   `domain::sensitive` wrapper. No password is stored, only its argon2id hash (E25).
//! - The `Debug` of [`PgStore`] and [`PgTx`] prints the type name alone, never the pool, its
//!   options or the database it reaches.
//!
//! # Transactions
//!
//! - [`PgStore`] runs each statement on a pooled connection in autocommit mode, except
//!   `replace_all`, which empties and reloads a table, and
//!   `PendingAuthSummaryRepository::lock_or_create`, which inserts and locks a summary: each of
//!   them opens and commits a transaction of its own. A row lock taken through [`PgStore`]
//!   therefore ends with its call.
//! - [`PgTx`] runs every statement inside the caller's transaction, from [`PgStore::begin`] to
//!   [`PgTx::commit`] or [`PgTx::rollback`], and its row locks hold until then. A failed
//!   statement aborts the whole transaction, so a caller that must go on after one runs it
//!   inside a [`PgTx::savepoint`].
//! - Repository methods take `&mut self`, because a `sqlx` transaction executes only through a
//!   mutable reference. A caller holds a [`PgStore`] by value and clones it, which copies only
//!   the pool handle, into a local `mut` binding.
//!
//! # The error log (AAP 0.5.2)
//!
//! `cssl_error_log` replaces TDQ `CSSL`, which `9500-LOG-ERROR` writes outside the CICS unit of
//! work (`COPAUA0C.cbl:983-1006`). Callers write it through [`PgStore`], never through the
//! authorization [`PgTx`] that holds the summary lock, so each row commits at once in its own
//! transaction, whether or not the authorization it records commits.

pub mod error;
pub mod repos;
pub mod rows;

use std::fmt;

use sqlx::{PgConnection, PgPool, Postgres, Transaction};

pub use domain;
pub use error::{DecodeKind, PersistenceError};
pub use repos::{
    AccountCustomerChain, AccountRepository, AuthFraudRepository, CardRepository,
    CardXrefRepository, ChainLink, CsslErrorLogRepository, CustomerRepository,
    Db2TransactionTypeCategoryRepository, Db2TransactionTypeRepository, DisclosureGroupRepository,
    PendingAuthDetailRepository, PendingAuthSummaryRepository, TranCatBalanceRepository,
    TransactionCategoryRepository, TransactionRepository, TransactionTypeRepository,
    UserRepository,
};

/// The connection pool, as the executor of every repository trait: each statement runs on a
/// pooled connection and commits on its own.
///
/// Cloning copies the pool handle, not the pool, so every clone shares the same connections.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// A store over `pool`, as the `runtime` crate opens it from `DATABASE_URL`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The pool, for the repository implementations of this crate and for callers that manage
    /// the pool itself, such as the `test-support` harness. No other crate issues SQL through
    /// it (R4).
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Opens a transaction on a connection taken from the pool, for a unit of work whose
    /// statements and row locks must commit or roll back together.
    ///
    /// The returned future owns a clone of the pool handle and borrows nothing from `self`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when no connection can be had: the pool timed out or is
    /// closed, or connecting failed. [`PersistenceError::Database`] when the server refuses to
    /// start the transaction.
    pub fn begin(
        &self,
    ) -> impl Future<Output = Result<PgTx<'static>, PersistenceError>> + Send + use<> {
        let pool = self.pool.clone();
        async move {
            let tx = pool.begin().await?;
            Ok(PgTx { tx })
        }
    }
}

// Hand-written: the derived form would print the pool's state and options.
impl fmt::Debug for PgStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgStore")
    }
}

/// One open transaction, or a savepoint inside one, as the executor of every repository trait.
///
/// Every statement runs inside it, and every row lock it takes holds until
/// [`commit`](Self::commit) or [`rollback`](Self::rollback). Dropping it without either rolls it
/// back: `sqlx` queues the rollback when the value drops, and `PostgreSQL` discards an open
/// transaction whose connection is lost. That is how the CP00 worker's summary lock is released,
/// with nothing written, when the process or the connection is lost after the request was
/// acknowledged and before the commit (i10, AAP 0.3.4.2).
pub struct PgTx<'c> {
    tx: Transaction<'c, Postgres>,
}

// The methods spell out `impl Future<..> + Send` rather than being `async fn`, so that `Send`,
// which the services and workers rely on, is part of each signature instead of being inferred.
// Each one starts the `sqlx` future first and only maps its result in the block that awaits it.
impl<'c> PgTx<'c> {
    /// Commits the transaction or, for a [`savepoint`](Self::savepoint), releases the savepoint
    /// and keeps its work in the enclosing transaction.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost;
    /// [`PersistenceError::Database`] when the server refuses the commit, as for a
    /// serialization failure (`40001`). The transaction is rolled back either way.
    pub fn commit(self) -> impl Future<Output = Result<(), PersistenceError>> + Send + 'c {
        let commit = self.tx.commit();
        async move { commit.await.map_err(PersistenceError::from) }
    }

    /// Rolls the transaction back or, for a [`savepoint`](Self::savepoint), rolls back to the
    /// savepoint: the work done since it was opened is undone and the enclosing transaction can
    /// go on, even after a failed statement.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost, which discards the
    /// transaction on the server; [`PersistenceError::Database`] when the server refuses the
    /// rollback.
    pub fn rollback(self) -> impl Future<Output = Result<(), PersistenceError>> + Send + 'c {
        let rollback = self.tx.rollback();
        async move { rollback.await.map_err(PersistenceError::from) }
    }

    /// Opens a savepoint inside this transaction, as a nested [`PgTx`] that borrows it until the
    /// savepoint is committed (released), rolled back or dropped (rolled back).
    ///
    /// A failed statement aborts a `PostgreSQL` transaction, so a caller that must go on after a
    /// statement fails runs that statement inside a savepoint and rolls back to it. CP00 does so
    /// for its writes after the reply is confirmed and the request acknowledged: it updates the
    /// summary on the outer transaction, inserts the detail inside a savepoint and, when the
    /// insert fails (`I004`, `IMS INSERT DETL FAILED`, such as a duplicate detail key), rolls
    /// back to the savepoint and commits the summary update alone. That is the COBOL's result,
    /// whose summary write stands without its detail because the task ends normally and CICS
    /// commits its unit of work (`COPAUA0C.cbl:334-336,463-465,925-931,1008-1009,1016-1022`,
    /// E29).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost;
    /// [`PersistenceError::Database`] when the server refuses the savepoint.
    pub fn savepoint(&mut self) -> impl Future<Output = Result<PgTx<'_>, PersistenceError>> + Send {
        // On a connection already inside a transaction, `sqlx` begins a savepoint.
        let begin = sqlx::Connection::begin(&mut *self.tx);
        async move {
            let tx = begin.await?;
            Ok(PgTx { tx })
        }
    }

    /// The transaction's connection, as the executor of the repository implementations. The
    /// transaction stays private, so no other crate can run a statement through it.
    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        &mut self.tx
    }
}

// Hand-written: only the type name is printed.
impl fmt::Debug for PgTx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgTx")
    }
}

// Every repository trait requires `Send`, and services and workers hold both executors across
// `.await` points of `Send` futures: a field that is not `Send` fails the build here.
const _: fn() = || {
    fn is_send<T: Send>() {}
    is_send::<PgStore>();
    is_send::<PgTx<'static>>();
};

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use sqlx::{Database, Postgres, TransactionManager};

    use super::{PersistenceError, PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A credential no rendering of a store may repeat.
    const PASSWORD: &str = "REDACTED_TEST_PASSWORD";

    /// A store whose pool connects lazily to an address nothing listens on; the tests that use
    /// it never acquire a connection. The pool's maintenance tasks need a Tokio runtime.
    fn lazy_store() -> PgStore {
        let options = PgConnectOptions::new()
            .host("127.0.0.1")
            .port(9)
            .username("carddemo")
            .password(PASSWORD)
            .database("carddemo");
        PgStore::new(PgPoolOptions::new().connect_lazy_with(options))
    }

    /// The nesting depth of `tx`'s connection: 1 for a transaction, one more per savepoint.
    fn depth(tx: &mut PgTx<'_>) -> usize {
        <Postgres as Database>::TransactionManager::get_transaction_depth(tx.conn())
    }

    /// Cardholder Data Redaction: neither the store nor a clone of it prints the pool, its
    /// options or the credentials it connects with.
    #[tokio::test]
    async fn store_debug_is_the_type_name_alone() {
        let store = lazy_store();
        for text in [format!("{store:?}"), format!("{:?}", store.clone())] {
            assert_eq!(text, "PgStore");
        }
        assert!(format!("{:?}", store.pool().connect_options()).contains(PASSWORD));
    }

    /// Clones share one pool, and `begin` on a closed pool reports the database unavailable.
    #[tokio::test]
    async fn clones_share_the_pool_and_a_closed_pool_is_unavailable() {
        let store = lazy_store();
        let copy = store.clone();
        store.pool().close().await;
        assert!(copy.pool().is_closed());
        assert_eq!(
            copy.begin().await.map(drop),
            Err(PersistenceError::Unavailable)
        );
    }

    /// `begin` opens a transaction and `savepoint` nests one on the same connection; each ends
    /// by commit, rollback or drop and leaves the enclosing transaction usable. Observed through
    /// the connection's nesting depth, so the test runs no SQL text of its own.
    #[tokio::test]
    async fn transactions_nest_savepoints_and_end_by_commit_rollback_or_drop() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());

        let mut tx = store.begin().await?;
        assert_eq!(format!("{tx:?}"), "PgTx");
        assert_eq!(depth(&mut tx), 1);

        let mut nested = tx.savepoint().await?;
        assert_eq!(format!("{nested:?}"), "PgTx");
        assert_eq!(depth(&mut nested), 2);
        nested.rollback().await?;
        assert_eq!(depth(&mut tx), 1);

        let mut nested = tx.savepoint().await?;
        let mut inner = nested.savepoint().await?;
        assert_eq!(depth(&mut inner), 3);
        inner.commit().await?;
        assert_eq!(depth(&mut nested), 2);
        nested.commit().await?;
        assert_eq!(depth(&mut tx), 1);

        drop(tx.savepoint().await?);
        assert_eq!(depth(&mut tx), 1);
        tx.commit().await?;

        store.begin().await?.rollback().await?;
        drop(store.begin().await?);
        let mut after_drop = store.begin().await?;
        assert_eq!(depth(&mut after_drop), 1);
        after_drop.commit().await?;
        Ok(())
    }
}
