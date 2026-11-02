//! CPVS and CPVD: the pending authorizations of an account (`COPAUS0C`, map
//! `COPAU00`) and one pending authorization with its fraud mark (`COPAUS1C`
//! with its linked `COPAUS2C`, map `COPAU01`).
//!
//! | Operation | Handler | Program and key |
//! |---|---|---|
//! | `GET /authorizations` | [`list_authorizations`] | `COPAUS0C` ENTER, PF7 (`direction=backward`) and PF8 (`direction=forward` from `next_key`) |
//! | `GET /authorizations/{id}` | [`get_authorization`] | `COPAUS1C` ENTER, with the PF8 read after it as `next_id` |
//! | `PUT /authorizations/{id}` | [`toggle_fraud`] | `COPAUS1C` PF5 `MARK-AUTH-FRAUD`, with `COPAUS2C` |
//!
//! The authorizations the CP00 consumer writes are read here. The REST key is
//! the row identity `auth_id`, which a summary page lists and the detail
//! carries.
//!
//! # Units of work
//!
//! Each request opens one unit of work, the PSB schedule of the COBOL. When it
//! cannot be opened the request answers 500 with
//! ` System error while scheduling PSB: Code:` and the status E-44 proposes,
//! on the program's cursor field (V-COPAUS0C-010, V-COPAUS1C-002). The key
//! edits read no store and run first, as `PROCESS-ENTER-KEY` runs before the
//! PSB is scheduled, so a rejected key keeps its own answer when no
//! connection can be had (V-COPAUS0C-001, V-COPAUS0C-002, V-COPAUS1C-001).
//!
//! - The read paths write nothing and lock nothing; their `TAKE-SYNCPOINT`
//!   and the `SYNCPOINT` of `SEND-PAULST-SCREEN` commit nothing, so the unit
//!   of work is rolled back once the rule has read (P-COPAUS0C-015,
//!   P-COPAUS1C-015). CPVD's view reads the summary of the detail's account,
//!   the detail and the twin after it, each once (P-COPAUS1C-003,
//!   P-COPAUS1C-014).
//! - The fraud mark reads the same summary and then the detail under a row
//!   lock, writes `AUTHFRDS` (Db2) and replaces the IMS detail. Both writes
//!   persist only together at the `SYNCPOINT` after the replace, and
//!   `ROLL-BACK` backs out both (P-COPAUS1C-007, P-COPAUS1C-008,
//!   P-COPAUS1C-015). The handler commits when the rule succeeds and rolls the
//!   whole unit of work back when it fails; either ends the lock, so a second
//!   mark of the same authorization waits for the first and toggles the flag
//!   it left. These are recoverable resources, unlike the `RECOVERY(NONE)`
//!   VSAM files, so the keep-earlier-writes rule of the other online writers
//!   does not apply here.
//!
//! # Identity
//!
//! Every route takes [`AuthUser`]. No request field names the account or the
//! customer of the fraud mark: the account is the detail's own `PA-ACCT-ID`,
//! and the customer is the one the card cross-reference of that account names,
//! as `COPAUS0C` fills `CDEMO-CUST-ID` for `COPAUS1C`.
//!
//! # Telemetry
//!
//! Each handler runs in a span with `transaction` (`CPVS` or `CPVD`),
//! `user_id`, the resource keys and `outcome`. A key is recorded only once its
//! edit has passed, in its canonical form: for CPVS the 11-digit `account_id`
//! the account edit returns, and for CPVD the positive `auth_id` the path
//! parses to, in plain decimal, then the detail's account once it is read. A
//! rejected key is never recorded, so no entered text reaches the span; its
//! `outcome`, the error body's `code` (`EMPTY` or `INVALID`), is the
//! rejection category. `outcome` is otherwise `ok` for a read, `committed` for
//! a fraud mark, or the error body's `code` once the unit of work has been
//! rolled back.

use axum::Json;
use axum::extract::State;
use carddemo_domain::clock::Clock;
use carddemo_domain::error::DomainError;
use carddemo_domain::messages;
use carddemo_domain::numeric::{AccountId, AuthId, CustomerId};
use carddemo_domain::ports::{AuthorizationStore, PortError, XrefReader};
use carddemo_domain::rules::auth_detail::{self, FraudOutcome};
use carddemo_domain::rules::auth_summary::{self, AuthSummaryInput};
use carddemo_persistence::UnitOfWork;
use tracing::Span;
use tracing::field::{Empty, display};

