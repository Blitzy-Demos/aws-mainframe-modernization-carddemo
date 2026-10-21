//! Endpoint 15, `POST /api/v1/bill-payments`: the service of COBIL00C (transaction `CB00`, map
//! `COBIL0A`; AAP 0.3.4.1), called only by `api::handlers::cobil00c`.
//!
//! `domain::online::cobil00c` decides every outcome; this module carries out the file statements
//! of `PROCESS-ENTER-KEY` (`COBIL00C.cbl:154-244,343-548`) through the repository traits, in the
//! program's order:
//!
//! 1. The edits of the two inputs, before any read: a blank account (COBIL00C-007), then the
//!    confirmation (COBIL00C-009 to COBIL00C-012), then the key (COBIL00C-008, COBIL00C-014).
//!    `N` or `n` clears the screen without a read (COBIL00C-010).
//! 2. `READ ACCTDAT … UPDATE` for `Y`, `y` and a blank confirmation alike: the account row is
//!    locked inside one `PgTx` until the request ends (COBIL00C-011, COBIL00C-013), and a read
//!    that finds no row or fails ends the request with nothing written (COBIL00C-014,
//!    COBIL00C-015).
//! 3. The balance test: zero or less is `You have nothing to pay...` (COBIL00C-018, ARITH-018);
//!    a blank confirmation shows the balance with the prompt (COBIL00C-017, COBIL00C-019); `Y`
//!    pays.
//! 4. The payment, in one transaction: the cross-reference by account (COBIL00C-020 to
//!    COBIL00C-022), the transaction with the highest ID and the next ID (COBIL00C-023,
//!    COBIL00C-024), the insert of the payment record (COBIL00C-025 to COBIL00C-030) and the
//!    balance rewrite, which runs whatever the insert gave (COBIL00C-031 to COBIL00C-033, E11).
//!    The transaction then commits whatever the two writes gave, as the task's normal `RETURN`
//!    commits every write that succeeded, and the last message set is the answer
//!    (COBIL00C-034).
//!
//! # The unit of work
//!
//! `SEND-BILLPAY-SCREEN` does not end the task, so a step that fails does not stop the steps
//! after it. A failed statement aborts a `PostgreSQL` transaction, so every statement the payment
//! must go on after runs inside its own savepoint, rolled back when the statement fails: the
//! cross-reference read, the insert and the rewrite. A successful insert therefore stands when
//! the rewrite fails, and the rewrite stands when the insert fails (E11).
//!
//! A store error with no audited message, from `begin`, `commit`, `rollback` or a savepoint, is
//! [`ServiceError::Store`]. Returned in the middle of the payment, it drops the open `PgTx`,
//! which rolls the transaction back, so nothing of the payment stands.
//!
//! # Money and cardholder data
//!
//! No amount is computed here: the transaction amount and the new balance come from
//! `cobil00c::payment` (ARITH-020 to ARITH-022), the shown balance from `cobil00c::decide`
//! (ARITH-019). The card number of the cross-reference reaches only the payment record
//! (CVTRA05Y-011). Nothing here logs or traces a record, an amount or a card number (Cardholder
//! Data Redaction), and no store error's text reaches an answer.

use domain::clock::Clock;
use domain::copybook::{PicU, PicX};
use domain::online::cobil00c::{
    self, AccountRewrite, BillPayDecision, BillPayFailure, BillPayInput, BillPayOutcome,
    EnterAction, ReadIntent, TranWrite,
};
use domain::records::account::AccountRecord;
use domain::records::card_xref::CardXrefRecord;
use domain::records::transaction::TransactionRecord;
use persistence::{
    AccountRepository, CardXrefRepository, PersistenceError, PgTx, TransactionRepository,
};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cobil00c::BillPaymentRequest;
use crate::error::ServiceError;

/// The answer of endpoint 15: the screen COBIL00C ends on with 200, or its failure.
type PayResult = Result<BillPayOutcome, ServiceError<BillPayFailure>>;

