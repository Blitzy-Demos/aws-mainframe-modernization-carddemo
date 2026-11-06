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
//! The allocation's advisory lock is held until that transaction ends, and an
//! add waiting for it holds its pooled connection. So the add is admitted
//! through the per-process TRAN-ID writer limit
//! (`AppState::admit_tran_id_writer`) after the body's own check and before
//! it opens the transaction, and keeps its permit until the transaction has
//! committed or rolled back: writers beyond the limit queue holding no
//! connection, and the reads keep the rest of the pool. The permit changes no
//! lock order, outcome or span field.
//!
//! # Telemetry
//!
//! Each handler runs inside one INFO span named after its operation, with
//! `transaction` (`CT00`, `CT01`, `CT02`), the signed-on `user_id`, the
//! resource keys and `outcome`, recorded once the unit of work has finished:
//!
//! | `outcome` | Recorded for |
//! |---|---|
//! | `ok` | A CT00 or CT01 read that returned its result |
//! | `rejected` | A 4xx answer before any write: a rejected edit, a key not found, an unconfirmed CT02 add |
//! | `failed` | A 5xx answer before any write: a failed read, no unit of work, an input class held at `halt` |
//! | `committed` | A CT02 add whose unit of work committed |
//! | `rolled_back` | A CT02 add that failed after validation, rolled back whole; after a failed `COMMIT` a reported label only, since a `COMMIT` the server rejects rolls the add back whole but one whose answer is lost may have committed it whole |
//! | `cancelled` | A request dropped before its answer, its client gone: an open unit of work is rolled back as it is dropped, unless its commit had already reached the server |
//!
//! CT02 also records `reward_points` when the add accrued points. Each field
//! is recorded at most once per request.
//!
//! A key is recorded only in the form its check gave it, never as entered:
//! `account_id` is the typed account of the CT00 filter, and `tran_id` the
//! TRAN-ID that CT02 allocated or that the CT01 read found. A CT00 filter that
//! fails its check records no key. A CT01 Tran ID that found no transaction
//! records no part of its text, only `tran_id_length`, its count of
//! characters, and `tran_id_rejected`:
//!
//! | `tran_id_rejected` | The Tran ID |
//! |---|---|
//! | `blank` | Was not entered: empty, spaces or `LOW-VALUES` (V-COTRN01C-001) |
//! | `not_found` | Names no live transaction (V-COTRN01C-002) |
//! | `unread` | Was never looked up: the read failed or no unit of work could be opened |
//!
//! The CT02 key reaches the span through `transaction_add::edit_key_fields`,
//! the `VALIDATE-INPUT-KEY-FIELDS` key edit the rule applies before its
//! cross-reference read, which the handler applies first, before the body's
//! own check, so every CT02 span carries the key or why it was refused. The
//! account ID is tested first, and the card number only when the account ID
//! is blank, as the edit tests them. An account ID the edit accepts is
//! recorded as `account_id`, its 11 digits. A card number the edit accepts is
//! recorded as `card_number`, masked to its last four digits as
//! `CardNumber`'s `Debug` shows it, and, once validation has read its
//! cross-reference, that record's account as `account_id`. A refused key
//! records no part of its text, only `account_id_length` or
//! `card_number_length`, its count of characters, and `account_id_rejected`
//! or `card_number_rejected`:
//!
//! | `*_rejected` | The key |
//! |---|---|
//! | `too_long` | Longer than its map field, `ACTIDIN` X(11) or `CARDNIN` X(16), whatever it holds: the member [`TransactionAddRequest::check`] refuses |
//! | `blank` | Neither key entered, recorded as `account_id_rejected` (V-COTRN02C-003) |
//! | `invalid` | Any other refused key: not a number of its field's width (V-COTRN02C-001, V-COTRN02C-002) |

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use carddemo_domain::clock::Clock;
use carddemo_domain::numeric::AccountId;
use carddemo_domain::rewards::RewardEngine;
use carddemo_domain::rules::transaction_add::{
    self, EditedKey, FIELD_ACTIDIN, Transaction, TransactionWriter, ValidatedTransaction,
};
use carddemo_domain::rules::transaction_list;
use carddemo_domain::rules::transaction_view::{self, TransactionViewInput};
use carddemo_domain::rules::{AccrualInput, ErrorKind, PortError, PortResult};
use carddemo_persistence::tx::UnitOfWork;
use tokio::sync::OwnedSemaphorePermit;
use tracing::field::{Empty, display};
use tracing::{Instrument, Span};

