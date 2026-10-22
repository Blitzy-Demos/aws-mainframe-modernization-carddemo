#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! The only `PostgreSQL` client of `CardDemo`: the repository traits of [`repos`], codec B in
//! [`rows`], the value-free errors of [`error`], the two executors that implement every
//! repository trait, [`PgStore`] (the pool) and [`PgTx`] (one open transaction), and a third,
//! [`PgSession`] (one session detached from the pool), which implements the five traits the
//! single-writer batch jobs use and holds their writer claims (see "Single-writer claims").
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
//! - The `Debug` of [`PgStore`], [`PgTx`] and [`PgSession`] prints the type name alone, never
//!   the pool, its options, the session or the database it reaches.
//!
//! # Transactions
//!
//! - [`PgStore`] runs each statement on a pooled connection in autocommit mode, except
//!   `replace_all`, which empties and reloads a table under a `SHARE ROW EXCLUSIVE` table
//!   lock, so replacements of one table run one at a time (LOADS-040), and
//!   `PendingAuthSummaryRepository::lock_or_create`, which inserts and locks a summary: each of
//!   them opens and commits a transaction of its own. A row lock taken through [`PgStore`]
//!   therefore ends with its call.
//! - [`PgTx`] runs every statement inside the caller's transaction, from [`PgStore::begin`] to
//!   [`PgTx::commit`] or [`PgTx::rollback`], and its row locks hold until then. A failed
//!   statement aborts the whole transaction, so a caller that must go on after one runs it
//!   inside a [`PgTx::savepoint`].
//! - [`PgSession`] runs each statement on its one connection in autocommit mode, as [`PgStore`]
//!   does on the pool, so every write commits as it completes; its `replace_all` opens and
//!   commits a transaction of its own on that connection, as [`PgStore`]'s does on a pooled one.
//!   The connection is never replaced: once the session is lost, every later statement on it
//!   fails.
//! - Repository methods take `&mut self`, because a `sqlx` transaction executes only through a
//!   mutable reference. A caller holds a [`PgStore`] by value and clones it, which copies only
//!   the pool handle, into a local `mut` binding.
//!
//! # Single-writer claims
//!
//! - The transaction, account and category-balance clusters are defined `SHAREOPTIONS(2 3)`
//!   (`TRANFILE.jcl:55`, `ACCTFILE.jcl:42`, `TCATBALF.jcl:42`): any number of jobs may read
//!   one, but only one may have it open for output or update, and VSAM fails a second writer's
//!   `OPEN` at once with file status `93`, resource not available. `posttran` and `intcalc`
//!   read those tables whole when they start and rewrite rows with the values computed from
//!   that read, so two writers running at once would overwrite each other's updates.
//!   [`PgSession::claim_writer`] restores the rule: the claim on a [`Dataset`] is held by one
//!   session at a time, and a second claim is refused at once, never queued (CBTRN02C-046,
//!   CBACT04C-039). Readers take no claim, as the share options let them read beside the
//!   writer.
//! - A claim is a session-level `PostgreSQL` advisory lock, taken with `pg_try_advisory_lock`
//!   under the key [`Dataset::lock_key`] names, on a [`PgSession`]: a connection detached from
//!   the pool, on which the job then runs every statement of the run the claim guards. It
//!   neither opens nor joins a transaction, so those statements still commit one by one.
//! - The claim fences the writes because it lives on their session. `PostgreSQL` ends a
//!   session's advisory locks with the session, whether it is closed, dropped, ended by
//!   `pg_terminate_backend` or `idle_session_timeout`, or cut off with its network. A claim on a
//!   session of its own would then be gone while the job went on writing through other
//!   connections, and a second run could claim, read and rewrite the same rows. On the claim's
//!   own session, which is never reconnected, the statement that follows the loss fails, so the
//!   job abends at that statement and writes nothing after the claim ended.
//! - [`PgSession::close`] unlocks every claim of the session and closes it; a session dropped
//!   unclosed, as when its job abends or its process ends, closes its connection, and the server
//!   ends the locks with the session. The connection never returns to the pool, so no other
//!   caller's statement runs on a session that holds a claim, where `PostgreSQL`, whose advisory
//!   locks are re-entrant within a session, would grant the claim a second time.
//! - Advisory locks are scoped to the database, as a cluster is to its catalog, so runs against
//!   other databases on the same server never meet the claim.
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
    TransactionCategoryRepository, TransactionRepository, TransactionRows,
    TransactionTypeRepository, UserRepository,
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

    /// Opens a [`PgSession`] on a connection taken from the pool and detached from it, for a run
    /// that claims data sets for writing and runs every statement on the session that holds
    /// its claims (see "Single-writer claims" above).
    ///
    /// The returned future owns a clone of the pool handle and borrows nothing from `self`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when no connection can be had: the pool timed out or is
    /// closed, or connecting failed.
    pub fn session(
        &self,
    ) -> impl Future<Output = Result<PgSession, PersistenceError>> + Send + use<> {
        let pool = self.pool.clone();
        async move {
            // Detached before any claim is tried, so that no outcome, a failed statement or a
            // future dropped mid-call included, hands a session that may hold a lock back to
            // the pool: dropping a detached connection closes it, and its session with it.
            let conn = pool.acquire().await?.detach();
            Ok(PgSession { conn })
        }
    }
}

