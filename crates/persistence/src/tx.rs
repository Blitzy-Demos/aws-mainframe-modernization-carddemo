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
//!   under its own savepoint. Success releases it (`RELEASE SAVEPOINT`).
//!   Failure rolls back to it (`ROLLBACK TO SAVEPOINT`), which undoes that
//!   statement alone and clears the aborted state, and returns the mapped
//!   [`PortError`](carddemo_domain::ports::PortError).
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
//! it back too: sqlx queues the `ROLLBACK` on the connection, and it runs
//! before the connection is used again or returned to the pool.
//!
//! # Tracing
//!
//! Spans belong to the adapters, which record the outcome of each request,
//! job record or message. Besides its `debug` events for a commit and a
//! rollback, this module logs every failure `savepoint` and `settle` map, once,
//! through `crate::port_error_at`: one `database statement failed` event with
//! the SQLSTATE, the outcome, the sanitized error text, the table and
//! constraint names, and the code location of the store method that ran the
//! statement (`code.filepath`, `code.lineno`). `NotFound` is not logged.

use std::future::Future;
use std::panic::Location;

use carddemo_domain::numeric::{AccountId, TranId};
use carddemo_domain::ports::PortResult;
use sqlx::{Acquire, Postgres, Transaction};

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
}

impl UnitOfWork {
    /// Wraps a transaction just begun on a pooled connection.
    #[must_use]
    pub(crate) fn new(tx: Transaction<'static, Postgres>) -> Self {
        Self { tx }
    }

    /// The transaction itself, for the adapters this crate binds to it
    /// outside this module.
    pub(crate) fn transaction(&mut self) -> &mut Transaction<'static, Postgres> {
        &mut self.tx
    }

    /// The port adapter bound to this transaction.
    ///
    /// The store borrows the unit of work, so every call made through it
    /// belongs to this transaction and none outlives it.
    #[must_use]
    pub fn store(&mut self) -> PgStore<'_, 'static> {
        PgStore::new(&mut self.tx)
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
    /// - [`PersistenceError::Sqlx`] when the lock or the read fails.
    /// - [`PersistenceError::Corrupt`] when the highest stored key is not 16
    ///   digits.
    pub async fn next_tran_id(&mut self) -> Result<TranId, PersistenceError> {
        crate::ids::next_tran_id(&mut self.tx).await
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
    /// [`PersistenceError::Sqlx`] when the statement fails.
    pub async fn lock_account_reference(
        &mut self,
        account_id: AccountId,
    ) -> Result<(), PersistenceError> {
        crate::ids::lock_account_reference(&mut self.tx, account_id).await
    }

    /// Commits the transaction, every released savepoint with it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Sqlx`] when the `COMMIT` fails, for example on a
    /// serialization failure or a lost connection.
    pub async fn commit(self) -> Result<(), PersistenceError> {
        self.tx.commit().await?;
        tracing::debug!("unit of work committed");
        Ok(())
    }

    /// Rolls back the whole transaction, including the writes whose
    /// savepoints were released.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Sqlx`] when the `ROLLBACK` fails. The server still
    /// rolls back a transaction that never ends once its session closes.
    pub async fn rollback(self) -> Result<(), PersistenceError> {
        self.tx.rollback().await?;
        tracing::debug!("unit of work rolled back");
        Ok(())
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
/// The store holds only the `&mut` borrow, which is `Send`, so the `async fn`
/// port methods give the `Send` futures the port traits declare.
#[derive(Debug)]
pub struct PgStore<'t, 'c> {
    pub(crate) tx: &'t mut Transaction<'c, Postgres>,
}

impl<'t, 'c> PgStore<'t, 'c> {
    /// Binds the adapter to a transaction the caller already holds: a test, a
    /// seed step, or an adapter working inside its own transaction.
    #[must_use]
    pub fn new(tx: &'t mut Transaction<'c, Postgres>) -> Self {
        Self { tx }
    }
}

/// Opens a savepoint inside `tx`, for exactly one statement of a store method:
/// a legacy write, a locking read or a read.
///
/// sqlx issues `SAVEPOINT` rather than `BEGIN` because the connection is
/// already in a transaction. The savepoint borrows `tx` until [`settle`]
/// consumes it. A port method follows this shape, with `expect_affected`
/// from [`crate::repos`] turning a rewrite of a missing key into `NotFound`:
///
/// ```text
/// let mut sp = savepoint(self.tx).await?;
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
/// here. Callers still write `savepoint(self.tx).await?`.
///
/// # Errors
///
/// The [`port_error_at`] mapping of a failed `SAVEPOINT`, in practice
/// `PortError::Unavailable` for a broken connection.
#[track_caller]
pub(crate) fn savepoint<'a>(
    tx: &'a mut Transaction<'_, Postgres>,
) -> impl Future<Output = PortResult<Transaction<'a, Postgres>>> + Send {
    let location = Location::caller();
    async move {
        Acquire::begin(tx)
            .await
            .map_err(|err| port_error_at(&err, location))
    }
}

/// Ends the savepoint `sp` according to the `result` of the statement run
/// under it.
///
/// - `Ok`: releases the savepoint (`RELEASE SAVEPOINT`) and returns the value.
///   What the statement did stays part of the outer transaction.
/// - `Err`: rolls back to the savepoint (`ROLLBACK TO SAVEPOINT`), which undoes
///   the statement and clears the aborted state it left on the server, then
///   returns the [`port_error_at`] mapping of the statement's error.
///
/// As [`savepoint`], the function takes the caller's code location through
/// `#[track_caller]` before it returns its future, so every failure it maps
/// is logged once with the line of the store method that ran the statement.
/// Callers still write `settle(sp, result).await?`.
///
/// # Errors
///
/// - The mapped statement error, as above.
/// - The mapped error of the `RELEASE SAVEPOINT` or `ROLLBACK TO SAVEPOINT`
///   itself when that fails. sqlx then still queues the rollback to the
///   savepoint for the connection's next use. The statement error a failed
///   rollback hides is logged too, by its own mapping.
#[track_caller]
pub(crate) fn settle<'s, T: Send + 's>(
    sp: Transaction<'s, Postgres>,
    result: Result<T, sqlx::Error>,
) -> impl Future<Output = PortResult<T>> + Send + 's {
    let location = Location::caller();
    async move {
        match result {
            Ok(value) => {
                sp.commit()
                    .await
                    .map_err(|err| port_error_at(&err, location))?;
                Ok(value)
            }
            Err(err) => {
                let outcome = port_error_at(&err, location);
                if let Err(rollback) = sp.rollback().await {
                    return Err(port_error_at(&rollback, location));
                }
                Err(outcome)
            }
        }
    }
}
