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
//! `user_id`, `account_id` (the `{id}` segment as entered) and `outcome`,
//! recorded once the unit of work has ended:
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

/// COACTVWC 2000-PROCESS-INPUTS, 9000-READ-ACCT, 1200-SETUP-SCREEN-VARS —
/// P-COACTVWC-004 to P-COACTVWC-011, P-COACTVWC-014, P-COACTVWC-015,
/// P-COACTVWC-019 to P-COACTVWC-021, P-COACTVWC-025, P-COACTVWC-030,
/// V-COACTVWC-001 to V-COACTVWC-008
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
///   (V-COACTVWC-001), or is not 11 digits or is zero (V-COACTVWC-002).
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
    params(("id" = String, Path, description = "ACCT-ID 9(11) as entered")),
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
        account_id = %id,
        outcome = tracing::field::Empty,
    )
)]
pub async fn get_account(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<AccountResponse>, ApiError> {
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
/// 1. the BMS field lengths of the body, then its `version` token
///    (V-COACTUPC-001), before any COBOL edit;
/// 2. the account number `{id}` and the fetch (`load_snapshot`);
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
///   each body field longer than its BMS field.
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
#[utoipa::path(
    put,
    path = "/accounts/{id}",
    tag = "accounts",
    params(("id" = String, Path, description = "ACCT-ID 9(11) as entered")),
    request_body = AccountUpdateRequest,
    responses(
        (status = 200, description = "Changes committed (P-COACTUPC-015), or no change \
            detected (V-COACTUPC-010): the records as they now stand",
            body = AccountUpdateResponse),
        (status = 400, description = "A body field longer than its BMS field, a missing or \
            malformed version token (V-COACTUPC-001), or a failed key or field edit \
            (V-COACTUPC-002, V-COACTUPC-003, V-COACTUPC-011 to V-COACTUPC-123)",
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
        account_id = %id,
        outcome = tracing::field::Empty,
    )
)]
pub async fn update_account(
    user: AuthUser,
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<AccountUpdateRequest>,
) -> Result<Json<AccountUpdateResponse>, ApiError> {
    body.check().map_err(rejected)?;
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

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::ErrorKind;

    use super::{MISSING_VERSION_MESSAGE, request_version};

    /// A well-formed token: 16 lower-case hex digits.
    const TOKEN: &str = "0123456789abcdef";

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
