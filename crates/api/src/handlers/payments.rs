//! CB00 bill payment (`COBIL00C`, map `COBIL00`): `POST /payments`.
//!
//! One request is one ENTER on the `COBIL0A` screen: the account ID
//! (`ACTIDIN`) and the `Y`/`N` confirmation (`CONFIRM`) travel in the body,
//! and `confirm` takes the place of the second ENTER the 3270 operator gives
//! after reading the balance. The rules are `carddemo_domain::rules::
//! bill_payment`; this module runs them in the COBOL order of
//! `PROCESS-ENTER-KEY` [app/cbl/COBIL00C.cbl:L154-244], the store-free entry
//! edits before it opens the unit of work and the reads and writes inside it,
//! and answers:
//!
//! | Outcome | Status | Writes |
//! |---|---|---|
//! | An entry edit rejects the request (V-COBIL00C-001, V-COBIL00C-003) | 400 | none, no unit of work opened |
//! | Confirmation `N`: the screen is cleared with no message (V-COBIL00C-002, E-49) | 400 | none, no unit of work opened |
//! | A read or the balance guard rejects the request (V-COBIL00C-004 to V-COBIL00C-007, V-COBIL00C-009, V-COBIL00C-010) | its kind's status | none, rolled back |
//! | Confirmation blank: the balance and `Confirm to make a bill payment...` (V-COBIL00C-008) | 200 | none, rolled back |
//! | No TRAN-ID can be allocated (V-COBIL00C-011 to V-COBIL00C-013, E-27) | 500 | none, rolled back |
//! | Confirmation `Y`: the payment transaction and the reduced balance (P-COBIL00C-016) | 201 | both, committed |
//! | Confirmation `Y` with a failed write (V-COBIL00C-014 to V-COBIL00C-017, E-19) | its kind's status | what succeeded, committed |
//!
//! # Write persistence (E-19)
//!
//! `COBIL00C` writes the payment transaction and then reduces and rewrites
//! the balance whatever the write answered [app/cbl/COBIL00C.cbl:L233-235],
//! and both files are `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96], so a
//! completed write survives the failure of the other (P-COBIL00C-016,
//! P-COBIL00C-024). The store runs each write under its own savepoint, and
//! this handler commits the unit of work once `bill_payment::pay` returns,
//! whether it succeeded or not. Only a request that wrote nothing is rolled
//! back.
//!
//! Bill payments accrue no reward points: the reward engine serves only CT02
//! and POSTTRAN.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use carddemo_domain::error::{DomainError, ErrorKind};
use carddemo_domain::messages::UNABLE_TO_LOOKUP_TRANSACTION;
use carddemo_domain::rules::bill_payment::{
    self, BillPaymentInput, BillPaymentStep, EntryEdit, FIELD_ACTIDIN, PaymentDraft,
};
use carddemo_persistence::UnitOfWork;
use tracing::field::{Empty, display};
use tracing::{Instrument, Span};

use crate::auth::AuthUser;
use crate::dto::bms_errors;
use crate::dto::payments::{BillPaymentField, PaymentRequest, PaymentResponse};
use crate::error::{ApiError, ApiJson, ErrorBody};
use crate::state::AppState;

/// The CICS transaction this handler replaces, the span's `transaction`.
const TRANSACTION: &str = "CB00";

/// The span `outcome` of the blank-confirmation prompt: answered, nothing
/// written.
const OUTCOME_PROMPTED: &str = "prompted";

/// The span `outcome` of a confirmed payment whose unit of work committed,
/// both writes or, after a failed write, the one that succeeded (E-19).
const OUTCOME_COMMITTED: &str = "committed";

/// The span `outcome` of a confirmed payment that persisted nothing: the unit
/// of work could not be opened, no TRAN-ID could be allocated, or the commit
/// failed.
const OUTCOME_ROLLED_BACK: &str = "rolled_back";

/// The span `outcome` of a request an edit or a read answered with a 4xx
/// status before any write: the length check, the account-ID and
/// confirmation edits, an account or cross-reference not on file, the
/// nothing-to-pay guard and the `N` confirmation.
const OUTCOME_REJECTED: &str = "rejected";

/// The span `outcome` of a request answered with a 5xx status before any
/// write: a failed account or cross-reference read.
const OUTCOME_FAILED: &str = "failed";

