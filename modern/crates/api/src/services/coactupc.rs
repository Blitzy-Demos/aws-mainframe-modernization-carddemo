//! COACTUPC, account update (transaction `CAUP`, map `CACTUPA`): the service of endpoint 5,
//! `GET /api/v1/accounts/{acct_id}` (ENTER fetch), and endpoint 6, `PUT
//! /api/v1/accounts/{acct_id}` (ENTER validate, F5 save).
//!
//! This module ports the program's file statements and nothing else: every edit, comparison,
//! decision and message is `domain::online::coactupc`'s, and only `api::handlers::coactupc`
//! calls it (Thin Request Handlers).
//!
//! # Endpoint 5
//!
//! [`fetch`] edits the path account number (COACTUPC-045 to COACTUPC-047) and runs the three
//! reads of `9000-READ-ACCT` on the pool: the cross-reference by account through `CXACAIX`, the
//! account, and the customer by the cross-reference's customer ID (COACTUPC-084). Each read's
//! outcome goes to the domain, which answers that read's message or builds the view and its
//! snapshot (COACTUPC-068, COACTUPC-085 to COACTUPC-100). Endpoint 5 writes nothing.
//!
//! # Endpoint 6
//!
//! [`save`] first re-reads that chain for what the COBOL holds on its own screen and in its
//! COMMAREA: the stored SSN the edits and the change check need when the SSN parts are omitted or
//! re-entered (COACTUPC-044, COACTUPC-064), and `CDEMO-CUST-ID`, the customer the save locks
//! (COACTUPC-112). The echoed snapshot serves only as the old values compared against: it is
//! never a key and grants nothing (COACTUPC-100, COACTUPC-110). `2000-DECIDE-ACTION` then
//! decides; ENTER, and F5 without validated changes, write nothing (COACTUPC-004, COACTUPC-012
//! to COACTUPC-014, COACTUPC-043, COACTUPC-070). F5 with validated changes runs
//! `9600-WRITE-PROCESSING` in one transaction (COACTUPC-071):
//!
//! 1. `READ ACCTDAT UPDATE` by the path account: no row, or a failed read or lock, is `Could not
//!    lock account record for update` (COACTUPC-101).
//! 2. `READ CUSTDAT UPDATE` by the cross-reference's customer ID: no row, or a failed read or
//!    lock, is `Could not lock customer record for update` (COACTUPC-102).
//! 3. `9700-CHECK-CHANGE-IN-REC` on the two locked records: a difference is the 409 `Record
//!    changed by some one else. Please review` (COACTUPC-113 to COACTUPC-116).
//! 4. `REWRITE ACCTDAT`: a failure is `Update of record failed`, neither record changed
//!    (COACTUPC-103, COACTUPC-106, COACTUPC-109, COACTUPC-111).
//! 5. `REWRITE CUSTDAT`: success commits both rewrites (COACTUPC-104, COACTUPC-108); a failure
//!    is `Update of record failed`, the account rewrite kept (COACTUPC-107).
//!
//! Every failure of steps 1 to 4 rolls the transaction back before it answers, so nothing is
//! written and both row locks are released. The locks are held only across these statements and
//! the pure domain calls between them.
//!
//! # Redaction
//!
//! The SSN stays inside its `Ssn` wrapper: the re-entered SSN parts are request input only the
//! domain's edits read, the change check compares the last four digits (E32), and this module
//! formats, logs and traces no field, record or key (Cardholder Data Redaction). A store error's
//! SQLSTATE and constraint never reach a message: a failed read reaches the domain only as
//! [`Lookup::Failed`], whose text the domain words, and a failed lock or rewrite only as the
//! save's failure. Each such error is logged once by [`traced`], with the paragraph and the
//! statement that failed, before it is reduced; its `Display` names an SQLSTATE and a constraint
//! or column at most, never a key or a stored value.

use domain::copybook::PicU;
use domain::online::coactupc::{
    self as rules, AccountUpdateView, CustomerRewrite, Decision, Fetched, Lookup, PendingSave,
    Snapshot, UpdateFailure, UpdateOutcome,
};
use domain::records::CustomerRecord;
use persistence::{
    AccountCustomerChain, AccountRepository, ChainLink, CustomerRepository, PersistenceError,
    PgStore, PgTx,
};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::coactupc::AccountUpdateRequest;
use crate::error::ServiceError;

