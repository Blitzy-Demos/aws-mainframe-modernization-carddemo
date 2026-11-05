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
//! `CCUP`), the caller's `user_id`, the keys of the request and the `outcome`
//! recorded once the unit of work has ended: `ok` for an answered read,
//! `committed` for an update, `rejected` for a 4xx answer and `failed` for a
//! 5xx one. A request dropped before its answer, its client gone, records
//! `cancelled`: an open unit of work is rolled back as it is dropped, unless
//! its commit had already reached the server.
//!
//! The account and card numbers reach the span only through their key edits,
//! which the handler applies before anything else and the rule applies again:
//! `card_list::edit_account_filter` and `card_list::edit_card_filter` for the
//! filters of the list, `card_view::edit_account` and `card_view::edit_card`
//! (`2210-EDIT-ACCOUNT`, `2220-EDIT-CARD`) for the keys of the view and the
//! update. A key the edit accepts is recorded as `account_id`, its 11 digits,
//! or as `card_number`, masked to its last four digits as `CardNumber`'s
//! `Debug` shows it, so the JSON logs hold no full card number. A refused key
//! records no part of its text, only `account_id_length` or
//! `card_number_length`, its count of characters, and `account_id_rejected`
//! or `card_number_rejected`:
//!
//! | `*_rejected` | The edit refused |
//! |---|---|
//! | `blank` | A view or update key not entered: absent, empty, spaces, `*`, zeros or `LOW-VALUES` |
//! | `too_long` | Any other key longer than its map field, `ACCTSID` X(11) or `CARDSID` X(16) |
//! | `invalid` | Any other key: not a number of its field's width |
//!
//! A list filter left blank or zero filters nothing and records nothing.

use axum::Json;
use axum::extract::State;
use carddemo_domain::error::DomainError;
use carddemo_domain::messages;
use carddemo_domain::numeric::{AccountId, CardNumber};
use carddemo_domain::rules::{
    ErrorKind, ReturnMsg, VersionToken, card_list, card_update, card_view,
};
use carddemo_persistence::tx::UnitOfWork;
use tracing::Span;
use tracing::field::Empty;

use crate::auth::AuthUser;
use crate::dto::cards::{
    CardListPageQuery, CardListQuery, CardSummary, CardUpdateRequest, CardUpdateResponse,
    CardViewQuery, CardViewResponse,
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

/// `CARDSID PIC X(16)`, the width of `CARD-NUM` [app/cpy/CVACT02Y.cpy:L5]: a
/// refused card number longer than this is [`KEY_TOO_LONG`].
const CARD_NUMBER_LEN: usize = 16;

/// The trailing characters of a card number a span shows unmasked.
const CARD_NUMBER_SHOWN: usize = 4;

/// `*_rejected` of a view or update key the edit reads as not entered.
const KEY_BLANK: &str = "blank";

/// `*_rejected` of any other refused key longer than its map field.
const KEY_TOO_LONG: &str = "too_long";

/// `*_rejected` of any other refused key.
const KEY_INVALID: &str = "invalid";

/// The span fields of one key: its value, and the category and length of a
/// refused key.
struct KeyFields {
    /// The key as the edit accepted it.
    value: &'static str,
    /// The category of a refusal.
    rejected: &'static str,
    /// The count of characters of a refused key.
    length: &'static str,
}

/// The span fields of the account number.
const ACCOUNT_FIELDS: KeyFields = KeyFields {
    value: "account_id",
    rejected: "account_id_rejected",
    length: "account_id_length",
};

/// The span fields of the card number.
const CARD_FIELDS: KeyFields = KeyFields {
    value: "card_number",
    rejected: "card_number_rejected",
    length: "card_number_length",
};

/// One key of a request as the handler span records it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SpanKey {
    /// A list filter left blank or zero, which filters nothing: nothing is
    /// recorded.
    Unfiltered,
    /// A key the edit accepted, as the span records it: an account number's
    /// 11 digits, or a card number masked by [`masked_card_number`].
    Accepted(String),
    /// A key the edit refused: `category` is recorded as `*_rejected` and
    /// `length`, the key's count of characters, as `*_length`.
    Refused {
        category: &'static str,
        length: usize,
    },
}

