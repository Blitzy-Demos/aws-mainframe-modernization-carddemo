//! CAVW · `COACTVWC` · map `COACTVW`: `GET /accounts/{id}`, the account view.
//! CAUP · `COACTUPC` · map `COACTUP`: `PUT /accounts/{id}`, the account update.
//!
//! Each handler is the HTTP adapter of one ported program. It opens the unit
//! of work, hands its store to `carddemo_domain::rules::account_view` or
//! `carddemo_domain::rules::account_update`, ends the unit as the program's
//! write rules require and answers with the rule's outcome. The edits, reads,
//! comparisons and writes, and every message text, belong to the rules.
//!
//! # Identity
//!
//! Both routes take [`AuthUser`]. Any signed-on user may view and update any
//! account: `USRSEC` links users to no account [app/cpy/CSUSR01Y.cpy], and
//! neither program checks the user type. The acting user is the verified
//! token's `sub`; no path, query or body field names it.
//!
//! # Version token
//!
//! Between its fetch and its write turns `COACTUPC` keeps the fetched records
//! in its COMMAREA, and `9700-CHECK-CHANGE-IN-REC` compares the records it
//! locks with that copy [app/cbl/COACTUPC.cbl:L4109-4193]. The stateless API
//! keeps a [`VersionToken`] instead: `GET` returns the token of the records it
//! read, `PUT` echoes it in `version`, and `account_update::apply` recomputes
//! it from the rows it locks. A body without a well-formed token has no COBOL
//! path or message and is answered 400 with [`MISSING_VERSION_MESSAGE`]
//! before any COBOL edit (V-COACTUPC-001, E-24). Tokens are keyed with the
//! process's [`AppState::version_key`], the [`VersionKey`] built from
//! `CARDDEMO_JWT_SECRET` at start-up and shared with `GET /cards/{id}`.
//!
//! # Write persistence
//!
//! `ACCTDAT` and `CUSTDAT` are `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L9]
//! [app/csd/CARDDEMO.CSD:L59], so the account rewrite survives a failed
//! customer rewrite although `COACTUPC` then issues `SYNCPOINT ROLLBACK`
//! [app/cbl/COACTUPC.cbl:L4065-4103] (P-COACTUPC-014). Each write runs under
//! its own savepoint in `carddemo_persistence::tx`, and the update commits
//! whatever `apply` returns. A request that writes nothing (a view, a
//! rejected edit, no change detected) rolls its unit of work back.
//!
//! # Telemetry
//!
//! Each handler runs in one span with `transaction` (`CAVW` or `CAUP`),
//! `user_id`, the account key and `outcome`.
//!
//! The `{id}` segment reaches the span only through the program's key edit,
//! `account_view::edit_account_id` for `CAVW` and
//! `account_update::edit_account_id` for `CAUP`, which the handler applies
//! before anything else; the rule applies the same edit again. `CAVW`'s edit
//! refuses a key longer than the 11 characters of `ACCTSID` itself. `CAUP`'s
//! would `MOVE` one into `CC-ACCT-ID X(11)` and read the account its first 11
//! characters name, so a longer `CAUP` segment, the one
//! [`AccountUpdateRequest::check`] refuses, is recorded as `too_long` without
//! that edit. A key the edit accepts is recorded as `account_id`, the 11
//! digits of the account the request addresses. A refused key leaves
//! `account_id` empty and records no part of its text, only
//! `account_id_length`, its count of characters, and `account_id_rejected`:
//!
//! | `account_id_rejected` | Recorded for |
//! |---|---|
//! | `too_long` | A key longer than the 11 characters of `ACCTSID`, whatever it holds |
//! | `blank` | Any other key not entered: empty, spaces, `*` or `LOW-VALUES` |
//! | `invalid` | Any other key: not 11 digits, or all zeros |
//!
//! `outcome` is recorded once the unit of work has ended:
//!
//! | `outcome` | Meaning |
//! |---|---|
//! | `ok` | 200 with nothing written: the account shown, or no change detected |
//! | `rejected` | An error answer with nothing written; any unit of work was rolled back |
//! | `committed` | The update's writes ran and committed; the status tells whether they all succeeded |
//! | `rolled_back` | The commit failed, so the server discarded the update's writes |