/// ENTER on `COBIL0A`: shows the balance of the account, pays it or clears the screen
/// (`PROCESS-ENTER-KEY`, `COBIL00C.cbl:154-244`; COBIL00C-007 to COBIL00C-034).
///
/// The signed-on user only guards the endpoint: COBIL00C checks no user type, and option 10 is
/// open to `U` and `A` users alike (COBIL00C-003).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the program's failure: a blank account or an unknown
///   confirmation (400), an account that is not found (404), a failed account read, a failed
///   browse of the transactions, a duplicate or failed transaction write or a failed balance
///   rewrite (500), and a balance of zero or less (400). The failure carries its audited
///   message, field and status class.
/// - [`ServiceError::Store`] when the transaction cannot be begun, committed or rolled back, or a
///   savepoint cannot be opened or ended.
pub(crate) async fn pay(
    state: &AppState,
    _user: &SignedOnUser,
    request: BillPaymentRequest,
) -> PayResult {
    let BillPaymentRequest { acct_id, confirm } = request;
    let input = BillPayInput {
        acct_id: &acct_id,
        confirm: &confirm,
    };
    match cobil00c::edit(&input).map_err(ServiceError::Rule)? {
        EnterAction::Clear => Ok(BillPayOutcome::Cleared),
        EnterAction::ReadAccount { id, intent } => read_account(state, id, intent).await,
    }
}

/// `READ-ACCTDAT-FILE` with `UPDATE`, then the balance test of `PROCESS-ENTER-KEY`
/// (`COBIL00C.cbl:193-210,343-372`; COBIL00C-013 to COBIL00C-019).
///
/// A blank confirmation reads for update too, as `READ-ACCTDAT-FILE` always does: a balance
/// request for an account whose payment is in flight waits for it and shows the balance it left
/// (COBIL00C-011, COBIL00C-013).
async fn read_account(state: &AppState, id: PicU<11>, intent: ReadIntent) -> PayResult {
    let mut tx = state.store.begin().await?;
    let account = match AccountRepository::lock_by_id(&mut tx, id).await {
        Ok(Some(account)) => account,
        Ok(None) => return abandon(tx, cobil00c::account_not_found()).await,
        Err(_) => return abandon(tx, cobil00c::account_lookup_failed()).await,
    };
    match cobil00c::decide(intent, &account) {
        Err(failure) => abandon(tx, failure).await,
        Ok(BillPayDecision::ShowBalance(outcome)) => show_balance(tx, outcome).await,
        Ok(BillPayDecision::Pay) => make_payment(tx, id, &account, &*state.clock).await,
    }
}

/// The balance and the prompt `Confirm to make a bill payment...` (COBIL00C-019): nothing is
/// written, so the transaction is rolled back, which releases the row lock.
async fn show_balance(tx: PgTx<'_>, outcome: BillPayOutcome) -> PayResult {
    tx.rollback().await?;
    Ok(outcome)
}

/// Ends the request on `failure` before anything is written: the transaction is rolled back,
/// releasing the row lock (COBIL00C-014, COBIL00C-015, COBIL00C-018, COBIL00C-024).
async fn abandon(tx: PgTx<'_>, failure: BillPayFailure) -> PayResult {
    tx.rollback().await?;
    Err(ServiceError::Rule(failure))
}

/// The payment block of `PROCESS-ENTER-KEY` (`COBIL00C.cbl:208-235`; COBIL00C-020 to
/// COBIL00C-034), on the transaction that holds the lock of `account`.
///
/// The insert and the rewrite each run in a savepoint, the rewrite whatever the insert gave
/// (E11), and the transaction commits whatever both gave; `cobil00c::payment_outcome` then
/// picks the last message set.
async fn make_payment(
    mut tx: PgTx<'_>,
    id: PicU<11>,
    account: &AccountRecord,
    clock: &dyn Clock,
) -> PayResult {
    let xref = read_xref(&mut tx, id).await?;
    // HALTED(X-COBIL00C-H3): a failed browse ends here with nothing written, the replacement's
    // answer; the COBOL runs on with the key the failed command left (COBIL00C-024).
    let Ok((prior, new_id)) = find_highest(&mut tx).await else {
        return abandon(tx, cobil00c::transaction_lookup_failed()).await;
    };
    let effects = cobil00c::payment(
        new_id.clone(),
        prior.as_ref(),
        account,
        xref.as_ref(),
        clock,
    );
    let write = write_transaction(&mut tx, &effects.transaction).await?;
    let rewrite = rewrite_account(&mut tx, &effects.account).await?;
    tx.commit().await?;
    cobil00c::payment_outcome(&new_id, write, rewrite).map_err(ServiceError::Rule)
}