/// The program the tracing event of a failed file statement names.
const PROGRAM: &str = "COACTUPC";

/// The paragraph of the save's locks and rewrites (`COACTUPC.cbl:3888-4103`).
const WRITE_PARAGRAPH: &str = "9600-WRITE-PROCESSING";

/// Endpoint 5, ENTER with an account number (COACTUPC-068): the account and customer as
/// `3202-SHOW-ORIGINAL-VALUES` shows them, with the snapshot of `9500-STORE-FETCHED-DATA`
/// (COACTUPC-074 to COACTUPC-077, COACTUPC-100).
///
/// `acct_id` is the raw path text, edited by the domain before any read (COACTUPC-045 to
/// COACTUPC-047).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the domain's failure: the account edit's 400 with no read; the
/// cross-reference's 404 or 500, after which nothing else is read; the account's and the
/// customer's 404 or 500, as the first message set stands (COACTUPC-084 to COACTUPC-099).
pub(crate) async fn fetch(
    state: &AppState,
    _user: &SignedOnUser,
    acct_id: &str,
) -> Result<AccountUpdateView, ServiceError<UpdateFailure>> {
    let key = rules::account_key(acct_id).map_err(ServiceError::Rule)?;
    let chain = read_chain(&state.store, key).await;
    rules::fetch(
        acct_id,
        lookup(&chain.xref),
        lookup(&chain.account),
        lookup(&chain.customer),
    )
    .map_err(ServiceError::Rule)
}

/// Endpoint 6, ENTER or F5 on the details shown: the change check and the edits, then, for F5 in
/// state `N` with changes that pass, the save of `9600-WRITE-PROCESSING` (COACTUPC-071,
/// COACTUPC-101 to COACTUPC-116).
///
/// `aid`, `fields` and the echoed state of `snapshot` go to `2000-DECIDE-ACTION` unchanged. The
/// domain runs `1205-COMPARE-OLD-NEW` before `1200-EDIT-MAP-INPUTS`, and runs neither in the
/// states that restart the screen, so this service calls its decision alone and never the edits
/// on their own (COACTUPC-006, COACTUPC-043, COACTUPC-110).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the domain's failure: the answer endpoint 5 gives when the
///   re-read cannot supply the cross-reference and the customer; the no-change 400, an edit's
///   400 and the abend's 500 of the decision; the lock failures, the 409 and the rewrite
///   failures of the save (COACTUPC-101 to COACTUPC-107, COACTUPC-113 to COACTUPC-116).
/// - [`ServiceError::Store`] when the transaction cannot begin, commit or roll back, or a
///   savepoint cannot be opened or ended: a store failure with no audited text.
/// - [`ServiceError::Internal`] when the read chain and the domain disagree on whether the
///   customer was read, which [`save_context`] rules out.
pub(crate) async fn save(
    state: &AppState,
    _user: &SignedOnUser,
    acct_id: &str,
    request: AccountUpdateRequest,
) -> Result<UpdateOutcome, ServiceError<UpdateFailure>> {
    let AccountUpdateRequest {
        aid,
        fields,
        snapshot,
    } = request;
    let old = Snapshot::from(&snapshot);
    let context = save_context(&state.store, acct_id).await?;
    let decision = rules::decide(
        aid.into(),
        &fields.input(acct_id),
        &old,
        &context.fetched,
        &*state.clock,
    )
    .map_err(ServiceError::Rule)?;
    match decision {
        Decision::Respond(outcome) => Ok(outcome),
        Decision::Save(pending) => {
            write_changes(&state.store, &pending, &old, context.customer_key).await
        }
    }
}

/// What endpoint 6 takes from the server-side re-read: the customer key the save locks and the
/// stored SSN the decision compares and edits.
struct SaveContext {
    /// `CDEMO-CUST-ID`: the cross-reference's customer ID, never a client value (COACTUPC-090,
    /// COACTUPC-112).
    customer_key: PicU<9>,
    /// The stored SSN of that customer (COACTUPC-044, COACTUPC-064).
    fetched: Fetched,
}

