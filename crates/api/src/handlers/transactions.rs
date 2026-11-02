//! The transaction resources: CT00 `GET /transactions` (`COTRN00C`, map
//! `COTRN00`), CT01 `GET /transactions/{id}` (`COTRN01C`, map `COTRN01`) and
//! CT02 `POST /transactions` (`COTRN02C`, map `COTRN02`), the CT02 add being
//! the API call site of the reward engine (AAP 0.3.1, 0.3.5).
//!
//! Each handler extracts its request, opens one unit of work, hands the
//! unit's store to the ported rule and maps the rule's result to its DTO. The
//! rules hold every COBOL edit, browse and message, and `crate::error` turns
//! each rule error into its status; nothing here tests a field or chooses a
//! text.
//!
//! # Reads
//!
//! CT00 and CT01 write nothing, so their unit of work is rolled back once the
//! rule has read. `GET /transactions` serves three callers through the one
//! browse of `transaction_list::list`: the CT00 list itself, the CT02 "copy
//! last transaction" lookup (`direction=backward&page_size=1`, the
//! highest-keyed transaction of the whole file, P-COTRN02C-026), and the
//! dashboard's latest rows (`account_id=…&direction=backward&page_size=5`).
//! The `account_id` filter is the REST resource's addition, and parity is
//! measured without it (E-17). After `batch tranbkp` the live master is empty,
//! so the list is a 200 page without items and the view answers not found
//! (E-09).
//!
//! # CT02 atomicity
//!
//! The reward accrual is mandated to commit with the transaction insert
//! (AAP 0.3.5, P-COTRN02C-024). One database transaction holds the
//! cross-reference read of the validation, the serialized TRAN-ID allocation,
//! the record write, the reward accrual and the balance update, and commits
//! only when all of them succeeded; any failure after it was opened rolls it
//! back whole, so the transaction row, its accrual and the balance update
//! persist together or not at all. This is separate from the `RECOVERY(NONE)`
//! VSAM write paths [app/csd/CARDDEMO.CSD:L1-96], which run each legacy write
//! under its own savepoint and keep completed writes when a later one fails
//! (AAP 0.6.4); the COBOL add has no write before the transaction record that
//! could persist.
//!
//! # Telemetry
//!
//! Each handler runs inside one INFO span named after its operation, with
//! `transaction` (`CT00`, `CT01`, `CT02`), the signed-on `user_id`, the
//! resource keys and `outcome`, recorded once the unit of work has finished:
//! `ok` or `rejected` for the reads, and `committed`, `rolled_back` (a failure
//! after validation) or `rejected` (the request refused before any write) for
//! CT02, which also records `reward_points` when the add accrued points. The
//! card number is never recorded.
//!
//! A key is recorded only in the form its check gave it, never as entered:
//! `account_id` is the typed account of the CT00 filter or of the validated
//! CT02 add, and `tran_id` the TRAN-ID that CT02 allocated or that the CT01
//! read found. A CT00 filter or a CT02 body that fails its check records
//! neither key. A CT01 Tran ID that found no transaction records no part of
//! its text, only `tran_id_length`, its count of characters, and
//! `tran_id_rejected`:
//!
//! | `tran_id_rejected` | The Tran ID |
//! |---|---|
//! | `blank` | Was not entered: empty, spaces or `LOW-VALUES` (V-COTRN01C-001) |
//! | `not_found` | Names no live transaction (V-COTRN01C-002) |
//! | `unread` | Was never looked up: the read failed or no unit of work could be opened |

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use carddemo_domain::clock::Clock;
use carddemo_domain::entities::Transaction;
use carddemo_domain::error::ErrorKind;
use carddemo_domain::ports::{PortError, PortResult, TransactionWriter};
use carddemo_domain::rewards::{AccrualInput, RewardEngine};
use carddemo_domain::rules::transaction_add::{self, ValidatedTransaction};
use carddemo_domain::rules::transaction_list;
use carddemo_domain::rules::transaction_view::{self, TransactionViewInput};
use carddemo_persistence::UnitOfWork;
use tracing::field::{Empty, display};
use tracing::{Instrument, Span};

use crate::auth::AuthUser;
use crate::dto::id_string;
use crate::dto::transactions::{
    TransactionAddRequest, TransactionAddResponse, TransactionListQuery, TransactionResponse,
};
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::{Page, PageQuery};
use crate::state::AppState;

