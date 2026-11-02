//! The limits on every database call of the CDRA and CP00 flows.
//!
//! The pool bounds only the wait for a free connection: `Db::begin` gives up
//! after sqlx's acquire timeout of 30 seconds. Once a connection is held, a
//! `BEGIN`, a read, a write, a `COMMIT` or a `ROLLBACK` can wait without end,
//! behind a lock another session holds or on a server that stopped answering.
//! Each consumer settles one delivery before it reads the next (prefetch 1),
//! so one such call would stop its flow for good.
//!
//! Two limits bound that wait, and each flow answers either one with the
//! outcome it already has for a failed call: CDRA's `ERROR WHILE READING
//! ACCTFILE` record (P-COACCT01-015), CP00's `C001` to `C003` or `I002` record
//! before the reply (P-COPAUA0C-006 to P-COPAUA0C-008) and its `I003` or
//! `I004` record after the acknowledgement (P-COPAUA0C-013, P-COPAUA0C-014).
//! Both fail the call as [`PortError::Unavailable`], the `WHEN OTHER` outcome
//! the COBOL gives any other I/O failure. No call is retried and no request is
//! requeued or replayed.
//!
//! - [`STATEMENT_TIMEOUT`], on the server: the `mq` binary opens its pool
//!   with it, so `PostgreSQL` cancels a statement that runs longer. The
//!   cancelled statement fails like any other. A read fails its read, and the
//!   flow rolls its read-only unit of work back. A write rolls back to its
//!   savepoint and the unit of work stays usable, so a `COMMIT` after a
//!   cancelled detail insert keeps the summary written before it and none of
//!   the detail, as `RECOVERY(NONE)` keeps a completed write
//!   (P-COPAUA0C-035).
//! - [`DB_DEADLINE`], on the client: every database call a flow makes runs
//!   under it, for a server that stops answering, or a pool opened without
//!   the statement timeout. A call that outlives it is abandoned: its future
//!   is dropped wherever it waits. The unit of work's next call, `COMMIT` or
//!   `ROLLBACK` first settles what the abandoned call left on the connection,
//!   within its own deadline: a statement that never returned is rolled back
//!   to its own savepoint, every statement reported done before it stays, and
//!   the same transaction still commits them (`carddemo_persistence::tx`,
//!   "Abandoned calls"). A `COMMIT` that cannot complete within its own
//!   deadline is reported as the lost summary, `I003`.
//!
//! [`BoundedStore`] puts every port call of a store under the deadline and
//! logs the cause of each failure other than `NOTFND`. The `CCPAUERY` record
//! and the error-queue record keep only the legacy codes, so this log line is
//! where the database cause stays; it carries the [`PortError`] text, which
//! holds no server detail line.
//!
//! The timer is tokio's `time` driver. sqlx's `runtime-tokio` support, which
//! `carddemo-persistence` enables, compiles it into every build of this
//! crate, and the `mq` binary's runtime and the test runtime enable it.

use std::future::Future;
use std::time::Duration;

use carddemo_domain::entities::{
    Account, AuthFraud, CardXref, Customer, NewPendingAuthDetail, PendingAuthDetail,
    PendingAuthSummary,
};
use carddemo_domain::numeric::{AccountId, AuthId, CardNumber, CustomerId};
use carddemo_domain::ports::{
    AccountReader, AuthorizationStore, Browse, CustomerReader, Direction, PortError, PortResult,
    XrefReader,
};

/// How long one database call may take: the pool's own acquire timeout, so a
/// call waits no longer for the database than `Db::begin` waits for a
/// connection.
pub(crate) const DB_DEADLINE: Duration = Duration::from_secs(30);

/// How long the server lets one statement of the `mq` process run before it
/// cancels it: the 30-second deadline of each database call, less five
/// seconds. The server therefore gives up on a blocked statement first, and
/// the rollback to its savepoint and the `COMMIT` or `ROLLBACK` that follow
/// still finish within the call's deadline.
pub const STATEMENT_TIMEOUT: Duration = DB_DEADLINE.saturating_sub(Duration::from_secs(5));

