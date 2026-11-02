//! CR00 · `CORPT00C` · map `CORPT00` → `GET /reports/transactions`: the
//! transaction report.
//!
//! `CORPT00C` edits the report request and, once it is confirmed, writes the
//! `TRANREPT` job to the TDQ `JOBS`; JES then runs `CBTRN03C` over the live
//! master [app/cbl/CORPT00C.cbl:L208-510] [app/jcl/TRANREPT.jcl]
//! [app/proc/TRANREPT.prc]. The replacement answers the same request
//! synchronously (P-CORPT00C-012, E-34):
//!
//! 1. `report_request::request` runs every `PROCESS-ENTER-KEY` and
//!    `SUBMIT-JOB-TO-INTRDR` edit in the COBOL order, with today from the
//!    processing clock.
//! 2. A confirmed request runs the report the job would print, through the
//!    `CBTRN03C` rules `batch tranrept` shares: the `STEP05R` selection and
//!    sort, then `ReportRun` record by record.
//!
//! The report reads only the live master (the view `transaction_master`),
//! `card_xref`, and the processing copies `trantype_file` and `trancatg_file`.
//! It never reads the Db2 masters `transaction_types` and
//! `transaction_categories`, so an admin edit of a type reaches the report
//! only after `batch seed --only tranextr`, as it reaches `TRANREPT` only after
//! `TRANEXTR` (AAP 0.3.2). It writes nothing: no database row, and no file.
//! The `TRANSACT.BKUP(+1)` backup, the `TRANSACT.DALY(+1)` selection and the
//! `TRANREPT(+1)` report dataset belong to `batch tranrept`; here the report
//! records are the response body.

use axum::Json;
use axum::extract::State;
use carddemo_domain::clock::Clock;
use carddemo_domain::ports::{TranTypeFileReader, TransactionReader, XrefReader};
use carddemo_domain::rules::report_request::{self, ReportRange, ReportRequestOutcome};
use carddemo_domain::rules::transaction_report::{ReportRun, select_for_report};
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::auth::AuthUser;
use crate::dto::reports::{ReportQuery, ReportResponse, ReportTypeParam};
use crate::error::{ApiError, ApiQuery, ErrorBody};
use crate::state::AppState;

/// The CICS transaction the handler span names: `WS-TRANID PIC X(04) VALUE
/// 'CR00'` [app/cbl/CORPT00C.cbl:L38].
const TRANSACTION: &str = "CR00";

/// Span `outcome` of a confirmed request whose report was produced: the
/// `<name> report submitted for printing ...` answer (P-CORPT00C-014).
const OUTCOME_SUBMITTED: &str = "submitted";

/// Span `outcome` of a request answered with a 4xx error body: an edit failed
/// or the request was not confirmed (V-CORPT00C-001 to V-CORPT00C-018).
const OUTCOME_REJECTED: &str = "rejected";

/// Span `outcome` of a request answered with a 500 error body.
const OUTCOME_FAILED: &str = "failed";

