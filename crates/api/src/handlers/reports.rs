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
//! However wide the range, a report holds neither the whole selection nor
//! the whole answer, and it runs under the report admission of `AppState`,
//! which every report shares:
//!
//! 1. A confirmed request waits for a report permit
//!    (`AppState::admit_report`) before it opens its unit of work, in arrival
//!    order, never refused and never timed out, holding no pooled
//!    connection. The response body keeps the permit until it is sent or
//!    dropped, so at most `report_permits` reports run or send at once, each
//!    holding one pooled connection and one snapshot, and the other
//!    operations keep the rest of the pool.
//! 2. The unit of work is one read-only snapshot (`Db::begin_snapshot`), so
//!    every page and lookup sees the master in the one state the first read
//!    saw, as one read of the whole selection did; a `TRANBKP`, `COMBTRAN` or
//!    CT02 commit meanwhile changes nothing the report reads.
//! 3. The selection is read through `ReportSelection` in pages of
//!    [`PAGE_ROWS`] records, in the `STEP05R` order, and each page is fed to
//!    `ReportRun` before the next one is read. Each record's lines are encoded
//!    as the run writes them: the `ReportJson` head, the lines as
//!    `ReportLines` array elements, the end-of-input lines and the tail, byte
//!    for byte the `serde_json` text of the whole `ReportResponse`. No vector
//!    of all the lines is built.
//! 4. The report runs twice over the one snapshot. The first pass keeps only
//!    the length of the JSON, and it ends with every outcome that answers
//!    500: a store failure, the `STEP05R` stop on a card number with a
//!    non-digit byte, which wins over a lookup abend because the sort ends
//!    before `CBTRN03C` starts, and a lookup abend. Every 500 is therefore
//!    answered before any byte of a 200, and the 200 carries that length as
//!    its `Content-Length`.
//! 5. The second pass is the response body. The server polls the body for
//!    its next chunk only once its own bounded write buffer has room, and
//!    each poll runs the pass until one [`CHUNK_BYTES`] chunk of JSON is
//!    full. Besides its connection and snapshot, a report therefore holds one
//!    page of records, the chunk it is filling and what the server's write
//!    buffer holds, whatever its size, and a client that reads slowly holds
//!    the pass back instead of letting the answer pile up. Once the tail is
//!    written, the unit of work is rolled back before the last chunk is
//!    sent.
//!
//! The snapshot gives the second pass the records of the first, so it does
//! not fail where the first succeeded. Should it fail all the same, on a lost
//! connection, a failed rollback or a length other than the first pass's,
//! the cause is logged and the body ends in an error short of its
//! `Content-Length`: the server aborts the answer and the client sees it
//! incomplete, never a report that looks whole and is cut short. No report
//! is refused or cut short to fit a limit.
//!
//! A client that reads slowly, or not at all, keeps its report's permit and
//! connection until it has read the answer or disconnected: the API has no
//! timeout, rate limiting or proxy in front of it. Once the body is dropped,
//! sent or abandoned, the permit is back, and a unit of work it still holds
//! rolls back as it drops. A request dropped while it waits for a permit
//! returns what it holds as it is dropped.
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

use std::mem;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use carddemo_domain::clock::Clock;
use carddemo_domain::rules::report_request::{self, ReportRange, ReportRequestOutcome};
use carddemo_domain::rules::transaction_report::{
    ReportAbend, ReportRun, ReportSelection, TranTypeFileReader, Transaction, TransactionReader,
    XrefReader,
};
use carddemo_persistence::tx::UnitOfWork;
use futures_util::stream;
use tokio::sync::OwnedSemaphorePermit;
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

/// The most transactions one read of the selection returns. Besides the JSON
/// it writes, a pass holds one page of `CVTRA05Y` records at a time, a few
/// hundred kilobytes at most, whatever the range selects.
const PAGE_ROWS: u16 = 1_000;

/// The bytes of response JSON in one chunk of a report's body, 64 KiB: every
/// chunk but the last is exactly this long, and the last is no longer.
pub const CHUNK_BYTES: usize = 64 * 1024;

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
///   other confirmation (V-CORPT00C-001 to V-CORPT00C-018).
/// - 400 with no message, before those edits, when a query value is longer
///   than its field (on `SDTYYYY`, `EDTYYYY` or `CONFIRM`) or the confirmation
///   holds a character above `U+007F` (on `CONFIRM`).
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
    let result = crate::cancellable(
        span.clone(),
        respond(&state, query, &span).instrument(span.clone()),
    )
    .await;
    span.record("outcome", outcome(&result));
    result
}