/// The re-read of endpoint 6: the chain of `9000-READ-ACCT` for the path account.
///
/// The COBOL holds the customer ID and the SSN it showed from the fetch; the replacement reads
/// them again, from the cross-reference row of the path account (COACTUPC-112, X-COACTUPC-1). A
/// found cross-reference and customer are enough to decide, whatever the account read returned:
/// after a fetch whose account read failed and whose customer was found, the screen takes
/// changes and the save then fails at the account lock (COACTUPC-099, COACTUPC-101).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the answer endpoint 5 gives for the same chain when the path
/// account fails its edit or the chain yields no cross-reference or no customer (COACTUPC-045 to
/// COACTUPC-098). The COBOL reads nothing again at this point, so no row words that answer: the
/// decision needs the stored SSN and the save needs the customer key, and without them only the
/// read's own message is the program's.
async fn save_context(
    store: &PgStore,
    acct_id: &str,
) -> Result<SaveContext, ServiceError<UpdateFailure>> {
    let key = rules::account_key(acct_id).map_err(ServiceError::Rule)?;
    let chain = read_chain(store, key).await;
    if let (ChainLink::Found(xref), ChainLink::Found(customer)) = (&chain.xref, &chain.customer) {
        return Ok(SaveContext {
            customer_key: xref.cust_id,
            fetched: Fetched::new(customer),
        });
    }
    match rules::fetch(
        acct_id,
        lookup(&chain.xref),
        lookup(&chain.account),
        lookup(&chain.customer),
    ) {
        Err(failure) => Err(ServiceError::Rule(failure)),
        // The domain builds a view only from a found cross-reference and customer, which the
        // branch above has taken.
        Ok(_) => Err(ServiceError::Internal),
    }
}

/// The three reads of `9000-READ-ACCT` for `acct_id`, each as its own link (COACTUPC-084).
///
/// They run on the pool, each statement on its own, as the plain `READ`s of the fetch hold no
/// lock. Each read that failed is logged here, once, for the fetch and the save's re-read alike.
async fn read_chain(store: &PgStore, acct_id: PicU<11>) -> AccountCustomerChain {
    let mut store = store.clone();
    let chain = AccountRepository::find_with_customer(&mut store, acct_id).await;
    report_failed_reads(&chain);
    chain
}

/// Logs each read of `chain` that failed through [`traced`], with its paragraph and dataset;
/// a read that found its record or none, or was never issued, logs nothing (COACTUPC-087,
/// COACTUPC-089, COACTUPC-093, COACTUPC-097).
fn report_failed_reads(chain: &AccountCustomerChain) {
    report_failed(&chain.xref, "9200-GETCARDXREF-BYACCT", "READ CXACAIX");
    report_failed(&chain.account, "9300-GETACCTDATA-BYACCT", "READ ACCTDAT");
    report_failed(&chain.customer, "9400-GETCUSTDATA-BYCUST", "READ CUSTDAT");
}

/// Logs `link` through [`traced`] when its read failed.
fn report_failed<T>(link: &ChainLink<T>, paragraph: &'static str, statement: &'static str) {
    if let ChainLink::Failed(error) = link {
        traced(paragraph, statement, error);
    }
}

/// A link of the read chain as the domain's lookup (COACTUPC-084).
///
/// A failed read keeps only the fact that it failed, its error already logged by
/// [`read_chain`]: the domain words the `File Error:` text of COACTUPC-087, COACTUPC-093 and
/// COACTUPC-097, and the store error's SQLSTATE and constraint stay out of it. The `DUPKEY` of
/// two cross-reference rows for one account is such a failure (COACTUPC-089).
fn lookup<T>(link: &ChainLink<T>) -> Lookup<&T> {
    match link {
        ChainLink::Found(record) => Lookup::Found(record),
        ChainLink::NotFound => Lookup::NotFound,
        ChainLink::Failed(_) => Lookup::Failed,
        ChainLink::NotReached => Lookup::NotReached,
    }
}