impl SpanKey {
    /// Records this key on `span` under `fields`; the fields it does not set
    /// stay empty.
    fn record(self, span: &Span, fields: &KeyFields) {
        match self {
            Self::Unfiltered => {}
            Self::Accepted(value) => {
                span.record(fields.value, value);
            }
            Self::Refused { category, length } => {
                span.record(fields.rejected, category)
                    .record(fields.length, length);
            }
        }
    }
}

/// COCRDLIC 0000-MAIN, 2200-EDIT-INPUTS, 9000-READ-FORWARD,
/// 9100-READ-BACKWARDS, 9500-FILTER-RECORDS, 1000-SEND-MAP,
/// 1400-SETUP-MESSAGE, 1500-SEND-SCREEN — P-COCRDLIC-003 to P-COCRDLIC-006,
/// P-COCRDLIC-011 to P-COCRDLIC-015, P-COCRDLIC-017, P-COCRDLIC-020,
/// P-COCRDLIC-021, P-COCRDLIC-023, V-COCRDLIC-001, V-COCRDLIC-002,
/// V-COCRDLIC-005 to V-COCRDLIC-010
///
/// One page of the card list in `CARDDAT` key order: forward (ENTER, PF8)
/// from `start_key` inclusively, or backward (PF7) from the first card of the
/// page shown, `WS-MAX-SCREEN-LINES` (7) rows unless `page_size` says
/// otherwise. Without `start_key` both directions read the first page from
/// the lowest card number: backward is PF7 on the first entry, which re-reads
/// that page with 'NO PREVIOUS PAGES TO DISPLAY'
/// [app/cbl/COCRDLIC.cbl:L439-454]. Because PF7 stays live on the first
/// page, every non-empty page, the first included, carries its first card as
/// `prev_key`, and the backward request from the first page's `prev_key`
/// answers as that PF7 does (E-54); an empty page read from a `start_key`
/// carries that key. The `account_id` and `card_number` filters reach the
/// rule as entered, so a filter that is neither blank nor a number of its
/// width is answered with the `2210-EDIT-ACCOUNT` or `2220-EDIT-CARD` text.
/// An empty page is a page: 200 with `items: []` and the text the COBOL shows
/// with it.
///
/// # Errors
///
/// - 400 with an empty `message` when a filter is longer than its map field.
/// - 400 with 'ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER' at
///   `ACCTSID` or 'CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER' at
///   `CARDSID` (V-COCRDLIC-001, V-COCRDLIC-002).
/// - 400 with code `INVALID` when the page parameters do not deserialize,
///   and with code `INVALID`, an empty `message` and no field when
///   `start_key` is longer than the 16 characters of a card number, holds a
///   character outside ASCII or holds a NUL character, none of which a card
///   number holds; no card is read.
/// - 401 without a valid bearer token.
/// - 500 with the `CARDDAT` file-error text at `ACCTSID` when a card read
///   fails (V-COCRDLIC-008, V-COCRDLIC-010), or 'Unexpected error' when the
///   database cannot be reached.
#[utoipa::path(
    get,
    path = "/cards",
    tag = "cards",
    params(CardListQuery, CardListPageQuery),
    responses(
        (status = 200, description = "One page of cards in card-number order, with the keys of its neighbours and the COBOL page text", body = Page<CardSummary>),
        (status = 400, description = "A filter or page parameter the COBOL edits reject, or a start key longer than 16 characters, not ASCII or holding a NUL character", body = ErrorBody),
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
        account_id_rejected = Empty,
        account_id_length = Empty,
        card_number = Empty,
        card_number_rejected = Empty,
        card_number_length = Empty,
        outcome = Empty,
    ),
)]
pub async fn list_cards(
    user: AuthUser,
    State(state): State<AppState>,
    ApiQuery(filters): ApiQuery<CardListQuery>,
    ApiQuery(page): ApiQuery<CardListPageQuery>,
) -> Result<Json<Page<CardSummary>>, ApiError> {
    finish(
        crate::cancellable(Span::current(), list(&state, filters, page)).await,
        OUTCOME_OK,
    )
    .map(Json)
}