/// Runs the database call `operation`, named `call` in its failure, for at
/// most [`DB_DEADLINE`].
///
/// # Errors
///
/// The [`PortError`] mapping of the call's own failure, or
/// [`PortError::Unavailable`] when the deadline elapses first.
pub(crate) async fn bounded<T, E>(
    call: &'static str,
    operation: impl Future<Output = Result<T, E>>,
) -> PortResult<T>
where
    PortError: From<E>,
{
    bounded_by(DB_DEADLINE, call, operation).await
}

/// [`bounded`] with the deadline `deadline`.
async fn bounded_by<T, E>(
    deadline: Duration,
    call: &'static str,
    operation: impl Future<Output = Result<T, E>>,
) -> PortResult<T>
where
    PortError: From<E>,
{
    match tokio::time::timeout(deadline, operation).await {
        Ok(result) => result.map_err(PortError::from),
        Err(_elapsed) => Err(PortError::Unavailable(format!(
            "{call} did not complete within its database deadline of {deadline:?}"
        ))),
    }
}

/// A store whose every port call runs under the database deadline and logs
/// the cause of its failure.
///
/// It wraps the `PgStore` of a unit of work for the domain rules that take a
/// store: CP00's reads (`auth_decision::gather`) and writes
/// (`8400-UPDATE-SUMMARY`, `8500-INSERT-AUTH`).
#[derive(Debug)]
pub(crate) struct BoundedStore<S> {
    /// The transaction ID of the flow, the `flow` field of the log line.
    flow: &'static str,
    /// The deadline of each call.
    deadline: Duration,
    /// The store the calls go to.
    inner: S,
}

impl<S> BoundedStore<S> {
    /// `inner` with every call bounded by [`DB_DEADLINE`], logged for `flow`.
    pub(crate) const fn new(flow: &'static str, inner: S) -> Self {
        Self::with_deadline(flow, inner, DB_DEADLINE)
    }

    /// `inner` with every call bounded by `deadline`, logged for `flow`.
    pub(crate) const fn with_deadline(flow: &'static str, inner: S, deadline: Duration) -> Self {
        Self {
            flow,
            deadline,
            inner,
        }
    }
}

/// Logs `result`'s failure other than `NOTFND`, which the COBOL branches on as
/// an ordinary outcome, and returns `result`.
fn logged<T>(flow: &'static str, call: &'static str, result: PortResult<T>) -> PortResult<T> {
    if let Err(error) = &result
        && !matches!(error, PortError::NotFound)
    {
        tracing::warn!(flow, call, error = %error, "database call failed");
    }
    result
}

/// Port methods forwarded to the inner store under the deadline, each failure
/// logged with the method's name as `call`.
macro_rules! bounded_calls {
    ($($method:ident($($arg:ident: $ty:ty),*) -> $output:ty;)+) => {
        $(
            async fn $method(&mut self, $($arg: $ty),*) -> PortResult<$output> {
                let call = stringify!($method);
                let result = bounded_by(self.deadline, call, self.inner.$method($($arg),*)).await;
                logged(self.flow, call, result)
            }
        )+
    };
}

impl<S: XrefReader + Send> XrefReader for BoundedStore<S> {
    bounded_calls! {
        read_xref_by_card(card: CardNumber) -> CardXref;
        read_xref_by_account(id: AccountId) -> CardXref;
        xrefs_after(after: Option<CardNumber>, limit: u16) -> Vec<CardXref>;
    }
}

impl<S: AccountReader + Send> AccountReader for BoundedStore<S> {
    bounded_calls! {
        read_account(id: AccountId) -> Account;
        read_account_for_update(id: AccountId) -> Account;
    }
}

impl<S: CustomerReader + Send> CustomerReader for BoundedStore<S> {
    bounded_calls! {
        read_customer(id: CustomerId) -> Customer;
        read_customer_for_update(id: CustomerId) -> Customer;
    }
}