// Hand-written: the derived form would print the pool's state and options.
impl fmt::Debug for PgStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgStore")
    }
}

/// One of the `SHAREOPTIONS(2 3)` clusters the batch jobs rewrite, which one run at a time
/// may claim for writing through [`PgSession::claim_writer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dataset {
    /// `transactions`, the cluster of DD `TRANFILE` (`TRANFILE.jcl:55`), which `posttran` opens
    /// for output (CBTRN02C-046).
    Transactions,
    /// `accounts`, the cluster of DD `ACCTFILE` (`ACCTFILE.jcl:42`), which `posttran` and
    /// `intcalc` open for update (CBTRN02C-046, CBACT04C-039).
    Accounts,
    /// `tran_cat_balances`, the cluster of DD `TCATBALF` (`TCATBALF.jcl:42`), which `posttran`
    /// opens for update (CBTRN02C-046).
    TranCatBalances,
}

impl Dataset {
    /// The key of the data set's advisory lock: the eight bytes of its DD name read as a
    /// big-endian 64-bit integer, a key no other value of this crate uses. In `pg_locks` the
    /// lock shows as `locktype = 'advisory'` with the key's high 32 bits as `classid` and its
    /// low 32 bits as `objid`:
    ///
    /// | Data set | DD name | Key | `classid` | `objid` |
    /// |---|---|---|---|---|
    /// | [`Transactions`](Self::Transactions) | `TRANFILE` | `0x5452_414E_4649_4C45` = 6075990651718683717 | 1414676814 | 1179208773 |
    /// | [`Accounts`](Self::Accounts) | `ACCTFILE` | `0x4143_4354_4649_4C45` = 4702676465140452421 | 1094927188 | 1179208773 |
    /// | [`TranCatBalances`](Self::TranCatBalances) | `TCATBALF` | `0x5443_4154_4241_4C46` = 6071768552770194502 | 1413693780 | 1111575622 |
    #[must_use]
    pub const fn lock_key(self) -> i64 {
        i64::from_be_bytes(*self.dd_name())
    }

    /// The DD name of the data set's cluster in `POSTTRAN.jcl` and `INTCALC.jcl`.
    const fn dd_name(self) -> &'static [u8; 8] {
        match self {
            Self::Transactions => b"TRANFILE",
            Self::Accounts => b"ACCTFILE",
            Self::TranCatBalances => b"TCATBALF",
        }
    }
}

/// One database session of a run's own, from [`PgStore::session`] until
/// [`close`](Self::close) or its drop: the holder of the run's single-writer claims and the
/// executor of every statement the run issues, through [`AccountRepository`],
/// [`CardXrefRepository`], [`DisclosureGroupRepository`], [`TranCatBalanceRepository`] and
/// [`TransactionRepository`].
///
/// It owns a connection detached from the pool, in autocommit mode: each statement commits as
/// it completes, as on [`PgStore`], and `replace_all` opens and commits a transaction of its own
/// on the connection. The connection is never replaced, so once the session is lost every later
/// statement fails, and a run cannot write after the server has ended its claims (see
/// "Single-writer claims" above). Dropping the session closes the connection, and the server
/// ends the claims with the session.
///
/// # Why a third executor
///
/// The shared persistence component names two executors, [`PgStore`] and [`PgTx`] (AAP 0.5.4),
/// and neither can carry a claim that fences the writes it guards. A claim must share the
/// session of those writes, and [`PgStore`] runs each statement on whichever pooled connection
/// is free, which may be a session other than the claim's, or a new one after the claim's was
/// lost. [`PgTx`] keeps one session, but it would make the run one unit of work, which
/// `posttran` and `intcalc` must not be: every completed write stands when they abend
/// (CBTRN02C-015, X-POSTTRAN-1, CBACT04C-026). This session keeps both properties: one
/// connection, and statements that commit one by one.
pub struct PgSession {
    /// The session: a connection detached from the pool, in autocommit mode.
    conn: PgConnection,
}