/// CORPT00C PROCESS-ENTER-KEY / SUBMIT-JOB-TO-INTRDR — P-CORPT00C-012,
/// P-CORPT00C-014, P-CORPT00C-017
///
/// The request edits, then the report of the confirmed range: once the report
/// admission lets it run, a first pass over one read-only snapshot measures
/// the response JSON, and the answer is an `application/json` body of that
/// length, which a second pass over the same snapshot writes as it is sent.
/// The range and the number of report records are recorded on `span` as soon
/// as they are known.
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

    // The permit is taken before the unit of work opens, so a queued report
    // holds no pooled connection, and the response body keeps it.
    let admission = state.admit_report().await?;
    let mut uow = state.db().begin_snapshot().await?;
    let mut measured = Measured::default();
    let first = write_report(&mut uow.store(), &range, &frame, &mut measured).await;
    let line_count = match first {
        Ok(line_count) => line_count,
        Err(error) => {
            // The report only reads, so its unit of work is rolled back before
            // the failed pass is answered: an abend leaves no transaction open.
            uow.rollback().await?;
            return Err(error);
        }
    };
    // tracing-opentelemetry exports an unsigned field through its Debug text,
    // a JSON string; an i64 exports as a JSON integer. No report holds more
    // than i64::MAX lines, so the saturation never applies.
    span.record("line_count", i64::try_from(line_count).unwrap_or(i64::MAX));
    let body = ReportBody::new(uow, &range, frame, measured.bytes, admission);
    Ok(report_response(body, span.clone()))
}

/// The answer of a report: `body`, the second pass, streamed as one
/// `application/json` body whose `Content-Length` is the length the first
/// pass measured. Each chunk is written in `span`, the handler span, so the
/// events of the second pass carry it.
fn report_response(body: ReportBody, span: Span) -> Response {
    let length = HeaderValue::from(body.length);
    let chunks = stream::try_unfold(body, move |body| body.next_chunk().instrument(span.clone()));
    (
        [
            (CONTENT_TYPE, HeaderValue::from_static("application/json")),
            (CONTENT_LENGTH, length),
        ],
        Body::from_stream(chunks),
    )
        .into_response()
}

/// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C 1000-TRANFILE-GET-NEXT /
/// 1100-WRITE-TRANSACTION-REPORT — P-CBTRN03C-003 to P-CBTRN03C-013,
/// P-CBTRN03C-474, P-CBTRN03C-477
///
/// The first pass, a [`ReportPass`] over `range` run to its end through
/// `store`: the response JSON put into `json` piece by piece, `frame` around
/// the `CVTRA07Y` records `CBTRN03C` writes over the range in order, and the
/// number of records returned.
///
/// # Errors
///
/// 500 `Unexpected error`, the cause logged: every error of
/// [`ReportPass::advance`] and [`ReportPass::finish`].
async fn write_report<S, J>(
    store: &mut S,
    range: &ReportRange,
    frame: &ReportJson,
    json: &mut J,
) -> Result<usize, ApiError>
where
    S: TransactionReader + XrefReader + TranTypeFileReader,
    J: ReportSink,
{
    let mut pass = ReportPass::start(range, frame, json);
    while pass.advance(store, json).await? {}
    pass.finish(frame, json)
}

/// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C 1000-TRANFILE-GET-NEXT /
/// 1100-WRITE-TRANSACTION-REPORT — P-CBTRN03C-003 to P-CBTRN03C-013,
/// P-CBTRN03C-474, P-CBTRN03C-477
///
/// One pass of the report over a range, run one step at a time: the response
/// JSON put into a [`ReportSink`] piece by piece, a [`ReportJson`] frame
/// around the `CVTRA07Y` records `CBTRN03C` writes over the range in order.
/// The live transactions whose processing date lies in the range, sorted by
/// card number (`STEP05R`), are read page by page and each fed to the run,
/// then the end-of-input lines follow; each record's lines are encoded and
/// put as the run writes them, so the pass holds one page and one record's
/// lines at a time. The end of input adds the last amount a second time and
/// never writes the last card's account total, as the COBOL does
/// (P-CBTRN03C-009, P-CBTRN03C-013).
///
/// [`ReportPass::start`] puts the head, each [`ReportPass::advance`] reports
/// one record or reads one page, and [`ReportPass::finish`] puts the
/// end-of-input lines and the tail, so the caller decides how far the pass
/// runs before it takes what was put. Each page is read whole before its
/// records are reported, so the store is free for the cross-reference, type
/// and category lookups of each record.
///
/// Nothing is put that depends on the sink or on how far a step runs, so two
/// passes over one snapshot put the same bytes, whatever each sink keeps of
/// them.
struct ReportPass {
    /// The `STEP05R` selection, read page by page.
    selection: ReportSelection,
    /// The `CBTRN03C` run the records are fed to.
    run: ReportRun,
    /// The encoder of the lines as `lines` array elements.
    elements: ReportLines,
    /// The records of the page read last that are not reported yet.
    page: std::vec::IntoIter<Transaction>,
    /// The abend of a lookup: no record is reported after it, and the pages
    /// are still read to their end.
    abend: Option<ReportAbend>,
    /// The `CVTRA07Y` records put so far.
    line_count: usize,
}

