//! The card transactions: `CCLI` (`COCRDLIC`, map `COCRDLI`) as
//! `GET /cards`, `CCDL` (`COCRDSLC`, map `COCRDSL`) as `GET /cards/{id}` and
//! `CCUP` (`COCRDUPC`, map `COCRDUP`) as `PUT /cards/{id}`.
//!
//! Each handler bounds the map field lengths through its DTO, hands the
//! fields as entered to the rule of `carddemo_domain::rules`, and turns the
//! rule's answer into the response or, through [`ApiError`], into the audited
//! COBOL message with its mapped status. No edit, message or browse decision
//! is made here.
//!
//! # Units of work
//!
//! - `GET /cards` and `GET /cards/{id}` read without a lock, as the CICS
//!   `STARTBR`/`READNEXT`/`READPREV` and `READ` they replace: the unit of work
//!   is opened for the read and rolled back after it.
//! - `PUT /cards/{id}` performs the fetch, the ENTER edits and the PF5 write
//!   of `COCRDUPC` in one request (E-56): the PF5 confirmation becomes the
//!   SPA's save action. The one `REWRITE` runs under its own savepoint in
//!   `carddemo_persistence`, so a failed lock, change check or rewrite leaves
//!   nothing written, and with a single write `RECOVERY(NONE)` keeps nothing
//!   either [app/csd/CARDDEMO.CSD:L25-33]. A rule error therefore rolls the
//!   unit of work back; success commits it.
//!
//! # Identity
//!
//! Every route takes [`AuthUser`]: any signed-on user may address any card,
//! because `USRSEC` links users to no account and the COBOL screens accept
//! any key (AAP 0.6.1). The account a card belongs to is a resource key the
//! caller supplies (E-55), never the actor.
//!
//! # Telemetry
//!
//! Each handler runs in a span carrying `transaction` (`CCLI`, `CCDL` or
//! `CCUP`), the caller's `user_id`, the keys of the request (`account_id` and
//! `card_number`) and the `outcome` recorded once the unit of work has ended:
//! `ok` for an answered read, `committed` for an update, `rejected` for a 4xx
//! answer and `failed` for a 5xx one. A card number is recorded masked to its
//! last four characters, as `CardNumber`'s `Debug` shows it, so the JSON
//! logs hold no full card number.

use axum::Json;
use axum::extract::State;
use carddemo_domain::error::ErrorKind;
use carddemo_domain::rules::{VersionToken, card_list, card_update, card_view};
use carddemo_persistence::UnitOfWork;
use tracing::Span;
use tracing::field::Empty;