impl PgSession {
    /// Claims `dataset` for the writes of this session's run, as the `OPEN OUTPUT` or
    /// `OPEN I-O` of a `SHAREOPTIONS(2 3)` cluster admits one writer (see "Single-writer
    /// claims" above).
    ///
    /// Never waits: `Ok(false)` when another session holds the claim, as VSAM refuses a second
    /// writer's open at once (CBTRN02C-046, CBACT04C-039). A granted claim holds until
    /// [`close`](Self::close), the drop of the session or the loss of its connection. A claim
    /// this session already holds is granted again, advisory locks being re-entrant within a
    /// session, and [`close`](Self::close) gives every hold up at once.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost;
    /// [`PersistenceError::Database`] when the server refuses the statement. No claim is taken
    /// then.
    pub fn claim_writer(
        &mut self,
        dataset: Dataset,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send + '_ {
        let claim = sqlx::query_scalar!(
            r#"SELECT pg_try_advisory_lock($1) AS "claimed!""#,
            dataset.lock_key()
        )
        .fetch_one(&mut self.conn);
        async move { claim.await.map_err(PersistenceError::from) }
    }

    /// Ends the session: unlocks every claim it holds, so that a run that claims one of them
    /// after this call returns `Ok` is granted it, then closes the connection.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost before the unlock is
    /// answered or the close fails; [`PersistenceError::Database`] when the server refuses the
    /// unlock. The connection is closed or dropped in every case, and the server ends the
    /// session's claims with it, so a caller loses nothing by reporting the error and going on.
    pub fn close(self) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let Self { mut conn } = self;
        async move {
            // `pg_advisory_unlock_all` returns `void`, which has no Rust type; the one row its
            // call yields is selected as a constant instead.
            sqlx::query_scalar!(r#"SELECT true AS "unlocked!" FROM pg_advisory_unlock_all()"#)
                .fetch_one(&mut conn)
                .await?;
            sqlx::Connection::close(conn).await?;
            Ok(())
        }
    }

    /// Opens a transaction on the session's connection, for a `replace_all` whose delete and
    /// inserts must commit or roll back together, as [`PgStore::begin`] opens one on a pooled
    /// connection. The session's claims are not bound to it and outlast its end.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost;
    /// [`PersistenceError::Database`] when the server refuses to start the transaction.
    pub(crate) fn begin(
        &mut self,
    ) -> impl Future<Output = Result<PgTx<'_>, PersistenceError>> + Send {
        let begin = sqlx::Connection::begin(&mut self.conn);
        async move {
            let tx = begin.await?;
            Ok(PgTx { tx })
        }
    }

    /// The session's connection, as the executor of the repository implementations. It stays
    /// private, so no other crate can run a statement on the session.
    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        &mut self.conn
    }
}

// Hand-written: only the type name is printed, never the session it owns.
impl fmt::Debug for PgSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgSession")
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