use axum::Json;
use axum::extract::State;
use carddemo_domain::clock::Clock;
use carddemo_domain::error::{DomainError, ErrorKind};
use carddemo_domain::messages;
use carddemo_domain::numeric::{AccountId, CustomerId};
use carddemo_domain::rules::account_update::{
    self, AccountEdit, AccountSnapshot, AccountUpdated, ValidatedAccountChanges,
};
use carddemo_domain::rules::account_view::{self, AccountView, AccountViewInput};
use carddemo_domain::rules::{VersionKey, VersionToken};
use carddemo_persistence::UnitOfWork;
use tracing::Span;
use tracing::field::display;

use crate::auth::AuthUser;
use crate::dto::accounts::{AccountResponse, AccountUpdateRequest, AccountUpdateResponse};
use crate::error::{ApiError, ApiJson, ApiPath, ErrorBody};
use crate::state::AppState;

/// The `message` of the 400 answer to a `PUT /accounts/{id}` body whose
/// `version` is missing or is not 16 lower-case hex digits (V-COACTUPC-001).
///
/// Empty: the validation audit records the path as `no-message (E-24)`, and
/// E-24 names no text, so `carddemo_domain::messages` holds no constant for
/// it.
pub const MISSING_VERSION_MESSAGE: &str = "";

/// `outcome` of a 200 answer that wrote nothing.
const OUTCOME_OK: &str = "ok";

/// `outcome` of an error answer that wrote nothing.
const OUTCOME_REJECTED: &str = "rejected";

/// `outcome` of an update whose writes ran and committed.
const OUTCOME_COMMITTED: &str = "committed";

/// `outcome` of an update whose commit failed.
const OUTCOME_ROLLED_BACK: &str = "rolled_back";

/// `account_id_rejected` of a key of at most the 11 characters of `ACCTSID`
/// that the edit reads as not entered.
const KEY_BLANK: &str = "blank";

/// `account_id_rejected` of a key longer than `ACCTSID`, whatever it holds.
const KEY_TOO_LONG: &str = "too_long";

/// `account_id_rejected` of any other refused key.
const KEY_INVALID: &str = "invalid";

/// The `{id}` key as the handler span records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpanKey {
    /// The account number the key edit accepted, recorded as `account_id`.
    Accepted(AccountId),
    /// A key the edit refused: `category` is recorded as
    /// `account_id_rejected` and `length`, the key's count of characters, as
    /// `account_id_length`.
    Refused {
        category: &'static str,
        length: usize,
    },
}

