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
//!   the pool, its options, the session or the database it reaches, and the `Debug` of a
//!   [`ProbeCall`] prints its point alone, never the arguments it carries.
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
//!
//! # Probes
//!
//! A [`RepositoryProbe`], installed with [`PgStore::with_probe`], is the endpoint tests' seam for
//! the audited outcomes a running `PostgreSQL` cannot produce on demand: a read that fails after
//! the reads before it succeeded, a rewrite or a delete that matches no row or fails after its
//! row lock was taken, and the values a write carried when its transaction then rolled back.
//!
//! - At each [`ProbePoint`] a probed [`PgStore`] or [`PgTx`] hands the probe a [`ProbeCall`],
//!   the point and the method's arguments, before it builds the statement, and follows the
//!   [`ProbeVerdict`]: run the statement, or skip it and either fail with a given error or
//!   answer as when no row has the key. A skipped statement sends no SQL.
//! - Production code never installs a probe: `carddemo-api`, the `carddemo-mq` workers and
//!   `carddemo-batch` build their store with [`PgStore::new`].
//! - A store without a probe behaves exactly as before: no extra statement, one `Option` check
//!   per probed call.
//! - [`PgStore::begin`] and [`PgTx::savepoint`] hand the probe on, so every unit of work answers
//!   to the probe of the store that opened it. A [`PgSession`] is never probed, nor is a
//!   transaction opened on one.

pub mod error;
pub mod repos;
pub mod rows;

use std::any::Any;
use std::fmt;
use std::sync::Arc;

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

/// The endpoint tests' seam into a [`PgStore`] and the transactions it opens (see "Probes"
/// above): asked at each [`ProbePoint`] of a call, before its statement is built, whether to run
/// it.
///
/// The store calls [`answer`](Self::answer) on the caller's task, once per probed call, so an
/// implementation must not block. It may keep what [`ProbeCall::arg`] shows it, such as the
/// record a rolled-back rewrite carried, for its test to assert on.
pub trait RepositoryProbe: Send + Sync {
    /// The verdict on `call`: [`ProbeVerdict::Proceed`] runs the statement as a store without a
    /// probe does; any other verdict skips it.
    fn answer(&self, call: &ProbeCall<'_>) -> ProbeVerdict;
}

/// A probed repository call. Each point covers the method it names on [`PgStore`] and on
/// [`PgTx`] alike, and lists the arguments [`ProbeCall::arg`] returns, in the method's parameter
/// order, `self` excluded, each as the type it is passed as or refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProbePoint {
    /// [`AccountRepository::find_by_id`], and the account read of
    /// [`AccountRepository::find_with_customer`], which is asked only when its cross-reference
    /// was found: `[PicU<11>]`, the account ID.
    AccountRead,
    /// [`AccountRepository::lock_by_id`]: `[PicU<11>]`, the account ID.
    AccountLock,
    /// [`AccountRepository::update`]: `[AccountRecord]`.
    AccountUpdate,
    /// [`CustomerRepository::find_by_id`], and the customer read of
    /// [`AccountRepository::find_with_customer`], which is asked only when its cross-reference
    /// was found: `[PicU<9>]`, the customer ID.
    CustomerRead,
    /// [`TransactionRepository::insert`]: `[TransactionRecord]`.
    TransactionInsert,
    /// [`UserRepository::update`]: `[UserEntry]`.
    UserUpdate,
    /// [`UserRepository::delete`]: `[PicX<8>]`, the user ID.
    UserDelete,
    /// [`PendingAuthSummaryRepository::find_by_id`]: `[PicS<11>]`, the account ID.
    AuthSummaryRead,
    /// [`PendingAuthDetailRepository::find_by_id`]: `[PicS<11>, PicS<5>, PicS<9>]`, the account
    /// ID and the inverted date and time.
    AuthDetailRead,
    /// [`PendingAuthDetailRepository::page_after`]: `[PicS<11>, PicS<5>, PicS<9>, i64]`, the
    /// account ID, the inverted start date and time and the limit.
    AuthDetailPage,
    /// [`PendingAuthDetailRepository::update`]: `[PicS<11>, PendingAuthDetailRecord]`, the
    /// account ID and the detail.
    AuthDetailUpdate,
    /// [`AuthFraudRepository::insert`]: `[AuthFraudRow, NaiveDate]`, the
    /// `domain::online::copaus2c` row and the report date.
    AuthFraudInsert,
    /// [`AuthFraudRepository::update_flag`]: `[CardNumber, NaiveDateTime, PicX<1>, NaiveDate]`,
    /// the card number, the authorization timestamp, the flag and the report date.
    AuthFraudUpdate,
}

