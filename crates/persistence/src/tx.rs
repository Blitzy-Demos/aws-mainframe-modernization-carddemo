//! The unit of work: the database transaction an adapter opens, hands to the
//! rules and commits; [`PgStore`], the single port adapter bound to it; and
//! the savepoint around each statement of a store.
//!
//! No other module begins, commits or rolls back a transaction or a
//! savepoint. `carddemo-api`, `carddemo-batch` and `carddemo-messaging` open a
//! [`UnitOfWork`] with `Db::begin`, pass [`UnitOfWork::store`] to the rules and
//! finish with [`UnitOfWork::commit`]. A reader whose statements must all see
//! one state, the CR00 report and the TRANREPT backup, opens it with
//! `Db::begin_snapshot` instead and finishes with [`UnitOfWork::rollback`].
//! The port implementations in [`crate::repos`] and the reward store in
//! [`crate::rewards`] wrap each statement in the crate-internal `savepoint`
//! and `settle`.
//!
//! # Write persistence
//!
//! Every `CardDemo` VSAM file is defined `RECOVERY(NONE)`: `ACCTDAT`,
//! `CARDAIX`, `CARDDAT`, `CCXREF`, `CUSTDAT`, `CXACAIX`, `TRANSACT` and
//! `USRSEC` [app/csd/CARDDEMO.CSD:L9] [app/csd/CARDDEMO.CSD:L21]
//! [app/csd/CARDDEMO.CSD:L33] [app/csd/CARDDEMO.CSD:L46]
//! [app/csd/CARDDEMO.CSD:L59] [app/csd/CARDDEMO.CSD:L72]
//! [app/csd/CARDDEMO.CSD:L84] [app/csd/CARDDEMO.CSD:L96]. A completed write
//! therefore survives a later failure in the same task, and `SYNCPOINT
//! ROLLBACK` does not undo it:
//!
//! - `COACTUPC` rewrites the account, then the customer, and issues `SYNCPOINT
//!   ROLLBACK` when the customer rewrite fails
//!   [app/cbl/COACTUPC.cbl:L4060-4102]. The account rewrite stays in place
//!   (P-COACTUPC-014).
//! - `COBIL00C` subtracts the payment and rewrites the account whatever the
//!   outcome of the transaction write before it [app/cbl/COBIL00C.cbl:L233-235]
//!   (P-COBIL00C-018, E-19).
//!
//! The replacement keeps those outcomes inside one `PostgreSQL` transaction,
//! which fails in two ways:
//!
//! - A client-side outcome leaves the transaction usable: a key that names no
//!   row comes back as `RowNotFound` or `None` and maps to `NotFound`, and a
//!   stored value that does not fit its domain type is
//!   [`PersistenceError::Corrupt`] once the row is read.
//! - Any server-side statement error, whatever its SQLSTATE (`42P01`,
//!   `57014`, `55P03`, `40001`, ...) and whether the statement reads or
//!   writes, aborts the whole transaction: every later statement fails with
//!   `25P02`, and `COMMIT` ends as `ROLLBACK`. A failed read would therefore
//!   undo every write the unit made before it, such as the INTCALC account
//!   rewrite before the next account's reads (P-CBACT04C-010) or the CBPAUP0J
//!   deletes before the later summary and detail reads (P-CBPAUP0C-005,
//!   P-CBPAUP0C-009), or turn the next legacy read into a false error, such as
//!   the `CUSTDAT` read after a failed `ACCTDAT` read of `COACTVWC` and
//!   `COACTUPC` (V-COACTVWC-006, V-COACTUPC-007).
//!
//! Hence:
//!
//! - Every statement a store method issues, legacy write, locking read
//!   (`SELECT ... FOR UPDATE`, the CICS `READ UPDATE`) or read alike, runs
//!   under its own savepoint, `carddemo_statement`. Failure rolls back to it
//!   (`ROLLBACK TO SAVEPOINT`), which undoes that statement alone and clears
//!   the aborted state, and returns the mapped [`PortError`]. Success keeps
//!   the statement: the next statement of the unit of work releases the
//!   savepoint (`RELEASE SAVEPOINT`) before it opens its own, and `COMMIT`
//!   or `ROLLBACK` ends the last one.
//! - A failed statement therefore never leaves the outer transaction aborted.
//!   The writes before it survive, and the statements after it still run, as
//!   the COBOL carries on after a `RESP` other than `NORMAL`.
//! - The adapter commits what the "Persists on later failure" column of
//!   section B of `docs/parity/parity-oracle-audit.md` says persists, and
//!   returns the COBOL message with its mapped status. It never rolls back a
//!   whole request by default.
//! - The one exception is the `TRAN-ID` allocation in [`crate::ids`]: the
//!   advisory lock and highest-key read of [`crate::ids::next_tran_id`], and
//!   the account-reference lock CT02 takes before it
//!   ([`crate::ids::lock_account_reference`]), run on the transaction itself,
//!   because their callers, CT02 and CB00, run them before their first write
//!   and roll back the whole unit when one fails, so no earlier write has to
//!   survive it.
//!
//! # Abandoned calls
//!
//! A caller may drop the future of a store call at any await point and go on
//! using the unit of work, as `carddemo-messaging` does with a call that
//! outlives its deadline. The unit of work records what its last call left on
//! the connection, and its next use, a store call,
//! [`UnitOfWork::next_tran_id`], [`UnitOfWork::lock_account_reference`],
//! [`UnitOfWork::commit`] or [`UnitOfWork::rollback`], first reads every
//! response the abandoned call still owes, whatever the server did with it,
//! and then:
//!
//! - rolls back to the savepoint of a statement abandoned before it returned,
//!   so that statement is undone;
//! - sends again a `ROLLBACK TO SAVEPOINT` that was abandoned;
//! - goes on after an abandoned `SAVEPOINT` or `RELEASE SAVEPOINT`, which
//!   leaves at most a savepoint open that is empty or holds statements already
//!   reported done, and `COMMIT` keeps those.
//!
//! A statement whose store call settled it as done is therefore kept, one that
//! failed or was abandoned before it was settled is undone, the statements
//! before it stay, and the same transaction commits. For the store calls that
//! run one statement, every call but `PgStore::exclude_master_writers`, a call
//! that returned `Ok` is kept and one that failed or was abandoned is undone.
//! A `RELEASE SAVEPOINT` is sent only for a savepoint no
//! rollback is pending for, and a `ROLLBACK TO SAVEPOINT` only for one not yet
//! released, so no path rolls back the outer transaction or a released
//! savepoint.
//!
//! When a `SAVEPOINT`, `RELEASE SAVEPOINT` or `ROLLBACK TO SAVEPOINT` fails, a
//! statement run outside a savepoint fails on the server, or the connection
//! fails, the transaction is aborted or its state unknown. Every later store
//! call then fails with `PortError::Unavailable` without reaching the server,
//! and [`UnitOfWork::commit`] rolls back and fails rather than send a `COMMIT`
//! the server would end as a `ROLLBACK` reported as success.
//! [`UnitOfWork::rollback`] still rolls back.
//!
//! The record belongs to the unit of work and is shared by
//! [`UnitOfWork::store`], `UnitOfWork::rewards` and `PgStore::reward_store`. A
//! store bound by [`PgStore::new`] or `PgRewardStore::new` to a transaction the
//! caller holds keeps a record of its own, which another store on the same
//! transaction does not see: a caller that may abandon calls makes them all
//! through one store.
//!
//! # Whole-transaction rollback
//!
//! [`UnitOfWork::rollback`] undoes everything, savepoints included. Adapters
//! call it only where a rollback of the whole unit is mandated:
//!
//! - CT02: the transaction insert, its reward accrual and the balance update
//!   commit or roll back together, and an accrual failure answers 500 with
//!   nothing persisted.
//! - POSTTRAN: one unit of work per posted record; a reward-engine failure
//!   rolls back that record before the run halts.
//! - Db2 and IMS paths whose COBOL issues `SYNCPOINT ROLLBACK`: unlike a
//!   `RECOVERY(NONE)` file, Db2 tables and IMS databases are backed out by
//!   it. The CPVD fraud marking is one: when the IMS detail replace fails,
//!   `COPAUS1C` `ROLL-BACK`
//!   [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L565-568] backs out
//!   the `AUTHFRDS` write `COPAUS2C` made (P-COPAUS1C-007, P-COPAUS2C-003).
//!
//! Dropping a [`UnitOfWork`] that was neither committed nor rolled back rolls
//! it back too, savepoints and abandoned calls included: sqlx queues the
//! `ROLLBACK` on the connection, and it runs before the connection is used
//! again or returned to the pool.
//!
//! # Tracing
//!
//! Spans belong to the adapters, which record the outcome of each request,
//! job record or message. Besides its `debug` events for a commit and a
//! rollback, this module logs every failure `savepoint` and `settle` map, once,
//! through `crate::port_error_at`: one `database statement failed` event with
//! the SQLSTATE, the outcome, the sanitized error text, the table and
//! constraint names, and the code location of the store method that ran the
//! statement (`code.filepath`, `code.lineno`). `NotFound` is not logged. A
//! call refused on an unusable transaction is one `warn` event,
//! `database statement refused`, at the same location; the error an abandoned
//! statement left, read before the next use, is a `debug` event.