/// COACTVWC 2000-PROCESS-INPUTS, 9000-READ-ACCT, 1200-SETUP-SCREEN-VARS,
/// 1400-SEND-SCREEN, COMMON-RETURN — P-COACTVWC-004 to P-COACTVWC-012,
/// P-COACTVWC-014, P-COACTVWC-015, P-COACTVWC-018 to P-COACTVWC-021,
/// P-COACTVWC-025, P-COACTVWC-030, V-COACTVWC-001 to V-COACTVWC-008
///
/// `GET /accounts/{id}` (CAVW): the account number `{id}` as entered, edited
/// and read by `account_view::view` (the card cross-reference through
/// `CXACAIX`, then the account, then the customer) in a unit of work that is
/// rolled back once the reads are done. The answer carries the account and
/// customer records, the account's first card number and the version token
/// `PUT /accounts/{id}` echoes back. A successful view shows no message
/// (P-COACTVWC-025).
///
/// # Errors
///
/// - 400 at `ACCTSID` when the number is blank, spaces or `*`
///   (V-COACTVWC-001), or is not 11 digits or is zero (V-COACTVWC-002), a
///   segment longer than its published 11-character `maxLength` included.
/// - 404 at `ACCTSID` when the cross-reference, the account or the customer
///   is not on file (V-COACTVWC-003, V-COACTVWC-005, V-COACTVWC-007).
/// - 500 at `ACCTSID` with the file-error text when a read fails otherwise
///   (V-COACTVWC-004, V-COACTVWC-006, V-COACTVWC-008), and 500 with
///   [`messages::UNEXPECTED_ERROR`] when the database is unavailable.
/// - 401 from [`AuthUser`] for a request without a valid bearer token.
#[utoipa::path(
    get,
    path = "/accounts/{id}",
    tag = "accounts",
    params(("id" = String, Path, description = "ACCT-ID 9(11) as entered", max_length = 11)),
    responses(
        (status = 200, description = "The account, its customer, the account's first card \
            number and the version token (P-COACTVWC-011)", body = AccountResponse),
        (status = 400, description = "Account number blank, not 11 digits or zero \
            (V-COACTVWC-001, V-COACTVWC-002)", body = ErrorBody),
        (status = 401, description = "Missing, malformed, badly signed or expired bearer \
            token", body = ErrorBody),
        (status = 404, description = "No cross-reference, account or customer record \
            (V-COACTVWC-003, V-COACTVWC-005, V-COACTVWC-007)", body = ErrorBody),
        (status = 500, description = "A read failed (V-COACTVWC-004, V-COACTVWC-006, \
            V-COACTVWC-008)", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = "CAVW",
        user_id = %user.user_id,
        account_id = tracing::field::Empty,
        account_id_rejected = tracing::field::Empty,
        account_id_length = tracing::field::Empty,
        outcome = tracing::field::Empty,
    )
)]
pub async fn get_account(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<AccountResponse>, ApiError> {
    record_key(span_key(&id, &account_view::edit_account_id(Some(&id))));
    let key = state.version_key();
    let mut uow = state.db().begin().await.map_err(rejected)?;
    let input = AccountViewInput {
        account_id: Some(id),
    };
    let viewed = account_view::view(&mut uow.store(), key, input).await;
    end_read_only(uow).await;
    let view = viewed.map_err(rejected)?;
    record_outcome(OUTCOME_OK);
    Ok(Json(AccountResponse::from_view(&view)))
}

/// COACTUPC 0000-MAIN, 2000-DECIDE-ACTION, 1200-EDIT-MAP-INPUTS /
/// 9600-WRITE-PROCESSING — P-COACTUPC-003 to P-COACTUPC-015, P-COACTUPC-018
/// to P-COACTUPC-033, P-COACTUPC-043 to P-COACTUPC-045, P-COACTUPC-047 to
/// P-COACTUPC-050, V-COACTUPC-001 to V-COACTUPC-128
///
/// `PUT /accounts/{id}` (CAUP): the fetch, the edits and the write of the
/// three `COACTUPC` turns in one request, in the order the validation audit
/// ranks their messages:
///
/// 1. the BMS field lengths of the body and of the `{id}` segment
///    (`ACCTSIDI X(11)`), then the body's `version` token (V-COACTUPC-001),
///    before any COBOL edit and before the unit of work opens;
/// 2. the account number `{id}` and the fetch, `load_snapshot`;
/// 3. `account_update::validate` against the fetched records, with the
///    processing clock's date as `FUNCTION CURRENT-DATE`: the comparison
///    with the fetched values, then every `1200-EDIT-MAP-INPUTS` edit in
///    COBOL order, the first failing edit supplying the message;
/// 4. for a change that passed every edit, the locked reads, the change
///    check and the two rewrites (`persist`).
///
/// Steps 1 to 3 write nothing, and their unit of work is rolled back. The
/// answer is 200 with the records as they now stand, their version token and
/// the text `COACTUPC` shows: [`messages::CHANGES_COMMITTED_TO_DATABASE`]
/// after both rewrites (P-COACTUPC-015), or
/// [`messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`] when the
/// body repeats the fetched values (V-COACTUPC-010).
///
/// # Errors
///
/// - 400 with [`MISSING_VERSION_MESSAGE`] and no fields when `version` is
///   missing or malformed (V-COACTUPC-001), and 400 with no message naming
///   each body field longer than its BMS field and `ACCTSID` for an `{id}`
///   segment longer than 11 characters, which `1210-EDIT-ACCOUNT` would
///   otherwise see cut to the account its first 11 characters name.
/// - 400 at `ACCTSID` for a blank, non-numeric or zero account number
///   (V-COACTUPC-002, V-COACTUPC-003), and 400 at the flagged fields for the
///   first failing field edit (V-COACTUPC-011 to V-COACTUPC-123).
/// - 404 or 500 at `ACCTSID` when the fetch finds no record or a read fails
///   (V-COACTUPC-004 to V-COACTUPC-009).
/// - 409 at `ACCTSID` with [`messages::RECORD_CHANGED_BY_SOME_ONE_ELSE`]
///   when the locked records differ from the token's (V-COACTUPC-126).
/// - 500 at `ACCTSID` when a lock or a rewrite fails (V-COACTUPC-124,
///   V-COACTUPC-125, V-COACTUPC-127, V-COACTUPC-128); after a failed customer
///   rewrite the account rewrite stays committed (P-COACTUPC-014).
/// - 500 with [`messages::UNEXPECTED_ERROR`] for an input class held at
///   `halt` (E-26, E-40, E-47, E-74), and when the database or the commit
///   fails.
/// - 401 from [`AuthUser`] for a request without a valid bearer token.
// This text reaches web/src/api/schema.d.ts, whose hygiene scan reads a word
// `fetch` followed by `(` as a call; the step-2 wording keeps them apart.
#[utoipa::path(
    put,
    path = "/accounts/{id}",
    tag = "accounts",
    params(("id" = String, Path, description = "ACCT-ID 9(11) as entered", max_length = 11)),
    request_body = AccountUpdateRequest,
    responses(
        (status = 200, description = "Changes committed (P-COACTUPC-015), or no change \
            detected (V-COACTUPC-010): the records as they now stand",
            body = AccountUpdateResponse),
        (status = 400, description = "A body field longer than its BMS field or an id longer \
            than 11 characters (no message, fields name each), a missing or malformed \
            version token (V-COACTUPC-001), or a failed key or field edit \
            (1210-EDIT-ACCOUNT: V-COACTUPC-002, V-COACTUPC-003; V-COACTUPC-011 to \
            V-COACTUPC-123)",
            body = ErrorBody),
        (status = 401, description = "Missing, malformed, badly signed or expired bearer \
            token", body = ErrorBody),
        (status = 404, description = "No cross-reference, account or customer record \
            (V-COACTUPC-004, V-COACTUPC-006, V-COACTUPC-008)", body = ErrorBody),
        (status = 409, description = "The record changed since it was fetched \
            (V-COACTUPC-126)", body = ErrorBody),
        (status = 500, description = "A read, lock or rewrite failed (V-COACTUPC-005, \
            V-COACTUPC-007, V-COACTUPC-009, V-COACTUPC-124, V-COACTUPC-125, \
            V-COACTUPC-127, V-COACTUPC-128), or an input class held at halt",
            body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = "CAUP",
        user_id = %user.user_id,
        account_id = tracing::field::Empty,
        account_id_rejected = tracing::field::Empty,
        account_id_length = tracing::field::Empty,
        outcome = tracing::field::Empty,
    )
)]
pub async fn update_account(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<AccountUpdateRequest>,
) -> Result<Json<AccountUpdateResponse>, ApiError> {
    record_key(update_span_key(&id));
    body.check(&id).map_err(rejected)?;
    let version = request_version(body.version.as_deref()).map_err(rejected)?;
    let input = body.into_input(id, version);
    let today = state.clock().now().date_naive();
    let key = state.version_key();
    let mut uow = state.db().begin().await.map_err(rejected)?;
    let (account_id, view) = match load_snapshot(&mut uow, key, &input.account_id).await {
        Ok(loaded) => loaded,
        Err(err) => return Err(refuse(uow, err).await),
    };
    let AccountView {
        account,
        customer,
        card_xref,
        version: fetched_version,
    } = view;
    let customer_id = customer.cust_id;
    let snapshot = AccountSnapshot { account, customer };
    let changes = match account_update::validate(&snapshot, &input, today) {
        Ok(AccountEdit::Changed(changes)) => changes,
        Ok(AccountEdit::NoChange) => {
            end_read_only(uow).await;
            record_outcome(OUTCOME_OK);
            let AccountSnapshot { account, customer } = snapshot;
            let unchanged = AccountView {
                account,
                customer,
                card_xref,
                version: fetched_version,
            };
            return Ok(Json(AccountUpdateResponse {
                account: AccountResponse::from_view(&unchanged),
                message: messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED.to_owned(),
            }));
        }
        Err(err) => return Err(refuse(uow, err).await),
    };
    let updated = persist(uow, key, account_id, customer_id, &input.version, *changes).await?;
    let AccountUpdated {
        account,
        customer,
        message,
    } = updated;
    let version = account_update::record_version(key, &account, &customer)?;
    let rewritten = AccountView {
        account,
        customer,
        card_xref,
        version,
    };
    Ok(Json(AccountUpdateResponse {
        account: AccountResponse::from_view(&rewritten),
        message: message.to_owned(),
    }))
}