use super::cards::masked_card_number;
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

/// `outcome` of a request answered with a 4xx status before anything was
/// written: a rejected edit or a key not found.
const OUTCOME_REJECTED: &str = "rejected";

/// `outcome` of a request answered with a 5xx status before anything was
/// written: a failed read, or a unit of work that could not be opened.
const OUTCOME_FAILED: &str = "failed";

/// `outcome` of a CT02 add whose unit of work committed.
const OUTCOME_COMMITTED: &str = "committed";

/// `outcome` of a CT02 add that failed after validation and was rolled back
/// whole, or whose `COMMIT` failed: a `COMMIT` the server rejects rolls the
/// add back whole, but one whose answer is lost may have committed it whole.
const OUTCOME_ROLLED_BACK: &str = "rolled_back";

/// `tran_id_rejected` of a CT01 Tran ID that was not entered.
const TRAN_ID_BLANK: &str = "blank";

/// `tran_id_rejected` of a CT01 Tran ID that names no live transaction.
const TRAN_ID_NOT_FOUND: &str = "not_found";

/// `tran_id_rejected` of a CT01 Tran ID that was never looked up.
const TRAN_ID_UNREAD: &str = "unread";

/// `CARDNINI PIC X(16)` [app/cpy-bms/COTRN02.CPY:L66]: a CT02 card number
/// longer than this is [`KEY_TOO_LONG`].
const CARD_NUMBER_LEN: usize = 16;

/// `account_id_rejected` of a CT02 body that enters neither key.
const KEY_BLANK: &str = "blank";

/// `*_rejected` of a CT02 key longer than its map field, whatever it holds.
const KEY_TOO_LONG: &str = "too_long";

/// `*_rejected` of any other CT02 key the edit refused.
const KEY_INVALID: &str = "invalid";

/// The key of a CT02 body as the handler span records it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AddSpanKey {
    /// The account ID the key edit accepted, recorded as `account_id`.
    Account(AccountId),
    /// The card number the key edit accepted, the account ID being blank,
    /// masked by [`masked_card_number`] and recorded as `card_number`.
    Card(String),
    /// A refused account ID, or no key entered: `category` is recorded as
    /// `account_id_rejected` and `length`, the account ID's count of
    /// characters, as `account_id_length`.
    AccountRefused {
        category: &'static str,
        length: usize,
    },
    /// A refused card number, the account ID being blank: `category` is
    /// recorded as `card_number_rejected` and `length`, the card number's
    /// count of characters, as `card_number_length`.
    CardRefused {
        category: &'static str,
        length: usize,
    },
}