use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::panic::Location;

use carddemo_domain::numeric::{AccountId, TranId};
use carddemo_domain::ports::{PortError, PortResult};
use sqlx::{PgConnection, Postgres, Transaction};

use crate::{PersistenceError, port_error_at};

/// One `PostgreSQL` transaction an adapter opens, works in and finishes.
///
/// Opened by `Db::begin`, or read-only on one snapshot by
/// `Db::begin_snapshot`; finished by [`UnitOfWork::commit`] or
/// [`UnitOfWork::rollback`]. Dropping it unfinished rolls it back. The reward
/// store bound to the same transaction comes from `UnitOfWork::rewards`,
/// declared beside [`crate::rewards::PgRewardStore`].
#[derive(Debug)]
pub struct UnitOfWork {
    tx: Transaction<'static, Postgres>,
    /// What the last call left of the statement savepoint, shared by every
    /// store bound to this unit of work.
    statement: Settlement,
}

impl UnitOfWork {
    /// Wraps a transaction just begun on a pooled connection.
    #[must_use]
    pub(crate) fn new(tx: Transaction<'static, Postgres>) -> Self {
        Self {
            tx,
            statement: Settlement::Clean,
        }
    }

    /// The transaction itself, for a statement this crate runs on it before
    /// any store is bound, such as the `SET TRANSACTION` of
    /// `Db::begin_snapshot`.
    pub(crate) fn transaction(&mut self) -> &mut Transaction<'static, Postgres> {
        &mut self.tx
    }

    /// The transaction and this unit of work's statement record, for the
    /// stores this crate binds to it outside this module.
    pub(crate) fn bound(&mut self) -> (&mut Transaction<'static, Postgres>, Ledger<'_>) {
        (&mut self.tx, Ledger::Shared(&mut self.statement))
    }

    /// The port adapter bound to this transaction.
    ///
    /// The store borrows the unit of work, so every call made through it
    /// belongs to this transaction and none outlives it. It shares the unit
    /// of work's record of the statement savepoint, so a call abandoned
    /// through one store is settled by the next call through any other.
    #[must_use]
    pub fn store(&mut self) -> PgStore<'_, 'static> {
        let (tx, ledger) = self.bound();
        PgStore { tx, ledger }
    }

    /// Allocates the next TRAN-ID inside this transaction, through
    /// [`crate::ids::next_tran_id`].
    ///
    /// The advisory lock the allocation takes is held until this unit of work
    /// commits or rolls back, so a concurrent CT02 or CB00 add waits for it
    /// and then reads the highest key as this one left it.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Port`] with `PortError::NotFound` when the live
    ///   transaction master is empty, the `STARTBR` `NOTFND` of `COTRN02C` and
    ///   `COBIL00C`.
    /// - [`PersistenceError::Port`] with `PortError::Unavailable` when the
    ///   unit of work is unusable, or what an abandoned store call left cannot
    ///   be settled first.
    /// - [`PersistenceError::Sqlx`] when the lock or the read fails; the unit
    ///   of work is unusable afterwards, since the failure aborted it.
    /// - [`PersistenceError::Corrupt`] when the highest stored key is not 16
    ///   digits.
    pub async fn next_tran_id(&mut self) -> Result<TranId, PersistenceError> {
        self.begin_unguarded().await?;
        let allocated = crate::ids::next_tran_id(&mut self.tx).await;
        self.end_unguarded(&allocated);
        allocated
    }

    /// Takes `FOR KEY SHARE` on the `accounts` row of `account_id` inside this
    /// transaction, through [`crate::ids::lock_account_reference`].
    ///
    /// CT02 calls it before [`UnitOfWork::next_tran_id`], so the lock its
    /// reward accrual needs on the account is held before the advisory lock,
    /// the order [`crate::ids`] documents. The lock is held until this unit of
    /// work commits or rolls back; an account without a row locks nothing.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Port`] with `PortError::Unavailable` when the
    ///   unit of work is unusable, or what an abandoned store call left cannot
    ///   be settled first.
    /// - [`PersistenceError::Sqlx`] when the statement fails; the unit of work
    ///   is unusable afterwards, since the failure aborted it.
    pub async fn lock_account_reference(
        &mut self,
        account_id: AccountId,
    ) -> Result<(), PersistenceError> {
        self.begin_unguarded().await?;
        let locked = crate::ids::lock_account_reference(&mut self.tx, account_id).await;
        self.end_unguarded(&locked);
        locked
    }

    /// Commits the transaction, every statement a store call reported done
    /// with it.
    ///
    /// What an abandoned store call left is settled first: a statement that
    /// never returned is rolled back to its savepoint, so the `COMMIT` keeps
    /// exactly the statements reported done. An unusable unit of work is
    /// rolled back instead of committed.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Port`] with `PortError::Unavailable` when the
    ///   unit of work is unusable, or what an abandoned store call left cannot
    ///   be settled; nothing is committed.
    /// - [`PersistenceError::Sqlx`] when the `COMMIT` fails, for example on a
    ///   serialization failure or a lost connection.
    pub async fn commit(mut self) -> Result<(), PersistenceError> {
        let location = Location::caller();
        if let Err(refused) = recover(&mut self.tx, &mut self.statement, location).await {
            if let Err(error) = self.rollback().await {
                tracing::debug!(error = %error, "rollback of an uncommittable unit of work failed");
            }
            return Err(refused.into());
        }
        self.tx.commit().await?;
        tracing::debug!("unit of work committed");
        Ok(())
    }

    /// Rolls back the whole transaction, including the statements whose
    /// savepoints were released.
    ///
    /// The responses an abandoned call still owes are read first and
    /// discarded, so an error they hold cannot fail the `ROLLBACK`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Sqlx`] when the `ROLLBACK` fails. The server still
    /// rolls back a transaction that never ends once its session closes.
    pub async fn rollback(mut self) -> Result<(), PersistenceError> {
        if !matches!(self.statement, Settlement::Clean | Settlement::Held) {
            discard_pending(&mut self.tx).await;
        }
        self.tx.rollback().await?;
        tracing::debug!("unit of work rolled back");
        Ok(())
    }

    /// Settles what the last store call left, then records that a statement
    /// runs on the transaction itself, outside any savepoint.
    async fn begin_unguarded(&mut self) -> PortResult<()> {
        clear(&mut self.tx, &mut self.statement, Location::caller()).await?;
        self.statement = Settlement::Unguarded;
        Ok(())
    }

    /// Records how the statement [`UnitOfWork::begin_unguarded`] announced
    /// ended: a database failure aborted the transaction or broke its
    /// connection; any other outcome left it as it was.
    fn end_unguarded<T>(&mut self, outcome: &Result<T, PersistenceError>) {
        self.statement = match outcome {
            Err(PersistenceError::Sqlx(_)) => Settlement::Broken,
            _ => Settlement::Clean,
        };
    }
}