/// COACTUPC 1210-EDIT-ACCOUNT, 9000-READ-ACCT, 9500-STORE-FETCHED-DATA —
/// P-COACTUPC-003, P-COACTUPC-004, P-COACTUPC-018, P-COACTUPC-043 to
/// P-COACTUPC-045, P-COACTUPC-050, V-COACTUPC-002 to V-COACTUPC-009
///
/// The account number `account_id` as entered, edited by
/// `account_update::edit_account_id` with the `COACTUPC` texts, and the
/// fetch of the records `account_update::validate` compares the body with.
/// [`AccountUpdateRequest::check`] has already refused a segment longer than
/// 11 characters, so the `MOVE` into `CC-ACCT-ID X(11)` that starts the edit
/// never cuts one down to another account's number.
///
/// The fetch is `account_view::view` of the edited key: `COACTUPC`
/// `9000-READ-ACCT` performs the `COACTVWC` reads with the same texts, kinds
/// and `ACCTSID` field (P-COACTUPC-004), and only that read also returns the
/// cross-reference whose card number the answer carries. The edited key is 11
/// non-zero digits, which the `COACTVWC` key edit always passes, so the
/// `COACTUPC` texts are the only key texts a `PUT` can answer with.
///
/// # Errors
///
/// The key edit's error (V-COACTUPC-002, V-COACTUPC-003), then the first
/// read failure (V-COACTUPC-004 to V-COACTUPC-009), each at `ACCTSID`.
async fn load_snapshot(
    uow: &mut UnitOfWork,
    key: &VersionKey,
    account_id: &str,
) -> Result<(AccountId, AccountView), DomainError> {
    let account_id = account_update::edit_account_id(account_id)?;
    let input = AccountViewInput {
        account_id: Some(account_id.to_string()),
    };
    let view = account_view::view(&mut uow.store(), key, input).await?;
    Ok((account_id, view))
}

