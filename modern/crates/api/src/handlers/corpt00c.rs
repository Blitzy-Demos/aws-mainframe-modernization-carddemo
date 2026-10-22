//! Endpoint 14, `POST /api/v1/reports/transactions` (transaction `CR00`, program CORPT00C, map
//! `CORPT0A`): the thin handler of the transaction report request.
//!
//! ENTER is the only key the endpoint serves. PF3, back to the main menu, is client-side and calls
//! nothing. A confirmed request writes the in-line job deck CORPT00C sends to TDQ `JOBS` as one
//! generation file `TRNRPT00.G<nnnn>` in `CARDDEMO_INTRDR_DIR`, and nothing else: no report is
//! produced and no job is run, since the operator runs the report with `carddemo-batch tranrept`,
//! as JES ran the submitted job (AAP 0.3.4.4; CORPT00C-036, CORPT00C-039).
//!
//! The answer is `ReportResult`: the message row and, when a deck was written, `job_deck`, the
//! generation file's name alone and never a directory or an absolute path. Report and statement
//! files hold full card numbers, so no stream and no file content leaves here (Cardholder Data
//! Redaction; CORPT00C-038).
//!
//! The handler extracts the verified identity and the validated body, calls
//! `services::corpt00c::submit` and maps its outcome through the `From` conversions of
//! `dto::corpt00c` and `error` (Thin Request Handlers). The edits, the confirmation and the deck
//! write, the crate's only filesystem access, all live in the service (AAP 0.5.3). The date parts
//! reach it as the raw text the screen sent, the raw-edit-input exception of Typed Dates and
//! Timestamps, so that CORPT00C's own month, day and year messages answer them.

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidJson;
use crate::dto::corpt00c::{ReportRequest, ReportResult};
use crate::error::ScreenError;
use crate::services;

/// Submit the transaction report (CR00 / CORPT00C)
///
/// ENTER on the report screen: the Monthly or Yearly report of the current date, or a Custom report
/// of the start and end dates the request carries, each date part as the screen typed it. A
/// confirmation of `Y` or `y` writes the job deck that runs the report and answers the submission
/// message, such as `Monthly report submitted for printing ...`, with `job_deck`, the name of the
/// deck's generation file, such as `TRNRPT00.G0001`. A confirmation of `N` or `n` clears the
/// screen: an empty message and no `job_deck`. The report itself is not produced here. PF3 is
/// client-side.
///
/// # Errors
///
/// A `ScreenError` with the status CORPT00C's audit assigns:
///
/// - 400: a body that does not fit the map's fields, no report selected, a blank or invalid date
///   part, a date that fails the date check, or a blank or invalid confirmation, each with the
///   program's message and the field it names.
/// - 401: no valid bearer token.
/// - 500: `Unable to Write TDQ (JOBS)...` when the job deck cannot be written, or
///   `Unable to process request...` for an internal failure.
#[utoipa::path(
    post,
    path = "/api/v1/reports/transactions",
    operation_id = "corpt00c_submit",
    tag = "CORPT00C",
    request_body = ReportRequest,
    responses(
        (
            status = 200,
            description = "Submitted, with the job deck's file name, or cleared",
            body = ReportResult
        ),
        (
            status = 400,
            description = "A map field, report selection, date part or confirmation edit failed",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 500,
            description = "The job deck could not be written, or an internal failure",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn submit(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidJson(request): ValidJson<ReportRequest>,
) -> Result<Json<ReportResult>, ScreenError> {
    services::corpt00c::submit(&state, &user, request)
        .await
        .map(ReportResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
