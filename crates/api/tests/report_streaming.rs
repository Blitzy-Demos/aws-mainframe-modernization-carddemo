//! A CR00 report several times larger than one chunk of its body, streamed
//! from its snapshot as the server polls the body.
//!
//! Over the wire the answer carries the length the first pass measured as
//! its `Content-Length`, with no chunked transfer coding, and its body is the
//! `serde_json` text of the whole `ReportResponse`, byte for byte. In
//! process, the same body arrives in frames of at most `CHUNK_BYTES`, every
//! one but the last a whole chunk, so the API never holds the whole answer,
//! and the report's unit of work has rolled back by the time the last frame
//! arrives. A body dropped mid-answer, as by a client gone, gives back its
//! report permit and its connection; a body whose connection is lost
//! mid-answer ends in an error short of its `Content-Length`, never as a
//! report that looks whole, and gives back its permit too.
//!
//! Docker must be running; the harness conventions are those of `common`.

pub mod common;

use std::time::{Duration, Instant};

use axum::body::{Body, BodyDataStream};
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING};
use axum::http::{HeaderMap, Request, StatusCode};
use carddemo_api::dto::reports::ReportResponse;
use carddemo_api::handlers::reports::CHUNK_BYTES;
use carddemo_domain::messages;
use futures_util::StreamExt;
use tower::ServiceExt;

use common::TestApp;

/// The processing clock, after the report range.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The live transactions [`LIVE_ROWS`] writes, all in the report range: three
/// pages of the selection, and a body of more than six chunks.
const ROWS: usize = 3_000;

/// [`ROWS`] live purchases on card `9680294154603697` of account 1, on
/// 2022-06-15, with type `01` and category `0001`, which the seeded
/// `card_xref`, `trantype_file` and `trancatg_file` hold. The seeded live
/// master is empty (E-07).
const LIVE_ROWS: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     SELECT lpad(n::text, 16, '0'), '01', 1, 'POS TERM', 'Report streaming row', \
      (10 + n)::numeric(11, 2), n, 'Report merchant', 'Report city', '12345', \
      '9680294154603697', '2022-06-15 12:00:00.000000', '2022-06-15 12:00:00.000000' \
     FROM generate_series(1, 3000) AS n";

/// The custom report of June 2022, confirmed: every row of [`LIVE_ROWS`].
const REPORT: &str =
    "/reports/transactions?report_type=custom&start_date=2022-06-01&end_date=2022-06-30&confirm=Y";

/// The sessions between two statements of an open transaction: the report's,
/// while its body is neither sent nor dropped, and no other.
const OPEN_TRANSACTIONS: &str =
    "SELECT count(*)::text FROM pg_stat_activity WHERE state = 'idle in transaction'";

/// Ends the sessions [`OPEN_TRANSACTIONS`] counts, the report's, waiting up
/// to 10 seconds for each to end: one row, `true`, once it has.
const END_REPORT_SESSION: &str = "SELECT pg_terminate_backend(pid, 10000)::text \
     FROM pg_stat_activity WHERE state = 'idle in transaction'";

/// How long the rollback of a dropped body may take to reach the server.
const ROLLBACK_DEADLINE: Duration = Duration::from_secs(30);

/// The pause between two reads of [`OPEN_TRANSACTIONS`].
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[test]
fn a_report_of_several_chunks_is_streamed_whole_with_its_length() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROWS).await;
        let token = app.user_token().await;

        let response = app
            .client()
            .get(app.url(REPORT))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap_or_else(|error| panic!("the report request failed: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers().clone();
        let wire = response
            .bytes()
            .await
            .unwrap_or_else(|error| panic!("reading the report body failed: {error}"));
        assert_eq!(headers[CONTENT_TYPE], "application/json");
        assert!(
            headers.get(TRANSFER_ENCODING).is_none(),
            "a body of known length has no transfer coding: {headers:?}"
        );
        let length = content_length(&headers);
        assert_eq!(
            wire.len(),
            length,
            "the body is as long as its Content-Length"
        );
        assert!(
            length > 6 * CHUNK_BYTES,
            "{length} bytes are not several chunks of {CHUNK_BYTES}"
        );
        let report = whole_report(&wire);
        let text = serde_json::to_vec(&report).expect("a report response serializes");
        assert!(
            text == wire,
            "the body is not the serde_json text of its ReportResponse"
        );

        let response = in_process(&app, &token).await;
        assert_eq!(content_length(response.headers()), length);
        let mut frames = response.into_body().into_data_stream();
        let mut sizes = Vec::new();
        let mut body = Vec::with_capacity(length);
        while let Some(frame) = frames.next().await {
            let frame = frame.unwrap_or_else(|error| panic!("a frame of the body failed: {error}"));
            sizes.push(frame.len());
            body.extend_from_slice(&frame);
        }
        assert_eq!(
            open_transactions(&app).await,
            0,
            "the unit of work rolls back before the last frame is sent"
        );
        let Some((last, whole)) = sizes.split_last() else {
            panic!("the body sent no frame");
        };
        assert!(
            !whole.is_empty(),
            "the body came as one frame of {last} bytes"
        );
        assert!(
            whole.iter().all(|&size| size == CHUNK_BYTES),
            "a frame before the last is not one chunk of {CHUNK_BYTES}: {sizes:?}"
        );
        assert!(
            (1..=CHUNK_BYTES).contains(last),
            "the last frame holds {last} bytes"
        );
        assert!(
            body == wire,
            "the body in process differs from the body sent"
        );
    });
}