impl ReportPass {
    /// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C 0550-DATEPARM-READ —
    /// P-CBTRN03C-002
    ///
    /// A pass over `range`, read [`PAGE_ROWS`] records at a time, with the
    /// head of `frame` put into `json`.
    fn start<J: ReportSink>(range: &ReportRange, frame: &ReportJson, json: &mut J) -> Self {
        json.put(frame.head());
        Self {
            selection: ReportSelection::new(range.start, range.end, PAGE_ROWS),
            run: ReportRun::new(range.start, range.end),
            elements: ReportLines::default(),
            page: Vec::new().into_iter(),
            abend: None,
            line_count: 0,
        }
    }

    /// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C 1000-TRANFILE-GET-NEXT /
    /// 1100-WRITE-TRANSACTION-REPORT — P-CBTRN03C-003 to P-CBTRN03C-008,
    /// P-CBTRN03C-474
    ///
    /// One step: the next record of the page read last, fed to the run with
    /// the lines it writes put into `json`, or, once that page is done, the
    /// next page read from `store`. `false` once the store has no page left.
    ///
    /// A lookup abend ends the reporting: the rest of the page and every later
    /// page are read but not reported, because the `STEP05R` stop on an
    /// unpinned card number, which only the whole selection reveals, wins
    /// over it ([`ReportPass::finish`]).
    ///
    /// # Errors
    ///
    /// 500 `Unexpected error`, the cause logged: the store fails, or a line
    /// cannot be written as JSON.
    async fn advance<S, J>(&mut self, store: &mut S, json: &mut J) -> Result<bool, ApiError>
    where
        S: TransactionReader + XrefReader + TranTypeFileReader,
        J: ReportSink,
    {
        if let Some(tran) = self.page.next() {
            match self.run.process(store, &tran).await {
                Ok(written) => {
                    self.line_count += written.len();
                    json.put(
                        &self
                            .elements
                            .encode(&written)
                            .map_err(|error| unserializable(&error))?,
                    );
                }
                Err(abend) => {
                    self.page = Vec::new().into_iter();
                    self.abend = Some(abend);
                }
            }
            return Ok(true);
        }
        let Some(page) = self.selection.next_page(store).await? else {
            return Ok(false);
        };
        if self.abend.is_none() {
            self.page = page.into_iter();
        }
        Ok(true)
    }

    /// TRANREPT STEP05R — P-TRANREPT-002; CBTRN03C end of input /
    /// 1110-WRITE-GRAND-TOTALS — P-CBTRN03C-005, P-CBTRN03C-009,
    /// P-CBTRN03C-013, P-CBTRN03C-477
    ///
    /// The end of a pass whose [`ReportPass::advance`] returned `false`: the
    /// end-of-input lines and the tail of `frame` put into `json`, and the
    /// number of `CVTRA07Y` records the pass put.
    ///
    /// # Errors
    ///
    /// 500 `Unexpected error`, the cause logged:
    ///
    /// - a selected record's card number holds a non-digit byte, whose
    ///   `STEP05R` sort position no audit row pins. The sort ends before
    ///   `CBTRN03C` starts, so this stop is the one answered after an abend;
    /// - a lookup abended the run (P-CBTRN03C-005);
    /// - a line cannot be written as JSON.
    fn finish<J: ReportSink>(self, frame: &ReportJson, json: &mut J) -> Result<usize, ApiError> {
        let Self {
            selection,
            run,
            mut elements,
            abend,
            mut line_count,
            ..
        } = self;
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
        json.put(
            &elements
                .encode(&written)
                .map_err(|error| unserializable(&error))?,
        );
        json.put(frame.tail());
        Ok(line_count)
    }
}

/// Where a [`ReportPass`] puts the response JSON.
trait ReportSink {
    /// Takes `bytes`, the next piece of the response JSON.
    fn put(&mut self, bytes: &[u8]);
}

/// The sink of the first pass: the length of the response JSON, without the
/// JSON.
#[derive(Debug, Default)]
struct Measured {
    /// The bytes put so far.
    bytes: usize,
}