/// COACTUPC 9600-WRITE-PROCESSING, 9700-CHECK-CHANGE-IN-REC,
/// 3250-SETUP-INFOMSG — P-COACTUPC-009 to P-COACTUPC-015, P-COACTUPC-048,
/// V-COACTUPC-124 to V-COACTUPC-128
///
/// The PF5 write of `changes` by `account_update::apply`: the locked reads
/// of the account `account_id` and the customer `customer_id`, the change
/// check against `expected`, then the account and customer rewrites. The
/// unit of work is committed whatever `apply` returns. A failed customer
/// rewrite must leave the account rewrite in place, because `COACTUPC`'s
/// `SYNCPOINT ROLLBACK` does not undo a `RECOVERY(NONE)` write
/// (P-COACTUPC-014). Each write ran under its own savepoint, so the commit
/// keeps exactly the writes that succeeded, and a lock failure or a changed
/// record commits a unit with no writes.
///
/// # Errors
///
/// - `apply`'s error with its mapped status once the commit succeeded:
///   409 for a changed record, 500 with the lock or rewrite text otherwise.
/// - 500 with [`messages::UNEXPECTED_ERROR`] when the commit fails; the
///   server then discards every write of the unit.
async fn persist(
    mut uow: UnitOfWork,
    key: &VersionKey,
    account_id: AccountId,
    customer_id: CustomerId,
    expected: &VersionToken,
    changes: ValidatedAccountChanges,
) -> Result<AccountUpdated, ApiError> {
    let applied = account_update::apply(
        &mut uow.store(),
        key,
        account_id,
        customer_id,
        expected,
        changes,
    )
    .await;
    if let Err(err) = uow.commit().await {
        record_outcome(OUTCOME_ROLLED_BACK);
        return Err(ApiError::from(err));
    }
    record_outcome(OUTCOME_COMMITTED);
    applied.map_err(ApiError::from)
}