/// One probed call as its [`RepositoryProbe`] sees it: the [`ProbePoint`] and the method's
/// arguments.
///
/// Hand-written `Debug` prints the point alone: an argument can be a card number or a record
/// holding one (Cardholder Data Redaction).
pub struct ProbeCall<'a> {
    /// The method called.
    point: ProbePoint,
    /// The method's arguments in parameter order, `self` excluded.
    args: &'a [&'a (dyn Any + Sync)],
}

impl ProbeCall<'_> {
    /// The method called.
    #[must_use]
    pub fn point(&self) -> ProbePoint {
        self.point
    }

    /// The `index`-th argument of the call, counted from 0 in the order its [`ProbePoint`] lists
    /// them; `None` when the call has no such argument or it is not a `T`.
    #[must_use]
    pub fn arg<T: Any>(&self, index: usize) -> Option<&T> {
        let arg: &dyn Any = *self.args.get(index)?;
        arg.downcast_ref()
    }
}

// Hand-written: only the point is printed, never the arguments.
impl fmt::Debug for ProbeCall<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeCall")
            .field("point", &self.point)
            .finish_non_exhaustive()
    }
}

/// A [`RepositoryProbe`]'s answer to a [`ProbeCall`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// Run the statement, as a store without a probe does.
    Proceed,
    /// Skip the statement and return this error, as when the statement failed with it. No SQL
    /// is sent, so a [`PgTx`] stays usable.
    Fail(PersistenceError),
    /// Skip the statement and answer as when no row has the key: `Ok(None)` for a read,
    /// `Ok(false)` for an update, a delete or a flag update, and an empty `Vec` for a page. An
    /// insert has no such answer and returns [`PersistenceError::NotFound`].
    NoRow,
}

/// The answer a probed method's result type gives for [`ProbeVerdict::NoRow`].
pub(crate) trait ProbeAnswer: Sized {
    /// The result of a call that finds no row with its key.
    fn no_row() -> Result<Self, PersistenceError>;
}

impl<T> ProbeAnswer for Option<T> {
    fn no_row() -> Result<Self, PersistenceError> {
        Ok(None)
    }
}

impl ProbeAnswer for bool {
    fn no_row() -> Result<Self, PersistenceError> {
        Ok(false)
    }
}

impl<T> ProbeAnswer for Vec<T> {
    fn no_row() -> Result<Self, PersistenceError> {
        Ok(Vec::new())
    }
}

/// An insert: no row to miss, so the not-found error.
impl ProbeAnswer for () {
    fn no_row() -> Result<Self, PersistenceError> {
        Err(PersistenceError::NotFound)
    }
}

/// `probe`'s verdict on a call at `point` with `args`, taken before the call's statement is
/// built; `None` when no probe is installed.
pub(crate) fn probe_verdict(
    probe: Option<&dyn RepositoryProbe>,
    point: ProbePoint,
    args: &[&(dyn Any + Sync)],
) -> Option<ProbeVerdict> {
    probe.map(|probe| probe.answer(&ProbeCall { point, args }))
}

/// The result of `statement`, unless `verdict` skips it (see [`ProbeVerdict`]): a skipped
/// statement future is dropped unpolled, so it sends no SQL.
pub(crate) async fn probed<T: ProbeAnswer>(
    verdict: Option<ProbeVerdict>,
    statement: impl Future<Output = Result<T, PersistenceError>>,
) -> Result<T, PersistenceError> {
    match verdict {
        None | Some(ProbeVerdict::Proceed) => statement.await,
        Some(ProbeVerdict::Fail(error)) => Err(error),
        Some(ProbeVerdict::NoRow) => T::no_row(),
    }
}

/// The connection pool, as the executor of every repository trait: each statement runs on a
/// pooled connection and commits on its own.
///
/// Cloning copies the pool handle, not the pool, so every clone shares the same connections,
/// and the probe handle, if any (see "Probes" above).
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    /// The probe asked at each [`ProbePoint`]; `None` outside the endpoint tests.
    probe: Option<Arc<dyn RepositoryProbe>>,
}