#[test]
fn a_report_body_dropped_mid_answer_gives_back_its_permit_and_connection() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROWS).await;
        let token = app.user_token().await;

        let mut frames = in_process(&app, &token)
            .await
            .into_body()
            .into_data_stream();
        first_frame(&mut frames).await;
        assert_eq!(
            open_transactions(&app).await,
            1,
            "the unsent body holds its snapshot"
        );
        drop(frames);
        let until = Instant::now() + ROLLBACK_DEADLINE;
        while open_transactions(&app).await != 0 {
            assert!(
                Instant::now() < until,
                "the dropped body's unit of work is still open after {ROLLBACK_DEADLINE:?}"
            );
            pause(POLL_INTERVAL).await;
        }

        // The testkit pool admits one report at once, so this report would
        // wait for the permit until the harness timeout had the dropped body
        // kept it.
        let (status, _, raw) = app.get(REPORT, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the report after the dropped body: {raw}"
        );
        whole_report(raw.as_bytes());
    });
}

#[test]
fn a_report_whose_connection_is_lost_mid_answer_ends_short_of_its_length() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROWS).await;
        let token = app.user_token().await;

        let response = in_process(&app, &token).await;
        let length = content_length(response.headers());
        let mut frames = response.into_body().into_data_stream();
        let mut received = first_frame(&mut frames).await;
        let ended = app.stack().fetch_text_rows(END_REPORT_SESSION).await;
        assert_eq!(
            ended,
            [[Some("true".to_owned())]],
            "the report's session, and only it, is ended"
        );
        let mut failed = false;
        while let Some(frame) = frames.next().await {
            let Ok(frame) = frame else {
                failed = true;
                break;
            };
            received += frame.len();
        }
        assert!(
            failed,
            "the body ends without an error after {received} bytes"
        );
        assert!(
            received < length,
            "{received} bytes of {length} were sent before the error"
        );

        let (status, _, raw) = app.get(REPORT, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the report after the lost one: {raw}"
        );
        whole_report(raw.as_bytes());
    });
}

/// The report of [`REPORT`] sent in process with `token` as the bearer token,
/// answered 200 with the report's JSON type, before any frame of its body is
/// read.
async fn in_process(app: &TestApp, token: &str) -> axum::response::Response {
    let request = Request::get(REPORT)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("a valid request");
    let Ok(response) = app.router().oneshot(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    response
}

/// Reads the first frame of `frames`, a report body larger than one chunk:
/// exactly one chunk long. Returns its length.
async fn first_frame(frames: &mut BodyDataStream) -> usize {
    let first = frames
        .next()
        .await
        .expect("the body sends a first frame")
        .unwrap_or_else(|error| panic!("the first frame of the body failed: {error}"));
    assert_eq!(first.len(), CHUNK_BYTES, "the first frame is one chunk");
    first.len()
}

/// The `Content-Length` of `headers`, which a report answer always carries.
fn content_length(headers: &HeaderMap) -> usize {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("no numeric Content-Length: {headers:?}"))
}

/// Asserts that `body` is the 200 custom report of the [`ROWS`] live rows:
/// its name, its submission text, and more lines than rows, the headers and
/// totals around one detail line per row. Returns the report.
fn whole_report(body: &[u8]) -> ReportResponse {
    let report: ReportResponse = serde_json::from_slice(body)
        .unwrap_or_else(|error| panic!("the body is no ReportResponse: {error}"));
    assert_eq!(report.report_name, messages::REPORT_NAME_CUSTOM);
    assert_eq!(
        report.message,
        messages::report_submitted_message(messages::REPORT_NAME_CUSTOM)
    );
    assert!(
        report.lines.len() > ROWS,
        "{} lines for {ROWS} rows",
        report.lines.len()
    );
    report
}

/// The number of sessions [`OPEN_TRANSACTIONS`] counts now.
async fn open_transactions(app: &TestApp) -> u64 {
    let rows = app.stack().fetch_text_rows(OPEN_TRANSACTIONS).await;
    let [row] = rows.as_slice() else {
        panic!("one count row expected, got {rows:?}");
    };
    row.first()
        .and_then(Option::as_deref)
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("a count expected, got {row:?}"))
}

/// Sleeps for `period` on a blocking thread, leaving the runtime's workers to
/// the server and the pool.
async fn pause(period: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(period))
        .await
        .unwrap_or_else(|error| panic!("the pause task failed: {error}"));
}