/// `9600-WRITE-PROCESSING` for validated changes F5 confirmed, in one transaction: lock the
/// account and the customer, check both against `old`, rewrite the account, then the customer
/// (COACTUPC-101 to COACTUPC-116).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the save's audited failure, after the transaction is rolled back,
/// except the customer rewrite's failure, which keeps the account rewrite ([`keep_account`]):
///
/// - `pending.account_lock_failed()` when the account read for update finds no row or fails
///   (COACTUPC-101);
/// - `pending.customer_lock_failed()` when the customer read for update finds no row or fails
///   (COACTUPC-102);
/// - the 409 of `9700-CHECK-CHANGE-IN-REC` (COACTUPC-113 to COACTUPC-116);
/// - `pending.rewrite_failed()` when a value cannot be stored or the account rewrite finds no
///   row or fails (COACTUPC-106, COACTUPC-111).
///
/// [`ServiceError::Store`] when the transaction cannot begin, roll back or commit.
async fn write_changes(
    store: &PgStore,
    pending: &PendingSave,
    old: &Snapshot,
    customer_key: PicU<9>,
) -> Result<UpdateOutcome, ServiceError<UpdateFailure>> {
    let mut tx = store.begin().await?;
    let account_read = AccountRepository::lock_by_id(&mut tx, pending.account_key()).await;
    let Some(account) = held(account_read, "READ ACCTDAT UPDATE") else {
        return abandon(tx, pending.account_lock_failed()).await;
    };
    let customer_read = CustomerRepository::lock_by_id(&mut tx, customer_key).await;
    let Some(customer) = held(customer_read, "READ CUSTDAT UPDATE") else {
        return abandon(tx, pending.customer_lock_failed()).await;
    };
    if let Err(changed) = rules::check_change_in_rec(old, (&account, &customer)) {
        return abandon(tx, changed).await;
    }
    let effects = match pending.save_effects(&account, &customer) {
        Ok(effects) => effects,
        Err(unwritable) => return abandon(tx, unwritable).await,
    };
    let (account, customer) = effects.into_parts();
    let account_rewrite = AccountRepository::update(&mut tx, &account).await;
    if !rewritten(account_rewrite, "REWRITE ACCTDAT") {
        return abandon(tx, pending.rewrite_failed()).await;
    }
    match customer {
        CustomerRewrite::Record(record) => rewrite_customer(tx, pending, &record).await,
        CustomerRewrite::KeyChanged => keep_account(tx, pending).await,
    }
}

/// `REWRITE CUSTDAT` after the account rewrite succeeded: success commits both rewrites
/// (COACTUPC-104, COACTUPC-108).
///
/// The rewrite runs inside a savepoint, because a failed statement aborts a `PostgreSQL`
/// transaction and the account rewrite must outlive the customer's failure ([`keep_account`]).
///
/// # Errors
///
/// [`ServiceError::Rule`] with `pending.rewrite_failed()` when the rewrite finds no row or fails,
/// after rolling back to the savepoint and committing the account rewrite (COACTUPC-107);
/// [`ServiceError::Store`] when the savepoint cannot be opened, released or rolled back, or the
/// transaction cannot commit.
async fn rewrite_customer(
    mut tx: PgTx<'_>,
    pending: &PendingSave,
    record: &CustomerRecord,
) -> Result<UpdateOutcome, ServiceError<UpdateFailure>> {
    let mut savepoint = tx.savepoint().await?;
    let customer_rewrite = CustomerRepository::update(&mut savepoint, record).await;
    if rewritten(customer_rewrite, "REWRITE CUSTDAT") {
        savepoint.commit().await?;
        tx.commit().await?;
        return Ok(pending.committed());
    }
    savepoint.rollback().await?;
    keep_account(tx, pending).await
}

/// The customer was not rewritten after the account was: commit the account rewrite alone and
/// answer `Update of record failed`, state `F` (COACTUPC-107). This also ends a save whose
/// received customer ID is not the key read for update, which CICS refuses to rewrite
/// (COACTUPC-112).
///
/// The COBOL issues `SYNCPOINT ROLLBACK` here, but `ACCTDAT` and `CUSTDAT` are defined
/// `RECOVERY(NONE)`, so that rollback backs out nothing and the account keeps its new values
/// (X-COACTUPC-7). The audited result is reproduced (Behaviour constraint), so this commits the
/// account rewrite instead of rolling it back as the statement's intent reads.
///
/// # Errors
///
/// [`ServiceError::Rule`] with `pending.rewrite_failed()` once the commit succeeded;
/// [`ServiceError::Store`] when the commit fails.
async fn keep_account(
    tx: PgTx<'_>,
    pending: &PendingSave,
) -> Result<UpdateOutcome, ServiceError<UpdateFailure>> {
    tx.commit().await?;
    Err(ServiceError::Rule(pending.rewrite_failed()))
}