impl ReportSink for Measured {
    fn put(&mut self, bytes: &[u8]) {
        // A length past `usize::MAX` could not be sent by the second pass
        // either; saturated, it differs from the length that pass writes.
        self.bytes = self.bytes.saturating_add(bytes.len());
    }
}

/// The sink of the second pass: the response JSON put and not yet sent,
/// taken in chunks of [`CHUNK_BYTES`].
#[derive(Debug, Default)]
struct Chunks {
    /// The bytes put and not yet taken: less than one chunk once
    /// [`Chunks::full`] has returned `None`, until the next piece is put.
    pending: Vec<u8>,
}

impl ReportSink for Chunks {
    fn put(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }
}

impl Chunks {
    /// The next chunk, exactly [`CHUNK_BYTES`] long, once the bytes put fill
    /// one. The chunk is copied out, so the pending bytes keep their
    /// allocation for the chunks after it.
    fn full(&mut self) -> Option<Bytes> {
        let chunk = self
            .pending
            .get(..CHUNK_BYTES)
            .map(Bytes::copy_from_slice)?;
        self.pending.drain(..CHUNK_BYTES);
        Some(chunk)
    }

    /// Every byte put and not yet taken, as the last chunk: no longer than
    /// [`CHUNK_BYTES`] once [`Chunks::full`] has returned `None`, and empty
    /// when nothing is left.
    fn rest(&mut self) -> Bytes {
        Bytes::from(mem::take(&mut self.pending))
    }

    /// The number of bytes put and not yet taken.
    fn held(&self) -> usize {
        self.pending.len()
    }
}

/// The response body of a report: the second pass over the snapshot the
/// first pass read, sent chunk by chunk as the server polls for them.
///
/// The body owns the unit of work and the report permit. Dropped, sent or
/// abandoned by a client gone mid-answer, it returns the permit, and a unit
/// of work it still holds rolls back as it drops.
struct ReportBody {
    /// The pass and its unit of work, until the pass has put its tail and the
    /// unit of work is rolled back.
    writing: Option<Writing>,
    /// The frame the pass puts the lines into.
    frame: ReportJson,
    /// The JSON the pass has put and the body has not sent yet.
    chunks: Chunks,
    /// The length of the JSON the first pass measured, the body's
    /// `Content-Length`.
    length: usize,
    /// The bytes of JSON sent so far.
    sent: usize,
    /// The `AppState::admit_report` permit, held only to be released when the
    /// body is dropped.
    _admission: OwnedSemaphorePermit,
}

/// The second pass of a [`ReportBody`] while it still reads its snapshot.
struct Writing {
    /// The read-only unit of work of both passes, on its one snapshot.
    uow: UnitOfWork,
    /// The second pass.
    pass: ReportPass,
}

impl ReportBody {
    /// The body of a report over `range` whose first pass, on the snapshot
    /// of `uow`, measured `length` bytes of JSON in `frame`; it holds
    /// `admission` until it is dropped.
    fn new(
        uow: UnitOfWork,
        range: &ReportRange,
        frame: ReportJson,
        length: usize,
        admission: OwnedSemaphorePermit,
    ) -> Self {
        let mut chunks = Chunks::default();
        let pass = ReportPass::start(range, &frame, &mut chunks);
        Self {
            writing: Some(Writing { uow, pass }),
            frame,
            chunks,
            length,
            sent: 0,
            _admission: admission,
        }
    }

    /// The next chunk of the body with the body that sends the rest, or
    /// `None` once every byte is sent.
    ///
    /// The pass runs until the chunk is full. Once it has put its tail, the
    /// length it wrote is checked against [`ReportBody::length`] and the unit
    /// of work is rolled back, before the last chunk is sent.
    ///
    /// # Errors
    ///
    /// [`ReportAborted`], the cause logged, which ends the body short of its
    /// `Content-Length`: every error of [`ReportPass::advance`] and
    /// [`ReportPass::finish`], a failed rollback, and a pass whose length
    /// differs from the first pass's. Over the first pass's snapshot only a
    /// lost connection or a failed rollback can occur.
    async fn next_chunk(mut self) -> Result<Option<(Bytes, Self)>, ReportAborted> {
        loop {
            if let Some(chunk) = self.chunks.full() {
                return self.send(chunk).map(Some);
            }
            let Some(Writing { mut uow, mut pass }) = self.writing.take() else {
                let rest = self.chunks.rest();
                if rest.is_empty() {
                    return Ok(None);
                }
                return self.send(rest).map(Some);
            };
            if pass.advance(&mut uow.store(), &mut self.chunks).await? {
                self.writing = Some(Writing { uow, pass });
                continue;
            }
            pass.finish(&self.frame, &mut self.chunks)?;
            let written = self.sent.saturating_add(self.chunks.held());
            if written != self.length {
                tracing::error!(
                    measured = self.length,
                    written,
                    "the two report passes over one snapshot differ in length"
                );
                return Err(ReportAborted);
            }
            uow.rollback().await.map_err(ApiError::from)?;
        }
    }