use crate::auth::AuthUser;
use crate::dto::cards::{
    CardListQuery, CardResponse, CardSummary, CardUpdateRequest, CardUpdateResponse, CardViewQuery,
};
use crate::dto::id_string;
use crate::error::{ApiError, ApiJson, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::{Page, PageQuery};
use crate::state::AppState;

/// The `message` of a `PUT /cards/{id}` without a well-formed version token:
/// empty, because V-COCRDUPC-001 records the path as `no-message (E-24)` and
/// the catalog holds no text for it.
const NO_MESSAGE: &str = "";

/// The span `outcome` of a read answered with its data.
const OUTCOME_OK: &str = "ok";

/// The span `outcome` of an update whose unit of work committed.
const OUTCOME_COMMITTED: &str = "committed";

/// The span `outcome` of a request answered with a 4xx status.
const OUTCOME_REJECTED: &str = "rejected";

/// The span `outcome` of a request answered with a 5xx status.
const OUTCOME_FAILED: &str = "failed";

/// `CARD-NUM PIC X(16)` [app/cpy/CVACT02Y.cpy:L5]: the characters of a card
/// number a span shows, as the `MOVE` into the key cuts the entry.
const CARD_NUMBER_LEN: usize = 16;

/// The trailing characters of a card number a span shows unmasked.
const CARD_NUMBER_SHOWN: usize = 4;

/// COCRDLIC 0000-MAIN, 2200-EDIT-INPUTS, 9000-READ-FORWARD,
/// 9100-READ-BACKWARDS, 9500-FILTER-RECORDS, 1400-SETUP-MESSAGE —
/// P-COCRDLIC-003 to P-COCRDLIC-006, P-COCRDLIC-011 to P-COCRDLIC-014,
/// P-COCRDLIC-017, P-COCRDLIC-021, P-COCRDLIC-023, V-COCRDLIC-001,
/// V-COCRDLIC-002, V-COCRDLIC-005 to V-COCRDLIC-010
///
/// One page of the card list in `CARDDAT` key order: forward (ENTER, PF8)
/// from `start_key` inclusively, or backward (PF7) from the first card of the
/// page shown, `WS-MAX-SCREEN-LINES` (7) rows unless `page_size` says
/// otherwise. The `account_id` and `card_number` filters reach the rule as
/// entered, so a filter that is neither blank nor a number of its width is
/// answered with the `2210-EDIT-ACCOUNT` or `2220-EDIT-CARD` text. An empty
/// page is a page: 200 with `items: []` and the text the COBOL shows with it.
///
/// # Errors
///
/// - 400 with an empty `message` when a filter is longer than its map field.
/// - 400 with 'ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER' at
///   `ACCTSID` or 'CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER' at
///   `CARDSID` (V-COCRDLIC-001, V-COCRDLIC-002).
/// - 400 with code `INVALID` when the page parameters do not deserialize.
/// - 401 without a valid bearer token.
/// - 500 with the `CARDDAT` file-error text at `ACCTSID` when a card read
///   fails (V-COCRDLIC-008, V-COCRDLIC-010), or 'Unexpected error' when the
///   database cannot be reached.
#[utoipa::path(
    get,
    path = "/cards",
    tag = "cards",
    params(CardListQuery, PageQuery),
    responses(
        (status = 200, description = "One page of cards in card-number order, with the keys of its neighbours and the COBOL page text", body = Page<CardSummary>),
        (status = 400, description = "A filter or page parameter the COBOL edits reject", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 500, description = "The card file could not be read", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = "CCLI",
        user_id = %user.user_id,
        account_id = Empty,
        card_number = Empty,
        outcome = Empty,
    ),
)]
pub async fn list_cards(
    user: AuthUser,
    State(state): State<AppState>,
    ApiQuery(filters): ApiQuery<CardListQuery>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> Result<Json<Page<CardSummary>>, ApiError> {
    finish(list(&state, filters, page).await, OUTCOME_OK).map(Json)
}

/// COCRDSLC 2000-PROCESS-INPUTS, 9000-READ-DATA, 1200-SETUP-SCREEN-VARS —
/// P-COCRDSLC-002 to P-COCRDSLC-006, P-COCRDSLC-010, P-COCRDSLC-013 to
/// P-COCRDSLC-016, V-COCRDSLC-001 to V-COCRDSLC-007
///
/// The card whose number is the `{id}` path segment, with the account number
/// of the `account_id` query parameter: both keys are edited as
/// `2210-EDIT-ACCOUNT` and `2220-EDIT-CARD` edit them, an absent account
/// being a blank one, and the card is then read by its number alone (E-55).
/// The response carries the card's version token, which a following
/// `PUT /cards/{id}` sends back, and never the verification code.
///
/// # Errors
///
/// - 400 with an empty `message` when `account_id` is longer than 11
///   characters.
/// - 400 with 'Account number not provided', 'Card number not provided',
///   'No input received', or the account or card not-numeric text, at
///   `ACCTSID` or `CARDSID` (V-COCRDSLC-001 to V-COCRDSLC-005).
/// - 401 without a valid bearer token.
/// - 404 with 'Did not find cards for this search condition' at `ACCTSID`
///   when no card has the number (V-COCRDSLC-006).
/// - 500 with the `CARDDAT` file-error text at `ACCTSID` when the read fails
///   (V-COCRDSLC-007), or 'Unexpected error' when the database cannot be
///   reached.
#[utoipa::path(
    get,
    path = "/cards/{id}",
    tag = "cards",
    params(
        ("id" = String, Path, description = "The card number, CARD-NUM X(16), as entered"),
        CardViewQuery,
    ),
    responses(
        (status = 200, description = "The card with its version token", body = CardResponse),
        (status = 400, description = "A key the COBOL edits reject", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "No card has the number", body = ErrorBody),
        (status = 500, description = "The card file could not be read", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = "CCDL",
        user_id = %user.user_id,
        account_id = Empty,
        card_number = Empty,
        outcome = Empty,
    ),
)]
pub async fn get_card(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(card_number): ApiPath<String>,
    ApiQuery(query): ApiQuery<CardViewQuery>,
) -> Result<Json<CardResponse>, ApiError> {
    finish(view(&state, card_number, query).await, OUTCOME_OK).map(Json)
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS, 2000-DECIDE-ACTION, 9000-READ-DATA,
/// 9200-WRITE-PROCESSING, 9300-CHECK-CHANGE-IN-REC, 3250-SETUP-INFOMSG —
/// P-COCRDUPC-002 to P-COCRDUPC-007, P-COCRDUPC-010 to P-COCRDUPC-016,
/// P-COCRDUPC-019, P-COCRDUPC-022, P-COCRDUPC-023, P-COCRDUPC-028,
/// V-COCRDUPC-001 to V-COCRDUPC-020
///
/// Updates the card whose number is the `{id}` path segment from the body's
/// map fields, in one request: the key edits, the fetch, the comparison with
/// the fetched card and the name, status, expiry month and year edits, then
/// the locked read, the change check against the body's `version` and the
/// rewrite. 200 on success, with 'Changes committed to database' after a
/// rewrite or 'No change detected with respect to values fetched.' when the
/// body changes nothing, and the card as it now stands with its new token.
///
/// # Errors
///
/// - 400 with an empty `message` and no field when `version` is absent or is
///   not 16 lower-case hex digits (V-COCRDUPC-001, E-24), or with an empty
///   `message` and the BMS fields of the values longer than their map
///   fields.
/// - 400 with the key edit texts at `ACCTSID` or `CARDSID`
///   (V-COCRDUPC-002 to V-COCRDUPC-006) or the card-field edit texts at
///   `CRDNAME`, `CRDSTCD`, `EXPMON` or `EXPYEAR` (V-COCRDUPC-010 to
///   V-COCRDUPC-017), the first in COBOL order.
/// - 400 with code `INVALID` when the body is not a JSON object of strings.
/// - 401 without a valid bearer token.
/// - 404 with 'Did not find cards for this search condition' at `ACCTSID`
///   (V-COCRDUPC-007).
/// - 409 with 'Record changed by some one else. Please review' at `CRDNAME`
///   when the card changed since `version` was issued (V-COCRDUPC-019).
/// - 500 with the `CARDDAT` file-error text (V-COCRDUPC-008), 'Could not lock
///   record for update' (V-COCRDUPC-018) or 'Update of record failed'
///   (V-COCRDUPC-020) at `ACCTSID`, or 'Unexpected error' when the database
///   cannot be reached or the commit fails.
#[utoipa::path(
    put,
    path = "/cards/{id}",
    tag = "cards",
    params(
        ("id" = String, Path, description = "The card number, CARD-NUM X(16), as entered"),
    ),
    request_body = CardUpdateRequest,
    responses(
        (status = 200, description = "The card as it now stands, with its new version token and the COBOL information text", body = CardUpdateResponse),
        (status = 400, description = "A missing version token, or a value the COBOL edits reject", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "No card has the number", body = ErrorBody),
        (status = 409, description = "The card changed since its version token was issued", body = ErrorBody),
        (status = 500, description = "The card could not be read, locked or rewritten", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = "CCUP",
        user_id = %user.user_id,
        account_id = Empty,
        card_number = Empty,
        outcome = Empty,
    ),
)]
pub async fn update_card(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(card_number): ApiPath<String>,
    ApiJson(body): ApiJson<CardUpdateRequest>,
) -> Result<Json<CardUpdateResponse>, ApiError> {
    finish(update(&state, card_number, body).await, OUTCOME_COMMITTED).map(Json)
}