/// What the last call on a transaction left of the statement savepoint
/// `carddemo_statement`, and so what its next use must settle first.
///
/// A state that names an in-flight request is set before the request is sent
/// and replaced only once its response has been read, so a call dropped at
/// any await point leaves the state that describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settlement {
    /// No savepoint of a store call is open, and the connection owes nothing.
    Clean,
    /// A `SAVEPOINT` was sent and its response not read: the savepoint may
    /// exist, empty.
    Opening,
    /// The savepoint exists, and the statement under it is in flight, failed
    /// or never reported done, or its `ROLLBACK TO SAVEPOINT` is in flight:
    /// it must be rolled back to.
    Open,
    /// The savepoint exists and holds only statements reported done, or none:
    /// it is released by the next store call, and `COMMIT` or `ROLLBACK` ends
    /// it with the transaction.
    Held,
    /// A `RELEASE SAVEPOINT` of a held savepoint was sent and its response not
    /// read: released or still open, the savepoint keeps what it holds.
    Releasing,
    /// A statement outside any savepoint was sent and its response not read.
    Unguarded,
    /// A savepoint statement or a statement outside any savepoint failed, or
    /// the connection did: the transaction is aborted or its state unknown.
    Broken,
}

/// The [`Settlement`] a store records into: its unit of work's, or its own
/// when it was bound to a transaction directly.
#[derive(Debug)]
pub(crate) enum Ledger<'t> {
    /// The record of the unit of work the store borrows.
    Shared(&'t mut Settlement),
    /// The store's own record.
    Owned(Settlement),
}