/// The text of the `N` confirmation: none. `CLEAR-CURRENT-SCREEN` sets no
/// message [app/cbl/COBIL00C.cbl:L178-181], and E-49 names no text for it
/// (V-COBIL00C-002).
const NO_MESSAGE: &str = "";

/// The answer to a request and the status it is sent with.
type PaymentAnswer = Result<(StatusCode, Json<PaymentResponse>), ApiError>;

/// COBIL00C PROCESS-ENTER-KEY / WRITE-TRANSACT-FILE / UPDATE-ACCTDAT-FILE —
/// P-COBIL00C-003 to P-COBIL00C-019, P-COBIL00C-024; V-COBIL00C-001 to
/// V-COBIL00C-017 (E-19).
///
/// Pays the account balance in full with an online bill-payment transaction:
/// `confirm` blank shows the balance and asks for confirmation (200, nothing
/// written), `Y` writes the type `02` / category 0002 `BILL PAYMENT - ONLINE`
/// transaction for the balance and rewrites the account with the balance
/// reduced by it (201), `N` cancels (400, no message).
///
/// The actor is the signed-on user of the bearer token. As on the 3270
/// screen, any signed-on user may pay any account. The handler span carries
/// `transaction = "CB00"`, `user_id`, `account_id`, `tran_id` and `outcome`
/// (`prompted`, `committed`, `rolled_back`, `rejected` for a 4xx answer before
/// any write or `failed` for a 5xx one), the last recorded once the unit of
/// work is committed or rolled back.
///
/// # Errors
///
/// - 401 without a valid bearer token.
/// - 400 for a body that does not deserialize, a field longer than its BMS
///   field (no message), an empty account ID (`Acct ID can NOT be empty...`),
///   an invalid confirmation (`Invalid value. Valid values are (Y/N)...`), the
///   `N` confirmation (no message, `ACTIDIN`) and a balance of zero or less
///   (`You have nothing to pay...`).
/// - 404 `Account ID NOT found...` for an account or cross-reference that is
///   not on file, or an account rewrite that finds no record.
/// - 409 `Tran ID already exist...` when the payment transaction's key is
///   taken; the reduced balance is still rewritten and committed (E-19).
/// - 500 `Unable to lookup Account...`, `Unable to lookup XREF AIX file...`,
///   `Unable to lookup Transaction...` (no TRAN-ID could be allocated),
///   `Unable to Add Bill pay Transaction...` (the balance is still rewritten,
///   E-19) or `Unable to Update Account...` (the transaction write keeps its
///   outcome, E-19); `Unexpected error` when the unit of work cannot be
///   opened or committed.
#[utoipa::path(
    post,
    path = "/payments",
    tag = "payments",
    summary = "Pay an account's balance in full (CB00)",
    description = "One ENTER on the COBIL00 bill-payment screen. `confirm` blank shows the \
        balance and asks for confirmation without writing (200); `Y` writes the bill-payment \
        transaction for the whole balance and reduces the balance by it (201); `N` cancels \
        (400, no message). After a failed write of a confirmed payment the other write is \
        kept, as the COBOL program keeps it.",
    request_body = PaymentRequest,
    responses(
        (status = 201, description = "payment made: the transaction written and the balance reduced", body = PaymentResponse),
        (status = 200, description = "confirm prompt, no write", body = PaymentResponse),
        (status = 400, description = "an edit rejected the request, or the payment was cancelled", body = ErrorBody),
        (status = 401, description = "no valid bearer token", body = ErrorBody),
        (status = 404, description = "the account or its cross-reference is not on file", body = ErrorBody),
        (status = 409, description = "the payment's TRAN-ID is already on file", body = ErrorBody),
        (status = 500, description = "a read, the TRAN-ID allocation or a write failed", body = ErrorBody),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_payment(
    user: AuthUser,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PaymentRequest>,
) -> PaymentAnswer {
    let AuthUser { user_id, .. } = user;
    let span = tracing::info_span!(
        "create_payment",
        transaction = TRANSACTION,
        user_id = %user_id,
        account_id = Empty,
        tran_id = Empty,
        outcome = Empty,
    );
    process(&state, body).instrument(span).await
}

/// COBIL00C PROCESS-ENTER-KEY up to the confirmation — P-COBIL00C-004 to
/// P-COBIL00C-011; V-COBIL00C-001 to V-COBIL00C-010
/// [app/cbl/COBIL00C.cbl:L156-211].
///
/// The length check and the store-free [`entry_edits`], which answer before
/// this handler opens a unit of work, then `bill_payment::prepare` in a fresh
/// unit of work, which repeats those edits with the same answer. Every
/// outcome but [`BillPaymentStep::Ready`] wrote nothing and is rolled back
/// here; `Ready` goes on to [`complete_payment`].
async fn process(state: &AppState, body: PaymentRequest) -> PaymentAnswer {
    let span = Span::current();
    if let Err(error) = body.check() {
        span.record("outcome", OUTCOME_REJECTED);
        return Err(error);
    }
    let input = body.into_input();
    let account_key = input.account_key();
    if let Some(account_id) = account_key {
        span.record("account_id", display(account_id));
    }
    if let Err(error) = entry_edits(&input) {
        span.record("outcome", OUTCOME_REJECTED);
        return Err(error);
    }
    let mut uow = match state.db().begin().await {
        Ok(uow) => uow,
        Err(error) => {
            span.record("outcome", OUTCOME_ROLLED_BACK);
            return Err(error.into());
        }
    };
    let step = bill_payment::prepare(&mut uow.store(), input).await;
    match step {
        Err(error) => {
            discard(uow).await;
            let error = ApiError::from(error);
            span.record("outcome", unwritten_outcome(&error));
            Err(error)
        }
        Ok(BillPaymentStep::Cancelled) => {
            discard(uow).await;
            span.record("outcome", OUTCOME_REJECTED);
            Err(cancelled())
        }
        Ok(BillPaymentStep::ConfirmPrompt {
            message,
            current_balance,
        }) => {
            discard(uow).await;
            // `prepare` reads the account only through the key the entry
            // gives, and answers an entry without one as not found, so a
            // prompt always comes with a key.
            let Some(account_id) = account_key else {
                tracing::error!("bill payment prompt without an account key");
                span.record("outcome", OUTCOME_FAILED);
                return Err(ApiError::unexpected());
            };
            span.record("outcome", OUTCOME_PROMPTED);
            Ok((
                StatusCode::OK,
                Json(PaymentResponse::confirm_prompt(
                    account_id,
                    current_balance,
                    message.to_owned(),
                )),
            ))
        }
        Ok(BillPaymentStep::Ready(draft)) => complete_payment(state, uow, draft).await,
    }
}

/// COBIL00C PROCESS-ENTER-KEY — P-COBIL00C-004 to P-COBIL00C-008;
/// V-COBIL00C-001 to V-COBIL00C-003 [app/cbl/COBIL00C.cbl:L156-191]: the
/// edits of the entry that come before `READ-ACCTDAT-FILE`, answered before
/// the unit of work is opened.
///
/// [`bill_payment::edit_entry`] reads no store, so its rejections and the `N`
/// cancellation give their audited answers before this handler opens its
/// unit of work. The request reaches the handler only once [`AuthUser`] has
/// read the signed-on user from `USRSEC`, so when the database cannot be
/// reached the request answers 500 from that read before these edits run.
/// `Ok` leaves `Y`, `y` and blank to the reads of `bill_payment::prepare`,
/// which edits the entry again with the same answer.
///
/// # Errors
///
/// 400 with code `EMPTY` and `Acct ID can NOT be empty...` on `ACTIDIN` for a
/// blank account ID; 400 with code `INVALID`, no message and the field
/// `ACTIDIN` for the `N` confirmation ([`cancelled`]); 400 with code
/// `INVALID` and `Invalid value. Valid values are (Y/N)...` on `CONFIRM` for
/// any other confirmation value.
fn entry_edits(input: &BillPaymentInput) -> Result<(), ApiError> {
    match bill_payment::edit_entry(input)? {
        EntryEdit::Cancelled => Err(cancelled()),
        EntryEdit::Confirmed | EntryEdit::Unconfirmed => Ok(()),
    }
}

/// COBIL00C PROCESS-ENTER-KEY, the confirmed payment — P-COBIL00C-012 to
/// P-COBIL00C-019, P-COBIL00C-024; V-COBIL00C-011 to V-COBIL00C-017 (E-19)
/// [app/cbl/COBIL00C.cbl:L212-235].
///
/// Allocates the TRAN-ID, runs `bill_payment::pay`, then commits whatever
/// `pay` returned: a failed transaction write leaves the balance rewrite in
/// place and a failed rewrite leaves the transaction write in place, as the
/// `RECOVERY(NONE)` files keep them. A failed allocation wrote nothing and is
/// rolled back. The account row lock `prepare` took precedes the allocation's
/// advisory lock, the order `carddemo_persistence::ids` documents.
async fn complete_payment(
    state: &AppState,
    mut uow: UnitOfWork,
    draft: PaymentDraft,
) -> PaymentAnswer {
    let span = Span::current();
    let tran_id = match uow.next_tran_id().await {
        Ok(tran_id) => tran_id,
        Err(error) => {
            tracing::warn!(error = %error, "TRAN-ID allocation failed");
            discard(uow).await;
            span.record("outcome", OUTCOME_ROLLED_BACK);
            return Err(allocation_failed());
        }
    };
    span.record("tran_id", tran_id.trimmed().as_ref());
    let paid = bill_payment::pay(&mut uow.store(), draft, tran_id, state.clock()).await;
    if let Err(error) = uow.commit().await {
        span.record("outcome", OUTCOME_ROLLED_BACK);
        return Err(error.into());
    }
    span.record("outcome", OUTCOME_COMMITTED);
    match paid {
        Ok(success) => Ok((StatusCode::CREATED, Json(PaymentResponse::paid(&success)))),
        Err(error) => {
            tracing::warn!(
                code = error.code,
                "bill payment write failed; the other write was committed"
            );
            Err(error.into())
        }
    }
}

/// The span `outcome` of `error` answering a request an edit or a read
/// refused before any write: [`OUTCOME_FAILED`] for a 5xx status,
/// [`OUTCOME_REJECTED`] for any other.
fn unwritten_outcome(error: &ApiError) -> &'static str {
    if error.status().is_server_error() {
        OUTCOME_FAILED
    } else {
        OUTCOME_REJECTED
    }
}

