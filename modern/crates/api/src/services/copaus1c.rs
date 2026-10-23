//! The service of transaction `CPVD`, program COPAUS1C and its `LINK` target COPAUS2C: endpoint
//! 22, `GET /api/v1/authorizations/{acct_id}/{auth_key}` (ENTER and F8), and endpoint 23,
//! `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud` (F5; AAP 0.3.4.1).
//!
//! Only `api::handlers::copaus1c` calls it. It performs the reads and writes COPAUS1C and
//! COPAUS2C issue, through the repository traits of `persistence`, and hands each outcome to
//! `domain::online::copaus1c` and `domain::online::copaus2c`, which hold every message literal
//! and every decision (Thin Request Handlers).
//!
//! # Endpoint 22
//!
//! - Both path segments go through `copaus1c::edit` before any read: an account that is not 11
//!   digits or a blank key is 400, a key that names no authorization 404 (COPAUS1C-006,
//!   COPAUS1C-009).
//! - [`read_current`] is `READ-AUTH-RECORD`: the `GU` of the summary by `ACCNTID`, then the `GNP`
//!   of the detail by `PAUT9CTS`, on the pool (COPAUS1C-008 to COPAUS1C-011). Neither COBOL call
//!   holds the segment, so neither read takes a lock, and endpoint 22 answers while an F5 holds
//!   the detail.
//! - No `direction` is ENTER, the detail of the path's key (COPAUS1C-008). `forward` is F8,
//!   `PROCESS-PF8-KEY`: after the current read, [`read_next`] is `READ-NEXT-AUTH-RECORD`, the next
//!   detail of the account in twin order (COPAUS1C-024 to COPAUS1C-026). `backward` is no key of
//!   `COPAU1A` and takes ENTER's processing, as `WHEN OTHER` does; the invalid-key message is the
//!   screen's (COPAUS1C-005).
//!
//! # Endpoint 23
//!
//! `MARK-AUTH-FRAUD` and COPAUS2C run in one `PostgreSQL` transaction, the unit of work that
//! COPAUS1C's `TAKE-SYNCPOINT` commits and `ROLL-BACK` backs out, and which COPAUS2C's statements
//! join (COPAUS1C-030, COPAUS2C-012):
//!
//! 1. [`read_current`] on the transaction, its detail read `SELECT … FOR UPDATE`, then
//!    `copaus1c::toggle` flips the fraud flag and `FraudToggle::with_report_date` stamps
//!    COPAUS2C's `MM/DD/YY` report date into the record the replace writes back (COPAUS1C-028,
//!    COPAUS2C-001).
//! 2. [`read_customer`] supplies `WS-FRD-CUST-ID`, the customer COPAUS0C left in the COMMAREA
//!    (COPAUS1C-030, X-COPAUS1C-4).
//! 3. `copaus2c::fraud_row` builds the `AUTHFRDS` row and [`record_fraud`] inserts it inside a
//!    savepoint, falling back to `FRAUD-UPDATE` on the duplicate key, `SQLCODE -803`
//!    (COPAUS2C-006 to COPAUS2C-011).
//! 4. Only after COPAUS2C's success does `UPDATE-AUTH-DETAILS` replace the detail row
//!    (COPAUS1C-032, COPAUS1C-033), and `copaus1c::toggle_outcome` decides between commit and
//!    rollback (COPAUS1C-031 to COPAUS1C-034).
//!
//! A read that misses or fails stops both endpoints with that read's outcome, and nothing is
//! written (X-COPAUS1C-6).
//!
//! ## Concurrent F5s
//!
//! The flag the stored detail holds decides each toggle (COPAUS1C-028), and the replace follows
//! a read that IMS requires to be a get-hold call, which holds the segment until the syncpoint
//! (X-COPAUS1C-1, X-COPAUS1C-H2). Endpoint 23 therefore holds the detail row from its read to
//! its commit or rollback, so concurrent F5s on one authorization run one after the other. A
//! waiting transaction's read returns the row as the transaction before it committed it, since
//! under `READ COMMITTED` a `FOR UPDATE` that waited re-reads the row it locks; each committed
//! toggle thus flips the committed flag exactly once and reports that flip. The summary read
//! stays plain, as endpoint 23 never writes the summary. Only the database serialises: no
//! process-local lock guards the detail, so a second API process waits on the same row. The
//! request pool (`runtime::db::connect_for_requests`) adds no `lock_timeout` or
//! `statement_timeout`: its sessions carry only the server options `DATABASE_URL` names, so by
//! default an F5 that meets the held detail waits until the holding transaction ends, then reads
//! the row it left and goes on. Only a `lock_timeout` or `statement_timeout` set in
//! `DATABASE_URL`, as `options[lock_timeout]=…`, or in the server's configuration ends the wait
//! sooner; the detail read then fails, with COPAUS1C-011's text and nothing written.
//!
//! # Errors and halts
//!
//! A store error the program answers with a text of its own becomes the domain's lookup or result
//! value, never `ServiceError::Store`: the IMS status those texts carry is `HALTED(H4)`
//! (COPAUS1C-038) and the Db2 codes of COPAUS2C's texts are `HALTED(H3)` (COPAUS2C-010,
//! COPAUS2C-011). Such an error is recorded as a tracing event that holds only its SQLSTATE and
//! constraint name. `ServiceError::Store` is left for the transaction calls themselves: `begin`,
//! `savepoint`, `commit` and `rollback`, which have no COBOL message.
//!
//! # Cardholder data and totality
//!
//! The card number stays a `CardNumber` from the detail row to the fraud row and the response, so
//! every rendering of it is redacted; it reaches SQL only through the repositories, and no event
//! here records a record, a row or a key (Cardholder Data Redaction, COPAUS1C-012,
//! COPAUS2C-013). Path text of any length goes through the total domain edits, and the account
//! keys the authorization tables only through `PicS::<11>::from` (Total Parsers Over Untrusted
//! Input, Checked Integer Narrowing).