impl Ledger<'_> {
    /// A record of its own, for a store bound to a bare transaction.
    pub(crate) const fn own() -> Self {
        Self::Owned(Settlement::Clean)
    }

    /// The record itself.
    fn state(&mut self) -> &mut Settlement {
        match self {
            Self::Shared(state) => state,
            Self::Owned(state) => state,
        }
    }

    /// The same record, lent to another store on the same transaction.
    pub(crate) fn share(&mut self) -> Ledger<'_> {
        Ledger::Shared(self.state())
    }
}

/// The port adapter: every `carddemo_domain::ports` trait, implemented over
/// one borrowed transaction.
///
/// The rules take one store that implements several ports, for example
/// `S: AccountReader + AccountWriter + CustomerReader + CustomerWriter` for
/// CAUP, so a single type carries them all. The `impl` blocks and the inherent
/// batch and admin methods live in [`crate::repos`], one file per table group;
/// `PgStore::reward_store`, declared beside [`crate::rewards::PgRewardStore`],
/// hands the same transaction to the reward engine.
///
/// The store holds the `&mut` borrow of the transaction and the record of its
/// statement savepoint, both `Send`, so the `async fn` port methods give the
/// `Send` futures the port traits declare.
#[derive(Debug)]
pub struct PgStore<'t, 'c> {
    pub(crate) tx: &'t mut Transaction<'c, Postgres>,
    /// The record the store's calls settle and update: its unit of work's, or
    /// its own.
    pub(crate) ledger: Ledger<'t>,
}