/// Rolls back a unit of work that wrote nothing.
///
/// A failed `ROLLBACK` is logged and the request keeps its answer: nothing
/// was written, and the server rolls back a transaction whose session ends.
async fn discard(uow: UnitOfWork) {
    if let Err(error) = uow.rollback().await {
        tracing::warn!(error = %error, "rollback of the bill payment unit of work failed");
    }
}

/// COBIL00C PROCESS-ENTER-KEY, CLEAR-CURRENT-SCREEN — P-COBIL00C-006,
/// P-COBIL00C-030; V-COBIL00C-002 (E-49).
///
/// The `N` confirmation clears the screen and sets `WS-ERR-FLG` with no
/// message, so nothing is read or written [app/cbl/COBIL00C.cbl:L178-181].
/// The audited answer is 400 `Invalid` with no message and the cursor field
/// `ACTIDIN`.
fn cancelled() -> ApiError {
    ApiError::no_message(
        ErrorKind::Invalid,
        NO_MESSAGE,
        bms_errors([BillPaymentField::ActIdIn], NO_MESSAGE),
    )
}

/// COBIL00C STARTBR-TRANSACT-FILE, READPREV-TRANSACT-FILE — P-COBIL00C-013,
/// P-COBIL00C-014; V-COBIL00C-011 to V-COBIL00C-013 (E-27).
///
/// Every failure of the TRAN-ID allocation shows `Unable to lookup
/// Transaction...` on `ACTIDIN` and writes nothing: the empty live master,
/// whose `STARTBR` `NOTFND` text the `READPREV` without a browse replaces
/// before the `ENDBR` abends, and any failed browse
/// [app/cbl/COBIL00C.cbl:L441-496].
///
/// The audit records this COBOL text for the failure, so the 500 carries it
/// rather than the generic `Unexpected error` (`ApiError::unexpected`).
fn allocation_failed() -> ApiError {
    DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION)
        .with_field(FIELD_ACTIDIN)
        .into()
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages::{
        self, ACCT_ID_CAN_NOT_BE_EMPTY, INVALID_VALUE_VALID_VALUES_ARE_Y_N,
    };
    use carddemo_domain::rules::bill_payment::{FIELD_ACTIDIN, FIELD_CONFIRM};
    use serde_json::{Value, json};

    use super::{OUTCOME_FAILED, OUTCOME_REJECTED, cancelled, entry_edits, unwritten_outcome};
    use crate::dto::payments::PaymentRequest;
    use crate::error::ApiError;

    #[test]
    fn a_refusal_before_any_write_is_failed_for_a_5xx_status_and_rejected_otherwise() {
        // (`prepare` error, `outcome`) — V-COBIL00C-001, V-COBIL00C-003 to
        // V-COBIL00C-007, V-COBIL00C-009.
        let cases = [
            (
                DomainError::empty(messages::ACCT_ID_CAN_NOT_BE_EMPTY),
                OUTCOME_REJECTED,
            ),
            (
                DomainError::not_found(messages::ACCOUNT_ID_NOT_FOUND),
                OUTCOME_REJECTED,
            ),
            (
                DomainError::invalid(messages::YOU_HAVE_NOTHING_TO_PAY),
                OUTCOME_REJECTED,
            ),
            (
                DomainError::unexpected(messages::UNABLE_TO_LOOKUP_ACCOUNT),
                OUTCOME_FAILED,
            ),
            (
                DomainError::unexpected(messages::UNABLE_TO_LOOKUP_XREF_AIX_FILE),
                OUTCOME_FAILED,
            ),
        ];
        for (error, expected) in cases {
            let error = ApiError::from(error.with_field(FIELD_ACTIDIN));
            assert_eq!(unwritten_outcome(&error), expected, "{error:?}");
        }
        // The status alone decides: the 400 of the `N` confirmation
        // (V-COBIL00C-002), and a 500 no rule chose a message for.
        assert_eq!(unwritten_outcome(&cancelled()), OUTCOME_REJECTED);
        assert_eq!(unwritten_outcome(&ApiError::unexpected()), OUTCOME_FAILED);
    }

    /// The status and JSON body an error answers with.
    fn answer(error: &ApiError) -> (StatusCode, Value) {
        let body = serde_json::to_value(error.body()).expect("serializes");
        (error.status(), body)
    }

    /// The 400 answer of an entry edit: `message` on the one BMS field
    /// `field`.
    fn rejection(kind: ErrorKind, message: &str, field: &str) -> (StatusCode, Value) {
        let body = json!({
            "code": kind.code(),
            "message": message,
            "fields": [{"field": field, "message": message}],
        });
        (StatusCode::BAD_REQUEST, body)
    }

    #[test]
    fn the_cb00_entry_edits_answer_without_opening_the_store() {
        // COBIL00C PROCESS-ENTER-KEY: V-COBIL00C-001 (order 1), V-COBIL00C-002
        // and V-COBIL00C-003 (order 2) come before READ-ACCTDAT-FILE, so the
        // edits read no store; Y, y and blank go on to the reads.
        let empty = Err(rejection(
            ErrorKind::Empty,
            ACCT_ID_CAN_NOT_BE_EMPTY,
            FIELD_ACTIDIN,
        ));
        let cancelled = Err(rejection(ErrorKind::Invalid, "", FIELD_ACTIDIN));
        let invalid = Err(rejection(
            ErrorKind::Invalid,
            INVALID_VALUE_VALID_VALUES_ARE_Y_N,
            FIELD_CONFIRM,
        ));
        let cases = [
            (Some(""), Some(""), empty.clone()),
            (None, Some("Y"), empty.clone()),
            (Some("   "), Some("X"), empty),
            (Some("00000000006"), Some("N"), cancelled.clone()),
            (Some("00000000006"), Some("n"), cancelled),
            (Some("00000000006"), Some("X"), invalid.clone()),
            (Some("99999999999"), Some("?"), invalid),
            (Some("00000000006"), Some("Y"), Ok(())),
            (Some("00000000006"), Some("y"), Ok(())),
            (Some("00000000006"), Some(""), Ok(())),
            (Some("00000000006"), None, Ok(())),
        ];
        for (account_id, confirm, expected) in cases {
            let input = PaymentRequest {
                account_id: account_id.map(str::to_owned),
                confirm: confirm.map(str::to_owned),
            }
            .into_input();
            let outcome = entry_edits(&input).map_err(|error| answer(&error));
            assert_eq!(outcome, expected, "{account_id:?} {confirm:?}");
        }
    }
}