/// Ends a save that writes nothing: rolls `tx` back, which also releases its row locks, and
/// answers `failure` (COACTUPC-101, COACTUPC-102, COACTUPC-106, COACTUPC-113).
///
/// # Errors
///
/// [`ServiceError::Rule`] with `failure` once the rollback succeeded; [`ServiceError::Store`]
/// when the rollback fails.
async fn abandon(
    tx: PgTx<'_>,
    failure: UpdateFailure,
) -> Result<UpdateOutcome, ServiceError<UpdateFailure>> {
    tx.rollback().await?;
    Err(ServiceError::Rule(failure))
}

/// The row a `READ … UPDATE` of `9600-WRITE-PROCESSING` holds, or `None` when the read found no
/// row or failed, which the save answers alike (COACTUPC-101, COACTUPC-102). A failure is logged
/// first through [`traced`], naming `statement`.
fn held<T>(read: Result<Option<T>, PersistenceError>, statement: &'static str) -> Option<T> {
    read.unwrap_or_else(|error| {
        traced(WRITE_PARAGRAPH, statement, &error);
        None
    })
}

/// Whether a `REWRITE` of `9600-WRITE-PROCESSING` found its row and stored it; a rewrite that
/// finds no row and one that fails are both `false`, which the save answers alike
/// (COACTUPC-106, COACTUPC-107, COACTUPC-111). A failure is logged first through [`traced`],
/// naming `statement`.
fn rewritten(result: Result<bool, PersistenceError>, statement: &'static str) -> bool {
    result.unwrap_or_else(|error| {
        traced(WRITE_PARAGRAPH, statement, &error);
        false
    })
}