impl AddSpanKey {
    /// Records this key on `span`; the fields it does not set stay empty.
    fn record(&self, span: &Span) {
        match self {
            Self::Account(account_id) => {
                span.record("account_id", display(account_id));
            }
            Self::Card(masked) => {
                span.record("card_number", masked.as_str());
            }
            Self::AccountRefused { category, length } => {
                span.record("account_id_rejected", *category)
                    .record("account_id_length", *length);
            }
            Self::CardRefused { category, length } => {
                span.record("card_number_rejected", *category)
                    .record("card_number_length", *length);
            }
        }
    }
}

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
    let result = crate::cancellable(
        span.clone(),
        list_page(&state, filter, page, &span).instrument(span.clone()),
    )
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
    let result = crate::cancellable(
        span.clone(),
        view_one(&state, tran_id).instrument(span.clone()),
    )
    .await;
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
///   unit of work can be opened. A failure after validation and before the
///   `COMMIT`, or a `COMMIT` the server rejects, rolls back the insert, the
///   accrual and the balance update together. A `COMMIT` whose answer is
///   lost, its connection gone, leaves the outcome unknown: the three
///   committed or rolled back together, so the transactions are listed again
///   before the add is retried.
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
        (status = 500, description = "A read, the allocation, the write, the reward accrual or the commit failed; nothing was persisted unless a `COMMIT` lost its answer with its connection, which leaves the outcome unknown, so the transactions are listed again before a retry", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn create_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(body): ApiJson<TransactionAddRequest>,
) -> Result<(StatusCode, Json<TransactionAddResponse>), ApiError> {
    let span = add_span(&user.user_id);
    let (outcome, result) = crate::cancellable(
        span.clone(),
        add_transaction(&state, body, &span).instrument(span.clone()),
    )
    .await;
    span.record("outcome", outcome);
    result.map(|added| (StatusCode::CREATED, Json(added)))
}

/// The CT02 handler span of the signed-on `user_id`, its key, `tran_id`,
/// `reward_points` and `outcome` fields empty until recorded.
fn add_span(user_id: &str) -> Span {
    tracing::info_span!(
        "create_transaction",
        transaction = "CT02",
        user_id = user_id,
        account_id = Empty,
        account_id_rejected = Empty,
        account_id_length = Empty,
        card_number = Empty,
        card_number_rejected = Empty,
        card_number_length = Empty,
        tran_id = Empty,
        reward_points = Empty,
        outcome = Empty,
    )
}