/// The `GET /cards` page for `filters` and `page`, read in a unit of work
/// that writes nothing.
async fn list(
    state: &AppState,
    filters: CardListQuery,
    page: PageQuery,
) -> Result<Page<CardSummary>, ApiError> {
    filters.check()?;
    record_keys(
        filters.account_id.as_deref(),
        filters.card_number.as_deref(),
    );
    let (start_key, direction, page_size) = page.resolve(card_list::SCREEN_ROWS);
    let input = filters.into_input(start_key, direction, page_size);
    let mut uow = state.db().begin().await?;
    let listed = card_list::list(&mut uow.store(), input).await;
    release(uow).await;
    let listed = listed?;
    Ok(Page::new(
        listed.items.iter().map(CardSummary::from).collect(),
        listed.next_key.as_ref().map(id_string),
        listed.prev_key.as_ref().map(id_string),
        listed.message.map(str::to_owned),
    ))
}

/// The `GET /cards/{id}` answer for `card_number` and `query`, read in a unit
/// of work that writes nothing.
async fn view(
    state: &AppState,
    card_number: String,
    query: CardViewQuery,
) -> Result<CardResponse, ApiError> {
    query.check()?;
    record_keys(query.account_id.as_deref(), Some(&card_number));
    let input = query.into_input(card_number);
    let mut uow = state.db().begin().await?;
    let viewed = card_view::view(&mut uow.store(), state.version_key(), input).await;
    release(uow).await;
    Ok(CardResponse::from_view(&viewed?))
}