impl<'t, 'c> PgStore<'t, 'c> {
    /// Binds the adapter to a transaction the caller already holds: a test, a
    /// seed step, or an adapter working inside its own transaction.
    ///
    /// The store keeps its own record of the statement savepoint, so a call
    /// abandoned through it is settled by its next call, but not by another
    /// store bound to the same transaction: a caller that may abandon calls
    /// makes them all through this one, or works in a [`UnitOfWork`].
    #[must_use]
    pub fn new(tx: &'t mut Transaction<'c, Postgres>) -> Self {
        Self {
            tx,
            ledger: Ledger::own(),
        }
    }
}

/// The savepoint of one store statement, open on the connection it borrows.
///
/// A statement runs on it as on the connection, `&mut *sp` as the executor or
/// `&mut sp` where a `&mut PgConnection` is expected, and [`settle`] consumes
/// it. Dropped unsettled, it leaves its record [`Settlement::Open`], so the
/// next use of the transaction rolls the statement back.
pub(crate) struct Savepoint<'a> {
    conn: &'a mut PgConnection,
    state: &'a mut Settlement,
}

impl Deref for Savepoint<'_> {
    type Target = PgConnection;

    fn deref(&self) -> &PgConnection {
        self.conn
    }
}

impl DerefMut for Savepoint<'_> {
    fn deref_mut(&mut self) -> &mut PgConnection {
        self.conn
    }
}

