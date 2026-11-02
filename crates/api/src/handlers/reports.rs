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
//! # Bounded reading and the answer
//!
//! The report runs in one pass that never holds the whole selection and holds
//! the report once, however wide the range:
//!
//! 1. The unit of work is one read-only snapshot (`Db::begin_snapshot`), so
//!    every page and lookup sees the master in the one state the first read
//!    saw, as one read of the whole selection did; a `TRANBKP`, `COMBTRAN` or
//!    CT02 commit meanwhile changes nothing the report reads.
//! 2. The selection is read through `ReportSelection` in pages of
//!    [`PAGE_ROWS`] records, in the `STEP05R` order, and each page is fed to
//!    `ReportRun` before the next one is read.
//! 3. Each record's lines are encoded straight into the response JSON: the
//!    `ReportJson` head, the lines as `ReportLines` array elements, the
//!    end-of-input lines and the tail, byte for byte the `serde_json` text of
//!    the whole `ReportResponse`. No vector of all the lines and no second
//!    copy of the body is built.
//! 4. The body is answered as one piece once the report has run and the unit
//!    of work is rolled back, so every outcome that answers 500 is known
//!    before any byte of the answer is sent: a store failure, the `STEP05R`
//!    stop on a card number with a non-digit byte, which wins over a lookup
//!    abend because the sort ends before `CBTRN03C` starts, and a lookup
//!    abend.
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

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use carddemo_domain::clock::Clock;
use carddemo_domain::ports::{TranTypeFileReader, TransactionReader, XrefReader};
use carddemo_domain::rules::report_request::{self, ReportRange, ReportRequestOutcome};
use carddemo_domain::rules::transaction_report::{ReportRun, ReportSelection};
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::auth::AuthUser;
use crate::dto::reports::{
    ReportJson, ReportJsonError, ReportLines, ReportQuery, ReportResponse, ReportTypeParam,
};
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

/// The most transactions one read of the selection returns. Besides the
/// report's JSON, the pass holds one page of `CVTRA05Y` records at a time, a
/// few hundred kilobytes at most, whatever the range selects.
const PAGE_ROWS: u16 = 1_000;

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
) -> Result<Response, ApiError> {
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
    // Every error listed above is answered before any byte of a 200, as the
    // module documentation describes; the doc comment above is the OpenAPI
    // description.
    let result = respond(&state, query, &span).instrument(span.clone()).await;
    span.record("outcome", outcome(&result));
    result
}

/// CORPT00C PROCESS-ENTER-KEY / SUBMIT-JOB-TO-INTRDR — P-CORPT00C-012,
/// P-CORPT00C-014, P-CORPT00C-017
///
/// The request edits, then the report of the confirmed range: one pass over
/// one read-only snapshot writes the response JSON, answered as one
/// `application/json` body. The range and the number of report records are
/// recorded on `span` as soon as they are known.
async fn respond(state: &AppState, query: ReportQuery, span: &Span) -> Result<Response, ApiError> {
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
    let frame = ReportJson::new(&range, message).map_err(|error| unserializable(&error))?;

    // The report only reads, so its unit of work is always rolled back. The
    // rollback runs before the report's own result is looked at, so an abend
    // leaves no transaction open either.
    let mut uow = state.db().begin_snapshot().await?;
    let report = write_report(&mut uow.store(), &range, &frame).await;
    uow.rollback().await?;
    let report = report?;
    // tracing-opentelemetry exports an unsigned field through its Debug text,
    // a JSON string; an i64 exports as a JSON integer. No report holds more
    // than i64::MAX lines, so the saturation never applies.
    span.record(
        "line_count",
        i64::try_from(report.line_count).unwrap_or(i64::MAX),
    );
    // The body is assembled whole rather than streamed: a streamed body needs
    // a futures `Stream`, a package the api crate graph does not give this
    // crate. Whole, it is also sent with its content-length.
    Ok((
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        Body::from(report.json),
    )
        .into_response())
}

/// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C 1000-TRANFILE-GET-NEXT /
/// 1100-WRITE-TRANSACTION-REPORT — P-CBTRN03C-003 to P-CBTRN03C-013,
/// P-CBTRN03C-474, P-CBTRN03C-477
///
/// The pass: the response JSON, `frame` around the `CVTRA07Y` records
/// `CBTRN03C` writes over `range` in order, and the number of records. The
/// live transactions whose processing date lies in the range, sorted by card
/// number (`STEP05R`), are read page by page and each fed to the run, then the
/// end-of-input lines follow; each record's lines are encoded into the JSON as
/// the run writes them. The end of input adds the last amount a second time
/// and never writes the last card's account total, as the COBOL does
/// (P-CBTRN03C-009, P-CBTRN03C-013).
///
/// Each page is read whole before its records are reported, so `store` is
/// free for the cross-reference, type and category lookups of each record.
///
/// # Errors
///
/// 500 `Unexpected error`, the cause logged:
///
/// - the store fails;
/// - a selected record's card number holds a non-digit byte, whose `STEP05R`
///   sort position no audit row pins. The sort ends before `CBTRN03C` starts,
///   so after an abend the pages are still read to the end, and this stop is
///   the one answered;
/// - a lookup abends the run (P-CBTRN03C-005);
/// - a line cannot be written as JSON.
async fn write_report<S>(
    store: &mut S,
    range: &ReportRange,
    frame: &ReportJson,
) -> Result<WrittenReport, ApiError>
where
    S: TransactionReader + XrefReader + TranTypeFileReader,
{
    let mut selection = ReportSelection::new(range.start, range.end, PAGE_ROWS);
    let mut run = ReportRun::new(range.start, range.end);
    let mut elements = ReportLines::default();
    let mut json = frame.head().to_vec();
    let mut abend = None;
    let mut line_count = 0_usize;
    while let Some(page) = selection.next_page(store).await? {
        if abend.is_some() {
            continue;
        }
        for tran in &page {
            match run.process(store, tran).await {
                Ok(written) => {
                    line_count += written.len();
                    json.extend(
                        elements
                            .encode(&written)
                            .map_err(|error| unserializable(&error))?,
                    );
                }
                Err(error) => {
                    abend = Some(error);
                    break;
                }
            }
        }
    }
    if let Some(unpinned) = selection.unpinned() {
        tracing::error!(error = %unpinned, "report selection cannot be sorted");
        return Err(ApiError::unexpected());
    }
    if let Some(abend) = abend {
        // The abend's DISPLAY goes to the job log, never to the CR00 screen,
        // which had already shown the submission text: the COBOL has no message
        // for the user here, so the answer is the generic 500. The error's
        // Display leaves the card number out of the log.
        tracing::error!(error = %abend, "transaction report abended");
        return Err(ApiError::unexpected());
    }
    // CBTRN03C end of input — P-CBTRN03C-009, P-CBTRN03C-013, P-CBTRN03C-477:
    // the end-of-input lines, then the text after the lines array.
    let written = run.finish();
    line_count += written.len();
    json.extend(
        elements
            .encode(&written)
            .map_err(|error| unserializable(&error))?,
    );
    json.extend_from_slice(frame.tail());
    Ok(WrittenReport { json, line_count })
}

/// The report one pass wrote.
struct WrittenReport {
    /// The complete `ReportResponse` JSON: the head, the lines and the tail.
    json: Vec<u8>,
    /// The number of `CVTRA07Y` records in `json`.
    line_count: usize,
}

/// The generic 500 of a report whose response JSON cannot be written, with
/// `error` logged. `serde_json` does not fail on the strings a report holds.
fn unserializable(error: &ReportJsonError) -> ApiError {
    tracing::error!(error = %error, "report response cannot be serialized");
    ApiError::unexpected()
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
fn outcome(result: &Result<Response, ApiError>) -> &'static str {
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

    use axum::response::Response;

    use super::{OUTCOME_FAILED, OUTCOME_REJECTED, OUTCOME_SUBMITTED, outcome, report_type_name};
    use crate::dto::reports::ReportTypeParam;
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
        // A report is answered with a 200 response carrying its JSON body.
        let cases: [(Result<Response, ApiError>, &str); 4] = [
            (Ok(Response::default()), OUTCOME_SUBMITTED),
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