/// CORPT00C PROCESS-ENTER-KEY / SUBMIT-JOB-TO-INTRDR — P-CORPT00C-002 to
/// P-CORPT00C-012, P-CORPT00C-014, P-CORPT00C-017, V-CORPT00C-001 to
/// V-CORPT00C-018; CBTRN03C 1000-TRANFILE-GET-NEXT /
/// 1100-WRITE-TRANSACTION-REPORT — P-CBTRN03C-002 to P-CBTRN03C-015,
/// P-TRANREPT-002, P-TRANREPT-003
///
/// Prints the transaction report of the requested range: the current month
/// (`monthly`), the current year (`yearly`) or `start_date` to `end_date`
/// (`custom`), once `confirm` is `Y`. The answer carries the `CVTRA07Y` report
/// records `CBTRN03C` writes for that range, untrimmed and in order, and the
/// `<name> report submitted for printing ...` message.
///
/// # Errors
///
/// - 400 with the audited `CORPT00C` message and its BMS field when an edit
///   fails or the request is not confirmed: no report type, an empty or
///   invalid custom date part, a date `CSUTLDTC` rejects, a blank, `N` or
///   other confirmation (V-CORPT00C-001 to V-CORPT00C-018); also a query
///   value longer than its field.
/// - 401 without a valid bearer token.
/// - 500 `Unexpected error` when the database fails, when a report record's
///   cross-reference, type or category is missing from `card_xref`,
///   `trantype_file` or `trancatg_file` (the `CBTRN03C` abend, P-CBTRN03C-005),
///   or when the outcome is one the audit holds at halt.
#[utoipa::path(
    get,
    path = "/reports/transactions",
    tag = "reports",
    params(ReportQuery),
    responses(
        (status = 200, description = "The report of the confirmed range", body = ReportResponse),
        (status = 400, description = "An edit failed, or no confirmation", body = ErrorBody),
        (status = 401, description = "No valid bearer token", body = ErrorBody),
        (status = 500, description = "The report could not be produced", body = ErrorBody),
    ),
    security(("bearer_auth" = [])),
)]
pub async fn transaction_report(
    user: AuthUser,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ReportQuery>,
) -> Result<Json<ReportResponse>, ApiError> {
    let span = tracing::info_span!(
        "handler",
        transaction = TRANSACTION,
        user_id = %user.user_id,
        report_type = Empty,
        start_date = Empty,
        end_date = Empty,
        line_count = Empty,
        outcome = Empty,
    );
    if let Some(report_type) = query.report_type {
        span.record("report_type", report_type_name(report_type));
    }
    let result = respond(&state, query, &span).instrument(span.clone()).await;
    span.record("outcome", outcome(&result));
    result.map(Json)
}

/// CORPT00C PROCESS-ENTER-KEY / SUBMIT-JOB-TO-INTRDR — P-CORPT00C-012,
/// P-CORPT00C-014, P-CORPT00C-017
///
/// The request edits, then the report of the confirmed range. The range and
/// the number of report records are recorded on `span` as soon as they are
/// known.
async fn respond(
    state: &AppState,
    query: ReportQuery,
    span: &Span,
) -> Result<ReportResponse, ApiError> {
    query.check()?;
    let input = query.into_input();
    // FUNCTION CURRENT-DATE [app/cbl/CORPT00C.cbl:L215] [app/cbl/CORPT00C.cbl:L241]:
    // the monthly and yearly ranges follow the processing clock, which
    // CARDDEMO_CLOCK_OVERRIDE pins.
    let today = state.clock().now().date_naive();
    // Submitted is the only outcome that is not an error. Every other answer of
    // CORPT00C sets WS-ERR-FLG, the confirmation prompt and the N that clears
    // the form included (V-CORPT00C-016, V-CORPT00C-017, E-49), so the rule
    // returns it as a DomainError, a 400 error body: no request answers 200
    // with a message and no report lines.
    let ReportRequestOutcome::Submitted { range, message } =
        report_request::request(&input, today)?;
    span.record("start_date", &*range.start.trimmed());
    span.record("end_date", &*range.end.trimmed());

    // The report only reads, so its unit of work is always rolled back. The
    // rollback runs before the report's own result is looked at, so an abend
    // leaves no transaction open either.
    let mut uow = state.db().begin().await?;
    let lines = report_lines(&mut uow.store(), &range).await;
    uow.rollback().await?;
    let lines = lines?;
    // tracing-opentelemetry exports an unsigned field through its Debug text,
    // a JSON string; an i64 exports as a JSON integer. No vector holds more
    // than i64::MAX lines, so the saturation never applies.
    span.record("line_count", i64::try_from(lines.len()).unwrap_or(i64::MAX));
    Ok(ReportResponse::generated(&range, lines, message))
}

/// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C 1000-TRANFILE-GET-NEXT /
/// 1100-WRITE-TRANSACTION-REPORT — P-CBTRN03C-003 to P-CBTRN03C-013,
/// P-CBTRN03C-474, P-CBTRN03C-477
///
/// The `CVTRA07Y` records `CBTRN03C` writes over `range`, in order: the live
/// transactions whose processing date lies in the range, sorted by card number
/// (`STEP05R`), each fed to the run, then the end-of-input lines. The end of
/// input adds the last amount a second time and never writes the last card's
/// account total, as the COBOL does (P-CBTRN03C-009, P-CBTRN03C-013).
///
/// The selection is read into memory first, so `store` is free for the
/// cross-reference, type and category lookups of each record.
///
/// # Errors
///
/// 500 `Unexpected error`, the cause logged:
///
/// - the store fails;
/// - a selected record's card number holds a non-digit byte, whose `STEP05R`
///   sort position no audit row pins;
/// - a lookup abends the run (P-CBTRN03C-005).
async fn report_lines<S>(store: &mut S, range: &ReportRange) -> Result<Vec<String>, ApiError>
where
    S: TransactionReader + XrefReader + TranTypeFileReader,
{
    let records = store
        .transactions_by_proc_date(range.start, range.end)
        .await?;
    let selected = select_for_report(&range.start, &range.end, records).map_err(|unpinned| {
        tracing::error!(error = %unpinned, "report selection cannot be sorted");
        ApiError::unexpected()
    })?;
    let mut run = ReportRun::new(range.start, range.end);
    let mut lines = Vec::new();
    for tran in &selected {
        // The abend's DISPLAY goes to the job log, never to the CR00 screen,
        // which had already shown the submission text: the COBOL has no message
        // for the user here, so the answer is the generic 500. The error's
        // Display leaves the card number out of the log.
        let written = run.process(store, tran).await.map_err(|abend| {
            tracing::error!(error = %abend, "transaction report abended");
            ApiError::unexpected()
        })?;
        lines.extend(written);
    }
    lines.extend(run.finish());
    Ok(lines)
}

/// The `report_type` query value the span records, as the query carries it.
const fn report_type_name(report_type: ReportTypeParam) -> &'static str {
    match report_type {
        ReportTypeParam::Monthly => "monthly",
        ReportTypeParam::Yearly => "yearly",
        ReportTypeParam::Custom => "custom",
    }
}

/// The span `outcome` of a result: [`OUTCOME_SUBMITTED`] for a report,
/// [`OUTCOME_FAILED`] for a 5xx error, [`OUTCOME_REJECTED`] for any other.
fn outcome(result: &Result<ReportResponse, ApiError>) -> &'static str {
    match result {
        Ok(_) => OUTCOME_SUBMITTED,
        Err(error) if error.status().is_server_error() => OUTCOME_FAILED,
        Err(_) => OUTCOME_REJECTED,
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::error::DomainError;
    use carddemo_domain::messages;
    use serde_json::Value;

    use super::{OUTCOME_FAILED, OUTCOME_REJECTED, OUTCOME_SUBMITTED, outcome, report_type_name};
    use crate::dto::reports::{ReportResponse, ReportTypeParam};
    use crate::error::ApiError;

    #[test]
    fn report_type_name_is_the_query_value() {
        for param in [
            ReportTypeParam::Monthly,
            ReportTypeParam::Yearly,
            ReportTypeParam::Custom,
        ] {
            let wire = serde_json::to_value(param).expect("a report type serializes");
            assert_eq!(wire, Value::from(report_type_name(param)), "{param:?}");
        }
    }

    #[test]
    fn outcome_follows_the_answer() {
        let report = ReportResponse {
            report_name: messages::REPORT_NAME_MONTHLY.to_owned(),
            start_date: "2022-07-01".to_owned(),
            end_date: "2022-07-31".to_owned(),
            lines: Vec::new(),
            message: messages::report_submitted_message(messages::REPORT_NAME_MONTHLY),
        };
        let cases: [(Result<ReportResponse, ApiError>, &str); 4] = [
            (Ok(report), OUTCOME_SUBMITTED),
            (
                Err(ApiError::from(DomainError::empty(
                    messages::SELECT_A_REPORT_TYPE_TO_PRINT_REPORT,
                ))),
                OUTCOME_REJECTED,
            ),
            (Err(ApiError::unauthenticated()), OUTCOME_REJECTED),
            (Err(ApiError::unexpected()), OUTCOME_FAILED),
        ];
        for (result, expected) in cases {
            assert_eq!(outcome(&result), expected, "{result:?}");
        }
    }
}