/// Opens the savepoint `carddemo_statement` inside `tx`, for exactly one
/// statement of a store method: a legacy write, a locking read or a read.
///
/// What the last call on `tx` left, as `ledger` records it, is settled first
/// (the module's "Abandoned calls"), and a savepoint the last statement
/// reported done is released. The savepoint borrows `tx` and `ledger` until
/// [`settle`] consumes it. A port method follows this shape, with
/// `expect_affected` from [`crate::repos`] turning a rewrite of a missing key
/// into `NotFound`:
///
/// ```text
/// let mut sp = savepoint(self.tx, &mut self.ledger).await?;
/// let result = sqlx::query!("UPDATE ... WHERE account_id = $1", id)
///     .execute(&mut *sp)
///     .await;
/// let done = settle(sp, result).await?;
/// expect_affected(done.rows_affected())
/// ```
///
/// Plain functions rather than a helper taking an async closure keep every
/// port future concretely typed and `Send`.
///
/// The function is not `async`: it takes the caller's code location first,
/// through `#[track_caller]`, and returns the future that opens the
/// savepoint, so a failure is logged at the store method's line rather than
/// here. Callers still write `savepoint(self.tx, &mut self.ledger).await?`.
///
/// # Errors
///
/// - `PortError::Unavailable`, without a statement sent, when the
///   transaction is unusable.
/// - The [`port_error_at`] mapping of a failed `RELEASE SAVEPOINT`,
///   `ROLLBACK TO SAVEPOINT` or `SAVEPOINT`, or of a connection failure while
///   an abandoned call's responses are read, in practice
///   `PortError::Unavailable`; the transaction is unusable afterwards.
#[track_caller]
pub(crate) fn savepoint<'a>(
    tx: &'a mut Transaction<'_, Postgres>,
    ledger: &'a mut Ledger<'_>,
) -> impl Future<Output = PortResult<Savepoint<'a>>> + Send {
    let location = Location::caller();
    let conn: &'a mut PgConnection = tx;
    let state = ledger.state();
    async move {
        clear(conn, state, location).await?;
        // The savepoint SQL is sent here rather than through sqlx's nested
        // `Transaction`: dropped while its `SAVEPOINT` is in flight, that type
        // rolls back the whole parent transaction, and dropped while its
        // `RELEASE` is in flight, it rolls back to the released savepoint,
        // which aborts the parent.
        *state = Settlement::Opening;
        if let Err(err) = sqlx::query!("SAVEPOINT carddemo_statement")
            .execute(&mut *conn)
            .await
        {
            *state = Settlement::Broken;
            return Err(port_error_at(&err, location));
        }
        *state = Settlement::Open;
        Ok(Savepoint { conn, state })
    }
}