use crate::auth::AuthUser;
use crate::dto::authorizations::{
    AuthDetailResponse, AuthIdPath, AuthSummaryPageQuery, AuthSummaryQuery, AuthSummaryResponse,
    FraudToggleResponse, parse_auth_id,
};
use crate::error::{ApiError, ApiPath, ApiQuery, ErrorBody};
use crate::pagination::PageQuery;
use crate::state::AppState;

/// The transaction ID of `COPAUS0C` [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L33-36].
const CPVS: &str = "CPVS";

/// The transaction ID of `COPAUS1C` [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L33-36].
const CPVD: &str = "CPVD";

/// The span `outcome` of a read that answered 200.
const OUTCOME_OK: &str = "ok";

/// The span `outcome` of a fraud mark whose unit of work committed.
const OUTCOME_COMMITTED: &str = "committed";

/// The text of a path that sets no message (E-49, E-53).
const NO_MESSAGE: &str = "";

/// COPAUS0C GATHER-DETAILS, PROCESS-ENTER-KEY, PROCESS-PF7-KEY,
/// PROCESS-PF8-KEY — P-COPAUS0C-001, P-COPAUS0C-003, P-COPAUS0C-006 to
/// P-COPAUS0C-015, P-COPAUS0C-020, V-COPAUS0C-001, V-COPAUS0C-002,
/// V-COPAUS0C-004 to V-COPAUS0C-015: the pending authorizations of an account.
///
/// The account ID is edited as entered, then the card cross-reference, the
/// account, the customer and the authorization summary of the account are
/// read, and, when the summary exists, one page of its authorizations, newest
/// first. Without `start_key` the page is the first one forward (ENTER) and
/// the last one, the oldest authorizations, backward; `next_key` with
/// `direction=forward` is PF8 and `prev_key` with `direction=backward` is
/// PF7. A request past either end lists nothing and carries the top- or
/// bottom-of-page text.
///
/// # Errors
///
/// - 400 with `Please enter Acct Id...` or `Acct Id must be Numeric ...` on
///   `ACCTID` for a blank or non-numeric account ID, and 400 with code
///   `INVALID` for a `start_key` that is not an authorization identity or a
///   `page_size` outside 1 to 100.
/// - 401 without a valid bearer token.
/// - 404 with the `COPAUS0C` text when the account has no cross-reference,
///   or the cross-reference names no account or customer.
/// - 500 with the `COPAUS0C` system-error text when a read fails or the
///   authorization store cannot be opened.
#[utoipa::path(
    get,
    path = "/authorizations",
    tag = "authorizations",
    params(AuthSummaryQuery, AuthSummaryPageQuery),
    responses(
        (status = 200, description = "The account header, its authorization summary and one page of its pending authorizations, newest first", body = AuthSummaryResponse),
        (status = 400, description = "The account ID is blank or not 11 digits, or the page position is malformed", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "The account has no card cross-reference, or the cross-reference names no account or customer", body = ErrorBody),
        (status = 500, description = "A read failed or the authorization store could not be opened", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = CPVS,
        user_id = %user.user_id,
        account_id = Empty,
        outcome = Empty,
    ),
)]
pub async fn list_authorizations(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<AuthSummaryQuery>,
    ApiQuery(page): ApiQuery<AuthSummaryPageQuery>,
) -> Result<Json<AuthSummaryResponse>, ApiError> {
    let result = summary_page(&state, query, page.into()).await;
    record_outcome(&result, OUTCOME_OK);
    result.map(Json)
}

/// The CPVS page of the request: the key edits of [`summary_input`], then the
/// reads in a unit of work that is rolled back.
async fn summary_page(
    state: &AppState,
    query: AuthSummaryQuery,
    page: PageQuery,
) -> Result<AuthSummaryResponse, ApiError> {
    let (account_id, input) = summary_input(query, page)?;
    Span::current().record("account_id", display(account_id));
    let mut uow = open_store(state, auth_summary::schedule_psb_failure).await?;
    let view = auth_summary::summary(&mut uow.store(), input).await;
    roll_back(uow).await;
    Ok(AuthSummaryResponse::from_view(&view?))
}