use chrono::NaiveDate;
use domain::Clock;
use domain::copybook::{PicS, PicU};
use domain::online::copaus0c::{AuthKey, Lookup};
use domain::online::copaus1c::{
    self as rules, AuthDetailFailure, AuthDetailView, DetailKey, DetailLookup, FraudToggle,
    FraudToggleResult as ToggleOutcome, NextLookup, SegmentUpdate,
};
use domain::online::copaus2c::{
    self as fraud, AuthFraudRow, FraudOutcome, InsertResult, UpdateResult,
};
use domain::records::PendingAuthDetailRecord;
use persistence::{
    AuthFraudRepository, CardXrefRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PersistenceError, PgTx,
};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::PageDirection;
use crate::dto::copaus1c::AuthDetailQuery;
use crate::error::ServiceError;

/// The rows `READ-NEXT-AUTH-RECORD` asks the inclusive twin-order read for: the current detail,
/// when it is still stored, and the one after it (COPAUS1C-024).
const NEXT_READ_LIMIT: i64 = 2;

/// Endpoint 22: ENTER, or F8 when `query.direction` is `forward`; the authorization the path
/// names, or the one after it, as `POPULATE-AUTH-DETAILS` shows it (COPAUS1C-006, COPAUS1C-008
/// to COPAUS1C-027).
///
/// The caller's identity is not read: any signed-on user may view an authorization, as CPVD
/// checks no user type.
///
/// # Errors
///
/// [`ServiceError::Rule`] with the [`AuthDetailFailure`] of the edit or of the failed or missing
/// read: 400 for a malformed account or a blank key (COPAUS1C-006), 404 for an authorization that
/// is not stored (COPAUS1C-009), 500 with the summary, detail or next-read text (COPAUS1C-010,
/// COPAUS1C-011, COPAUS1C-026).
pub(crate) async fn detail(
    state: &AppState,
    _user: &SignedOnUser,
    acct_id: &str,
    auth_key: &str,
    query: AuthDetailQuery,
) -> Result<AuthDetailView, ServiceError<AuthDetailFailure>> {
    let key = rules::edit(acct_id, auth_key).map_err(ServiceError::Rule)?;
    let mut store = state.store.clone();
    let current = read_current(&mut store, key, DetailHold::None).await;
    let view = match query.direction {
        Some(PageDirection::Forward) => match current.lookup().record() {
            Ok(record) => {
                let following = read_next(&mut store, key).await;
                rules::next(key.acct_id(), record, following.lookup())
            }
            // F8 stops at a missed or failed current read and never reads on (COPAUS1C-027,
            // X-COPAUS1C-6).
            Err(failure) => Err(failure),
        },
        // `backward` is no key of `COPAU1A`: `WHEN OTHER` performs ENTER's processing, and the
        // screen shows the invalid-key message itself (COPAUS1C-005).
        Some(PageDirection::Backward) | None => rules::detail(key.acct_id(), current.lookup()),
    };
    view.map_err(ServiceError::Rule)
}