/// `READ-CXACAIX-FILE`: the first cross-reference of the account through `CXACAIX`, the one
/// with the lowest card number when several share it (`COBIL00C.cbl:408-436`; COBIL00C-020).
///
/// The payment block tests no error between its steps, so a cross-reference that is not found
/// or a read that fails does not stop the payment, and its message never reaches the answer
/// (COBIL00C-021, COBIL00C-022, X-COBIL00C-1). Both give `None`, for which `cobil00c::payment`
/// writes 16 spaces as the card number: `HALTED(X-COBIL00C-H2)`, no parity result. The read
/// runs in a savepoint, so a failed statement leaves the payment's transaction usable.
///
/// # Errors
///
/// The [`PersistenceError`] of opening, releasing or rolling back the savepoint.
async fn read_xref(
    tx: &mut PgTx<'_>,
    id: PicU<11>,
) -> Result<Option<CardXrefRecord>, PersistenceError> {
    let mut savepoint = tx.savepoint().await?;
    if let Ok(found) = CardXrefRepository::find_by_acct_id(&mut savepoint, id).await {
        savepoint.commit().await?;
        Ok(found)
    } else {
        savepoint.rollback().await?;
        Ok(None)
    }
}

/// `STARTBR` at `HIGH-VALUES`, `READPREV` and `ENDBR` (`COBIL00C.cbl:212-217,441-505`;
/// COBIL00C-023, COBIL00C-025): the transaction with the highest ID, whose FILLER the payment
/// record keeps, and the next ID.
///
/// On an empty table there is no such transaction and the next ID is `0000000000000001`.
///
/// # Errors
///
/// The first [`PersistenceError`] of the two reads, which the caller answers with
/// `Unable to lookup Transaction...` (COBIL00C-024).
async fn find_highest(
    tx: &mut PgTx<'_>,
) -> Result<(Option<TransactionRecord>, PicX<16>), PersistenceError> {
    let prior = TransactionRepository::find_last(tx).await?;
    let new_id = TransactionRepository::next_id(tx).await?;
    Ok((prior, new_id))
}

/// `WRITE-TRANSACT-FILE` of the payment record, in a savepoint (`COBIL00C.cbl:510-547`):
/// written (COBIL00C-028), a duplicate ID (`DUPREC`, COBIL00C-029) or another failure
/// (COBIL00C-030). A failed insert is rolled back to the savepoint, so the rewrite after it
/// still runs and stands (E11).
///
/// # Errors
///
/// The [`PersistenceError`] of opening, releasing or rolling back the savepoint.
async fn write_transaction(
    tx: &mut PgTx<'_>,
    record: &TransactionRecord,
) -> Result<TranWrite, PersistenceError> {
    let mut savepoint = tx.savepoint().await?;
    match TransactionRepository::insert(&mut savepoint, record).await {
        Ok(()) => {
            savepoint.commit().await?;
            Ok(TranWrite::Written)
        }
        Err(error) => {
            savepoint.rollback().await?;
            Ok(if error.is_duplicate() {
                TranWrite::Duplicate
            } else {
                TranWrite::Failed
            })
        }
    }
}

/// `UPDATE-ACCTDAT-FILE`, the `REWRITE` of the debited account, in a savepoint
/// (`COBIL00C.cbl:377-403`): rewritten (COBIL00C-031), no row with the key (COBIL00C-032) or a
/// failure (COBIL00C-033). A failed rewrite is rolled back to the savepoint, so a transaction
/// the insert wrote still commits.
///
/// # Errors
///
/// The [`PersistenceError`] of opening, releasing or rolling back the savepoint.
async fn rewrite_account(
    tx: &mut PgTx<'_>,
    record: &AccountRecord,
) -> Result<AccountRewrite, PersistenceError> {
    let mut savepoint = tx.savepoint().await?;
    if let Ok(stored) = AccountRepository::update(&mut savepoint, record).await {
        savepoint.commit().await?;
        Ok(if stored {
            AccountRewrite::Rewritten
        } else {
            AccountRewrite::NotFound
        })
    } else {
        savepoint.rollback().await?;
        Ok(AccountRewrite::Failed)
    }
}