    /// `chunk` as the next chunk sent, with the body that sends the rest,
    /// unless it would take the body past its `Content-Length`.
    ///
    /// # Errors
    ///
    /// [`ReportAborted`], the cause logged, when the second pass has written
    /// more than the first pass measured.
    fn send(mut self, chunk: Bytes) -> Result<(Bytes, Self), ReportAborted> {
        let sent = self.sent.saturating_add(chunk.len());
        if sent > self.length {
            tracing::error!(
                measured = self.length,
                written = sent,
                "the second report pass outgrew the first"
            );
            return Err(ReportAborted);
        }
        self.sent = sent;
        Ok((chunk, self))
    }
}

/// The error that ends a report body short of its `Content-Length`. The
/// server then aborts the answer, so the client never takes a body cut short
/// for a whole report. The cause was logged where it was found, in the
/// handler span.
#[derive(Debug, thiserror::Error)]
#[error("the report body was aborted; its cause is logged")]
struct ReportAborted;

impl From<ApiError> for ReportAborted {
    /// A failure of the second pass: each [`ApiError`] the pass returns has
    /// logged its cause as it was built, and its 500 can no longer be
    /// answered once the body has begun.
    fn from(_: ApiError) -> Self {
        Self
    }
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
    use axum::body::Bytes;
    use axum::response::Response;
    use carddemo_domain::error::DomainError;
    use carddemo_domain::messages;
    use serde_json::Value;

    use super::{
        CHUNK_BYTES, Chunks, Measured, OUTCOME_FAILED, OUTCOME_REJECTED, OUTCOME_SUBMITTED,
        ReportSink, outcome, report_type_name,
    };
    use crate::dto::reports::ReportTypeParam;
    use crate::error::ApiError;

    #[test]
    fn the_body_sends_every_byte_put_in_full_chunks_and_one_last() {
        // The lengths of the pieces a pass puts, in order: none, empty pieces,
        // one byte either side of a chunk, a chunk, pieces that end a chunk
        // and start the next, a piece of several chunks, and as many
        // record-sized pieces as a report of some 2,000 lines puts.
        let cases: [&[usize]; 10] = [
            &[],
            &[0, 0],
            &[1],
            &[CHUNK_BYTES - 1],
            &[CHUNK_BYTES],
            &[CHUNK_BYTES + 1],
            &[CHUNK_BYTES - 1, 1, 1],
            &[CHUNK_BYTES / 2, CHUNK_BYTES, 5, 2 * CHUNK_BYTES - 5],
            &[3 * CHUNK_BYTES + 7],
            &[136; 2_000],
        ];
        for lengths in cases {
            // Synthetic bytes, so a byte out of place shows in the comparison.
            let mut source = (0..=u8::MAX).cycle();
            let pieces: Vec<Vec<u8>> = lengths
                .iter()
                .map(|&length| source.by_ref().take(length).collect())
                .collect();
            let mut measured = Measured::default();
            let mut chunks = Chunks::default();
            let mut sent: Vec<Bytes> = Vec::new();
            for piece in &pieces {
                measured.put(piece);
                chunks.put(piece);
                // The body takes every full chunk before its pass puts more.
                sent.extend(std::iter::from_fn(|| chunks.full()));
            }
            let rest = chunks.rest();
            if !rest.is_empty() {
                sent.push(rest);
            }

            let total: usize = lengths.iter().sum();
            assert_eq!(measured.bytes, total, "{lengths:?}: the measured length");
            assert_eq!(chunks.held(), 0, "{lengths:?}: nothing is left unsent");
            assert_eq!(
                sent.len(),
                total.div_ceil(CHUNK_BYTES),
                "{lengths:?}: the number of chunks"
            );
            if let Some((last, full)) = sent.split_last() {
                for chunk in full {
                    assert_eq!(
                        chunk.len(),
                        CHUNK_BYTES,
                        "{lengths:?}: a chunk before the last"
                    );
                }
                assert!(
                    (1..=CHUNK_BYTES).contains(&last.len()),
                    "{lengths:?}: the last chunk holds {} bytes",
                    last.len()
                );
            }
            assert!(
                sent.concat() == pieces.concat(),
                "{lengths:?}: the chunks are the pieces in order"
            );
        }
    }

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