/// COPAUS0C PROCESS-ENTER-KEY — P-COPAUS0C-003, V-COPAUS0C-001,
/// V-COPAUS0C-002 [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L261-280]:
/// the CPVS request as the rule input, with the account it names, edited
/// before the authorization store is opened.
///
/// `PROCESS-ENTER-KEY` edits `ACCTIDI` before `GET-AUTH-SUMMARY` schedules
/// the PSB. The length check, the browse position and
/// [`auth_summary::edit_account_id`] read no store, so their rejections come
/// before the scheduling failure (V-COPAUS0C-010) even when no connection can
/// be had. The input keeps the account as entered, and the rule edits it
/// again with the same answer.
///
/// # Errors
///
/// 400 on `ACCTID` with code `EMPTY` and `Please enter Acct Id...` for a
/// blank account ID, or with code `INVALID` and `Acct Id must be Numeric ...`
/// for one that is not 11 digits; 400 with code `INVALID` and no fields for a
/// `start_key` that is not an authorization identity.
fn summary_input(
    query: AuthSummaryQuery,
    page: PageQuery,
) -> Result<(AccountId, AuthSummaryInput), ApiError> {
    query.check()?;
    let (start_key, direction, page_size) = page.resolve(auth_summary::SCREEN_ROWS);
    let input = query.into_input(start_key, direction, page_size)?;
    let account_id =
        auth_summary::edit_account_id(input.account_id.as_deref()).map_err(ApiError::from)?;
    Ok((account_id, input))
}

/// COPAUS1C PROCESS-ENTER-KEY, READ-AUTH-RECORD, POPULATE-AUTH-DETAILS,
/// PROCESS-PF8-KEY, READ-NEXT-AUTH-RECORD — P-COPAUS1C-003 to P-COPAUS1C-005,
/// P-COPAUS1C-009, P-COPAUS1C-014 to P-COPAUS1C-016, V-COPAUS1C-001 to
/// V-COPAUS1C-006, V-COPAUS1C-012, V-COPAUS1C-013: one pending authorization.
///
/// The authorization summary of the detail's account is read, then the
/// detail by its `auth_id` (ENTER), then the PF8 read under the same summary
/// gives `next_id`, the next older authorization. At the last authorization
/// `next_id` is `null` and `message` carries
/// `Already at the last Authorization...`.
///
/// # Errors
///
/// Each on `CARDNUM`:
///
/// - 400 with no message when `{id}` is not a positive integer, the COBOL
///   path with no selected key (V-COPAUS1C-001, E-49).
/// - 401 without a valid bearer token.
/// - 404 with no message when no authorization has the identity or no
///   summary is found over it (V-COPAUS1C-004, V-COPAUS1C-005).
/// - 500 with ` System error while reading Auth Details: Code:` and the IMS
///   status when the detail read fails, whatever the summary read answered
///   (V-COPAUS1C-006); with ` System error while reading Auth Summary: Code:`
///   and the IMS status when only the summary read fails (V-COPAUS1C-003);
///   with ` System error while reading next Auth: Code:` when the next read
///   fails (V-COPAUS1C-013); with the scheduling text when the authorization
///   store cannot be opened (V-COPAUS1C-002).
#[utoipa::path(
    get,
    path = "/authorizations/{id}",
    tag = "authorizations",
    params(AuthIdPath),
    responses(
        (status = 200, description = "The authorization as the detail screen shows it, with the identity of the next one", body = AuthDetailResponse),
        (status = 400, description = "The identity is not a positive integer", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "No authorization has the identity", body = ErrorBody),
        (status = 500, description = "A read failed or the authorization store could not be opened", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = CPVD,
        user_id = %user.user_id,
        account_id = Empty,
        auth_id = Empty,
        outcome = Empty,
    ),
)]
pub async fn get_authorization(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(AuthIdPath { id }): ApiPath<AuthIdPath>,
) -> Result<Json<AuthDetailResponse>, ApiError> {
    let result = detail_view(&state, &id).await;
    record_outcome(&result, OUTCOME_OK);
    result.map(Json)
}

/// The CPVD view of `id`: the key edit of [`selected_auth_id`], then the
/// reads in a unit of work that is rolled back.
async fn detail_view(state: &AppState, id: &str) -> Result<AuthDetailResponse, ApiError> {
    let auth_id = selected_auth_id(id)?;
    Span::current().record("auth_id", display(auth_id));
    let mut uow = open_store(state, schedule_psb_failure).await?;
    let view = read_with_next(&mut uow.store(), auth_id).await;
    roll_back(uow).await;
    Ok(view?)
}