// Every repository trait requires `Send`, and services and workers hold the executors across
// `.await` points of `Send` futures, as the batch jobs hold their sessions: a field that is not
// `Send` fails the build here.
const _: fn() = || {
    fn is_send<T: Send>() {}
    is_send::<PgStore>();
    is_send::<PgTx<'static>>();
    is_send::<PgSession>();
};

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use std::time::Duration;

    use copybook::{PicU, PicX};
    use domain::money::{Decimal, Money};
    use domain::records::TranCatBalanceRecord;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use sqlx::{Database, Postgres, TransactionManager};
    use test_support::postgres::start_postgres;
    use tokio::time::{Instant, sleep};

    use super::{
        AccountRepository, Dataset, PersistenceError, PgSession, PgStore, PgTx,
        TranCatBalanceRepository,
    };

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The three claimable data sets.
    const DATASETS: [Dataset; 3] = [
        Dataset::Transactions,
        Dataset::Accounts,
        Dataset::TranCatBalances,
    ];

    /// Bound on the wait for the server to end a session that was dropped or terminated.
    const DROP_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

    /// Pause between two claim attempts while that session ends.
    const DROP_RELEASE_POLL: Duration = Duration::from_millis(50);

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

    /// Category balance (`acct_id`, `01`, `0001`) of 1.00, in no fixture file.
    fn balance(acct_id: u64) -> Result<TranCatBalanceRecord, Box<dyn StdError>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(*b"01"),
            cd: PicU::new(1)?,
            bal: Money::new(Decimal::new(100, 2))?,
            filler: PicX::spaces(),
        })
    }

    /// A new session of `store` holding `dataset`'s claim, tried until the server has ended the
    /// session that held it before, within [`DROP_RELEASE_TIMEOUT`]. A refused try takes no
    /// lock, so the one session tries again.
    async fn claim_once_free(
        store: &PgStore,
        dataset: Dataset,
    ) -> Result<PgSession, Box<dyn StdError>> {
        let deadline = Instant::now() + DROP_RELEASE_TIMEOUT;
        let mut session = store.session().await?;
        while !session.claim_writer(dataset).await? {
            if Instant::now() >= deadline {
                return Err(format!("{dataset:?} stayed claimed after its session ended").into());
            }
            sleep(DROP_RELEASE_POLL).await;
        }
        Ok(session)
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

    /// Each data set's lock key is the documented big-endian reading of its DD name, and no two
    /// data sets share one.
    #[test]
    fn lock_keys_are_the_dd_names_and_distinct() {
        let cases = [
            (
                Dataset::Transactions,
                0x5452_414E_4649_4C45,
                6_075_990_651_718_683_717,
            ),
            (
                Dataset::Accounts,
                0x4143_4354_4649_4C45,
                4_702_676_465_140_452_421,
            ),
            (
                Dataset::TranCatBalances,
                0x5443_4154_4241_4C46,
                6_071_768_552_770_194_502,
            ),
        ];
        for (dataset, hex, decimal) in cases {
            assert_eq!(dataset.lock_key(), hex, "{dataset:?}");
            assert_eq!(dataset.lock_key(), decimal, "{dataset:?}");
        }
        for (index, dataset) in DATASETS.iter().enumerate() {
            for other in DATASETS.iter().skip(index.saturating_add(1)) {
                assert_ne!(
                    dataset.lock_key(),
                    other.lock_key(),
                    "{dataset:?} {other:?}"
                );
            }
        }
    }

    /// A session on a closed pool is the database unavailable, so no claim can be refused or
    /// granted on it.
    #[tokio::test]
    async fn a_session_on_a_closed_pool_is_unavailable() {
        let store = lazy_store();
        store.pool().close().await;
        assert_eq!(
            store.session().await.map(drop),
            Err(PersistenceError::Unavailable)
        );
    }

    /// One session at a time holds each data set's claim: another session's claim is refused at
    /// once while it is held, whatever the other data sets' claims, and granted again once the
    /// holder is closed or dropped. One session holds all three at once, as `posttran`'s does,
    /// and its close gives every one up. The pool has one connection, so a session that went
    /// back to the pool would be granted the claim a second time there, advisory locks being
    /// re-entrant within a session; and a store on a second pool, as a second process would
    /// open, is refused alike.
    // audit: CBTRN02C-046
    // audit: CBACT04C-039
    #[tokio::test]
    async fn a_claim_is_held_by_one_session_until_closed_or_dropped() -> TestResult {
        let guard = start_postgres().await?;
        let connect = || {
            PgPoolOptions::new()
                .max_connections(1)
                .connect(guard.database_url())
        };
        let store = PgStore::new(connect().await?);
        let other_process = PgStore::new(connect().await?);

        for dataset in DATASETS {
            let mut holder = store.session().await?;
            assert_eq!(format!("{holder:?}"), "PgSession");
            assert!(holder.claim_writer(dataset).await?, "a free {dataset:?}");
            for contender in [&store, &other_process] {
                let mut contender = contender.session().await?;
                assert!(
                    !contender.claim_writer(dataset).await?,
                    "{dataset:?} is claimed twice"
                );
                // The other data sets stay free: each is claimed beside this claim.
                for other in DATASETS.into_iter().filter(|other| *other != dataset) {
                    assert!(
                        contender.claim_writer(other).await?,
                        "{other:?} beside {dataset:?}"
                    );
                }
                contender.close().await?;
            }
            holder.close().await?;

            let mut again = other_process.session().await?;
            assert!(
                again.claim_writer(dataset).await?,
                "{dataset:?} is free as soon as its holder is closed"
            );
            let mut refused = store.session().await?;
            assert!(!refused.claim_writer(dataset).await?, "{dataset:?}");
            refused.close().await?;
            drop(again);
            claim_once_free(&store, dataset).await?.close().await?;
        }

        let mut all = store.session().await?;
        let mut contender = other_process.session().await?;
        for dataset in DATASETS {
            assert!(all.claim_writer(dataset).await?, "{dataset:?}");
            assert!(!contender.claim_writer(dataset).await?, "{dataset:?}");
        }
        all.close().await?;
        for dataset in DATASETS {
            assert!(
                contender.claim_writer(dataset).await?,
                "{dataset:?} once the session holding all three is closed"
            );
        }
        contender.close().await?;
        Ok(())
    }

    /// A session the server ends while it holds claims, as `pg_terminate_backend`,
    /// `idle_session_timeout` or a lost network does, is never replaced: every later statement on
    /// it fails, a write and its close included, and its claims go to another session once the
    /// server has ended it. A run whose session is lost therefore writes nothing after another
    /// run can claim its data sets.
    // audit: CBTRN02C-046
    // audit: CBACT04C-039
    #[tokio::test]
    async fn a_lost_session_fails_every_later_statement_and_frees_its_claims() -> TestResult {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let mut lost = store.session().await?;
        for dataset in DATASETS {
            assert!(lost.claim_writer(dataset).await?, "{dataset:?}");
        }
        let pid = sqlx::query_scalar!(r#"SELECT pg_backend_pid() AS "pid!""#)
            .fetch_one(lost.conn())
            .await?;
        let terminated =
            sqlx::query_scalar!(r#"SELECT pg_terminate_backend($1) AS "terminated!""#, pid)
                .fetch_one(store.pool())
                .await?;
        assert!(terminated, "the session's backend is told to end");

        assert!(
            AccountRepository::find_all(&mut lost).await.is_err(),
            "a read after the loss"
        );
        assert!(
            TranCatBalanceRepository::insert(&mut lost, &balance(1)?)
                .await
                .is_err(),
            "a write after the loss"
        );
        assert!(
            lost.claim_writer(Dataset::Accounts).await.is_err(),
            "a claim after the loss"
        );
        assert!(lost.close().await.is_err(), "the close after the loss");

        for dataset in DATASETS {
            claim_once_free(&store, dataset).await?.close().await?;
        }
        let mut reads = store.clone();
        assert!(
            TranCatBalanceRepository::find_all(&mut reads)
                .await?
                .is_empty(),
            "the write after the loss reached the table"
        );
        Ok(())
    }

    /// `replace_all` on a session runs in a transaction of its own on the session's connection,
    /// while every other statement there commits on its own: a replacement that fails leaves the
    /// table as the session's last committed write left it and the session usable with its claim
    /// held, and one that succeeds is committed when the call returns.
    #[tokio::test]
    async fn a_session_replaces_a_table_in_a_transaction_of_its_own() -> TestResult {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let mut reads = store.clone();
        let mut session = store.session().await?;
        assert!(session.claim_writer(Dataset::TranCatBalances).await?);

        let kept = balance(1)?;
        let replacement = balance(2)?;
        TranCatBalanceRepository::insert(&mut session, &kept).await?;
        assert_eq!(
            TranCatBalanceRepository::find_all(&mut reads).await?,
            [kept.clone()],
            "an insert on the session commits on its own"
        );
        let duplicate = TranCatBalanceRepository::replace_all(
            &mut session,
            &[replacement.clone(), replacement.clone()],
        )
        .await;
        assert!(
            matches!(duplicate, Err(PersistenceError::Duplicate { .. })),
            "{duplicate:?}"
        );
        assert_eq!(
            TranCatBalanceRepository::find_all(&mut reads).await?,
            [kept],
            "a failed replacement rolls back"
        );
        assert_eq!(
            TranCatBalanceRepository::replace_all(&mut session, &[replacement.clone()]).await?,
            1
        );
        assert_eq!(
            TranCatBalanceRepository::find_all(&mut reads).await?,
            [replacement]
        );
        let mut contender = store.session().await?;
        assert!(
            !contender.claim_writer(Dataset::TranCatBalances).await?,
            "the claim outlasts the session's transactions"
        );
        contender.close().await?;
        session.close().await?;
        Ok(())
    }
}