/// The tracing event of a failed file statement, in place of the `RESP` and `RESP2` codes the
/// COBOL puts in its `File Error:` text or drops (`HALTED(H4)`).
///
/// `paragraph` and `statement` are constants of this module; the error's `Display` holds only
/// an SQLSTATE and a constraint or column name, never a key or a stored value (Cardholder Data
/// Redaction).
fn traced(paragraph: &'static str, statement: &'static str, error: &PersistenceError) {
    tracing::warn!(
        program = PROGRAM,
        paragraph,
        statement,
        error = %error,
        "file statement failed; answering the program's message"
    );
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use domain::CardNumber;
    use domain::copybook::{PicU, PicX};
    use domain::records::card_xref::CardXrefRecord;
    use persistence::{AccountCustomerChain, ChainLink, PersistenceError};
    use test_support::trace_capture;

    use super::{held, report_failed_reads, rewritten};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The parts every event of a failed statement carries.
    fn event_parts(paragraph: &str, statement: &str) -> [String; 4] {
        [
            r#""level":"WARN""#.to_owned(),
            r#""program":"COACTUPC""#.to_owned(),
            format!(r#""paragraph":"{paragraph}""#),
            format!(r#""statement":"{statement}""#),
        ]
    }

    /// A store error carrying an SQLSTATE and a constraint name.
    fn store_error() -> PersistenceError {
        PersistenceError::Database {
            sqlstate: Some("40P01".to_owned()),
            constraint: Some("customers_pkey".to_owned()),
        }
    }

    /// COACTUPC-087, COACTUPC-089, COACTUPC-093, COACTUPC-097: each failed read of the chain is
    /// logged once with its paragraph and the store error; a found, missing or unissued read
    /// logs nothing, and no event holds a key or a stored value.
    #[test]
    fn each_failed_chain_read_is_logged_once_without_any_value() -> TestResult {
        let xref = CardXrefRecord {
            card_num: CardNumber::parse("4111111111111111")?,
            cust_id: PicU::new(7)?,
            acct_id: PicU::new(42)?,
            filler: PicX::spaces(),
        };
        let duplicate = PersistenceError::Duplicate {
            constraint: Some("card_xref_acct_id_idx".to_owned()),
        };
        let traces = trace_capture::capture();
        let cases = [
            (
                AccountCustomerChain {
                    xref: ChainLink::Failed(duplicate),
                    account: ChainLink::NotReached,
                    customer: ChainLink::NotReached,
                },
                &[(
                    "9200-GETCARDXREF-BYACCT",
                    "READ CXACAIX",
                    "duplicate key (constraint card_xref_acct_id_idx)",
                )][..],
            ),
            (
                AccountCustomerChain {
                    xref: ChainLink::Found(xref.clone()),
                    account: ChainLink::Failed(store_error()),
                    customer: ChainLink::Failed(PersistenceError::Unavailable),
                },
                &[
                    ("9300-GETACCTDATA-BYACCT", "READ ACCTDAT", "SQLSTATE 40P01"),
                    (
                        "9400-GETCUSTDATA-BYCUST",
                        "READ CUSTDAT",
                        "database unavailable",
                    ),
                ][..],
            ),
            (
                AccountCustomerChain {
                    xref: ChainLink::Found(xref),
                    account: ChainLink::NotFound,
                    customer: ChainLink::NotFound,
                },
                &[][..],
            ),
            (
                AccountCustomerChain {
                    xref: ChainLink::NotFound,
                    account: ChainLink::NotReached,
                    customer: ChainLink::NotReached,
                },
                &[][..],
            ),
        ];
        for (chain, logged) in cases {
            traces.clear();
            report_failed_reads(&chain);
            let lines = traces.lines();
            assert_eq!(lines.len(), logged.len(), "{}", traces.text());
            for (line, &(paragraph, statement, cause)) in lines.iter().zip(logged) {
                assert!(line.contains(cause), "{cause} missing from {line}");
                for expected in event_parts(paragraph, statement) {
                    assert!(line.contains(&expected), "{expected} missing from {line}");
                }
            }
        }
        for value in ["4111111111111111", "00000000042", "000000007"] {
            assert!(!traces.contains(value), "the trace holds {value}");
        }
        Ok(())
    }

    /// COACTUPC-101, COACTUPC-102: a lock that finds no row and one that fails both end the
    /// save, and only the failure is logged.
    #[test]
    fn a_lock_that_fails_is_logged_and_answers_as_a_missing_row() {
        let traces = trace_capture::capture();
        for statement in ["READ ACCTDAT UPDATE", "READ CUSTDAT UPDATE"] {
            let cases = [
                (Ok(Some(7_u8)), Some(7_u8), false),
                (Ok(None), None, false),
                (Err(store_error()), None, true),
            ];
            for (read, expected, logs) in cases {
                traces.clear();
                assert_eq!(held(read, statement), expected, "{statement}");
                let lines = traces.lines();
                assert_eq!(lines.len(), usize::from(logs), "{}", traces.text());
                for line in &lines {
                    assert!(line.contains("SQLSTATE 40P01"), "{line}");
                    for part in event_parts("9600-WRITE-PROCESSING", statement) {
                        assert!(line.contains(&part), "{part} missing from {line}");
                    }
                }
            }
        }
    }

    /// COACTUPC-106, COACTUPC-107, COACTUPC-111: a rewrite that finds no row and one that fails
    /// are both not rewritten, and only the failure is logged.
    #[test]
    fn a_rewrite_that_fails_is_logged_and_answers_as_not_rewritten() {
        let traces = trace_capture::capture();
        for statement in ["REWRITE ACCTDAT", "REWRITE CUSTDAT"] {
            let cases = [
                (Ok(true), true, false),
                (Ok(false), false, false),
                (Err(PersistenceError::Unavailable), false, true),
            ];
            for (result, expected, logs) in cases {
                traces.clear();
                assert_eq!(rewritten(result, statement), expected, "{statement}");
                let lines = traces.lines();
                assert_eq!(lines.len(), usize::from(logs), "{}", traces.text());
                for line in &lines {
                    assert!(line.contains("database unavailable"), "{line}");
                    for part in event_parts("9600-WRITE-PROCESSING", statement) {
                        assert!(line.contains(&part), "{part} missing from {line}");
                    }
                }
            }
        }
    }
}