/// COPAUS1C PROCESS-ENTER-KEY then PROCESS-PF8-KEY — P-COPAUS1C-003,
/// P-COPAUS1C-009, P-COPAUS1C-014: the detail `auth_id` names and the PF8 step
/// after it, under the summary of the detail's own account.
///
/// `READ-AUTH-RECORD` reads the summary of the detail's account, then the
/// detail, unlocked; the PF8 step is one lookup of the twin after that
/// detail's key, which reads neither again.
async fn read_with_next<S: AuthorizationStore>(
    store: &mut S,
    auth_id: AuthId,
) -> Result<AuthDetailResponse, DomainError> {
    let detail = auth_detail::detail(store, auth_id).await?;
    Span::current().record("account_id", display(detail.pa_acct_id));
    let step = auth_detail::next(store, &detail).await?;
    Ok(AuthDetailResponse::from_step(&detail, &step))
}

/// COPAUS1C MARK-AUTH-FRAUD, READ-AUTH-RECORD, UPDATE-AUTH-DETAILS,
/// TAKE-SYNCPOINT, ROLL-BACK with COPAUS2C — P-COPAUS1C-003, P-COPAUS1C-004,
/// P-COPAUS1C-006 to P-COPAUS1C-008, P-COPAUS1C-015, P-COPAUS1C-016,
/// P-COPAUS2C-001 to P-COPAUS2C-006, V-COPAUS1C-002 to V-COPAUS1C-007,
/// V-COPAUS1C-009 to V-COPAUS1C-011: PF5, the fraud mark of one pending
/// authorization.
///
/// The request has no body: PF5 acts on the record shown. The authorization
/// summary of the detail's account is read, then the detail, under a row
/// lock held until the request commits or rolls back, so overlapping marks
/// of one authorization take turns and each toggles the flag the one before
/// it left. A detail holding `F` becomes `R` and its fraud report is removed;
/// any other detail becomes `F` and fraud is reported. `COPAUS2C` stamps the
/// report date of the processing clock and inserts the `AUTHFRDS` row, or
/// updates it when the authorization was reported before; then the detail is
/// replaced. Both writes commit together, and the response carries the
/// replaced detail with `AUTH MARKED FRAUD...` or `AUTH FRAUD REMOVED...`.
///
/// # Errors
///
/// Each on `CARDNUM`, and nothing is written:
///
/// - 400 with no message when `{id}` is not a positive integer.
/// - 401 without a valid bearer token.
/// - 404 with no message when no authorization has the identity or no
///   summary is found over it, the outcome E-53 proposes for a fraud mark
///   whose read finds nothing, or when the authorization's account has no
///   card cross-reference to name its customer.
/// - 500 with ` System error while reading Auth Details: Code:` and the IMS
///   status when the detail read fails or its lock cannot be taken, whatever
///   the summary read answered (V-COPAUS1C-006); with
///   ` System error while reading Auth Summary: Code:` and the IMS status
///   when only the summary read fails (V-COPAUS1C-003); with the `COPAUS1C`
///   or `COPAUS2C` text when the fraud-table write or the detail replace
///   fails, or the scheduling text when the authorization store cannot be
///   opened; 500 `Unexpected error` when the cross-reference read or the
///   commit fails.
#[utoipa::path(
    put,
    path = "/authorizations/{id}",
    tag = "authorizations",
    params(AuthIdPath),
    responses(
        (status = 200, description = "The fraud mark was toggled: the replaced authorization and the text of the new mark", body = FraudToggleResponse),
        (status = 400, description = "The identity is not a positive integer", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 404, description = "No authorization has the identity, or its account has no card cross-reference", body = ErrorBody),
        (status = 500, description = "A read or write failed and nothing was changed", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
#[tracing::instrument(
    skip_all,
    fields(
        transaction = CPVD,
        user_id = %user.user_id,
        account_id = Empty,
        auth_id = Empty,
        outcome = Empty,
    ),
)]
pub async fn toggle_fraud(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(AuthIdPath { id }): ApiPath<AuthIdPath>,
) -> Result<Json<FraudToggleResponse>, ApiError> {
    let result = fraud_mark(&state, &id).await;
    record_outcome(&result, OUTCOME_COMMITTED);
    result.map(Json)
}