impl<S: AuthorizationStore + Send> AuthorizationStore for BoundedStore<S> {
    bounded_calls! {
        read_summary(id: AccountId) -> PendingAuthSummary;
        read_summary_of_detail(id: AuthId) -> PendingAuthSummary;
        insert_summary(summary: PendingAuthSummary) -> ();
        update_summary(summary: PendingAuthSummary) -> ();
        delete_summary(id: AccountId) -> ();
        summaries_after(after: Option<AccountId>, limit: u16) -> Vec<PendingAuthSummary>;
        insert_detail(detail: NewPendingAuthDetail) -> AuthId;
        read_detail(id: AuthId) -> PendingAuthDetail;
        read_detail_for_update(id: AuthId) -> PendingAuthDetail;
        details_for_account(id: AccountId, browse: Browse<AuthId>) -> Vec<PendingAuthDetail>;
        adjacent_detail(
            account: AccountId,
            auth_date_9c: i32,
            auth_time_9c: i32,
            direction: Direction
        ) -> Option<PendingAuthDetail>;
        update_detail(detail: PendingAuthDetail) -> ();
        delete_detail(id: AuthId) -> ();
        insert_fraud(fraud: AuthFraud) -> ();
        update_fraud(fraud: AuthFraud) -> ();
    }
}

#[cfg(test)]
mod tests {
    use std::future;
    use std::time::Duration;

    use carddemo_domain::entities::Account;
    use carddemo_domain::numeric::AccountId;
    use carddemo_domain::ports::{AccountReader, PortError, PortResult};
    use carddemo_testkit::block_on;

    use super::{BoundedStore, DB_DEADLINE, STATEMENT_TIMEOUT, bounded, bounded_by};

    /// A deadline short enough for a test to wait it out.
    const SHORT: Duration = Duration::from_millis(20);

    /// An account file whose reads answer `NOTFND`, or never answer.
    struct Accounts {
        stalled: bool,
    }

    impl Accounts {
        async fn answer(&self) -> PortResult<Account> {
            if self.stalled {
                future::pending().await
            } else {
                Err(PortError::NotFound)
            }
        }
    }

    impl AccountReader for Accounts {
        async fn read_account(&mut self, _id: AccountId) -> PortResult<Account> {
            self.answer().await
        }

        async fn read_account_for_update(&mut self, _id: AccountId) -> PortResult<Account> {
            self.answer().await
        }
    }

    fn account_id() -> AccountId {
        AccountId::new(1).expect("an account number")
    }

    #[test]
    fn the_deadline_is_the_pool_acquire_timeout() {
        // The server cancels a blocked statement five seconds before the
        // client gives up on the call.
        assert_eq!(DB_DEADLINE, Duration::from_secs(30));
        assert_eq!(STATEMENT_TIMEOUT, Duration::from_secs(25));
    }

    #[test]
    fn a_call_past_its_deadline_fails_as_unavailable() {
        // P-COACCT01-015, P-COPAUA0C-006: the WHEN OTHER outcome of a failed
        // call, never a retry.
        let result = block_on(bounded_by(
            SHORT,
            "read_account",
            future::pending::<PortResult<()>>(),
        ));
        assert_eq!(
            result,
            Err(PortError::Unavailable(
                "read_account did not complete within its database deadline of 20ms".to_owned()
            ))
        );
    }

    #[test]
    fn a_call_within_its_deadline_keeps_its_outcome() {
        assert_eq!(
            block_on(bounded("begin", future::ready(Ok::<u8, PortError>(7)))),
            Ok(7)
        );
        for error in [
            PortError::NotFound,
            PortError::Duplicate,
            PortError::Contention,
        ] {
            let outcome = block_on(bounded(
                "commit",
                future::ready(Err::<(), _>(error.clone())),
            ));
            assert_eq!(outcome, Err(error));
        }
    }

    #[test]
    fn every_store_call_is_bounded_and_keeps_notfnd() {
        let id = account_id();
        let mut stalled = BoundedStore::with_deadline("CDRA", Accounts { stalled: true }, SHORT);
        for result in [
            block_on(stalled.read_account(id)),
            block_on(stalled.read_account_for_update(id)),
        ] {
            assert!(
                matches!(&result, Err(PortError::Unavailable(text)) if text.contains("deadline")),
                "{result:?}"
            );
        }
        let mut answering = BoundedStore::new("CDRA", Accounts { stalled: false });
        assert_eq!(
            block_on(answering.read_account(id)),
            Err(PortError::NotFound)
        );
    }
}