/// Ends the savepoint `sp` according to the `result` of the statement run
/// under it.
///
/// - `Ok`: returns the value, and records the savepoint as held before the
///   future is returned, with no await between, so a statement reported done
///   is never rolled back. The next store call releases the savepoint, and
///   `COMMIT` keeps what it holds.
/// - `Err`: rolls back to the savepoint (`ROLLBACK TO SAVEPOINT`), which undoes
///   the statement and clears the aborted state it left on the server, then
///   returns the [`port_error_at`] mapping of the statement's error. Until
///   the rollback's response is read, the record names the savepoint as still
///   to be rolled back to, so a call dropped meanwhile is rolled back by the
///   next use of the transaction.
///
/// As [`savepoint`], the function takes the caller's code location through
/// `#[track_caller]` before it returns its future, so every failure it maps
/// is logged once with the line of the store method that ran the statement.
/// Callers still write `settle(sp, result).await?`.
///
/// # Errors
///
/// - The mapped statement error, as above.
/// - The mapped error of the `ROLLBACK TO SAVEPOINT` itself, or of a
///   connection failure before it, when that fails; the transaction is
///   unusable afterwards. The statement error it hides is logged too, by its
///   own mapping.
#[track_caller]
pub(crate) fn settle<'s, T: Send + 's>(
    sp: Savepoint<'s>,
    result: Result<T, sqlx::Error>,
) -> impl Future<Output = PortResult<T>> + Send + 's {
    let location = Location::caller();
    let Savepoint { conn, state } = sp;
    if result.is_ok() {
        *state = Settlement::Held;
    }
    async move {
        let err = match result {
            Ok(value) => return Ok(value),
            Err(err) => err,
        };
        let outcome = port_error_at(&err, location);
        // A database error is the statement's last response but its
        // ReadyForQuery, which the rollback reads. Any other failure may leave
        // responses unread, an error among them, which would fail the rollback
        // before it is sent.
        if !matches!(err, sqlx::Error::Database(_)) {
            drain(conn, state, Owed::Outcome, location).await?;
        }
        roll_back_to(conn, state, location).await?;
        Err(outcome)
    }
}

/// What the responses [`drain`] reads may hold besides their
/// `ReadyForQuery`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owed {
    /// The outcome of a statement that may have failed: an error is expected,
    /// and dropped.
    Outcome,
    /// The acknowledgement of a savepoint statement or of a statement outside
    /// any savepoint: an error means that statement failed.
    Acknowledgement,
}

/// Brings the record to [`Settlement::Clean`] or [`Settlement::Held`], with
/// nothing owed on the connection, by settling what the last call left: the
/// responses an abandoned call still owes are read, and a savepoint whose
/// statement never reported done is rolled back to.
///
/// # Errors
///
/// `PortError::Unavailable` when the record is [`Settlement::Broken`], and
/// the mapped error of a failed read or rollback, which leaves it so.
async fn recover(
    conn: &mut PgConnection,
    state: &mut Settlement,
    location: &'static Location<'static>,
) -> PortResult<()> {
    match *state {
        Settlement::Clean | Settlement::Held => Ok(()),
        Settlement::Broken => Err(unusable(location)),
        Settlement::Opening | Settlement::Releasing | Settlement::Unguarded => {
            drain(conn, state, Owed::Acknowledgement, location).await?;
            *state = Settlement::Clean;
            Ok(())
        }
        Settlement::Open => {
            drain(conn, state, Owed::Outcome, location).await?;
            roll_back_to(conn, state, location).await
        }
    }
}