/// The CPVD fraud mark of `id`: the key edit of [`selected_auth_id`], then a
/// unit of work committed when the rule succeeds and rolled back as a whole
/// when it fails.
async fn fraud_mark(state: &AppState, id: &str) -> Result<FraudToggleResponse, ApiError> {
    let auth_id = selected_auth_id(id)?;
    Span::current().record("auth_id", display(auth_id));
    let mut uow = open_store(state, schedule_psb_failure).await?;
    match mark_fraud(&mut uow.store(), auth_id, state.clock()).await {
        Ok(outcome) => {
            uow.commit().await?;
            Ok(FraudToggleResponse::from(&outcome))
        }
        Err(error) => {
            roll_back(uow).await;
            Err(error.into())
        }
    }
}

/// COPAUS1C MARK-AUTH-FRAUD, READ-AUTH-RECORD — P-COPAUS1C-003,
/// P-COPAUS1C-006, P-COPAUS1C-007: reads the summary of the detail's account
/// and the detail under a row lock, names the customer of that account and
/// toggles the fraud mark of the locked detail on `clock`.
///
/// The detail is read once, locked, and the toggle works from that read; the
/// lock lasts until [`fraud_mark`] commits or rolls back the unit of work, so
/// a second mark of the same authorization waits and toggles the flag this
/// one committed.
async fn mark_fraud<S>(
    store: &mut S,
    auth_id: AuthId,
    clock: &impl Clock,
) -> Result<FraudOutcome, DomainError>
where
    S: AuthorizationStore + XrefReader,
{
    let locked = auth_detail::detail_for_update(store, auth_id).await?;
    let account_id = locked.detail().pa_acct_id;
    Span::current().record("account_id", display(account_id));
    let customer_id = fraud_customer(store, account_id).await?;
    auth_detail::toggle_fraud(store, locked, customer_id, clock).await
}

/// COPAUS0C GETCARDXREF-BYACCT — P-COPAUS0C-007
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L812-831]: the customer
/// `COPAUS1C` passes to `COPAUS2C` as `WS-FRD-CUST-ID`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L246].
///
/// `COPAUS1C` reads no customer itself; it passes the `CDEMO-CUST-ID` that
/// `COPAUS0C` moved from `XREF-CUST-ID` of the account's cross-reference. The
/// stateless request has no COMMAREA, so the same read names it here. The
/// summary's own `PA-CUST-ID` is not used in its place, because no audit row
/// records the two as equal.
///
/// A missing cross-reference has no `COPAUS1C` path: the COBOL would pass
/// whatever customer the COMMAREA held (E-52). Nothing is guessed: the fraud
/// mark is refused with the no-message not-found answer of an authorization
/// that cannot be read (E-53), and any other read failure is the
/// unexpected-error text; both on `CARDNUM`, the cursor of every `COPAUS1C`
/// message (P-COPAUS1C-011).
async fn fraud_customer<S: XrefReader>(
    store: &mut S,
    account_id: AccountId,
) -> Result<CustomerId, DomainError> {
    match store.read_xref_by_account(account_id).await {
        Ok(xref) => Ok(xref.xref_cust_id),
        Err(PortError::NotFound) => {
            Err(DomainError::not_found(NO_MESSAGE).with_field(auth_detail::CARD_NUM_FIELD))
        }
        Err(other) => {
            tracing::error!(error = %other, "card cross-reference read failed");
            Err(DomainError::unexpected(messages::UNEXPECTED_ERROR)
                .with_field(auth_detail::CARD_NUM_FIELD))
        }
    }
}

/// COPAUS1C PROCESS-ENTER-KEY — V-COPAUS1C-001, E-49
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L208-227]: the path
/// `{id}` of `GET` and `PUT /authorizations/{id}` as the authorization it
/// selects, edited before the authorization store is opened.
///
/// `PROCESS-ENTER-KEY` sets its error flag with no message, and schedules no
/// PSB, when no key is selected. A path that is not a decimal integer keeps
/// the [`parse_auth_id`] answer, and a non-positive one gets the
/// [`auth_detail::edit_auth_id`] answer, the same body. Neither reads a
/// store, so both come before the scheduling failure (V-COPAUS1C-002) even
/// when no connection can be had; the rule's read repeats the edit.
///
/// # Errors
///
/// 400 with code `INVALID`, no message and the field `CARDNUM` when `raw` is
/// not a positive integer.
fn selected_auth_id(raw: &str) -> Result<AuthId, ApiError> {
    let auth_id = parse_auth_id(raw)?;
    auth_detail::edit_auth_id(auth_id).map_err(ApiError::from)
}