/// COCRDSLC 2000-PROCESS-INPUTS, 9000-READ-DATA, 1200-SETUP-SCREEN-VARS,
/// 1400-SEND-SCREEN, COMMON-RETURN — P-COCRDSLC-002 to P-COCRDSLC-007,
/// P-COCRDSLC-010, P-COCRDSLC-012 to P-COCRDSLC-016, V-COCRDSLC-001 to
/// V-COCRDSLC-007
///
/// The card whose number is the `{id}` path segment, with the account number
/// of the `account_id` query parameter: both keys are edited as
/// `2210-EDIT-ACCOUNT` and `2220-EDIT-CARD` edit them, an absent account
/// being a blank one, and the card is then read by its number alone (E-55).
/// The response carries the card's version token, which a following
/// `PUT /cards/{id}` sends back, never the verification code, and in
/// `message` the information line of a found card, '   Displaying requested
/// details' with its three leading spaces (P-COCRDSLC-010).
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
        (status = 200, description = "The card with its version token and the COBOL information text", body = CardViewResponse),
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
        account_id_rejected = Empty,
        account_id_length = Empty,
        card_number = Empty,
        card_number_rejected = Empty,
        card_number_length = Empty,
        outcome = Empty,
    ),
)]
pub async fn get_card(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(card_number): ApiPath<String>,
    ApiQuery(query): ApiQuery<CardViewQuery>,
) -> Result<Json<CardViewResponse>, ApiError> {
    finish(
        crate::cancellable(Span::current(), view(&state, card_number, query)).await,
        OUTCOME_OK,
    )
    .map(Json)
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS, 2000-DECIDE-ACTION, 9000-READ-DATA,
/// 9200-WRITE-PROCESSING, 9300-CHECK-CHANGE-IN-REC, 3000-SEND-MAP,
/// 3250-SETUP-INFOMSG, 3400-SEND-SCREEN, COMMON-RETURN — P-COCRDUPC-002 to
/// P-COCRDUPC-008, P-COCRDUPC-010 to P-COCRDUPC-017, P-COCRDUPC-019,
/// P-COCRDUPC-021 to P-COCRDUPC-023, P-COCRDUPC-028, V-COCRDUPC-001 to
/// V-COCRDUPC-020
///
/// Updates the card whose number is the `{id}` path segment from the body's
/// map fields, in one request: the key edits, the fetch, the comparison with
/// the fetched card and the name, status, expiry month and year edits, then
/// the locked read, the change check against the body's `version` and the
/// rewrite. 200 on success, with 'Changes committed to database' after a
/// rewrite or 'No change detected with respect to values fetched.' when the
/// body changes nothing, and the card as it now stands with its new token.
/// `version` stands for the `CCUP-OLD-DETAILS` copy that `COMMON-RETURN`
/// keeps in the COMMAREA (E-24).
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
        account_id_rejected = Empty,
        account_id_length = Empty,
        card_number = Empty,
        card_number_rejected = Empty,
        card_number_length = Empty,
        outcome = Empty,
    ),
)]
pub async fn update_card(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(card_number): ApiPath<String>,
    ApiJson(body): ApiJson<CardUpdateRequest>,
) -> Result<Json<CardUpdateResponse>, ApiError> {
    finish(
        crate::cancellable(Span::current(), update(&state, card_number, body)).await,
        OUTCOME_COMMITTED,
    )
    .map(Json)
}

/// The `GET /cards` page for `filters` and `page`, read in a unit of work
/// that writes nothing.
async fn list(
    state: &AppState,
    filters: CardListQuery,
    page: CardListPageQuery,
) -> Result<Page<CardSummary>, ApiError> {
    record_keys(list_keys(
        filters.account_id.as_deref(),
        filters.card_number.as_deref(),
    ));
    filters.check()?;
    page.check()?;
    let (start_key, direction, page_size) = PageQuery::from(page).resolve(card_list::SCREEN_ROWS);
    let input = filters.into_input(start_key, direction, page_size)?;
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
) -> Result<CardViewResponse, ApiError> {
    record_keys(entered_keys(query.account_id.as_deref(), &card_number));
    query.check()?;
    let input = query.into_input(card_number);
    let mut uow = state.db().begin().await?;
    let viewed = card_view::view(&mut uow.store(), state.version_key(), input).await;
    release(uow).await;
    Ok(CardViewResponse::from_view(&viewed?))
}