/// The `version` a `PUT /accounts/{id}` body echoes back, read by
/// [`VersionToken::parse`].
///
/// # Errors
///
/// 400 `Invalid` with [`MISSING_VERSION_MESSAGE`] and no fields when the
/// token is absent or is not exactly 16 lower-case hex digits: the
/// `no-message` row V-COACTUPC-001, whose BMS field is none (E-24).
fn request_version(version: Option<&str>) -> Result<VersionToken, ApiError> {
    version.and_then(VersionToken::parse).ok_or_else(|| {
        ApiError::no_message(ErrorKind::Invalid, MISSING_VERSION_MESSAGE, Vec::new())
    })
}

/// Ends a unit of work that wrote nothing by rolling it back.
///
/// A failed `ROLLBACK` leaves the answer as it is: nothing was written, and
/// the server discards an unfinished transaction when its connection closes,
/// so the failure is only logged.
async fn end_read_only(uow: UnitOfWork) {
    if let Err(err) = uow.rollback().await {
        tracing::warn!(error = %err, "read-only unit of work did not roll back");
    }
}

/// The answer to a request the rules refused before any write: the unit of
/// work rolled back and `err` with its mapped status.
async fn refuse(uow: UnitOfWork, err: DomainError) -> ApiError {
    end_read_only(uow).await;
    rejected(err)
}

/// `err` as the answer of a request that wrote nothing, with `outcome`
/// recorded as `rejected`.
fn rejected(err: impl Into<ApiError>) -> ApiError {
    record_outcome(OUTCOME_REJECTED);
    err.into()
}

/// Records `outcome` on the handler's span.
fn record_outcome(outcome: &'static str) {
    Span::current().record("outcome", outcome);
}

/// The `{id}` key `entered` as the span records it, from `edited`, the
/// program's key edit of it: the account the edit accepted, or the category
/// of the refusal with the key's count of characters, keeping nothing of a
/// refused key's text.
fn span_key(entered: &str, edited: &Result<AccountId, DomainError>) -> SpanKey {
    match edited {
        Ok(account_id) => SpanKey::Accepted(*account_id),
        Err(error) => {
            let length = entered.chars().count();
            let category = if error.kind == ErrorKind::Empty {
                KEY_BLANK
            } else if length > AccountId::WIDTH {
                KEY_TOO_LONG
            } else {
                KEY_INVALID
            };
            SpanKey::Refused { category, length }
        }
    }
}

/// The `{id}` key `entered` of a `PUT /accounts/{id}` as the span records it.
///
/// A segment longer than the 11 characters of `ACCTSIDI X(11)` is the one
/// [`AccountUpdateRequest::check`] refuses before any key edit, and is
/// recorded as [`KEY_TOO_LONG`] whatever it holds, spaces included:
/// `account_update::edit_account_id` would `MOVE` it into `CC-ACCT-ID X(11)`
/// and read only the account its first 11 characters name, which the request
/// does not address. Any other segment is recorded by [`span_key`] from that
/// edit, the edit's own reading deciding a blank key.
fn update_span_key(entered: &str) -> SpanKey {
    let length = entered.chars().count();
    if length > AccountId::WIDTH {
        SpanKey::Refused {
            category: KEY_TOO_LONG,
            length,
        }
    } else {
        span_key(entered, &account_update::edit_account_id(entered))
    }
}