/// COPAUS1C SCHEDULE-PSB — P-COPAUS1C-016, P-COPAUS1C-017, V-COPAUS1C-002: the
/// error of a CPVD request whose authorization store cannot be opened.
///
/// `COPAUS0C` and `COPAUS1C` show the same literal with the same status slot,
/// so the text is the one `rules::auth_summary` builds; only the cursor field
/// differs, `CARDNUM` for `COPAUS1C`.
fn schedule_psb_failure() -> DomainError {
    DomainError::unexpected(auth_summary::schedule_psb_failure().message)
        .with_field(auth_detail::CARD_NUM_FIELD)
}

/// Opens the unit of work of a request, the `SCHEDULE-PSB` of the COBOL.
///
/// # Errors
///
/// The `failure` of the program, 500 with the scheduling text, when no
/// transaction can be begun; the database detail goes to the log only.
async fn open_store(
    state: &AppState,
    failure: fn() -> DomainError,
) -> Result<UnitOfWork, ApiError> {
    state.db().begin().await.map_err(|error| {
        tracing::error!(error = %error, "authorization store could not be opened");
        ApiError::from(failure())
    })
}

/// Ends a unit of work whose changes must not persist: a read path, or a
/// fraud mark that failed.
///
/// A failed `ROLLBACK` leaves the answer unchanged: the read is complete, and
/// a transaction that never ends is rolled back when its session closes, so
/// nothing persists either way.
async fn roll_back(uow: UnitOfWork) {
    if let Err(error) = uow.rollback().await {
        tracing::error!(error = %error, "unit of work did not roll back");
    }
}