/// The `PUT /cards/{id}` answer for `card_number` and `body`: the whole
/// `COCRDUPC` dialogue in one unit of work, committed when the rule succeeds
/// and rolled back when it rejects the request.
async fn update(
    state: &AppState,
    card_number: String,
    body: CardUpdateRequest,
) -> Result<CardUpdateResponse, ApiError> {
    body.check(&card_number)?;
    record_keys(body.account_id.as_deref(), Some(&card_number));
    let version = body
        .version
        .as_deref()
        .and_then(VersionToken::parse)
        .ok_or_else(missing_version)?;
    let input = body.into_input(card_number, version);
    let key = state.version_key();
    let mut uow = state.db().begin().await?;
    let outcome = card_update::update(&mut uow.store(), key, &input).await;
    match outcome {
        Ok(updated) => {
            // The rewrite stands once committed, so a commit failure is the
            // only failure that can still undo it; the new token is computed
            // from the committed record.
            uow.commit().await?;
            let version = card_update::record_version(key, &updated.card)?;
            Ok(CardUpdateResponse::from_updated(&updated, &version))
        }
        Err(error) => {
            // Every rejection precedes the rewrite or is the rewrite failing
            // under its savepoint, so nothing was written to keep.
            release(uow).await;
            Err(ApiError::from(error))
        }
    }
}

/// COCRDUPC 9300-CHECK-CHANGE-IN-REC — V-COCRDUPC-001 (E-24)
///
/// The answer to a body whose `version` is absent or not a token
/// `VersionToken::parse` accepts: the token stands for the fetched record the
/// COBOL keeps in its COMMAREA, and its absence has no COBOL path or message.
/// 400, kind `Invalid`, an empty `message` and no field, as the row records.
fn missing_version() -> ApiError {
    ApiError::no_message(ErrorKind::Invalid, NO_MESSAGE, Vec::new())
}

/// Ends a unit of work that wrote nothing by rolling it back.
///
/// The request's answer is already decided, and a failed `ROLLBACK` cannot
/// change it: nothing was written, and the server rolls back a transaction
/// whose session ends. The failure is logged and the answer stands.
async fn release(uow: UnitOfWork) {
    if let Err(error) = uow.rollback().await {
        tracing::warn!(error = %error, "a unit of work that wrote nothing did not roll back");
    }
}

/// Records the request's keys on the handler span: the account number as
/// entered and the card number masked by [`masked_card_number`].
fn record_keys(account_id: Option<&str>, card_number: Option<&str>) {
    let span = Span::current();
    if let Some(account_id) = account_id {
        span.record("account_id", account_id);
    }
    if let Some(card_number) = card_number {
        span.record("card_number", masked_card_number(card_number));
    }
}

/// `text` as a span shows a card number: its first [`CARD_NUMBER_LEN`]
/// characters without trailing spaces, each but the last
/// [`CARD_NUMBER_SHOWN`] replaced by `*`, or all of them when it has no more
/// than that many: the mask of `CardNumber`'s `Debug`.
fn masked_card_number(text: &str) -> String {
    let kept: String = text.chars().take(CARD_NUMBER_LEN).collect();
    let trimmed = kept.trim_end_matches(' ');
    let count = trimmed.chars().count();
    let hidden = if count > CARD_NUMBER_SHOWN {
        count - CARD_NUMBER_SHOWN
    } else {
        count
    };
    std::iter::repeat_n('*', hidden)
        .chain(trimmed.chars().skip(hidden))
        .collect()
}

/// Records the span `outcome` of `result` and passes it on: `success` for an
/// answer, [`OUTCOME_FAILED`] for a 5xx error and [`OUTCOME_REJECTED`] for
/// any other.
fn finish<T>(result: Result<T, ApiError>, success: &'static str) -> Result<T, ApiError> {
    let outcome = match &result {
        Ok(_) => success,
        Err(error) if error.status().is_server_error() => OUTCOME_FAILED,
        Err(_) => OUTCOME_REJECTED,
    };
    Span::current().record("outcome", outcome);
    result
}