/// Records `key` on the handler's span; the fields it does not set stay
/// empty.
fn record_key(key: SpanKey) {
    let span = Span::current();
    match key {
        SpanKey::Accepted(account_id) => span.record("account_id", display(account_id)),
        SpanKey::Refused { category, length } => span
            .record("account_id_rejected", category)
            .record("account_id_length", length),
    };
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::numeric::AccountId;
    use carddemo_domain::rules::account_view;
    use serde_json::{Value, json};
    use utoipa_axum::router::OpenApiRouter;
    use utoipa_axum::routes;

    use super::{
        KEY_BLANK, KEY_INVALID, KEY_TOO_LONG, MISSING_VERSION_MESSAGE, SpanKey, request_version,
        span_key, update_span_key,
    };
    use crate::state::AppState;

    /// A well-formed token: 16 lower-case hex digits.
    const TOKEN: &str = "0123456789abcdef";

    /// The `OpenAPI` document of the two operations, registered as the route
    /// registry registers them: one `routes!` for the path.
    fn document() -> Value {
        let (_router, api) = OpenApiRouter::<AppState>::new()
            .routes(routes!(super::get_account, super::update_account))
            .split_for_parts();
        serde_json::to_value(api).expect("the document serializes")
    }

    #[test]
    fn the_id_segment_is_published_with_the_acctsid_width() {
        let document = document();
        for method in ["get", "put"] {
            assert_eq!(
                document["paths"]["/accounts/{id}"][method]["parameters"],
                json!([{
                    "name": "id",
                    "in": "path",
                    "description": "ACCT-ID 9(11) as entered",
                    "required": true,
                    "schema": { "type": "string", "maxLength": 11 },
                }]),
                "{method}"
            );
        }
    }

    /// What a handler's span records for the `{id}` segment it was given.
    type SpanKeyOf = fn(&str) -> SpanKey;

    /// The span key of a key the edit refused.
    const fn refused(category: &'static str, length: usize) -> SpanKey {
        SpanKey::Refused { category, length }
    }

    #[test]
    fn span_key_is_the_edited_account_or_only_the_refusal_class_and_length() {
        // `CAVW` and `CAUP` each record what their own key edit decides, as
        // their handlers derive it; `CAUP` holds the key to `ACCTSID` first.
        let view: SpanKeyOf = |text| span_key(text, &account_view::edit_account_id(Some(text)));
        let update: SpanKeyOf = update_span_key;
        // The fixture account (P-CVACT01Y-015) [app/data/ASCII/acctdata.txt:L1].
        let account = AccountId::new(1).expect("an account number of 11 digits");
        // (handler, `{id}` as entered) -> what the span records
        let cases = [
            (view, "00000000001", SpanKey::Accepted(account)),
            (update, "00000000001", SpanKey::Accepted(account)),
            (view, "", refused(KEY_BLANK, 0)),
            (view, "   ", refused(KEY_BLANK, 3)),
            (view, "*", refused(KEY_BLANK, 1)),
            (view, "\0\0\0\0\0\0\0\0\0\0\0", refused(KEY_BLANK, 11)),
            (update, "", refused(KEY_BLANK, 0)),
            (update, "*", refused(KEY_BLANK, 1)),
            (view, "00000000000", refused(KEY_INVALID, 11)),
            (view, "0000000001A", refused(KEY_INVALID, 11)),
            (view, "1", refused(KEY_INVALID, 1)),
            (view, "\u{e9}", refused(KEY_INVALID, 1)),
            (update, "00000000000", refused(KEY_INVALID, 11)),
            (update, "abc", refused(KEY_INVALID, 3)),
            (view, "000000000012", refused(KEY_TOO_LONG, 12)),
            (view, "4111111111111111", refused(KEY_TOO_LONG, 16)),
            (update, "1234567890ab", refused(KEY_TOO_LONG, 12)),
            // The `CAUP` edit would cut these to the valid account their
            // first 11 characters name, and to a blank key.
            (update, "123456789012", refused(KEY_TOO_LONG, 12)),
            (update, "000000000011", refused(KEY_TOO_LONG, 12)),
            (update, "            ", refused(KEY_TOO_LONG, 12)),
            (view, "            ", refused(KEY_TOO_LONG, 12)),
            (update, "           ", refused(KEY_BLANK, 11)),
        ];
        for (key_of, entered, expected) in cases {
            assert_eq!(key_of(entered), expected, "{entered:?}");
        }
    }

    #[test]
    fn missing_or_malformed_version_is_the_no_message_400() {
        let malformed = [
            None,
            Some(""),
            Some("0123456789abcde"),
            Some("0123456789abcdef0"),
            Some("0123456789ABCDEF"),
            Some("0123456789abcdeg"),
            Some(" 123456789abcdef"),
        ];
        for version in malformed {
            let Err(error) = request_version(version) else {
                panic!("{version:?} is not a well-formed token");
            };
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{version:?}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{version:?}");
            assert_eq!(error.body().message, MISSING_VERSION_MESSAGE, "{version:?}");
            assert!(error.body().fields.is_empty(), "{version:?}");
        }
    }

    #[test]
    fn well_formed_version_is_passed_on_unchanged() {
        let token = request_version(Some(TOKEN)).expect("a well-formed token");
        assert_eq!(token.as_str(), TOKEN);
    }
}