/// The CT02 add with the `outcome` it ended in: the key the body names
/// recorded on `span`, validation in a fresh unit of work, then the writes in
/// the same unit, committed only when every one of them succeeded. The TRAN-ID
/// writer permit validation was admitted with is held until the unit has
/// committed or rolled back.
async fn add_transaction(
    state: &AppState,
    body: TransactionAddRequest,
    span: &Span,
) -> (&'static str, Result<TransactionAddResponse, ApiError>) {
    let key = add_span_key(body.account_id.as_deref(), body.card_number.as_deref());
    key.record(span);
    // `_writer` is dropped when this function returns, after the unit of work
    // below has ended.
    let (_writer, mut uow, validated) = match validate_request(state, body).await {
        Ok(opened) => opened,
        Err(err) => return (unwritten_outcome(&err), Err(err)),
    };
    // A card-keyed add learns its account from the cross-reference that
    // validation read; an account-keyed add has recorded it already.
    if !matches!(key, AddSpanKey::Account(_)) {
        span.record("account_id", display(validated.account_id()));
    }
    match insert_and_accrue(&mut uow, &validated, state.clock(), span).await {
        Ok(added) => match uow.commit().await {
            Ok(()) => (OUTCOME_COMMITTED, Ok(added)),
            // `rolled_back` is the reported label, not proof: a `COMMIT`
            // the server rejects ends the transaction without applying it,
            // but one whose answer is lost, its connection gone, may have
            // committed it.
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
/// body, then the TRAN-ID writer permit, then the `COTRN02C` edits in the unit
/// of work the add continues in, returned with the permit, which the caller
/// holds until that unit ends.
///
/// # Errors
///
/// That check's 400, a failed admission or `Db::begin` (500), or the first
/// edit the rule rejects; the unit of work is rolled back before an edit
/// error is returned.
async fn validate_request(
    state: &AppState,
    body: TransactionAddRequest,
) -> Result<(OwnedSemaphorePermit, UnitOfWork, ValidatedTransaction), ApiError> {
    body.check()?;
    let input = body.into_input();
    let writer = state.admit_tran_id_writer().await?;
    let mut uow = state.db().begin().await?;
    let validated = transaction_add::validate(&mut uow.store(), input).await;
    match validated {
        Ok(validated) => Ok((writer, uow, validated)),
        Err(err) => {
            roll_back(uow).await;
            Err(ApiError::from(err))
        }
    }
}

/// COTRN02C VALIDATE-INPUT-KEY-FIELDS — P-COTRN02C-005 to P-COTRN02C-007;
/// V-COTRN02C-001 to V-COTRN02C-003
///
/// The account ID `account_id` and card number `card_number` of a CT02 body
/// as the span records them, through `transaction_add::edit_key_fields`, the
/// key edit `transaction_add::validate` applies to the same texts.
///
/// An account ID longer than the 11 characters of `ACTIDINI X(11)`, or, with
/// the account ID blank, a card number longer than the 16 of `CARDNINI X(16)`,
/// is the member [`TransactionAddRequest::check`] refuses before any edit, and
/// is recorded as [`KEY_TOO_LONG`] whatever it holds. Any other key is
/// recorded as the edit decides: the account or the masked card it accepted,
/// [`KEY_BLANK`] when neither is entered, else [`KEY_INVALID`] for the key it
/// refused. No part of a refused key's text is kept.
fn add_span_key(account_id: Option<&str>, card_number: Option<&str>) -> AddSpanKey {
    let account_length = account_id.map_or(0, |text| text.chars().count());
    if account_length > AccountId::WIDTH {
        return AddSpanKey::AccountRefused {
            category: KEY_TOO_LONG,
            length: account_length,
        };
    }
    let card_length = card_number.map_or(0, |text| text.chars().count());
    match transaction_add::edit_key_fields(account_id, card_number) {
        Ok(EditedKey::Account(account)) => AddSpanKey::Account(account),
        Err(error)
            if error.kind != ErrorKind::Empty && error.first_field() == Some(FIELD_ACTIDIN) =>
        {
            AddSpanKey::AccountRefused {
                category: KEY_INVALID,
                length: account_length,
            }
        }
        // The account ID is blank from here on, so the card number is the key.
        _ if card_length > CARD_NUMBER_LEN => AddSpanKey::CardRefused {
            category: KEY_TOO_LONG,
            length: card_length,
        },
        Ok(EditedKey::Card(card)) => AddSpanKey::Card(masked_card_number(&card)),
        Err(error) if error.kind == ErrorKind::Empty => AddSpanKey::AccountRefused {
            category: KEY_BLANK,
            length: account_length,
        },
        Err(_) => AddSpanKey::CardRefused {
            category: KEY_INVALID,
            length: card_length,
        },
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
    // rather than the generic unexpected error. The conversion is called here
    // rather than passed to `map_err`, so a failure it logs names this line.
    let allocated = match uow.next_tran_id().await {
        Ok(tran_id) => Ok(tran_id),
        Err(err) => Err(PortError::from(err)),
    };
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

/// The `outcome` of a read handler's `result`: [`OUTCOME_OK`] for its result,
/// else that of [`unwritten_outcome`].
fn read_outcome<T>(result: &Result<T, ApiError>) -> &'static str {
    match result {
        Ok(_) => OUTCOME_OK,
        Err(error) => unwritten_outcome(error),
    }
}

/// The `outcome` of `error` answering a request before anything was written:
/// [`OUTCOME_FAILED`] for a 5xx status, [`OUTCOME_REJECTED`] for any other.
fn unwritten_outcome(error: &ApiError) -> &'static str {
    if error.status().is_server_error() {
        OUTCOME_FAILED
    } else {
        OUTCOME_REJECTED
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use carddemo_domain::error::DomainError;
    use carddemo_domain::messages::{
        TRAN_ID_CAN_NOT_BE_EMPTY, TRANSACTION_ID_NOT_FOUND, UNABLE_TO_LOOKUP_TRANSACTION,
    };
    use carddemo_domain::numeric::AccountId;
    use carddemo_domain::rules::transaction_view::TRAN_ID_FIELD;
    use serde_json::{Map, Value, json};
    use tracing::field::display;

    use super::{
        AddSpanKey, KEY_BLANK, KEY_INVALID, KEY_TOO_LONG, OUTCOME_FAILED, OUTCOME_OK,
        OUTCOME_REJECTED, TRAN_ID_BLANK, TRAN_ID_NOT_FOUND, TRAN_ID_UNREAD, add_span, add_span_key,
        read_outcome, tran_id_rejection, unwritten_outcome,
    };
    use crate::error::ApiError;

    /// The first fixture card, of account 00000000050 (P-CVACT02Y-009)
    /// [app/data/ASCII/carddata.txt:L1].
    const CARD: &str = "0500024453765740";

    /// [`CARD`] as the span shows it.
    const MASKED_CARD: &str = "************5740";

    /// The span key of the account `number`, as the edit accepted it.
    fn account(number: u64) -> AddSpanKey {
        AddSpanKey::Account(AccountId::new(number).expect("an account number of 11 digits"))
    }

    const fn account_refused(category: &'static str, length: usize) -> AddSpanKey {
        AddSpanKey::AccountRefused { category, length }
    }

    const fn card_refused(category: &'static str, length: usize) -> AddSpanKey {
        AddSpanKey::CardRefused { category, length }
    }

    #[test]
    fn the_ct02_span_key_is_the_edited_key_or_only_the_refusal_class_and_length() {
        let masked = || AddSpanKey::Card(MASKED_CARD.to_owned());
        // (account ID, card number) -> what the span records
        let cases = [
            (Some("00000000050"), None, account(50)),
            (Some("00000000050"), Some(CARD), account(50)),
            // The account wins: the card is ignored, whatever it holds.
            (Some("00000000050"), Some("05000244537657401"), account(50)),
            // An account the cross-reference does not know (404), and one
            // whose add fails a data edit (400 TRNAMT).
            (Some("99999999999"), None, account(99_999_999_999)),
            (Some("00000000001"), None, account(1)),
            (None, Some(CARD), masked()),
            (Some(""), Some(CARD), masked()),
            (Some("           "), Some(CARD), masked()),
            (Some("\0\0"), Some(CARD), masked()),
            (None, None, account_refused(KEY_BLANK, 0)),
            (Some("   "), Some(""), account_refused(KEY_BLANK, 3)),
            (
                Some("\0\0\0\0\0\0\0\0\0\0\0"),
                Some("    "),
                account_refused(KEY_BLANK, 11),
            ),
            (
                Some("0000000005A"),
                Some(CARD),
                account_refused(KEY_INVALID, 11),
            ),
            (Some("50"), None, account_refused(KEY_INVALID, 2)),
            (Some("*"), Some(CARD), account_refused(KEY_INVALID, 1)),
            (Some("\u{e9}"), None, account_refused(KEY_INVALID, 1)),
            (
                Some("000000000500"),
                None,
                account_refused(KEY_TOO_LONG, 12),
            ),
            (
                Some("            "),
                Some(CARD),
                account_refused(KEY_TOO_LONG, 12),
            ),
            (
                None,
                Some("050002445376574X"),
                card_refused(KEY_INVALID, 16),
            ),
            (
                Some(" "),
                Some("0500024453765"),
                card_refused(KEY_INVALID, 13),
            ),
            (None, Some("5740"), card_refused(KEY_INVALID, 4)),
            (
                None,
                Some("05000244537657401"),
                card_refused(KEY_TOO_LONG, 17),
            ),
            // Over-length members the body check refuses although the edit
            // would read them as the card, or as no key.
            (
                None,
                Some("0500024453765740 "),
                card_refused(KEY_TOO_LONG, 17),
            ),
            (
                Some(""),
                Some("                 "),
                card_refused(KEY_TOO_LONG, 17),
            ),
        ];
        for (account_id, card_number, expected) in cases {
            assert_eq!(
                add_span_key(account_id, card_number),
                expected,
                "{account_id:?}, {card_number:?}"
            );
        }
    }

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The `account_id*` and `card_number*` attributes the span exporter of
    /// the API's telemetry subscriber wrote for a CT02 handler span on which
    /// `record` ran.
    fn exported_key_attributes(record: impl FnOnce(&tracing::Span)) -> Map<String, Value> {
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) =
            carddemo_runtime::telemetry::subscriber_with_writer("api", "info", move || {
                sink.clone()
            })
            .expect("valid directives");
        tracing::subscriber::with_default(subscriber, || record(&add_span("USER0001")));
        guard.shutdown().expect("the tracer provider shuts down");
        let bytes = capture.0.lock().expect("capture buffer").clone();
        let text = String::from_utf8(bytes).expect("JSON lines are UTF-8");
        let spans: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
            .filter(|line| line["type"] == "span" && line["name"] == "create_transaction")
            .collect();
        let [span] = spans.as_slice() else {
            panic!("one CT02 span expected: {text}");
        };
        assert_eq!(span["attributes"]["transaction"], "CT02", "{text}");
        assert_eq!(span["attributes"]["user_id"], "USER0001", "{text}");
        span["attributes"]
            .as_object()
            .expect("an attribute object")
            .iter()
            .filter(|(name, _)| name.starts_with("account_id") || name.starts_with("card_number"))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    #[test]
    fn the_ct02_span_exports_each_key_under_its_declared_fields() {
        // (key, account of the validated card-keyed add) -> exported attributes.
        // `tracing-opentelemetry` carries an unsigned length through its debug
        // form, as it carries the `CAVW`, `CAUP` and `CCDL` lengths, so the
        // exporter writes it as a string.
        let cases = [
            (account(50), None, json!({"account_id": "00000000050"})),
            (
                AddSpanKey::Card(MASKED_CARD.to_owned()),
                Some(50),
                json!({"card_number": MASKED_CARD, "account_id": "00000000050"}),
            ),
            (
                account_refused(KEY_INVALID, 11),
                None,
                json!({"account_id_rejected": "invalid", "account_id_length": "11"}),
            ),
            (
                card_refused(KEY_TOO_LONG, 17),
                None,
                json!({"card_number_rejected": "too_long", "card_number_length": "17"}),
            ),
        ];
        for (key, validated_account, expected) in cases {
            let attributes = exported_key_attributes(|span| {
                key.record(span);
                if let Some(number) = validated_account {
                    let account_id = AccountId::new(number).expect("an account of 11 digits");
                    span.record("account_id", display(account_id));
                }
            });
            assert_eq!(Value::Object(attributes), expected, "{key:?}");
        }
    }

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

    #[test]
    fn an_answer_before_any_write_is_failed_for_a_5xx_status_and_rejected_otherwise() {
        // (read result, `outcome`)
        let cases = [
            (Ok(()), OUTCOME_OK),
            (
                Err(DomainError::empty(TRAN_ID_CAN_NOT_BE_EMPTY).with_field(TRAN_ID_FIELD)),
                OUTCOME_REJECTED,
            ),
            (
                Err(DomainError::not_found(TRANSACTION_ID_NOT_FOUND).with_field(TRAN_ID_FIELD)),
                OUTCOME_REJECTED,
            ),
            (
                Err(DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION).with_field(TRAN_ID_FIELD)),
                OUTCOME_FAILED,
            ),
        ];
        for (result, outcome) in cases {
            let result = result.map_err(ApiError::from);
            assert_eq!(read_outcome(&result), outcome, "{result:?}");
            if let Err(error) = &result {
                assert_eq!(unwritten_outcome(error), outcome, "{error:?}");
            }
        }
        // No unit of work could be opened.
        let unopened = ApiError::unexpected();
        assert_eq!(unwritten_outcome(&unopened), OUTCOME_FAILED);
        assert_eq!(read_outcome::<()>(&Err(unopened)), OUTCOME_FAILED);
    }
}