/// `outcome` of a read that returned its result.
const OUTCOME_OK: &str = "ok";

/// `outcome` of a request refused before anything was written: a rejected
/// edit, a failed read, or a unit of work that could not be opened.
const OUTCOME_REJECTED: &str = "rejected";

/// `outcome` of a CT02 add whose unit of work committed.
const OUTCOME_COMMITTED: &str = "committed";

/// `outcome` of a CT02 add that failed after validation and was rolled back
/// whole.
const OUTCOME_ROLLED_BACK: &str = "rolled_back";

/// `tran_id_rejected` of a CT01 Tran ID that was not entered.
const TRAN_ID_BLANK: &str = "blank";

/// `tran_id_rejected` of a CT01 Tran ID that names no live transaction.
const TRAN_ID_NOT_FOUND: &str = "not_found";

/// `tran_id_rejected` of a CT01 Tran ID that was never looked up.
const TRAN_ID_UNREAD: &str = "unread";

/// COTRN00C PROCESS-ENTER-KEY, PROCESS-PF7-KEY, PROCESS-PF8-KEY,
/// PROCESS-PAGE-FORWARD/BACKWARD, SEND-TRNLST-SCREEN — P-COTRN00C-005 to
/// P-COTRN00C-016, P-COTRN00C-018; V-COTRN00C-001 to V-COTRN00C-013.
///
/// One page of the live transaction master in TRAN-ID order: `direction`
/// `forward` is ENTER (and PF8 from the `next_key` of the previous page),
/// `backward` is PF7 from the first key shown, or the end of the file when no
/// `start_key` is given. `page_size` defaults to the 10 rows of the screen.
/// The page-boundary texts the COBOL sends without its error flag come back in
/// `message` of the 200 page; an empty master is a page without items.
///
/// # Errors
///
/// - 400 for an `account_id` filter that is not 11 digits (V-COTRN00C-001)
///   or a `start_key` that is not 16 digits (V-COTRN00C-004), and for a query
///   that does not deserialize, such as a `page_size` outside 1 to 100.
/// - 401 without a valid bearer token.
/// - 500 when the browse fails (V-COTRN00C-006, V-COTRN00C-008,
///   V-COTRN00C-011, V-COTRN00C-013) or no unit of work can be opened.
#[utoipa::path(
    get,
    path = "/transactions",
    tag = "transactions",
    params(TransactionListQuery, PageQuery),
    responses(
        (status = 200, description = "One page of transactions in TRAN-ID order, with the page-boundary message of the list, if any", body = Page<TransactionResponse>),
        (status = 400, description = "Malformed account filter, start key or query", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 500, description = "The transaction browse failed", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn list_transactions(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(filter): ApiQuery<TransactionListQuery>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> Result<Json<Page<TransactionResponse>>, ApiError> {
    let span = tracing::info_span!(
        "list_transactions",
        transaction = "CT00",
        user_id = %user.user_id,
        account_id = Empty,
        outcome = Empty,
    );
    let result = list_page(&state, filter, page, &span)
        .instrument(span.clone())
        .await;
    span.record("outcome", read_outcome(&result));
    result.map(Json)
}

/// The CT00 page of the request: the page parameters resolved with the
/// screen's 10 rows, the account filter typed, then the browse.
async fn list_page(
    state: &AppState,
    filter: TransactionListQuery,
    page: PageQuery,
    span: &Span,
) -> Result<Page<TransactionResponse>, ApiError> {
    let (start_key, direction, page_size) = page.resolve(transaction_list::SCREEN_ROWS);
    let input = filter.into_input(start_key, direction, page_size)?;
    if let Some(account_id) = input.account_id {
        span.record("account_id", display(account_id));
    }
    let mut uow = state.db().begin().await?;
    let listed = transaction_list::list(&mut uow.store(), input).await;
    roll_back(uow).await;
    let listed = listed?;
    Ok(Page::new(
        listed.items.iter().map(TransactionResponse::from).collect(),
        listed.next_key.as_ref().map(id_string),
        listed.prev_key.as_ref().map(id_string),
        listed.message.map(str::to_owned),
    ))
}

/// COTRN01C PROCESS-ENTER-KEY, READ-TRANSACT-FILE, SEND-TRNVIEW-SCREEN —
/// P-COTRN01C-003 to P-COTRN01C-005, P-COTRN01C-007; V-COTRN01C-001 to
/// V-COTRN01C-003.
///
/// The live transaction whose TRAN-ID is the path segment, moved into the
/// 16-byte key as entered, with every `CVTRA05Y` field.
///
/// # Errors
///
/// - 400 for a blank Tran ID (V-COTRN01C-001).
/// - 401 without a valid bearer token.
/// - 404 when no live transaction has the Tran ID, every Tran ID after
///   `batch tranbkp` included (V-COTRN01C-002).
/// - 500 when the read fails (V-COTRN01C-003) or no unit of work can be
///   opened.
#[utoipa::path(
    get,
    path = "/transactions/{id}",
    tag = "transactions",
    params(("id" = String, Path, description = "The Tran ID, 16 digits")),
    responses(
        (status = 200, description = "The transaction", body = TransactionResponse),
        (status = 400, description = "Blank Tran ID", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "No live transaction has the Tran ID", body = ErrorBody),
        (status = 500, description = "The transaction read failed", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn get_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(tran_id): ApiPath<String>,
) -> Result<Json<TransactionResponse>, ApiError> {
    let span = tracing::info_span!(
        "get_transaction",
        transaction = "CT01",
        user_id = %user.user_id,
        tran_id = Empty,
        tran_id_rejected = Empty,
        tran_id_length = Empty,
        outcome = Empty,
    );
    let entered_length = tran_id.chars().count();
    let result = view_one(&state, tran_id).instrument(span.clone()).await;
    record_tran_id(&span, &result, entered_length);
    span.record("outcome", read_outcome(&result));
    result.map(Json)
}

/// Records the Tran ID of the CT01 `result` on `span`: the TRAN-ID of the
/// transaction read, or, when none was, only the category of the failure and
/// `entered_length`, the entered Tran ID's count of characters.
fn record_tran_id(
    span: &Span,
    result: &Result<TransactionResponse, ApiError>,
    entered_length: usize,
) {
    match result {
        Ok(found) => {
            span.record("tran_id", found.tran_id.as_str());
        }
        Err(error) => {
            span.record("tran_id_rejected", tran_id_rejection(error))
                .record("tran_id_length", entered_length);
        }
    }
}

/// The `tran_id_rejected` category of the CT01 `error`, by its code, which
/// `crate::error` takes from the error's kind: [`TRAN_ID_BLANK`] for the
/// `EMPTY` blank Tran ID, [`TRAN_ID_NOT_FOUND`] for `NOT_FOUND`, and
/// [`TRAN_ID_UNREAD`] for any other failure.
fn tran_id_rejection(error: &ApiError) -> &'static str {
    let code = error.body().code.as_str();
    if code == ErrorKind::Empty.code() {
        TRAN_ID_BLANK
    } else if code == ErrorKind::NotFound.code() {
        TRAN_ID_NOT_FOUND
    } else {
        TRAN_ID_UNREAD
    }
}

/// The CT01 view of `tran_id`.
async fn view_one(state: &AppState, tran_id: String) -> Result<TransactionResponse, ApiError> {
    let mut uow = state.db().begin().await?;
    let input = TransactionViewInput {
        tran_id: Some(tran_id),
    };
    let viewed = transaction_view::view(&mut uow.store(), input).await;
    roll_back(uow).await;
    Ok(TransactionResponse::from(&viewed?))
}

/// COTRN02C PROCESS-ENTER-KEY, ADD-TRANSACTION, WRITE-TRANSACT-FILE —
/// P-COTRN02C-003 to P-COTRN02C-025; V-COTRN02C-001 to V-COTRN02C-033;
/// `RewardEngine` (AAP 0.3.5).
///
/// Adds a transaction to the live master in one database transaction: the
/// `COTRN02C` edits in the COBOL order (key fields with their cross-reference
/// read, data fields with the `CSUTLDTC` date checks, then the `Y`/`N`
/// confirmation), the highest live TRAN-ID plus one, the record write, and the
/// reward accrual of the new transaction under the processing clock. A 201
/// carries the transaction and the `COTRN02C` success text.
///
/// Every `COTRN02C` outcome other than the add sets `WS-ERR-FLG`, the `N` and
/// blank confirmations included, so each is an error status and nothing is
/// written.
///
/// # Errors
///
/// - 400 for a member longer than its map field, a failed field edit or an
///   unconfirmed add (V-COTRN02C-001 to V-COTRN02C-003, V-COTRN02C-008 to
///   V-COTRN02C-028).
/// - 401 without a valid bearer token.
/// - 404 for an account or card without a cross-reference (V-COTRN02C-004,
///   V-COTRN02C-006), and for an add against an empty live master
///   (V-COTRN02C-029, E-27).
/// - 409 when the allocated TRAN-ID is already live (V-COTRN02C-032).
/// - 500 when a cross-reference read, the TRAN-ID allocation or the write
///   fails (V-COTRN02C-005, V-COTRN02C-007, V-COTRN02C-030, V-COTRN02C-031,
///   V-COTRN02C-033), and when the reward accrual or the commit fails or no
///   unit of work can be opened. A failure after validation rolls back the
///   insert, the accrual and the balance update together.
#[utoipa::path(
    post,
    path = "/transactions",
    tag = "transactions",
    request_body = TransactionAddRequest,
    responses(
        (status = 201, description = "The transaction was added, with its reward accrual", body = TransactionAddResponse),
        (status = 400, description = "Over-length member, failed field edit or unconfirmed add", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "No cross-reference for the account or card, or an empty transaction master", body = ErrorBody),
        (status = 409, description = "The allocated Tran ID is already live", body = ErrorBody),
        (status = 500, description = "A read, the allocation, the write, the reward accrual or the commit failed; nothing was persisted", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn create_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<TransactionAddRequest>,
) -> Result<(StatusCode, Json<TransactionAddResponse>), ApiError> {
    let span = tracing::info_span!(
        "create_transaction",
        transaction = "CT02",
        user_id = %user.user_id,
        account_id = Empty,
        tran_id = Empty,
        reward_points = Empty,
        outcome = Empty,
    );
    let (outcome, result) = add_transaction(&state, body, &span)
        .instrument(span.clone())
        .await;
    span.record("outcome", outcome);
    result.map(|added| (StatusCode::CREATED, Json(added)))
}

/// The CT02 add with the `outcome` it ended in: validation in a fresh unit of
/// work, then the writes in the same unit, committed only when every one of
/// them succeeded.
async fn add_transaction(
    state: &AppState,
    body: TransactionAddRequest,
    span: &Span,
) -> (&'static str, Result<TransactionAddResponse, ApiError>) {
    let (mut uow, validated) = match validate_request(state, body).await {
        Ok(opened) => opened,
        Err(err) => return (OUTCOME_REJECTED, Err(err)),
    };
    span.record("account_id", display(validated.account_id()));
    match insert_and_accrue(&mut uow, &validated, state.clock(), span).await {
        Ok(added) => match uow.commit().await {
            Ok(()) => (OUTCOME_COMMITTED, Ok(added)),
            // A failed `COMMIT` leaves nothing behind: the server ends the
            // transaction without applying it.
            Err(err) => (OUTCOME_ROLLED_BACK, Err(ApiError::from(err))),
        },
        Err(err) => {
            roll_back(uow).await;
            (OUTCOME_ROLLED_BACK, Err(err))
        }
    }
}

/// COTRN02C PROCESS-ENTER-KEY — P-COTRN02C-003, P-COTRN02C-004;
/// V-COTRN02C-001 to V-COTRN02C-028: the width and encoding check of the
/// body, then the `COTRN02C` edits in the unit of work the add continues in.
///
/// # Errors
///
/// That check's 400, a failed `Db::begin` (500), or the first edit the
/// rule rejects; the unit of work is rolled back before an edit error is
/// returned.
async fn validate_request(
    state: &AppState,
    body: TransactionAddRequest,
) -> Result<(UnitOfWork, ValidatedTransaction), ApiError> {
    body.check()?;
    let input = body.into_input();
    let mut uow = state.db().begin().await?;
    let validated = transaction_add::validate(&mut uow.store(), input).await;
    match validated {
        Ok(validated) => Ok((uow, validated)),
        Err(err) => {
            roll_back(uow).await;
            Err(ApiError::from(err))
        }
    }
}

/// COTRN02C ADD-TRANSACTION, WRITE-TRANSACT-FILE — P-COTRN02C-020 to
/// P-COTRN02C-025; V-COTRN02C-029 to V-COTRN02C-033; `RewardEngine`
/// (AAP 0.3.5): lock the account the accrual credits, allocate the TRAN-ID,
/// write the record, accrue its points.
///
/// Nothing here commits or rolls back; the caller ends `uow` according to the
/// result.
///
/// # Errors
///
/// 500 when the account lock fails, the allocation error of
/// `transaction_add::allocated_tran_id`, the write error of
/// `transaction_add::write_result`, and 500 for a reward-engine failure.
async fn insert_and_accrue(
    uow: &mut UnitOfWork,
    validated: &ValidatedTransaction,
    clock: &impl Clock,
    span: &Span,
) -> Result<TransactionAddResponse, ApiError> {
    // Lock order (`carddemo_persistence::ids`): the account lock the reward
    // balance's foreign key takes comes before the TRAN-ID advisory lock, as
    // CB00's `READ UPDATE` of the account does, so a CT02 and a CB00 on the
    // same account queue on the account row and never deadlock.
    uow.lock_account_reference(validated.account_id()).await?;
    // An allocation failure gets the audited `COTRN02C` browse messages, an
    // empty live master its not-found (V-COTRN02C-029 to V-COTRN02C-031),
    // rather than the generic unexpected error.
    let allocated = uow.next_tran_id().await.map_err(PortError::from);
    log_unavailable("allocate TRAN-ID", &allocated);
    let tran_id = transaction_add::allocated_tran_id(allocated)?;
    span.record("tran_id", display(tran_id));

    let record = transaction_add::build(validated, tran_id);
    let written = uow.store().write_transaction(record.clone()).await;
    log_unavailable("write transaction", &written);
    let message = transaction_add::write_result(written.clone(), &tran_id)?;
    // `write_result` has answered every write error, so `written` holds the
    // row identity here.
    let transaction = Transaction::from_new(written?, record);

    let input = AccrualInput::for_transaction(&transaction, validated.account_id());
    let accrual = RewardEngine::accrue(&mut uow.rewards(), input, clock.now()).await?;
    if let Some(accrual) = accrual {
        span.record("reward_points", display(accrual.points_earned));
    }
    Ok(TransactionAddResponse::added(&transaction, message))
}

/// Logs the adapter detail of a failed store call whose COBOL message the rule
/// chooses without it, so the cause of the 500 stays in the log.
fn log_unavailable<T>(step: &'static str, result: &PortResult<T>) {
    if let Err(PortError::Unavailable(detail)) = result {
        tracing::error!(step, error = %detail, "store unavailable");
    }
}

/// Rolls back `uow`, which holds either reads only or a CT02 add that failed.
///
/// A failed `ROLLBACK` changes no response: the transaction never commits,
/// and the server discards it when its connection closes.
async fn roll_back(uow: UnitOfWork) {
    if let Err(err) = uow.rollback().await {
        tracing::warn!(error = %err, "unit of work rollback failed");
    }
}

/// The `outcome` of a read handler's `result`.
const fn read_outcome<T>(result: &Result<T, ApiError>) -> &'static str {
    match result {
        Ok(_) => OUTCOME_OK,
        Err(_) => OUTCOME_REJECTED,
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::error::DomainError;
    use carddemo_domain::messages::{
        TRAN_ID_CAN_NOT_BE_EMPTY, TRANSACTION_ID_NOT_FOUND, UNABLE_TO_LOOKUP_TRANSACTION,
    };
    use carddemo_domain::rules::transaction_view::TRAN_ID_FIELD;

    use super::{TRAN_ID_BLANK, TRAN_ID_NOT_FOUND, TRAN_ID_UNREAD, tran_id_rejection};
    use crate::error::ApiError;

    #[test]
    fn a_tran_id_that_found_no_transaction_is_recorded_only_by_its_failure_class() {
        // CT01 error -> `tran_id_rejected`
        let cases = [
            (
                DomainError::empty(TRAN_ID_CAN_NOT_BE_EMPTY).with_field(TRAN_ID_FIELD),
                TRAN_ID_BLANK,
            ),
            (
                DomainError::not_found(TRANSACTION_ID_NOT_FOUND).with_field(TRAN_ID_FIELD),
                TRAN_ID_NOT_FOUND,
            ),
            (
                DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION).with_field(TRAN_ID_FIELD),
                TRAN_ID_UNREAD,
            ),
        ];
        for (error, expected) in cases {
            let error = ApiError::from(error);
            assert_eq!(tran_id_rejection(&error), expected, "{error:?}");
        }
        // No unit of work could be opened, so the Tran ID was never edited.
        assert_eq!(tran_id_rejection(&ApiError::unexpected()), TRAN_ID_UNREAD);
    }
}