/// Records the span `outcome`: `success` for a 2xx answer, else the error
/// body's `code`.
fn record_outcome<T>(result: &Result<T, ApiError>, success: &'static str) {
    let outcome = match result {
        Ok(_) => success,
        Err(error) => error.body().code.as_str(),
    };
    Span::current().record("outcome", outcome);
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use carddemo_domain::entities::CardXref;
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{AccountId, AuthId, CardNumber, CustomerId};
    use carddemo_domain::ports::{PortError, PortResult, XrefReader};
    use carddemo_domain::rules::{auth_detail, auth_summary};
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use utoipa::IntoParams;
    use utoipa_axum::router::OpenApiRouter;
    use utoipa_axum::routes;

    use super::{fraud_customer, schedule_psb_failure, selected_auth_id, summary_input};
    use crate::dto::authorizations::{AuthIdPath, AuthSummaryPageQuery, AuthSummaryQuery};
    use crate::error::{ApiError, ApiPath};
    use crate::pagination::PageQuery;
    use crate::state::AppState;

    /// A cross-reference file holding at most one record, or failing every
    /// read with one error.
    struct OneXref(Result<CardXref, PortError>);

    impl XrefReader for OneXref {
        async fn read_xref_by_card(&mut self, _card: CardNumber) -> PortResult<CardXref> {
            self.0.clone()
        }

        async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
            match &self.0 {
                Ok(xref) if xref.xref_acct_id == id => Ok(xref.clone()),
                Ok(_) => Err(PortError::NotFound),
                Err(error) => Err(error.clone()),
            }
        }

        async fn xrefs_after(
            &mut self,
            _after: Option<CardNumber>,
            _limit: u16,
        ) -> PortResult<Vec<CardXref>> {
            Ok(self.0.iter().cloned().collect())
        }
    }

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn customer_id(value: u32) -> CustomerId {
        CustomerId::new(value).expect("a customer number of at most 9 digits")
    }

    fn xref() -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from("0500024453765740"),
            xref_cust_id: customer_id(50),
            xref_acct_id: account_id(50),
        }
    }

    fn body(error: DomainError) -> Value {
        serde_json::to_value(ApiError::from(error).body()).expect("serializes")
    }

    /// The status and JSON body an error answers with.
    fn answer(error: &ApiError) -> (StatusCode, Value) {
        let body = serde_json::to_value(error.body()).expect("serializes");
        (error.status(), body)
    }

    /// The 400 answer of a key edit: `message` on the one BMS field `field`.
    fn rejection(kind: ErrorKind, message: &str, field: &str) -> (StatusCode, Value) {
        let body = json!({
            "code": kind.code(),
            "message": message,
            "fields": [{"field": field, "message": message}],
        });
        (StatusCode::BAD_REQUEST, body)
    }

    #[test]
    fn the_cpvs_account_edit_answers_without_opening_the_store() {
        // COPAUS0C PROCESS-ENTER-KEY: V-COPAUS0C-001 and V-COPAUS0C-002
        // (orders 1 and 2) come before the scheduling failure V-COPAUS0C-010
        // (order 7), so the edit reads no store; a passing entry is its
        // canonical account, and the rule input keeps the entry as sent.
        let empty = Err((ErrorKind::Empty, messages::PLEASE_ENTER_ACCT_ID));
        let not_numeric = Err((ErrorKind::Invalid, messages::ACCT_ID_MUST_BE_NUMERIC));
        let cases = [
            (None, empty),
            (Some(""), empty),
            (Some("   "), empty),
            (Some("0000000000A"), not_numeric),
            (Some("123"), not_numeric),
            (Some("000000000012"), not_numeric),
            (Some("00000000001"), Ok(1)),
            (Some("99999999999"), Ok(99_999_999_999)),
        ];
        for (entered, expected) in cases {
            let query = AuthSummaryQuery {
                account_id: entered.map(str::to_owned),
            };
            let outcome = summary_input(query, PageQuery::default())
                .map(|(account, input)| (account, input.account_id))
                .map_err(|error| answer(&error));
            let expected = expected
                .map(|id| (account_id(id), entered.map(str::to_owned)))
                .map_err(|(kind, message)| rejection(kind, message, "ACCTID"));
            assert_eq!(outcome, expected, "{entered:?}");
        }
        assert_eq!(account_id(1).to_string(), "00000000001");
    }

    #[test]
    fn the_cpvd_key_edit_answers_without_opening_the_store() {
        // COPAUS1C PROCESS-ENTER-KEY: V-COPAUS1C-001, E-49 (order 1) comes
        // before the scheduling failure V-COPAUS1C-002 (order 2). A path that
        // is no integer keeps the settled DTO answer, and a non-positive one
        // gets the same body from the rule's edit.
        let cases = [
            ("abc", None),
            ("", None),
            ("1.5", None),
            ("0", None),
            ("-5", None),
            ("1", Some(1)),
            ("9223372036854775807", Some(i64::MAX)),
        ];
        for (path, expected) in cases {
            let outcome = selected_auth_id(path).map_err(|error| answer(&error));
            let expected = expected
                .map(AuthId::from)
                .ok_or_else(|| rejection(ErrorKind::Invalid, "", auth_detail::CARD_NUM_FIELD));
            assert_eq!(outcome, expected, "{path:?}");
        }
    }

    #[tokio::test]
    async fn the_fraud_customer_is_the_cross_reference_customer_of_the_account() {
        // COPAUS0C L830 MOVE XREF-CUST-ID TO CDEMO-CUST-ID; COPAUS1C L246.
        let mut store = OneXref(Ok(xref()));
        let customer = fraud_customer(&mut store, account_id(50))
            .await
            .expect("the account has a cross-reference");
        assert_eq!(customer, customer_id(50));
    }

    #[tokio::test]
    async fn a_fraud_customer_that_cannot_be_read_refuses_the_mark_on_cardnum() {
        let cases = [
            (
                OneXref(Ok(xref())),
                account_id(51),
                ErrorKind::NotFound,
                StatusCode::NOT_FOUND,
                "",
            ),
            (
                OneXref(Err(PortError::Unavailable("connection reset".to_owned()))),
                account_id(50),
                ErrorKind::Unexpected,
                StatusCode::INTERNAL_SERVER_ERROR,
                messages::UNEXPECTED_ERROR,
            ),
        ];
        for (mut store, account, kind, status, message) in cases {
            let error = fraud_customer(&mut store, account)
                .await
                .expect_err("no customer");
            assert_eq!(error.kind, kind, "{account}");
            assert_eq!(ApiError::from(error.clone()).status(), status, "{account}");
            assert_eq!(
                body(error),
                json!({
                    "code": kind.code(),
                    "message": message,
                    "fields": [{"field": auth_detail::CARD_NUM_FIELD, "message": message}],
                }),
                "{account}"
            );
        }
    }

    #[test]
    fn a_store_that_cannot_be_opened_is_the_psb_scheduling_text_of_each_program() {
        // V-COPAUS0C-010 on ACCTID, V-COPAUS1C-002 on CARDNUM; E-44 status AO.
        let expected = " System error while scheduling PSB: Code:AO";
        let cases = [
            (auth_summary::schedule_psb_failure(), "ACCTID"),
            (schedule_psb_failure(), auth_detail::CARD_NUM_FIELD),
        ];
        for (error, field) in cases {
            let error = ApiError::from(error);
            assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR, "{field}");
            assert_eq!(
                serde_json::to_value(error.body()).expect("serializes"),
                json!({
                    "code": ErrorKind::Unexpected.code(),
                    "message": expected,
                    "fields": [{"field": field, "message": expected}],
                }),
            );
        }
    }

    #[test]
    fn the_handlers_register_their_operations_with_bearer_security() {
        let (_router, spec) = OpenApiRouter::<AppState>::new()
            .routes(routes!(super::list_authorizations))
            .routes(routes!(super::get_authorization, super::toggle_fraud))
            .split_for_parts();
        let spec = serde_json::to_value(&spec).expect("serializes");
        let paths = &spec["paths"];
        let operations = [
            (
                "/authorizations",
                "get",
                "list_authorizations",
                "AuthSummaryResponse",
            ),
            (
                "/authorizations/{id}",
                "get",
                "get_authorization",
                "AuthDetailResponse",
            ),
            (
                "/authorizations/{id}",
                "put",
                "toggle_fraud",
                "FraudToggleResponse",
            ),
        ];
        for (path, method, operation_id, body) in operations {
            let operation = &paths[path][method];
            assert_eq!(operation["operationId"], operation_id, "{path} {method}");
            assert_eq!(
                operation["tags"],
                json!(["authorizations"]),
                "{operation_id}"
            );
            assert_eq!(
                operation["security"],
                json!([{"bearer_auth": []}]),
                "{operation_id}"
            );
            let ok = &operation["responses"]["200"]["content"]["application/json"]["schema"];
            assert_eq!(
                ok["$ref"],
                format!("#/components/schemas/{body}"),
                "{operation_id}"
            );
            for status in ["400", "401", "404", "500"] {
                let error = &operation["responses"][status]["content"]["application/json"];
                assert_eq!(
                    error["schema"]["$ref"], "#/components/schemas/ErrorBody",
                    "{operation_id} {status}"
                );
            }
            assert!(operation.get("requestBody").is_none(), "{operation_id}");
        }

        let list_params: Vec<&str> = paths["/authorizations"]["get"]["parameters"]
            .as_array()
            .expect("parameters")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(
            list_params,
            ["account_id", "start_key", "direction", "page_size"]
        );
        // The list publishes its own key grammar, not the shared query's.
        let list = &paths["/authorizations"]["get"]["parameters"];
        assert_eq!(list[0]["schema"]["pattern"], "^[0-9]{11}$");
        let own =
            serde_json::to_value(AuthSummaryPageQuery::into_params(|| None)).expect("serializes");
        assert_eq!(list[1], own[0]);
        assert!(list[1]["schema"]["pattern"].is_string(), "{}", list[1]);

        let path = serde_json::to_value(AuthIdPath::into_params(|| None)).expect("serializes");
        for method in ["get", "put"] {
            let params = &paths["/authorizations/{id}"][method]["parameters"];
            assert_eq!(params.as_array().map(Vec::len), Some(1), "{method}");
            assert_eq!(params[0]["name"], "id", "{method}");
            assert_eq!(params[0]["in"], "path", "{method}");
            assert_eq!(params[0]["required"], true, "{method}");
            assert_eq!(params[0]["schema"]["type"], "string", "{method}");
            assert_eq!(params[0], path[0], "{method}");
            assert!(params[0]["schema"]["pattern"].is_string(), "{method}");
        }
    }

    #[tokio::test]
    async fn the_path_extractor_hands_the_segment_to_the_identity_parse_as_sent() {
        // The pattern is published only: `parse_auth_id` and the rule still
        // answer every segment, so the extractor keeps any text unchanged.
        let router = Router::new().route(
            "/authorizations/{id}",
            get(|ApiPath(AuthIdPath { id }): ApiPath<AuthIdPath>| async move { id }),
        );
        let cases = [
            ("42", "42"),
            ("%2B0042", "+0042"),
            ("-1", "-1"),
            ("abc", "abc"),
            ("1%20", "1 "),
        ];
        for (segment, expected) in cases {
            let request = Request::builder()
                .uri(format!("/authorizations/{segment}"))
                .body(Body::empty())
                .expect("a valid request");
            let response = router
                .clone()
                .oneshot(request)
                .await
                .expect("the router is infallible");
            assert_eq!(response.status(), StatusCode::OK, "{segment}");
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("the body is readable");
            assert_eq!(bytes, expected.as_bytes(), "{segment}");
        }
    }
}