impl PgStore {
    /// A store over `pool`, as the `runtime` crate opens it from `DATABASE_URL`, with no probe.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool, probe: None }
    }

    /// This store with `probe` asked at each [`ProbePoint`] of its calls and of the
    /// transactions it opens, in place of any probe it had (see "Probes" above). Only the
    /// endpoint tests install one.
    #[must_use]
    pub fn with_probe(self, probe: Arc<dyn RepositoryProbe>) -> Self {
        Self {
            probe: Some(probe),
            ..self
        }
    }

    /// The probe's verdict on a call at `point` with `args`, for the repository
    /// implementations; `None` without a probe.
    pub(crate) fn verdict(
        &self,
        point: ProbePoint,
        args: &[&(dyn Any + Sync)],
    ) -> Option<ProbeVerdict> {
        probe_verdict(self.probe(), point, args)
    }

    /// The probe, for the repository implementations that consult it inside a statement
    /// sequence.
    pub(crate) fn probe(&self) -> Option<&dyn RepositoryProbe> {
        self.probe.as_deref()
    }

    /// The pool, for the repository implementations of this crate and for callers that manage
    /// the pool itself, such as the `test-support` harness. No other crate issues SQL through
    /// it (R4).
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Opens a transaction on a connection taken from the pool, for a unit of work whose
    /// statements and row locks must commit or roll back together. The transaction answers to
    /// this store's probe, if any.
    ///
    /// The returned future owns clones of the pool and probe handles and borrows nothing from
    /// `self`.
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
        let probe = self.probe.clone();
        async move {
            let tx = pool.begin().await?;
            Ok(PgTx { tx, probe })
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
/// A session is never probed, whatever the probe of the store that opened it (see "Probes"
/// above): it serves only the batch jobs, and a probe is a seam for the endpoints, which run on
/// [`PgStore`] and [`PgTx`].
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
    /// connection. The session's claims are not bound to it and outlast its end. Like the
    /// session, the transaction has no probe.
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
            Ok(PgTx { tx, probe: None })
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
    /// The probe of the store that opened the transaction (see "Probes" above); `None` on a
    /// [`PgSession`]'s.
    probe: Option<Arc<dyn RepositoryProbe>>,
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
    /// The savepoint answers to this transaction's probe, if any.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the connection is lost;
    /// [`PersistenceError::Database`] when the server refuses the savepoint.
    pub fn savepoint(&mut self) -> impl Future<Output = Result<PgTx<'_>, PersistenceError>> + Send {
        let probe = self.probe.clone();
        // On a connection already inside a transaction, `sqlx` begins a savepoint.
        let begin = sqlx::Connection::begin(&mut *self.tx);
        async move {
            let tx = begin.await?;
            Ok(PgTx { tx, probe })
        }
    }

    /// The transaction's connection, as the executor of the repository implementations. The
    /// transaction stays private, so no other crate can run a statement through it.
    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        &mut self.tx
    }

    /// The probe's verdict on a call at `point` with `args`, for the repository
    /// implementations; `None` without a probe.
    pub(crate) fn verdict(
        &self,
        point: ProbePoint,
        args: &[&(dyn Any + Sync)],
    ) -> Option<ProbeVerdict> {
        probe_verdict(self.probe.as_deref(), point, args)
    }

    /// The connection and the probe together, for the repository implementations that consult
    /// the probe inside a statement sequence on the connection.
    pub(crate) fn conn_and_probe(&mut self) -> (&mut PgConnection, Option<&dyn RepositoryProbe>) {
        (&mut self.tx, self.probe.as_deref())
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
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use chrono::NaiveDate;
    use copybook::framing::decode_all;
    use copybook::{FixedRecord, PicS, PicU, PicX};
    use domain::clock::FixedClock;
    use domain::money::{Decimal, Money};
    use domain::online::copaus2c::{AuthFraudRow, FraudAction, fraud_row};
    use domain::records::{
        AccountRecord, CardXrefRecord, CustomerRecord, PendingAuthDetailRecord,
        TranCatBalanceRecord, TransactionRecord, UserEntry,
    };
    use domain::sensitive::CardNumber;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use sqlx::{Database, Postgres, TransactionManager};
    use test_support::postgres::start_postgres;
    use tokio::time::{Instant, sleep};

    use super::{
        AccountCustomerChain, AccountRepository, AuthFraudRepository, CardXrefRepository,
        ChainLink, CustomerRepository, Dataset, PendingAuthDetailRepository,
        PendingAuthSummaryRepository, PersistenceError, PgSession, PgStore, PgTx, ProbeCall,
        ProbePoint, ProbeVerdict, RepositoryProbe, TranCatBalanceRepository, TransactionRepository,
        UserRepository,
    };

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// Every probe point.
    const POINTS: [ProbePoint; 13] = [
        ProbePoint::AccountRead,
        ProbePoint::AccountLock,
        ProbePoint::AccountUpdate,
        ProbePoint::CustomerRead,
        ProbePoint::TransactionInsert,
        ProbePoint::UserUpdate,
        ProbePoint::UserDelete,
        ProbePoint::AuthSummaryRead,
        ProbePoint::AuthDetailRead,
        ProbePoint::AuthDetailPage,
        ProbePoint::AuthDetailUpdate,
        ProbePoint::AuthFraudInsert,
        ProbePoint::AuthFraudUpdate,
    ];

    /// The fixture account whose one cross-reference joins it to its customer.
    const LINKED: u64 = 50;

    /// A card number in no fixture file: no rendering of a probe call may repeat it.
    const CARD: &str = "9876543210987654";

    /// PHC-shaped stand-in for an argon2id hash: the repository stores whatever it is given.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$cHJvYmVzYWx0$cHJvYmVmYWtlaGFzaA";

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

    /// The records of `app/data/ASCII/<file>`, read in place and decoded by codec A.
    fn fixture<R: FixedRecord>(file: &str) -> Result<Vec<R>, Box<dyn StdError>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file);
        let bytes = std::fs::read(path)?;
        Ok(decode_all::<R>(&bytes)?
            .into_iter()
            .map(|framed| framed.record)
            .collect())
    }

    /// The arguments the probe tests pass, one value for each argument of every probed method.
    struct Inputs {
        /// Fixture account [`LINKED`], and its key.
        account: AccountRecord,
        /// The customer the account's cross-reference names.
        cust_id: PicU<9>,
        /// `dailytran.txt` line 1, the twin layout of a `TRAN-RECORD`.
        transaction: TransactionRecord,
        /// A user no fixture holds.
        user: UserEntry,
        /// The account of the pending authorization.
        auth_acct: PicS<11>,
        /// A pending authorization detail under `auth_acct`.
        detail: PendingAuthDetailRecord,
        /// The page limit.
        limit: i64,
        /// The fraud row COPAUS2C builds for `detail`.
        fraud: AuthFraudRow,
        /// The fraud report date.
        report_date: NaiveDate,
    }

    impl Inputs {
        fn new() -> Result<Self, Box<dyn StdError>> {
            let linked = PicU::new(LINKED)?;
            let account = fixture::<AccountRecord>("acctdata.txt")?
                .into_iter()
                .find(|account| account.id == linked)
                .ok_or("fixture account 50")?;
            let cust_id = fixture::<CardXrefRecord>("cardxref.txt")?
                .into_iter()
                .find(|xref| xref.acct_id == linked)
                .ok_or("the cross-reference of fixture account 50")?
                .cust_id;
            let transaction = fixture::<TransactionRecord>("dailytran.txt")?
                .into_iter()
                .next()
                .ok_or("dailytran.txt line 1")?;
            let detail = PendingAuthDetailRecord {
                auth_date_9c: PicS::complement_from(25_001)?,
                auth_time_9c: PicS::complement_from(123_456_789)?,
                orig_date: PicX::from_text("250101")?,
                orig_time: PicX::from_text("123456")?,
                card_num: CardNumber::parse(CARD)?,
                auth_type: PicX::from_text("AUTH")?,
                card_expiry_date: PicX::from_text("1227")?,
                message_type: PicX::from_text("0100")?,
                message_source: PicX::from_text("POS")?,
                auth_id_code: PicX::from_text("123456")?,
                auth_resp_code: PicX::from_text("00")?,
                auth_resp_reason: PicX::from_text("0000")?,
                processing_code: PicU::new(3_000)?,
                transaction_amt: Money::new(Decimal::new(12_345, 2))?,
                approved_amt: Money::new(Decimal::new(12_345, 2))?,
                merchant_catagory_code: PicX::from_text("5411")?,
                acqr_country_code: PicX::from_text("840")?,
                pos_entry_mode: PicU::new(5)?,
                merchant_id: PicX::from_text("MERCH0000000001")?,
                merchant_name: PicX::from_text("CORNER GROCERY")?,
                merchant_city: PicX::from_text("SEATTLE")?,
                merchant_state: PicX::from_text("WA")?,
                merchant_zip: PicX::from_text("98101")?,
                transaction_id: PicX::from_text("TXN000000000001")?,
                match_status: PicX::from_text("P")?,
                auth_fraud: PicX::spaces(),
                fraud_rpt_date: PicX::spaces(),
                filler: PicX::spaces(),
            };
            let now = NaiveDate::from_ymd_opt(2025, 1, 2)
                .and_then(|day| day.and_hms_opt(9, 0, 0))
                .ok_or("clock time")?;
            let fraud = fraud_row(
                &detail,
                FraudAction::Report,
                linked,
                cust_id,
                &FixedClock::new(now),
            )?;
            Ok(Self {
                account,
                cust_id,
                transaction,
                user: UserEntry {
                    id: PicX::from_text("PROBE001")?,
                    fname: PicX::from_text("ADA")?,
                    lname: PicX::from_text("BYRON")?,
                    password_hash: HASH.to_owned(),
                    usr_type: PicX::from_text("U")?,
                    filler: PicX::spaces(),
                },
                auth_acct: PicS::new(50)?,
                detail,
                limit: 7,
                fraud,
                report_date: now.date(),
            })
        }

        /// Whether `call` carries these inputs as the arguments its point lists, in order.
        fn passed_to(&self, call: &ProbeCall<'_>) -> bool {
            let (detail, fraud) = (&self.detail, &self.fraud);
            let auth_key = || {
                call.arg(0) == Some(&self.auth_acct)
                    && call.arg(1) == Some(&detail.auth_date_9c)
                    && call.arg(2) == Some(&detail.auth_time_9c)
            };
            match call.point() {
                ProbePoint::AccountRead | ProbePoint::AccountLock => {
                    call.arg(0) == Some(&self.account.id)
                }
                ProbePoint::AccountUpdate => call.arg(0) == Some(&self.account),
                ProbePoint::CustomerRead => call.arg(0) == Some(&self.cust_id),
                ProbePoint::TransactionInsert => call.arg(0) == Some(&self.transaction),
                ProbePoint::UserUpdate => call.arg(0) == Some(&self.user),
                ProbePoint::UserDelete => call.arg(0) == Some(&self.user.id),
                ProbePoint::AuthSummaryRead => call.arg(0) == Some(&self.auth_acct),
                ProbePoint::AuthDetailRead => auth_key(),
                ProbePoint::AuthDetailPage => auth_key() && call.arg(3) == Some(&self.limit),
                ProbePoint::AuthDetailUpdate => {
                    call.arg(0) == Some(&self.auth_acct) && call.arg(1) == Some(detail)
                }
                ProbePoint::AuthFraudInsert => {
                    call.arg(0) == Some(fraud) && call.arg(1) == Some(&self.report_date)
                }
                ProbePoint::AuthFraudUpdate => {
                    call.arg(0) == Some(&fraud.card_num)
                        && call.arg(1) == Some(&fraud.auth_ts)
                        && call.arg(2) == Some(&fraud.auth_fraud)
                        && call.arg(3) == Some(&self.report_date)
                }
            }
        }
    }

    /// A probe that answers its scripted verdict at its scripted point and lets every other call
    /// proceed, recording each call it is asked about with whether it carried [`Inputs`].
    struct Scripted {
        inputs: Inputs,
        script: Mutex<Option<(ProbePoint, ProbeVerdict)>>,
        seen: Mutex<Vec<(ProbePoint, bool)>>,
    }

    impl Scripted {
        fn new() -> Result<Arc<Self>, Box<dyn StdError>> {
            Ok(Arc::new(Self {
                inputs: Inputs::new()?,
                script: Mutex::new(None),
                seen: Mutex::new(Vec::new()),
            }))
        }

        /// Answers `verdict` at `point` from now on.
        fn script(&self, point: ProbePoint, verdict: ProbeVerdict) -> TestResult {
            *self.script.lock().map_err(|_| "script lock poisoned")? = Some((point, verdict));
            Ok(())
        }

        /// The calls asked about since the last look.
        fn seen(&self) -> Result<Vec<(ProbePoint, bool)>, Box<dyn StdError>> {
            Ok(std::mem::take(
                &mut *self.seen.lock().map_err(|_| "seen lock poisoned")?,
            ))
        }
    }

    impl RepositoryProbe for Scripted {
        fn answer(&self, call: &ProbeCall<'_>) -> ProbeVerdict {
            if let Ok(mut seen) = self.seen.lock() {
                seen.push((call.point(), self.inputs.passed_to(call)));
            }
            self.script
                .lock()
                .ok()
                .and_then(|script| script.clone())
                .filter(|(point, _)| *point == call.point())
                .map_or(ProbeVerdict::Proceed, |(_, verdict)| verdict)
        }
    }

    /// A probed call's answer with its rows dropped: a row read, changed or written, or none.
    #[derive(Debug, PartialEq, Eq)]
    enum Shape {
        Row,
        NoRow,
    }

    impl From<bool> for Shape {
        fn from(row: bool) -> Self {
            if row { Self::Row } else { Self::NoRow }
        }
    }

    /// The call at `point` on `executor`, with the arguments of `inputs`.
    async fn call_at<E>(
        executor: &mut E,
        point: ProbePoint,
        inputs: &Inputs,
    ) -> Result<Shape, PersistenceError>
    where
        E: AccountRepository
            + AuthFraudRepository
            + CustomerRepository
            + PendingAuthDetailRepository
            + PendingAuthSummaryRepository
            + TransactionRepository
            + UserRepository,
    {
        let Inputs {
            account,
            cust_id,
            transaction,
            user,
            auth_acct,
            detail,
            limit,
            fraud,
            report_date,
        } = inputs;
        let (acct, date, time) = (*auth_acct, detail.auth_date_9c, detail.auth_time_9c);
        Ok(match point {
            ProbePoint::AccountRead => AccountRepository::find_by_id(executor, account.id)
                .await?
                .is_some()
                .into(),
            ProbePoint::AccountLock => AccountRepository::lock_by_id(executor, account.id)
                .await?
                .is_some()
                .into(),
            ProbePoint::AccountUpdate => AccountRepository::update(executor, account).await?.into(),
            ProbePoint::CustomerRead => CustomerRepository::find_by_id(executor, *cust_id)
                .await?
                .is_some()
                .into(),
            ProbePoint::TransactionInsert => {
                TransactionRepository::insert(executor, transaction).await?;
                Shape::Row
            }
            ProbePoint::UserUpdate => UserRepository::update(executor, user).await?.into(),
            ProbePoint::UserDelete => UserRepository::delete(executor, &user.id).await?.into(),
            ProbePoint::AuthSummaryRead => PendingAuthSummaryRepository::find_by_id(executor, acct)
                .await?
                .is_some()
                .into(),
            ProbePoint::AuthDetailRead => {
                PendingAuthDetailRepository::find_by_id(executor, acct, date, time)
                    .await?
                    .is_some()
                    .into()
            }
            ProbePoint::AuthDetailPage => Shape::from(
                !PendingAuthDetailRepository::page_after(executor, acct, date, time, *limit)
                    .await?
                    .is_empty(),
            ),
            ProbePoint::AuthDetailUpdate => {
                PendingAuthDetailRepository::update(executor, acct, detail)
                    .await?
                    .into()
            }
            ProbePoint::AuthFraudInsert => {
                AuthFraudRepository::insert(executor, fraud, *report_date).await?;
                Shape::Row
            }
            ProbePoint::AuthFraudUpdate => AuthFraudRepository::update_flag(
                executor,
                &fraud.card_num,
                fraud.auth_ts,
                &fraud.auth_fraud,
                *report_date,
            )
            .await?
            .into(),
        })
    }

    /// The answer at `point` under [`ProbeVerdict::NoRow`]: none, or the not-found error of an
    /// insert.
    fn no_row(point: ProbePoint) -> Result<Shape, PersistenceError> {
        match point {
            ProbePoint::TransactionInsert | ProbePoint::AuthFraudInsert => {
                Err(PersistenceError::NotFound)
            }
            _ => Ok(Shape::NoRow),
        }
    }

    /// The failure the probe injects, SQLSTATE `58030` (`io_error`), which neither a closed pool
    /// nor any statement of these tests raises.
    fn injected() -> PersistenceError {
        PersistenceError::Database {
            sqlstate: Some("58030".to_owned()),
            constraint: None,
        }
    }

    /// A call shows each argument at its index as its own type, nothing for another type or an
    /// index past the last, and its `Debug` names the point alone (Cardholder Data Redaction).
    #[test]
    fn a_call_shows_its_arguments_by_index_and_type_and_prints_its_point_alone() -> TestResult {
        let card = CardNumber::parse(CARD)?;
        let id = PicU::<11>::new(LINKED)?;
        let call = ProbeCall {
            point: ProbePoint::AuthFraudUpdate,
            args: &[&card, &id],
        };
        assert_eq!(call.point(), ProbePoint::AuthFraudUpdate);
        assert!(call.arg::<CardNumber>(0) == Some(&card));
        assert_eq!(call.arg::<PicU<11>>(1), Some(&id));
        assert_eq!(
            call.arg::<PicU<9>>(1),
            None,
            "another width is another type"
        );
        assert_eq!(
            call.arg::<&PicU<11>>(1),
            None,
            "arguments are their values' types"
        );
        assert_eq!(call.arg::<PicU<11>>(2), None, "past the last argument");
        let text = format!("{call:?}");
        assert_eq!(text, "ProbeCall { point: AuthFraudUpdate, .. }");
        assert!(!text.contains(CARD) && !text.contains("7654"), "{text}");
        Ok(())
    }

    /// On a store whose pool is closed, so that any statement it ran would fail as unavailable,
    /// every point answers `NoRow` and `Fail` without running one, and `Proceed` runs it. Each
    /// call asks the probe once, at its own point, with its own arguments.
    #[test]
    fn every_point_skips_its_statement_unless_the_probe_lets_it_proceed() -> TestResult {
        block_on(async {
            let probe = Scripted::new()?;
            let mut store = lazy_store().with_probe(probe.clone());
            store.pool().close().await;
            for point in POINTS {
                for (verdict, expected) in [
                    (ProbeVerdict::NoRow, no_row(point)),
                    (ProbeVerdict::Fail(injected()), Err(injected())),
                    (ProbeVerdict::Proceed, Err(PersistenceError::Unavailable)),
                ] {
                    probe.script(point, verdict.clone())?;
                    let answer = call_at(&mut store, point, &probe.inputs).await;
                    assert_eq!(answer, expected, "{point:?} {verdict:?}");
                    assert_eq!(probe.seen()?, [(point, true)], "{point:?} {verdict:?}");
                }
            }
            Ok(())
        })
    }

    /// A transaction answers to the probe of the store that opened it, and a savepoint to the
    /// probe of its transaction: each point skips its statement on `NoRow` and `Fail`, leaving
    /// the transaction usable, and on `Proceed` answers as the same call in a transaction of a
    /// store without a probe, which never asks it.
    #[test]
    fn transactions_and_savepoints_answer_to_the_probe_of_their_store() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let probe = Scripted::new()?;
            let plain = PgStore::new(guard.pool().clone());
            let watched = plain.clone().with_probe(probe.clone());
            let mut tx = watched.begin().await?;
            let mut plain_tx = plain.begin().await?;
            for point in POINTS {
                for (verdict, expected) in [
                    (ProbeVerdict::NoRow, no_row(point)),
                    (ProbeVerdict::Fail(injected()), Err(injected())),
                ] {
                    probe.script(point, verdict.clone())?;
                    let answer = call_at(&mut tx, point, &probe.inputs).await;
                    assert_eq!(answer, expected, "{point:?} {verdict:?} in the transaction");
                    let mut savepoint = tx.savepoint().await?;
                    let answer = call_at(&mut savepoint, point, &probe.inputs).await;
                    assert_eq!(answer, expected, "{point:?} {verdict:?} in a savepoint");
                    savepoint.commit().await?;
                    assert_eq!(probe.seen()?, [(point, true), (point, true)], "{point:?}");
                }
                probe.script(point, ProbeVerdict::Proceed)?;
                let mut savepoint = tx.savepoint().await?;
                let answer = call_at(&mut savepoint, point, &probe.inputs).await;
                savepoint.rollback().await?;
                let mut plain_savepoint = plain_tx.savepoint().await?;
                let unprobed = call_at(&mut plain_savepoint, point, &probe.inputs).await;
                plain_savepoint.rollback().await?;
                assert_eq!(answer, unprobed, "{point:?} proceeds");
                assert_eq!(probe.seen()?, [(point, true)], "{point:?} proceeds");
            }
            tx.commit().await?;
            plain_tx.commit().await?;
            Ok(())
        })
    }

    /// On a live table: `Fail` and `NoRow` at a user's rewrite or delete leave the stored row as
    /// it was, `Proceed` runs the rewrite, and a store without a probe, over the same pool, is
    /// never asked and deletes the row the probe would have kept.
    #[test]
    fn a_skipped_write_changes_nothing_and_a_store_without_a_probe_is_unaffected() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let probe = Scripted::new()?;
            let rewrite = &probe.inputs.user;
            let stored = UserEntry {
                lname: PicX::from_text("LOVELACE")?,
                ..rewrite.clone()
            };
            let mut plain = PgStore::new(guard.pool().clone());
            let mut watched = plain.clone().with_probe(probe.clone());
            UserRepository::insert(&mut plain, &stored).await?;

            for (point, verdict, expected) in [
                (ProbePoint::UserUpdate, ProbeVerdict::NoRow, Ok(false)),
                (
                    ProbePoint::UserUpdate,
                    ProbeVerdict::Fail(injected()),
                    Err(injected()),
                ),
                (ProbePoint::UserDelete, ProbeVerdict::NoRow, Ok(false)),
                (
                    ProbePoint::UserDelete,
                    ProbeVerdict::Fail(injected()),
                    Err(injected()),
                ),
            ] {
                probe.script(point, verdict.clone())?;
                let answer = if point == ProbePoint::UserUpdate {
                    UserRepository::update(&mut watched, rewrite).await
                } else {
                    UserRepository::delete(&mut watched, &rewrite.id).await
                };
                assert_eq!(answer, expected, "{point:?} {verdict:?}");
                assert_eq!(probe.seen()?, [(point, true)], "{point:?} {verdict:?}");
                assert!(
                    UserRepository::find_by_id(&mut plain, &rewrite.id).await?
                        == Some(stored.clone()),
                    "{point:?} {verdict:?} wrote nothing"
                );
            }

            probe.script(ProbePoint::UserUpdate, ProbeVerdict::Proceed)?;
            assert_eq!(
                UserRepository::update(&mut watched, rewrite).await,
                Ok(true)
            );
            assert_eq!(probe.seen()?, [(ProbePoint::UserUpdate, true)]);
            assert!(
                UserRepository::find_by_id(&mut plain, &rewrite.id).await? == Some(rewrite.clone()),
                "the rewrite proceeded"
            );

            probe.script(ProbePoint::UserDelete, ProbeVerdict::NoRow)?;
            assert_eq!(
                UserRepository::delete(&mut plain, &rewrite.id).await,
                Ok(true)
            );
            assert!(
                probe.seen()?.is_empty(),
                "a store without a probe never asks one"
            );
            assert!(
                UserRepository::find_by_id(&mut plain, &rewrite.id)
                    .await?
                    .is_none()
            );
            Ok(())
        })
    }

    /// The account chain asks the probe before its account read and before its customer read,
    /// on a store and in a transaction alike: a skipped account read becomes its link and the
    /// customer is still read, a skipped customer read becomes its link, and a chain the probe
    /// lets proceed equals the chain of a store without one.
    #[test]
    fn the_chain_asks_the_probe_before_its_account_and_customer_reads() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let probe = Scripted::new()?;
            let acct_id = probe.inputs.account.id;
            let mut plain = PgStore::new(guard.pool().clone());
            CardXrefRepository::replace_all(
                &mut plain,
                &fixture::<CardXrefRecord>("cardxref.txt")?,
            )
            .await?;
            CustomerRepository::replace_all(
                &mut plain,
                &fixture::<CustomerRecord>("custdata.txt")?,
            )
            .await?;
            AccountRepository::replace_all(&mut plain, &fixture::<AccountRecord>("acctdata.txt")?)
                .await?;
            let found = AccountRepository::find_with_customer(&mut plain, acct_id).await;
            assert!(
                matches!(
                    (&found.xref, &found.account, &found.customer),
                    (
                        ChainLink::Found(_),
                        ChainLink::Found(_),
                        ChainLink::Found(_)
                    )
                ),
                "fixture account 50 reaches its cross-reference, account and customer"
            );
            let both_reads = [
                (ProbePoint::AccountRead, true),
                (ProbePoint::CustomerRead, true),
            ];
            let mut watched = plain.clone().with_probe(probe.clone());

            for (point, verdict, account, customer) in [
                (
                    ProbePoint::AccountRead,
                    ProbeVerdict::Fail(injected()),
                    ChainLink::Failed(injected()),
                    found.customer.clone(),
                ),
                (
                    ProbePoint::AccountRead,
                    ProbeVerdict::NoRow,
                    ChainLink::NotFound,
                    found.customer.clone(),
                ),
                (
                    ProbePoint::CustomerRead,
                    ProbeVerdict::Fail(injected()),
                    found.account.clone(),
                    ChainLink::Failed(injected()),
                ),
                (
                    ProbePoint::CustomerRead,
                    ProbeVerdict::NoRow,
                    found.account.clone(),
                    ChainLink::NotFound,
                ),
                (
                    ProbePoint::AccountRead,
                    ProbeVerdict::Proceed,
                    found.account.clone(),
                    found.customer.clone(),
                ),
            ] {
                probe.script(point, verdict.clone())?;
                let expected = AccountCustomerChain {
                    xref: found.xref.clone(),
                    account,
                    customer,
                };
                let on_store = AccountRepository::find_with_customer(&mut watched, acct_id).await;
                assert!(on_store == expected, "{point:?} {verdict:?} on the store");
                assert_eq!(
                    probe.seen()?,
                    both_reads,
                    "{point:?} {verdict:?} on the store"
                );
                let mut tx = watched.begin().await?;
                let in_tx = AccountRepository::find_with_customer(&mut tx, acct_id).await;
                assert!(in_tx == expected, "{point:?} {verdict:?} in a transaction");
                assert_eq!(
                    probe.seen()?,
                    both_reads,
                    "{point:?} {verdict:?} in a transaction"
                );
                tx.rollback().await?;
            }
            Ok(())
        })
    }
}