/// [`recover`], then releases a held savepoint, so the record is
/// [`Settlement::Clean`].
///
/// # Errors
///
/// Those of [`recover`], and the mapped error of a failed
/// `RELEASE SAVEPOINT`, which leaves the record [`Settlement::Broken`].
async fn clear(
    conn: &mut PgConnection,
    state: &mut Settlement,
    location: &'static Location<'static>,
) -> PortResult<()> {
    recover(conn, state, location).await?;
    if *state == Settlement::Held {
        *state = Settlement::Releasing;
        if let Err(err) = sqlx::query!("RELEASE SAVEPOINT carddemo_statement")
            .execute(&mut *conn)
            .await
        {
            *state = Settlement::Broken;
            return Err(port_error_at(&err, location));
        }
        *state = Settlement::Clean;
    }
    Ok(())
}

/// Rolls back to the open savepoint: the statement under it is undone, the
/// aborted state it left cleared, and the savepoint stays, empty, for the next
/// store call to release. The record stays [`Settlement::Open`] until the
/// response is read, so a rollback abandoned in flight is sent again.
///
/// # Errors
///
/// The mapped error of a failed `ROLLBACK TO SAVEPOINT`, which leaves the
/// record [`Settlement::Broken`].
async fn roll_back_to(
    conn: &mut PgConnection,
    state: &mut Settlement,
    location: &'static Location<'static>,
) -> PortResult<()> {
    match sqlx::query!("ROLLBACK TO SAVEPOINT carddemo_statement")
        .execute(&mut *conn)
        .await
    {
        Ok(_) => {
            *state = Settlement::Held;
            Ok(())
        }
        Err(err) => {
            *state = Settlement::Broken;
            Err(port_error_at(&err, location))
        }
    }
}

/// Reads every response the connection still owes, for the requests an
/// abandoned call sent before it was dropped.
///
/// Each `ping` sends a `Sync` and reads up to its `ReadyForQuery`, stopping
/// early at an error response, which it consumes. The loop therefore ends once
/// every error is read: an [`Owed::Outcome`] error is logged at `debug` and
/// dropped, an [`Owed::Acknowledgement`] error or a connection failure leaves
/// the record [`Settlement::Broken`].
///
/// # Errors
///
/// The mapped error that breaks the record.
async fn drain(
    conn: &mut PgConnection,
    state: &mut Settlement,
    owed: Owed,
    location: &'static Location<'static>,
) -> PortResult<()> {
    loop {
        match sqlx::Connection::ping(&mut *conn).await {
            Ok(()) => return Ok(()),
            Err(sqlx::Error::Database(db)) if owed == Owed::Outcome => {
                tracing::debug!(
                    error = %crate::database_text(db.as_ref()),
                    "abandoned statement failed"
                );
            }
            Err(err) => {
                *state = Settlement::Broken;
                return Err(port_error_at(&err, location));
            }
        }
    }
}

/// Reads and drops every response the connection still owes, errors
/// included, before a `ROLLBACK` discards the work they belong to. A
/// connection failure is left to the `ROLLBACK` to report.
async fn discard_pending(conn: &mut PgConnection) {
    while let Err(sqlx::Error::Database(db)) = sqlx::Connection::ping(&mut *conn).await {
        tracing::debug!(
            error = %crate::database_text(db.as_ref()),
            "abandoned statement failed"
        );
    }
}

/// The text of a call refused on an unusable transaction.
const UNUSABLE: &str = "the unit of work is unusable: a savepoint statement, a statement \
                        outside a savepoint or the connection failed earlier";

/// The outcome of a call refused, without a statement sent, on a transaction
/// whose record is [`Settlement::Broken`], logged once at `location`.
fn unusable(location: &'static Location<'static>) -> PortError {
    tracing::warn!(
        outcome = crate::OUTCOME_UNAVAILABLE,
        error = UNUSABLE,
        code.filepath = location.file(),
        code.lineno = location.line(),
        "database statement refused"
    );
    PortError::Unavailable(UNUSABLE.to_owned())
}