/// The `PUT /cards/{id}` answer for `card_number` and `body`: the whole
/// `COCRDUPC` dialogue in one unit of work, committed when the rule succeeds
/// and rolled back when it rejects the request.
async fn update(
    state: &AppState,
    card_number: String,
    body: CardUpdateRequest,
) -> Result<CardUpdateResponse, ApiError> {
    record_keys(entered_keys(body.account_id.as_deref(), &card_number));
    body.check(&card_number)?;
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
            // The rewrite stands once committed. A `COMMIT` the server
            // rejects undoes it; one whose answer is lost, its connection
            // gone, may have committed it, so the card is read again before
            // a retry. The new token is computed from the committed record.
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

/// Records `keys`, the account and card numbers of the request as
/// [`list_keys`] or [`entered_keys`] gives them, on the handler span.
fn record_keys((account, card): (SpanKey, SpanKey)) {
    let span = Span::current();
    account.record(&span, &ACCOUNT_FIELDS);
    card.record(&span, &CARD_FIELDS);
}

/// COCRDLIC 2210-EDIT-ACCOUNT, 2220-EDIT-CARD — P-COCRDLIC-005,
/// P-COCRDLIC-006
///
/// The filters `account_id` and `card_number` of a `GET /cards` request as
/// the span records them, through `card_list::edit_account_filter` and
/// `card_list::edit_card_filter`.
fn list_keys(account_id: Option<&str>, card_number: Option<&str>) -> (SpanKey, SpanKey) {
    let account = card_list::edit_account_filter(account_id)
        .map(|filter| filter.as_ref().map(ToString::to_string));
    let card = card_list::edit_card_filter(card_number)
        .map(|filter| filter.as_ref().map(masked_card_number));
    (
        filter_key(account_id, AccountId::WIDTH, account),
        filter_key(card_number, CARD_NUMBER_LEN, card),
    )
}

/// The list filter `entered`, of a map field `width` characters wide, as the
/// span records it, from `edited`: its edit, with an accepted key in the
/// span's form.
fn filter_key(
    entered: Option<&str>,
    width: usize,
    edited: Result<Option<String>, DomainError>,
) -> SpanKey {
    match edited {
        Ok(Some(key)) => SpanKey::Accepted(key),
        Ok(None) => SpanKey::Unfiltered,
        Err(_) => refused(entered, width, false),
    }
}

/// COCRDSLC 2210-EDIT-ACCOUNT, 2220-EDIT-CARD — P-COCRDSLC-002,
/// P-COCRDSLC-003
///
/// The account number `account_id`, absent being blank, and the card number
/// `card_number` of a `GET` or `PUT /cards/{id}` request as the span records
/// them, through `card_view::edit_account` and `card_view::edit_card`.
///
/// `COCRDUPC`'s `1210-EDIT-ACCOUNT` and `1220-EDIT-CARD` accept the same keys,
/// and read the same ones as not entered, for every value within its map
/// field, and `CardUpdateRequest::check` refuses any longer value before the
/// rule; these edits therefore describe the update's keys too.
fn entered_keys(account_id: Option<&str>, card_number: &str) -> (SpanKey, SpanKey) {
    let mut msg = ReturnMsg::default();
    let account = match card_view::edit_account(account_id, &mut msg) {
        Some(account) => SpanKey::Accepted(account.to_string()),
        None => refused(
            account_id,
            AccountId::WIDTH,
            msg.message() == Some(messages::ACCOUNT_NUMBER_NOT_PROVIDED),
        ),
    };
    let mut msg = ReturnMsg::default();
    let card = match card_view::edit_card(Some(card_number), &mut msg) {
        Some(card) => SpanKey::Accepted(masked_card_number(&card)),
        None => refused(
            Some(card_number),
            CARD_NUMBER_LEN,
            msg.message() == Some(messages::CARD_NUMBER_NOT_PROVIDED),
        ),
    };
    (account, card)
}

/// The key `entered`, of a map field `width` characters wide, that its edit
/// refused, as the span records it: [`KEY_BLANK`] when `blank`, the edit
/// having read it as not entered, else [`KEY_TOO_LONG`] when it holds more
/// than `width` characters, else [`KEY_INVALID`]; with its count of
/// characters, an absent key having none. No part of its text is kept.
fn refused(entered: Option<&str>, width: usize, blank: bool) -> SpanKey {
    let length = entered.map_or(0, |text| text.chars().count());
    let category = if blank {
        KEY_BLANK
    } else if length > width {
        KEY_TOO_LONG
    } else {
        KEY_INVALID
    };
    SpanKey::Refused { category, length }
}

/// `card`, a card number its edit accepted, as a span shows it: its text
/// without trailing spaces, each character but the last
/// [`CARD_NUMBER_SHOWN`] replaced by `*`, or all of them when it has no more
/// than that many: the mask of `CardNumber`'s `Debug`. The CT02 span records
/// its card key through it too.
pub(super) fn masked_card_number(card: &CardNumber) -> String {
    let trimmed = card.trimmed();
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

#[cfg(test)]
mod tests {
    use super::{KEY_BLANK, KEY_INVALID, KEY_TOO_LONG, SpanKey, entered_keys, list_keys};

    /// The first fixture card, of account 00000000050 (P-CVACT02Y-009)
    /// [app/data/ASCII/carddata.txt:L1].
    const CARD: &str = "0500024453765740";

    /// [`CARD`] as the span shows it.
    const MASKED_CARD: &str = "************5740";

    /// The account of [`CARD`].
    const ACCOUNT: &str = "00000000050";

    fn accepted(value: &str) -> SpanKey {
        SpanKey::Accepted(value.to_owned())
    }

    const fn refused(category: &'static str, length: usize) -> SpanKey {
        SpanKey::Refused { category, length }
    }

    #[test]
    fn list_filters_record_their_edited_keys_or_only_the_refusal_class_and_length() {
        // (account filter, card filter) -> (account key, card key)
        let cases = [
            (
                Some(ACCOUNT),
                Some(CARD),
                accepted(ACCOUNT),
                accepted(MASKED_CARD),
            ),
            (None, None, SpanKey::Unfiltered, SpanKey::Unfiltered),
            (
                Some(""),
                Some("   "),
                SpanKey::Unfiltered,
                SpanKey::Unfiltered,
            ),
            (
                Some("00000000000"),
                Some("0000000000000000"),
                SpanKey::Unfiltered,
                SpanKey::Unfiltered,
            ),
            (
                Some("*"),
                Some("0500024453765"),
                refused(KEY_INVALID, 1),
                refused(KEY_INVALID, 13),
            ),
            (
                Some("0000000005A"),
                Some("050002445376574A"),
                refused(KEY_INVALID, 11),
                refused(KEY_INVALID, 16),
            ),
            (
                Some("000000000501"),
                Some("05000244537657401"),
                refused(KEY_TOO_LONG, 12),
                refused(KEY_TOO_LONG, 17),
            ),
        ];
        for (account, card, account_key, card_key) in cases {
            assert_eq!(
                list_keys(account, card),
                (account_key, card_key),
                "{account:?}, {card:?}"
            );
        }
    }

    #[test]
    fn view_and_update_keys_record_their_edited_keys_or_only_the_refusal_class_and_length() {
        // (account number, card number) -> (account key, card key)
        let cases = [
            (
                Some(ACCOUNT),
                CARD,
                accepted(ACCOUNT),
                accepted(MASKED_CARD),
            ),
            (None, "", refused(KEY_BLANK, 0), refused(KEY_BLANK, 0)),
            (Some("*"), "*", refused(KEY_BLANK, 1), refused(KEY_BLANK, 1)),
            (
                Some("00000000000"),
                "0000000000000000",
                refused(KEY_BLANK, 11),
                refused(KEY_BLANK, 16),
            ),
            (
                Some("           "),
                "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
                refused(KEY_BLANK, 11),
                refused(KEY_BLANK, 16),
            ),
            (
                Some("0000000005A"),
                "050002445376574A",
                refused(KEY_INVALID, 11),
                refused(KEY_INVALID, 16),
            ),
            (
                Some("50"),
                "5740",
                refused(KEY_INVALID, 2),
                refused(KEY_INVALID, 4),
            ),
            (
                Some("000000000501"),
                "05000244537657401",
                refused(KEY_TOO_LONG, 12),
                refused(KEY_TOO_LONG, 17),
            ),
            (
                Some("4111111111111111"),
                CARD,
                refused(KEY_TOO_LONG, 16),
                accepted(MASKED_CARD),
            ),
        ];
        for (account, card, account_key, card_key) in cases {
            assert_eq!(
                entered_keys(account, card),
                (account_key, card_key),
                "{account:?}, {card:?}"
            );
        }
    }
}