/// Endpoint 23, F5: `MARK-AUTH-FRAUD`, COPAUS2C's `AUTHFRDS` write and the replace of the detail,
/// committed together or rolled back together (COPAUS1C-028 to COPAUS1C-034, COPAUS2C-001 to
/// COPAUS2C-012).
///
/// The `Ok` value is always the committed outcome, carrying `AUTH MARKED FRAUD...` or
/// `AUTH FRAUD REMOVED...` and the detail as replaced, its key unchanged (COPAUS1C-032,
/// COPAUS1C-034). A rolled-back outcome is returned as its failure, so no 200 carries one.
///
/// # Errors
///
/// - [`ServiceError::Rule`]: the failure of the edit or of a missed or failed read, 400, 404 or
///   500, with nothing written (COPAUS1C-006, COPAUS1C-009 to COPAUS1C-011, X-COPAUS1C-4,
///   X-COPAUS1C-6); COPAUS2C's 50-byte failure text after a failed insert or update, rolled back
///   (COPAUS1C-031); or the fraud-tagging text after a failed replace, rolled back
///   (COPAUS1C-033).
/// - [`ServiceError::Store`]: `begin`, a savepoint call, `commit` or `rollback` failed; the
///   transaction is rolled back or discarded with its connection.
pub(crate) async fn toggle_fraud(
    state: &AppState,
    _user: &SignedOnUser,
    acct_id: &str,
    auth_key: &str,
) -> Result<ToggleOutcome, ServiceError<AuthDetailFailure>> {
    let key = rules::edit(acct_id, auth_key).map_err(ServiceError::Rule)?;
    let mut tx = state.store.begin().await?;
    match mark_fraud(&mut tx, &*state.clock, key).await {
        // `TAKE-SYNCPOINT`: the fraud row and the replaced detail commit together (COPAUS1C-032).
        Ok(ToggleOutcome::Committed(view)) => {
            tx.commit().await?;
            Ok(ToggleOutcome::Committed(view))
        }
        // `ROLL-BACK`: neither write remains (COPAUS1C-031, COPAUS1C-033).
        Ok(ToggleOutcome::RolledBack(failure)) => {
            tx.rollback().await?;
            Err(ServiceError::Rule(failure))
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

/// F5 inside the open transaction `tx`, up to COPAUS1C's decision: the reads, the detail held
/// until the caller commits or rolls back, the toggle, COPAUS2C's insert or update and, after its
/// success only, the replace (COPAUS1C-028 to COPAUS1C-033). The caller commits or rolls back on
/// the outcome returned.
///
/// # Errors
///
/// [`ServiceError::Rule`] for a missed or failed detail or cross-reference read, before any write
/// (X-COPAUS1C-4, X-COPAUS1C-6); [`ServiceError::Store`] for a failed savepoint call.
async fn mark_fraud(
    tx: &mut PgTx<'_>,
    clock: &dyn Clock,
    key: DetailKey,
) -> Result<ToggleOutcome, ServiceError<AuthDetailFailure>> {
    // The flag the detail holds decides the toggle (COPAUS1C-028), so the detail is held from
    // this read to the commit or rollback: a concurrent F5 on it waits here and then reads the
    // flag this one committed.
    let current = read_current(tx, key, DetailHold::UntilCommit).await;
    let detail = current.lookup().record().map_err(ServiceError::Rule)?;
    // COPAUS2C moves its report date into the COMMAREA copy of the detail, the copy COPAUS1C
    // writes back, so the replaced segment carries it (COPAUS1C-032, COPAUS2C-001).
    let toggle = rules::toggle(detail).with_report_date(fraud::report_date(clock));
    let customer = read_customer(tx, key.acct_id()).await;
    let cust_id = rules::fraud_customer(customer).map_err(ServiceError::Rule)?;
    let outcome = match fraud::fraud_row(
        &toggle.updated,
        toggle.action,
        key.acct_id(),
        cust_id,
        clock,
    ) {
        // Db2's `CURRENT DATE`, taken from the injected clock (COPAUS2C-007, COPAUS2C-008).
        Ok(row) => record_fraud(tx, &row, clock.now().date()).await?,
        // A row whose `AUTH_TS` names no timestamp takes the insert-failure path before any
        // statement runs (COPAUS2C-005, COPAUS2C-010).
        Err(error) => FraudOutcome::from(error),
    };
    // `UPDATE-AUTH-DETAILS` runs only after `WS-FRD-UPDT-SUCCESS` (COPAUS1C-031, COPAUS1C-032).
    let segment_update = if outcome.is_success() {
        Some(replace_detail(tx, key, &toggle).await)
    } else {
        None
    };
    Ok(rules::toggle_outcome(
        key.acct_id(),
        &toggle,
        outcome,
        segment_update,
    ))
}

/// What `READ-AUTH-RECORD` read, owned so that the [`DetailLookup`] the domain takes can borrow
/// the detail (COPAUS1C-008 to COPAUS1C-011).
enum CurrentRead {
    /// Both reads found their row, boxed because a detail is far larger than the other variants.
    Found(Box<PendingAuthDetailRecord>),
    /// The summary or the detail is not stored, IMS `GE`.
    NotFound,
    /// The summary read failed.
    SummaryError,
    /// The summary was found and the detail read failed.
    DetailError,
}

impl CurrentRead {
    /// The read as the domain's lookup.
    fn lookup(&self) -> DetailLookup<'_> {
        match self {
            Self::Found(record) => DetailLookup::Found(record.as_ref()),
            Self::NotFound => DetailLookup::NotFound,
            Self::SummaryError => DetailLookup::SummaryError,
            Self::DetailError => DetailLookup::DetailError,
        }
    }
}

/// How [`read_current`] reads the detail.
#[derive(Clone, Copy)]
enum DetailHold {
    /// A plain read, which takes no lock: endpoint 22, whose `GU` and `GNP` hold nothing
    /// (COPAUS1C-008).
    None,
    /// `SELECT … FOR UPDATE` inside endpoint 23's transaction, the row held until its commit or
    /// rollback: the read the `REPL` of `UPDATE-AUTH-DETAILS` follows, held as the get-hold call
    /// IMS requires before a `REPL` (COPAUS1C-028, X-COPAUS1C-1).
    UntilCommit,
}

/// `READ-AUTH-RECORD` on `store`, the pool for endpoint 22 and the transaction for endpoint 23:
/// `GU PAUTSUM0 WHERE (ACCNTID = ..)`, then, only when the summary is found, `GNP PAUTDTL1 WHERE
/// (PAUT9CTS = ..)` (`COPAUS1C.cbl:431-491`; COPAUS1C-008 to COPAUS1C-011). The summary read
/// never takes a lock, and `hold` says whether the detail read does.
///
/// The COBOL issues the detail call after a failed summary call too; the replacement stops at
/// the summary (X-COPAUS1C-6). A held read whose row another transaction holds waits until that
/// transaction ends and then reads the row it left, as the request pool adds no `lock_timeout`
/// or `statement_timeout`. Only one set in `DATABASE_URL`, as `options[lock_timeout]=…`, or in
/// the server's configuration ends the wait sooner, and the held read then fails like any other
/// detail read.
async fn read_current<E>(store: &mut E, key: DetailKey, hold: DetailHold) -> CurrentRead
where
    E: PendingAuthSummaryRepository + PendingAuthDetailRepository,
{
    let acct = PicS::<11>::from(key.acct_id());
    match PendingAuthSummaryRepository::find_by_id(store, acct).await {
        Ok(Some(_)) => {}
        Ok(None) => return CurrentRead::NotFound,
        Err(error) => {
            note_store_error("pending_auth_summaries find_by_id", &error);
            return CurrentRead::SummaryError;
        }
    }
    let auth_key = key.auth_key();
    let (date_9c, time_9c) = (auth_key.date_9c(), auth_key.time_9c());
    let (read, statement) = match hold {
        DetailHold::None => (
            PendingAuthDetailRepository::find_by_id(store, acct, date_9c, time_9c).await,
            "pending_auth_details find_by_id",
        ),
        DetailHold::UntilCommit => (
            PendingAuthDetailRepository::lock_by_id(store, acct, date_9c, time_9c).await,
            "pending_auth_details lock_by_id",
        ),
    };
    match read {
        Ok(Some(record)) => CurrentRead::Found(Box::new(record)),
        Ok(None) => CurrentRead::NotFound,
        Err(error) => {
            note_store_error(statement, &error);
            CurrentRead::DetailError
        }
    }
}

/// What `READ-NEXT-AUTH-RECORD` read, owned so that the [`NextLookup`] the domain takes can
/// borrow the detail (COPAUS1C-024 to COPAUS1C-026).
enum NextRead {
    /// The next detail of the account, boxed as in [`CurrentRead`].
    Found(Box<PendingAuthDetailRecord>),
    /// No detail follows, IMS `GE`.
    End,
    /// The read failed.
    Error,
}

impl NextRead {
    /// The read as the domain's lookup.
    fn lookup(&self) -> NextLookup<'_> {
        match self {
            Self::Found(record) => NextLookup::Found(record.as_ref()),
            Self::End => NextLookup::End,
            Self::Error => NextLookup::Error,
        }
    }
}

/// `READ-NEXT-AUTH-RECORD`, the unqualified `GNP` after the current detail: the detail of the
/// same account with the smallest key above the current one, in IMS twin order
/// (`COPAUS1C.cbl:493-518`; COPAUS1C-024 to COPAUS1C-026).
///
/// The twin-order read starts at the current key, that key included, so it asks for two rows
/// and keeps the first whose key differs; that holds whether or not the current row is still
/// stored.
async fn read_next<E>(store: &mut E, key: DetailKey) -> NextRead
where
    E: PendingAuthDetailRepository,
{
    let current = key.auth_key();
    let read = PendingAuthDetailRepository::page_after(
        store,
        PicS::<11>::from(key.acct_id()),
        current.date_9c(),
        current.time_9c(),
        NEXT_READ_LIMIT,
    )
    .await;
    match read {
        Ok(rows) => rows
            .into_iter()
            .find(|row| AuthKey::of(row) != current)
            .map_or(NextRead::End, |row| NextRead::Found(Box::new(row))),
        Err(error) => {
            note_store_error("pending_auth_details page_after", &error);
            NextRead::Error
        }
    }
}

/// `CDEMO-CUST-ID`, the customer COPAUS0C moved into the COMMAREA from the account's
/// cross-reference, which COPAUS1C passes to COPAUS2C as `WS-FRD-CUST-ID` (`COPAUS0C.cbl:812-831`;
/// COPAUS1C-030, X-COPAUS1C-4).
///
/// Endpoint 23 carries no customer, so the service makes the read COPAUS0C makes: through the
/// account path `CXACAIX`, where exactly one row is `NORMAL` and gives its customer
/// (COPAUS0C-013), none is `NOTFND` and more than one is the `DUPKEY` COPAUS0C answers as a
/// failure (COPAUS0C-014, COPAUS0C-015).
// The first two rows of the account are read, not only the first, because a `DUPKEY` read sets
// no customer in COPAUS0C, and `Lookup::Failed` is the domain's value for it; a second row
// already shows the `DUPKEY`, so the read never takes more than two.
async fn read_customer<E>(store: &mut E, acct_id: PicU<11>) -> Lookup<PicU<9>>
where
    E: CardXrefRepository,
{
    match CardXrefRepository::find_first_two_by_acct_id(store, acct_id).await {
        Ok(rows) => match rows.as_slice() {
            [] => Lookup::NotFound,
            [xref] => Lookup::Found(xref.cust_id),
            [_, _, ..] => Lookup::Failed,
        },
        Err(error) => {
            note_store_error("card_xref find_first_two_by_acct_id", &error);
            Lookup::Failed
        }
    }
}

/// COPAUS2C's two statements for `row`, inside the transaction `tx` (`COPAUS2C.cbl:141-244`;
/// COPAUS2C-007 to COPAUS2C-011): the insert, then `FRAUD-UPDATE` exactly when the insert met the
/// duplicate key, `SQLCODE -803`. `report_date` is the `FRAUD_RPT_DATE` both statements write.
///
/// # Errors
///
/// [`PersistenceError`] when the savepoint around the insert cannot be opened, released or
/// rolled back. Every statement failure is a value of the outcome instead.
async fn record_fraud(
    tx: &mut PgTx<'_>,
    row: &AuthFraudRow,
    report_date: NaiveDate,
) -> Result<FraudOutcome, PersistenceError> {
    let insert = insert_fraud(tx, row, report_date).await?;
    let update = if insert.needs_update() {
        Some(update_fraud(tx, row, report_date).await)
    } else {
        None
    };
    Ok(fraud::fraud_outcome(insert, update))
}

/// `INSERT INTO CARDDEMO.AUTHFRDS` inside a savepoint (COPAUS2C-007, COPAUS2C-008,
/// COPAUS2C-010). A failed statement aborts a `PostgreSQL` transaction, and after the duplicate
/// key the unit of work goes on with the update, so the insert runs on a savepoint that is
/// released after a success and rolled back after any failure.
///
/// # Errors
///
/// [`PersistenceError`] when the savepoint cannot be opened, released or rolled back.
async fn insert_fraud(
    tx: &mut PgTx<'_>,
    row: &AuthFraudRow,
    report_date: NaiveDate,
) -> Result<InsertResult, PersistenceError> {
    let mut savepoint = tx.savepoint().await?;
    match AuthFraudRepository::insert(&mut savepoint, row, report_date).await {
        Ok(()) => {
            savepoint.commit().await?;
            Ok(InsertResult::Inserted)
        }
        Err(error) => {
            savepoint.rollback().await?;
            if error.is_duplicate() {
                Ok(InsertResult::Duplicate)
            } else {
                note_store_error("auth_fraud insert", &error);
                Ok(InsertResult::Failed)
            }
        }
    }
}

/// `FRAUD-UPDATE`: the row with `row`'s card number and `AUTH_TS` takes its fraud flag and the
/// report date (`COPAUS2C.cbl:221-244`; COPAUS2C-008, COPAUS2C-009, COPAUS2C-011). No row with
/// the key, Db2's `SQLCODE +100`, fails like any other code.
async fn update_fraud(
    tx: &mut PgTx<'_>,
    row: &AuthFraudRow,
    report_date: NaiveDate,
) -> UpdateResult {
    match AuthFraudRepository::update_flag(
        tx,
        &row.card_num,
        row.auth_ts,
        &row.auth_fraud,
        report_date,
    )
    .await
    {
        Ok(true) => UpdateResult::Updated,
        Ok(false) => UpdateResult::Failed,
        Err(error) => {
            note_store_error("auth_fraud update_flag", &error);
            UpdateResult::Failed
        }
    }
}

/// `UPDATE-AUTH-DETAILS`: `REPL` of the detail with the toggled record, its flipped flag and new
/// report date, inside the transaction `tx` (`COPAUS1C.cbl:520-552`; COPAUS1C-032,
/// COPAUS1C-033). A row that is no longer stored fails like a store error.
async fn replace_detail(tx: &mut PgTx<'_>, key: DetailKey, toggle: &FraudToggle) -> SegmentUpdate {
    let acct = PicS::<11>::from(key.acct_id());
    match PendingAuthDetailRepository::update(tx, acct, &toggle.updated).await {
        Ok(true) => SegmentUpdate::Replaced,
        Ok(false) => SegmentUpdate::Failed,
        Err(error) => {
            note_store_error("pending_auth_details update", &error);
            SegmentUpdate::Failed
        }
    }
}

/// Records a store error that the program answers with a text of its own, so that the error,
/// which the response does not carry, stays visible to the operator. The event holds the
/// statement's name and the error's `Display`, which is only an SQLSTATE and a constraint name:
/// never a key, a record or a row (Cardholder Data Redaction).
fn note_store_error(statement: &'static str, error: &PersistenceError) {
    tracing::error!(
        program = "COPAUS1C",
        statement,
        error = %error,
        "store error answered with the program's message"
    );
}
